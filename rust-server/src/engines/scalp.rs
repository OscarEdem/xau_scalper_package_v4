use crate::{EvalRequest, EvalResponse, PriceLevel, SignalDirection};
use ::uuid::Uuid;
use crate::{adx, get_trend_bias, find_imbalance_zones, find_swing_points, calculate_dynamic_thickness, engines::{news_guard, predictor_cache::PredictorCache, ensemble_predictor}, config::ScalpSettings};
use std::collections::HashMap;

#[derive(Debug, PartialEq)]
enum ScalpMode {
    Momentum,
    Pullback,
    Fade,
}

/// Internal struct to hold the decision from a strategy sub-function.
struct Decision {
    entry_type: SignalDirection,
    conviction: f64,
    reason: String,
    scalp_mode: ScalpMode,
    recommended_order_type: String,
    entry_price: f64,
    vol_regime_val: f64,
    tp2_target: Option<f64>, // Specific for Fade (Mean Reversion)
    debug_info: HashMap<String, String>,
}

impl Default for Decision {
    fn default() -> Self {
        Self {
            entry_type: SignalDirection::None,
            conviction: 0.0,
            reason: "No Signal".to_string(),
            scalp_mode: ScalpMode::Pullback,
            recommended_order_type: "none".to_string(),
            entry_price: 0.0,
            vol_regime_val: 1.0,
            tp2_target: None,
            debug_info: HashMap::new(),
        }
    }
}

pub struct ScalpEngine;

