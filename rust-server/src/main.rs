// v4 XAU/USD scalper server
use axum::{
    extract::{
        ws::{Message, WebSocket},
        Path, Query, State, WebSocketUpgrade,
    },
    http::StatusCode,
    routing::get,
    Json, Router,
};
use axum::routing::post;
use chrono::{NaiveDate, Utc};
use expo_push_notification_client::{Expo, ExpoClientOptions, ExpoPushMessage};
use tokio::fs; // Use tokio's async fs module
use std::collections::{BTreeSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::broadcast;
use tracing_subscriber::EnvFilter;
use utoipa::{OpenApi};
use utoipa_swagger_ui::SwaggerUi;
pub use xau_scalper_server::{
    ArrivalConfirmation, EvalRequest, EvalResponse, SessionManager,
    ExecutionConfirmation, HistoryParams, HistoryResponse, HistoryStats, TradeLog, PriceLevel, VwapBands
};

const MAX_TICK_HISTORY: usize = 10000;
const HISTORY_PAGE_SIZE: usize = 1000;
const PUSH_TOKENS_FILE: &str = "push_tokens.json";

#[derive(serde::Serialize, utoipa::ToSchema, Clone)]
#[serde(rename_all = "camelCase")]
struct HistoricalSignal {
    #[serde(flatten)]
    signal: ActiveSignal,
    created_at: i64, // Unix timestamp
}

/// A more flexible state for the whole application, including the new SessionManager.
#[derive(Clone)]
struct ApplicationState {
    session_manager: SessionManager,
    predictor_cache: xau_scalper_server::engines::predictor_cache::PredictorCache,
    signal_history: Arc<Mutex<VecDeque<HistoricalSignal>>>,
    tick_history: Arc<Mutex<VecDeque<String>>>, // NEW: Cache for recent ticks
    push_tokens: Arc<Mutex<BTreeSet<String>>>,
    ws_clients: Arc<AtomicUsize>,
}

#[derive(serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
struct LatestSignalsResponse {
    scalp_signal: Option<EvalResponse>,
    swing_signal: Option<EvalResponse>,
}

#[derive(serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
struct LatestSignalsForSymbol {
    symbol: String,
    scalp_signal: Option<EvalResponse>,
    swing_signal: Option<EvalResponse>,
}

#[derive(serde::Serialize, utoipa::ToSchema, Clone)]
#[serde(rename_all = "camelCase")]
struct ActiveSignal {
    symbol: String,
    #[serde(flatten)]
    signal: EvalResponse,
}

/// Request body for saving a push notification token.
#[derive(serde::Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
struct SavePushTokenRequest {
    token: String,
}

/// Request body for updating a session's configuration.
#[derive(serde::Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
struct UpdateSessionConfigRequest {
    /// The name of the predictor model to use (e.g., "gbm", "heston", "lstm").
    predictor_model: String,
}

/// Represents a single market tick.
#[derive(serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
struct TickData {
    symbol: String,
    bid: f64,
    ask: f64,
    timestamp: i64,
}

/// Wrapper for sending historical ticks to a new WebSocket client.
#[derive(serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
struct HistoryPayload {
    r#type: String,
    payload: VecDeque<String>,
}

/// Incoming message from a WebSocket client to request historical data.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct WsClientRequest {
    r#type: String,
    /// The page number to retrieve (0-indexed).
    page: usize,
}

#[allow(dead_code)] // This is used by utoipa macro to generate OpenAPI spec
#[derive(OpenApi)]
#[openapi(
    paths(
        health_check_handler,
        process_data_handler,
        documented_tick_ingest_handler,
        get_latest_signals_handler,
        get_all_latest_signals_handler,
        get_signals_handler,
        update_session_config_handler,
        confirm_arrival_handler,
        confirm_execution_handler,
        save_push_token_handler,
        log_trade_handler,
        get_logs_handler,
        clear_history_handler
    ),
    components(
        schemas(EvalRequest, EvalResponse, TradeLog, HistoryResponse, HistoryStats, PriceLevel, VwapBands, ArrivalConfirmation, ExecutionConfirmation, HistoryParams, LatestSignalsResponse, LatestSignalsForSymbol, ActiveSignal, HistoricalSignal, SavePushTokenRequest, UpdateSessionConfigRequest, TickData, HistoryPayload)
    ),
    info(
        description = "This API provides endpoints for the XAU/USD Scalping and Swing Trading Engines. It processes market data, generates trading signals, and provides a real-time data stream via WebSockets. The WebSocket endpoint at `/ws` streams live tick data as JSON strings. On connection, it first sends a `HistoryPayload` object containing recent ticks."
    ),
    tags((name = "Trading Signal API", description = "Endpoints for signal generation, data processing, and session management."))
)]
struct ApiDoc;

