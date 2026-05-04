use std::collections::{BTreeSet, HashMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use tokio::sync::{Mutex, broadcast};
use dashmap::DashMap;
use xau_scalper_server::{EvalResponse, NewsEvent, SessionManager, NewsItem, CalendarEvent, ActiveSignal, HistoricalSignal};
use xau_scalper_server::config::Settings;
use xau_scalper_server::metrics::AppMetrics;


#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct ChatMessage {
    pub role: String, // "user" or "model"
    pub content: String,
    pub timestamp: i64,
}

/// A more flexible state for the whole application, including the new SessionManager.
#[derive(Clone)]
pub struct ApplicationState {
    pub session_manager: SessionManager,
    pub predictor_cache: xau_scalper_server::engines::predictor_cache::PredictorCache,
    pub signal_history: Arc<Mutex<VecDeque<HistoricalSignal>>>,
    pub push_tokens: Arc<Mutex<BTreeSet<String>>>,
    pub news_events: Arc<Mutex<Vec<NewsEvent>>>,
    pub external_calendar_events: Arc<Mutex<Vec<CalendarEvent>>>,
    pub external_rss_news: Arc<Mutex<Vec<NewsItem>>>,
    /// Cache for fundamental analysis reports. Key: "PAIR_period", Value: (events_hash, report)
    pub fundamental_analysis_cache: Arc<DashMap<String, (u64, String)>>,
    pub ws_clients: Arc<AtomicUsize>,
    // NEW: Metrics
    /// Rate limiter for chat requests: Key = IP/User/Symbol, Value = (count, window_start_timestamp)
    pub chat_rate_limiter: Arc<DashMap<String, (u32, i64)>>,
    /// Chat sessions for multi-turn context. Key: Symbol
    pub chat_sessions: Arc<DashMap<String, VecDeque<ChatMessage>>>,
    pub server_start_time: chrono::DateTime<chrono::Utc>,
    pub total_signals_generated: Arc<AtomicUsize>,
    pub http_client: reqwest::Client,
    pub config: Settings,
    pub metrics: AppMetrics,
    pub db: sqlx::PgPool,
}

// We need to add the broadcast sender to our application state
#[derive(Clone)]
pub struct ApplicationStateWithTicks {
    pub inner: ApplicationState,
    pub tick_tx: broadcast::Sender<String>,
}

impl ApplicationState {
    pub fn with_ticks(self, tick_tx: broadcast::Sender<String>) -> ApplicationStateWithTicks {
        ApplicationStateWithTicks { inner: self, tick_tx }
    }
}

#[derive(serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LatestSignalsForSymbol {
    pub symbol: String,
    pub scalp_signal: Option<EvalResponse>,
    pub swing_signal: Option<EvalResponse>,
}

/// Request body for saving a push notification token.
#[derive(serde::Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SavePushTokenRequest {
    pub token: String,
}

/// Represents a single market tick.
#[derive(serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TickData {
    pub symbol: String,
    pub bid: f64,
    pub ask: f64,
    pub timestamp: i64,
}

#[derive(serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SignalReasonInfo {
    pub explanation: String,
    pub advice: String,
}

#[derive(serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SignalDefinitionsResponse {
    pub definitions: HashMap<String, SignalReasonInfo>,
}

/// Server performance and usage metrics.
#[derive(serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MetricsResponse {
    pub uptime_seconds: i64,
    pub total_signals_generated: usize,
    pub active_sessions: usize,
    pub active_websockets: usize,
}