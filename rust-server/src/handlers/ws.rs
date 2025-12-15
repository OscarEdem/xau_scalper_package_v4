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
    tracing::info!("New WebSocket client connected. Starting live tick stream... total_clients={}", new_total);

    let mut rx = state.tick_tx.subscribe();

    // Keepalive ping interval to avoid idle connection closures by proxies
    let mut keepalive = tokio::time::interval(tokio::time::Duration::from_secs(30));

    loop {
        tokio::select! {
            biased;
            // Prefer processing ticks when they arrive
            recv = rx.recv() => {
                match recv {
                    Ok(msg) => {
                        if socket.send(Message::Text(msg)).await.is_err() {
                            let remaining = state.inner.ws_clients.fetch_sub(1, Ordering::SeqCst) - 1;
                            tracing::info!("WebSocket client disconnected while sending tick. remaining_clients={}", remaining);
                            state.inner.metrics.active_ws_clients.set(remaining as f64);
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                        tracing::warn!("WebSocket subscriber lagged; skipped {} messages", skipped);
                        continue;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        break;
                    }
                }
            }
            _ = keepalive.tick() => {
                if socket.send(Message::Ping(Vec::new())).await.is_err() {
                    let remaining = state.inner.ws_clients.fetch_sub(1, Ordering::SeqCst) - 1;
                    tracing::info!("WebSocket client disconnected during keepalive ping. remaining_clients={}", remaining);
                    state.inner.metrics.active_ws_clients.set(remaining as f64);
                    break;
                }
            }
        }
    }
}