#[tokio::main]
async fn main() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    // --- NEW: Load push tokens from file on startup ---
    let initial_push_tokens = match fs::read_to_string(PUSH_TOKENS_FILE).await {
        Ok(content) => {
            let tokens: BTreeSet<String> = serde_json::from_str(&content).unwrap_or_default();
            tracing::info!("Loaded {} push notification tokens from {}", tokens.len(), PUSH_TOKENS_FILE);
            tokens
        }
        Err(_) => {
            tracing::info!("No '{}' file found. Starting with an empty set of push tokens.", PUSH_TOKENS_FILE);
            BTreeSet::new()
        }
    };

    // Initialize the shared state
    let shared_state = ApplicationState {
        session_manager: SessionManager::default(),
        predictor_cache: xau_scalper_server::engines::predictor_cache::PredictorCache::default(),
        signal_history: Arc::new(Mutex::new(VecDeque::new())),
        tick_history: Arc::new(Mutex::new(VecDeque::with_capacity(MAX_TICK_HISTORY))), // NEW: Initialize cache
        // Use the tokens loaded from the file
        push_tokens: Arc::new(Mutex::new(initial_push_tokens)),
        ws_clients: Arc::new(AtomicUsize::new(0)),
    };

    // --- Background task for stale signal cleanup (invalidates signals in live sessions) ---
    let cleanup_state = shared_state.clone();
    tokio::spawn(async move {
        loop {
            // Check every minute
            tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;
            let now = Utc::now().timestamp();
            const STALE_THRESHOLD_SECONDS: i64 = 300; // 5 minutes

            if let Ok(mut sessions) = cleanup_state.session_manager.sessions.lock() {
                let stale_symbols: Vec<String> = sessions.iter()
                    .filter(|(_, session)| (now - session.get_last_eval_timestamp()) > STALE_THRESHOLD_SECONDS)
                    .map(|(symbol, _)| symbol.clone())
                    .collect();

                for symbol in stale_symbols {
                    if let Some(session) = sessions.get_mut(&symbol) {
                        session.invalidate_signals();
                        tracing::info!("Invalidated stale signals for symbol: {}", symbol);
                    }
                }
            }
        }
    });

    // --- Background task for 12-hour signal history cleanup ---
    let history_cleanup_state = shared_state.clone();
    tokio::spawn(async move {
        loop {
            // Check every hour
            tokio::time::sleep(tokio::time::Duration::from_secs(3600)).await;
            let now = Utc::now().timestamp();
            const TWELVE_HOURS_IN_SECONDS: i64 = 12 * 60 * 60;

            if let Ok(mut history) = history_cleanup_state.signal_history.lock() {
                let original_len = history.len();
                history.retain(|hs| (now - hs.created_at) < TWELVE_HOURS_IN_SECONDS);
                let removed_count = original_len - history.len();
                if removed_count > 0 {
                    tracing::info!("Removed {} signals from history older than 12 hours.", removed_count);
                }
            }
        }
    });

    // --- Phase 2: Real-time Data Gateway ---

    // 1. Create a channel to broadcast live market data from MT5 to WebSocket clients.
    let (tick_tx, _) = broadcast::channel::<String>(100);

    // Add the tick_tx channel to the application state so the handler can access it.
    let app_state_with_ticks = Arc::new(shared_state.with_ticks(tick_tx));

    let app = Router::new()
        .merge(SwaggerUi::new("/docs").url("/api-docs/openapi.json", ApiDoc::openapi()))
        .route("/ws", get(websocket_handler))
        .route("/health", get(health_check_handler))
        .route("/data", post(process_data_handler)) // For main analysis
        .route("/ticks", post(tick_ingest_handler)) // NEW: For live ticks
        .route("/signals/:symbol", get(get_latest_signals_handler))
        .route("/signals/latest", get(get_all_latest_signals_handler))
        .route("/signals", get(get_signals_handler))
        // --- Add new routes for logging ---
        .route("/sessions/:symbol/config", post(update_session_config_handler))
        .route("/confirm_arrival", post(confirm_arrival_handler))
        .route("/confirm_execution", post(confirm_execution_handler))
        .route("/save-push-token", post(save_push_token_handler))
        .route("/log_trade", post(log_trade_handler))
        // --- The history endpoint is now more powerful ---
        .route("/history", get(get_logs_handler).delete(clear_history_handler))
        .route("/models/loaded", get(get_loaded_models_handler))
        // Provide the state (including the tick channel) to the handlers
        .with_state(app_state_with_ticks);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    tracing::info!("listening on {}", listener.local_addr().unwrap());
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .unwrap();
}

