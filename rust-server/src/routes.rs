use crate::{state::ApplicationStateWithTicks, llm::gemini::generate_analysis, llm::prompts::MACRO_SYSTEM_PROMPT_V1};
use xau_scalper_server::macro_analysis::builder::build_context;
use xau_scalper_server::macro_analysis::types::TechnicalSignal;
use axum::{
    extract::{Query, State},
    Json,
};
use chrono::{Datelike, Timelike, Utc};
use serde::Deserialize;
use tokio::sync::broadcast;
use tokio::time::Duration;
use utoipa::IntoParams;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use tracing::info;

#[derive(Deserialize, IntoParams)]
pub struct SymbolQuery {
    symbol: String,
    #[serde(default)]
    force_refresh: bool,
}

#[utoipa::path(
    get, path = "/daily-analysis", params(SymbolQuery),
    responses((status = 200, description = "Returns a daily fundamental analysis (Last 24h context + Outlook)"))
)]
pub async fn daily_analysis(
    State(state): State<Arc<ApplicationStateWithTicks>>,
    Query(q): Query<SymbolQuery>,
) -> Json<serde_json::Value> {
    // Daily: Uses events from last 24h + upcoming 24h
    let result = generate_fundamental_report(state, &q.symbol, "daily", q.force_refresh).await;
    Json(result)
}

#[utoipa::path(
    get, path = "/weekly-analysis", params(SymbolQuery),
    responses((status = 200, description = "Returns a weekly fundamental analysis (Last 7d context + Outlook)"))
)]
pub async fn weekly_analysis(
    State(state): State<Arc<ApplicationStateWithTicks>>,
    Query(q): Query<SymbolQuery>,
) -> Json<serde_json::Value> {
    // Weekly: Uses all high-impact events in last 7 days + upcoming
    let result = generate_fundamental_report(state, &q.symbol, "weekly", q.force_refresh).await;
    Json(result)
}

