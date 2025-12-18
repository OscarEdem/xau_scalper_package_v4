use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use std::sync::Arc;
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use chrono::Utc;
use xau_scalper_server::EvalRequest;
use xau_scalper_server::engines::news_guard::GuardResult;

use crate::state::{
    ApplicationStateWithTicks, ActiveSignal, HistoricalSignal, LatestSignalsForSymbol,
    SignalDefinitionsResponse, SignalReasonInfo
};

/// New handler to ingest a single tick via HTTP POST and broadcast it.
pub async fn tick_ingest_handler(
    State(state): State<Arc<ApplicationStateWithTicks>>,
    tick_json: String, // Axum can receive the raw body as a String
) -> StatusCode {
    state.inner.metrics.http_requests.inc();
    let service = crate::services::trading::TradingService::new(state);
    service.broadcast_tick(tick_json);
    StatusCode::OK
}

/// This is a trick for utoipa to document the raw JSON body of `tick_ingest_handler`.
#[utoipa::path(
    post,
    path = "/ticks",
    request_body(content = TickData, description = "A single market tick in JSON format", content_type = "application/json"),
    responses(
        (status = 200, description = "Tick received and broadcasted successfully"),
    ),
    tag = "Trading Signal API"
)]
#[allow(dead_code)]
pub async fn documented_tick_ingest_handler() {}

