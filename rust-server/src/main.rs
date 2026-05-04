// v4 XAU/USD scalper server
use axum::{
    routing::get,
    Router,
};
use axum::routing::post;
use chrono::Utc;
use dashmap::DashMap;
use std::collections::{BTreeSet, VecDeque};
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use tokio::sync::broadcast;
use tracing_subscriber::EnvFilter;
use utoipa::{OpenApi};
use utoipa_swagger_ui::SwaggerUi;
use tokio::fs; // Use tokio's async fs module
pub use xau_scalper_server::{
    EvalRequest, EvalResponse, SessionManager, PriceLevel, VwapBands, NewsEvent, TradingSession, MacroCategory, NewsItem, CalendarEvent
};
use tokio::sync::Mutex;

pub mod llm;
mod routes;
mod background;
pub mod state;
pub mod handlers;
pub mod services;
pub mod external_feeds;
use xau_scalper_server::db;

use prometheus::{register_counter, register_counter_vec, register_gauge};
use xau_scalper_server::config::Settings;
use xau_scalper_server::config::{TradingSettings, ScalpSettings, SwingSettings, RiskSettings};
use xau_scalper_server::engines::news_guard::GuardResult;
use xau_scalper_server::macro_analysis::types::{MacroOutlook, Bias};
use crate::state::*;
use xau_scalper_server::metrics::AppMetrics;
use crate::routes::ChatRequest;

#[cfg(test)]
mod e2e_tests;