/// Internal helper to generate fundamental analysis report
pub async fn generate_fundamental_report(
    state: Arc<ApplicationStateWithTicks>,
    symbol: &str,
    period: &str,
    force_refresh: bool,
) -> serde_json::Value {
    let all_events = state.inner.news_events.lock().await.clone();

    let now = Utc::now();

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
                // Daily: Context from last 24h and Outlook for next 24h
                event.timestamp >= (now.timestamp() - 86400) && event.timestamp <= (now.timestamp() + 86400)
            } else {
                // Weekly: Context from last 7 days and Outlook for next 7 days (bounded by fetcher's "this week")
                event.timestamp >= (now.timestamp() - 7 * 86400) && event.timestamp <= (now.timestamp() + 7 * 86400)
            }
        })
        .collect();

    // --- Extract Technical Levels from Session ---
    let mut technical_levels = Vec::new();
    let mut swing_signals_ctx = Vec::new();
    let mut htf_bias_ctx = "Neutral".to_string();
    let mut fvg_zones_ctx = Vec::new();
    let mut swing_conviction = 0.0;

    if let Some(session_entry) = state.inner.session_manager.sessions.get(symbol) {
        let session = session_entry.value().lock().await;
        // We use the swing signal for macro-relevant levels
        let (_, swing_signal) = session.get_latest_signals();
        
        if let Some(signal) = swing_signal {
            if !signal.liquidity_zones.is_empty() {
                for zone in signal.liquidity_zones {
                    let label = if zone.is_bullish.unwrap_or(false) { "Support" } else { "Resistance" };
                    technical_levels.push(format!("{} Zone: {:.2}-{:.2}", label, zone.bottom, zone.top));
                }
            }

            // Extract Swing Signals
            if signal.entry_type != "none" {
                swing_signals_ctx.push(TechnicalSignal {
                    signal_type: signal.entry_type.clone(),
                    confidence: signal.conviction_score.unwrap_or(0.0),
                    price_level: signal.entry_price,
                });
                swing_conviction = signal.conviction_score.unwrap_or(0.0);
            }

            // Extract HTF Bias
            if let Some(debug) = &signal.debug_info {
                if let Some(score_str) = debug.get("htf_bias_score") {
                    if let Ok(score) = score_str.parse::<f64>() {
                        if score > 0.1 { htf_bias_ctx = "Bullish".to_string(); }
                        else if score < -0.1 { htf_bias_ctx = "Bearish".to_string(); }
                    }
                }
            }

            // Extract FVG Zones
            for zone in &signal.imbalance_zones {
                 let label = if zone.is_bullish.unwrap_or(false) { "Bullish FVG" } else { "Bearish FVG" };
                 fvg_zones_ctx.push(format!("{}: {:.2}-{:.2}", label, zone.bottom, zone.top));
            }
        }
    }

    // --- Caching Logic ---
    // 1. Create a hash of the relevant events AND technical levels to see if they've changed.
    // We build the context first to hash it, or hash inputs. Hashing inputs is cheaper.
    let mut hasher = DefaultHasher::new();
    relevant_events.hash(&mut hasher);
    technical_levels.hash(&mut hasher);
    // Hash new context fields (using debug format for simplicity as they don't impl Hash)
    format!("{:?}", swing_signals_ctx).hash(&mut hasher);
    htf_bias_ctx.hash(&mut hasher);
    format!("{:?}", fvg_zones_ctx).hash(&mut hasher);
    let events_hash = hasher.finish();

    // 2. Create a unique key for this request.
    let cache_key = format!("{}_{}", symbol, period);

    // 3. Check the cache.
    if !force_refresh {
        if let Some(entry) = state.inner.fundamental_analysis_cache.get(&cache_key) {
            let (cached_hash, cached_report) = entry.value();
            if *cached_hash == events_hash {
                // The events haven't changed, so we can return the cached report.
                tracing::info!("Serving fundamental analysis for '{}' from cache.", cache_key);
                let analysis_json: serde_json::Value = serde_json::from_str(cached_report).unwrap_or(serde_json::Value::String(cached_report.clone()));
                return serde_json::json!({ "analysis": analysis_json, "source": "cache" });
            }
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

    // --- MACRO ENGINE (Deterministic) ---
    let context = build_context(symbol, period, &relevant_events, technical_levels, swing_signals_ctx, htf_bias_ctx, fvg_zones_ctx);

    // --- BRIDGE LAYER (Serialization) ---
    let context_json = serde_json::to_string(&context).unwrap_or_else(|_| "{}".to_string());

    // --- LLM CALL ---
    // We pass the context as the data payload, and the system prompt as the instruction.
    let llm_response = generate_analysis(&state.inner.http_client, symbol, &context_json, MACRO_SYSTEM_PROMPT_V1)
        .await;

    let mut result: serde_json::Value = match llm_response {
        Ok(json_str) => {
            // Attempt to clean the response if it contains markdown code blocks or extra text
            let cleaned_json = if let Some(start) = json_str.find('{') {
                if let Some(end) = json_str.rfind('}') {
                    &json_str[start..=end]
                } else {
                    &json_str
                }
            } else {
                &json_str
            };
            serde_json::from_str(cleaned_json).unwrap_or_else(|e| {
                serde_json::json!({
                    "error": "Failed to parse LLM JSON",
                    "details": e.to_string(),
                    "raw": json_str
                })
            })
        },
        Err(e) => serde_json::json!({
            "error": format!("Fundamental analysis failed: {}", e)
        }),
    };

    // --- MERGE DETERMINISTIC DATA ---
    // Inject the calculated global_risk_off score into the final JSON response
    if let Some(obj) = result.as_object_mut() {
        obj.insert("global_risk_off".to_string(), serde_json::json!(context.global_risk_off));
        
        // Post-LLM Scoring: Combine LLM confidence with Swing Engine score
        let llm_confidence = obj.get("confidence").and_then(|v| v.as_u64()).unwrap_or(0) as f64;
        let final_conviction = if swing_conviction > 0.0 {
            (llm_confidence + swing_conviction) / 2.0
        } else {
            llm_confidence
        };
        
        obj.insert("swing_engine_score".to_string(), serde_json::json!(swing_conviction));
        obj.insert("final_conviction".to_string(), serde_json::json!(final_conviction));
    }

    // --- Cache the new result ---
    tracing::info!("Caching new fundamental analysis for '{}'.", cache_key);
    state.inner.fundamental_analysis_cache.insert(cache_key, (events_hash, result.to_string()));

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
pub fn start_background_analysis_task(state: Arc<ApplicationStateWithTicks>, mut shutdown_rx: broadcast::Receiver<()>) {
    tokio::spawn(async move {
        info!("Background analysis scheduler started.");
        
        // Check every minute
        let mut interval = tokio::time::interval(Duration::from_secs(60));

        loop {
            tokio::select! {
                _ = shutdown_rx.recv() => {
                    info!("Background analysis scheduler shutting down.");
                    break;
                }
                _ = interval.tick() => {}
            }
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
                    let fund_report = generate_fundamental_report(state.clone(), &symbol, "daily", false).await;
                    info!("Scheduled Fundamental Daily Report generated for {}: {:?}", symbol, fund_report.as_object().and_then(|o| o.get("overall_bias")));
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
                    let fund_report = generate_fundamental_report(state.clone(), &symbol, "weekly", false).await;
                    info!("Scheduled Fundamental Weekly Report generated for {}: {:?}", symbol, fund_report.as_object().and_then(|o| o.get("overall_bias")));
                }
            }
        }
    });
}