#[utoipa::path(
    get,
    path = "/definitions/reasons",
    responses(
        (status = 200, description = "Returns a dictionary of signal reasons and their explanations", body = SignalDefinitionsResponse)
    )
)]
/// Handler to return a dictionary of signal reasons and their explanations.
pub async fn get_signal_definitions_handler() -> Json<SignalDefinitionsResponse> {
    let mut definitions = HashMap::new();

    macro_rules! insert {
        ($key:expr, $expl:expr, $adv:expr) => {
            definitions.insert($key.to_string(), SignalReasonInfo {
                explanation: $expl.to_string(),
                advice: $adv.to_string(),
            });
        };
    }

    // Scalp Reasons
    insert!("HTF Bullish Bias", "Higher Timeframe (H1/H4) structure and moving averages indicate a dominant uptrend.", "Look for long entries. Short trades are counter-trend and riskier.");
    insert!("HTF Bearish Bias", "Higher Timeframe (H1/H4) structure and moving averages indicate a dominant downtrend.", "Look for short entries. Long trades are counter-trend and riskier.");
    insert!("Ensemble Bullish Bias", "A consensus of Machine Learning models (LSTM, GBM, Heston) predicts a price increase.", "Statistical probability favors upside. Good confluence for long setups.");
    insert!("Ensemble Bearish Bias", "A consensus of Machine Learning models (LSTM, GBM, Heston) predicts a price decrease.", "Statistical probability favors downside. Good confluence for short setups.");
    insert!("Bullish M5 Flow", "The 5-minute Kalman Filter slope is positive, indicating immediate bullish momentum.", "Price is moving up now. Supports trend-following entries.");
    insert!("Bearish M5 Flow", "The 5-minute Kalman Filter slope is negative, indicating immediate bearish momentum.", "Price is moving down now. Supports trend-following entries.");
    insert!("M1 Bullish Surge", "A rapid acceleration of price and volume detected on the 1-minute chart.", "Micro-timing signal. Often marks the exact moment of a breakout or reversal.");
    insert!("M1 Bearish Surge", "A rapid acceleration of selling pressure detected on the 1-minute chart.", "Micro-timing signal. Often marks the exact moment of a breakout or reversal.");
    insert!("Flow+Surge Confluence", "Alignment of 5-minute momentum (Flow) and 1-minute acceleration (Surge).", "High-probability momentum entry. The trend and timing are synchronized.");
    insert!("Bullish Inducement", "Price swept a recent low to trap sellers, then immediately reversed higher.", "Classic 'Stop Hunt' reversal. Enter long as trapped sellers are forced to cover.");
    insert!("Bearish Inducement", "Price swept a recent high to trap buyers, then immediately reversed lower.", "Classic 'Stop Hunt' reversal. Enter short as trapped buyers are forced to sell.");
    insert!("Fade Long", "Price has extended significantly below the mean (oversold) and is showing signs of exhaustion.", "Mean reversion trade. Expect a bounce back towards the average. Strict stop loss required.");
    insert!("Fade Short", "Price has extended significantly above the mean (overbought) and is showing signs of exhaustion.", "Mean reversion trade. Expect a pullback towards the average. Strict stop loss required.");
    insert!("London Open Stop-Hunt", "Price swept the Asian Session high/low during the London Open, a common institutional trap.", "High-probability reversal. Institutions are grabbing liquidity to fuel the real move.");
    insert!("Breakout_Add", "This is a 'Pyramiding' signal. The trend is strong, and we are adding to a winner.", "Add to Position. Only take this if your first trade is already in profit.");

    // Swing Reasons
    insert!("Bullish Liquidity Grab (SFP)", "Swing Failure Pattern. Price pierced a major support level but failed to close below it.", "Strong rejection. Buyers stepped in at value. Target the opposing liquidity.");
    insert!("Bearish Liquidity Grab (SFP)", "Swing Failure Pattern. Price pierced a major resistance level but failed to close above it.", "Strong rejection. Sellers stepped in at value. Target the opposing liquidity.");
    insert!("Bullish Displacement", "A high-volume, large-body candle that breaks through market structure upwards.", "Confirming sign of a trend change or continuation. 'Smart Money' is active.");
    insert!("Bearish Displacement", "A high-volume, large-body candle that breaks through market structure downwards.", "Confirming sign of a trend change or continuation. 'Smart Money' is active.");
    insert!("Bullish FVG Support", "Price is retesting a Fair Value Gap (inefficiency) created by a strong upward move.", "Limit entry zone. Price often fills these gaps before continuing the trend.");
    insert!("Bearish FVG Resistance", "Price is retesting a Fair Value Gap (inefficiency) created by a strong downward move.", "Limit entry zone. Price often fills these gaps before continuing the trend.");
    insert!("Re-Entry", "Price retraced to the breakeven/entry level of a valid setup and bounced.", "Second chance entry. Validates that the support/resistance level is holding.");

    // Blocking/Status
    insert!("Blocked: Volatility Spike Detected", "The market is moving abnormally fast (ATR Spike).", "Safety First. Algorithms are paused to prevent getting stopped out by noise/slippage.");
    insert!("Blocked: Low Trend Strength (ADX)", "The market is flat/ranging (ADX is very low).", "No Trade. Scalping strategies fail in flat markets. Wait for a breakout.");
    insert!("Filtered: Scalp signal conflicts with swing trend", "The Scalp engine wanted to trade, but the Swing trend is opposite.", "Trend Filter. We blocked a counter-trend trade to protect your capital.");
    insert!("No Signal (low conviction)", "The setup appeared but didn't reach the required confidence score.", "Patience. The setup wasn't 'A+' quality. Better to wait for a clearer setup.");
    insert!("No Signal (SL sanity check failed)", "The calculated Stop Loss was either dangerously tight or way too wide.", "Risk Management. The risk profile for this specific candle setup was unsafe.");

    Json(SignalDefinitionsResponse { definitions })
}

#[utoipa::path(
    get,
    path = "/news-guard/{symbol}",
    params(
        ("symbol" = String, Path, description = "The trading symbol, e.g., XAUUSD")
    ),
    responses(
        (status = 200, description = "Returns the current news guard status", body = GuardResult)
    )
)]
pub async fn get_news_guard_status_handler(
    State(state): State<Arc<ApplicationStateWithTicks>>,
    Path(symbol): Path<String>,
) -> Json<GuardResult> {
    let news_events_guard = state.inner.news_events.lock().await;
    let now = Utc::now().timestamp();
    
    let settings = state.inner.session_manager.settings.read().expect("Settings lock poisoned");
    // Using Scalp settings for the general status check
    let result = xau_scalper_server::engines::news_guard::evaluate_news_guard(&symbol, now, &*news_events_guard, settings.scalp.news_pre_event_block_minutes, settings.scalp.news_post_event_block_minutes);
    Json(result)
}

