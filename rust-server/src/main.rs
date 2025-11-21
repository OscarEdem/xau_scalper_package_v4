// v4 XAU/USD scalper server
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::get,
    Json, Router,
};
use axum::routing::post;
use chrono::{NaiveDate, Utc};
use expo_push_notification_client::{Expo, ExpoClientOptions, ExpoPushMessage};
use std::collections::{VecDeque, BTreeSet};
use std::sync::{Arc, Mutex};
use tracing_subscriber::EnvFilter;
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;
pub use xau_scalper_server::{
    ArrivalConfirmation, EvalRequest, EvalResponse, SessionManager,
    ExecutionConfirmation, HistoryParams, HistoryResponse, HistoryStats, TradeLog, PriceLevel, VwapBands
};

/// A wrapper to store a signal with its creation timestamp for historical logging.
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
    signal_history: Arc<Mutex<VecDeque<HistoricalSignal>>>,
    push_tokens: Arc<Mutex<BTreeSet<String>>>,
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


#[allow(dead_code)] // This is used by utoipa macro to generate OpenAPI spec
#[derive(OpenApi)]
#[openapi(
    paths(process_data_handler, log_trade_handler, get_logs_handler, clear_history_handler, get_latest_signals_handler, get_all_latest_signals_handler, get_signals_handler, confirm_arrival_handler, confirm_execution_handler, save_push_token_handler),
    components(
        schemas(EvalRequest, EvalResponse, TradeLog, HistoryResponse, HistoryStats, PriceLevel, VwapBands, ArrivalConfirmation, ExecutionConfirmation, HistoryParams, LatestSignalsResponse, LatestSignalsForSymbol, ActiveSignal, HistoricalSignal, SavePushTokenRequest)
    ),
    tags((name = "XAU Scalper API", description = "API for XAU/USD Scalping Strategy"))
)]
struct ApiDoc;

#[tokio::main]
async fn main() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    // Initialize the shared state
    let shared_state = ApplicationState {
        session_manager: SessionManager::default(),
        signal_history: Arc::new(Mutex::new(VecDeque::new())),
        push_tokens: Arc::new(Mutex::new(BTreeSet::new())),
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

    // --- NEW: Background task for 24-hour signal history cleanup ---
    let history_cleanup_state = shared_state.clone();
    tokio::spawn(async move {
        loop {
            // Check every hour
            tokio::time::sleep(tokio::time::Duration::from_secs(3600)).await;
            let now = Utc::now().timestamp();
            const ONE_DAY_IN_SECONDS: i64 = 24 * 60 * 60;

            if let Ok(mut history) = history_cleanup_state.signal_history.lock() {
                let original_len = history.len();
                history.retain(|hs| (now - hs.created_at) < ONE_DAY_IN_SECONDS);
                let removed_count = original_len - history.len();
                if removed_count > 0 {
                    tracing::info!("Removed {} signals from history older than 24 hours.", removed_count);
                }
            }
        }
    });

    let app = Router::new()
        .merge(SwaggerUi::new("/docs").url("/api-docs/openapi.json", ApiDoc::openapi()))
        .route("/health", get(|| async { "OK" }))
        .route("/data", post(process_data_handler))
        .route("/signals/:symbol", get(get_latest_signals_handler))
        .route("/signals/latest", get(get_all_latest_signals_handler))
        .route("/signals", get(get_signals_handler))
        // --- Add new routes for logging ---
        .route("/confirm_arrival", post(confirm_arrival_handler))
        .route("/confirm_execution", post(confirm_execution_handler))
        .route("/save-push-token", post(save_push_token_handler))
        .route("/log_trade", post(log_trade_handler))
        // --- The history endpoint is now more powerful ---
        .route("/history", get(get_logs_handler).delete(clear_history_handler))
        // Provide the state to the handlers
        .with_state(shared_state);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    tracing::info!("listening on {}", listener.local_addr().unwrap());
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .unwrap();
}

