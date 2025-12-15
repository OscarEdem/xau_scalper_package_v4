use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use std::sync::Arc;
use std::collections::HashMap;
use chrono::Utc;
use xau_scalper_server::EvalRequest;
use xau_scalper_server::engines::news_guard::GuardResult;

use crate::state::{
    ApplicationStateWithTicks, HistoricalSignal, LatestSignalsForSymbol,
    SignalDefinitionsResponse, SignalReasonInfo
};

/// New handler to ingest a single tick via HTTP POST and broadcast it.
pub async fn tick_ingest_handler(
    State(state): State<Arc<ApplicationStateWithTicks>>,
    tick_json: String, // Axum can receive the raw body as a String
) -> StatusCode {
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
    insert!("HTF Bullish Bias", "The Higher Timeframes (H1/H4) are trending Up.", "Trade with confidence. Aligning with the big trend increases win rate.");
    insert!("HTF Bearish Bias", "The Higher Timeframes (H1/H4) are trending Down.", "Trade with confidence. Aligning with the big trend increases win rate.");
    insert!("Ensemble Bullish Bias", "Our AI models (LSTM, GBM, Heston) collectively predict price rising.", "High Confidence. Mathematical models agree with the technicals.");
    insert!("Ensemble Bearish Bias", "Our AI models (LSTM, GBM, Heston) collectively predict price falling.", "High Confidence. Mathematical models agree with the technicals.");
    insert!("Bullish M5 Flow", "The 5-minute momentum (Kalman Filter) is sloping upwards.", "Momentum Entry. Price is currently moving in your favor.");
    insert!("Bearish M5 Flow", "The 5-minute momentum (Kalman Filter) is sloping downwards.", "Momentum Entry. Price is currently moving in your favor.");
    insert!("M1 Bullish Surge", "A sudden burst of buying volume/speed detected on the 1-minute chart.", "Precision Timing. This confirms the exact moment to enter.");
    insert!("M1 Bearish Surge", "A sudden burst of selling volume/speed detected on the 1-minute chart.", "Precision Timing. This confirms the exact moment to enter.");
    insert!("Bullish Inducement", "Price swept a recent low to trap sellers, then reversed up.", "Reversal Trade. Expect a fast move away from the trap. Use a tighter Stop Loss.");
    insert!("Bearish Inducement", "Price swept a recent high to trap buyers, then reversed down.", "Reversal Trade. Expect a fast move away from the trap. Use a tighter Stop Loss.");
    insert!("Breakout_Add", "This is a 'Pyramiding' signal. The trend is strong, and we are adding to a winner.", "Add to Position. Only take this if your first trade is already in profit.");

    // Swing Reasons
    insert!("Bullish Liquidity Grab (SFP)", "Swing Failure Pattern. Price pierced a major support level but closed back above it.", "Strong Reversal. Institutions bought the lows. Target the next high.");
    insert!("Bearish Liquidity Grab (SFP)", "Swing Failure Pattern. Price pierced a major resistance level but closed back below it.", "Strong Reversal. Institutions sold the highs. Target the next low.");
    insert!("Bullish Displacement", "A large, strong green candle broke market structure.", "Trend Start. This indicates 'Smart Money' has entered the market with intent.");
    insert!("Bearish Displacement", "A large, strong red candle broke market structure.", "Trend Start. This indicates 'Smart Money' has entered the market with intent.");
    insert!("Bullish FVG Support", "Price is reacting off a 'Fair Value Gap' (Imbalance) created by buyers.", "Limit Entry. These gaps often act as magnets and then trampolines for price.");
    insert!("Bearish FVG Resistance", "Price is reacting off a 'Fair Value Gap' (Imbalance) created by sellers.", "Limit Entry. These gaps often act as magnets and then ceilings for price.");

    // Blocking/Status
    insert!("Blocked: Volatility Spike Detected", "The market is moving abnormally fast (ATR Spike).", "Safety First. Algorithms are paused to prevent getting stopped out by noise.");
    insert!("Blocked: Low Trend Strength (ADX)", "The market is flat/ranging (ADX is very low).", "No Trade. Scalping strategies fail in flat markets. Wait for a breakout.");
    insert!("Filtered: Scalp signal conflicts with swing trend", "The Scalp engine wanted to trade, but the Swing trend is opposite.", "Trend Filter. We blocked a counter-trend trade to protect your capital.");
    insert!("No Signal (low conviction)", "The setup appeared but didn't reach the required confidence score (e.g., < 45%).", "Patience. The setup wasn't 'A+' quality. Better to wait for a clearer setup.");
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
    let service = crate::services::trading::TradingService::new(state);
    service.process_eval_request(req).await?;
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