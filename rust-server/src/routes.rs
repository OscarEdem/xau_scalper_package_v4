use crate::{state::ApplicationStateWithTicks, llm::gemini::generate_analysis, llm::prompts::MACRO_SYSTEM_PROMPT_V1, HistoricalSignal};
use xau_scalper_server::{NewsItem, CalendarEvent, SignalDirection};
use crate::services::trading::TradingService;
use xau_scalper_server::macro_analysis::builder::build_context;
use xau_scalper_server::macro_analysis::types::TechnicalSignal;
use xau_scalper_server::EvalResponse;
use axum::{
    extract::{Query, State},
    http::StatusCode,
    Json,
};
use chrono::{Datelike, Timelike, Utc};
use serde::Deserialize;
use tokio::sync::broadcast;
use tokio::time::Duration;
use utoipa::{IntoParams, ToSchema};
use std::collections::{hash_map::DefaultHasher, VecDeque};
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use tracing::info;

#[derive(Deserialize, IntoParams)]
pub struct SymbolQuery {
    symbol: String,
    #[serde(default)]
    force_refresh: bool,
}

#[derive(Deserialize, ToSchema)]
pub struct ChatRequest {
    #[schema(example = "XAUUSD")]
    pub symbol: String,
    #[schema(example = "daily")]
    pub period: String,
    #[schema(example = "What are the key risks mentioned in the report?")]
    pub query: String,
}

#[derive(Deserialize, IntoParams)]
pub struct PaginationQuery {
    #[serde(default = "default_page")]
    pub page: usize,
    #[serde(default = "default_limit")]
    pub limit: usize,
}

fn default_page() -> usize { 1 }
fn default_limit() -> usize { 50 }

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