impl ScalpEngine {
    pub fn evaluate<'a>(req: &EvalRequest<'a>, predictor_cache: &PredictorCache, settings: &ScalpSettings) -> EvalResponse {
        // ---- SAFETY: ensure enough data ----
        let m5_closes = &req.m5_closes;
        let m1_closes = &req.closes; // m1 for triggers
        let n_m5 = m5_closes.len();
        let n_m1 = m1_closes.len();

        if n_m5 < settings.min_data_len || n_m1 < settings.min_data_len {
            return EvalResponse { reason: "Insufficient Data".to_string(), ..Default::default() };
        }

        // ---- Volatility: ATR on M5 (used to normalize thresholds) ----
        let atr_vals = crate::atr(&req.m5_highs, &req.m5_lows, m5_closes, settings.atr_period);
        let last_atr = atr_vals.last().cloned().unwrap_or(1.0).max(0.0001);

        // ---- Guard: Check for high-risk news or volatility before proceeding ----
        let adx_vals = adx(&req.m5_highs, &req.m5_lows, m5_closes, settings.atr_period);
        let guard = news_guard::combined_guard(
            &req.symbol,
            req.last_m1_timestamp,
            &req.upcoming_events.clone().unwrap_or_default(),
            settings.news_pre_event_block_minutes,
            settings.news_post_event_block_minutes,
            &atr_vals,
            &adx_vals,
            settings.news_guard_atr_spike_multiplier,
            req.adx_threshold.unwrap_or(10.0) // ADX threshold (configurable)
        );

        if !guard.allowed {
            return EvalResponse { reason: guard.reason.unwrap(), ..Default::default() };
        }

        // Common Data for Visualization & Logic
        let fvg_zones = find_imbalance_zones(&req.m5_highs, &req.m5_lows, 20);
        let (swing_highs, swing_lows) = find_swing_points(&req.m5_highs, &req.m5_lows, 60, 3);
        let last_swing_low = swing_lows.last().map(|(_, p)| *p);
        let last_swing_high = swing_highs.last().map(|(_, p)| *p);
        
        let mut liquidity_zones = Vec::new();
        for (idx, price) in swing_highs.iter().rev().take(2) {
            let thickness = calculate_dynamic_thickness(*idx, &req.m5_highs, &req.m5_lows, last_atr);
            liquidity_zones.push(PriceLevel { top: *price + thickness, bottom: *price, is_bullish: Some(false) });
        }
        for (idx, price) in swing_lows.iter().rev().take(2) {
            let thickness = calculate_dynamic_thickness(*idx, &req.m5_highs, &req.m5_lows, last_atr);
            liquidity_zones.push(PriceLevel { top: *price, bottom: *price - thickness, is_bullish: Some(true) });
        }

        // ---- STRATEGY SELECTION ----
        // 1. Try Fade Strategy (Priority)
        // 2. If no Fade, try Standard Strategy (Momentum/Pullback)
        
        let mut decision = if let Some(fade_decision) = Self::evaluate_fade(req, m5_closes, last_atr, settings, &adx_vals) {
            fade_decision
        } else {
            Self::evaluate_standard(req, predictor_cache, settings, m5_closes, m1_closes, last_atr, &atr_vals, last_swing_low, last_swing_high)
        };

        // ---- NEW: London Open Stop-Hunt Detector (Override) ----
        let session = Self::session_utc(req.last_m1_timestamp);
        let mut london_hunt_signal = "none";
        let mut asia_range_cache = None;
        
        if session == "london_open" {
            if let Some((asia_high, asia_low)) = Self::get_asia_range(&req.m5_highs, &req.m5_lows, req.m5_timestamps.as_deref(), req.last_m1_timestamp) {
                asia_range_cache = Some((asia_high, asia_low));
                if req.current_price > asia_high && m1_closes.last().unwrap_or(&0.0) < &asia_high {
                    london_hunt_signal = "bearish_hunt";
                } else if req.current_price < asia_low && m1_closes.last().unwrap_or(&0.0) > &asia_low {
                    london_hunt_signal = "bullish_hunt";
                }
            }
        }

        // ---- Session-Aware Gating ----
        let mode_allowed = match (session, &decision.scalp_mode) {
            // Asia: Only allow Pullback/Fade if explicitly enabled in settings
            ("asia", ScalpMode::Pullback) | ("asia", ScalpMode::Fade) => settings.allow_asia_trading,
            ("asia", ScalpMode::Momentum) => false, // Momentum is generally unsafe in Asia

            // London Open: Momentum is risky (fake-outs), so it's gated. Pullback/Fade always allowed.
            ("london_open", ScalpMode::Momentum) => settings.allow_london_open_momentum,
            ("london_open", _) => true,

            ("london_ny", _) => true,

            // NY Late: Momentum is risky (choppy/reversal), so it's gated.
            ("ny", ScalpMode::Momentum) => settings.allow_ny_late_momentum,
            ("ny", _) => true,

            _ => false,
        };

        if decision.entry_type != SignalDirection::None && !mode_allowed {
            decision.entry_type = SignalDirection::None;
            decision.reason = format!("No Signal ({} session blocks {:?})", session, decision.scalp_mode);
        }

        // Override gating if London Hunt detected (High Probability Setup)
        if london_hunt_signal != "none" {
            // 4️⃣ Fix: Require confirmation candle (Close back inside range)
            let last_close = *m1_closes.last().unwrap_or(&req.current_price);
            let confirmed = if let Some((asia_high, asia_low)) = asia_range_cache {
                if london_hunt_signal == "bullish_hunt" { last_close > asia_low } else { last_close < asia_high }
            } else { false };

            if confirmed {
                decision.entry_type = if london_hunt_signal == "bullish_hunt" { SignalDirection::Long } else { SignalDirection::Short };
                decision.reason = format!("London Open Stop-Hunt: Swept Asia {}", if london_hunt_signal == "bullish_hunt" { "Low" } else { "High" });
                decision.entry_price = req.current_price;
                decision.recommended_order_type = format!("market_{}", decision.entry_type);
            }
        }

        // ---- Risk Calculation (SL/TP) ----
        let (sl, tp1, tp2) = Self::calculate_risk(&mut decision, last_atr, last_swing_low, last_swing_high, settings);

        // ---- Dynamic Position Sizing ----
        let base_size = 1.0;
        let session_mult = match session {
            "london_ny" => 1.5,
            "london_open" => 1.0,
            "ny" => 0.8,
            _ => 0.5,
        };
        let conviction_mult = if decision.conviction > 80.0 { 1.2 } else { 1.0 };
        let setup_mult = if london_hunt_signal != "none" { 1.5 } else { 1.0 };
        let position_size = base_size * session_mult * conviction_mult * setup_mult;

        // 5️⃣ Fix: Position sizing inversely proportional to SL distance
        let position_size = if sl != 0.0 && last_atr > 0.0 {
            let risk_atr = (decision.entry_price - sl).abs() / last_atr;
            let risk_mult = if risk_atr > 0.0 { (1.0 / risk_atr).clamp(0.5, 1.5) } else { 1.0 };
            position_size * risk_mult
        } else { position_size };

        // Add common debug info
        decision.debug_info.insert("atr".to_string(), format!("{:.5}", last_atr));
        decision.debug_info.insert("entry_atr".to_string(), format!("{:.5}", last_atr));
        decision.debug_info.insert("vol_regime".to_string(), format!("{:.2}", decision.vol_regime_val));

        // ---- Deterministic Signal ID ----
        let signal_id = if decision.entry_type != SignalDirection::None {
            let m5_timestamp = req.last_m1_timestamp - (req.last_m1_timestamp % 300);
            format!("{}-{}-{}", req.symbol, decision.entry_type.to_string(), m5_timestamp)
        } else {
            Uuid::new_v4().to_string()
        };

        let scalp_mode_str = if decision.entry_type != SignalDirection::None {
            Some(match decision.scalp_mode {
                ScalpMode::Momentum => "momentum".to_string(),
                ScalpMode::Pullback => "pullback".to_string(),
                ScalpMode::Fade => "fade".to_string(),
            })
        } else {
            None
        };

        // Determine sweep string for UI
        let sweep_detected = decision.debug_info.get("inducement").map(|s| match s.as_str() {
            "bullish_inducement" => "low_sweep".to_string(),
            "bearish_inducement" => "high_sweep".to_string(),
            _ => "none".to_string(),
        }).unwrap_or("none".to_string());

        EvalResponse {
            signal_id,
            entry_type: decision.entry_type,
            entry_price: decision.entry_price,
            sl_price: sl,
            tp1_price: tp1,
            tp2_price: tp2,
            reason: decision.reason,
            classification: "scalp".to_string(),
            scalp_mode: scalp_mode_str,
            conviction_score: Some(decision.conviction),
            suggested_position_size: Some(position_size),
            recommended_order_type: decision.recommended_order_type,
            limit_order_price: decision.entry_price,
            expiration_seconds: Some(settings.limit_order_expiration),
            time_stop_seconds: settings.time_stop_seconds,
            imbalance_zones: fvg_zones,
            liquidity_zones,
            order_blocks: vec![],
            sweep_detected,
            volatility_regime: if decision.vol_regime_val > 1.5 { "high".to_string() } else if decision.vol_regime_val < 0.8 { "low".to_string() } else { "normal".to_string() },
            debug_info: Some(decision.debug_info),
            ..Default::default()
        }
    }

    /// Evaluates the "Fade" strategy (Mean Reversion).
    /// Returns Some(Decision) if a fade setup is detected, None otherwise.
    fn evaluate_fade(req: &EvalRequest, m5_closes: &[f64], last_atr: f64, settings: &ScalpSettings, adx_vals: &[f64]) -> Option<Decision> {
        if m5_closes.len() >= settings.fade_sma_period {
            // 3️⃣ Fix: Fade Guards (ADX & Trend Bias)
            if let Some(&last_adx) = adx_vals.last() {
                if last_adx > settings.fade_max_adx { return None; }
            }
            
            // Calculate bands first to check price location
            let sma_period = settings.fade_sma_period;
            let slice = &m5_closes[m5_closes.len() - sma_period..];
            let sma_val = slice.iter().sum::<f64>() / sma_period as f64;
            
            let variance = slice.iter().map(|x| (x - sma_val).powi(2)).sum::<f64>() / sma_period as f64;
            let std_dev = variance.sqrt();
            
            let upper_fence = sma_val + (settings.fade_std_dev_mult * std_dev);
            let lower_fence = sma_val - (settings.fade_std_dev_mult * std_dev);

            // Never fade strength: Check trend bias
            let trend_bias = get_trend_bias(m5_closes, 20);
            if (req.current_price > upper_fence && trend_bias > 0) || (req.current_price < lower_fence && trend_bias < 0) {
                return None;
            }
            
            // Parabolic Move: M1 Range > Factor * M5 ATR
            let current_m1_high = req.highs.last().cloned().unwrap_or(req.current_price);
            let current_m1_low = req.lows.last().cloned().unwrap_or(req.current_price);
            let m1_range = current_m1_high - current_m1_low;
            let is_parabolic = m1_range > (settings.fade_parabolic_atr_mult * last_atr);

            let mut decision = Decision::default();
            let mut triggered = false;
            
            if req.current_price >= upper_fence && is_parabolic {
                // Fix C: Safer Fade Trigger (Check for rejection candle)
                let m1_close = *req.closes.last().unwrap_or(&0.0);
                let prev_m1_close = if req.closes.len() > 1 { req.closes[req.closes.len() - 2] } else { m1_close };
                if m1_close < prev_m1_close {
                    decision.entry_type = SignalDirection::Short;
                    decision.reason = format!("Fade Short: Parabolic Rejection at Upper Fence");
                    triggered = true;
                }
            } else if req.current_price <= lower_fence && is_parabolic {
                let m1_close = *req.closes.last().unwrap_or(&0.0);
                let prev_m1_close = if req.closes.len() > 1 { req.closes[req.closes.len() - 2] } else { m1_close };
                if m1_close > prev_m1_close {
                    decision.entry_type = SignalDirection::Long;
                    decision.reason = format!("Fade Long: Parabolic Rejection at Lower Fence");
                    triggered = true;
                }
            }

            if triggered {
                decision.scalp_mode = ScalpMode::Fade;
                decision.conviction = 85.0; // High conviction, but not maxed out
                decision.recommended_order_type = format!("limit_{}", decision.entry_type);
                decision.entry_price = req.current_price;
                decision.vol_regime_val = 2.0; // Parabolic implies high volatility
                decision.tp2_target = Some(sma_val);
                return Some(decision);
            }
        }
        None
    }

    /// Evaluates Standard strategies (Momentum and Pullback).
    fn evaluate_standard(
        req: &EvalRequest,
        predictor_cache: &PredictorCache,
        settings: &ScalpSettings,
        m5_closes: &[f64],
        m1_closes: &[f64],
        last_atr: f64,
        atr_vals: &[f64],
        last_swing_low: Option<f64>,
        last_swing_high: Option<f64>
    ) -> Decision {
            let mut decision = Decision::default();

            // Context bias: prefer H4 then H1 fallback
            let h4_bias = if let Some(h4_closes) = &req.h4_closes {
                get_trend_bias(h4_closes, 40)
            } else { 0 };
            let h1_bias = if let Some(h1_closes) = &req.h1_closes {
                get_trend_bias(h1_closes, 20)
            } else { 0 };
            let bias = if h4_bias != 0 { h4_bias } else { h1_bias };

            // Kalman slope
            let kf_q = req.kf_process_noise.unwrap_or(0.01);
            let kf_r = req.kf_measurement_noise.unwrap_or(0.1);
            let (k_est, k_slope) = crate::kalman_slope(m5_closes, settings.kalman_period, kf_q, kf_r);
            let norm_k_slope = k_slope / last_atr;
            decision.debug_info.insert("kalman_slope".to_string(), format!("{:.4}", norm_k_slope));

            // Inducement detection
            let inducement = crate::detect_inducement(&req.m5_highs, &req.m5_lows, m5_closes, 5);
            decision.debug_info.insert("inducement".to_string(), inducement.clone());
            
            let inducement_score = match inducement.as_str() {
                "bullish_inducement" | "bearish_inducement" => 1.0,
                _ => 0.0
            };

            // Micro timing trigger
            let m1_roc = crate::roc(m1_closes, settings.m1_roc_period);
            let m1_surge = m1_roc.last().cloned().unwrap_or(0.0);
            let m1_atr_equivalent = last_atr / settings.m1_atr_conversion_div;
            let norm_m1_surge = if m1_atr_equivalent > 0.0 { m1_surge / m1_atr_equivalent } else { 0.0 };
            decision.debug_info.insert("m1_surge".to_string(), format!("{:.4}", norm_m1_surge));

            // Dynamic thresholds
            let avg_atr: f64 = if atr_vals.len() > 0 {
                atr_vals.iter().skip(atr_vals.len().saturating_sub(50)).sum::<f64>() / 50.0_f64.min(atr_vals.len() as f64)
            } else {
                last_atr
            };
            let vol_ratio = if avg_atr > 0.0 { last_atr / avg_atr } else { 1.0 };
            let vol_regime = if vol_ratio > 1.0 { vol_ratio.sqrt() } else { vol_ratio }.clamp(settings.vol_regime_clamp_min, settings.vol_regime_clamp_max);
            decision.vol_regime_val = vol_regime;

            let kalman_slope_threshold = settings.base_kalman_threshold * vol_regime;
            let m1_surge_threshold = settings.base_m1_surge_threshold * vol_regime;

            // Signal scoring
            let kalman_score = (norm_k_slope.abs() / (kalman_slope_threshold * 2.0)).clamp(0.0, 1.0);
            let m1_score = (norm_m1_surge.abs() / (m1_surge_threshold * 2.0)).clamp(0.0, 1.0);

            let is_bull_flow = norm_k_slope > (kalman_slope_threshold * settings.flow_threshold_mult);
            let is_bear_flow = norm_k_slope < -(kalman_slope_threshold * settings.flow_threshold_mult);
            let is_bull_m1 = norm_m1_surge > (m1_surge_threshold * settings.flow_threshold_mult);
            let is_bear_m1 = norm_m1_surge < -(m1_surge_threshold * settings.flow_threshold_mult);
            let is_bear_m1_surge = norm_m1_surge < -(m1_surge_threshold); // M1 crashing
            let is_bull_m1_surge = norm_m1_surge > (m1_surge_threshold); // M1 spiking

            let mut long_reasons = Vec::new();
            let mut long_score = 0.0;
            let mut short_reasons = Vec::new();
            let mut short_score = 0.0;

            let mut htf_weight = settings.htf_bias_weight;
            let mut ensemble_weight = settings.ensemble_weight;

            if (bias > 0 && is_bear_flow) || (bias < 0 && is_bull_flow) {
                htf_weight *= 0.5;
                ensemble_weight *= 0.5;
            }

            if bias > 0 {
                long_score += htf_weight;
                long_reasons.push("HTF Bullish Bias");
            } else if bias < 0 {
                short_score += htf_weight;
                short_reasons.push("HTF Bearish Bias");
            }

            let final_prediction_bias = ensemble_predictor::calculate_bias(
                predictor_cache, "m5", m5_closes, req.current_price, 1.0,
            );
            decision.debug_info.insert("ensemble_bias".to_string(), format!("{:.4}", final_prediction_bias));

            if final_prediction_bias > 0.0 {
                long_score += ensemble_weight * (final_prediction_bias / last_atr).clamp(0.0, 1.5);
                long_reasons.push("Ensemble Bullish Bias");
            } else if final_prediction_bias < 0.0 {
                short_score += ensemble_weight * (final_prediction_bias.abs() / last_atr).clamp(0.0, 1.5);
                short_reasons.push("Ensemble Bearish Bias");
            }

            if is_bull_flow {
                long_score += kalman_score * settings.kalman_weight;
                long_reasons.push("Bullish M5 Flow");

                // Fix B: Pullback Buy Logic
                if is_bear_m1_surge {
                    long_score += m1_score * settings.m1_surge_weight * 1.5;
                    long_reasons.push("Discount Entry (Bearish M1 Surge in Bull Trend)");
                } else if is_bull_m1 {
                    long_score += m1_score * settings.m1_surge_weight;
                    long_reasons.push("M1 Momentum Alignment");
                }
            }
            if is_bear_flow {
                short_score += kalman_score * settings.kalman_weight;
                short_reasons.push("Bearish M5 Flow");

                // Fix B: Pullback Sell Logic
                if is_bull_m1_surge {
                    short_score += m1_score * settings.m1_surge_weight * 1.5;
                    short_reasons.push("Premium Entry (Bullish M1 Surge in Bear Trend)");
                } else if is_bear_m1 {
                    short_score += m1_score * settings.m1_surge_weight;
                    short_reasons.push("M1 Momentum Alignment");
                }
            }

            if is_bull_flow && is_bull_m1 {
                long_score += settings.flow_surge_confluence_boost; 
                long_reasons.push("Flow+Surge Confluence");
            }
            if is_bear_flow && is_bear_m1 {
                short_score += settings.flow_surge_confluence_boost;
                short_reasons.push("Flow+Surge Confluence");
            }

            if inducement.as_str() == "bullish_inducement" {
                long_score += settings.inducement_weight * inducement_score;
                long_reasons.push("Bullish Inducement");
                short_score *= settings.inducement_opposing_reduction;
            } else if inducement.as_str() == "bearish_inducement" {
                short_score += settings.inducement_weight * inducement_score;
                short_reasons.push("Bearish Inducement");
                long_score *= settings.inducement_opposing_reduction;
            }

            let conv_long = (1.0 / (1.0 + (-settings.logistic_scale * (long_score - settings.logistic_offset)).exp())) * 100.0;
            let conv_short = (1.0 / (1.0 + (-settings.logistic_scale * (short_score - settings.logistic_offset)).exp())) * 100.0;
            decision.debug_info.insert("conviction_long".to_string(), format!("{:.2}", conv_long));
            decision.debug_info.insert("conviction_short".to_string(), format!("{:.2}", conv_short));
            decision.debug_info.insert("raw_score_long".to_string(), format!("{:.4}", long_score));
            decision.debug_info.insert("raw_score_short".to_string(), format!("{:.4}", short_score));

            let min_conv_to_trade = settings.min_conviction;
            // Add hysteresis to prevent flickering between Long/Short when scores are close
            let hysteresis = 5.0;

            if conv_long >= min_conv_to_trade && conv_long > (conv_short + hysteresis) {
                decision.entry_type = SignalDirection::Long;
                decision.conviction = conv_long;
                decision.reason = long_reasons.join(" + ");
            } else if conv_short >= min_conv_to_trade && conv_short > (conv_long + hysteresis) {
                decision.entry_type = SignalDirection::Short;
                decision.conviction = conv_short;
                decision.reason = short_reasons.join(" + ");
            } else {
                let (leaning_reasons, score) = if conv_long >= conv_short { (long_reasons, conv_long) } else { (short_reasons, conv_short) };
                let reasons_str = leaning_reasons.join(" + ");
                let detailed_reason = if !reasons_str.is_empty() {
                    format!("No Signal (Low Conviction {:.1}%): {}", score, reasons_str)
                } else {
                    "No Signal (Low Conviction)".to_string()
                };
                decision.entry_type = SignalDirection::None;
                decision.conviction = 0.0;
                decision.reason = detailed_reason;
            };

            // Execution Decision
            if decision.entry_type == SignalDirection::None {
                decision.recommended_order_type = "none".to_string();
                decision.entry_price = 0.0;
            } else {
                let suffix = decision.entry_type.to_string();
                if inducement_score > 0.0 {
                    decision.recommended_order_type = format!("market_{}", suffix);
                    decision.entry_price = req.current_price;
                } else {
                    let max_limit_distance = last_atr * settings.max_limit_dist_atr_mult;
                    let desired_limit = k_est;
                    let distance = (desired_limit - req.current_price).abs();
                    if distance <= max_limit_distance {
                        decision.recommended_order_type = format!("limit_{}", suffix);
                        decision.entry_price = desired_limit;
                    } else {
                        decision.recommended_order_type = format!("market_{}", suffix);
                        decision.entry_price = req.current_price;
                    }
                }
            }

            // Mode Classification
            let near_structure = match decision.entry_type {
                SignalDirection::Long => last_swing_low.map(|p| (decision.entry_price - p) < last_atr).unwrap_or(false),
                SignalDirection::Short => last_swing_high.map(|p| (p - decision.entry_price) < last_atr).unwrap_or(false),
                _ => false,
            };

            decision.scalp_mode = if inducement_score == 0.0 
                && kalman_score > settings.momentum_kalman_threshold 
                && m1_score > settings.momentum_m1_threshold 
                && !near_structure 
            {
                ScalpMode::Momentum
            } else {
                ScalpMode::Pullback
            };
            
            decision
    }

    /// Calculates Risk Parameters (SL, TP1, TP2) based on the decision mode.
    fn calculate_risk(
        decision: &mut Decision,
        last_atr: f64,
        last_swing_low: Option<f64>,
        last_swing_high: Option<f64>,
        settings: &ScalpSettings
    ) -> (f64, f64, f64) {
        let mut sl = 0.0;
        let mut tp1 = 0.0;
        let mut tp2 = 0.0;
        let entry_price = decision.entry_price;
        let entry_type = &decision.entry_type;

        if *entry_type != SignalDirection::None {
            match decision.scalp_mode {
                ScalpMode::Fade => {
                    let sl_dist = last_atr * settings.fade_sl_atr_mult;
                    let tp1_dist = last_atr * settings.fade_tp1_atr_mult;
                    if *entry_type == SignalDirection::Long {
                        sl = entry_price - sl_dist;
                        tp1 = entry_price + tp1_dist;
                    } else {
                        sl = entry_price + sl_dist;
                        tp1 = entry_price - tp1_dist;
                    }
                    tp2 = decision.tp2_target.unwrap_or(entry_price); // Mean Reversion Target
                },
                ScalpMode::Momentum => {
                    // 1️⃣ Fix: Enforce floor on Momentum SL
                    let risk = (last_atr * settings.momentum_risk_atr_mult).max(last_atr * settings.momentum_min_risk_atr);
                    sl = if *entry_type == SignalDirection::Long { entry_price - risk } else { entry_price + risk };
                    tp1 = if *entry_type == SignalDirection::Long { entry_price + last_atr * settings.momentum_tp1_atr_mult } else { entry_price - last_atr * settings.momentum_tp1_atr_mult };
                    tp2 = if *entry_type == SignalDirection::Long { entry_price + last_atr * settings.momentum_tp2_atr_mult } else { entry_price - last_atr * settings.momentum_tp2_atr_mult };
                },
                ScalpMode::Pullback => {
                    // 2️⃣ Fix: Structure-first, ATR-second. No structure = No trade.
                    let base_sl = match entry_type {
                        SignalDirection::Long => last_swing_low,
                        SignalDirection::Short => last_swing_high,
                        _ => None,
                    };

                    if let Some(anchor) = base_sl {
                        sl = if *entry_type == SignalDirection::Long { anchor - last_atr * settings.pullback_sl_atr_mult } else { anchor + last_atr * settings.pullback_sl_atr_mult };
                        tp1 = if *entry_type == SignalDirection::Long { entry_price + last_atr * settings.pullback_tp1_atr_mult } else { entry_price - last_atr * settings.pullback_tp1_atr_mult };
                        tp2 = if *entry_type == SignalDirection::Long { entry_price + last_atr * settings.pullback_tp2_atr_mult } else { entry_price - last_atr * settings.pullback_tp2_atr_mult };
                    } else {
                        return (0.0, 0.0, 0.0);
                    }
                }
            }

            // Step 2.5: Enforce Directionality (Sanity Check)
            // Ensure SL is always on the correct side of entry before clamping distance
            if *entry_type == SignalDirection::Long && sl >= entry_price {
                sl = entry_price - (last_atr * settings.min_sl_atr_mult);
            } else if *entry_type == SignalDirection::Short && sl <= entry_price {
                sl = entry_price + (last_atr * settings.min_sl_atr_mult);
            }

            // Step 3: Safety Clamp (ATR Guardrail)
            let sl_distance = (entry_price - sl).abs();
            let min_sl = last_atr * settings.min_sl_atr_mult;
            let max_sl = last_atr * settings.max_sl_atr_mult;

            if sl_distance < min_sl {
                // Too tight: Push SL away to meet min_sl
                sl = if *entry_type == SignalDirection::Long { entry_price - min_sl } else { entry_price + min_sl };
            } else if sl_distance > max_sl {
                // Too wide: Pull SL closer to meet max_sl
                sl = if *entry_type == SignalDirection::Long { entry_price - max_sl } else { entry_price + max_sl };
            }
        }
        (sl, tp1, tp2)
    }

    pub fn session_utc(ts: i64) -> &'static str {
        let hour = (ts % 86400) / 3600;
        match hour {
            0..=6 => "asia",
            7..=10 => "london_open",
            11..=16 => "london_ny",
            17..=20 => "ny",
            _ => "dead",
        }
    }

    fn get_asia_range(highs: &[f64], lows: &[f64], timestamps: Option<&[i64]>, current_ts: i64) -> Option<(f64, f64)> {
        let day_start = current_ts - (current_ts % 86400);
        let asia_end = day_start + 7 * 3600;
        
        let mut max_h = f64::MIN;
        let mut min_l = f64::MAX;
        let mut found = false;

        if let Some(ts_vec) = timestamps {
            // Fix A: Robust Asia Range with explicit timestamps
            for (i, &ts) in ts_vec.iter().enumerate() {
                if i >= highs.len() || i >= lows.len() { continue; }
                if ts >= day_start && ts < asia_end {
                    max_h = max_h.max(highs[i]);
                    min_l = min_l.min(lows[i]);
                    found = true;
                }
            }
        } else {
            // Fallback to index-based calculation if timestamps missing
            for (i, &h) in highs.iter().enumerate().rev() {
                let bar_ts = current_ts - ((highs.len() - 1 - i) as i64 * 300);
                if bar_ts >= day_start && bar_ts < asia_end {
                    max_h = max_h.max(h);
                    min_l = min_l.min(lows[i]);
                    found = true;
                }
                if bar_ts < day_start { break; }
            }
        }
        
        if found { Some((max_h, min_l)) } else { None }
    }
}