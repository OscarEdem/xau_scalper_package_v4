use crate::{ApplicationStateWithTicks, gemini::generate_analysis};
use axum::{
    extract::{Query, State},
    Json,
};
use chrono::{Datelike, Timelike, Utc};
use serde::Deserialize;
use tokio::time::Duration;
use utoipa::IntoParams;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use tracing::info;

#[derive(Deserialize, IntoParams)]
pub struct FundamentalQuery {
    symbol: String,
    /// "daily" or "weekly"
    #[serde(default = "default_period")]
    period: String,
}

fn default_period() -> String {
    "daily".to_string()
}

#[utoipa::path(
    get, path = "/analysis/fundamental", params(FundamentalQuery),
    responses((status = 200, description = "Returns a fundamental analysis report from the AI based on upcoming news events"))
)]
pub async fn fundamental_analysis(
    State(state): State<Arc<ApplicationStateWithTicks>>,
    Query(q): Query<FundamentalQuery>,
) -> Json<serde_json::Value> {
    let result = generate_fundamental_report(state, &q.symbol, &q.period).await;
    Json(result)
}

/// Internal helper to generate fundamental analysis report
pub async fn generate_fundamental_report(
    state: Arc<ApplicationStateWithTicks>,
    symbol: &str,
    period: &str,
) -> serde_json::Value {
    let all_events = state.inner.news_events.lock().unwrap().clone();

    let now = Utc::now();
    let end_of_day = now.with_hour(23).unwrap().with_minute(59).unwrap().with_second(59).unwrap();

    // Extract base and quote from symbol (e.g., XAUUSD -> XAU, USD)
    let base = if symbol.len() >= 3 { &symbol[0..3] } else { "" };
    let quote = if symbol.len() >= 6 { &symbol[3..6] } else { "" };

    // Filter events based on the requested period
    let relevant_events: Vec<_> = all_events
        .into_iter()
        .filter(|event| {
            // Filter by currency (Base OR Quote)
            if event.currency != base && event.currency != quote {
                return false;
            }
            if period == "daily" {
                event.timestamp >= now.timestamp() && event.timestamp <= end_of_day.timestamp()
            } else {
                // "weekly" includes all cached future events
                event.timestamp >= now.timestamp()
            }
        })
        .collect();

    // --- Extract Technical Levels from Session ---
    let mut technical_levels_str = String::new();
    if let Some(session_entry) = state.inner.session_manager.sessions.get(symbol) {
        let session = session_entry.value().lock().unwrap();
        // We use the swing signal for macro-relevant levels
        let (_, swing_signal) = session.get_latest_signals();
        
        if let Some(signal) = swing_signal {
            if !signal.liquidity_zones.is_empty() {
                technical_levels_str.push_str("\nPROVIDED TECHNICAL LEVELS (Swing):\n");
                for zone in signal.liquidity_zones {
                    let label = if zone.is_bullish.unwrap_or(false) { "Support" } else { "Resistance" };
                    technical_levels_str.push_str(&format!("- {} Zone: {:.2}-{:.2}\n", label, zone.bottom, zone.top));
                }
            }
        }
    }

    // --- Caching Logic ---
    // 1. Create a hash of the relevant events AND technical levels to see if they've changed.
    let mut hasher = DefaultHasher::new();
    relevant_events.hash(&mut hasher);
    technical_levels_str.hash(&mut hasher);
    let events_hash = hasher.finish();

    // 2. Create a unique key for this request.
    let cache_key = format!("{}_{}", symbol, period);

    // 3. Check the cache.
    if let Some(entry) = state.inner.fundamental_analysis_cache.get(&cache_key) {
        let (cached_hash, cached_report) = entry.value();
        if *cached_hash == events_hash {
            // The events haven't changed, so we can return the cached report.
            tracing::info!("Serving fundamental analysis for '{}' from cache.", cache_key);
            return serde_json::json!({ "analysis": cached_report, "source": "cache" });
        }
    }
    // --- End Caching Logic ---

    // If no events are found, return a default response instead of calling the AI.
    if relevant_events.is_empty() {
        return serde_json::json!({
            "symbol": symbol,
            "period": period,
            "analysis": "No high-impact news events found for this period. Market likely driven by technicals.",
            "source_events_count": 0,
            "source": "api",
        });
    }

    let events_json = serde_json::to_string(&relevant_events).unwrap_or_else(|_| "[]".to_string());

    // Embed the prompt at compile time
    const PROMPT: &str = include_str!("fundamental_report_prompt.txt");

    // Inject the requested period and technical levels into the prompt
    let dynamic_prompt = format!(
        "ANALYSIS HORIZON: {} OUTLOOK\n\n{}\n\n{}", 
        period.to_uppercase(), 
        PROMPT,
        technical_levels_str
    );

    let result = generate_analysis(&state.inner.http_client, symbol, &events_json, &dynamic_prompt)
        .await
        .unwrap_or_else(|e| format!("Fundamental analysis failed: {}", e));

    // --- Cache the new result ---
    tracing::info!("Caching new fundamental analysis for '{}'.", cache_key);
    state.inner.fundamental_analysis_cache.insert(cache_key, (events_hash, result.clone()));

    serde_json::json!({
        "symbol": symbol,
        "period": period,
        "analysis": result,
        "source_events_count": relevant_events.len(),
        "source": "api",
    })
}

/// Starts a background task that runs analysis at specific times.
/// Call this from main.rs after creating the application state.
pub fn start_background_analysis_task(state: Arc<ApplicationStateWithTicks>) {
    tokio::spawn(async move {
        info!("Background analysis scheduler started.");
        
        // Check every minute
        let mut interval = tokio::time::interval(Duration::from_secs(60));

        loop {
            interval.tick().await;
            let now = Utc::now();

            // 1. Daily Analysis at 00:00 UTC
            if now.hour() == 0 && now.minute() == 0 {
                info!("Starting scheduled Daily Analysis...");
                
                // Get active symbols from session manager
                let symbols: Vec<String> = {
                    let sessions = &state.inner.session_manager.sessions;
                    sessions.iter().map(|r| r.key().clone()).collect()
                };

                for symbol in symbols {
                    // Fundamental Daily
                    let fund_report = generate_fundamental_report(state.clone(), &symbol, "daily").await;
                    info!("Scheduled Fundamental Daily Report generated for {}: {:?}", symbol, fund_report.get("overall_bias"));
                }
            }

            // 2. Weekly Analysis at Market Open (Sunday 22:00 UTC approx)
            if now.weekday() == chrono::Weekday::Sun && now.hour() == 22 && now.minute() == 0 {
                info!("Starting scheduled Weekly Analysis (Market Open)...");

                // Get active symbols from session manager
                let symbols: Vec<String> = {
                    let sessions = &state.inner.session_manager.sessions;
                    sessions.iter().map(|r| r.key().clone()).collect()
                };

                for symbol in symbols {
                    // Fundamental Weekly
                    let fund_report = generate_fundamental_report(state.clone(), &symbol, "weekly").await;
                    info!("Scheduled Fundamental Weekly Report generated for {}: {:?}", symbol, fund_report.get("overall_bias"));
                }
            }
        }
    });
}