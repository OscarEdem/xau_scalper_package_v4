// v4 XAU/USD scalper server
use axum::{
    extract::{Query, State},
    http::StatusCode,
    routing::get,
    routing::post,
    Json, Router,
};
use chrono::NaiveDate;
use std::sync::{Arc, Mutex};
use tracing_subscriber::EnvFilter;
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;
use xau_scalper_server::{
    strategy::evaluate_strategy, EvalRequest, EvalResponse, HistoryParams, HistoryResponse,
    HistoryStats, TradeLog,
};

/// Application state to hold trade logs and signal history in memory
#[derive(Clone)]
struct AppState {
    trade_logs: Arc<Mutex<Vec<TradeLog>>>,
    signal_history: Arc<Mutex<Vec<EvalResponse>>>, // Changed from last_eval_response
}

#[derive(OpenApi)]
#[openapi(
    paths(eval_handler, log_trade_handler, get_logs_handler, get_latest_signal_handler, get_signals_handler),
    components(
        schemas(EvalRequest, EvalResponse, TradeLog, HistoryResponse, HistoryStats)
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
        trade_logs: Arc::new(Mutex::new(Vec::new())),
        signal_history: Arc::new(Mutex::new(Vec::new())), // Initialize with an empty Vec
    };

    let app = Router::new()
        .merge(SwaggerUi::new("/swagger-ui").url("/api-docs/openapi.json", ApiDoc::openapi()))
        .route("/health", get(|| async { "OK" }))
        .route("/eval", post(eval_handler))
        .route("/latest-signal", get(get_latest_signal_handler))
        .route("/signals", get(get_signals_handler)) // New endpoint for all signals
        // --- Add new routes for logging ---
        .route("/log_trade", post(log_trade_handler))
        // --- The history endpoint is now more powerful ---
        .route("/history", get(get_logs_handler))
        // Provide the state to the handlers
        .with_state(shared_state);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    tracing::info!("listening on {}", listener.local_addr().unwrap());
    axum::serve(listener, app).await.unwrap();
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

    let response = evaluate_strategy(&req);

    tracing::info!(
        action = %response.action_advice,
        reason = %response.reason,
        rsi = response.rsi,
        ema_fast = response.ema_fast_last,
        ema_slow = response.ema_slow_last,
        tp_pips = response.tp_pips,
        sl_pips = response.sl_pips,
        atr = response.atr,
        "Sending evaluation response"
    );

    // --- New logic to store signal history ---
    let mut history = state.signal_history.lock().unwrap();
    history.push(response.clone()); // Add the new signal
    if history.len() > 60 {
        history.remove(0); // Remove the oldest signal if we're over the limit
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
    let history = state.signal_history.lock().unwrap();
    // Get the last signal from the history vector.
    // .last() returns an Option<&T>, and we clone it to get Option<T>.
    Json(history.last().cloned())
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
    let history = state.signal_history.lock().unwrap();
    Json(history.clone())
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
    let mut logs = state.trade_logs.lock().unwrap();
    // Insert new logs at the beginning to keep them sorted by most recent
    logs.insert(0, log);
    tracing::info!("Logged new trade event. Total logs: {}", logs.len());
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
    let logs = state.trade_logs.lock().unwrap();

    let start_date = params
        .start_date
        .and_then(|s| NaiveDate::parse_from_str(&s, "%Y-%m-%d").ok());
    let end_date = params
        .end_date
        .and_then(|s| NaiveDate::parse_from_str(&s, "%Y-%m-%d").ok());

    let filtered_trades: Vec<TradeLog> = logs
        .iter()
        .filter(|log| {
            let log_date = log
                .timestamp
                .split(' ')
                .next()
                .and_then(|s| NaiveDate::parse_from_str(s, "%Y.%m.%d").ok());

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
