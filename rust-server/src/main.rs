// v4 XAU/USD scalper server
use axum::{
    extract::{Query, State},
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
use utoipa_swagger_ui::{SwaggerUi, Config};
use xau_scalper_server::{
    ArrivalConfirmation, EvalRequest, EvalResponse,
    ExecutionConfirmation, HistoryParams, HistoryResponse, HistoryStats, TradeLog, PriceLevel, VwapBands
};

/// Application state to hold trade logs and signal history in memory
#[derive(Clone)]
struct AppState {
    trade_logs: Arc<Mutex<VecDeque<TradeLog>>>,
    signal_history: Arc<Mutex<VecDeque<EvalResponse>>>,
}

#[allow(dead_code)] // This is used by utoipa macro to generate OpenAPI spec
#[derive(OpenApi)]
#[openapi(
    paths(eval_handler, log_trade_handler, get_logs_handler, get_latest_signal_handler, get_signals_handler, confirm_arrival_handler, confirm_execution_handler),
    components(
        schemas(EvalRequest, EvalResponse, TradeLog, HistoryResponse, HistoryStats, PriceLevel, VwapBands, ArrivalConfirmation, ExecutionConfirmation)
    ),
    tags((name = "XAU Scalper API", description = "API for XAU/USD Scalping Strategy"))
)]
struct ApiDoc;

#[tokio::main]
async fn main() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    // Initialize the shared state
    let shared_state = AppState {
        trade_logs: Arc::new(Mutex::new(VecDeque::new())),
        signal_history: Arc::new(Mutex::new(VecDeque::new())), // No capacity limit, will be cleared weekly
    };

    // --- New: Spawn a background task for weekly signal history cleanup ---
    let cleanup_state = shared_state.clone();
    tokio::spawn(async move {
        let mut last_cleanup_week = 0;
        loop {
            // Check every hour
            tokio::time::sleep(tokio::time::Duration::from_secs(3600)).await;
            let now = Utc::now();
            let current_week = now.iso_week().week();

            // Cleanup window: Saturday 04:00 - 04:59 UTC (6 hours after Friday 22:00 market close)
            if now.weekday() == Weekday::Sat && now.hour() == 4 && last_cleanup_week != current_week {
                tracing::info!("Performing weekly cleanup of signal history.");
                if let Ok(mut history) = cleanup_state.signal_history.lock() {
                    history.clear();
                    last_cleanup_week = current_week;
                    tracing::info!("Signal history cleared for the week.");
                }
            }
        }
    });

    let app = Router::new()
        .merge(SwaggerUi::new("/docs")
            .config(Config::from("/api-docs/openapi.json")))
        .route("/health", get(|| async { "OK" }))
        .route("/eval", post(eval_handler))
        .route("/latest-signal", get(get_latest_signal_handler))
        .route("/signals", get(get_signals_handler)) // New endpoint for all signals
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
    post,
    path = "/eval",
    request_body = EvalRequest,
    responses(
        (status = 200, description = "Returns a trade signal", body = EvalResponse)
    )
)]
async fn eval_handler(State(state): State<AppState>, Json(req): Json<EvalRequest>) -> Json<EvalResponse> {
    tracing::info!(
        symbol = %req.symbol,
        timeframe = %req.timeframe,
        closes_count = req.closes.len(),
        "Received evaluation request"
    );

    let response = xau_scalper_server::evaluate_signal(&req);

    tracing::info!(
        signal_id = %response.signal_id,
        entry_type = %response.entry_type,
        reason = %response.reason,
        entry_price = response.entry_price,
        sl_price = response.sl_price,
        tp1_price = response.tp1_price,
        "Sending evaluation response"
    );

    // --- New logic to store signal history ---
    // Only store actionable new trade signals for the mobile app/history.
    // Do not store position management actions like "hold" or "close".
    match response.entry_type.as_str() {
        "none" => {
            // Optionally log 'none' signals for diagnostics, but for now we skip.
        }
        _ => { // "long", "short"
            if let Ok(mut history) = state.signal_history.lock() {
                history.push_back(response.clone());
            } else {
                tracing::error!("Signal history mutex was poisoned.");
            }
        }
    }
    // --- End of new logic ---

    Json(response)
}

#[utoipa::path(
    get,
    path = "/latest-signal",
    responses(
        (status = 200, description = "Returns the last generated trade signal", body = Option<EvalResponse>),
        (status = 404, description = "No signal available yet")
    )
)]
/// Handler to return the last generated trade signal
async fn get_latest_signal_handler(State(state): State<AppState>) -> Json<Option<EvalResponse>> {
    if let Ok(history) = state.signal_history.lock() {
        // Find the last signal that is not a position management action.
        let latest_signal = history.iter().rev().find(|&res| res.entry_type != "none");
        // Clone the found signal to return it.
        Json(latest_signal.cloned())
    } else {
        tracing::error!(
            "Signal history mutex was poisoned. A thread panicked while holding the lock."
        );
        Json(None)
    }
}

#[utoipa::path(
    get,
    path = "/signals",
    responses(
        (status = 200, description = "Returns the last 60 trade signals", body = Vec<EvalResponse>)
    )
)]
/// Handler to return the last 60 generated trade signals
async fn get_signals_handler(State(state): State<AppState>) -> Json<Vec<EvalResponse>> {
    if let Ok(history) = state.signal_history.lock() {
        Json(history.iter().cloned().collect())
    } else {
        tracing::error!(
            "Signal history mutex was poisoned. A thread panicked while holding the lock."
        );
        Json(vec![])
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
    State(state): State<AppState>,
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
    State(state): State<AppState>,
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