/// Awaits a shutdown signal (e.g., Ctrl+C) for graceful server shutdown.
async fn shutdown_signal() {
    tokio::signal::ctrl_c()
        .await
        .expect("failed to install CTRL+C signal handler");
    tracing::info!("Received shutdown signal, shutting down gracefully.");
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
    State(state): State<ApplicationState>,
    Json(body): Json<SavePushTokenRequest>,
) -> (StatusCode, Json<&'static str>) {
    if body.token.is_empty() || !body.token.starts_with("ExponentPushToken[") {
        return (StatusCode::BAD_REQUEST, Json("Invalid push token format"));
    }

    let mut tokens = state.push_tokens.lock().unwrap();
    tokens.insert(body.token);
    tracing::info!("Saved new push token. Total tokens: {}", tokens.len());

    (StatusCode::OK, Json("Token saved"))
}

#[utoipa::path(
    get,
    path = "/signals",
    responses(
        (status = 200, description = "Returns a log of all signals generated in the last 24 hours", body = Vec<HistoricalSignal>)
    )
)]
/// Handler to return a log of all signals generated in the last 24 hours.
async fn get_signals_handler(State(state): State<ApplicationState>) -> Json<Vec<HistoricalSignal>> {
    let history = state.signal_history.lock().unwrap_or_else(|e| {
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
async fn process_data_handler(
    State(state): State<ApplicationState>,
    Json(req): Json<EvalRequest>,
) -> (StatusCode, Json<&'static str>) {
    tracing::info!(
        symbol = %req.symbol,
        timeframe = %req.timeframe,
        closes_count = req.closes.len(),
        "Received data processing request"
    );

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
            .session_manager.sessions.lock().unwrap_or_else(|e| {
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
        session.on_data(&req);

        // Now, get the new signals and add them to history
        let (scalp_sig_opt, swing_sig_opt) = session.get_latest_signals();
        let mut history = state.signal_history.lock().unwrap_or_else(|e| e.into_inner());

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
        send_push_notification(state.push_tokens.clone(), &signal, &symbol).await;
    }

    (StatusCode::OK, Json("Data processed"))
}

/// Sends a push notification for a new signal to all registered devices.
async fn send_push_notification(
    tokens_arc: Arc<Mutex<BTreeSet<String>>>,
    signal: &EvalResponse,
    symbol: &str,
) {
    let tokens = tokens_arc.lock().unwrap().clone();
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
                .body(format!("Entry: {:.5}, SL: {:.5}, TP1: {:.5}", signal.entry_price, signal.sl_price, signal.tp1_price))
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
    State(state): State<ApplicationState>,
    Path(symbol): Path<String>,
) -> Result<Json<LatestSignalsResponse>, StatusCode> {
    let sessions = state.session_manager.sessions.lock().unwrap_or_else(|e| {
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
    State(state): State<ApplicationState>,
) -> Json<Vec<LatestSignalsForSymbol>> {
    let sessions = state.session_manager.sessions.lock().unwrap_or_else(|e| {
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
    State(state): State<ApplicationState>,
    Json(log): Json<TradeLog>,
) -> Json<&'static str> {
    let mut sessions = state.session_manager.sessions.lock().unwrap();
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
    State(state): State<ApplicationState>,
    Query(mut params): Query<HistoryParams>,
) -> Json<HistoryResponse> {
    // If a symbol is provided, get logs for that symbol. Otherwise, get all logs.
    let symbol_filter = params.symbol.take();
    let sessions = state.session_manager.sessions.lock().unwrap();

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
    State(state): State<ApplicationState>,
) -> (StatusCode, Json<&'static str>) {
    let mut sessions = state.session_manager.sessions.lock().unwrap();
    let mut cleared_count = 0;
    for session in sessions.values_mut() {
        if !session.get_trade_logs().is_empty() {
            session.invalidate_signals(); // Also clear logs from session, assuming this is desired
            cleared_count += 1;
        }
    }
    tracing::info!("Trade history cleared manually for {} sessions.", cleared_count);
    (StatusCode::OK, Json("Trade history cleared for all sessions"))
}
