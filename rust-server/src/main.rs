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
use utoipa_swagger_ui::SwaggerUi; pub use xau_scalper_server::{news_fetcher::fetch_calendar_events};
use tokio::fs; // Use tokio's async fs module
pub use xau_scalper_server::{
    EvalRequest, EvalResponse, SessionManager, PriceLevel, VwapBands, NewsEvent, TradingSession, MacroCategory
};
use tokio::sync::Mutex;

pub mod llm;
mod routes;
mod background;
pub mod state;
pub mod handlers;
pub mod services;

use prometheus::{register_counter, register_counter_vec, register_gauge};
use xau_scalper_server::config::Settings;
use xau_scalper_server::config::{TradingSettings, ScalpSettings, SwingSettings};
use xau_scalper_server::engines::news_guard::GuardResult;
use xau_scalper_server::macro_analysis::types::{MacroOutlook, Bias};
use crate::state::*;
use crate::routes::ChatRequest;

#[allow(dead_code)] // This is used by utoipa macro to generate OpenAPI spec
#[derive(OpenApi)]
#[openapi(
    paths(
        handlers::system::health_check_handler,
        handlers::trading::process_data_handler,
        handlers::trading::documented_tick_ingest_handler,
        handlers::trading::get_latest_signals_handler,
        handlers::trading::get_all_latest_signals_handler,
        handlers::trading::get_signals_handler,
        handlers::system::save_push_token_handler,
        routes::daily_analysis,
        routes::weekly_analysis,
        routes::chat_analysis_handler,
        handlers::system::get_loaded_models_handler,
        handlers::trading::get_signal_definitions_handler,
        handlers::system::metrics_handler,
        handlers::system::prometheus_metrics_handler,
        handlers::system::update_settings_handler,
        handlers::system::reset_settings_handler,
        handlers::system::get_settings_handler,
        handlers::trading::get_news_guard_status_handler
    ),
    components(
        schemas(EvalRequest, EvalResponse, PriceLevel, VwapBands, LatestSignalsForSymbol, ActiveSignal, HistoricalSignal, SavePushTokenRequest, TickData, MetricsResponse, SignalReasonInfo, SignalDefinitionsResponse, NewsEvent, TradingSettings, ScalpSettings, SwingSettings, GuardResult, MacroOutlook, Bias, MacroCategory, ChatRequest)
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

    // --- NEW: Load push tokens from file on startup ---
    let initial_push_tokens = match fs::read_to_string(&config.paths.push_tokens_file).await {
        Ok(content) => {
            let tokens: BTreeSet<String> = serde_json::from_str(&content).unwrap_or_default();
            tracing::info!("Loaded {} push notification tokens from {}", tokens.len(), config.paths.push_tokens_file);
            tokens
        }
        Err(_) => {
            tracing::info!("No '{}' file found. Starting with an empty set of push tokens.", config.paths.push_tokens_file);
            BTreeSet::new()
        }
    };

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

    let metrics = AppMetrics {
        signal_counter, active_sessions, active_ws_clients, http_requests, gemini_429_errors, gemini_success_model
    };

    // Initialize the shared state
    let shared_state = ApplicationState {
        session_manager: SessionManager::new(config.trading.clone()),
        predictor_cache: xau_scalper_server::engines::predictor_cache::PredictorCache::new(config.paths.models_dir.clone()),
        signal_history: Arc::new(Mutex::new(VecDeque::new())),
        // Use the tokens loaded from the file
        push_tokens: Arc::new(Mutex::new(initial_push_tokens)),
        news_events: Arc::new(Mutex::new(Vec::new())),
        fundamental_analysis_cache: Arc::new(DashMap::new()),
        ws_clients: Arc::new(AtomicUsize::new(0)),
        chat_rate_limiter: Arc::new(DashMap::new()),
        server_start_time: Utc::now(),
        total_signals_generated: Arc::new(AtomicUsize::new(0)),
        http_client: reqwest::Client::builder()
            .user_agent("xau_scalper_ml/1.0")
            .build()
            .unwrap(),
        config: config.clone(),
        metrics,
    };

    // --- NEW: Load persisted sessions ---
    let sessions_dir = &config.paths.sessions_dir;
    if let Err(e) = fs::create_dir_all(sessions_dir.as_str()).await {
        tracing::error!("Failed to create sessions directory: {}", e);
    }

    if let Ok(mut entries) = fs::read_dir(sessions_dir.as_str()).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("json") {
                match fs::read_to_string(&path).await {
                    Ok(content) => {
                        match serde_json::from_str::<TradingSession>(&content) {
                            Ok(session) => {
                                tracing::info!("Loaded session for {} from disk.", session.symbol);
                                shared_state.session_manager.sessions.insert(session.symbol.clone(), Arc::new(Mutex::new(session)));
                            },
                            Err(e) => tracing::error!("Failed to deserialize session from {:?}: {}", path, e),
                        }
                    },
                    Err(e) => tracing::error!("Failed to read session file {:?}: {}", path, e),
                }
            }
        }
    }

    // --- Phase 2: Real-time Data Gateway ---

    // 1. Create a channel to broadcast live market data from MT5 to WebSocket clients.
    let (tick_tx, _) = broadcast::channel::<String>(100);

    // Add the tick_tx channel to the application state so the handler can access it.
    let app_state_with_ticks = Arc::new(shared_state.with_ticks(tick_tx));

    // --- Create Shutdown Channel ---
    let (shutdown_tx, _) = broadcast::channel::<()>(1);

    // --- Start Background Tasks ---
    background::spawn_stale_signal_cleanup_task(app_state_with_ticks.clone(), shutdown_tx.subscribe());
    background::spawn_history_cleanup_task(app_state_with_ticks.clone(), shutdown_tx.subscribe());
    background::spawn_news_fetch_task(app_state_with_ticks.clone(), shutdown_tx.subscribe());
    background::spawn_session_persistence_task(app_state_with_ticks.clone(), shutdown_tx.subscribe());

    // --- Start Background Analysis Task ---
    routes::start_background_analysis_task(app_state_with_ticks.clone(), shutdown_tx.subscribe());

    let app = Router::new()
        .merge(SwaggerUi::new("/docs").url("/api-docs/openapi.json", ApiDoc::openapi()))
        .route("/", get(handlers::system::health_check_handler))
        .route("/ws", get(handlers::ws::websocket_handler))
        .route("/metrics", get(handlers::system::metrics_handler))
        .route("/metrics/prometheus", get(handlers::system::prometheus_metrics_handler)) // NEW: Prometheus endpoint
        .route("/health", get(handlers::system::health_check_handler))
        .route("/data", post(handlers::trading::process_data_handler)) // For main analysis
        .route("/ticks", post(handlers::trading::tick_ingest_handler)) // NEW: For live ticks
        .route("/signals/latest", get(handlers::trading::get_all_latest_signals_handler))
        .route("/signals/:symbol", get(handlers::trading::get_latest_signals_handler))
        .route("/signals", get(handlers::trading::get_signals_handler))
        .route("/definitions/reasons", get(handlers::trading::get_signal_definitions_handler))
        // --- Add new routes for logging ---
        .route("/save-push-token", post(handlers::system::save_push_token_handler))
        .route("/settings", post(handlers::system::update_settings_handler).get(handlers::system::get_settings_handler))
        .route("/settings/reset", post(handlers::system::reset_settings_handler))
        .route("/news-guard/:symbol", get(handlers::trading::get_news_guard_status_handler))
        .route("/models/loaded", get(handlers::system::get_loaded_models_handler))
        // --- Analysis Endpoints ---
        .route("/daily-analysis", get(routes::daily_analysis))
        .route("/weekly-analysis", get(routes::weekly_analysis))
        .route("/chat-analysis", post(routes::chat_analysis_handler))
        // Provide the state to all handlers
        .with_state(app_state_with_ticks);

    let addr = format!("{}:{}", config.server.host, config.server.port);
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