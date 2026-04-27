use crate::{EvalRequest, EvalResponse, PriceLevel, SignalDirection};
use ::uuid::Uuid;
use crate::{adx, get_trend_bias, find_imbalance_zones, find_swing_points, calculate_dynamic_thickness, engines::{news_guard, predictor_cache::PredictorCache, ensemble_predictor}, config::{ScalpSettings, RiskSettings}};
use std::collections::HashMap;
use tracing::info;

#[derive(Debug, PartialEq, Clone)]
enum ExpectancyClass {
    Linear,     // For trend-following (Momentum, Pullback)
    Asymmetric, // For counter-trend (Fade, Stop-Hunt)
}

#[derive(Debug, PartialEq, Clone)]
enum ScalpMode {
    Momentum,
    Pullback,
    Fade,
    LondonHunt,
}

/// Internal struct to hold the decision from a strategy sub-function.
#[derive(Debug, Clone)]
struct Decision {
    entry_type: SignalDirection,
    conviction: f64,
    reason: String,
    scalp_mode: ScalpMode,
    expectancy_class: ExpectancyClass,
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
            expectancy_class: ExpectancyClass::Linear,
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
    pub fn evaluate<'a>(req: &EvalRequest<'a>, predictor_cache: &PredictorCache, settings: &ScalpSettings, risk_settings: &RiskSettings) -> EvalResponse {
        // ---- SAFETY: ensure enough data ----
        let m5_closes = &req.m5_closes;
        let m1_closes = &req.closes;
        let n_m5 = m5_closes.len();
        let n_m1 = m1_closes.len();

        if n_m5 < settings.min_data_len || n_m1 < settings.min_data_len {
            return EvalResponse { reason: "Insufficient Data".to_string(), ..Default::default() };
        }

        // ---- Guard: Spread Check ----
        if let (Some(spread), Some(limit)) = (req.spread_points, req.spread_limit_points) {
            if spread > limit {
                return EvalResponse { reason: format!("Spread too high: {:.0} > {:.0}", spread, limit), ..Default::default() };
            }
        }

        // ---- Volatility: ATR on M5 (used to normalize thresholds) ----
        let atr_vals = crate::atr(&req.m5_highs, &req.m5_lows, &req.m5_closes, settings.atr_period);
        let last_atr = atr_vals.last().cloned().unwrap_or(1.0).max(0.0001);

        // ---- Guard: Check for high-risk news or volatility before proceeding ----
        let adx_vals = adx(&req.m5_highs, &req.m5_lows, &req.m5_closes, settings.atr_period);
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

        // ---- 0. Pre-calculation (Shared) ----
        let avg_atr: f64 = if atr_vals.len() > 0 {
            atr_vals.iter().skip(atr_vals.len().saturating_sub(50)).sum::<f64>() / 50.0_f64.min(atr_vals.len() as f64)
        } else {
            last_atr
        };
        let vol_ratio = if avg_atr > 0.0 { last_atr / avg_atr } else { 1.0 };
        // Clamp regime to avoid extreme scaling
        let vol_regime = if vol_ratio > 1.0 { vol_ratio.sqrt() } else { vol_ratio }.clamp(0.75, 1.5);

        // Common Structure
        let (swing_highs, swing_lows) = find_swing_points(&req.m5_highs, &req.m5_lows, 60, 3);
        let fvg_zones = find_imbalance_zones(&req.m5_highs, &req.m5_lows, 20);
        
        let last_swing_low = swing_lows.last().map(|(_, p)| *p);
        let last_swing_high = swing_highs.last().map(|(_, p)| *p);

        // ---- 1. Hierarchy of execution (The "Waterfall") ----
        // We evaluate strictly in order of Setup Quality/Specificity.
        
        // Priority A: Time-Based Specialist Setups (London Hunt)
        let mut decision = if let Some(d) = Self::evaluate_london_hunt(req, settings) {
            d
        // Priority B: Trend Momentum (Breakouts/Fast Flow) - Prioritize Trend Following
        } else if let Some(d) = Self::evaluate_momentum(req, settings, predictor_cache, last_atr, vol_regime, &adx_vals) {
            d
        // Priority C: Trend Pullback (Discount Entries)
        } else if let Some(d) = Self::evaluate_pullback(req, settings, predictor_cache, last_atr, vol_regime) {
            d
        // Priority D: Mean Reversion (Fade) - Lowest priority
        } else if let Some(d) = Self::evaluate_fade(req, m5_closes, last_atr, settings, &adx_vals, last_swing_high, last_swing_low, &fvg_zones) {
            d
        } else {
            Decision::default()
        };

        // ---- Session-Aware Gating ----
        let session = Self::session_utc(req.last_m1_timestamp);
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

        // ---- Risk Calculation (SL/TP) ----
        let (sl, tp1, tp2) = Self::calculate_risk(&mut decision, last_atr, last_swing_low, last_swing_high, settings);

        // ---- Dynamic Position Sizing ----
        // 1. Calculate Base Lot Size from Risk Settings (Equity % / SL Distance)
        let mut base_size = 0.0;
        if sl != 0.0 && decision.entry_price > 0.0 {
            let sl_dist = (decision.entry_price - sl).abs();
            if sl_dist > 0.00001 {
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
                let risk_amt = equity * risk_settings.risk_per_trade_pct;
                let risk_per_lot = sl_dist * risk_settings.xauusd_lot_point_value;
                base_size = risk_amt / risk_per_lot;
            }
        }

        let session_mult = match session {
            "london_ny" => 1.5,
            "london_open" => 1.0,
            "ny" => 0.8,
            _ => 0.5,
        };
        let conviction_mult = if decision.expectancy_class == ExpectancyClass::Linear && decision.conviction > 80.0 {
            1.2
        } else {
            1.0
        };
        let position_size = base_size * session_mult * conviction_mult;
        
        // 5️⃣ Fix: Position sizing inversely proportional to SL distance
        let position_size = if sl != 0.0 && last_atr > 0.0 {
            let risk_atr = (decision.entry_price - sl).abs() / last_atr;
            
            // Mode-Aware Risk Scaling
            let risk_mult = match decision.scalp_mode {
                ScalpMode::Momentum => {
                    // Momentum: Inverse-SL OK (Aggressive scaling for tight stops)
                    if risk_atr > 0.0 { (1.0 / risk_atr).clamp(0.5, 1.5) } else { 1.0 }
                },
                ScalpMode::Pullback => {
                    // Pullback: Mild inverse (Dampened scaling)
                    if risk_atr > 0.0 { (1.0 / risk_atr.sqrt()).clamp(0.75, 1.25) } else { 1.0 }
                },
                ScalpMode::Fade => {
                    // Fade: Fixed size only (Never scale up on tight stops)
                    1.0
                },
                ScalpMode::LondonHunt => 1.2, // Aggressive on specialist setups
            };

            position_size * risk_mult
        } else { position_size };

        // Round to 2 decimal places (standard lot size precision)
        let position_size = (position_size * 100.0).round() / 100.0;

        // Add common debug info
        decision.debug_info.insert("atr_m5".to_string(), format!("{:.5}", last_atr));
        if sl != 0.0 && last_atr > 0.0 {
            let risk_in_atr = (decision.entry_price - sl).abs() / last_atr;
            decision.debug_info.insert("entry_sl_atr".to_string(), format!("{:.2}", risk_in_atr));
        }
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
                ScalpMode::LondonHunt => "london_hunt".to_string(),
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

        // Construct Liquidity Zones for visualization
        let mut liquidity_zones = Vec::new();
        for (idx, price) in swing_highs.iter().rev().take(2) {
            let thickness = calculate_dynamic_thickness(*idx, &req.m5_highs, &req.m5_lows, last_atr);
            liquidity_zones.push(PriceLevel { top: *price + thickness, bottom: *price, is_bullish: Some(false) });
        }
        for (idx, price) in swing_lows.iter().rev().take(2) {
            let thickness = calculate_dynamic_thickness(*idx, &req.m5_highs, &req.m5_lows, last_atr);
            liquidity_zones.push(PriceLevel { top: *price, bottom: *price - thickness, is_bullish: Some(true) });
        }

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

    // --- STRATEGY 1: LONDON HUNT (Specialist) ---
    fn evaluate_london_hunt(req: &EvalRequest, settings: &ScalpSettings) -> Option<Decision> {
        let session = Self::session_utc(req.last_m1_timestamp);
        // Allow London Open, London/NY Overlap, and NY Session
        if session != "london_open" && session != "london_ny" && session != "ny" { return None; }

        let (asia_high, asia_low) = Self::get_asia_range(&req.m5_highs, &req.m5_lows, req.m5_timestamps.as_deref(), req.last_m1_timestamp)?;
        let current_price = req.current_price;
        let last_close = *req.closes.last().unwrap_or(&current_price);
        
        // Sniper Logic: Check if M1 recently traded beyond the level (using closes as proxy if highs missing)
        let m1_n = req.closes.len();
        let prev_m1_close = if m1_n > 1 { req.closes[m1_n - 2] } else { current_price };

        // Dynamic conviction: scale with sweep depth relative to ATR
        let atr_vals = crate::atr(&req.m5_highs, &req.m5_lows, &req.m5_closes, settings.atr_period);
        let last_atr = atr_vals.last().cloned().unwrap_or(1.0).max(0.0001);

        // Logic: Price swept Asia High/Low and closed back inside
        if current_price < asia_high && last_close < asia_high && (req.m5_highs.last().map(|&h| h > asia_high).unwrap_or(false) || prev_m1_close > asia_high) {
             // Bearish Hunt (Swept High)
             let sweep_high = req.m5_highs.last().cloned().unwrap_or(asia_high);
             let sweep_depth_atr = (sweep_high - asia_high) / last_atr;
             // Base 70 + up to 20 bonus for deep sweeps; cap at 92
             let conviction = (70.0 + (sweep_depth_atr * 20.0).min(20.0)).min(92.0);
             return Some(Decision {
                 entry_type: SignalDirection::Short,
                 conviction,
                 reason: format!("London Hunt: Asia High Sweep (depth {:.2} ATR)", sweep_depth_atr),
                 scalp_mode: ScalpMode::LondonHunt,
                 expectancy_class: ExpectancyClass::Asymmetric,
                 entry_price: current_price,
                 recommended_order_type: "market_short".to_string(),
                 ..Default::default()
             });
        }
        
        if current_price > asia_low && last_close > asia_low && (req.m5_lows.last().map(|&l| l < asia_low).unwrap_or(false) || prev_m1_close < asia_low) {
             // Bullish Hunt (Swept Low)
             let sweep_low = req.m5_lows.last().cloned().unwrap_or(asia_low);
             let sweep_depth_atr = (asia_low - sweep_low) / last_atr;
             let conviction = (70.0 + (sweep_depth_atr * 20.0).min(20.0)).min(92.0);
             return Some(Decision {
                 entry_type: SignalDirection::Long,
                 conviction,
                 reason: format!("London Hunt: Asia Low Sweep (depth {:.2} ATR)", sweep_depth_atr),
                 scalp_mode: ScalpMode::LondonHunt,
                 expectancy_class: ExpectancyClass::Asymmetric,
                 entry_price: current_price,
                 recommended_order_type: "market_long".to_string(),
                 ..Default::default()
             });
        }
        None
    }

    /// Evaluates the "Fade" strategy (Mean Reversion).
    /// Returns Some(Decision) if a fade setup is detected, None otherwise.
    fn evaluate_fade(
        req: &EvalRequest, 
        m5_closes: &[f64], 
        last_atr: f64, 
        settings: &ScalpSettings, 
        adx_vals: &[f64],
        last_swing_high: Option<f64>,
        last_swing_low: Option<f64>,
        fvg_zones: &[PriceLevel]
    ) -> Option<Decision> {
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
                // 4️⃣ Fix: Liquidity Context Guard (Sweep or FVG)
                // Use M5 high/low as M1 data might be sparse in request
                let current_high = req.m5_highs.last().cloned().unwrap_or(req.current_price);
                let swept_high = last_swing_high.map(|h| current_high > h).unwrap_or(false);
                let in_bearish_fvg = fvg_zones.iter().any(|z| !z.is_bullish.unwrap_or(true) && req.current_price >= z.bottom && req.current_price <= z.top);

                if !swept_high && !in_bearish_fvg {
                    return None;
                }

                // Fix C: Safer Fade Trigger (Check for rejection candle)
                let m1_close = *req.closes.last().unwrap_or(&0.0);
                let prev_m1_close = if req.closes.len() > 1 { req.closes[req.closes.len() - 2] } else { m1_close };
                if m1_close < prev_m1_close {
                    decision.entry_type = SignalDirection::Short;
                    decision.reason = format!("Fade Short: Parabolic Rejection + Liquidity Context");
                    triggered = true;
                }
            } else if req.current_price <= lower_fence && is_parabolic {
                // 4️⃣ Fix: Liquidity Context Guard (Sweep or FVG)
                let current_low = req.m5_lows.last().cloned().unwrap_or(req.current_price);
                let swept_low = last_swing_low.map(|l| current_low < l).unwrap_or(false);
                let in_bullish_fvg = fvg_zones.iter().any(|z| z.is_bullish.unwrap_or(false) && req.current_price >= z.bottom && req.current_price <= z.top);

                if !swept_low && !in_bullish_fvg {
                    return None;
                }

                let m1_close = *req.closes.last().unwrap_or(&0.0);
                let prev_m1_close = if req.closes.len() > 1 { req.closes[req.closes.len() - 2] } else { m1_close };
                if m1_close > prev_m1_close {
                    decision.entry_type = SignalDirection::Long;
                    decision.reason = format!("Fade Long: Parabolic Rejection + Liquidity Context");
                    triggered = true;
                }
            }

            if triggered {
                decision.scalp_mode = ScalpMode::Fade;
                decision.expectancy_class = ExpectancyClass::Asymmetric;
                // Dynamic conviction: lower ADX = cleaner range = higher conviction for fade
                // Ranges from 65 (ADX near 30) to 85 (ADX near 0)
                let last_adx = adx_vals.last().cloned().unwrap_or(15.0);
                decision.conviction = (85.0 - (last_adx / settings.fade_max_adx) * 20.0).clamp(65.0, 85.0);
                decision.recommended_order_type = format!("limit_{}", decision.entry_type);
                decision.entry_price = req.current_price;
                decision.vol_regime_val = 2.0; // Parabolic implies high volatility
                decision.tp2_target = Some(sma_val);
                return Some(decision);
            }
        }
        None
    }

    // --- STRATEGY 3: MOMENTUM (Trend Following - Fast) ---
    fn evaluate_momentum(
        req: &EvalRequest,
        settings: &ScalpSettings,
        predictor_cache: &PredictorCache,
        last_atr: f64,
        vol_regime: f64,
        adx_vals: &[f64]
    ) -> Option<Decision> {
        let m5_closes = &req.m5_closes;
        
        // 0. ADX Filter: Avoid Momentum entries in low ADX (Chop)
        if let Some(&last_adx) = adx_vals.last() {
            if last_adx < 20.0 { return None; }
        }

        // 1. Flow Check (Kalman)
        let kf_q = req.kf_process_noise.unwrap_or(0.01);
        let kf_r = req.kf_measurement_noise.unwrap_or(0.1);
        let (_, k_slope) = crate::kalman_slope(m5_closes, settings.kalman_period, kf_q, kf_r);
        let norm_slope = k_slope / last_atr;
        let threshold = settings.base_kalman_threshold * vol_regime; // Scale threshold by volatility
        
        // 2. Surge Check (M1 Price Delta vs ATR)
        // Fix: Use Price Change in ATR units instead of ROC % which is too small for XAUUSD
        let m1_n = req.closes.len();
        let roc_p = settings.m1_roc_period;
        let current_close = *req.closes.last().unwrap_or(&req.current_price);
        let prev_close_n = if m1_n > roc_p { req.closes[m1_n - 1 - roc_p] } else { req.closes.first().cloned().unwrap_or(current_close) };
        
        let m1_surge = (current_close - prev_close_n) / last_atr; // e.g. 0.3 means moved 0.3 ATR in N minutes
        
        // LOGGING: Verify m1_surge calculation in console
        info!(symbol = %req.symbol, m1_surge = m1_surge, threshold = settings.base_m1_surge_threshold, "Momentum Surge Check");

        // Requirement: Both M5 Flow AND M1 Surge must agree and be strong
        let is_bullish = norm_slope > threshold && m1_surge > settings.base_m1_surge_threshold;
        let is_bearish = norm_slope < -threshold && m1_surge < -settings.base_m1_surge_threshold;

        if !is_bullish && !is_bearish { return None; }

        // 3. Ensemble Confirmation (Required for Momentum)
        if settings.momentum_require_ml_confluence {
            let ml_bias = ensemble_predictor::calculate_bias(predictor_cache, "m5", m5_closes, req.current_price, 1.0);
            if (is_bullish && ml_bias < 0.0) || (is_bearish && ml_bias > 0.0) {
                return None; // ML disagrees, filter fakeout
            }
        }

        let direction = if is_bullish { SignalDirection::Long } else { SignalDirection::Short };
        
        let mut debug_info = HashMap::new();
        debug_info.insert("m1_surge".to_string(), format!("{:.3}", m1_surge));
        debug_info.insert("kalman_slope".to_string(), format!("{:.3}", norm_slope));
        
        Some(Decision {
            entry_type: direction,
            conviction: 75.0 + (norm_slope.abs() * 10.0).min(15.0), // Base 75 + Bonus
            reason: format!("Momentum: Strong Flow ({:.2}) + Surge ({:.2})", norm_slope, m1_surge),
            scalp_mode: ScalpMode::Momentum,
            expectancy_class: ExpectancyClass::Linear,
            entry_price: req.current_price,
            vol_regime_val: vol_regime,
            recommended_order_type: format!("market_{}", if is_bullish { "long" } else { "short" }),
            debug_info,
            ..Default::default()
        })
    }

    // --- STRATEGY 4: PULLBACK (Trend Following - Discount) ---
    fn evaluate_pullback(
        req: &EvalRequest, 
        settings: &ScalpSettings, 
        _predictor_cache: &PredictorCache,
        last_atr: f64,
        vol_regime: f64
    ) -> Option<Decision> {
        let m5_closes = &req.m5_closes;
        
        // 1. Flow Check (Kalman) - Must still be trending
        let kf_q = req.kf_process_noise.unwrap_or(0.01);
        let kf_r = req.kf_measurement_noise.unwrap_or(0.1);
        let (k_est, k_slope) = crate::kalman_slope(m5_closes, settings.kalman_period, kf_q, kf_r);
        let norm_slope = k_slope / last_atr;
        // Lower threshold for pullback (trend can be decelerating slightly)
        let threshold = (settings.base_kalman_threshold * 0.7) * vol_regime; 

        if norm_slope.abs() < threshold { return None; } // No trend to pullback into

        // 2. M1 Rejection (The Logic Flip)
        // For Momentum, we wanted M1 Surge WITH trend.
        // For Pullback, we want M1 moving AGAINST trend, then turning? 
        // OR we simply want Price < Kalman Estimate (Discount) for Longs.
        
        let dist_to_fair_value = (req.current_price - k_est) / last_atr;

        // 3. Confirmation: Require M1 candle to show resumption (Close > Prev Close)
        // This prevents catching a falling knife.
        let m1_n = req.closes.len();
        let current_close = *req.closes.last().unwrap_or(&req.current_price);
        let prev_close = if m1_n > 1 { req.closes[m1_n - 2] } else { current_close };

        // Logic: Trend is UP, but Price is slightly cheap (Pullback)
        // FIX: Require 2 consecutive closes in the resumption direction to avoid falling-knife entries.
        let threshold_dist = settings.pullback_entry_displacement_atr;
        let prev_close_2 = if m1_n > 2 { req.closes[m1_n - 3] } else { prev_close };

        let is_bullish_pullback = norm_slope > 0.0
            && dist_to_fair_value < -threshold_dist
            && current_close > prev_close      // Bar N closes up
            && prev_close > prev_close_2;      // Bar N-1 also closed up (2-bar confirmation)

        let is_bearish_pullback = norm_slope < 0.0
            && dist_to_fair_value > threshold_dist
            && current_close < prev_close      // Bar N closes down
            && prev_close < prev_close_2;      // Bar N-1 also closed down (2-bar confirmation)

        if !is_bullish_pullback && !is_bearish_pullback { return None; }

        let direction = if is_bullish_pullback { SignalDirection::Long } else { SignalDirection::Short };

        Some(Decision {
            entry_type: direction,
            conviction: 65.0, // Lower base conviction than momentum (fighting immediate flow)
            reason: format!("Pullback: Trend ({:.2}) + Value Area ({:.2} ATR)", norm_slope, dist_to_fair_value),
            scalp_mode: ScalpMode::Pullback,
            expectancy_class: ExpectancyClass::Linear,
            entry_price: req.current_price,
            vol_regime_val: vol_regime,
            recommended_order_type: format!("market_{}", if is_bullish_pullback { "long" } else { "short" }),
            ..Default::default()
        })
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
                },
                ScalpMode::LondonHunt => {
                    // Tight stop above the sweep
                    let sl_dist = last_atr * 0.5;
                    if *entry_type == SignalDirection::Long {
                         sl = entry_price - sl_dist;
                         tp1 = entry_price + (last_atr * 3.0);
                         tp2 = entry_price + (last_atr * 5.0);
                    } else {
                         sl = entry_price + sl_dist;
                         tp1 = entry_price - (last_atr * 3.0);
                         tp2 = entry_price - (last_atr * 5.0);
                    }
                },
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