/// Axum handler for WebSocket connections.
async fn websocket_handler(
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
    tracing::info!("New WebSocket client connected. Sending price history... total_clients={}", new_total);

    // --- NEW: Send recent tick history on connect ---
    // We only send the most recent `HISTORY_PAGE_SIZE` ticks initially.
    let initial_history_vec = {
        let history = state.inner.tick_history.lock().unwrap_or_else(|e| {
            tracing::error!("Tick history mutex poisoned in websocket_stream! Recovering. Error: {}", e);
            e.into_inner()
        });

        let start_index = history.len().saturating_sub(HISTORY_PAGE_SIZE);

        // Parse each stored JSON string into a serde_json::Value. If parsing fails, skip that entry.
        history
            .range(start_index..)
            .filter_map(|s| serde_json::from_str::<serde_json::Value>(s).ok())
            .collect::<Vec<serde_json::Value>>()
    };

    if !initial_history_vec.is_empty() {
        // Send history as an array of JSON objects (no double-parse required on client)
        let history_payload = serde_json::json!({
            "type": "history_page",
            "payload": initial_history_vec
        });

        if let Ok(json_string) = serde_json::to_string(&history_payload) {
            if socket.send(Message::Text(json_string)).await.is_err() {
                tracing::info!("Failed to send initial price history to WebSocket client; it may have disconnected immediately.");
                // Decrement counter since the client is gone
                let remaining = state.inner.ws_clients.fetch_sub(1, Ordering::SeqCst) - 1;
                tracing::info!("WebSocket client disconnected during initial send. remaining_clients={}", remaining);
                return; // Client is gone, no need to proceed.
            }
        }
    }
    // --- END NEW ---

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
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                        tracing::warn!("WebSocket subscriber lagged; skipped {} messages", skipped);
                        // continue listening for new messages
                        continue;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        tracing::info!("Tick broadcast channel closed; ending websocket stream.");
                        break;
                    }
                }
            }
            _ = keepalive.tick() => {
                // send a ping frame to keep the connection alive through proxies
                if socket.send(Message::Ping(Vec::new())).await.is_err() {
                    let remaining = state.inner.ws_clients.fetch_sub(1, Ordering::SeqCst) - 1;
                    tracing::info!("WebSocket client disconnected during keepalive ping. remaining_clients={}", remaining);
                    break;
                }
            }
            // --- NEW: Handle incoming messages from the client ---
            Some(msg) = socket.recv() => {
                if let Ok(Message::Text(text)) = msg {
                    // Attempt to parse the client's request for a history page
                    if let Ok(request) = serde_json::from_str::<WsClientRequest>(&text) {
                        if request.r#type == "get_history" {
                            tracing::info!("Client requested history page: {}", request.page);

                            let history_page = {
                                let history = state.inner.tick_history.lock().unwrap_or_else(|e| e.into_inner());
                                let total_items = history.len();

                                // Calculate the start and end indices for the requested page.
                                // Pages are requested from newest to oldest, so page 0 is the latest data.
                                let end = total_items.saturating_sub(request.page * HISTORY_PAGE_SIZE);
                                let start = end.saturating_sub(HISTORY_PAGE_SIZE);

                                history
                                    .range(start..end)
                                    .filter_map(|s| serde_json::from_str::<serde_json::Value>(s).ok())
                                    .collect::<Vec<serde_json::Value>>()
                            };

                            if !history_page.is_empty() {
                                let payload = serde_json::json!({
                                    "type": "history_page",
                                    "page": request.page,
                                    "payload": history_page,
                                });
                                if let Ok(json_string) = serde_json::to_string(&payload) {
                                    if socket.send(Message::Text(json_string)).await.is_err() {
                                        let remaining = state.inner.ws_clients.fetch_sub(1, Ordering::SeqCst) - 1;
                                        tracing::info!("WebSocket client disconnected while sending history page. remaining_clients={}", remaining);
                                        break;
                                    }
                                }
                            }
                        }
                    }
                } else if let Ok(Message::Close(_)) = msg {
                    let remaining = state.inner.ws_clients.fetch_sub(1, Ordering::SeqCst) - 1;
                    tracing::info!("WebSocket client sent close frame. remaining_clients={}", remaining);
                    break;
                }
            }
        }
    }
}