#[utoipa::path(
    post, path = "/chat-analysis", request_body = ChatRequest,
    responses(
        (status = 200, description = "Returns the LLM response to the user query based on cached analysis", body = String),
        (status = 404, description = "No analysis found for the given symbol and period. Please run analysis first."),
        (status = 500, description = "Internal LLM error")
    )
)]
pub async fn chat_analysis_handler(
    State(state): State<Arc<ApplicationStateWithTicks>>,
    Json(req): Json<ChatRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    // --- Rate Limiting Logic ---
    // Limit: 5 requests per minute per symbol (simple keying by symbol for now)
    let limit_key = req.symbol.clone();
    let window_size = 60; // seconds
    let max_requests = 5;
    let now = Utc::now().timestamp();

    let mut allowed = false;
    {
        // DashMap entry API handles locking internally for the bucket
        let mut entry = state.inner.chat_rate_limiter.entry(limit_key).or_insert((0, now));
        let (count, window_start) = entry.value_mut();

        if now - *window_start > window_size {
            // Reset window
            *window_start = now;
            *count = 1;
            allowed = true;
        } else if *count < max_requests {
            // Increment count
            *count += 1;
            allowed = true;
        }
    }

    if !allowed {
        return Err(StatusCode::TOO_MANY_REQUESTS);
    }

    // Validate period
    if req.period != "daily" && req.period != "weekly" {
        return Err(StatusCode::BAD_REQUEST);
    }

    // --- NEW: Price Action Mini-Scribe ---
    let mut candle_scribe = String::new();
    if let Some(session_entry) = state.inner.session_manager.sessions.get(&req.symbol) {
        let session = session_entry.value().lock().await;
        let m5 = &session.market_data;
        let count = m5.m5_closes.len();
        let start = if count > 20 { count - 20 } else { 0 };
        for i in start..count {
            candle_scribe.push_str(&format!("H:{:.2},L:{:.2},C:{:.2};", 
                m5.m5_highs.get(i).unwrap_or(&0.0), 
                m5.m5_lows.get(i).unwrap_or(&0.0), 
                m5.m5_closes.get(i).unwrap_or(&0.0)));
        }
    }

    // --- NEW: Multi-Turn Contextual Memory ---
    let mut history_str = String::new();
    {
        if let Some(history) = state.inner.chat_sessions.get(&req.symbol) {
            for msg in history.value() {
                history_str.push_str(&format!("{}: {}\n", msg.role, msg.content));
            }
        }
    }

    let normalized_symbol = req.symbol.trim_end_matches('m').trim_end_matches(".pro").trim_end_matches(".k").to_string();
    let cache_key = format!("{}_{}", normalized_symbol, req.period);

    // 1. Retrieve cached analysis
    let cached_report = if let Some(entry) = state.inner.fundamental_analysis_cache.get(&cache_key) {
        entry.value().1.clone()
    } else {
        return Err(StatusCode::NOT_FOUND);
    };

    // 2. Construct Prompt
    let system_instruction = "You are an elite financial analyst. Answer questions based on the RECENT DATA and RECENT CANDLES. \
        Use the CHAT HISTORY for context. If you mention specific price levels, include them in the 'targets' array in the JSON response \
        so they can be drawn on the chart. Be concise, professional, and insight-driven.";
    
    let full_prompt = format!(
        "{}\n\nCHAT HISTORY:\n{}\nRECENT CANDLES (M5):\n{}\n\nUSER QUESTION: {}", 
        system_instruction, history_str, candle_scribe, req.query
    );

    // 3. Call LLM (Now returns structured JSON)
    match generate_analysis(&state.inner.http_client, &req.symbol, &cached_report, &full_prompt, &state.inner.metrics).await {
        Ok(structured_res) => {
            // Update history
            let mut history = state.inner.chat_sessions.entry(req.symbol.clone()).or_insert_with(|| VecDeque::with_capacity(10));
            history.push_back(crate::state::ChatMessage { 
                role: "user".to_string(), 
                content: req.query.clone(), 
                timestamp: Utc::now().timestamp() 
            });
            
            let model_text = structured_res["macro_narrative"].as_str().unwrap_or("").to_string();
            history.push_back(crate::state::ChatMessage { 
                role: "model".to_string(), 
                content: model_text, 
                timestamp: Utc::now().timestamp() 
            });
            
            if history.len() > 10 {
                history.pop_front();
            }

            // --- WS BROADCAST ---
            let service = TradingService::new(state.clone());
            service.broadcast_ai_update(structured_res.clone());

            Ok(Json(structured_res))
        },
        Err(e) => {
            tracing::error!("LLM Chat failed: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

#[utoipa::path(
    post, path = "/test-push",
    responses(
        (status = 200, description = "Test notification sent"),
        (status = 500, description = "Failed to send notification")
    )
)]
pub async fn test_push_handler(
    State(state): State<Arc<ApplicationStateWithTicks>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let service = TradingService::new(state.clone());
    
    let mut dummy_signal = EvalResponse::default();
    dummy_signal.signal_id = format!("TEST-{}", Utc::now().timestamp());
    dummy_signal.entry_type = SignalDirection::Long;
    dummy_signal.entry_price = 2000.0;
    dummy_signal.push_title = Some("🔔 Test Notification".to_string());
    dummy_signal.push_body = Some("This is a test signal to verify Expo integration.".to_string());
    dummy_signal.should_push = true;

    service.send_push_notification(&dummy_signal, "TEST-USD").await;

    Ok(Json(serde_json::json!({ "status": "sent", "signal_id": dummy_signal.signal_id })))
}

#[utoipa::path(
    delete, path = "/signals/clear",
    responses(
        (status = 200, description = "Database and memory cleared of signals"),
        (status = 500, description = "Failed to clear database")
    )
)]
pub async fn clear_database_handler(
    State(state): State<Arc<ApplicationStateWithTicks>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    // 1. Clear Database
    match crate::db::clear_all_signals(&state.inner.db, Some(&state.inner.metrics.db_retries_total)).await {
        Ok(count) => {
            // 2. Clear In-Memory History
            let mut history = state.inner.signal_history.lock().await;
            history.clear();
            
            info!("Manually cleared {} signals from database and memory.", count);
            Ok(Json(serde_json::json!({ "status": "success", "deleted_count": count })))
        },
        Err(e) => {
            tracing::error!("Failed to clear database: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

#[utoipa::path(
    get, path = "/signals/paginated", params(PaginationQuery),
    responses((status = 200, description = "Returns paginated historical signals"))
)]
pub async fn get_signals_paginated_handler(
    State(state): State<Arc<ApplicationStateWithTicks>>,
    Query(q): Query<PaginationQuery>,
) -> Json<serde_json::Value> {
    let history = state.inner.signal_history.lock().await;
    let start = (q.page - 1) * q.limit;
    let signals: Vec<HistoricalSignal> = history.iter().skip(start).take(q.limit).cloned().collect();
    
    Json(serde_json::json!({
        "page": q.page,
        "limit": q.limit,
        "total": history.len(),
        "signals": signals
    }))
}

#[utoipa::path(
    get, path = "/external/calendar",
    tag = "External Data",
    responses((status = 200, description = "Returns cached external economic calendar events", body = Vec<CalendarEvent>))
)]
pub async fn get_external_calendar_handler(
    State(state): State<Arc<ApplicationStateWithTicks>>,
) -> Json<Vec<CalendarEvent>> {
    let events = state.inner.external_calendar_events.lock().await.clone();
    Json(events)
}

