use axum::{
    extract::State,
    http::StatusCode,
    Json,
};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use chrono::Utc;
use prometheus::{Encoder, TextEncoder};
use xau_scalper_server::config::TradingSettings;

use crate::state::{ApplicationStateWithTicks, MetricsResponse, SavePushTokenRequest};

#[utoipa::path(
    get,
    path = "/health",
    responses((status = 200, description = "Server is running"))
)]
/// Simple health check endpoint.
pub async fn health_check_handler() -> &'static str { "OK" }

#[utoipa::path(
    get,
    path = "/metrics",
    responses(
        (status = 200, description = "Returns server performance and usage metrics", body = MetricsResponse)
    )
)]
/// Handler to return server performance and usage metrics.
pub async fn metrics_handler(
    State(state): State<Arc<ApplicationStateWithTicks>>,
) -> Json<MetricsResponse> {
    let metrics = MetricsResponse {
        uptime_seconds: (Utc::now() - state.inner.server_start_time).num_seconds(),
        total_signals_generated: state.inner.total_signals_generated.load(Ordering::SeqCst),
        active_sessions: state.inner.session_manager.sessions.len(),
        active_websockets: state.inner.ws_clients.load(Ordering::SeqCst),
    };
    Json(metrics)
}

#[utoipa::path(
    get,
    path = "/metrics/prometheus",
    responses(
        (status = 200, description = "Returns Prometheus metrics", body = String, content_type = "text/plain")
    )
)]
/// Handler to return Prometheus metrics.
pub async fn prometheus_metrics_handler() -> String {
    let encoder = TextEncoder::new();
    let metric_families = prometheus::gather();
    let mut buffer = vec![];
    encoder.encode(&metric_families, &mut buffer).unwrap();
    String::from_utf8(buffer).unwrap()
}

#[utoipa::path(
    get,
    path = "/settings",
    responses(
        (status = 200, description = "Returns current trading settings", body = TradingSettings)
    )
)]
pub async fn get_settings_handler(
    State(state): State<Arc<ApplicationStateWithTicks>>,
) -> Json<TradingSettings> {
    let settings = state.inner.session_manager.settings.read().expect("Settings lock poisoned").clone();
    Json(settings)
}

#[utoipa::path(
    post,
    path = "/settings",
    request_body = TradingSettings,
    responses(
        (status = 200, description = "Settings updated successfully", body = String),
        (status = 400, description = "Invalid settings provided", body = String)
    )
)]
pub async fn update_settings_handler(
    State(state): State<Arc<ApplicationStateWithTicks>>,
    Json(settings): Json<TradingSettings>,
) -> (StatusCode, Json<String>) {
    if let Err(e) = settings.validate() {
        tracing::warn!("Invalid settings update rejected: {}", e);
        return (StatusCode::BAD_REQUEST, Json(format!("Invalid settings: {}", e)));
    }
    tracing::info!("Received request to update trading settings.");
    state.inner.session_manager.update_settings(settings.clone()).await;

    (StatusCode::OK, Json("Settings updated successfully".to_string()))
}

#[utoipa::path(
    post,
    path = "/settings/reset",
    responses(
        (status = 200, description = "Settings reset to base config file successfully, undoing any API changes.", body = String),
        (status = 500, description = "Failed to reset settings.", body = String)
    )
)]
pub async fn reset_settings_handler(
    State(state): State<Arc<ApplicationStateWithTicks>>,
) -> (StatusCode, Json<String>) {
    tracing::info!("Received request to reset trading settings to base config file.");

    // 2. Reload settings from the base config files (e.g., config.toml or /etc/secrets/config.toml).
    // This re-runs the logic in Settings::new() which will now not find the override file.
    match xau_scalper_server::config::Settings::new() {
        Ok(base_config) => {
            // 3. Update the in-memory settings with the reloaded ones.
            state.inner.session_manager.update_settings(base_config.trading).await;
            tracing::info!("Successfully reloaded settings from base config file.");
            (StatusCode::OK, Json("Settings reset to base config file successfully".to_string()))
        }
        Err(e) => {
            tracing::error!("Failed to reload base configuration after deleting override: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, Json(format!("Failed to reload base configuration: {}", e)))
        }
    }
}

#[utoipa::path(
    get,
    path = "/models/loaded",
    responses(
        (status = 200, description = "Returns a list of currently loaded predictor models", body = Vec<String>)
    )
)]
#[axum::debug_handler]
pub async fn get_loaded_models_handler(
    State(state): State<Arc<ApplicationStateWithTicks>>,
) -> Json<Vec<String>> {
    let keys = state.inner.predictor_cache.loaded_keys();
    Json(keys)
}