#[utoipa::path(
    get,
    path = "/health",
    responses((status = 200, description = "Server is running"))
)]
/// Simple health check endpoint.
async fn health_check_handler() -> &'static str { "OK" }

/// New handler to ingest a single tick via HTTP POST and broadcast it.
async fn tick_ingest_handler(
    State(state): State<Arc<ApplicationStateWithTicks>>,
    tick_json: String, // Axum can receive the raw body as a String
) -> StatusCode {
    // --- NEW: Add the incoming tick to our history cache ---
    if !tick_json.is_empty() {
        let mut history = state.inner.tick_history.lock().unwrap_or_else(|e| {
            tracing::error!("Tick history mutex poisoned in tick_ingest_handler! Recovering. Error: {}", e);
            e.into_inner()
        });
        history.push_back(tick_json.clone());
        if history.len() > MAX_TICK_HISTORY {
            history.pop_front();
        }
    }
    if !tick_json.is_empty() {
        // Send this JSON string to all connected WebSocket clients
        if state.tick_tx.send(tick_json).is_err() {
            // This is not a client error, just a server state observation.
            // It's fine to log it, but we still return 200 OK to the EA.
            tracing::warn!("Tick received, but no active WebSocket clients to broadcast to.");
        }
    }
    StatusCode::OK
}

/// This is a trick for utoipa to document the raw JSON body of `tick_ingest_handler`.
#[utoipa::path(
    post,
    path = "/ticks",
    request_body(content = TickData, description = "A single market tick in JSON format", content_type = "application/json"),
    responses(
        (status = 200, description = "Tick received and broadcasted successfully"),
    ),
    tag = "Trading Signal API"
)]
#[allow(dead_code)]
async fn documented_tick_ingest_handler() {}

// We need to add the broadcast sender to our application state
#[derive(Clone)]
struct ApplicationStateWithTicks {
    inner: ApplicationState,
    tick_tx: broadcast::Sender<String>,
}

/// Awaits a shutdown signal (e.g., Ctrl+C) for graceful server shutdown.
async fn shutdown_signal() {
    tokio::signal::ctrl_c()
        .await
        .expect("failed to install CTRL+C signal handler");
    tracing::info!("Received shutdown signal, shutting down gracefully.");
}

impl ApplicationState {
    fn with_ticks(self, tick_tx: broadcast::Sender<String>) -> ApplicationStateWithTicks {
        ApplicationStateWithTicks { inner: self, tick_tx }
    }
}


#[utoipa::path(
    post,
    path = "/save-push-token",
    request_body = SavePushTokenRequest,
    responses(
        (status = 200, description = "Token saved successfully"),
        (status = 400, description = "Invalid token provided")
    )
)]
/// Handler to receive and store a push notification token from a client app.
async fn save_push_token_handler(
    State(state): State<Arc<ApplicationStateWithTicks>>,
    Json(body): Json<SavePushTokenRequest>,
) -> (StatusCode, Json<&'static str>) {
    if body.token.is_empty() || !body.token.starts_with("ExponentPushToken[") {
        return (StatusCode::BAD_REQUEST, Json("Invalid push token format"));
    }

    let mut tokens = state.inner.push_tokens.lock().unwrap_or_else(|e| {
        tracing::error!("Push tokens mutex poisoned; recovering. Error: {}", e);
        e.into_inner()
    });
    let inserted = tokens.insert(body.token);

    // --- NEW: Persist tokens to file if a new one was added ---
    if inserted {
        tracing::info!("Saved new push token. Total tokens: {}", tokens.len());
        // Clone the tokens to write them to the file without holding the lock.
        let tokens_to_save = tokens.clone();
        // In a separate task to avoid blocking the response.
        tokio::spawn(async move {
            if let Ok(json) = serde_json::to_string(&tokens_to_save) {
                if let Err(e) = fs::write(PUSH_TOKENS_FILE, json).await {
                    tracing::error!("Failed to write push tokens to file: {}", e);
                }
            }
        });
    }

    (StatusCode::OK, Json("Token processed"))
}

