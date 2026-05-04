use crate::{EvalRequest, EvalResponse, SignalDirection};
use crate::{adx, atr, atr_pulse, find_imbalance_zones, find_swing_points, find_order_blocks, calculate_dynamic_thickness, get_daily_bias, get_trend_bias, rsi, get_fvg_limit_price, PriceLevel, engines::{news_guard, predictor_cache::PredictorCache, ensemble_predictor}, config::SwingSettings};
use tracing::debug;
use std::collections::HashMap;
use serde::Serialize;

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
    /// True when price has broken a structural level AND retested it from the other side.
    /// Prevents entries on the breakout candle itself (the most common swing loss cause).
    bos_bullish_retested: bool,
    bos_bearish_retested: bool,
}

#[derive(Debug, Clone, Serialize)]
pub enum SetupDriver {
    Sfp(SfpQuality),
    BosRetest(DisplacementQuality),
    OrderBlockBounce(ObQuality),
}

#[derive(Debug, Clone, Serialize)]
pub struct SfpQuality {
    pub direction: SignalDirection,
    pub wick_body_ratio: f64,
    pub sweep_depth_atr: f64,
    pub close_position: f64,
    pub is_vol_expansion: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct DisplacementQuality {
    pub direction: SignalDirection,
    pub strength_atr: f64,
    pub is_reversal: bool,
    pub rsi_val: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ObQuality {
    pub direction: SignalDirection,
    pub depth_ratio: f64,
    pub is_fvg_confluence: bool,
    pub timeframe: String,
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

        // --- NEW: Dynamic Regime Adjustment ---
        let vol_ratio = if avg_atr > 0.0 { last_atr / avg_atr } else { 1.0 };
        let mut adjusted_settings = settings.clone();
        let mut regime_note = "Normal".to_string();

        if vol_ratio > 1.5 {
            // High Volatility: "Kill Switch" / Defensive Mode
            // 1. Require higher conviction (filter weak trades)
            adjusted_settings.conviction_threshold += 15.0; 
            // 2. Widen stops (prevent noise stop-outs)
            adjusted_settings.sl_atr_buffer *= 1.5;
            // 3. Take profit earlier (mean reversion risk)
            adjusted_settings.risk_reward_ratio_tp1 *= 0.8;
            
            regime_note = format!("High Vol (Ratio {:.2}) -> Defensive Mode", vol_ratio);
        } else if vol_ratio < 0.75 {
            // Low Volatility: Aggressive Mode
            // 1. Tighter stops allowed
            adjusted_settings.sl_atr_buffer *= 0.8;
            
            regime_note = format!("Low Vol (Ratio {:.2}) -> Aggressive Mode", vol_ratio);
        }
        let settings = &adjusted_settings; // Shadow original settings

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

        // --- 4. FVG & OB Engine ---
        let fvg_zones = find_imbalance_zones(h1_highs, h1_lows, settings.fvg_lookback);

        let h1_opens = req.h1_opens.as_ref().map(|v| v.as_ref()).unwrap_or(h1_closes.as_ref());
        let order_blocks = find_order_blocks(
            h1_highs,
            h1_lows,
            h1_opens,
            h1_closes,
            settings.swing_lookback * 4 // Look deeper into history for OBs
        );

        // --- NEW: M30 Order Blocks ---
        let m30_order_blocks = if let (Some(m30_highs), Some(m30_lows)) = (req.m30_highs.as_deref(), req.m30_lows.as_deref()) {
             find_order_blocks(
                m30_highs,
                m30_lows,
                req.m30_closes.as_ref(),
                req.m30_closes.as_ref(),
                settings.swing_lookback * 4
            )
        } else {
            Vec::new()
        };

        // --- NEW: Ensemble Prediction Model Bias ---
        // Load all three models and combine their predictions using confidence weighting.
        let h1_bias = ensemble_predictor::calculate_bias(
            predictor_cache,
            "h1",
            req.h1_closes.as_ref().map_or(&[][..], |v| v.as_ref()),
            req.current_price,
            1.0, // Swing prediction for next H1 period
        );

        let d1_bias = 0.0; // Disabled D1 models to save memory on Render
        /*
        let d1_bias = ensemble_predictor::calculate_bias(
            predictor_cache,
            "d1",
            req.d1_closes.as_ref().map_or(&[][..], |v| v.as_ref()),
            req.current_price,
            1.0, // Swing prediction for next D1 period
        );
        */

        let final_prediction_bias = (h1_bias * settings.ensemble_h1_weight) + (d1_bias * settings.ensemble_d1_weight);
        debug!(h1_bias, d1_bias, final_bias = final_prediction_bias, "Swing ensemble prediction calculated");

        // --- 6. Driver Detection & Probability Scoring ---
        let mut drivers = Vec::new();
        if let Some(d) = Self::detect_sfp_driver(req, &structure, &atr_vals) { drivers.push(d); }
        if let Some(d) = Self::detect_displacement_driver(req, &structure, settings, last_atr) { drivers.push(d); }
        if let Some(d) = Self::detect_ob_bounce_driver(req, &order_blocks, &fvg_zones, settings, "H1") { drivers.push(d); }
        if let Some(d) = Self::detect_ob_bounce_driver(req, &m30_order_blocks, &fvg_zones, settings, "M30") { drivers.push(d); }

        let mut best_signal = (SignalDirection::None, 0.0, "No Signal".to_string(), None::<SetupDriver>);

        for driver in drivers {
            let dir_str = match &driver {
                SetupDriver::Sfp(q) => if q.direction == SignalDirection::Long { "long" } else { "short" },
                SetupDriver::BosRetest(q) => if q.direction == SignalDirection::Long { "long" } else { "short" },
                SetupDriver::OrderBlockBounce(q) => if q.direction == SignalDirection::Long { "long" } else { "short" },
            };
            
            // --- Multi-Fractal Guard ---
            // Check both M15 and M5 structure to get a more robust view of LTF momentum.
            let guard_m15 = Self::fractal_guard(
                dir_str,
                req.m15_highs.as_deref(),
                req.m15_lows.as_deref(),
                req.m15_closes.as_deref(),
                req.current_price,
                settings,
                "M15",
                settings.m15_swing_lookback
            );

            // Note: M5 highs/lows are not optional on EvalRequest, so we wrap them in Some().
            let guard_m5 = Self::fractal_guard(
                dir_str,
                Some(req.m5_highs.as_ref()),
                Some(req.m5_lows.as_ref()),
                Some(req.m5_closes.as_ref()),
                req.current_price,
                settings,
                "M5",
                settings.m5_swing_lookback // Using the new setting
            );

            // M30 Guard
            let guard_m30 = Self::fractal_guard(
                dir_str,
                req.m30_highs.as_deref(),
                req.m30_lows.as_deref(),
                Some(req.m30_closes.as_ref()),
                req.current_price,
                settings,
                "M30",
                settings.m30_swing_lookback
            );

            // Combine penalties
            let penalty = if guard_m15.is_fighting_trend { guard_m15.penalty_score } else { 0.0 }
                        + if guard_m5.is_fighting_trend { guard_m5.penalty_score * 0.5 } else { 0.0 }
                        + if guard_m30.is_fighting_trend { guard_m30.penalty_score } else { 0.0 };

            let (score, reason) = Self::calculate_probability_score(&driver, htf_bias_score, vol_ratio, final_prediction_bias, penalty, settings);
            
            if score > best_signal.1 {
                let dir = match &driver {
                    SetupDriver::Sfp(q) => q.direction.clone(),
                    SetupDriver::BosRetest(q) => q.direction.clone(),
                    SetupDriver::OrderBlockBounce(q) => q.direction.clone(),
                };
                best_signal = (dir, score, reason, Some(driver));
            }
        }

        let (entry_type, conviction_score, reason, driver_opt) = best_signal;

        // Threshold Check
        let (entry_type, conviction_score, reason) = if conviction_score >= settings.conviction_threshold {
            (entry_type, conviction_score, reason)
        } else {
            let detailed_reason = if !reason.is_empty() && reason != "No Signal" {
                format!("No Signal (Low Conviction {:.1}%): {}", conviction_score, reason)
            } else {
                "No Signal (Low Conviction)".to_string()
            };
            (SignalDirection::None, 0.0, detailed_reason)
        };

        // --- 7b. Order Type & Price Logic ---
        // If the signal is based on an FVG, try to get a limit entry at equilibrium (50% of gap).
        // Otherwise, default to market execution.
        let (recommended_order_type, execution_price) = if entry_type == SignalDirection::None {
            ("none".to_string(), 0.0)
        } else {
            let suffix = entry_type.to_string(); // "long" or "short"
            
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
        let (sl_price, tp1_price, tp2_price) = if entry_type != SignalDirection::None {
            Self::risk_model(
                &entry_type, &reason, execution_price, last_atr, &structure, h1_highs[n-1], h1_lows[n-1], settings, vol_ratio
            )
        } else {
            (0.0, 0.0, 0.0)
        };

        // --- NEW: Position Sizing ---
        let suggested_position_size = if sl_price != 0.0 && execution_price != 0.0 {
            let sl_distance = (execution_price - sl_price).abs();
            if sl_distance > 0.0 {
                // Currency Validation
                if let Some(acc_ccy) = &req.account_currency {
                    let quote_ccy = if req.symbol.len() >= 6 { &req.symbol[3..6] } else { "" };
                    if !quote_ccy.is_empty() && quote_ccy != acc_ccy.as_ref() {
                        tracing::warn!(
                            symbol = %req.symbol, 
                            account_currency = %acc_ccy, 
                            quote_currency = %quote_ccy, 
                            "Currency mismatch: Position sizing assumes point value is in account currency."
                        );
                    }
                }
                let equity = req.account_equity.unwrap_or(risk_settings.account_equity);
                let risk_amount_per_trade = equity * risk_settings.risk_per_trade_pct;
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
        let sweep_detected = if let Some(SetupDriver::Sfp(q)) = &driver_opt {
            if q.direction == SignalDirection::Long { "low_sweep".to_string() } else { "high_sweep".to_string() }
        } else {
            "none".to_string()
        };

        let mut debug_info = HashMap::new();
        debug_info.insert("htf_bias_score".to_string(), format!("{:.2}", htf_bias_score));
        debug_info.insert("ensemble_bias".to_string(), format!("{:.4}", final_prediction_bias));
        debug_info.insert("entry_atr".to_string(), format!("{:.5}", last_atr));
        debug_info.insert("final_score".to_string(), format!("{:.2}", conviction_score));
        debug_info.insert("regime_note".to_string(), regime_note);

        if let Some(driver) = &driver_opt {
            if let Ok(json) = serde_json::to_string(driver) {
                debug_info.insert("driver_quality".to_string(), json);
            }
        }

        // --- Visualize M30 Structure ---
        if let (Some(h), Some(l)) = (req.m30_highs.as_deref(), req.m30_lows.as_deref()) {
             let m30_settings = SwingSettings { swing_lookback: settings.m30_swing_lookback, swing_neighbors: 2, ..settings.clone() };
             let m30_struct = Self::market_structure_engine(h, l, req.m30_closes.as_ref(), req.current_price, &m30_settings);
             debug_info.insert("m30_ext_high".to_string(), format!("{:.2}", m30_struct.external_high.1));
             debug_info.insert("m30_ext_low".to_string(), format!("{:.2}", m30_struct.external_low.1));
        }

        // --- Deterministic Signal ID ---
        // We generate a stable ID based on the structural level being traded.
        // This ensures that as long as the setup exists at this price, the ID remains the same.
        let signal_id = if entry_type != SignalDirection::None {
            let (price_key, idx) = if entry_type == SignalDirection::Long { 
                (structure.external_low.1, structure.external_low.0) 
            } else { 
                (structure.external_high.1, structure.external_high.0) 
            };

            // 1. Get the actual timestamp of the structural point
            let swing_ts = if let Some(timestamps) = &req.h1_timestamps {
                // Safety check for index bounds
                if idx < timestamps.len() {
                    timestamps[idx]
                } else {
                    // Fallback (only if data is corrupted)
                    req.last_h1_timestamp.unwrap_or(0)
                }
            } else {
                // Legacy fallback (calculation) - only use if timestamps missing
                let n = h1_closes.len();
                let last_ts = req.last_h1_timestamp.unwrap_or(0);
                if last_ts > 0 { last_ts - ((n.saturating_sub(1).saturating_sub(idx)) as i64 * 3600) } else { 0 }
            };

            // 2. Generate ID using swing_ts (absolute) and price_key (structural)
            format!("{}-{}-{}-{}", req.symbol, entry_type.to_string(), swing_ts, price_key as i64)
        } else {
            "none".to_string()
        };

        EvalResponse {
            signal_id,
            entry_type,
            entry_price: execution_price,
            atr: last_atr,
            sl_price,
            tp1_price,
            tp2_price,
            reason,
            classification: "swing".to_string(),
            conviction_score: Some(conviction_score),
            recommended_order_type,
            limit_order_price: if execution_price != req.current_price { execution_price } else { 0.0 },
            expiration_seconds: Some(settings.limit_order_expiration), // 1 Hour expiry for swing limits
            time_stop_seconds: settings.time_stop_seconds, // 24 Hours: Close trade if stagnant
            imbalance_zones: fvg_zones,
            liquidity_zones,
            order_blocks: if let Some(SetupDriver::OrderBlockBounce(q)) = &driver_opt {
                if q.timeframe == "M30" {
                    m30_order_blocks
                } else {
                    order_blocks
                }
            } else { order_blocks },
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
        if is_uptrend && current_close > structure.external_high.1 {
            structure.is_bos_bullish = true;
            // BOS Retest Check: Is price now pulling back toward the broken level?
            // We require price to close within 0.5 ATR of the broken high to confirm retest.
            let atr_approx = if closes.len() > 14 {
                let slice = &closes[closes.len()-14..];
                let range: f64 = slice.windows(2).map(|w| (w[1] - w[0]).abs()).sum::<f64>() / 13.0;
                range.max(0.0001)
            } else { 1.0 };
            let retest_zone = structure.external_high.1 + atr_approx * 0.5;
            if current_close <= retest_zone {
                structure.bos_bullish_retested = true;
            }
        }
        if is_downtrend && current_close < structure.external_low.1 {
            structure.is_bos_bearish = true;
            let atr_approx = if closes.len() > 14 {
                let slice = &closes[closes.len()-14..];
                let range: f64 = slice.windows(2).map(|w| (w[1] - w[0]).abs()).sum::<f64>() / 13.0;
                range.max(0.0001)
            } else { 1.0 };
            let retest_zone = structure.external_low.1 - atr_approx * 0.5;
            if current_close >= retest_zone {
                structure.bos_bearish_retested = true;
            }
        }

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

    fn detect_sfp_driver<'a>(req: &EvalRequest<'a>, structure: &MarketStructure, atr_vals: &[f64]) -> Option<SetupDriver> {
        let n = req.h1_closes.as_ref()?.len();
        let current_high = req.h1_highs.as_ref()?[n - 1];
        let current_low = req.h1_lows.as_ref()?[n - 1];
        let current_close = req.h1_closes.as_ref()?[n - 1];
        let current_open = req.h1_opens.as_ref().map(|o| o[n-1]).unwrap_or(current_close);
        
        let candle_range = current_high - current_low;
        let body_size = (current_close - current_open).abs();
        let last_atr = atr_vals.last().cloned().unwrap_or(1.0);

        // Bullish SFP
        if current_low < structure.external_low.1 && current_close > structure.external_low.1 {
            let lower_wick = current_close.min(current_open) - current_low;
            let wick_body_ratio = if body_size > 0.0 { lower_wick / body_size } else { 10.0 };
            let sweep_depth = structure.external_low.1 - current_low;
            let close_pos = if candle_range > 0.0 { (current_close - current_low) / candle_range } else { 0.5 };
            
            if sweep_depth < (last_atr * 0.05) { return None; }

            return Some(SetupDriver::Sfp(SfpQuality {
                direction: SignalDirection::Long,
                wick_body_ratio,
                sweep_depth_atr: sweep_depth / last_atr,
                close_position: close_pos,
                is_vol_expansion: atr_pulse(atr_vals, 20, 1.5),
            }));
        }

        // Bearish SFP
        if current_high > structure.external_high.1 && current_close < structure.external_high.1 {
            let upper_wick = current_high - current_close.max(current_open);
            let wick_body_ratio = if body_size > 0.0 { upper_wick / body_size } else { 10.0 };
            let sweep_depth = current_high - structure.external_high.1;
            let rejection_strength = if candle_range > 0.0 { (current_high - current_close) / candle_range } else { 0.5 };

            if sweep_depth < (last_atr * 0.05) { return None; }

            return Some(SetupDriver::Sfp(SfpQuality {
                direction: SignalDirection::Short,
                wick_body_ratio,
                sweep_depth_atr: sweep_depth / last_atr,
                close_position: rejection_strength,
                is_vol_expansion: atr_pulse(atr_vals, 20, 1.5),
            }));
        }

        None
    }

    fn detect_displacement_driver<'a>(req: &EvalRequest<'a>, structure: &MarketStructure, settings: &SwingSettings, last_atr: f64) -> Option<SetupDriver> {
        let n = req.h1_closes.as_ref()?.len();
        let current_close = req.h1_closes.as_ref()?[n - 1];
        let current_open = req.h1_opens.as_ref().map(|o| o[n-1]).unwrap_or(current_close);
        let body_size = (current_close - current_open).abs();
        let rsi_val = rsi(req.h1_closes.as_ref()?.as_ref(), settings.displacement_rsi_period).last().cloned().unwrap_or(50.0);

        if (structure.is_bos_bullish && structure.bos_bullish_retested) || structure.is_choch_bullish {
            return Some(SetupDriver::BosRetest(DisplacementQuality {
                direction: SignalDirection::Long,
                strength_atr: body_size / last_atr,
                is_reversal: structure.is_choch_bullish,
                rsi_val,
            }));
        }

        if (structure.is_bos_bearish && structure.bos_bearish_retested) || structure.is_choch_bearish {
            return Some(SetupDriver::BosRetest(DisplacementQuality {
                direction: SignalDirection::Short,
                strength_atr: body_size / last_atr,
                is_reversal: structure.is_choch_bearish,
                rsi_val,
            }));
        }
        None
    }

    fn detect_ob_bounce_driver<'a>(req: &EvalRequest<'a>, order_blocks: &[PriceLevel], fvg_zones: &[PriceLevel], _settings: &SwingSettings, timeframe: &str) -> Option<SetupDriver> {
        let current_price = req.current_price;
        
        // Bullish OB Bounce
        if let Some(ob) = order_blocks.iter().find(|ob| ob.is_bullish.unwrap_or(false) && current_price <= ob.top && current_price >= ob.bottom * 0.998) {
             let has_fvg = fvg_zones.iter().any(|z| z.is_bullish.unwrap_or(false) && z.bottom <= ob.top && z.top >= ob.bottom);
             let depth = (ob.top - current_price) / (ob.top - ob.bottom);
             return Some(SetupDriver::OrderBlockBounce(ObQuality {
                 direction: SignalDirection::Long,
                 depth_ratio: depth.clamp(0.0, 1.0),
                 is_fvg_confluence: has_fvg,
                 timeframe: timeframe.to_string(),
             }));
        }

        // Bearish OB Bounce
        if let Some(ob) = order_blocks.iter().find(|ob| !ob.is_bullish.unwrap_or(true) && current_price >= ob.bottom && current_price <= ob.top * 1.002) {
             let has_fvg = fvg_zones.iter().any(|z| !z.is_bullish.unwrap_or(true) && z.bottom <= ob.top && z.top >= ob.bottom);
             let depth = (current_price - ob.bottom) / (ob.top - ob.bottom);
             return Some(SetupDriver::OrderBlockBounce(ObQuality {
                 direction: SignalDirection::Short,
                 depth_ratio: depth.clamp(0.0, 1.0),
                 is_fvg_confluence: has_fvg,
                 timeframe: timeframe.to_string(),
             }));
        }
        None
    }

    fn calculate_probability_score(
        driver: &SetupDriver,
        htf_bias: f64,
        volatility_regime: f64,
        ensemble_pred: f64,
        fractal_penalty: f64,
        settings: &SwingSettings,
    ) -> (f64, String) {
        let (mut probability, mut reason) = match driver {
            SetupDriver::Sfp(q) => (55.0, format!("SFP Setup ({})", q.direction)),
            SetupDriver::BosRetest(q) => (50.0, format!("Trend Continuation ({})", q.direction)),
            SetupDriver::OrderBlockBounce(q) => (if q.timeframe == "H1" { 48.0 } else { 45.0 }, format!("{} OB Bounce ({})", q.timeframe, q.direction)),
        };

        // Intrinsic Quality
        match driver {
            SetupDriver::Sfp(q) => {
                if q.wick_body_ratio > 2.0 { probability += 5.0; reason.push_str(" + Strong Rejection"); }
                else if q.wick_body_ratio < 0.5 { probability -= 10.0; reason.push_str(" - Weak Wick"); }
                
                if q.close_position > 0.8 { probability += 5.0; reason.push_str(" + Strong Close"); }
                if q.is_vol_expansion { probability += 5.0; reason.push_str(" + Vol Surge"); }
            },
            SetupDriver::BosRetest(q) => {
                if q.strength_atr > settings.displacement_atr_mult { probability += 5.0; reason.push_str(" + Strong Body"); }
                if q.is_reversal { probability -= 5.0; reason.push_str(" (Reversal Risk)"); }
                
                if q.direction == SignalDirection::Long && q.rsi_val > 60.0 { probability += 5.0; reason.push_str(" + RSI Mom"); }
                if q.direction == SignalDirection::Short && q.rsi_val < 40.0 { probability += 5.0; reason.push_str(" + RSI Mom"); }
            },
            SetupDriver::OrderBlockBounce(q) => {
                if q.is_fvg_confluence { probability += 10.0; reason.push_str(" + FVG Confluence"); }
                if q.depth_ratio > 0.8 { probability -= 10.0; reason.push_str(" - Deep Retrace"); }
                else if q.depth_ratio < 0.2 { probability += 5.0; reason.push_str(" + Precision Touch"); }
            }
        }

        // Context Modifiers
        let direction = match driver {
            SetupDriver::Sfp(q) => &q.direction,
            SetupDriver::BosRetest(q) => &q.direction,
            SetupDriver::OrderBlockBounce(q) => &q.direction,
        };

        // HTF Alignment
        if *direction == SignalDirection::Long {
            if htf_bias > 0.5 { probability += 15.0; reason.push_str(" + HTF Aligned"); }
            else if htf_bias < -0.3 { probability -= 20.0; reason.push_str(" - HTF Fight"); }
        } else {
            if htf_bias < -0.5 { probability += 15.0; reason.push_str(" + HTF Aligned"); }
            else if htf_bias > 0.3 { probability -= 20.0; reason.push_str(" - HTF Fight"); }
        }

        // Ensemble
        if ensemble_pred.abs() > 0.6 {
            if (*direction == SignalDirection::Long && ensemble_pred > 0.0) || (*direction == SignalDirection::Short && ensemble_pred < 0.0) {
                probability += 10.0; reason.push_str(" + ML Conf");
            } else {
                probability -= 10.0; reason.push_str(" - ML Divergence");
            }
        }

        // Volatility
        if volatility_regime > 1.3 {
            probability *= 0.9;
            reason.push_str(" [High Vol Penalty]");
        }

        // Fractal Guard
        if fractal_penalty > 0.0 {
            probability -= fractal_penalty;
            reason.push_str(" [Fractal Warn]");
        }

        (probability.clamp(0.0, 99.9), reason)
    }

    /// Calculates SL and TP based on ATR and setup type.
    fn risk_model(
        entry_type: &SignalDirection,
        reason: &str,
        entry_price: f64,
        last_atr: f64,
        structure: &MarketStructure,
        current_high: f64,
        current_low: f64,
        settings: &SwingSettings,
        vol_ratio: f64,
    ) -> (f64, f64, f64) {
        // Clamp max SL distance to avoid excessive risk on volatile candles (e.g. 3 ATRs)
        let max_sl_dist = last_atr * 3.0;
        let is_defensive = vol_ratio > 1.5;

        if *entry_type == SignalDirection::Long {
            let mut sl = if is_defensive {
                // Defensive Mode: ATR-first anchor.
                // In high volatility, internal structure often fails. We anchor to entry with a wide buffer.
                entry_price - (last_atr * 2.0)
            } else {
                // Standard Mode: Structure-first anchor.
                let sl_anchor = if reason.contains("SFP") { 
                    current_low 
                } else { 
                    if structure.internal_low.1 < entry_price && structure.internal_low.1 > 0.0 {
                        structure.internal_low.1
                    } else if structure.external_low.1 < entry_price && structure.external_low.1 > 0.0 {
                        structure.external_low.1 
                    } else { 
                        current_low 
                    }
                };
                sl_anchor - (last_atr * settings.sl_atr_buffer)
            };
            
            // Sanity Check: Ensure SL is below entry
            if sl >= entry_price {
                sl = entry_price - (last_atr * 0.5); // Fallback to 0.5 ATR stop
            }
            if (entry_price - sl) > max_sl_dist { sl = entry_price - max_sl_dist; }

            let risk = (entry_price - sl).abs();
            let tp1 = entry_price + risk * settings.risk_reward_ratio_tp1; // Aim for 1:2 R:R
            let tp2 = entry_price + risk * settings.risk_reward_ratio_tp2; // Aim for 1:4 R:R
            (sl, tp1, tp2)
        } else { // "short"
            let mut sl = if is_defensive {
                // Defensive Mode: ATR-first anchor.
                entry_price + (last_atr * 2.0)
            } else {
                // Standard Mode: Structure-first anchor.
                let sl_anchor = if reason.contains("SFP") { 
                    current_high 
                } else { 
                    if structure.internal_high.1 > entry_price && structure.internal_high.1 > 0.0 {
                        structure.internal_high.1
                    } else if structure.external_high.1 > entry_price && structure.external_high.1 > 0.0 {
                        structure.external_high.1
                    } else { 
                        current_high 
                    }
                };
                sl_anchor + (last_atr * settings.sl_atr_buffer)
            };
            
            // Sanity Check: Ensure SL is above entry
            if sl <= entry_price {
                sl = entry_price + (last_atr * 0.5); // Fallback to 0.5 ATR stop
            }
            if (sl - entry_price) > max_sl_dist { sl = entry_price + max_sl_dist; }

            let risk = (entry_price - sl).abs();
            let tp1 = entry_price - risk * settings.risk_reward_ratio_tp1;
            let tp2 = entry_price - risk * settings.risk_reward_ratio_tp2;
            (sl, tp1, tp2)
        }
    }

    /// Checks LTF structure to prevent entering against immediate momentum.
    /// Returns a penalty to subtract from your conviction score.
    pub fn fractal_guard(
        entry_type: &str, // Kept as str for internal helper usage or update to enum if preferred
        ltf_highs: Option<&[f64]>,
        ltf_lows: Option<&[f64]>,
        ltf_closes: Option<&[f64]>,
        current_price: f64,
        settings: &SwingSettings,
        timeframe_label: &str,
        lookback: usize,
    ) -> FractalGuardResult {
        if !settings.fractal_guard_enabled {
             return FractalGuardResult { is_fighting_trend: false, penalty_score: 0.0, warning: None };
        }

        let (Some(highs), Some(lows), Some(closes)) = (ltf_highs, ltf_lows, ltf_closes) else {
            return FractalGuardResult { is_fighting_trend: false, penalty_score: 0.0, warning: Some(format!("Missing {} Data", timeframe_label)) };
        };

        if highs.len() < 50 {
             return FractalGuardResult { is_fighting_trend: false, penalty_score: 0.0, warning: None };
        }

        let ltf_settings = SwingSettings {
            swing_lookback: lookback,
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

        // Determine LTF Trend using indices (External is always Extreme)
        // External High is Max. If Index(Ext) > Index(Int), Max is Newer => Higher High.
        // External Low is Min. If Index(Ext) > Index(Int), Min is Newer => Lower Low.
        let is_higher_high = ltf_structure.external_high.0 > ltf_structure.internal_high.0;
        let is_lower_high = !is_higher_high;
        
        let is_lower_low = ltf_structure.external_low.0 > ltf_structure.internal_low.0;
        let is_higher_low = !is_lower_low;

        let _is_ltf_downtrend = is_lower_high && is_lower_low;
        let _is_ltf_uptrend = is_higher_high && is_higher_low;

        match entry_type {
            "long" => {
                // DANGER: H1 says Long, but LTF is making Lower Lows (Downtrend) or breaking down
                let is_breaking_down = current_close < ltf_structure.internal_low.1;

                // Fix: Don't penalize just for downtrend (pullback), only for active breakdown
                if is_breaking_down {
                    result.is_fighting_trend = true;
                    result.penalty_score = settings.fractal_penalty_score;
                    result.warning = Some(format!("{} Structure Breakdown (Falling Knife)", timeframe_label));
                }
            },
            "short" => {
                // DANGER: H1 says Short, but LTF is making Higher Highs (Uptrend) or breaking up
                let is_breaking_up = current_close > ltf_structure.internal_high.1;

                // Fix: Don't penalize just for uptrend (pullback), only for active breakout
                if is_breaking_up {
                    result.is_fighting_trend = true;
                    result.penalty_score = settings.fractal_penalty_score;
                    result.warning = Some(format!("{} Structure Breakout (Step in front of train)", timeframe_label));
                }
            },
            _ => {}
        }

        result
    }
}