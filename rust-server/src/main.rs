// v4 XAU/USD scalper server
use axum::{routing::get, routing::post, Json, Router};
use tracing_subscriber::EnvFilter;
use xau_scalper_server::{strategy::evaluate_strategy, EvalRequest, EvalResponse};

#[tokio::main]
async fn main() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let app = Router::new()
        .route("/health", get(|| async { "OK" }))
        .route("/eval", post(eval_handler));

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    tracing::info!("listening on {}", listener.local_addr().unwrap());
    axum::serve(listener, app).await.unwrap();
}

async fn eval_handler(Json(req): Json<EvalRequest>) -> Json<EvalResponse> {
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
        "Sending evaluation response"
    );

    Json(response)
}