#[utoipa::path(
    get,
    path = "/signals",
    responses(
        (status = 200, description = "Returns a log of all signals generated in the last 12 hours", body = Vec<HistoricalSignal>)
    )
)]
/// Handler to return a log of all signals generated in the last 12 hours.
async fn get_signals_handler(State(state): State<Arc<ApplicationStateWithTicks>>) -> Json<Vec<HistoricalSignal>> {
    let history = state.inner.signal_history.lock().unwrap_or_else(|e| {
        tracing::error!("Signal history mutex poisoned! Recovering. Error: {}", e);
        e.into_inner()
    });
    // Return a clone of the current history
    Json(history.iter().cloned().collect())
}

/// This handler now acts as the primary data ingress point.
/// It finds or creates a session for the symbol in the request,
/// and passes the data to the session for processing.
#[utoipa::path(
    post,
    path = "/data",
    request_body = EvalRequest,
    responses(
        (status = 200, description = "Data processed successfully")
    )
)]
#[axum::debug_handler]
async fn process_data_handler(
    State(state): State<Arc<ApplicationStateWithTicks>>,
    Json(req): Json<EvalRequest>,
) -> (StatusCode, Json<&'static str>) {
    tracing::info!(
        symbol = %req.symbol,
        timeframe = %req.timeframe,
        closes_count = req.closes.len(),
        "Received data processing request"
    );

    // Also broadcast this incoming data as a tick-like message to any connected WebSocket clients
    // and store it in the tick history so newly-connected clients receive recent data.
    // We construct a simple JSON payload that the WS clients expect (stringified JSON).
    let tick_payload = serde_json::json!({
        "type": "tick",
        "symbol": req.symbol.clone(),
        "currentPrice": req.current_price,
        "spreadPoints": req.spread_points.unwrap_or(0.0),
        "lastM1Timestamp": req.last_m1_timestamp,
    })
    .to_string();

    // Push into the in-memory tick history (bounded) and broadcast to clients. If the mutex
    // is poisoned we recover and continue processing but log the error.
    {
        let mut history = state.inner.tick_history.lock().unwrap_or_else(|e| {
            tracing::error!("Tick history mutex poisoned in process_data_handler: {}", e);
            e.into_inner()
        });
        history.push_back(tick_payload.clone());
        if history.len() > MAX_TICK_HISTORY {
            history.pop_front();
        }
    }
    // Broadcast to any connected WebSocket clients. If there are none, this returns Err and we log.
    if state.tick_tx.send(tick_payload).is_err() {
        tracing::debug!("No active WebSocket clients to broadcast tick to.");
    }

    // For now, we assume a global config for filtering. This could also be part of the request.
    let filter_scalp_by_swing = true;

    // --- NEW: Define signals to send outside the lock scope ---
    let mut signals_to_send = Vec::new();

    // Scope the mutex lock to release it before the .await call
    {
        // Lock the sessions map, get the specific session for the symbol, and process data.
        // The `get_or_create_session` handles the logic of creating a new session if it's the first time
        // we see this symbol. The lock is held for the duration of the data processing to ensure consistency.
        let mut sessions = state
            .inner.session_manager.sessions.lock().unwrap_or_else(|e| {
                tracing::error!("Session manager mutex poisoned! Recovering. Error: {}", e);
                e.into_inner()
            });
        
        let session = sessions.entry(req.symbol.clone()).or_insert_with(|| {
            tracing::info!("Creating new trading session for symbol: {}", req.symbol);
            xau_scalper_server::TradingSession::new(req.symbol.clone(), filter_scalp_by_swing)
        });

        // --- Get old signals BEFORE processing new data ---
        let (old_scalp_sig, old_swing_sig) = session.get_latest_signals();

        // Process data, which updates the signals within the session
        session.on_data(&req, &state.inner.predictor_cache);

        // Now, get the new signals and add them to history
        let (scalp_sig_opt, swing_sig_opt) = session.get_latest_signals();
        let mut history = state.inner.signal_history.lock().unwrap_or_else(|e| e.into_inner());

        let now = Utc::now().timestamp();

        if let Some(swing_sig) = swing_sig_opt {
            // Check if it's a new, actionable signal
            if swing_sig.entry_type != "none" && old_swing_sig.as_ref().map_or(true, |old| old.signal_id != swing_sig.signal_id) {
                signals_to_send.push((swing_sig.clone(), req.symbol.clone()));
            }
            // Store in history regardless
            history.push_front(HistoricalSignal {
                signal: ActiveSignal {
                    symbol: req.symbol.clone(),
                    signal: swing_sig,
                },
                created_at: now,
            });
        }
        if let Some(scalp_sig) = scalp_sig_opt {
            // Check if it's a new, actionable signal
            if scalp_sig.entry_type != "none" && old_scalp_sig.as_ref().map_or(true, |old| old.signal_id != scalp_sig.signal_id) {
                signals_to_send.push((scalp_sig.clone(), req.symbol.clone()));
            }
            // Store in history regardless
            history.push_front(HistoricalSignal {
                signal: ActiveSignal {
                    symbol: req.symbol.clone(),
                    signal: scalp_sig,
                },
                created_at: now,
            });
        }
    } // --- The `sessions` lock is dropped here ---

    // --- NEW: Send notifications AFTER releasing the lock ---
    for (signal, symbol) in signals_to_send {
        send_push_notification(state.inner.push_tokens.clone(), &signal, &symbol).await;
    }

    (StatusCode::OK, Json("Data processed"))
}

