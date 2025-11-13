use crate::{atr, ema, rsi, sma, stochastic, EvalRequest, EvalResponse};

pub fn evaluate_strategy(req: &EvalRequest) -> EvalResponse {
    let rsi_period = req.rsi_period.unwrap_or(16); // From backtest: 16
    let ema_fast_p = req.ema_fast.unwrap_or(5);   // From backtest: 5
    let ema_slow_p = req.ema_slow.unwrap_or(50); // From backtest: 50
    let atr_period = req.atr_period.unwrap_or(14);
    let sma_period = req.sma_period.unwrap_or(200);
    let stoch_k_period = req.stoch_k_period.unwrap_or(14); // Default K period
    let stoch_d_period = req.stoch_d_period.unwrap_or(3);  // Default D period
    let stoch_slowing = req.stoch_slowing.unwrap_or(3);    // Default slowing for %K
    let mut tp_pips = req.tp_pips.unwrap_or(0.0); // Will be calculated by ATR
    let mut sl_pips = req.sl_pips.unwrap_or(0.0); // Will be calculated by ATR

    // Ensure we have enough data for all indicators
    let required_data = ema_slow_p.max(rsi_period).max(atr_period).max(sma_period)
        .max(stoch_k_period + stoch_slowing + stoch_d_period) + 2; // Adjusted for Stochastic

    if req.closes.len() < required_data || req.m5_closes.len() < ema_slow_p {
        return EvalResponse {
            action_advice: "none".into(), tp_pips, sl_pips, // Use default pips
            reason: format!("insufficient data: need at least {}", required_data),
            atr: 0.0,
            rsi: 50.0,
            ema_fast_last: 0.0,
            ema_slow_last: 0.0,
            sma_last: 0.0,
            stoch_k_last: 0.0, // Fix: Add missing field
            stoch_d_last: 0.0, // Fix: Add missing field
            conviction_score: 0, // Fix: Add missing field
        };
    }

    let ema_f = ema(&req.closes, ema_fast_p);
    let ema_s = ema(&req.closes, ema_slow_p);
    let rsi_vals = rsi(&req.closes, rsi_period);
    let atr_vals = atr(&req.highs, &req.lows, &req.closes, atr_period);
    let sma_vals = sma(&req.closes, sma_period); // New: Calculate SMA
    let (stoch_k_vals, stoch_d_vals) = stochastic(&req.highs, &req.lows, &req.closes, stoch_k_period, stoch_d_period, stoch_slowing);
    
    // --- New: Multi-Timeframe Analysis ---
    let m5_ema_slow = ema(&req.m5_closes, ema_slow_p);

    let n = req.closes.len();
    let prev_fast = ema_f[n-2]; let last_fast = ema_f[n-1];
    let prev_slow = ema_s[n-2]; let last_slow = ema_s[n-1];
    let last_rsi = rsi_vals[n-1];
    let last_atr = atr_vals.last().cloned().unwrap_or(0.0);
    let last_sma = sma_vals.last().cloned().unwrap_or(0.0);
    let prev_stoch_k = stoch_k_vals[n-2]; let last_stoch_k = stoch_k_vals[n-1];
    let prev_stoch_d = stoch_d_vals[n-2]; let last_stoch_d = stoch_d_vals[n-1];
    let last_m5_ema_slow = m5_ema_slow.last().cloned().unwrap_or(0.0);
    
    let last_close = req.closes[n-1];
    
    let mut action = "none".to_string();
    let mut reason = "no signal".to_string();
    let mut conviction_score: u8 = 0;

    // --- Evaluate Buy Signal Potential and Score ---
    let mut potential_buy_score: u8 = 0;
    let ema_buy_cross = prev_fast <= prev_slow && last_fast > last_slow;
    let price_confirms_buy = last_close > last_slow; // Price above slow EMA
    let trend_confirms_buy = last_close > last_m5_ema_slow; // Price above M5 slow EMA
    let rsi_ok_buy = last_rsi < 80.0; // RSI not overbought
    let stoch_confirms_buy = prev_stoch_k <= prev_stoch_d && last_stoch_k > last_stoch_d && last_stoch_k < 80.0; // Stochastic K crossing D from below 80

    if ema_buy_cross { potential_buy_score += 1; }
    if price_confirms_buy { potential_buy_score += 1; }
    if trend_confirms_buy { potential_buy_score += 1; }
    if rsi_ok_buy { potential_buy_score += 1; }
    if stoch_confirms_buy { potential_buy_score += 1; }

    // --- Evaluate Sell Signal Potential and Score ---
    let mut potential_sell_score: u8 = 0;
    let ema_sell_cross = prev_fast >= prev_slow && last_fast < last_slow;
    let price_confirms_sell = last_close < last_slow; // Price below slow EMA
    let trend_confirms_sell = last_close < last_m5_ema_slow; // Price below M5 slow EMA
    let rsi_ok_sell = last_rsi > 20.0; // RSI not oversold
    let stoch_confirms_sell = prev_stoch_k >= prev_stoch_d && last_stoch_k < last_stoch_d && last_stoch_k > 20.0; // Stochastic K crossing D from above 20

    if ema_sell_cross { potential_sell_score += 1; }
    if price_confirms_sell { potential_sell_score += 1; }
    if trend_confirms_sell { potential_sell_score += 1; }
    if rsi_ok_sell { potential_sell_score += 1; }
    if stoch_confirms_sell { potential_sell_score += 1; }

    // --- Determine final action and conviction score ---
    if potential_buy_score >= 4 { // Strong buy signal
        action = "buy".to_string();
        reason = format!("strong buy signal with score {}/5", potential_buy_score);
        conviction_score = potential_buy_score;
    } else if potential_sell_score >= 4 { // Strong sell signal
        action = "sell".to_string();
        reason = format!("strong sell signal with score {}/5", potential_sell_score);
        conviction_score = potential_sell_score;
    } else if potential_buy_score == 3 { // Weak buy signal
        action = "buy".to_string();
        reason = format!("potential buy signal with score {}/5", potential_buy_score);
        conviction_score = potential_buy_score;
    } else if potential_sell_score == 3 { // Weak sell signal
        action = "sell".to_string();
        reason = format!("potential sell signal with score {}/5", potential_sell_score);
        conviction_score = potential_sell_score;
    } else {
        // If no full signal, we still want to return the highest conviction score for potential signals
        conviction_score = potential_buy_score.max(potential_sell_score); // will be < 3
        if conviction_score > 0 { // conviction_score will be < 4 here
            reason = format!("no full signal (max score: {})", conviction_score);
        } else {
            reason = "no signal".to_string();
        }
    }

    // --- Trader's Insight: Use ATR for dynamic TP/SL ---
    // Calculate TP/SL if a potential or full signal is generated
    if conviction_score >= 3 {
        let sl_multiplier = req.sl_atr_multiplier.unwrap_or(1.0); // From backtest: 1.0
        let tp_multiplier = req.tp_atr_multiplier.unwrap_or(1.5); // From backtest: 1.5

        let pip_size = 0.01;
        sl_pips = (last_atr * sl_multiplier) / pip_size;
        tp_pips = (last_atr * tp_multiplier) / pip_size;
    } else {
        // If no full signal, set TP/SL to 0 to prevent accidental trades
        tp_pips = 0.0;
        sl_pips = 0.0;
    }

    EvalResponse {
        action_advice: action,
        tp_pips,
        sl_pips,
        reason,
        rsi: last_rsi,
        ema_fast_last: last_fast,
        ema_slow_last: last_slow,
        atr: last_atr,
        sma_last: last_sma,
        stoch_k_last: last_stoch_k, // Fix: Use correct variable name
        stoch_d_last: last_stoch_d, // Fix: Use correct variable name
        conviction_score, // New
    }
}