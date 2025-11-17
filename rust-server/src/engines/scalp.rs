use crate::{
    atr, ema, find_imbalance_zones, find_swing_points, roc, EvalRequest, EvalResponse,
};
use ::uuid::Uuid;

pub struct ScalpEngine;

impl ScalpEngine {
    pub fn evaluate(req: &EvalRequest) -> EvalResponse {
        // --- 0. Parameter & Data Validation ---
        let fast_ema_period = req.ema_fast.unwrap_or(12);
        let slow_ema_period = req.ema_slow.unwrap_or(26);
        let roc_period = req.roc_period.unwrap_or(9);
        let roc_threshold = req.roc_threshold.unwrap_or(0.05);
        let max_spread_points = req.spread_limit_points.unwrap_or(20.0);
        let imbalance_mitigation = req.imbalance_mitigation.unwrap_or(true);
        let min_sl_pips = req.min_sl_pips.unwrap_or(10.0);
        let max_sl_pips = req.max_sl_pips.unwrap_or(50.0);
        let tp2_pips = req.tp2_pips; // Optional, from the request
        let atr_period = req.atr_period.unwrap_or(14);
        let sl_atr_mult = req.sl_atr_multiplier.unwrap_or(1.5);
        let tp_atr_mult = req.tp_atr_multiplier.unwrap_or(2.0);
        let atr_collapse_window = 20; // Look at last 20 ATR values for collapse

        // Use M5 data for scalping
        let closes = &req.m5_closes;
        let highs = &req.m5_highs;
        let lows = &req.m5_lows;
        let n = closes.len();

        if n < slow_ema_period.max(roc_period).max(atr_period + atr_collapse_window) {
            return EvalResponse {
                reason: format!("Insufficient M5 data: need at least {} bars", slow_ema_period.max(roc_period).max(atr_period + atr_collapse_window)),
                ..Default::default()
            };
        }

        // --- A. ENTRY CONDITIONS ---

        // 1. Trend Filter: EMA Crossover
        let fast_emas = ema(closes, fast_ema_period);
        let slow_emas = ema(closes, slow_ema_period);
        let last_fast_ema = fast_emas[n - 1];
        let last_slow_ema = slow_emas[n - 1];

        let is_buy_trend = last_fast_ema > last_slow_ema;
        let is_sell_trend = last_fast_ema < last_slow_ema;

        // 2. Momentum Filter: ROC Breakout
        let rocs = roc(closes, roc_period);
        let last_roc = rocs[n - 1];
        let is_buy_momentum = last_roc > roc_threshold;
        let is_sell_momentum = last_roc < -roc_threshold;

        // 3. Spread Filter
        let current_spread = req.spread_points.unwrap_or(0.0);
        if current_spread > max_spread_points {
            return EvalResponse {
                reason: format!("Spread too high: {:.1} > {:.1}", current_spread, max_spread_points),
                ..Default::default()
            };
        }

        // 4. Fair Value Gap (FVG) Filter
        let imbalance_zones = find_imbalance_zones(highs, lows, 10);
        if imbalance_mitigation {
            for zone in &imbalance_zones {
                if req.current_price > zone.bottom && req.current_price < zone.top {
                    return EvalResponse {
                        reason: "Price is inside a Fair Value Gap, signal rejected".to_string(),
                        ..Default::default()
                    };
                }
            }
        }

        // 5. Liquidity Sweep Detection (as a confirmation, not primary trigger)
        let (swing_highs, swing_lows) = find_swing_points(highs, lows, 20, 3);
        let current_high = highs[n - 1];
        let current_low = lows[n - 1];
        let mut sweep_detected = "none".to_string();

        if let Some((_, prev_swing_high)) = swing_highs.iter().filter(|(idx, _)| *idx < n - 1).last() {
            if current_high > *prev_swing_high { sweep_detected = "high_sweep".to_string(); }
        }
        if let Some((_, prev_swing_low)) = swing_lows.iter().filter(|(idx, _)| *idx < n - 1).last() {
            if current_low < *prev_swing_low { sweep_detected = "low_sweep".to_string(); }
        }

        // --- Combine Entry Signals ---
        let mut entry_type = "none".to_string();
        if is_buy_trend && is_buy_momentum {
            entry_type = "long".to_string();
        } else if is_sell_trend && is_sell_momentum {
            entry_type = "short".to_string();
        }

        if entry_type == "none" {
            return EvalResponse { reason: "No valid EMA/ROC signal".to_string(), ..Default::default() };
        }

        // --- B. STOP LOSS / TAKE PROFIT ---
        // Dynamic point value based on price decimals (e.g., 2 for XAUUSD -> 0.01)
        // For XAUUSD, with 2 decimals (e.g., 2050.15), a "point" is 0.01.
        // A "pip" is often considered the second-to-last digit, so for XAUUSD it's 0.1.
        // We will use the provided price_decimals to calculate the smallest price increment ("point").
        let point_value = 10.0_f64.powi(-req.price_decimals.unwrap_or(2));
        let atr_vals = atr(highs, lows, closes, atr_period);
        let last_atr = atr_vals.last().cloned().unwrap_or(0.0);

        if last_atr <= 0.0 {
            return EvalResponse { reason: "ATR is zero, cannot calculate volatility-based SL/TP".to_string(), ..Default::default() };
        }

        // Volatility-based SL/TP in points
        let sl_points = (sl_atr_mult * last_atr).max(min_sl_pips * point_value).min(max_sl_pips * point_value);
        // Use fixed pips if provided, otherwise use ATR multiplier.
        let tp1_points = if let Some(pips) = req.tp1_pips {
            pips * point_value
        } else {
            tp_atr_mult * last_atr
        };
        let tp2_points = if let Some(pips) = tp2_pips { pips * point_value } else { tp1_points * 2.0 };

        let (sl_price, mut tp1_price, tp2_price) = if entry_type == "long" {
            let sl = req.current_price - sl_points;
            let tp1 = req.current_price + tp1_points;
            let tp2 = req.current_price + tp2_points;
            (sl, tp1, tp2)
        } else { // Short
            let sl = req.current_price + sl_points;
            let tp1 = req.current_price - tp1_points;
            let tp2 = req.current_price + tp2_points;
            (sl, tp1, tp2)
        };

        // Override TP1 if mitigating imbalance
        if imbalance_mitigation {
            if entry_type == "long" {
                // Find the closest FVG bottom above the entry that offers at least 0.5R
                if let Some(fvg_target) = imbalance_zones.iter() // find the nearest FVG to fill
                    .filter(|z| z.bottom > req.current_price && (z.bottom - req.current_price) > (req.current_price - sl_price) * 0.5)
                    .map(|z| z.bottom)
                    .min_by(|a, b| a.partial_cmp(b).unwrap())
                {
                    tp1_price = tp1_price.min(fvg_target); // Take the more conservative TP
                }
            } else {
                // Find the closest FVG top below the entry that offers at least 0.5R
                if let Some(fvg_target) = imbalance_zones.iter() // find the nearest FVG to fill
                    .filter(|z| z.top < req.current_price && (req.current_price - z.top) > (sl_price - req.current_price) * 0.5)
                    .map(|z| z.top)
                    .max_by(|a, b| a.partial_cmp(b).unwrap())
                {
                    tp1_price = tp1_price.max(fvg_target); // Take the more conservative TP
                }
            }
        }

        // --- C. TRADE MANAGEMENT ---
        // Time stop in seconds (M5 bars * 300 seconds/bar)
        let time_stop_seconds = (req.max_hold_bars.unwrap_or(12) * 5 * 60) as u32;

        // --- D. OUTPUT ---
        EvalResponse {
            signal_id: Uuid::new_v4().to_string(),
            entry_type,
            entry_price: req.current_price,
            sl_price,
            tp1_price,
            tp2_price: if tp2_price > 0.0 { tp2_price } else { 0.0 },
            time_stop_seconds,
            reason: format!("ema_crossover + roc_breakout (sweep: {})", sweep_detected),
            classification: "scalp".to_string(),
            sweep_detected,
            ..Default::default()
        }
    }
}