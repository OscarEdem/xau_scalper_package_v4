use crate::{atr, ema, rsi, EvalRequest, EvalResponse};

pub fn evaluate_strategy(req: &EvalRequest) -> EvalResponse {
    let rsi_period = req.rsi_period.unwrap_or(14); // From backtest: 14
    let ema_fast_p = req.ema_fast.unwrap_or(11); // From backtest: 11
    let ema_slow_p = req.ema_slow.unwrap_or(50); // From backtest: 50
    let atr_period = req.atr_period.unwrap_or(14);
    let mut tp_pips = req.tp_pips.unwrap_or(0.0); // Will be calculated by ATR
    let mut sl_pips = req.sl_pips.unwrap_or(0.0); // Will be calculated by ATR

    // Ensure we have enough data for all indicators
    let required_data = ema_slow_p.max(rsi_period).max(atr_period) + 2;

    if req.closes.len() < required_data {
        return EvalResponse {
            action_advice: "none".into(), tp_pips, sl_pips, // Use default pips
            reason: format!("insufficient data: need at least {}", required_data),
            rsi: 50.0, ema_fast_last: 0.0, ema_slow_last: 0.0
        };
    }

    let ema_f = ema(&req.closes, ema_fast_p);
    let ema_s = ema(&req.closes, ema_slow_p);
    let rsi_vals = rsi(&req.closes, rsi_period);
    let atr_vals = atr(&req.highs, &req.lows, &req.closes, atr_period);

    let n = req.closes.len();
    let prev_fast = ema_f[n-2]; let last_fast = ema_f[n-1];
    let prev_slow = ema_s[n-2]; let last_slow = ema_s[n-1];
    let last_rsi = rsi_vals[n-1];
    let last_atr = atr_vals.last().cloned().unwrap_or(0.0);

    // --- Trader's Insight: Add Price Action Confirmation ---
    // For a buy, the close should be above the slow EMA. For a sell, below.
    let last_close = req.closes[n-1];
    let price_confirms_buy = last_close > last_slow;
    let price_confirms_sell = last_close < last_slow;

    let (action, reason) = if prev_fast <= prev_slow && last_fast > last_slow && price_confirms_buy {
        if last_rsi < 80.0 { ("buy".to_string(), format!("bull cross; RSI {:.1} < 80", last_rsi)) }
        else { ("none".to_string(), format!("bull cross; RSI {:.1} >= 80 (skip)", last_rsi)) }
    } else if prev_fast >= prev_slow && last_fast < last_slow && price_confirms_sell {
        if last_rsi > 20.0 { ("sell".to_string(), format!("bear cross; RSI {:.1} > 20", last_rsi)) }
        else { ("none".to_string(), format!("bear cross; RSI {:.1} <= 20 (skip)", last_rsi)) }
    } else {
        ("none".to_string(), "no crossover".to_string())
    };

    // --- Trader's Insight: Use ATR for dynamic TP/SL ---
    if action != "none" {
        let sl_multiplier = req.sl_atr_multiplier.unwrap_or(2.0); // From backtest: 2.0
        let tp_multiplier = req.tp_atr_multiplier.unwrap_or(1.5); // From backtest: 1.5

        let pip_size = 0.01;
        sl_pips = (last_atr * sl_multiplier) / pip_size;
        tp_pips = (last_atr * tp_multiplier) / pip_size;
    }

    EvalResponse { action_advice: action, tp_pips, sl_pips, reason, rsi: last_rsi, ema_fast_last: last_fast, ema_slow_last: last_slow }
}