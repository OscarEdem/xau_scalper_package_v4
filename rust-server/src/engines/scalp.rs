use crate::{EvalRequest, EvalResponse};
use ::uuid::Uuid;
use crate::{adx, get_trend_bias, engines::news_guard};

pub struct ScalpEngine;

impl ScalpEngine {
    pub fn evaluate(req: &EvalRequest) -> EvalResponse {
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
            15.0 // ADX threshold (lower for scalping)
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
        let mut long_score = 0.0;
        let mut short_score = 0.0;

        // Bias weighting: if higher timeframe bias exists, give it weight
        let bias_weight: f64 = if bias > 0 { 0.2 } else if bias < 0 { -0.2 } else { 0.0 };

        // Kalman flow contributes to continuation score
        if is_bull_flow {
            long_score += kalman_score * 0.5;
        }
        if is_bear_flow {
            short_score += kalman_score * 0.5;
        }

        // M1 surge gives timing confirmation
        if is_bull_m1 {
            long_score += m1_score * 0.35;
        }
        if is_bear_m1 {
            short_score += m1_score * 0.35;
        }

        // Inducement (reversal) adds weight but must be aligned with bias
        if inducement.as_str() == "bullish_inducement" {
            long_score += 0.6 * inducement_score; // strong but not absolute
            short_score *= 0.5; // reduce opposing score to avoid contradiction
        } else if inducement.as_str() == "bearish_inducement" {
            short_score += 0.6 * inducement_score;
            long_score *= 0.5;
        }

        // Add timeframe bias to tilt the final score
        if bias > 0 {
            long_score += bias_weight.abs(); // small nudge
        } else if bias < 0 {
            short_score += bias_weight.abs();
        }

        // Map raw scores [0..~1.5] to conviction % using a conservative scaler
        let conv_long = (1.0 / (1.0 + (-6.0 * (long_score - 0.6)).exp())) * 100.0; // logistic mapping
        let conv_short = (1.0 / (1.0 + (-6.0 * (short_score - 0.6)).exp())) * 100.0;

        // Decide side if above minimum conviction threshold
        let min_conv_to_trade = 45.0; // tuned conservatively
        let (entry_type, conviction, reason) = if conv_long >= min_conv_to_trade && conv_long > conv_short {
            ("long".to_string(), conv_long, "Composite_Long".to_string())
        } else if conv_short >= min_conv_to_trade && conv_short > conv_long {
            ("short".to_string(), conv_short, "Composite_Short".to_string())
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

        // ---- Build EvalResponse with metadata useful for calibration ----
        // Populate conviction and include feature snapshot in reason (concise)
        let meta_reason = format!(
            "{}|k_slope_norm={:.4}|m1_norm={:.4}|atr={:.4}|conv={:.1}",
            reason, norm_k_slope, norm_m1_surge, last_atr, conviction
        );

        EvalResponse {
            signal_id: Uuid::new_v4().to_string(),
            entry_type,
            entry_price,
            sl_price: sl,
            tp1_price: tp1,
            tp2_price: tp2,
            reason: meta_reason,
            classification: "scalp_v2_adaptive".to_string(),
            conviction_score: Some(conviction),
            recommended_order_type,
            limit_order_price: entry_price,
            expiration_seconds: Some(180),
            ..Default::default()
        }
    }
}