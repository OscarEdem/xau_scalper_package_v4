use crate::{EvalRequest, EvalResponse};
use ::uuid::Uuid;
use crate::{adx, atr, atr_pulse, find_imbalance_zones, find_swing_points, get_daily_bias, get_trend_bias, rsi, PriceLevel, engines::{news_guard, predictor_cache::PredictorCache, ensemble_predictor}};
use tracing::debug;

// --- 1. Engine Architecture: Helper Structs ---

/// Holds the last two significant swing highs and lows.
#[derive(Debug, Default)]
struct MarketStructure {
    // External are the highest high and lowest low in the lookback.
    external_high: (usize, f64),
    external_low: (usize, f64),
    // Internal are the most recent swing points before the external ones.
    internal_high: (usize, f64),
    internal_low: (usize, f64),
    is_bos_bullish: bool,
    is_bos_bearish: bool,
    is_choch_bullish: bool, // Change of Character
    is_choch_bearish: bool,
}

/// Represents a detected Swing Failure Pattern (liquidity grab).
#[derive(Debug, Default)]
struct LiquidityAnalysis {
    is_sfp_bullish: bool,
    is_sfp_bearish: bool,
    sfp_confidence: f64, // 0-100 score
}

/// Represents a strong, impulsive price move.
#[derive(Debug, Default)]
struct Displacement {
    is_bullish: bool,
    is_bearish: bool,
    strength: f64, // 0-100 score
}

pub struct SwingEngine;

impl SwingEngine {
    pub fn evaluate(req: &EvalRequest, predictor_cache: &PredictorCache) -> EvalResponse {
        // --- 0. Data Validation ---
        let (Some(h1_closes), Some(h1_highs), Some(h1_lows)) = (&req.h1_closes, &req.h1_highs, &req.h1_lows) else {
            return EvalResponse { reason: "Missing H1 Data".to_string(), ..Default::default() };
        };
        let n = h1_closes.len();
        if n < 60 { return EvalResponse { reason: "Insufficient H1 Data".to_string(), ..Default::default() }; }

        // --- 1. Volatility Model ---
        let atr_vals = atr(h1_highs, h1_lows, h1_closes, 14);
        let last_atr = atr_vals.last().cloned().unwrap_or(0.0).max(0.0001);
        let adx_vals = adx(h1_highs, h1_lows, h1_closes, 14);

        // --- Guard: Check for high-risk news or volatility before proceeding ---
        let guard = news_guard::combined_guard(
            req.last_m1_timestamp,
            &req.upcoming_events.clone().unwrap_or_default(),
            60,   // pre-news block (minutes)
            30,   // post-news block
            &atr_vals,
            &adx_vals,
            2.0, // ATR spike multiplier
            18.0 // ADX threshold
        );

        if !guard.allowed {
            return EvalResponse { reason: guard.reason.unwrap(), ..Default::default() };
        }

        // --- 2. HTF Bias Engine ---
        let htf_bias_score = Self::htf_bias_engine(req);

        // --- 3. Market Structure Engine ---
        let structure = Self::market_structure_engine(h1_highs, h1_lows, h1_closes);
        if structure.external_high.1 == 0.0 {
            return EvalResponse { reason: "Not enough structure found".to_string(), ..Default::default() };
        }

        // --- 4. Liquidity & SFP Engine ---
        let liquidity = Self::liquidity_analysis_engine(req, &structure, &atr_vals);

        // --- 5. Displacement & FVG Engine ---
        let displacement = Self::displacement_engine(req, &structure);
        let fvg_zones = find_imbalance_zones(h1_highs, h1_lows, 10);

        // --- 6. Confluence Scoring ---
        let (long_score, short_score, reason_long, reason_short) = Self::confluence_scoring_engine(
            req, htf_bias_score, &liquidity, &displacement, &fvg_zones, req.current_price, last_atr, predictor_cache
        );

        // --- 7. Execution Logic ---
        let conviction_threshold = 60.0;
        let (entry_type, conviction_score, reason) = if long_score > short_score && long_score >= conviction_threshold {
            ("long".to_string(), long_score, reason_long)
        } else if short_score > long_score && short_score >= conviction_threshold {
            ("short".to_string(), short_score, reason_short)
        } else {
            return EvalResponse { reason: "No Signal (Low Conviction)".to_string(), ..Default::default() };
        };

        // --- 8. Risk Model ---
        let (sl_price, tp1_price, tp2_price) = Self::risk_model(
            &entry_type, &reason, req.current_price, last_atr, h1_highs[n-1], h1_lows[n-1]
        );

        EvalResponse {
            signal_id: Uuid::new_v4().to_string(),
            entry_type,
            entry_price: req.current_price,
            sl_price,
            tp1_price,
            tp2_price,
            reason,
            classification: "swing_structure_plus_liquidity".to_string(),
            conviction_score: Some(conviction_score),
            recommended_order_type: "market".to_string(),
            limit_order_price: 0.0, // Market order for swings
            expiration_seconds: None,
            ..Default::default()
        }
    }

