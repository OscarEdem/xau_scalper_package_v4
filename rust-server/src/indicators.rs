use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

#[derive(Debug, Serialize, Deserialize, Clone, ToSchema, IntoParams)]
#[serde(rename_all = "camelCase")]
pub struct OpenPosition {
    pub ticket: u64,
    pub symbol: String,
    pub direction: String, // "buy" or "sell"
    pub entry_price: f64,
    pub sl: f64,
    pub tp: f64,
    pub lot_size: f64,
    pub entry_timestamp: String, // ISO
    pub mode: String, // "scalp" | "swing"
}

#[derive(Debug, Serialize, Deserialize, Clone, ToSchema, IntoParams, Default)]
#[serde(rename_all = "camelCase")]
pub struct EvalRequest {
    pub symbol: String,
    pub timeframe: String,
    pub closes: Vec<f64>, // For a new candle, this might just contain one value
    pub highs: Vec<f64>,  // For a new candle, this might just contain one value
    pub opens: Vec<f64>,
    pub lows: Vec<f64>,
    pub volumes: Vec<u64>,
    pub m5_closes: Vec<f64>, // These can be sent as full buffers or as single new values
    pub m5_highs: Vec<f64>,
    pub m5_lows: Vec<f64>,
    pub m30_closes: Vec<f64>,
    pub h1_closes: Option<Vec<f64>>,
    pub h1_highs: Option<Vec<f64>>,
    pub h1_lows: Option<Vec<f64>>,
    pub open_positions: Option<Vec<OpenPosition>>, // current live trades
    // Optional base parameters
    pub rsi_period: Option<usize>,
    pub ema_fast: Option<usize>,
    pub ema_slow: Option<usize>,
    pub atr_period: Option<usize>,
    pub sma_period: Option<usize>,
    pub stoch_k_period: Option<usize>,
    pub stoch_d_period: Option<usize>,
    pub stoch_slowing: Option<usize>,
    pub tp1_pips: Option<f64>,
    pub tp2_pips: Option<f64>,
    pub sl_pips: Option<f64>,
    pub tp_atr_multiplier: Option<f64>,
    pub sl_atr_multiplier: Option<f64>,
    // New scalping-focused parameters
    pub ema_mid: Option<usize>,
    pub confirmation_bars: Option<usize>,
    pub spread_limit_points: Option<f64>,
    pub imbalance_mitigation: Option<bool>,
    pub atr_regime_threshold: Option<f64>,
    pub spread_points: Option<f64>,
    pub price_decimals: Option<i32>,
    pub min_sl_pips: Option<f64>,
    pub max_sl_pips: Option<f64>,
    // Early entry parameters
    pub roc_period: Option<usize>,
    pub roc_threshold: Option<f64>,
    // new swing params:
    pub adx_period: Option<usize>,
    pub adx_threshold: Option<f64>,
    pub persistence_bars: Option<usize>,
    pub chandelier_period: Option<usize>,
    pub chandelier_atr_mult: Option<f64>,
    pub max_hold_bars: Option<usize>,
    // Timestamp synchronization
    pub last_m1_timestamp: i64,
    pub last_m5_timestamp: Option<i64>,
    pub last_m30_timestamp: Option<i64>,
    pub last_h1_timestamp: Option<i64>,
    // New fields for engine mode
    pub mode: String, // "scalp" or "swing"
    pub current_price: f64,
}

#[derive(Debug, Serialize, Deserialize, Clone, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PriceLevel {
    pub top: f64,
    pub bottom: f64,
}

#[derive(Debug, Serialize, Deserialize, Clone, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct VwapBands {
    pub vwap: f64,
    pub upper_band1: f64,
    pub lower_band1: f64,
    pub upper_band2: f64,
    pub lower_band2: f64,
}

