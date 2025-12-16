use crate::{EvalRequest, EvalResponse};
use ::uuid::Uuid;
use crate::{adx, atr, atr_pulse, find_imbalance_zones, find_swing_points, calculate_dynamic_thickness, get_daily_bias, get_trend_bias, rsi, get_fvg_limit_price, PriceLevel, engines::{news_guard, predictor_cache::PredictorCache, ensemble_predictor}, config::SwingSettings};
use tracing::debug;
use std::collections::HashMap;

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
    pub fn evaluate<'a>(req: &EvalRequest<'a>, predictor_cache: &PredictorCache, settings: &SwingSettings) -> EvalResponse {
        // --- 0. Data Validation ---
        let (Some(h1_closes), Some(h1_highs), Some(h1_lows)) = (&req.h1_closes, &req.h1_highs, &req.h1_lows) else {
            return EvalResponse { reason: "Missing H1 Data".to_string(), ..Default::default() };
        };
        let n = h1_closes.len();
        if n < settings.min_data_len { return EvalResponse { reason: "Insufficient H1 Data".to_string(), ..Default::default() }; }

        // --- 1. Volatility Model ---
        let atr_vals = atr(h1_highs, h1_lows, h1_closes, settings.atr_period);
        let last_atr = atr_vals.last().cloned().unwrap_or(0.0).max(0.0001);
        
        // Calculate baseline ATR for regime detection
        let avg_atr: f64 = if atr_vals.len() > 0 {
            atr_vals.iter().skip(atr_vals.len().saturating_sub(50)).sum::<f64>() / 50.0_f64.min(atr_vals.len() as f64)
        } else {
            last_atr
        };

        let adx_vals = adx(h1_highs, h1_lows, h1_closes, 14);

        // --- Guard: Check for high-risk news or volatility before proceeding ---
        let guard = news_guard::combined_guard(
            &req.symbol,
            req.last_m1_timestamp,
            &req.upcoming_events.clone().unwrap_or_default(),
            settings.news_pre_event_block_minutes,
            settings.news_post_event_block_minutes,
            &atr_vals,
            &adx_vals,
            settings.news_guard_atr_spike_multiplier,
            req.adx_threshold.unwrap_or(12.0) // ADX threshold (configurable)
        );

        if !guard.allowed {
            return EvalResponse { reason: guard.reason.unwrap(), ..Default::default() };
        }

        // --- 2. HTF Bias Engine ---
        let htf_bias_score = Self::htf_bias_engine(req, settings);

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

        // --- NEW: Ensemble Prediction Model Bias ---
        // Load all three models and combine their predictions using confidence weighting.
        let h1_bias = ensemble_predictor::calculate_bias(
            predictor_cache,
            "h1",
            req.h1_closes.as_ref().map_or(&[][..], |v| v.as_ref()),
            req.current_price,
            1.0, // Swing prediction for next H1 period
        );

        let d1_bias = ensemble_predictor::calculate_bias(
            predictor_cache,
            "d1",
            req.d1_closes.as_ref().map_or(&[][..], |v| v.as_ref()),
            req.current_price,
            1.0, // Swing prediction for next D1 period
        );

        let final_prediction_bias = (h1_bias * 0.7) + (d1_bias * 0.3);
        debug!(h1_bias, d1_bias, final_bias = final_prediction_bias, "Swing ensemble prediction calculated");

        // --- 6. Confluence Scoring ---
        let (long_score, short_score, reason_long, reason_short) = Self::confluence_scoring_engine(
            htf_bias_score, &liquidity, &displacement, &fvg_zones, req.current_price, last_atr, final_prediction_bias, settings
        );

        // --- 7. Execution Logic ---
        let conviction_threshold = settings.conviction_threshold; // Lowered to capture Context-only trades (HTF + AI)
        let (entry_type, conviction_score, reason) = if long_score > short_score && long_score >= conviction_threshold {
            ("long".to_string(), long_score, reason_long)
        } else if short_score > long_score && short_score >= conviction_threshold {
            ("short".to_string(), short_score, reason_short)
        } else {
            let (leaning_reason, score) = if long_score >= short_score {
                (reason_long, long_score)
            } else {
                (reason_short, short_score)
            };
            let detailed_reason = if !leaning_reason.is_empty() {
                format!("No Signal (Low Conviction {:.1}%): {}", score, leaning_reason)
            } else {
                "No Signal (Low Conviction)".to_string()
            };
            ("none".to_string(), 0.0, detailed_reason)
        };

        // --- 7b. Order Type & Price Logic ---
        // If the signal is based on an FVG, try to get a limit entry at equilibrium (50% of gap).
        // Otherwise, default to market execution.
        let (recommended_order_type, execution_price) = if entry_type == "none" {
            ("none".to_string(), 0.0)
        } else {
            let suffix = entry_type.clone(); // "long" or "short"
            
            if reason.contains("FVG") {
                 if let Some(price) = get_fvg_limit_price(&fvg_zones, req.current_price, &entry_type, "optimal") {
                     (format!("limit_{}", suffix), price)
                 } else {
                     (format!("market_{}", suffix), req.current_price)
                 }
            } else {
                 (format!("market_{}", suffix), req.current_price)
            }
        };

        // --- 8. Risk Model ---
        let (sl_price, tp1_price, tp2_price) = if entry_type != "none" {
            Self::risk_model(
                &entry_type, &reason, execution_price, last_atr, h1_highs[n-1], h1_lows[n-1], settings
            )
        } else {
            (0.0, 0.0, 0.0)
        };

        // --- 9. Data Population for Response ---
        // Construct Liquidity Zones from Market Structure (External Highs/Lows)
        let mut liquidity_zones = Vec::new();

        if structure.external_high.1 > 0.0 {
            let thickness = calculate_dynamic_thickness(structure.external_high.0, h1_highs, h1_lows, last_atr);

            liquidity_zones.push(PriceLevel { 
                top: structure.external_high.1 + thickness, 
                bottom: structure.external_high.1, 
                is_bullish: Some(false) // Resistance / Buy-side Liquidity
            });
        }
        if structure.external_low.1 > 0.0 {
            let thickness = calculate_dynamic_thickness(structure.external_low.0, h1_highs, h1_lows, last_atr);

            liquidity_zones.push(PriceLevel { 
                top: structure.external_low.1, 
                bottom: structure.external_low.1 - thickness, 
                is_bullish: Some(true) // Support / Sell-side Liquidity
            });
        }

        // Determine Sweep Detected String
        let sweep_detected = if liquidity.is_sfp_bullish {
            "low_sweep".to_string()
        } else if liquidity.is_sfp_bearish {
            "high_sweep".to_string()
        } else {
            "none".to_string()
        };

        let mut debug_info = HashMap::new();
        debug_info.insert("htf_bias_score".to_string(), format!("{:.2}", htf_bias_score));
        debug_info.insert("sfp_bullish".to_string(), liquidity.is_sfp_bullish.to_string());
        debug_info.insert("sfp_bearish".to_string(), liquidity.is_sfp_bearish.to_string());
        debug_info.insert("displacement_strength".to_string(), format!("{:.2}", displacement.strength));
        debug_info.insert("ensemble_bias".to_string(), format!("{:.4}", final_prediction_bias));
        debug_info.insert("long_score".to_string(), format!("{:.2}", long_score));
        debug_info.insert("short_score".to_string(), format!("{:.2}", short_score));

        // --- Deterministic Signal ID ---
        // We generate a stable ID based on the structural level being traded.
        // This ensures that as long as the setup exists at this price, the ID remains the same.
        let signal_id = if entry_type != "none" {
            let price_key = if entry_type == "long" { structure.external_low.1 } else { structure.external_high.1 };
            // Use UUID v5 (Name-based) to create a unique hash for this specific setup
            format!("{}-{}-{:.5}", req.symbol, entry_type, price_key)
        } else {
            Uuid::new_v4().to_string()
        };

        EvalResponse {
            signal_id,
            entry_type,
            entry_price: execution_price,
            sl_price,
            tp1_price,
            tp2_price,
            reason,
            classification: "swing_structure_plus_liquidity".to_string(),
            conviction_score: Some(conviction_score),
            recommended_order_type,
            limit_order_price: if execution_price != req.current_price { execution_price } else { 0.0 },
            expiration_seconds: Some(settings.limit_order_expiration), // 1 Hour expiry for swing limits
            time_stop_seconds: settings.time_stop_seconds, // 24 Hours: Close trade if stagnant
            imbalance_zones: fvg_zones,
            liquidity_zones,
            sweep_detected,
            volatility_regime: if last_atr > (avg_atr * 1.5) { "high".to_string() } else { "normal".to_string() },
            debug_info: Some(debug_info),
            ..Default::default()
        }
    }

    /// Computes a score based on Daily and H4 direction.
    fn htf_bias_engine<'a>(req: &EvalRequest<'a>, settings: &SwingSettings) -> f64 {
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
        (daily_bias_val * settings.htf_bias_daily_weight) + (h4_bias_val * settings.htf_bias_h4_weight)
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
    fn liquidity_analysis_engine<'a>(req: &EvalRequest<'a>, structure: &MarketStructure, atr_vals: &Vec<f64>) -> LiquidityAnalysis {
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
    fn displacement_engine<'a>(req: &EvalRequest<'a>, structure: &MarketStructure) -> Displacement {
        let mut disp = Displacement::default();
        let n = req.h1_closes.as_ref().unwrap().len();
        let current_close = req.h1_closes.as_ref().unwrap()[n - 1];
        let current_open = req.h1_opens.as_ref().and_then(|c| c.get(n - 1)).cloned().unwrap_or(current_close);
        let body_size = (current_close - current_open).abs();
        let last_atr = atr(
            req.h1_highs.as_ref().unwrap().as_ref(), 
            req.h1_lows.as_ref().unwrap().as_ref(), 
            req.h1_closes.as_ref().unwrap().as_ref(), 
            14
        ).last().cloned().unwrap_or(1.0);

        if structure.is_bos_bullish || structure.is_choch_bullish {
            disp.is_bullish = true;
            if body_size > last_atr * 0.7 { disp.strength += 50.0; } // Strong body
            let rsi_val = rsi(req.h1_closes.as_ref().unwrap().as_ref(), 14).last().cloned().unwrap_or(50.0);
            if rsi_val > 55.0 { disp.strength += 30.0; } // Momentum confirmation
        }

        if structure.is_bos_bearish || structure.is_choch_bearish {
            disp.is_bearish = true;
            if body_size > last_atr * 0.7 { disp.strength += 50.0; }
            let rsi_val = rsi(req.h1_closes.as_ref().unwrap().as_ref(), 14).last().cloned().unwrap_or(50.0);
            if rsi_val < 45.0 { disp.strength += 30.0; }
        }
        disp
    }

    /// Combines all analysis into a final conviction score.
    fn confluence_scoring_engine(
        htf_bias_score: f64,
        liquidity: &LiquidityAnalysis,
        displacement: &Displacement,
        fvg_zones: &Vec<PriceLevel>,
        current_price: f64,
        last_atr: f64,
        final_prediction_bias: f64,
        settings: &SwingSettings,
    ) -> (f64, f64, String, String) {
        let mut long_score = 0.0;
        let mut short_score = 0.0;
        let mut reason_long = Vec::new();
        let mut reason_short = Vec::new();

        // 1. HTF Bias (Weight: 30)
        if htf_bias_score > 0.0 {
            long_score += htf_bias_score.abs() * settings.htf_bias_weight;
            reason_long.push("Bullish HTF Bias");
        } else if htf_bias_score < 0.0 {
            short_score += htf_bias_score.abs() * settings.htf_bias_weight;
            reason_short.push("Bearish HTF Bias");
        }

        // 2. SFP Confidence (Weight: 40) - High impact event
        if liquidity.is_sfp_bullish {
            long_score += liquidity.sfp_confidence * settings.sfp_weight_mult; // Increased weight
            reason_long.push("Bullish Liquidity Grab (SFP)");
        }
        if liquidity.is_sfp_bearish {
            short_score += liquidity.sfp_confidence * settings.sfp_weight_mult; // Increased weight
            reason_short.push("Bearish Liquidity Grab (SFP)");
        }

        // 3. Displacement Strength (Weight: 30)
        if displacement.is_bullish {
            long_score += displacement.strength * settings.displacement_weight_mult; // Increased weight
            reason_long.push("Bullish Displacement");
        }
        if displacement.is_bearish {
            short_score += displacement.strength * settings.displacement_weight_mult; // Increased weight
            reason_short.push("Bearish Displacement");
        }

        // 4. FVG Alignment (Weight: 20)
        // Bullish FVG created below price after a bullish move
        if displacement.is_bullish {
            if fvg_zones.iter().any(|z| z.bottom < current_price && z.is_bullish.unwrap_or(false)) {
                long_score += settings.fvg_weight;
                reason_long.push("Bullish FVG Support");
            }
        }
        // Bearish FVG created above price after a bearish move
        if displacement.is_bearish {
             if fvg_zones.iter().any(|z| z.top > current_price && !z.is_bullish.unwrap_or(true)) {
                short_score += settings.fvg_weight;
                reason_short.push("Bearish FVG Resistance");
            }
        }

        if final_prediction_bias > 0.0 {
            long_score += settings.ensemble_weight_mult * (final_prediction_bias / last_atr).clamp(0.0, 1.5); // Add a slightly higher weight for the ensemble
            reason_long.push("Ensemble Bullish Bias");
        } else if final_prediction_bias < 0.0 {
            short_score += settings.ensemble_weight_mult * (final_prediction_bias.abs() / last_atr).clamp(0.0, 1.5);
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
        settings: &SwingSettings,
    ) -> (f64, f64, f64) {
        if entry_type == "long" {
            // For SFP, SL goes below the liquidity wick. For BOS, below the breakout candle.
            let sl_anchor = if reason.contains("SFP") { current_low } else { current_low };
            let sl = sl_anchor - (last_atr * settings.sl_atr_buffer); // Small buffer below the low
            let risk = (entry_price - sl).abs();
            let tp1 = entry_price + risk * settings.risk_reward_ratio_tp1; // Aim for 1:2 R:R
            let tp2 = entry_price + risk * settings.risk_reward_ratio_tp2; // Aim for 1:4 R:R
            (sl, tp1, tp2)
        } else { // "short"
            let sl_anchor = if reason.contains("SFP") { current_high } else { current_high };
            let sl = sl_anchor + (last_atr * settings.sl_atr_buffer);
            let risk = (entry_price - sl).abs();
            let tp1 = entry_price - risk * settings.risk_reward_ratio_tp1;
            let tp2 = entry_price - risk * settings.risk_reward_ratio_tp2;
            (sl, tp1, tp2)
        }
    }
}