    /// Computes a score based on Daily and H4 direction.
    fn htf_bias_engine(req: &EvalRequest) -> f64 {
        let daily_bias_str = if let (Some(d_opens), Some(d_closes)) = (&req.d1_opens, &req.d1_closes) {
            get_daily_bias(d_opens, d_closes)
        } else { "neutral".to_string() };

        let daily_bias_val = match daily_bias_str.as_str() {
            "bullish" => 1.0,
            "bearish" => -1.0,
            _ => 0.0,
        };

        let h4_bias_val = if let Some(h4_closes) = &req.h4_closes {
            get_trend_bias(h4_closes, 20) as f64
        } else { 0.0 };

        // Weighted average: Daily bias is more significant.
        (daily_bias_val * 0.6) + (h4_bias_val * 0.4)
    }

    /// Identifies key swing points and structural breaks.
    fn market_structure_engine(highs: &[f64], lows: &[f64], closes: &[f64]) -> MarketStructure {
        let mut structure = MarketStructure::default();
        let (swing_highs, swing_lows) = find_swing_points(highs, lows, 60, 3);

        if swing_highs.len() < 2 || swing_lows.len() < 2 {
            return structure;
        }

        // Get the last two of each, excluding the current candle
        let mut recent_highs = swing_highs.iter().filter(|&&(i, _)| i < highs.len() - 1).rev().take(2).collect::<Vec<_>>();
        let mut recent_lows = swing_lows.iter().filter(|&&(i, _)| i < lows.len() - 1).rev().take(2).collect::<Vec<_>>();
        recent_highs.reverse();
        recent_lows.reverse();

        if recent_highs.len() < 2 || recent_lows.len() < 2 {
            return structure;
        }

        structure.internal_high = *recent_highs[0];
        structure.external_high = *recent_highs[1];
        structure.internal_low = *recent_lows[0];
        structure.external_low = *recent_lows[1];

        // Sort to ensure external is truly the max/min
        if structure.internal_high.1 > structure.external_high.1 { std::mem::swap(&mut structure.internal_high, &mut structure.external_high); }
        if structure.internal_low.1 < structure.external_low.1 { std::mem::swap(&mut structure.internal_low, &mut structure.external_low); }

        let current_close = *closes.last().unwrap();
        let is_uptrend = structure.internal_high.1 > structure.external_high.1 && structure.internal_low.1 > structure.external_low.1;
        let is_downtrend = structure.internal_high.1 < structure.external_high.1 && structure.internal_low.1 < structure.external_low.1;

        // BOS: Breaking structure in the direction of the trend
        if is_uptrend && current_close > structure.external_high.1 { structure.is_bos_bullish = true; }
        if is_downtrend && current_close < structure.external_low.1 { structure.is_bos_bearish = true; }

        // CHoCH: Breaking structure against the trend
        if is_uptrend && current_close < structure.internal_low.1 { structure.is_choch_bearish = true; }
        if is_downtrend && current_close > structure.internal_high.1 { structure.is_choch_bullish = true; }

        structure
    }

    /// Detects liquidity grabs (SFPs) at key structural points.
    fn liquidity_analysis_engine(req: &EvalRequest, structure: &MarketStructure, atr_vals: &Vec<f64>) -> LiquidityAnalysis {
        let mut analysis = LiquidityAnalysis::default();
        let n = req.h1_closes.as_ref().unwrap().len();
        let current_high = req.h1_highs.as_ref().unwrap()[n - 1];
        let current_low = req.h1_lows.as_ref().unwrap()[n - 1];
        let current_close = req.h1_closes.as_ref().unwrap()[n - 1];

        let vol_expansion = atr_pulse(atr_vals, 20, 1.5);

        // Bullish SFP: Wick takes external low, body closes above it.
        if current_low < structure.external_low.1 && current_close > structure.external_low.1 {
            analysis.is_sfp_bullish = true;
            analysis.sfp_confidence += 50.0;
            if vol_expansion { analysis.sfp_confidence += 30.0; }
        }

        // Bearish SFP: Wick takes external high, body closes below it.
        if current_high > structure.external_high.1 && current_close < structure.external_high.1 {
            analysis.is_sfp_bearish = true;
            analysis.sfp_confidence += 50.0;
            if vol_expansion { analysis.sfp_confidence += 30.0; }
        }
        analysis
    }

    /// Validates the strength of a structural break.
    fn displacement_engine(req: &EvalRequest, structure: &MarketStructure) -> Displacement {
        let mut disp = Displacement::default();
        let n = req.h1_closes.as_ref().unwrap().len();
        let current_close = req.h1_closes.as_ref().unwrap()[n - 1];
        let current_open = req.h1_opens.as_ref().unwrap_or(&vec![0.0]).get(n-1).cloned().unwrap_or(current_close);
        let body_size = (current_close - current_open).abs();
        let last_atr = atr(req.h1_highs.as_ref().unwrap(), req.h1_lows.as_ref().unwrap(), req.h1_closes.as_ref().unwrap(), 14).last().cloned().unwrap_or(1.0);

        if structure.is_bos_bullish || structure.is_choch_bullish {
            disp.is_bullish = true;
            if body_size > last_atr * 0.7 { disp.strength += 50.0; } // Strong body
            let rsi_val = rsi(req.h1_closes.as_ref().unwrap(), 14).last().cloned().unwrap_or(50.0);
            if rsi_val > 55.0 { disp.strength += 30.0; } // Momentum confirmation
        }

        if structure.is_bos_bearish || structure.is_choch_bearish {
            disp.is_bearish = true;
            if body_size > last_atr * 0.7 { disp.strength += 50.0; }
            let rsi_val = rsi(req.h1_closes.as_ref().unwrap(), 14).last().cloned().unwrap_or(50.0);
            if rsi_val < 45.0 { disp.strength += 30.0; }
        }
        disp
    }

