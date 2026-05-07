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
                        if let Ok(req) = serde_json::from_str::<xau_scalper_server::EvalRequest>(&text) {
                            let service = crate::services::trading::TradingService::new(state.clone());
                            if let Err(e) = service.process_eval_request(req).await {
                                tracing::error!("Failed to process WS data ingestion: {:?}", e);
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
                        // Filter: Only send Signals or AI Updates. Skip Ticks.
                        if msg.contains("\"type\":\"signal\"") || msg.contains("\"type\":\"ai_update\"") || msg.contains("\"signalId\"") {
                            if socket.send(Message::Text(msg)).await.is_err() {
                                break;
                            }
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