#[allow(dead_code)] // This is used by utoipa macro to generate OpenAPI spec
#[derive(OpenApi)]
#[openapi(
    paths(
        handlers::system::health_check_handler,
        handlers::trading::process_data_handler,
        handlers::trading::documented_tick_ingest_handler,
        handlers::system::save_push_token_handler,
        handlers::system::remove_push_token_handler,
        routes::daily_analysis,
        routes::weekly_analysis,
        routes::chat_analysis_handler,
        routes::test_push_handler,
        handlers::system::get_loaded_models_handler,
        handlers::trading::get_signal_definitions_handler,
        handlers::system::metrics_handler,
        handlers::system::prometheus_metrics_handler,
        handlers::system::update_settings_handler,
        handlers::system::reset_settings_handler,
        handlers::system::get_settings_handler,
        handlers::trading::get_news_guard_status_handler,
        routes::get_signals_paginated_handler,
        routes::clear_database_handler,
        routes::get_external_calendar_handler,
        routes::get_external_news_handler
    ),
    components(
        schemas(EvalRequest, EvalResponse, PriceLevel, VwapBands, ActiveSignal, HistoricalSignal, SavePushTokenRequest, TickData, MetricsResponse, SignalReasonInfo, SignalDefinitionsResponse, NewsEvent, TradingSettings, ScalpSettings, SwingSettings, RiskSettings, GuardResult, MacroOutlook, Bias, MacroCategory, ChatRequest, CalendarEvent, NewsItem)
    ),
    info(
        description = "This API provides endpoints for the XAU/USD Scalping and Swing Trading Engines. It processes market data, generates trading signals, and provides a real-time data stream via WebSockets. It also includes AI-powered Technical and Fundamental analysis endpoints."
    ),
    tags((name = "Trading Signal API", description = "Endpoints for signal generation, data processing, and session management."))
)]
struct ApiDoc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Load environment variables from .env file
    dotenvy::dotenv().ok();

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,ort=warn"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    // --- NEW: Load Configuration ---
    let config = Settings::new().expect("Failed to load configuration");
    tracing::info!("Configuration loaded. Max buffer size: {}", config.trading.max_buffer_size);

    // Log if we are using a persistent override so you know your API changes are active
    let persistent_config_path = format!("{}/config.toml", config.paths.sessions_dir);
    if fs::try_exists(&persistent_config_path).await.unwrap_or(false) {
        tracing::info!("Active configuration is being overridden by persistent settings found in: {}", persistent_config_path);
    }

    // Initialize Database
    // Prioritize DATABASE_URL env var (Render), fallback to config, then panic
    let db_url = std::env::var("DATABASE_URL").unwrap_or(config.database.url.clone());
    if db_url.is_empty() {
        panic!("Database URL is not set. Please set DATABASE_URL environment variable.");
    }

    let mut db_pool = None;
    let max_retries = 10;
    for attempt in 1..=max_retries {
        match db::init_db(&db_url).await {
            Ok(pool) => {
                db_pool = Some(pool);
                break;
            }
            Err(e) => {
                tracing::warn!("Failed to initialize database (attempt {}/{}): {}", attempt, max_retries, e);
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            }
        }
    }
    let db_pool = db_pool.expect("Failed to initialize database after multiple attempts");

    // --- NEW: Load push tokens from DB on startup ---
    let initial_push_tokens = match db::load_push_tokens(&db_pool, None).await {
        Ok(tokens) => {
            tracing::info!("Loaded {} push notification tokens from DB", tokens.len());
            tokens
        },
        Err(e) => {
            tracing::error!("Failed to load push tokens from DB: {}. Falling back to empty set.", e);
            BTreeSet::new()
        }
    };

    // --- NEW: Load settings from DB ---
    let mut trading_settings = config.trading.clone();
    match db::load_settings(&db_pool, None).await {
        Ok(Some(db_settings)) => {
            tracing::info!("Loaded trading settings from Database, overriding config file.");
            trading_settings = db_settings;
        },
        Ok(None) => {
            tracing::info!("No settings found in Database, using config file defaults.");
        },
        Err(e) => {
            tracing::error!("Failed to load settings from DB: {}. Using config file defaults.", e);
        }
    }

    // Initialize metrics with proper error handling
    let signal_counter = register_counter!("xau_scalper_signals_total", "Total number of signals generated")
        .map_err(|e| format!("Failed to register signal_counter: {}", e))?;
    let active_sessions = register_gauge!("xau_scalper_active_sessions", "Number of active trading sessions")
        .map_err(|e| format!("Failed to register active_sessions: {}", e))?;
    let active_ws_clients = register_gauge!("xau_scalper_active_ws_clients", "Number of active WebSocket clients")
        .map_err(|e| format!("Failed to register active_ws_clients: {}", e))?;
    let http_requests = register_counter!("xau_scalper_http_requests_total", "Total HTTP requests received")
        .map_err(|e| format!("Failed to register http_requests: {}", e))?;

    let gemini_429_errors = register_counter!("xau_scalper_gemini_429_errors_total", "Total Gemini API 429 Too Many Requests errors")
        .map_err(|e| format!("Failed to register gemini_429_errors: {}", e))?;

    let gemini_success_model = register_counter_vec!("xau_scalper_gemini_success_model_total", "Total successful Gemini API requests by model", &["model"])
        .map_err(|e| format!("Failed to register gemini_success_model: {}", e))?;

    let db_retries_total = register_counter!("xau_scalper_db_retries_total", "Total number of database operation retries")
        .map_err(|e| format!("Failed to register db_retries_total: {}", e))?;

    let metrics = AppMetrics {
        signal_counter, active_sessions, active_ws_clients, http_requests, gemini_429_errors, gemini_success_model, db_retries_total
    };

    // 1. Create a channel to broadcast live market data AND signals to WebSocket clients.
    // Reduced buffer size to 1,000 to save memory on Render.
    let (tick_tx, _) = broadcast::channel::<String>(1000);

    // Initialize the shared state
    let shared_state = ApplicationState {
        session_manager: SessionManager::new(trading_settings, tick_tx.clone(), Some(db_pool.clone()), Some(Arc::new(metrics.clone()))),
        predictor_cache: xau_scalper_server::engines::predictor_cache::PredictorCache::new(config.paths.models_dir.clone()),
        signal_history: Arc::new(Mutex::new(VecDeque::new())),
        // Use the tokens loaded from the file
        push_tokens: Arc::new(Mutex::new(initial_push_tokens)),
        news_events: Arc::new(Mutex::new(Vec::new())),
        external_calendar_events: Arc::new(Mutex::new(Vec::new())),
        external_rss_news: Arc::new(Mutex::new(Vec::new())),
        fundamental_analysis_cache: Arc::new(DashMap::new()),
        ws_clients: Arc::new(AtomicUsize::new(0)),
        chat_rate_limiter: Arc::new(DashMap::new()),
        chat_sessions: Arc::new(DashMap::new()),
        server_start_time: Utc::now(),
        total_signals_generated: Arc::new(AtomicUsize::new(0)),
        http_client: reqwest::Client::builder()
            .user_agent("xau_scalper_ml/1.0")
            .build()
            .unwrap(),
        config: config.clone(),
        metrics,
        db: db_pool,
    };

    // --- NEW: Load persisted sessions from DB ---
    match db::load_sessions(&shared_state.db, Some(&shared_state.metrics.db_retries_total)).await {
        Ok(sessions) => {
            for mut session in sessions {
                session.broadcast_tx = Some(tick_tx.clone());
                session.db = Some(shared_state.db.clone()); // Inject DB pool
                session.metrics = Some(Arc::new(shared_state.metrics.clone())); // Inject Metrics
                tracing::info!("Loaded session for {} from DB.", session.symbol);
                shared_state.session_manager.sessions.insert(session.symbol.clone(), Arc::new(Mutex::new(session)));
            }
        },
        Err(e) => {
            tracing::error!("Failed to load sessions from DB: {}", e);
        }
    };

    // --- Phase 2: Real-time Data Gateway ---

    // Add the tick_tx channel to the application state so the handler can access it.
    let app_state_with_ticks = Arc::new(shared_state.with_ticks(tick_tx));

    // --- Create Shutdown Channel ---
    let (shutdown_tx, _) = broadcast::channel::<()>(1);

    // --- Start Background Tasks ---
    background::spawn_stale_signal_cleanup_task(app_state_with_ticks.clone(), shutdown_tx.subscribe());
    background::spawn_history_cleanup_task(app_state_with_ticks.clone(), shutdown_tx.subscribe());
    background::spawn_session_persistence_task(app_state_with_ticks.clone(), shutdown_tx.subscribe());
    background::spawn_db_cleanup_task(app_state_with_ticks.clone(), shutdown_tx.subscribe());
    external_feeds::spawn_external_feeds_task(app_state_with_ticks.clone(), shutdown_tx.subscribe());

    // --- Start Background Analysis Task ---
    routes::start_background_analysis_task(app_state_with_ticks.clone(), shutdown_tx.subscribe());

    let app = create_app(app_state_with_ticks);

    // Render/Docker specific: Listen on 0.0.0.0 and use the PORT env var
    let host = "0.0.0.0"; 
    let port = std::env::var("PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(config.server.port);
    
    let addr = format!("{}:{}", host, port);
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    tracing::info!("listening on {}", addr);
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal(shutdown_tx))
        .await
        .unwrap();

    Ok(())
}

