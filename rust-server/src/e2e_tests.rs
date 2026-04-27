use super::*;
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use tower::util::ServiceExt; // for `oneshot`
use std::sync::Arc;
use tokio::sync::broadcast;
use xau_scalper_server::config::TradingSettings;
use xau_scalper_server::metrics::AppMetrics;
use prometheus::{register_counter, register_gauge, register_counter_vec};

/// Helper to create a mock application state for testing.
/// Note: This requires a running Database if we use the real `init_db`.
/// For pure unit tests, we would mock the DB, but for E2E we often want the real thing or a test container.
async fn setup_test_state() -> Arc<ApplicationStateWithTicks> {
    // 1. Metrics (Registering duplicates panics, so we might need to handle that or run tests in separate processes)
    // For simplicity in this example, we assume tests run sequentially or we ignore registration errors.
    let signal_counter = register_counter!("test_signals_total", "Test").unwrap_or_else(|_| prometheus::Counter::new("test_signals_total", "Test").unwrap());
    let active_sessions = register_gauge!("test_active_sessions", "Test").unwrap_or_else(|_| prometheus::Gauge::new("test_active_sessions", "Test").unwrap());
    let active_ws_clients = register_gauge!("test_active_ws_clients", "Test").unwrap_or_else(|_| prometheus::Gauge::new("test_active_ws_clients", "Test").unwrap());
    let http_requests = register_counter!("test_http_requests_total", "Test").unwrap_or_else(|_| prometheus::Counter::new("test_http_requests_total", "Test").unwrap());
    let gemini_429_errors = register_counter!("test_gemini_429_errors_total", "Test").unwrap_or_else(|_| prometheus::Counter::new("test_gemini_429_errors_total", "Test").unwrap());
    let gemini_success_model = register_counter_vec!("test_gemini_success_model_total", "Test", &["model"]).unwrap_or_else(|_| prometheus::CounterVec::new(prometheus::Opts::new("test_gemini_success_model_total", "Test"), &["model"]).unwrap());
    let db_retries_total = register_counter!("test_db_retries_total", "Test").unwrap_or_else(|_| prometheus::Counter::new("test_db_retries_total", "Test").unwrap());

    let metrics = AppMetrics {
        signal_counter,
        active_sessions,
        active_ws_clients,
        http_requests,
        gemini_429_errors,
        gemini_success_model,
        db_retries_total,
    };

    // 2. Database
    // Expects DATABASE_URL to be set, or defaults to a local test DB.
    // Ensure you have a DB running or use `sqlx::test` macro for isolated DB tests.
    let db_url = std::env::var("DATABASE_URL").unwrap_or("postgres://postgres:password@localhost:5432/xau_scalper".to_string());
    let db_pool = db::init_db(&db_url).await.expect("Failed to connect to Test DB. Ensure Postgres is running.");

    // 3. Channels
    let (tick_tx, _) = broadcast::channel(100);

    // 4. Config
    let config = Settings::new().unwrap_or_else(|_| {
        // Fallback config if file missing
        let s = Settings::new().unwrap(); // This might panic if no config, so we should construct manually if needed
        s
    });

    // 5. State Construction
    let shared_state = ApplicationState {
        session_manager: SessionManager::new(TradingSettings::default(), tick_tx.clone(), Some(db_pool.clone()), Some(Arc::new(metrics.clone()))),
        predictor_cache: xau_scalper_server::engines::predictor_cache::PredictorCache::new("./models".to_string()),
        signal_history: Arc::new(Mutex::new(VecDeque::new())),
        push_tokens: Arc::new(Mutex::new(BTreeSet::new())),
        news_events: Arc::new(Mutex::new(Vec::new())),
        external_calendar_events: Arc::new(Mutex::new(Vec::new())),
        external_rss_news: Arc::new(Mutex::new(Vec::new())),
        fundamental_analysis_cache: Arc::new(DashMap::new()),
        ws_clients: Arc::new(AtomicUsize::new(0)),
        chat_rate_limiter: Arc::new(DashMap::new()),
        server_start_time: Utc::now(),
        total_signals_generated: Arc::new(AtomicUsize::new(0)),
        http_client: reqwest::Client::new(),
        config,
        metrics,
        db: db_pool,
    };

    Arc::new(shared_state.with_ticks(tick_tx))
}

#[tokio::test]
#[ignore = "Requires a running Postgres instance. Run locally with: cargo test -- --include-ignored"]
async fn test_health_check_endpoint() {
    let state = setup_test_state().await;
    let app = create_app(state);

    // Send a request to the app
    let response = app
        .oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    // Verify body content
    let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body_str = String::from_utf8(body.to_vec()).unwrap();
    // Assuming health check returns JSON or simple text
    assert!(body_str.contains("ok") || body_str.contains("healthy") || body_str.contains("uptime"));
}