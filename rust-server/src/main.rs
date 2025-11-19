// v4 XAU/USD scalper server
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::get,
    Json, Router,
};
use axum::routing::post;
use chrono::{NaiveDate, Utc};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use tracing_subscriber::EnvFilter;
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;
use xau_scalper_server::{
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
    trade_logs: Arc<Mutex<VecDeque<TradeLog>>>,
    signal_history: Arc<Mutex<VecDeque<HistoricalSignal>>>,
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

#[allow(dead_code)] // This is used by utoipa macro to generate OpenAPI spec
#[derive(OpenApi)]
#[openapi(
    paths(process_data_handler, log_trade_handler, get_logs_handler, clear_history_handler, get_latest_signals_handler, get_all_latest_signals_handler, get_signals_handler, confirm_arrival_handler, confirm_execution_handler),
    components(
        schemas(EvalRequest, EvalResponse, TradeLog, HistoryResponse, HistoryStats, PriceLevel, VwapBands, ArrivalConfirmation, ExecutionConfirmation, HistoryParams, LatestSignalsResponse, LatestSignalsForSymbol, ActiveSignal, HistoricalSignal)
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
        trade_logs: Arc::new(Mutex::new(VecDeque::new())),
        signal_history: Arc::new(Mutex::new(VecDeque::new())),
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

    // Lock the sessions map, get the specific session for the symbol, and process data.
    // The `get_or_create_session` handles the logic of creating a new session if it's the first time
    // we see this symbol. The lock is held for the duration of the data processing to ensure consistency.
    let mut sessions = state
        .session_manager.sessions.lock().unwrap_or_else(|e| {
            tracing::error!("Session manager mutex poisoned! Recovering. Error: {}", e);
            e.into_inner()
        });
    
    sessions.entry(req.symbol.clone()).or_insert_with(|| {
        tracing::info!("Creating new trading session for symbol: {}", req.symbol);
        xau_scalper_server::TradingSession::new(req.symbol.clone(), filter_scalp_by_swing)
    });

    if let Some(session) = sessions.get_mut(&req.symbol) {
        session.on_data(&req);

        // --- NEW: Add latest signals to history ---
        let (scalp_sig_opt, swing_sig_opt) = session.get_latest_signals();
        let mut history = state.signal_history.lock().unwrap_or_else(|e| e.into_inner());
        let now = Utc::now().timestamp();

        if let Some(swing_sig) = swing_sig_opt {
            history.push_front(HistoricalSignal {
                signal: ActiveSignal {
                    symbol: req.symbol.clone(),
                    signal: swing_sig,
                },
                created_at: now,
            });
        }
        if let Some(scalp_sig) = scalp_sig_opt {
            history.push_front(HistoricalSignal {
                signal: ActiveSignal {
                    symbol: req.symbol.clone(),
                    signal: scalp_sig,
                },
                created_at: now,
            });
        }
    } else {
        tracing::error!(symbol = %req.symbol, "Could not find or create session.");
        return (StatusCode::INTERNAL_SERVER_ERROR, Json("Session error"));
    }

    (StatusCode::OK, Json("Data processed"))
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
    if let Ok(mut logs) = state.trade_logs.lock() {
        // Insert new logs at the beginning to keep them sorted by most recent
        logs.push_front(log);
        tracing::info!("Logged new trade event. Total logs: {}", logs.len());
    } else {
        tracing::error!("Trade logs mutex was poisoned. A thread panicked while holding the lock.");
        // Potentially return an error status code here in a real scenario
    }
    Json("Log received")
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
    Query(params): Query<HistoryParams>,
) -> Json<HistoryResponse> {
    let logs = match state.trade_logs.lock() {
        Ok(logs) => logs,
        Err(_) => {
            tracing::error!("Trade logs mutex was poisoned. Returning empty history.");
            return Json(HistoryResponse::default());
        }
    };

    let start_date = params
        .start_date
        .and_then(|s| NaiveDate::parse_from_str(&s, "%Y-%m-%d").map_err(|e| tracing::warn!("Failed to parse start_date '{}': {}", s, e)).ok());
    let end_date = params
        .end_date
        .and_then(|s| NaiveDate::parse_from_str(&s, "%Y-%m-%d").map_err(|e| tracing::warn!("Failed to parse end_date '{}': {}", s, e)).ok());

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
                (Some(ld), Some(sd), None) => ld >= sd,
                (Some(ld), None, Some(ed)) => ld <= ed,
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
    match state.trade_logs.lock() {
        Ok(mut logs) => {
            logs.clear();
            tracing::info!("Trade history has been cleared manually.");
            (StatusCode::OK, Json("Trade history cleared"))
        }
        Err(_) => {
            tracing::error!("Trade logs mutex was poisoned. Could not clear history.");
            (StatusCode::INTERNAL_SERVER_ERROR, Json("Failed to acquire lock on trade history"))
        }
    }
}