/// Sends a push notification for a new signal to all registered devices.
async fn send_push_notification(
    tokens_arc: Arc<Mutex<BTreeSet<String>>>,
    signal: &EvalResponse,
    symbol: &str,
) {
    let tokens = tokens_arc.lock().unwrap_or_else(|e| {
        tracing::error!("Push tokens mutex poisoned in send_push_notification; recovering. Error: {}", e);
        e.into_inner()
    }).clone();
    if tokens.is_empty() {
        tracing::warn!("Skipping push notification for signal ID: {}. No push tokens are registered.", signal.signal_id);
        return;
    }

    tracing::info!("Sending push notification for signal ID: {}", signal.signal_id);

    let messages: Vec<ExpoPushMessage> = tokens
        .into_iter()
        .map(|token| {
            let data = serde_json::json!({ "signalId": signal.signal_id, "symbol": symbol });
            ExpoPushMessage::builder(vec![token])
                .title(format!("New {} Signal: {} {}", symbol, signal.classification.to_uppercase(), signal.entry_type.to_uppercase()))
                .body(format!("Entry: {:.5}, SL: {:.5}, TP1: {:.5}, TP2: {:.5}", signal.entry_price, signal.sl_price, signal.tp1_price, signal.tp2_price))
                .data(&data)
                .and_then(|builder| builder.build())
        })
        .filter_map(Result::ok) // Filter out any messages that failed to build
        .collect();

    let client = Expo::new(ExpoClientOptions::default());
    match client.send_push_notifications(messages).await {
        Ok(receipts) => tracing::info!("Push notifications sent successfully: {:?}", receipts),
        Err(e) => tracing::error!("Failed to send push notifications: {:?}", e),
    }
}

#[utoipa::path(
    post,
    path = "/sessions/{symbol}/config",
    params(
        ("symbol" = String, Path, description = "The trading symbol to configure, e.g., XAUUSD")
    ),
    request_body = UpdateSessionConfigRequest,
    responses(
        (status = 200, description = "Session configuration updated successfully"),
        (status = 404, description = "No session found for the specified symbol")
    )
)]
/// Handler to update the configuration for a specific trading session.
async fn update_session_config_handler(
    State(state): State<Arc<ApplicationStateWithTicks>>,
    Path(symbol): Path<String>,
    Json(body): Json<UpdateSessionConfigRequest>,
) -> Result<Json<&'static str>, StatusCode> {
    let mut sessions = state.inner.session_manager.sessions.lock().unwrap_or_else(|e| {
        tracing::error!("Session manager mutex poisoned while updating config; recovering. Error: {}", e);
        e.into_inner()
    });

    if let Some(session) = sessions.get_mut(&symbol) {
        tracing::info!(
            symbol = %symbol,
            old_model = %session.predictor_model,
            new_model = %body.predictor_model,
            "Updating predictor model for session."
        );
        session.predictor_model = body.predictor_model;
        Ok(Json("Session configuration updated"))
    } else {
        tracing::warn!("Attempted to configure non-existent session for symbol: {}", symbol);
        Err(StatusCode::NOT_FOUND)
    }
}