#[derive(Debug, Serialize, Deserialize, Clone, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EvalResponse {
    pub signal_id: String, // e.g., a UUID or timestamp-based ID
    pub entry_type: String, // "long", "short", "none"
    pub entry_price: f64,
    pub sl_price: f64,
    pub tp1_price: f64, // Imbalance fill
    pub tp2_price: f64, // Liquidity target
    pub tp3_price: f64, // VWAP band target
    pub time_stop_seconds: u32,
    pub volatility_regime: String, // "low", "medium", "high"
    pub sweep_detected: String, // "high_sweep", "low_sweep", "none"
    pub imbalance_zones: Vec<PriceLevel>,
    pub liquidity_zones: Vec<PriceLevel>,
    pub vwap_bands: Option<VwapBands>,
    pub reason: String,
    pub classification: String, // "scalp" or "swing"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conviction_score: Option<f64>,
}

impl Default for EvalResponse {
    fn default() -> Self {
        Self {
            signal_id: "".to_string(),
            entry_type: "none".to_string(),
            entry_price: 0.0,
            sl_price: 0.0,
            tp1_price: 0.0,
            tp2_price: 0.0,
            tp3_price: 0.0,
            time_stop_seconds: 0,
            volatility_regime: "low".to_string(),
            sweep_detected: "none".to_string(),
            imbalance_zones: vec![],
            liquidity_zones: vec![],
            vwap_bands: None,
            reason: "No signal".to_string(),
            classification: "none".to_string(),
            conviction_score: None,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, ToSchema)]
pub struct TradeLog {
    pub timestamp: String,
    pub event_type: String, // "Open" or "Close"
    pub ticket: u64,
    pub symbol: String,
    pub direction: String,
    pub lot_size: f64,
    pub price: f64,
    pub sl: f64,
    pub tp: f64,
    pub profit: f64,
    pub comment: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, ToSchema, IntoParams)]
#[serde(rename_all = "camelCase")]
pub struct ArrivalConfirmation {
    pub signal_id: String,
    pub arrival_price: f64,
    pub spread: f64,
    pub timestamp: String, // ISO
}

#[derive(Debug, Serialize, Deserialize, Clone, ToSchema, IntoParams)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionConfirmation {
    pub signal_id: String,
    pub order_ticket: u64,
    pub fill_price: f64,
    pub sl: f64,
    pub tp: f64,
    pub timestamp: String, // ISO
}