#[utoipa::path(
    get,
    path = "/signals",
    responses(
        (status = 200, description = "Returns a log of all signals generated in the last 12 hours", body = Vec<HistoricalSignal>)
    )
)]
/// Handler to return a log of all signals generated in the last 12 hours.
pub async fn get_signals_handler(State(state): State<Arc<ApplicationStateWithTicks>>) -> Json<Vec<HistoricalSignal>> {
    let history = state.inner.signal_history.lock().await;
    // Return a clone of the current history
    Json(history.iter().cloned().collect())
}

/// This handler now acts as the primary data ingress point.
#[utoipa::path(
    post,
    path = "/data",
    request_body = EvalRequest,
    responses(
        (status = 200, description = "Data processed successfully")
    )
)]
#[axum::debug_handler]
pub async fn process_data_handler(
    State(state): State<Arc<ApplicationStateWithTicks>>,
    Json(req): Json<EvalRequest<'static>>,
) -> Result<(StatusCode, Json<&'static str>), StatusCode> {
    state.inner.metrics.http_requests.inc();

    let symbol = req.symbol.to_string();

    // 1. Access settings to get filter flag
    let settings = state.inner.session_manager.settings.read().expect("Settings lock poisoned").clone();
    let filter_scalp = settings.scalp.filter_scalp_by_swing;

    // 2. Get or create session
    let session_arc = state.inner.session_manager.get_or_create_session(&symbol, filter_scalp);
    let mut session = session_arc.lock().await;

    // 3. Process data (updates session state for /signals/{symbol} and /signals/latest)
    let signals = session.on_data(req, &state.inner.predictor_cache, &settings);

    // 4. Save to history (for /signals)
    if !signals.is_empty() {
        let count = signals.len();
        state.inner.total_signals_generated.fetch_add(count, Ordering::SeqCst);
        state.inner.metrics.signal_counter.inc_by(count as f64);

        let mut history = state.inner.signal_history.lock().await;
        for signal in signals {
            let active_signal = ActiveSignal { symbol: symbol.clone(), signal };
            let historical_signal = HistoricalSignal { signal: active_signal, created_at: Utc::now().timestamp() };
            history.push_front(historical_signal);
        }
    }

    Ok((StatusCode::OK, Json("Data processed")))
}

#[utoipa::path(
    get,
    path = "/signals/{symbol}",
    params(
        ("symbol" = String, Path, description = "The trading symbol, e.g., XAUUSD")
    ),
    responses(
        (status = 200, description = "Returns the latest scalp and swing signals for the symbol", body = LatestSignalsForSymbol),
        (status = 404, description = "No session found for symbol")
    )
)]
/// Handler to return the last generated signals for a specific symbol.
pub async fn get_latest_signals_handler(
    State(state): State<Arc<ApplicationStateWithTicks>>,
    Path(symbol): Path<String>,
) -> Result<Json<LatestSignalsForSymbol>, StatusCode> {
    let sessions = &state.inner.session_manager.sessions;

    if let Some(session_arc) = sessions.get(&symbol) {
        let session = session_arc.lock().await;
        let (scalp_signal, swing_signal) = session.get_latest_signals();
        Ok(Json(LatestSignalsForSymbol {
            symbol: symbol.clone(),
            scalp_signal,
            swing_signal,
        }))
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}

#[utoipa::path(
    get,
    path = "/signals/latest",
    responses(
        (status = 200, description = "Returns the latest signals for all active symbols", body = Vec<LatestSignalsForSymbol>)
    )
)]
/// Handler to return the latest signals for all active symbols.
pub async fn get_all_latest_signals_handler(
    State(state): State<Arc<ApplicationStateWithTicks>>,
) -> Json<Vec<LatestSignalsForSymbol>> {
    let sessions = &state.inner.session_manager.sessions;

    let mut all_signals = Vec::new();

    for r in sessions.iter() {
        let symbol = r.key();
        let session_arc = r.value();
        let session = session_arc.lock().await;
        let (scalp_signal, swing_signal) = session.get_latest_signals();
        
        // Only include symbols that have generated at least one signal
        if scalp_signal.is_some() || swing_signal.is_some() {
            all_signals.push(LatestSignalsForSymbol {
                symbol: symbol.clone(),
                scalp_signal,
                swing_signal,
            });
        }
    }

    Json(all_signals)
}