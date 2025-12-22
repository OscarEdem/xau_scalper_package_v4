use crate::{EvalRequest, EvalResponse};
use ::uuid::Uuid;
use crate::{adx, atr, atr_pulse, find_imbalance_zones, find_swing_points, find_order_blocks, calculate_dynamic_thickness, get_daily_bias, get_trend_bias, rsi, get_fvg_limit_price, PriceLevel, engines::{news_guard, predictor_cache::PredictorCache, ensemble_predictor}, config::SwingSettings};
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
    is_reversal: bool, // NEW: true if CHoCH, false if BOS
}

pub struct FractalGuardResult {
    pub is_fighting_trend: bool,
    pub penalty_score: f64,
    pub warning: Option<String>,
}

pub struct SwingEngine;

impl SwingEngine {
    pub fn evaluate<'a>(req: &EvalRequest<'a>, predictor_cache: &PredictorCache, settings: &SwingSettings, risk_settings: &crate::config::RiskSettings) -> EvalResponse {
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
            atr_vals.iter().skip(atr_vals.len().saturating_sub(settings.atr_avg_lookback)).sum::<f64>() / (settings.atr_avg_lookback as f64).min(atr_vals.len() as f64)
        } else {
            last_atr
        };

        let adx_vals = adx(h1_highs, h1_lows, h1_closes, settings.adx_period);

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
        let structure = Self::market_structure_engine(h1_highs, h1_lows, h1_closes, req.current_price, settings);
        if structure.external_high.1 == 0.0 {
            return EvalResponse { reason: "Not enough structure found".to_string(), ..Default::default() };
        }

        // --- 4. Liquidity & SFP Engine ---
        let liquidity = Self::liquidity_analysis_engine(req, &structure, &atr_vals);

        // --- 5. Displacement & FVG Engine ---
        let displacement = Self::displacement_engine(req, &structure, settings);
        let fvg_zones = find_imbalance_zones(h1_highs, h1_lows, settings.fvg_lookback);

        // --- 5b. Order Block Engine (NEW) ---
        let h1_opens = req.h1_opens.as_ref().map(|v| v.as_ref()).unwrap_or(h1_closes.as_ref());
        let order_blocks = find_order_blocks(
            h1_highs,
            h1_lows,
            h1_opens,
            h1_closes,
            settings.swing_lookback * 4 // Look deeper into history for OBs
        );

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

        let final_prediction_bias = (h1_bias * settings.ensemble_h1_weight) + (d1_bias * settings.ensemble_d1_weight);
        debug!(h1_bias, d1_bias, final_bias = final_prediction_bias, "Swing ensemble prediction calculated");

        // --- 6. Confluence Scoring ---
        let (mut long_score, mut short_score, mut reason_long, mut reason_short) = Self::confluence_scoring_engine(
            htf_bias_score, &liquidity, &displacement, &fvg_zones, &order_blocks, req.current_price, last_atr, final_prediction_bias, settings
        );

        // --- NEW: Fractal Guard Integration ---
        if long_score > 0.0 {
            let guard = Self::fractal_guard("long", req.m15_highs.as_deref(), req.m15_lows.as_deref(), req.m15_closes.as_deref(), req.current_price, settings);
            if guard.is_fighting_trend {
                long_score = (long_score - guard.penalty_score).max(0.0);
                if let Some(w) = guard.warning {
                    reason_long = format!("{} [WARN: {}]", reason_long, w);
                }
            }
        }

        if short_score > 0.0 {
            let guard = Self::fractal_guard("short", req.m15_highs.as_deref(), req.m15_lows.as_deref(), req.m15_closes.as_deref(), req.current_price, settings);
            if guard.is_fighting_trend {
                short_score = (short_score - guard.penalty_score).max(0.0);
                if let Some(w) = guard.warning {
                    reason_short = format!("{} [WARN: {}]", reason_short, w);
                }
            }
        }

        // --- 7. Execution Logic ---
        let conviction_threshold = settings.conviction_threshold; // Lowered to capture Context-only trades (HTF + AI)
        let (entry_type, conviction_score, reason) = if long_score > short_score && long_score >= conviction_threshold {
            ("long".to_string(), long_score.min(100.0), reason_long)
        } else if short_score > long_score && short_score >= conviction_threshold {
            ("short".to_string(), short_score.min(100.0), reason_short)
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
                &entry_type, &reason, execution_price, last_atr, &structure, h1_highs[n-1], h1_lows[n-1], settings
            )
        } else {
            (0.0, 0.0, 0.0)
        };

        // --- NEW: Position Sizing ---
        let suggested_position_size = if sl_price != 0.0 && execution_price != 0.0 {
            let sl_distance = (execution_price - sl_price).abs();
            if sl_distance > 0.0 {
                let risk_amount_per_trade = risk_settings.account_equity * risk_settings.risk_per_trade_pct;
                let risk_per_lot = sl_distance * risk_settings.xauusd_lot_point_value;
                let lot_size = risk_amount_per_trade / risk_per_lot;
                // Round to 2 decimal places, typical for lot sizes
                Some((lot_size * 100.0).round() / 100.0)
            } else {
                Some(0.01) // Fallback to minimum lot size if SL is somehow zero
            }
        } else {
            Some(0.0)
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
        debug_info.insert("entry_atr".to_string(), format!("{:.5}", last_atr));

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
            order_blocks,
            sweep_detected,
            volatility_regime: if last_atr > (avg_atr * 1.5) { "high".to_string() } else { "normal".to_string() },
            debug_info: Some(debug_info), 
            suggested_position_size,
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
        let mut bias = (daily_bias_val * settings.htf_bias_daily_weight) + (h4_bias_val * settings.htf_bias_h4_weight);

        // Fix C: Recalculate HTF Bias with "Current State"
        let current_d1_open = req.d1_opens.as_ref().and_then(|v| v.last()).cloned().unwrap_or(0.0);
        if current_d1_open > 0.0 {
            if req.current_price < current_d1_open {
                bias -= 0.5; // Red day penalizes bullish bias / helps bearish bias
            } else {
                bias += 0.5; // Green day helps bullish bias / penalizes bearish bias
            }
        }
        bias
    }

    /// Identifies key swing points and structural breaks.
    fn market_structure_engine(highs: &[f64], lows: &[f64], closes: &[f64], current_price: f64, settings: &SwingSettings) -> MarketStructure {
        let mut structure = MarketStructure::default();
        let (swing_highs, swing_lows) = find_swing_points(highs, lows, settings.swing_lookback, settings.swing_neighbors);

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

        // Fix: Determine trend based on sequence (Older vs Newer) BEFORE sorting
        // recent_highs is [Older, Newer]. So internal=Older, external=Newer.
        let is_uptrend = structure.external_high.1 > structure.internal_high.1 && structure.external_low.1 > structure.internal_low.1;
        let is_downtrend = structure.external_high.1 < structure.internal_high.1 && structure.external_low.1 < structure.internal_low.1;

        // Sort to ensure external is truly the max/min
        if structure.internal_high.1 > structure.external_high.1 { std::mem::swap(&mut structure.internal_high, &mut structure.external_high); }
        if structure.internal_low.1 < structure.external_low.1 { std::mem::swap(&mut structure.internal_low, &mut structure.external_low); }

        let current_close = *closes.last().unwrap();

        // BOS: Breaking structure in the direction of the trend
        if is_uptrend && current_close > structure.external_high.1 { structure.is_bos_bullish = true; }
        if is_downtrend && current_close < structure.external_low.1 { structure.is_bos_bearish = true; }

        // CHoCH: Breaking structure against the trend
        if is_uptrend && current_close < structure.internal_low.1 { structure.is_choch_bearish = true; }
        if is_downtrend && current_close > structure.internal_high.1 { structure.is_choch_bullish = true; }

        // Fix A: Live Structure Degradation
        // If we are technically in an uptrend, but price is currently below the key low
        if is_uptrend && current_price < structure.internal_low.1 {
            structure.is_bos_bullish = false; // Force neutrality/bearish lean
        }
        if is_downtrend && current_price > structure.internal_high.1 {
            structure.is_bos_bearish = false;
        }

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
        let range = current_high - current_low;
        let close_pos = if range > 0.0 { (current_close - current_low) / range } else { 0.5 };

        // Bullish SFP: Wick takes external low, body closes above it.
        if current_low < structure.external_low.1 && current_close > structure.external_low.1 {
            // Fix B: Filter SFPs against Momentum (must close in upper half)
            if close_pos > 0.5 {
                analysis.is_sfp_bullish = true;
                analysis.sfp_confidence += 50.0;
                if vol_expansion { analysis.sfp_confidence += 30.0; }
            }
        }

        // Bearish SFP: Wick takes external high, body closes below it.
        if current_high > structure.external_high.1 && current_close < structure.external_high.1 {
            if close_pos < 0.5 {
                analysis.is_sfp_bearish = true;
                analysis.sfp_confidence += 50.0;
                if vol_expansion { analysis.sfp_confidence += 30.0; }
            }
        }
        analysis
    }

    /// Validates the strength of a structural break.
    fn displacement_engine<'a>(req: &EvalRequest<'a>, structure: &MarketStructure, settings: &SwingSettings) -> Displacement {
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

        if structure.is_bos_bullish {
            disp.is_bullish = true;
            disp.is_reversal = false;
            if body_size > last_atr * settings.displacement_atr_mult { disp.strength += settings.displacement_strength_bonus; } // Strong body
            let rsi_val = rsi(req.h1_closes.as_ref().unwrap().as_ref(), settings.displacement_rsi_period).last().cloned().unwrap_or(50.0);
            if rsi_val > settings.displacement_rsi_threshold_bull { disp.strength += settings.displacement_rsi_bonus; } // Momentum confirmation
        } else if structure.is_choch_bullish {
            disp.is_bullish = true;
            disp.is_reversal = true;
            if body_size > last_atr * settings.displacement_atr_mult { disp.strength += settings.displacement_strength_bonus; }
            let rsi_val = rsi(req.h1_closes.as_ref().unwrap().as_ref(), settings.displacement_rsi_period).last().cloned().unwrap_or(50.0);
            if rsi_val > settings.displacement_rsi_threshold_bull { disp.strength += settings.displacement_rsi_bonus; }
        }

        if structure.is_bos_bearish {
            disp.is_bearish = true;
            disp.is_reversal = false;
            if body_size > last_atr * settings.displacement_atr_mult { disp.strength += settings.displacement_strength_bonus; }
            let rsi_val = rsi(req.h1_closes.as_ref().unwrap().as_ref(), settings.displacement_rsi_period).last().cloned().unwrap_or(50.0);
            if rsi_val < settings.displacement_rsi_threshold_bear { disp.strength += settings.displacement_rsi_bonus; }
        } else if structure.is_choch_bearish {
            disp.is_bearish = true;
            disp.is_reversal = true;
            if body_size > last_atr * settings.displacement_atr_mult { disp.strength += settings.displacement_strength_bonus; }
            let rsi_val = rsi(req.h1_closes.as_ref().unwrap().as_ref(), settings.displacement_rsi_period).last().cloned().unwrap_or(50.0);
            if rsi_val < settings.displacement_rsi_threshold_bear { disp.strength += settings.displacement_rsi_bonus; }
        }
        disp
    }

    /// Combines all analysis into a final conviction score.
    fn confluence_scoring_engine(
        htf_bias_score: f64,
        liquidity: &LiquidityAnalysis,
        displacement: &Displacement,
        fvg_zones: &Vec<PriceLevel>,
        order_blocks: &Vec<PriceLevel>,
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
            if displacement.is_reversal {
                reason_long.push("Bullish Reversal (CHoCH)");
            } else {
                reason_long.push("Bullish Continuation (BOS)");
            }
        }
        if displacement.is_bearish {
            short_score += displacement.strength * settings.displacement_weight_mult; // Increased weight
            if displacement.is_reversal {
                reason_short.push("Bearish Reversal (CHoCH)");
            } else {
                reason_short.push("Bearish Continuation (BOS)");
            }
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

        // 5. Order Block Alignment (Weight: 25)
        if displacement.is_bullish {
             if order_blocks.iter().any(|ob| ob.is_bullish.unwrap_or(false) && current_price <= ob.top && current_price >= ob.bottom * 0.998) {
                 long_score += 25.0;
                 reason_long.push("Bullish Order Block Test");
             }
        }

        if displacement.is_bearish {
             if order_blocks.iter().any(|ob| !ob.is_bullish.unwrap_or(true) && current_price >= ob.bottom && current_price <= ob.top * 1.002) {
                 short_score += 25.0;
                 reason_short.push("Bearish Order Block Test");
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
        structure: &MarketStructure,
        current_high: f64,
        current_low: f64,
        settings: &SwingSettings,
    ) -> (f64, f64, f64) {
        if entry_type == "long" {
            // For SFP, SL goes below the liquidity wick (current_low).
            // For Structure/Trend, SL goes below the structural low (external_low) to give swing room.
            let sl_anchor = if reason.contains("SFP") { 
                current_low 
            } else { 
                // Use structural low if valid (below entry), otherwise fallback to candle low
                if structure.external_low.1 < entry_price { structure.external_low.1 } else { current_low }
            };

            let sl = sl_anchor - (last_atr * settings.sl_atr_buffer); // Small buffer below the low
            let risk = (entry_price - sl).abs();
            let tp1 = entry_price + risk * settings.risk_reward_ratio_tp1; // Aim for 1:2 R:R
            let tp2 = entry_price + risk * settings.risk_reward_ratio_tp2; // Aim for 1:4 R:R
            (sl, tp1, tp2)
        } else { // "short"
            let sl_anchor = if reason.contains("SFP") { 
                current_high 
            } else { 
                // Use structural high if valid (above entry)
                if structure.external_high.1 > entry_price { structure.external_high.1 } else { current_high }
            };

            let sl = sl_anchor + (last_atr * settings.sl_atr_buffer);
            let risk = (entry_price - sl).abs();
            let tp1 = entry_price - risk * settings.risk_reward_ratio_tp1;
            let tp2 = entry_price - risk * settings.risk_reward_ratio_tp2;
            (sl, tp1, tp2)
        }
    }

    /// Checks M15 structure to prevent entering against immediate momentum.
    /// Returns a penalty to subtract from your conviction score.
    pub fn fractal_guard(
        entry_type: &str,
        m15_highs: Option<&[f64]>,
        m15_lows: Option<&[f64]>,
        m15_closes: Option<&[f64]>,
        current_price: f64,
        settings: &SwingSettings,
    ) -> FractalGuardResult {
        if !settings.fractal_guard_enabled {
             return FractalGuardResult { is_fighting_trend: false, penalty_score: 0.0, warning: None };
        }

        let (Some(highs), Some(lows), Some(closes)) = (m15_highs, m15_lows, m15_closes) else {
            return FractalGuardResult { is_fighting_trend: false, penalty_score: 0.0, warning: Some("Missing M15 Data".to_string()) };
        };

        if highs.len() < 50 {
             return FractalGuardResult { is_fighting_trend: false, penalty_score: 0.0, warning: None };
        }

        let ltf_settings = SwingSettings {
            swing_lookback: settings.m15_swing_lookback,
            swing_neighbors: 2,
            ..settings.clone()
        };
        
        let ltf_structure = Self::market_structure_engine(highs, lows, closes, current_price, &ltf_settings);
        let current_close = *closes.last().unwrap_or(&0.0);
        
        let mut result = FractalGuardResult {
            is_fighting_trend: false,
            penalty_score: 0.0,
            warning: None,
        };

        // Determine M15 Trend using indices (External is always Extreme)
        // External High is Max. If Index(Ext) > Index(Int), Max is Newer => Higher High.
        // External Low is Min. If Index(Ext) > Index(Int), Min is Newer => Lower Low.
        let is_higher_high = ltf_structure.external_high.0 > ltf_structure.internal_high.0;
        let is_lower_high = !is_higher_high;
        
        let is_lower_low = ltf_structure.external_low.0 > ltf_structure.internal_low.0;
        let is_higher_low = !is_lower_low;

        let is_ltf_downtrend = is_lower_high && is_lower_low;
        let is_ltf_uptrend = is_higher_high && is_higher_low;

        match entry_type {
            "long" => {
                // DANGER: H1 says Long, but M15 is making Lower Lows (Downtrend) or breaking down
                let is_breaking_down = current_close < ltf_structure.internal_low.1;

                if is_ltf_downtrend || is_breaking_down {
                    result.is_fighting_trend = true;
                    result.penalty_score = settings.fractal_penalty_score;
                    result.warning = Some("M15 Trend is Bearish (Falling Knife)".to_string());
                }
            },
            "short" => {
                // DANGER: H1 says Short, but M15 is making Higher Highs (Uptrend) or breaking up
                let is_breaking_up = current_close > ltf_structure.internal_high.1;

                if is_ltf_uptrend || is_breaking_up {
                    result.is_fighting_trend = true;
                    result.penalty_score = settings.fractal_penalty_score;
                    result.warning = Some("M15 Trend is Bullish (Step in front of train)".to_string());
                }
            },
            _ => {}
        }

        result
    }
}