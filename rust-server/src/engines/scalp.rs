use crate::{EvalRequest, EvalResponse, PriceLevel};
use ::uuid::Uuid;
use crate::{adx, get_trend_bias, find_imbalance_zones, find_swing_points, engines::{news_guard, predictor_cache::PredictorCache, ensemble_predictor}};
use tracing::debug;

pub struct ScalpEngine;

impl ScalpEngine {
    pub fn evaluate(req: &EvalRequest, predictor_cache: &PredictorCache) -> EvalResponse {
        // ---- SAFETY: ensure enough data ----
        let m5_closes = &req.m5_closes;
        let m1_closes = &req.closes; // m1 for triggers
        let n_m5 = m5_closes.len();
        let n_m1 = m1_closes.len();

        if n_m5 < 60 || n_m1 < 60 {
            return EvalResponse { reason: "Insufficient Data".to_string(), ..Default::default() };
        }

        // ---- Volatility: ATR on M5 (used to normalize thresholds) ----
        let atr_vals = crate::atr(&req.m5_highs, &req.m5_lows, m5_closes, 14);
        let last_atr = atr_vals.last().cloned().unwrap_or(1.0).max(0.0001);

        // ---- Guard: Check for high-risk news or volatility before proceeding ----
        let adx_vals = adx(&req.m5_highs, &req.m5_lows, m5_closes, 14);
        let guard = news_guard::combined_guard(
            req.last_m1_timestamp,
            &req.upcoming_events.clone().unwrap_or_default(),
            30,   // pre-news block (minutes)
            15,   // post-news block
            &atr_vals,
            &adx_vals,
            2.5, // ATR spike multiplier
            req.adx_threshold.unwrap_or(10.0) // ADX threshold (configurable)
        );

        if !guard.allowed {
            return EvalResponse { reason: guard.reason.unwrap(), ..Default::default() };
        }

        // ---- Context bias: prefer H4 then H1 fallback ----
        let h4_bias = if let Some(h4_closes) = &req.h4_closes {
            get_trend_bias(h4_closes, 40)
        } else { 0 };
        let h1_bias = if let Some(h1_closes) = &req.h1_closes {
            get_trend_bias(h1_closes, 20)
        } else { 0 };
        // More stable bias: prefer h4 if present, else h1, else neutral
        let bias = if h4_bias != 0 { h4_bias } else { h1_bias };

        // ---- Kalman slope: compute and normalize by ATR ----
        let kf_q = req.kf_process_noise.unwrap_or(0.01);
        let kf_r = req.kf_measurement_noise.unwrap_or(0.1);
        let (k_est, k_slope) = crate::kalman_slope(m5_closes, 20, kf_q, kf_r);

        // Normalized slope: slope per ATR unit (makes threshold adaptive to regime)
        let norm_k_slope = k_slope / last_atr;

        // ---- Inducement detection (trust but verify): return enum-like & score ----
        // Assume detect_inducement returns ("bullish_inducement"/"bearish_inducement"/"none")
        let inducement = crate::detect_inducement(&req.m5_highs, &req.m5_lows, m5_closes, 5);
        let inducement_score = match inducement.as_str() {
            "bullish_inducement" | "bearish_inducement" => 1.0,
            _ => 0.0
        };

        // ---- Micro timing trigger: use normalized M1 ROC + structure break ----
        let m1_roc = crate::roc(m1_closes, 3);
        let m1_surge = m1_roc.last().cloned().unwrap_or(0.0);
        // Normalize m1 surge to ATR measured on M1 equivalent: scale ATR by (1/5)
        let m1_atr_equivalent = last_atr / 5.0;
        let norm_m1_surge = if m1_atr_equivalent > 0.0 { m1_surge / m1_atr_equivalent } else { 0.0 };

        // ---- Dynamic thresholds (use percentiles or multiplicative factors rather than hard constants) ----
        // These constants are starting points; calibration should tune them.
        let kalman_slope_threshold = 0.12; // normalized slope units (slope/ATR)
        let m1_surge_threshold = 0.9; // normalized units (roughly 0.9 * m1_atr_equivalent)
        // If market extremely quiet, scale thresholds down, else up
        let vol_regime = (last_atr / 0.5).clamp(0.5, 3.0); // baseline ATR ~0.5 (adjust for your price scale)
        let kalman_slope_threshold = kalman_slope_threshold * vol_regime;
        let m1_surge_threshold = m1_surge_threshold * vol_regime;

        // ---- Signal scoring (probabilistic combination) ----
        // Build feature scores (0..1) then map to conviction via logistic mapping.
        // Feature 1: normalized kalman strength (zero-centered)
        let kalman_score = (norm_k_slope.abs() / (kalman_slope_threshold * 2.0)).clamp(0.0, 1.0);
        // Feature 2: inducement presence (binary) with small weight
        let inducement_score = inducement_score; // 0 or 1
        // Feature 3: normalized m1 surge
        let m1_score = (norm_m1_surge.abs() / (m1_surge_threshold * 2.0)).clamp(0.0, 1.0);

        // Direction candidates
        let is_bull_flow = norm_k_slope > (kalman_slope_threshold * 0.5);
        let is_bear_flow = norm_k_slope < -(kalman_slope_threshold * 0.5);
        let is_bull_m1 = norm_m1_surge > (m1_surge_threshold * 0.5);
        let is_bear_m1 = norm_m1_surge < -(m1_surge_threshold * 0.5);

        // Compose a score (weights chosen conservatively)
        let mut long_reasons = Vec::new();
        let mut long_score = 0.0;
        let mut short_reasons = Vec::new();
        let mut short_score = 0.0;

        // Bias weighting: if higher timeframe bias exists, give it weight
        if bias > 0 {
            long_score += 0.2;
            long_reasons.push("HTF Bullish Bias");
        } else if bias < 0 {
            short_score += 0.2;
            short_reasons.push("HTF Bearish Bias");
        }

        // ---- NEW: Ensemble Prediction Model Bias ----
        let final_prediction_bias = ensemble_predictor::calculate_bias(
            predictor_cache,
            "m5",
            m5_closes,
            req.current_price,
            1.0 / 60.0, // Scalp prediction for next minute
        );
        debug!(bias = final_prediction_bias, "Scalp ensemble prediction calculated");

        if final_prediction_bias > 0.0 {
            long_score += 0.15 * (final_prediction_bias / last_atr).clamp(0.0, 1.5); // Add a slightly higher weight for the ensemble
            long_reasons.push("Ensemble Bullish Bias");
        } else {
            short_score += 0.15 * (final_prediction_bias.abs() / last_atr).clamp(0.0, 1.5);
            short_reasons.push("Ensemble Bearish Bias");
        }

        // Kalman flow contributes to continuation score
        if is_bull_flow {
            long_score += kalman_score * 0.5;
            long_reasons.push("Bullish M5 Flow");
        }
        if is_bear_flow {
            short_score += kalman_score * 0.5;
            short_reasons.push("Bearish M5 Flow");
        }

        // M1 surge gives timing confirmation
        if is_bull_m1 {
            long_score += m1_score * 0.35;
            long_reasons.push("M1 Bullish Surge");
        }
        if is_bear_m1 {
            short_score += m1_score * 0.35;
            short_reasons.push("M1 Bearish Surge");
        }

        // Inducement (reversal) adds weight
        if inducement.as_str() == "bullish_inducement" {
            long_score += 0.6 * inducement_score; // inducement_score is 1.0 here
            long_reasons.push("Bullish Inducement");
            short_score *= 0.5; // Reduce opposing score
        } else if inducement.as_str() == "bearish_inducement" {
            short_score += 0.6 * inducement_score; // inducement_score is 1.0 here
            short_reasons.push("Bearish Inducement");
            long_score *= 0.5; // Reduce opposing score
        }

        // Map raw scores [0..~1.5] to conviction % using a conservative scaler
        let conv_long = (1.0 / (1.0 + (-6.0 * (long_score - 0.6)).exp())) * 100.0; // logistic mapping
        let conv_short = (1.0 / (1.0 + (-6.0 * (short_score - 0.6)).exp())) * 100.0;

        // Decide side and final reason if above minimum conviction threshold
        let min_conv_to_trade = 45.0; // tuned conservatively
        let (entry_type, conviction, reason) = if conv_long >= min_conv_to_trade && conv_long > conv_short {
            ("long".to_string(), conv_long, long_reasons.join(" + "))
        } else if conv_short >= min_conv_to_trade && conv_short > conv_long {
            ("short".to_string(), conv_short, short_reasons.join(" + "))
        } else {
            return EvalResponse { reason: "No Signal (low conviction)".to_string(), ..Default::default() };
        };

        // ---- Execution Decision: market vs limit with safety ----
        // If inducement present, prefer market entry (reversal). Otherwise, use limit near kalman estimate.
        let (recommended_order_type, entry_price) = if inducement_score > 0.0 {
            ("market".to_string(), req.current_price)
        } else {
            // Use kalman estimate as the preferred limit, but enforce max distance and expiry
            let max_limit_distance = last_atr * 0.6; // don't place stale/unsafe limit orders too far
            let desired_limit = k_est;
            let distance = (desired_limit - req.current_price).abs();
            if distance <= max_limit_distance {
                (format!("limit_{}", entry_type), desired_limit)
            } else {
                // If kalman estimate is too far, fallback to market to avoid missed fills
                ("market".to_string(), req.current_price)
            }
        };

        // ---- SL/TP using ATR, but with sanity caps ----
        // SL multiplier dynamic: tighter for inducement, wider for flow trades
        let sl_mult = if inducement_score > 0.0 { 1.0 } else { 2.0 };
        // For scalp, use modest targets (1.25x and 2.5x ATR)
        let tp1_mult = 1.25;
        let tp2_mult = 2.5;

        let (sl, tp1, tp2) = if entry_type == "long" {
            (entry_price - (last_atr * sl_mult), entry_price + (last_atr * tp1_mult), entry_price + (last_atr * tp2_mult))
        } else {
            (entry_price + (last_atr * sl_mult), entry_price - (last_atr * tp1_mult), entry_price - (last_atr * tp2_mult))
        };

        // Additional safety: if SL distance is implausibly small or huge, reject
        let sl_distance = (entry_price - sl).abs();
        if sl_distance < (last_atr * 0.25) || sl_distance > (last_atr * 6.0) {
            // Too tight (likely to be whipsawed) or too wide (not a scalp) -> refuse
            return EvalResponse { reason: "No Signal (SL sanity check failed)".to_string(), ..Default::default() };
        }

        // ---- NEW: Data Population for Visualization ----
        // 1. Imbalance Zones (FVGs) on M5
        let fvg_zones = find_imbalance_zones(&req.m5_highs, &req.m5_lows, 20);

        // 2. Liquidity Zones (Recent Swing Points) on M5
        let (swing_highs, swing_lows) = find_swing_points(&req.m5_highs, &req.m5_lows, 60, 3);
        let mut liquidity_zones = Vec::new();
        
        // Add last 2 swing highs as resistance
        for (_, price) in swing_highs.iter().rev().take(2) {
            liquidity_zones.push(PriceLevel { top: *price, bottom: *price, is_bullish: Some(false) });
        }
        // Add last 2 swing lows as support
        for (_, price) in swing_lows.iter().rev().take(2) {
            liquidity_zones.push(PriceLevel { top: *price, bottom: *price, is_bullish: Some(true) });
        }

        // 3. Sweep Detected mapping
        let sweep_detected = match inducement.as_str() {
            "bullish_inducement" => "low_sweep".to_string(),
            "bearish_inducement" => "high_sweep".to_string(),
            _ => "none".to_string(),
        };

        EvalResponse {
            signal_id: Uuid::new_v4().to_string(),
            entry_type,
            entry_price,
            sl_price: sl,
            tp1_price: tp1,
            tp2_price: tp2,
            reason,
            classification: "scalp".to_string(),
            conviction_score: Some(conviction),
            recommended_order_type,
            limit_order_price: entry_price,
            expiration_seconds: Some(180),
            imbalance_zones: fvg_zones,
            liquidity_zones,
            sweep_detected,
            volatility_regime: if vol_regime > 1.5 { "high".to_string() } else if vol_regime < 0.8 { "low".to_string() } else { "normal".to_string() },
            ..Default::default()
        }
    }
}