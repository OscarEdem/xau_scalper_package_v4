use prometheus::{Counter, CounterVec, Gauge};

#[derive(Clone, Debug)]
pub struct AppMetrics {
    pub signal_counter: Counter,
    pub active_sessions: Gauge,
    pub active_ws_clients: Gauge,
    pub http_requests: Counter,
    pub gemini_429_errors: Counter,
    pub gemini_success_model: CounterVec,
    pub db_retries_total: Counter,
}