#[utoipa::path(
    get,
    path = "/signals/{symbol}",
    params(
        ("symbol" = String, Path, description = "The trading symbol, e.g., XAUUSD")
    ),
    responses(
        (status = 200, description = "Returns the latest scalp and swing signals for the symbol", body = LatestSignalsResponse),
        (status = 404, description = "No session found for symbol")
    )
)]
/// Handler to return the last generated signals for a specific symbol.
async fn get_latest_signals_handler(
    State(state): State<Arc<ApplicationStateWithTicks>>,
    Path(symbol): Path<String>,
) -> Result<Json<LatestSignalsResponse>, StatusCode> {
    let sessions = state.inner.session_manager.sessions.lock().unwrap_or_else(|e| {
        tracing::error!("Session manager mutex poisoned in get_latest_signals_handler! Recovering. Error: {}", e);
        e.into_inner()
    });

    if let Some(session) = sessions.get(&symbol) {
        let (scalp_signal, swing_signal) = session.get_latest_signals();
        Ok(Json(LatestSignalsResponse {
            scalp_signal,
            swing_signal,
        }))
    } else {
        tracing::warn!("No session found for symbol: {}", symbol);
        Err(StatusCode::NOT_FOUND)
    }
}

#[utoipa::path(
    get,
    path = "/signals/latest",
    responses(
        (status = 200, description = "Returns the latest signals for all active symbols", body = Vec<LatestSignalsForSymbol>)
    )
)]
/// Handler to return the latest signals for all active symbols.
async fn get_all_latest_signals_handler(
    State(state): State<Arc<ApplicationStateWithTicks>>,
) -> Json<Vec<LatestSignalsForSymbol>> {
    let sessions = state.inner.session_manager.sessions.lock().unwrap_or_else(|e| {
        tracing::error!("Session manager mutex poisoned in get_all_latest_signals_handler! Recovering. Error: {}", e);
        e.into_inner()
    });

    let mut all_signals = Vec::new();

    for (symbol, session) in sessions.iter() {
        let (scalp_signal, swing_signal) = session.get_latest_signals();
        
        // Only include symbols that have generated at least one signal
        if scalp_signal.is_some() || swing_signal.is_some() {
            all_signals.push(LatestSignalsForSymbol {
                symbol: symbol.clone(),
                scalp_signal,
                swing_signal,
            });
        }
    }

    Json(all_signals)
}

#[axum::debug_handler]
async fn get_loaded_models_handler(
    State(state): State<Arc<ApplicationStateWithTicks>>,
) -> Json<Vec<String>> {
    let keys = state.inner.predictor_cache.loaded_keys();
    Json(keys)
}


#[utoipa::path(
    post,
    path = "/confirm_arrival",
    request_body = ArrivalConfirmation,
    responses(
        (status = 200, description = "Confirms that the arrival price was received")
    )
)]
/// Handler for EA to confirm signal arrival price
async fn confirm_arrival_handler(
    Json(confirmation): Json<ArrivalConfirmation>,
) -> (StatusCode, Json<&'static str>) {
    // In a real system, you would store this in a database or cache against the signal_id
    // for slippage analysis.
    tracing::info!(
        signal_id = %confirmation.signal_id,
        arrival_price = confirmation.arrival_price,
        spread = confirmation.spread,
        "Received arrival confirmation from EA."
    );
    (StatusCode::OK, Json("Arrival confirmed"))
}

#[utoipa::path(
    post,
    path = "/confirm_execution",
    request_body = ExecutionConfirmation,
    responses(
        (status = 200, description = "Confirms that the trade execution was received")
    )
)]
/// Handler for EA to confirm trade execution details
async fn confirm_execution_handler(
    Json(confirmation): Json<ExecutionConfirmation>,
) -> (StatusCode, Json<&'static str>) {
    // This is critical. You would store this to link a signal to a live trade ticket.
    tracing::info!(signal_id = %confirmation.signal_id, order_ticket = confirmation.order_ticket, fill_price = confirmation.fill_price, "Received execution confirmation from EA.");
    (StatusCode::OK, Json("Execution confirmed"))
}

#[utoipa::path(
    post,
    path = "/log_trade",
    request_body = TradeLog,
    responses(
        (status = 200, description = "Confirms that the log was received")
    )
)]
/// Handler to receive and store a trade log from the MQL5 EA
async fn log_trade_handler(
    State(state): State<Arc<ApplicationStateWithTicks>>,
    Json(log): Json<TradeLog>,
) -> Json<&'static str> {
    let mut sessions = state.inner.session_manager.sessions.lock().unwrap_or_else(|e| {
        tracing::error!("Session manager mutex poisoned in clear_history_handler; recovering. Error: {}", e);
        e.into_inner()
    });
    if let Some(session) = sessions.get_mut(&log.symbol) {
        session.add_trade_log(log);
        tracing::info!(
            symbol = %session.symbol,
            "Logged new trade event. Total logs for symbol: {}",
            session.get_trade_logs().len()
        );
        Json("Log received and associated with session")
    } else {
        tracing::warn!(
            symbol = %log.symbol,
            "Received trade log for a symbol with no active session. Log was not stored."
        );
        Json("Log received but no active session found for symbol")
    }
}