#[derive(Deserialize, ToSchema, IntoParams)]
pub struct HistoryParams {
    /// Optional start date for filtering logs (format: YYYY-MM-DD)
    pub start_date: Option<String>,
    /// Optional end date for filtering logs (format: YYYY-MM-DD)
    pub end_date: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct HistoryStats {
    pub total_profit: f64,
    pub total_trades: usize,
    pub winning_trades: usize,
    pub losing_trades: usize,
    pub win_rate_percent: f64,
    pub profit_factor: f64,
}

#[derive(Serialize, ToSchema)]
pub struct HistoryResponse {
    #[serde(flatten)]
    pub stats: HistoryStats,
    pub trades: Vec<TradeLog>,
}

impl Default for HistoryResponse {
    fn default() -> Self {
        Self {
            stats: HistoryStats {
                total_profit: 0.0, total_trades: 0, winning_trades: 0, losing_trades: 0, win_rate_percent: 0.0, profit_factor: 0.0
            },
            trades: vec![]
        }
    }
}


pub fn ema(values: &Vec<f64>, period: usize) -> Vec<f64> {
    let mut out = vec![0.0; values.len()];
    if values.is_empty() || period == 0 {
        return out;
    }
    let k = 2.0 / (period as f64 + 1.0);
    let seed: f64 = if values.len() >= period {
        values.iter().take(period).sum::<f64>() / period as f64
    } else {
        values.iter().sum::<f64>() / values.len() as f64
    };
    let start = period.saturating_sub(1);
    out[start] = seed;
    for i in (start + 1)..values.len() {
        let prev = out[i - 1];
        out[i] = (values[i] - prev) * k + prev;
    }
    for i in 0..start {
        out[i] = out[start];
    }
    out
}

pub fn rsi(values: &Vec<f64>, period: usize) -> Vec<f64> {
    let mut out = vec![50.0; values.len()];
    if values.len() <= period {
        return out;
    }
    let mut gain = 0.0;
    let mut loss = 0.0;
    for i in 1..=period {
        let d = values[i] - values[i - 1];
        if d >= 0.0 {
            gain += d;
        } else {
            loss -= d;
        }
    }
    let mut avg_g = gain / period as f64;
    let mut avg_l = loss / period as f64;
    out[period] = if avg_l == 0.0 { 100.0 } else {
        let rs = avg_g / avg_l; 100.0 - (100.0 / (1.0 + rs))
    };
    for i in (period + 1)..values.len() {
        let d = values[i] - values[i - 1];
        let g = if d > 0.0 { d } else { 0.0 };
        let l = if d < 0.0 { -d } else { 0.0 };
        avg_g = (avg_g * (period as f64 - 1.0) + g) / period as f64;
        avg_l = (avg_l * (period as f64 - 1.0) + l) / period as f64;
        out[i] = if avg_l == 0.0 { 100.0 } else {
            let rs = avg_g / avg_l; 100.0 - (100.0 / (1.0 + rs))
        };
    }
    out
}

pub fn atr(highs: &Vec<f64>, lows: &Vec<f64>, closes: &Vec<f64>, period: usize) -> Vec<f64> {
    if highs.len() < period || period == 0 {
        return vec![0.0; highs.len()];
    }
    let mut trs = vec![0.0; highs.len()];
    for i in 1..highs.len() {
        let tr1 = highs[i] - lows[i];
        let tr2 = (highs[i] - closes[i-1]).abs();
        let tr3 = (lows[i] - closes[i-1]).abs();
        trs[i] = tr1.max(tr2).max(tr3);
    }

    let mut atrs = vec![0.0; highs.len()];
    let first_atr: f64 = trs.iter().skip(1).take(period).sum::<f64>() / period as f64;
    atrs[period] = first_atr;
    for i in (period + 1)..highs.len() {
        atrs[i] = (atrs[i-1] * (period - 1) as f64 + trs[i]) / period as f64;
    }
    atrs
}

pub fn sma(values: &Vec<f64>, period: usize) -> Vec<f64> {
    let mut out = vec![0.0; values.len()];
    if values.is_empty() || period == 0 || values.len() < period {
        return out;
    }

    // Calculate the initial sum for the first window
    let mut sum: f64 = values.iter().take(period).sum();
    out[period - 1] = sum / period as f64;

    // Use a rolling sum for the rest of the values for efficiency
    for i in period..values.len() {
        sum += values[i] - values[i - period];
        out[i] = sum / period as f64;
    }

    // Pad the beginning of the output with the first calculated SMA value
    // This provides a consistent, non-zero value for early candles.
    let first_val = out[period - 1];
    for i in 0..(period - 1) {
        out[i] = first_val;
    }

    out
}

pub fn stochastic(highs: &Vec<f64>, lows: &Vec<f64>, closes: &Vec<f64>, k_period: usize, d_period: usize, slowing_period: usize) -> (Vec<f64>, Vec<f64>) {
    let n = closes.len();
    if n == 0 || k_period == 0 || d_period == 0 || slowing_period == 0 || n < k_period {
        return (vec![0.0; n], vec![0.0; n]);
    }

    let mut raw_k_values = vec![0.0; n];
    for i in (k_period - 1)..n {
        let period_highs = &highs[(i - k_period + 1)..=i];
        let period_lows = &lows[(i - k_period + 1)..=i];

        let highest_high = period_highs.iter().fold(f64::NEG_INFINITY, |a, &b| a.max(b));
        let lowest_low = period_lows.iter().fold(f64::INFINITY, |a, &b| a.min(b));

        if (highest_high - lowest_low).abs() < f64::EPSILON { // Avoid division by zero
            raw_k_values[i] = 50.0; // Neutral value
        } else {
            raw_k_values[i] = ((closes[i] - lowest_low) / (highest_high - lowest_low)) * 100.0;
        }
    }

    // Smooth raw_k_values to get %K (using slowing_period)
    let k_values = sma(&raw_k_values, slowing_period);

    // Smooth %K to get %D (using d_period)
    let d_values = sma(&k_values, d_period);

    (k_values, d_values)
}

// --- Early Entry Helpers ---

/// Simple Rate-of-Change: (close_now - close_n_periods_ago) / close_n_periods_ago * 100
pub fn roc(closes: &Vec<f64>, period: usize) -> Vec<f64> {
    let n = closes.len();
    let mut out = vec![0.0; n];
    if period == 0 || n < period { return out; }
    for i in period..n {
        let prev = closes[i - period];
        out[i] = if prev.abs() > 0.0 { (closes[i] - prev) / prev * 100.0 } else { 0.0 };
    }
    out
}

/// EMA slope (difference between last two EMA points)
pub fn ema_slope(ema_vals: &Vec<f64>) -> f64 {
    if ema_vals.len() < 2 { return 0.0; }
    ema_vals[ema_vals.len() - 1] - ema_vals[ema_vals.len() - 2]
}

/// Simple wick/imbalance check for a candle
pub fn wick_imbalance(high: f64, low: f64, open: f64, close: f64) -> (bool, bool) {
    let body = (close - open).abs();
    let upper_wick = high - open.max(close);
    let lower_wick = open.min(close) - low;
    let range = high - low;
    if range <= 0.0 { return (false, false); }
    let body_ratio = body / range;

    // Refined Imbalance Logic: Body must be significant, and the rejection wick must be small relative to the *entire range*.
    // TODO: Add volume confirmation: `&& candle_volume >= 0.8 * median_volume`
    let bullish_imbalance = body_ratio > 0.62 && upper_wick < range * 0.3 && (close > open);
    let bearish_imbalance = body_ratio > 0.62 && lower_wick < range * 0.3 && (close < open);

    (bullish_imbalance, bearish_imbalance)
}

// --- Microstructure Analysis Functions ---

/// Finds significant swing points (highs/lows) in a given lookback period.
/// A simple swing high is a high with `n` lower highs on both sides.
pub fn find_swing_points(highs: &[f64], lows: &[f64], lookback: usize, n: usize) -> (Vec<(usize, f64)>, Vec<(usize, f64)>) {
    let mut swing_highs = Vec::new();
    let mut swing_lows = Vec::new();
    let start_index = highs.len().saturating_sub(lookback);

    for i in (start_index + n)..(highs.len() - n) {
        let is_swing_high = (1..=n).all(|j| highs[i] > highs[i - j] && highs[i] > highs[i + j]);
        if is_swing_high {
            swing_highs.push((i, highs[i]));
        }

        let is_swing_low = (1..=n).all(|j| lows[i] < lows[i - j] && lows[i] < lows[i + j]);
        if is_swing_low {
            swing_lows.push((i, lows[i]));
        }
    }
    (swing_highs, swing_lows)
}

/// Detects Fair Value Gaps (Imbalances).
/// A bullish FVG is when the low of candle `i` is higher than the high of candle `i-2`.
/// The FVG is the space between `high[i-2]` and `low[i]`.
pub fn find_imbalance_zones(highs: &[f64], lows: &[f64], lookback: usize) -> Vec<PriceLevel> {
    let mut zones = Vec::new();
    let start_index = highs.len().saturating_sub(lookback);

    for i in (start_index + 2)..highs.len() {
        // Correct Bullish FVG: The low of the current candle is above the high of the candle two periods ago.
        if lows[i] > highs[i-2] {
             zones.push(PriceLevel { top: lows[i], bottom: highs[i-2] });
        }
        // Correct Bearish FVG: The high of the current candle is below the low of the candle two periods ago.
        if highs[i] < lows[i-2] {
            zones.push(PriceLevel { top: lows[i-2], bottom: highs[i] });
        }
    }
    zones
}

/// Calculates Volume-Weighted Average Price (VWAP) and standard deviation bands.
pub fn vwap(closes: &[f64], highs: &[f64], lows: &[f64], volumes: &[u64], period: usize) -> Vec<VwapBands> {
    let n = closes.len();
    let mut vwap_bands = vec![];
    if n < period { return vwap_bands; }

    for i in period..=n {
        let start = i - period;
        let typical_price_vol: f64 = (start..i).map(|j| ((highs[j] + lows[j] + closes[j]) / 3.0) * volumes[j] as f64).sum();
        let total_volume: u64 = volumes[start..i].iter().sum();
        let vwap = if total_volume > 0 { typical_price_vol / total_volume as f64 } else { closes[i-1] };

        let variance: f64 = (start..i).map(|j| volumes[j] as f64 * (closes[j] - vwap).powi(2)).sum();
        let std_dev = if total_volume > 0 { (variance / total_volume as f64).sqrt() } else { 0.0 };

        vwap_bands.push(VwapBands { vwap, upper_band1: vwap + std_dev, lower_band1: vwap - std_dev, upper_band2: vwap + 2.0 * std_dev, lower_band2: vwap - 2.0 * std_dev });
    }
    vwap_bands
}

/// Volatility pulse: latest ATR > factor * median ATR over lookback
pub fn atr_pulse(atr_vals: &Vec<f64>, lookback: usize, factor: f64) -> bool {
    let n = atr_vals.len();
    if n < lookback + 1 { return false; }

    let slice = &atr_vals[n - lookback..n];
    let mut sorted = slice.to_vec();
    // Filter out zeros which can skew the median if they appear early in the series
    sorted.retain(|&v| v > 0.0);
    if sorted.is_empty() { return false; }

    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = sorted[sorted.len() / 2];

    atr_vals[n - 1] > median * factor
}

// --- New Swing Functions ---

/// ADX Implementation (compact)
pub fn adx(highs: &Vec<f64>, lows: &Vec<f64>, closes: &Vec<f64>, period: usize) -> Vec<f64> {
    let n = closes.len();
    let mut out = vec![0.0; n];
    if n <= period || period == 0 { return out; }

    // Compute TR, +DM, -DM
    let mut tr = vec![0.0; n];
    let mut plus_dm = vec![0.0; n];
    let mut minus_dm = vec![0.0; n];
    for i in 1..n {
        tr[i] = (highs[i] - lows[i]).max((highs[i] - closes[i-1]).abs()).max((lows[i] - closes[i-1]).abs());
        let up_move = highs[i] - highs[i-1];
        let down_move = lows[i-1] - lows[i];
        plus_dm[i] = if up_move > down_move && up_move > 0.0 { up_move } else { 0.0 };
        minus_dm[i] = if down_move > up_move && down_move > 0.0 { down_move } else { 0.0 };
    }

    // Smooth TR, +DM, -DM using Wilder's smoothing
    let mut atr_s = 0.0;
    let mut pdm_s = 0.0;
    let mut mdm_s = 0.0;
    // Initialize
    for i in 1..=period {
        atr_s += tr[i];
        pdm_s += plus_dm[i];
        mdm_s += minus_dm[i];
    }
    atr_s /= period as f64;
    pdm_s /= period as f64;
    mdm_s /= period as f64;
    // From index = period+1 onward use smoothing
    let mut di_plus = vec![0.0; n];
    let mut di_minus = vec![0.0; n];
    for i in (period + 1)..n {
        atr_s = (atr_s * (period as f64 - 1.0) + tr[i]) / period as f64;
        pdm_s = (pdm_s * (period as f64 - 1.0) + plus_dm[i]) / period as f64;
        mdm_s = (mdm_s * (period as f64 - 1.0) + minus_dm[i]) / period as f64;
        di_plus[i] = if atr_s > 0.0 { (pdm_s / atr_s) * 100.0 } else { 0.0 };
        di_minus[i] = if atr_s > 0.0 { (mdm_s / atr_s) * 100.0 } else { 0.0 };
        let denom = di_plus[i] + di_minus[i];
        let dx = if denom.abs() < f64::EPSILON { 0.0 }
        else {
            ((di_plus[i] - di_minus[i]).abs() / denom) * 100.0
        };
        // Now smooth DX to get ADX
        if i == period * 2 { // Initialize ADX
            out[i] = di_plus[i..=i].iter().sum::<f64>() / period as f64;
        } else if i > period * 2 {
            out[i] = (out[i-1] * (period as f64 - 1.0) + dx) / period as f64;
        }
    }
    // Pad beginning values
    let first_valid_adx = out.iter().find(|&&v| v > 0.0).cloned().unwrap_or(25.0);
    for i in 0..out.len() {
        if out[i] == 0.0 {
            out[i] = first_valid_adx;
        }
    }
    out
}

/// Chandelier Exit (long)
pub fn chandelier_exit_high(highs: &Vec<f64>, atr_vals: &Vec<f64>, lookback: usize, atr_mult: f64) -> Option<f64> {
    let n = highs.len();
    if n < lookback || atr_vals.len() < 1 { return None; }
    let hi = highs[n - lookback..n].iter().fold(f64::NEG_INFINITY, |a, &b| a.max(b));
    let last_atr = atr_vals.last().cloned().unwrap_or(0.0);
    Some(hi - atr_mult * last_atr)
}

pub fn chandelier_exit_low(lows: &Vec<f64>, atr_vals: &Vec<f64>, lookback: usize, atr_mult: f64) -> Option<f64> {
    let n = lows.len();
    if n < lookback || atr_vals.len() < 1 { return None; }
    let lo = lows[n - lookback..n].iter().fold(f64::INFINITY, |a, &b| a.min(b));
    let last_atr = atr_vals.last().cloned().unwrap_or(0.0);
    Some(lo + atr_mult * last_atr)
}

/// Trend detector (HTF EMA alignment + ADX)
pub fn is_strong_uptrend(mtf_ema_short: f64, mtf_ema_long: f64, adx_val: f64, adx_threshold: f64) -> bool {
    mtf_ema_short > mtf_ema_long && adx_val >= adx_threshold
}
pub fn is_strong_downtrend(mtf_ema_short: f64, mtf_ema_long: f64, adx_val: f64, adx_threshold: f64) -> bool {
    mtf_ema_short < mtf_ema_long && adx_val >= adx_threshold
}

/// Manage open positions (pseudo/proper)
pub fn manage_open_positions(
    open_positions: &Vec<OpenPosition>,
    highs: &Vec<f64>,
    lows: &Vec<f64>,
    closes: &Vec<f64>,
    atr_vals: &Vec<f64>,
    adx_vals: &Vec<f64>,
    params: &EvalRequest // use your params for chandelier, thresholds
) -> Vec<(u64, String /*action: hold|close|update*/, Option<f64> /*new_sl*/)> {
    let mut actions = vec![];
    for pos in open_positions.iter() {
        let n = closes.len();
        // compute chandelier stop depending on direction
        let chandelier_period = params.chandelier_period.unwrap_or(22);
        let chandelier_mult = params.chandelier_atr_mult.unwrap_or(3.0);
        if pos.direction == "buy" {
            // Check for TP hit first
            if highs[n-1] >= pos.tp {
                actions.push((pos.ticket, "close".to_string(), None));
                continue;
            }
            if let Some(ch_stop) = chandelier_exit_high(&highs, &atr_vals, chandelier_period, chandelier_mult) {
                // if price hits stop or ADX collapses -> close
                let adx_now = adx_vals.last().cloned().unwrap_or(0.0);
                if lows[n-1] <= ch_stop || adx_now < params.adx_threshold.unwrap_or(25.0) {
                    actions.push((pos.ticket, "close".to_string(), None));
                } else {
                    // update SL to higher of current SL and ch_stop
                    if ch_stop > pos.sl {
                        actions.push((pos.ticket, "update_sl".to_string(), Some(ch_stop)));
                    } else {
                        actions.push((pos.ticket, "hold".to_string(), None));
                    }
                }
            } else {
                actions.push((pos.ticket, "hold".to_string(), None));
            }
        } else { // sell
            // Check for TP hit first
            if lows[n-1] <= pos.tp {
                actions.push((pos.ticket, "close".to_string(), None));
                continue;
            }
            if let Some(ch_stop) = chandelier_exit_low(&lows, &atr_vals, chandelier_period, chandelier_mult) {
                let adx_now = adx_vals.last().cloned().unwrap_or(0.0);
                if highs[n-1] >= ch_stop || adx_now < params.adx_threshold.unwrap_or(25.0) {
                    actions.push((pos.ticket, "close".to_string(), None));
                } else {
                    if ch_stop < pos.sl {
                        actions.push((pos.ticket, "update_sl".to_string(), Some(ch_stop)));
                    } else {
                        actions.push((pos.ticket, "hold".to_string(), None));
                    }
                }
            } else {
                actions.push((pos.ticket, "hold".to_string(), None));
            }
        }
    }
    actions
}
