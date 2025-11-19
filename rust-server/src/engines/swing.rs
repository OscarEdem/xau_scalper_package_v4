use crate::{
    adx, atr, chandelier_exit_high, chandelier_exit_low, ema, rsi, EvalRequest, EvalResponse,
};
use ::uuid::Uuid;

pub struct SwingEngine;

impl SwingEngine {
    pub fn evaluate(req: &EvalRequest) -> EvalResponse {
        // --- 0. Parameter & Data Validation ---
        let adx_period = req.adx_period.unwrap_or(14);
        let adx_threshold = req.adx_threshold.unwrap_or(20.0);
        let mid_ema_period = req.ema_mid.unwrap_or(50);
        let sl_atr_multiplier = req.sl_atr_multiplier.unwrap_or(2.0);
        let tp_atr_multiplier = req.tp_atr_multiplier.unwrap_or(1.5);
        let persistence_bars = req.persistence_bars.unwrap_or(3);
        let atr_period = req.atr_period.unwrap_or(14);

        // Use H1 data for primary trend and signal generation
        let (Some(closes), Some(highs), Some(lows)) = (&req.h1_closes, &req.h1_highs, &req.h1_lows) else {
            return EvalResponse {
                reason: "H1 data (closes, highs, lows) is required for SwingEngine but was not provided.".to_string(),
                ..Default::default()
            };
        };

        let n = closes.len();

        let required_bars = mid_ema_period.max(adx_period).max(atr_period) + persistence_bars;
        if n < required_bars {
            return EvalResponse {
                reason: format!("Insufficient H1 data: need at least {} bars", required_bars),
                ..Default::default()
            };
        }

        // --- A. ENTRY CONDITIONS ---

        // 1. HTF Trend Alignment (M30/H1)
        let h1_emas = ema(&closes, mid_ema_period);
        let m30_emas = ema(&req.m30_closes, mid_ema_period);

        let h1_ema_slope = h1_emas[n - 1] - h1_emas[n - 1 - persistence_bars];
        let m30_ema_slope = m30_emas[req.m30_closes.len() - 1] - m30_emas[req.m30_closes.len() - 1 - persistence_bars];

        let is_buy_trend = h1_ema_slope > 0.0 && m30_ema_slope > 0.0;
        let is_sell_trend = h1_ema_slope < 0.0 && m30_ema_slope < 0.0;

        // 2. Trend Strength Filter: ADX
        let adx_vals = adx(&highs, &lows, &closes, adx_period);
        let last_adx = adx_vals.last().cloned().unwrap_or(0.0);
        if last_adx < adx_threshold {
            return EvalResponse {
                reason: format!("Weak trend: ADX {:.2} < {:.2}", last_adx, adx_threshold),
                ..Default::default()
            };
        }

        // 3. Continuation Setup: Pullback to Mid-EMA + Confirmation
        let last_close = closes[n - 1];
        let last_low = lows[n - 1];
        let last_high = highs[n - 1];
        let last_mid_ema = h1_emas[n - 1];
        let mut reason = "No valid entry signal".to_string();
        let mut entry_type = "none".to_string();

        // Using RSI for confirmation
        let rsi_vals = rsi(&closes, 14);
        let last_rsi = rsi_vals[n - 1];

        if is_buy_trend && last_low <= last_mid_ema && last_close > last_mid_ema && last_rsi > 50.0 {
            entry_type = "long".to_string();
            reason = "htf_trend(up) + adx_strong + pullback_to_ema_buy".to_string();
        } else if is_sell_trend && last_high >= last_mid_ema && last_close < last_mid_ema && last_rsi < 50.0 {
            entry_type = "short".to_string();
            reason = "htf_trend(down) + adx_strong + pullback_to_ema_sell".to_string();
        }

        if entry_type == "none" {
            return EvalResponse { reason, ..Default::default() };
        }

        // --- B. STOP LOSS / TAKE PROFIT ---
        let atr_vals = atr(&highs, &lows, &closes, atr_period);
        let last_atr = atr_vals.last().cloned().unwrap_or(0.0);

        if last_atr == 0.0 {
            return EvalResponse { reason: "ATR is zero, cannot calculate SL/TP".to_string(), ..Default::default() };
        }

        // Use Chandelier Exit for a dynamic, volatility-based stop loss.
        let chandelier_period = req.chandelier_period.unwrap_or(22);
        let chandelier_mult = req.chandelier_atr_mult.unwrap_or(3.0);

        let (sl_price, tp1_price, tp2_price) = if entry_type == "long" {
            let sl = chandelier_exit_low(&lows, &atr_vals, chandelier_period, chandelier_mult)
                .unwrap_or(req.current_price - last_atr * sl_atr_multiplier);
            let tp1 = req.current_price + last_atr * tp_atr_multiplier;
            let tp2 = req.current_price + last_atr * (tp_atr_multiplier * 2.0);
            (sl, tp1, tp2)
        } else { // Short
            let sl = chandelier_exit_high(&highs, &atr_vals, chandelier_period, chandelier_mult)
                .unwrap_or(req.current_price + last_atr * sl_atr_multiplier);
            let tp1 = req.current_price - last_atr * tp_atr_multiplier;
            let tp2 = req.current_price - last_atr * (tp_atr_multiplier * 2.0);
            (sl, tp1, tp2)
        };

        // --- C. TRADE MANAGEMENT ---
        // Time stop is less relevant for swing trades, but can be set as a failsafe
        let time_stop_seconds = (req.max_hold_bars.unwrap_or(240) as u32) * 3600; // N H1 bars * 3600 seconds/bar

        // --- D. OUTPUT ---
        EvalResponse {
            signal_id: Uuid::new_v4().to_string(),
            entry_type,
            entry_price: req.current_price,
            sl_price,
            tp1_price,
            tp2_price,
            time_stop_seconds,
            reason,
            classification: "swing".to_string(),
            ..Default::default()
        }
    }
}