/// Awaits a shutdown signal (e.g., Ctrl+C) for graceful server shutdown.
async fn shutdown_signal(shutdown_tx: broadcast::Sender<()>) {
    tokio::signal::ctrl_c()
        .await
        .expect("failed to install CTRL+C signal handler");
    tracing::info!("Received shutdown signal, shutting down gracefully.");
    let _ = shutdown_tx.send(());
}

pub fn create_app(state: Arc<ApplicationStateWithTicks>) -> Router {
    Router::new()
        .layer(axum::extract::DefaultBodyLimit::max(2 * 1024 * 1024)) // 2MB Limit
        .merge(SwaggerUi::new("/docs").url("/api-docs/openapi.json", ApiDoc::openapi()))
        .route("/", get(handlers::system::health_check_handler))
        .route("/ws", get(handlers::ws::websocket_handler))
        .route("/metrics", get(handlers::system::metrics_handler))
        .route("/metrics/prometheus", get(handlers::system::prometheus_metrics_handler)) // NEW: Prometheus endpoint
        .route("/health", get(handlers::system::health_check_handler))
        .route("/data", post(handlers::trading::process_data_handler)) // For main analysis
        .route("/ticks", post(handlers::trading::tick_ingest_handler)) // NEW: For live ticks
        .route("/signals/paginated", get(routes::get_signals_paginated_handler)) // NEW: Pagination
        .route("/signals/clear", axum::routing::delete(routes::clear_database_handler)) // NEW: Clear DB
        .route("/definitions/reasons", get(handlers::trading::get_signal_definitions_handler))
        .route("/external/calendar", get(routes::get_external_calendar_handler)) // NEW: External Calendar
        .route("/external/news", get(routes::get_external_news_handler)) // NEW: External RSS News
        // --- Add new routes for logging ---
        .route("/save-push-token", post(handlers::system::save_push_token_handler).delete(handlers::system::remove_push_token_handler))
        .route("/settings", post(handlers::system::update_settings_handler).get(handlers::system::get_settings_handler))
        .route("/settings/reset", post(handlers::system::reset_settings_handler))
        .route("/news-guard/:symbol", get(handlers::trading::get_news_guard_status_handler))
        .route("/models/loaded", get(handlers::system::get_loaded_models_handler))
        // --- Analysis Endpoints ---
        .route("/daily-analysis", get(routes::daily_analysis))
        .route("/weekly-analysis", get(routes::weekly_analysis))
        .route("/chat-analysis", post(routes::chat_analysis_handler))
        .route("/test-push", post(routes::test_push_handler))
        // Provide the state to all handlers
        .with_state(state)
}