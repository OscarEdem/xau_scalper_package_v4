use crate::{EvalRequest, EvalResponse};
use ::uuid::Uuid;
use crate::{adx, atr, chandelier_exit_high, chandelier_exit_low, detect_rsi_divergence, ema, get_daily_bias, get_trend_bias};

pub struct SwingEngine;

impl SwingEngine {
    pub fn evaluate(req: &EvalRequest) -> EvalResponse {
        // --- 0. Parameter & Data Validation ---
        let adx_period = req.adx_period.unwrap_or(14);
        let adx_threshold = req.adx_threshold.unwrap_or(25.0);
        let mid_ema_period = req.ema_mid.unwrap_or(50);
        let sl_atr_multiplier = req.sl_atr_multiplier.unwrap_or(2.0);
        let tp_atr_multiplier = req.tp_atr_multiplier.unwrap_or(1.5);
        let atr_period = req.atr_period.unwrap_or(14);
        // Use H1 data for primary trend and signal generation
        let (Some(closes), Some(highs), Some(lows)) = (&req.h1_closes, &req.h1_highs, &req.h1_lows) else {
            return EvalResponse {
                reason: "H1 data (closes, highs, lows) is required for SwingEngine but was not provided.".to_string(),
                ..Default::default()
            };
        };

        let n = closes.len();

        let required_bars = mid_ema_period.max(adx_period).max(atr_period);
        if n < required_bars {
            return EvalResponse {
                reason: format!("Insufficient H1 data: need at least {} bars", required_bars),
                ..Default::default()
            };
        }
        // --- NEW: FRACTAL CHECKS ---
        // A. H4 Trend Alignment
        // We default to 0 (Neutral) if H4 data is missing so we don't crash,
        // but for Gold, H4 data is critical.
        let h4_bias = if let Some(h4_closes) = &req.h4_closes {
            get_trend_bias(h4_closes, 50) // 50 EMA on H4
        } else {
            0
        };
        // B. Daily Context (Optional but recommended)
        // If yesterday was Bearish, be careful buying today unless deep discount.
        let daily_bias = if let (Some(d_opens), Some(d_closes)) = (&req.d1_opens, &req.d1_closes) {
            get_daily_bias(d_opens, d_closes)
        } else {
            "neutral".to_string()
        };
        // --- INDICATORS ---
        let h1_emas = ema(&closes, mid_ema_period);
        let adx_vals = adx(&highs, &lows, &closes, adx_period);
        let last_adx = adx_vals.last().cloned().unwrap_or(0.0);
        // --- NEW: DIVERGENCE CHECK ---
        let divergence = detect_rsi_divergence(lows, highs, closes, 14, 30);
        // --- ENTRY LOGIC ---
        let last_close = closes[n - 1];
        let last_mid_ema = h1_emas[n - 1];
        let is_h1_uptrend = last_close > last_mid_ema; // Simplified for brevity
        let is_h1_downtrend = last_close < last_mid_ema;

        let mut entry_type = "none".to_string();
        let mut conviction_score = 0.0;
        let mut reason = "No signal".to_string();

        // LONG LOGIC
        // 1. H1 is Uptrending
        // 2. H4 is NOT Bearish (Can be Bullish or Neutral)
        // 3. ADX shows valid trend strength
        if is_h1_uptrend && h4_bias >= 0 && last_adx > adx_threshold {
            // Pullback entry or Divergence entry
            if divergence == "bullish" {
                entry_type = "long".to_string();
                conviction_score = 85.0; // High conviction for divergence with trend
                reason = "H1_Trend + H4_Align + RSI_Bull_Div".to_string();
            } else if last_close > last_mid_ema && lows[n - 1] <= last_mid_ema {
                // Standard pullback to EMA
                entry_type = "long".to_string();
                conviction_score = 60.0;
                reason = "H1_Trend + H4_Align + EMA_Bounce".to_string();
            }
        }
        // SHORT LOGIC
        // 1. H1 is Downtrending
        // 2. H4 is NOT Bullish
        else if is_h1_downtrend && h4_bias <= 0 && last_adx > adx_threshold {
            if divergence == "bearish" {
                entry_type = "short".to_string();
                conviction_score = 85.0;
                reason = "H1_Trend + H4_Align + RSI_Bear_Div".to_string();
            } else if last_close < last_mid_ema && highs[n - 1] >= last_mid_ema {
                entry_type = "short".to_string();
                conviction_score = 60.0;
                reason = "H1_Trend + H4_Align + EMA_Reject".to_string();
            }
        }

        // If Daily Bias conflicts strongly, reduce conviction or kill trade
        if (entry_type == "long" && daily_bias == "bearish")
            || (entry_type == "short" && daily_bias == "bullish")
        {
            // Optional: Kill trade or reduce lot size
            conviction_score -= 20.0;
            reason = format!("{} (Warn: Daily Bias Conflict)", reason);
        }

        if entry_type == "none" {
            return EvalResponse {
                reason: "Structure/Fractal Mismatch".to_string(),
                ..Default::default()
            };
        }

        // --- B. STOP LOSS / TAKE PROFIT ---
        let atr_vals = atr(&highs, &lows, &closes, atr_period);
        let last_atr = atr_vals.last().cloned().unwrap_or(0.0);

        if last_atr <= 0.0 {
            return EvalResponse {
                reason: "ATR is zero, cannot calculate SL/TP".to_string(),
                ..Default::default()
            };
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
            tp2_price: if tp2_price > 0.0 { tp2_price } else { 0.0 },
            time_stop_seconds,
            reason,
            classification: "swing".to_string(),
            conviction_score: Some(conviction_score),
            ..Default::default()
        }
    }
}