    /// Combines all analysis into a final conviction score.
    fn confluence_scoring_engine(
        req: &EvalRequest,
        htf_bias_score: f64,
        liquidity: &LiquidityAnalysis,
        displacement: &Displacement,
        fvg_zones: &Vec<PriceLevel>,
        current_price: f64,
        last_atr: f64,
        predictor_cache: &PredictorCache,
    ) -> (f64, f64, String, String) {
        let mut long_score = 0.0;
        let mut short_score = 0.0;
        let mut reason_long = Vec::new();
        let mut reason_short = Vec::new();

        // 1. HTF Bias (Weight: 30)
        if htf_bias_score > 0.0 {
            long_score += htf_bias_score.abs() * 30.0;
            reason_long.push("Bullish HTF Bias");
        } else if htf_bias_score < 0.0 {
            short_score += htf_bias_score.abs() * 30.0;
            reason_short.push("Bearish HTF Bias");
        }

        // 2. SFP Confidence (Weight: 40) - High impact event
        if liquidity.is_sfp_bullish {
            long_score += liquidity.sfp_confidence * 0.4;
            reason_long.push("Bullish Liquidity Grab (SFP)");
        }
        if liquidity.is_sfp_bearish {
            short_score += liquidity.sfp_confidence * 0.4;
            reason_short.push("Bearish Liquidity Grab (SFP)");
        }

        // 3. Displacement Strength (Weight: 30)
        if displacement.is_bullish {
            long_score += displacement.strength * 0.3;
            reason_long.push("Bullish Displacement");
        }
        if displacement.is_bearish {
            short_score += displacement.strength * 0.3;
            reason_short.push("Bearish Displacement");
        }

        // 4. FVG Alignment (Weight: 20)
        // Bullish FVG created below price after a bullish move
        if displacement.is_bullish {
            if fvg_zones.iter().any(|z| z.bottom < current_price && z.is_bullish.unwrap_or(false)) {
                long_score += 20.0;
                reason_long.push("Bullish FVG Support");
            }
        }
        // Bearish FVG created above price after a bearish move
        if displacement.is_bearish {
             if fvg_zones.iter().any(|z| z.top > current_price && !z.is_bullish.unwrap_or(true)) {
                short_score += 20.0;
                reason_short.push("Bearish FVG Resistance");
            }
        }

        // --- NEW: Ensemble Prediction Model Bias ---
        // Load all three models and combine their predictions using confidence weighting.
        let final_prediction_bias = ensemble_predictor::calculate_bias(
            predictor_cache,
            "h1",
            req.h1_closes.as_ref().map_or(&[][..], |v| v.as_slice()),
            current_price,
            1.0, // Swing prediction for next H1 period
        );

        debug!(bias = final_prediction_bias, "Swing ensemble prediction calculated");

        if final_prediction_bias > 0.0 {
            long_score += 15.0 * (final_prediction_bias / last_atr).clamp(0.0, 1.5); // Add a slightly higher weight for the ensemble
            reason_long.push("Ensemble Bullish Bias");
        } else {
            short_score += 15.0 * (final_prediction_bias.abs() / last_atr).clamp(0.0, 1.5);
            reason_short.push("Ensemble Bearish Bias");
        }

        (long_score, short_score, reason_long.join(" + "), reason_short.join(" + "))
    }

    /// Calculates SL and TP based on ATR and setup type.
    fn risk_model(
        entry_type: &str,
        reason: &str,
        entry_price: f64,
        last_atr: f64,
        current_high: f64,
        current_low: f64,
    ) -> (f64, f64, f64) {
        if entry_type == "long" {
            // For SFP, SL goes below the liquidity wick. For BOS, below the breakout candle.
            let sl_anchor = if reason.contains("SFP") { current_low } else { current_low };
            let sl = sl_anchor - (last_atr * 0.25); // Small buffer below the low
            let risk = (entry_price - sl).abs();
            let tp1 = entry_price + risk * 2.0; // Aim for 1:2 R:R
            let tp2 = entry_price + risk * 4.0; // Aim for 1:4 R:R
            (sl, tp1, tp2)
        } else { // "short"
            let sl_anchor = if reason.contains("SFP") { current_high } else { current_high };
            let sl = sl_anchor + (last_atr * 0.25);
            let risk = (entry_price - sl).abs();
            let tp1 = entry_price - risk * 2.0;
            let tp2 = entry_price - risk * 4.0;
            (sl, tp1, tp2)
        }
    }
}