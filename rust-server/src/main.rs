// v4 XAU/USD scalper server
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::get,
    Json, Router,
};
use axum::routing::post;
use chrono::{Datelike, NaiveDate, Timelike, Utc, Weekday};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use tracing_subscriber::EnvFilter;
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;
use xau_scalper_server::{
    ArrivalConfirmation, EvalRequest, EvalResponse, SessionManager,
    ExecutionConfirmation, HistoryParams, HistoryResponse, HistoryStats, TradeLog, PriceLevel, VwapBands
};


/// A more flexible state for the whole application, including the new SessionManager.
#[derive(Clone, Default)]
struct ApplicationState {
    session_manager: SessionManager,
    trade_logs: Arc<Mutex<VecDeque<TradeLog>>>,
    // The global signal history can be removed if signals are only retrieved per-session.
    // We'll keep it for now to support the existing `/signals` endpoint.
    global_signal_history: Arc<Mutex<VecDeque<EvalResponse>>>,
}

#[derive(serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
struct LatestSignalsResponse {
    scalp_signal: Option<EvalResponse>,
    swing_signal: Option<EvalResponse>,
}

#[allow(dead_code)] // This is used by utoipa macro to generate OpenAPI spec
#[derive(OpenApi)]
#[openapi(
    paths(process_data_handler, log_trade_handler, get_logs_handler, get_latest_signals_handler, get_signals_handler, confirm_arrival_handler, confirm_execution_handler),
    components(
        schemas(EvalRequest, EvalResponse, TradeLog, HistoryResponse, HistoryStats, PriceLevel, VwapBands, ArrivalConfirmation, ExecutionConfirmation, HistoryParams, LatestSignalsResponse)
    ),
    tags((name = "XAU Scalper API", description = "API for XAU/USD Scalping Strategy"))
)]
struct ApiDoc;

#[tokio::main]
async fn main() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    // Initialize the shared state
    let shared_state = ApplicationState::default();

    // --- New: Spawn a background task for weekly signal history cleanup ---
    let cleanup_state = shared_state.clone();
    tokio::spawn(async move {
        let mut last_cleanup_week = 0;
        // Wait a bit on startup before starting the cleanup loop.
        tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;

        loop {
            // Check every hour
            tokio::time::sleep(tokio::time::Duration::from_secs(3600)).await;
            let now = Utc::now();
            let current_week = now.iso_week().week();

            // Cleanup window: Saturday 04:00 - 04:59 UTC (6 hours after Friday 22:00 market close)
            if now.weekday() == Weekday::Sat && now.hour() == 4 && last_cleanup_week != current_week {
                tracing::info!("Performing weekly cleanup of signal history.");
                if let Ok(mut history) = cleanup_state.global_signal_history.lock() {
                    history.clear();
                    last_cleanup_week = current_week;
                    tracing::info!("Signal history cleared for the week.");
                }
            }
        }
    });

    let app = Router::new()
        .merge(SwaggerUi::new("/docs").url("/api-docs/openapi.json", ApiDoc::openapi()))
        .route("/health", get(|| async { "OK" }))
        .route("/data", post(process_data_handler))
        .route("/signals/:symbol", get(get_latest_signals_handler))
        .route("/signals", get(get_signals_handler))
        // --- Add new routes for logging ---
        .route("/confirm_arrival", post(confirm_arrival_handler))
        .route("/confirm_execution", post(confirm_execution_handler))
        .route("/log_trade", post(log_trade_handler))
        // --- The history endpoint is now more powerful ---
        .route("/history", get(get_logs_handler))
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
        (status = 200, description = "Returns the last 60 trade signals", body = Vec<EvalResponse>)
    )
)]
/// Handler to return the last 60 generated trade signals
async fn get_signals_handler(State(state): State<ApplicationState>) -> Json<Vec<EvalResponse>> {
    if let Ok(history) = state.global_signal_history.lock() {
        Json(history.iter().cloned().collect())
    } else {
        tracing::error!(
            "Signal history mutex was poisoned. A thread panicked while holding the lock."
        );
        Json(vec![])
    }
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
        .session_manager
        .get_or_create_session(&req.symbol, filter_scalp_by_swing);

    if let Some(session) = sessions.get_mut(&req.symbol) {
        session.on_data(&req);

        // After processing, we can store the generated signals in the global history
        // for the `/signals` endpoint.
        let (scalp_sig, swing_sig) = session.get_latest_signals();
        let mut history = state.global_signal_history.lock().unwrap();
        if let Some(sig) = scalp_sig {
            if sig.entry_type != "none" {
                history.push_back(sig);
            }
        }
        if let Some(sig) = swing_sig {
            if sig.entry_type != "none" {
                history.push_back(sig);
            }
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
    let sessions = state.session_manager.sessions.lock().unwrap();
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