#[utoipa::path(
    get, path = "/external/news",
    tag = "External Data",
    responses((status = 200, description = "Returns cached external RSS news items", body = Vec<NewsItem>))
)]
pub async fn get_external_news_handler(
    State(state): State<Arc<ApplicationStateWithTicks>>,
) -> Json<Vec<NewsItem>> {
    let news = state.inner.external_rss_news.lock().await.clone();
    Json(news)
}

/// Internal helper to generate fundamental analysis report
pub async fn generate_fundamental_report(
    state: Arc<ApplicationStateWithTicks>,
    symbol: &str,
    period: &str,
    force_refresh: bool,
) -> serde_json::Value {
    // --- Symbol Normalization ---
    let normalized_symbol = symbol.trim_end_matches('m').trim_end_matches(".pro").trim_end_matches(".k").to_string();
    let symbol = normalized_symbol.as_str();

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
            if signal.entry_type != SignalDirection::None {
                swing_signals_ctx.push(TechnicalSignal {
                    signal_type: signal.entry_type.to_string(),
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

    // --- Extract News Headlines (Top 10) ---
    let headlines: Vec<String> = state.inner.external_rss_news.lock().await
        .iter()
        .take(10)
        .map(|item| format!("[{}] {}", item.source, item.title))
        .collect();

    // --- Caching Logic ---
    let mut hasher = DefaultHasher::new();
    relevant_events.hash(&mut hasher);
    technical_levels.hash(&mut hasher);
    headlines.hash(&mut hasher);
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
    let context = build_context(symbol, period, &relevant_events, technical_levels, swing_signals_ctx, htf_bias_ctx, fvg_zones_ctx, headlines);

    // --- BRIDGE LAYER (Serialization) ---
    let mut context_map = serde_json::to_value(&context).unwrap_or(serde_json::json!({}));
    
    // Inject current price for visual grounding calibration
    if let Some(session_entry) = state.inner.session_manager.sessions.get(symbol) {
        let session = session_entry.value().lock().await;
        context_map["current_price"] = serde_json::json!(session.market_data.m5_closes.back());
    }
    
    let context_json = serde_json::to_string(&context_map).unwrap_or_else(|_| "{}".to_string());

    // --- LLM CALL ---
    let llm_response = generate_analysis(&state.inner.http_client, symbol, &context_json, MACRO_SYSTEM_PROMPT_V1, &state.inner.metrics)
        .await;

    let mut result: serde_json::Value = match llm_response {
        Ok(structured_res) => structured_res,
        Err(e) => serde_json::json!({
            "error": format!("Fundamental analysis failed: {}", e)
        }),
    };

    // --- MERGE DETERMINISTIC DATA ---
    // Inject the calculated global_risk_off score into the final JSON response
    if let Some(obj) = result.as_object_mut() {
        obj.insert("global_risk_off".to_string(), serde_json::json!(context.global_risk_off));
        
        // Post-LLM Scoring: Combine LLM confidence with Swing Engine score
        let llm_confidence = obj.get("confidence").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let final_conviction = if swing_conviction > 0.0 {
            (llm_confidence + (swing_conviction / 100.0)) / 2.0
        } else {
            llm_confidence
        };
        
        obj.insert("swing_engine_score".to_string(), serde_json::json!(swing_conviction));
        obj.insert("final_conviction".to_string(), serde_json::json!(final_conviction));
    }

    // --- Cache the new result ---
    tracing::info!("Caching new fundamental analysis for '{}'.", cache_key);
    state.inner.fundamental_analysis_cache.insert(cache_key, (events_hash, result.to_string()));
    
    // --- Save to Database ---
    let db_pool = &state.inner.db;
    if let Err(e) = crate::db::save_analysis_report(db_pool, symbol, period, &result, events_hash, Some(&state.inner.metrics.db_retries_total)).await {
        tracing::error!("Failed to save analysis report to DB: {}", e);
    }

    let final_response = serde_json::json!({
        "symbol": symbol,
        "period": period,
        "analysis": result,
        "source_events_count": relevant_events.len(),
        "source": "api",
    });

    // --- WS BROADCAST ---
    let service = TradingService::new(state.clone());
    service.broadcast_ai_update(final_response.clone());

    final_response
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