#[utoipa::path(
    get,
    path = "/history",
    params(HistoryParams),
    responses(
        (status = 200, description = "Returns trade history with statistics", body = HistoryResponse)
    )
)]
/// Handler to return all stored trade logs
async fn get_logs_handler(
    State(state): State<Arc<ApplicationStateWithTicks>>,
    Query(mut params): Query<HistoryParams>,
) -> Json<HistoryResponse> {
    // If a symbol is provided, get logs for that symbol. Otherwise, get all logs.
    let symbol_filter = params.symbol.take();
    let sessions = state.inner.session_manager.sessions.lock().unwrap_or_else(|e| {
        tracing::error!("Session manager mutex poisoned in get_latest_signals_handler; recovering. Error: {}", e);
        e.into_inner()
    });

    let logs: Vec<TradeLog> = if let Some(symbol) = symbol_filter {
        sessions.get(&symbol).map_or(vec![], |s| s.get_trade_logs().into())
    } else {
        sessions.values().flat_map(|s| s.get_trade_logs()).collect()
    };

    let start_date = params
        .start_date
        .as_deref()
        .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok());
    let end_date = params
        .end_date
        .as_deref()
        .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok());

    let filtered_trades: Vec<TradeLog> = logs
        .iter()
        .filter(|log| {
            let log_date = log.timestamp.split(' ').next().and_then(|s| {
                NaiveDate::parse_from_str(s, "%Y.%m.%d")
                    .map_err(|e| {
                        tracing::debug!("Could not parse date from timestamp '{}': {}", log.timestamp, e)
                    }).ok()
            });

            match (log_date, start_date, end_date) {
                (Some(ld), Some(sd), Some(ed)) => ld >= sd && ld <= ed,
                (Some(ld), Some(sd), _) => ld >= sd,
                (Some(ld), _, Some(ed)) => ld <= ed,
                _ => true, // No date filters, include all
            }
        })
        .cloned()
        .collect();

    // Calculate stats only on "Close" events
    let mut total_profit = 0.0;
    let mut gross_profit = 0.0;
    let mut gross_loss = 0.0;
    let mut winning_trades = 0;

    let closing_trades: Vec<_> = filtered_trades.iter().filter(|t| t.event_type == "Close").collect();

    for trade in &closing_trades {
        total_profit += trade.profit;
        if trade.profit > 0.0 {
            winning_trades += 1;
            gross_profit += trade.profit;
        } else {
            gross_loss += trade.profit.abs();
        }
    }

    let total_trades = closing_trades.len();
    let losing_trades = total_trades - winning_trades;
    let win_rate_percent = if total_trades > 0 { (winning_trades as f64 / total_trades as f64) * 100.0 } else { 0.0 };
    let profit_factor = if gross_loss > 0.0 { gross_profit / gross_loss } else { 0.0 };

    Json(HistoryResponse {
        stats: HistoryStats { total_profit, total_trades, winning_trades, losing_trades, win_rate_percent, profit_factor },
        trades: filtered_trades,
    })
}

#[utoipa::path(
    delete,
    path = "/history",
    responses(
        (status = 200, description = "Trade history cleared successfully")
    )
)]
/// Handler to clear all stored trade logs.
async fn clear_history_handler(
    State(state): State<Arc<ApplicationStateWithTicks>>,
) -> (StatusCode, Json<&'static str>) {
    let mut sessions = state.inner.session_manager.sessions.lock().unwrap_or_else(|e| {
        tracing::error!("Session manager mutex poisoned in clear_history_handler; recovering. Error: {}", e);
        e.into_inner()
    });
    let mut cleared_count = 0;
    for session in sessions.values_mut() {
        if !session.get_trade_logs().is_empty() {
            session.clear_trade_logs(); // Correctly clear the trade logs
            cleared_count += 1; // Count how many sessions had logs cleared
        }
    }
    tracing::info!("Trade history cleared manually for {} sessions.", cleared_count);
    (StatusCode::OK, Json("Trade history cleared for all sessions"))
}
