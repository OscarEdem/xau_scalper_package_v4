use axum::{
    extract::{
        ws::{Message, WebSocket},
        State, WebSocketUpgrade,
    },
};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use crate::state::ApplicationStateWithTicks;

/// Axum handler for WebSocket connections.
pub async fn websocket_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<ApplicationStateWithTicks>>,
) -> impl axum::response::IntoResponse {
    ws.on_upgrade(|socket| websocket_stream(socket, state))
}

/// The actual WebSocket logic once a connection is upgraded.
async fn websocket_stream(mut socket: WebSocket, state: Arc<ApplicationStateWithTicks>) {
    // Increment the active WS client counter and log the new total
    let prev = state.inner.ws_clients.fetch_add(1, Ordering::SeqCst);
    let new_total = prev + 1;
    state.inner.metrics.active_ws_clients.set(new_total as f64);
    tracing::info!("New WebSocket client connected. total_clients={}", new_total);

    let mut rx = state.tick_tx.subscribe();
    let mut keepalive = tokio::time::interval(tokio::time::Duration::from_secs(30));

    loop {
        tokio::select! {
            biased;
            // 1. Handle incoming data from Python (Data Ingestion)
            Some(result) = socket.recv() => {
                match result {
                    Ok(Message::Text(text)) => {
                        // Attempt to parse as market data
                        if let Ok(req) = serde_json::from_str::<crate::EvalRequest>( &text) {
                            let symbol = req.symbol.to_string();
                            
                            // Process via Trading Session
                            let mut sessions = state.inner.sessions.write().unwrap();
                            let session = sessions.entry(symbol.clone()).or_insert_with(|| {
                                tracing::info!(symbol = %symbol, "Creating new trading session via WebSocket data ingestion.");
                                crate::session::TradingSession::new(
                                    symbol.clone(),
                                    state.inner.settings.read().unwrap().scalp.filter_scalp_by_swing,
                                    1000,
                                    Some(state.tick_tx.clone()),
                                    state.inner.db.clone(),
                                    Some(state.inner.metrics.clone()),
                                )
                            });

                            let signals = session.on_data(req, &state.inner.predictor_cache, &state.inner.settings);
                            
                            // The session.on_data already broadcasts signals to WS if they are valid.
                            // But we return them here for logging or additional processing if needed.
                            if !signals.is_empty() {
                                tracing::info!(symbol = %symbol, count = signals.len(), "Generated signals from WebSocket data ingestion.");
                            }
                        }
                    }
                    Ok(Message::Close(_)) | Err(_) => {
                        let remaining = state.inner.ws_clients.fetch_sub(1, Ordering::SeqCst) - 1;
                        state.inner.metrics.active_ws_clients.set(remaining as f64);
                        tracing::info!("WebSocket client disconnected. remaining_clients={}", remaining);
                        break;
                    }
                    _ => {}
                }
            }

            // 2. Outbound Tick/Signal Stream (Push to Clients)
            recv = rx.recv() => {
                match recv {
                    Ok(msg) => {
                        if socket.send(Message::Text(msg)).await.is_err() {
                            // Decrement happens in the recv block above if connection fails
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                        tracing::warn!("WebSocket subscriber lagged; skipped {} messages", skipped);
                        continue;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }

            // 3. Keepalive
            _ = keepalive.tick() => {
                if socket.send(Message::Ping(Vec::new())).await.is_err() {
                    break;
                }
            }
        }
    }
}