use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::HashMap;
use utoipa::{IntoParams, ToSchema};

#[derive(Debug, Serialize, Deserialize, Clone, ToSchema, Hash, PartialEq, Eq)]
pub enum MacroCategory {
    MonetaryPolicy,
    Inflation,
    Labor,
    Growth,
    Risk,
    Other,
}

#[derive(Debug, Serialize, Deserialize, Clone, ToSchema, IntoParams, Hash, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NewsEvent {
    pub event: String,
    pub timestamp: i64,      // Unix seconds
    pub impact: String,      // "high", "medium", "low"
    pub country: String,
    pub currency: String,
    pub forecast: Option<String>,
    pub previous: Option<String>,
    pub actual: Option<String>,
    pub category: MacroCategory, // NEW
}


#[derive(Debug, Serialize, Deserialize, Clone, ToSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct EvalRequest<'a> {
    pub symbol: Cow<'a, str>,
    pub timeframe: Cow<'a, str>,
    #[schema(value_type = Vec<f64>)]
    pub closes: Cow<'a, [f64]>, 
    #[schema(value_type = Vec<f64>)]
    pub highs: Cow<'a, [f64]>,
    #[schema(value_type = Vec<f64>)]
    pub opens: Cow<'a, [f64]>,
    #[schema(value_type = Vec<f64>)]
    pub lows: Cow<'a, [f64]>,
    #[schema(value_type = Vec<u64>)]
    pub volumes: Cow<'a, [u64]>,
    #[schema(value_type = Vec<f64>)]
    pub m5_closes: Cow<'a, [f64]>,
    #[schema(value_type = Vec<f64>)]
    pub m5_highs: Cow<'a, [f64]>,
    #[schema(value_type = Vec<f64>)]
    pub m5_lows: Cow<'a, [f64]>,
    #[schema(value_type = Option<Vec<i64>>)]
    pub m5_timestamps: Option<Cow<'a, [i64]>>,
    #[schema(value_type = Option<Vec<f64>>)]
    pub m15_closes: Option<Cow<'a, [f64]>>,
    #[schema(value_type = Option<Vec<f64>>)]
    pub m15_highs: Option<Cow<'a, [f64]>>,
    #[schema(value_type = Option<Vec<f64>>)]
    pub m15_lows: Option<Cow<'a, [f64]>>,
    #[schema(value_type = Vec<f64>)]
    pub m30_closes: Cow<'a, [f64]>,
    #[schema(value_type = Option<Vec<f64>>)]
    pub h1_closes: Option<Cow<'a, [f64]>>,
    #[schema(value_type = Option<Vec<f64>>)]
    pub h1_highs: Option<Cow<'a, [f64]>>,
    #[schema(value_type = Option<Vec<f64>>)]
    pub h1_opens: Option<Cow<'a, [f64]>>,
    #[schema(value_type = Option<Vec<f64>>)]
    pub h1_lows: Option<Cow<'a, [f64]>>,
    // Add these Higher Time Frames
    #[schema(value_type = Option<Vec<f64>>)]
    pub h4_closes: Option<Cow<'a, [f64]>>,
    #[schema(value_type = Option<Vec<f64>>)]
    pub h4_highs: Option<Cow<'a, [f64]>>, 
    #[schema(value_type = Option<Vec<f64>>)]
    pub h4_lows: Option<Cow<'a, [f64]>>,  
    #[schema(value_type = Option<Vec<f64>>)]
    pub d1_opens: Option<Cow<'a, [f64]>>,
    #[schema(value_type = Option<Vec<f64>>)]
    pub d1_closes: Option<Cow<'a, [f64]>>,
    #[schema(value_type = Option<Vec<NewsEvent>>)]
    pub upcoming_events: Option<Cow<'a, [NewsEvent]>>,
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
    /// Process noise (q) for the Kalman Filter. Represents the uncertainty in the price model.
    #[schema(example = 0.01)]
    pub kf_process_noise: Option<f64>,
    /// Measurement noise (r) for the Kalman Filter. Represents the uncertainty in the price measurement.
    #[schema(example = 0.1)]
    pub kf_measurement_noise: Option<f64>,
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
    pub max_risk_pct: Option<f64>,
    // Timestamp synchronization
    pub last_m1_timestamp: i64,
    pub last_m5_timestamp: Option<i64>,
    pub last_m15_timestamp: Option<i64>,
    pub last_m30_timestamp: Option<i64>,
    pub last_h1_timestamp: Option<i64>,
    pub last_h4_timestamp: Option<i64>,
    pub last_d1_timestamp: Option<i64>,
    // New fields for engine mode
    pub mode: Cow<'a, str>, // "scalp" or "swing"
    pub current_price: f64,
    /// The predictor model to use (e.g., "gbm", "heston", "lstm").
    pub predictor_model: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PriceLevel {
    pub top: f64,
    pub bottom: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_bullish: Option<bool>,
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
    pub order_blocks: Vec<PriceLevel>,
    pub vwap_bands: Option<VwapBands>,
    pub reason: String,
    pub classification: String, // "scalp" or "swing"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scalp_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conviction_score: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggested_position_size: Option<f64>,
    // --- NEW: Fields for Limit Order Execution ---
    pub recommended_order_type: String, // "market", "limit_buy", "limit_sell"
    pub limit_order_price: f64,         // The exact price to place the limit
    pub expiration_seconds: Option<u64>, // Cancel limit if not filled in X seconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub debug_info: Option<HashMap<String, String>>,
    #[serde(skip)]
    pub should_push: bool,
    // --- NEW: Fields for pre-formatted push notifications ---
    /// The pre-formatted title for the push notification.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub push_title: Option<String>,
    /// The pre-formatted body for the push notification.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub push_body: Option<String>,
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
            order_blocks: vec![],
            vwap_bands: None,
            reason: "No signal".to_string(),
            classification: "none".to_string(),
            scalp_mode: None,
            conviction_score: None,
            suggested_position_size: None,
            recommended_order_type: "none".to_string(),
            limit_order_price: 0.0,
            expiration_seconds: None,
            debug_info: None,
            should_push: false,
            push_title: None,
            push_body: None,
        }
    }
}

pub fn ema(values: &[f64], period: usize) -> Vec<f64> {
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

pub fn rsi(values: &[f64], period: usize) -> Vec<f64> {
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

pub fn atr(highs: &[f64], lows: &[f64], closes: &[f64], period: usize) -> Vec<f64> {
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

pub fn sma(values: &[f64], period: usize) -> Vec<f64> {
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

pub fn stochastic(highs: &[f64], lows: &[f64], closes: &[f64], k_period: usize, d_period: usize, slowing_period: usize) -> (Vec<f64>, Vec<f64>) {
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
pub fn roc(closes: &[f64], period: usize) -> Vec<f64> {
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
pub fn ema_slope(ema_vals: &[f64]) -> f64 {
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

/// Finds liquidity zones based on swing points, adding a buffer to create a zone.
/// This prevents zones from being single price points (top == bottom).
pub fn find_liquidity_zones(
    highs: &[f64],
    lows: &[f64],
    lookback: usize,
    swing_n: usize,
    buffer_points: f64,
) -> Vec<PriceLevel> {
    let (swing_highs, swing_lows) = find_swing_points(highs, lows, lookback, swing_n);
    let mut zones = Vec::new();

    for (_, price) in swing_highs {
        zones.push(PriceLevel {
            top: price + buffer_points,
            bottom: price,
            is_bullish: Some(false), // Supply/Resistance
        });
    }

    for (_, price) in swing_lows {
        zones.push(PriceLevel {
            top: price,
            bottom: price - buffer_points,
            is_bullish: Some(true), // Demand/Support
        });
    }
    zones
}

/// Calculates dynamic zone thickness based on a specific candle's range and ATR.
pub fn calculate_dynamic_thickness(idx: usize, highs: &[f64], lows: &[f64], last_atr: f64) -> f64 {
    let range = if idx < highs.len() { highs[idx] - lows[idx] } else { last_atr };
    (range * 0.25).max(last_atr * 0.1)
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
            let top = lows[i];
            let bottom = highs[i-2];
            
            // Check for mitigation: has any subsequent candle traded below the bottom of the gap?
            // If price has traded completely through the gap, it is invalidated.
            let is_mitigated = lows[(i + 1)..].iter().any(|&l| l <= bottom);

            if !is_mitigated {
                zones.push(PriceLevel { top, bottom, is_bullish: Some(true) });
            }
        }
        // Correct Bearish FVG: The high of the current candle is below the low of the candle two periods ago.
        if highs[i] < lows[i-2] {
            let top = lows[i-2];
            let bottom = highs[i];

            // Check for mitigation: has any subsequent candle traded above the top of the gap?
            let is_mitigated = highs[(i + 1)..].iter().any(|&h| h >= top);

            if !is_mitigated {
                zones.push(PriceLevel { top, bottom, is_bullish: Some(false) });
            }
        }
    }
    zones
}

/// Calculates Volume-Weighted Average Price (VWAP) and standard deviation bands.
pub fn vwap(closes: &[f64], highs: &[f64], lows: &[f64], volumes: &[u64], period: usize) -> Vec<VwapBands> {
    let n = closes.len();
    let mut vwap_bands = vec![];
    if n < period { return vwap_bands; }

    // Rolling variables for O(N) complexity
    let mut sum_vol = 0.0;
    let mut sum_pv = 0.0;
    let mut sum_pv2 = 0.0;

    // Initialize first window
    for i in 0..period {
        let tp = (highs[i] + lows[i] + closes[i]) / 3.0;
        let v = volumes[i] as f64;
        sum_vol += v;
        sum_pv += v * tp;
        sum_pv2 += v * tp * tp;
    }

    // Helper closure to calculate bands from current sums
    let mut calc_bands = |s_vol: f64, s_pv: f64, s_pv2: f64, close: f64| {
        let vwap = if s_vol > 0.0 { s_pv / s_vol } else { close };
        // Variance = E[X^2] - (E[X])^2. Clamp to 0.0 to handle float precision issues.
        let variance = if s_vol > 0.0 { ((s_pv2 / s_vol) - vwap * vwap).max(0.0) } else { 0.0 };
        let std_dev = variance.sqrt();
        vwap_bands.push(VwapBands { vwap, upper_band1: vwap + std_dev, lower_band1: vwap - std_dev, upper_band2: vwap + 2.0 * std_dev, lower_band2: vwap - 2.0 * std_dev });
    };

    calc_bands(sum_vol, sum_pv, sum_pv2, closes[period - 1]);

    // Rolling update
    for i in period..n {
        let remove_idx = i - period;
        let tp_out = (highs[remove_idx] + lows[remove_idx] + closes[remove_idx]) / 3.0;
        let v_out = volumes[remove_idx] as f64;
        sum_vol -= v_out;
        sum_pv -= v_out * tp_out;
        sum_pv2 -= v_out * tp_out * tp_out;

        let tp_in = (highs[i] + lows[i] + closes[i]) / 3.0;
        let v_in = volumes[i] as f64;
        sum_vol += v_in;
        sum_pv += v_in * tp_in;
        sum_pv2 += v_in * tp_in * tp_in;

        calc_bands(sum_vol, sum_pv, sum_pv2, closes[i]);
    }
    vwap_bands
}

/// Volatility pulse: latest ATR > factor * median ATR over lookback
pub fn atr_pulse(atr_vals: &[f64], lookback: usize, factor: f64) -> bool {
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

/// Determines the trend bias based on an EMA.........................
/// Returns 1 for bullish, -1 for bearish, 0 for neutral/insufficient data.............
pub fn get_trend_bias(closes: &[f64], period: usize) -> i8 {
    let n = closes.len();
    if n < period + 2 {
        return 0; // Not enough data
    }
    let emas = ema(closes, period);
    let last = n - 1;

    // Logic: Price > EMA AND EMA is sloping up
    if closes[last] > emas[last] && emas[last] > emas[last - 1] {
        return 1;
    }
    // Logic: Price < EMA AND EMA is sloping down
    if closes[last] < emas[last] && emas[last] < emas[last - 1] {
        return -1;
    }

    0 // Choppy/Consolidation
}

/// Determines the daily bias based on the previous day's candle.
/// Returns "bullish", "bearish", or "neutral".
pub fn get_daily_bias(opens: &[f64], closes: &[f64]) -> String {
    if opens.is_empty() || closes.is_empty() {
        return "neutral".to_string();
    }
    let last = closes.len() - 1;
    // Assuming the last element is the current incomplete day, we look at last-1
    if last < 1 {
        return "neutral".to_string();
    }

    let prev_open = opens[last - 1];
    let prev_close = closes[last - 1];

    if prev_close > prev_open {
        "bullish".to_string()
    } else {
        "bearish".to_string()
    }
}

/// Detects regular bullish or bearish divergence on the RSI.
/// Returns "bullish", "bearish", or "none".
pub fn detect_rsi_divergence(lows: &[f64], highs: &[f64], closes: &[f64], rsi_period: usize, lookback: usize) -> String {
    let n = closes.len();
    if n < lookback || n < rsi_period {
        return "none".to_string();
    }

    let rsi_values = rsi(&closes.to_vec(), rsi_period);
    let curr_idx = n - 1;
    let curr_low = lows[curr_idx];
    let curr_high = highs[curr_idx];
    let curr_rsi = rsi_values[curr_idx];

    // Bullish Divergence Check
    let (prev_low_idx, prev_low_val) = lows[n - lookback..n - 1].iter().enumerate()
        .fold((0, f64::INFINITY), |(min_idx, min_val), (i, &val)| if val < min_val { (i, val) } else { (min_idx, min_val) });
    let global_prev_low_idx = (n - lookback) + prev_low_idx;
    let prev_low_rsi = rsi_values[global_prev_low_idx];
    if curr_low < prev_low_val && curr_rsi > prev_low_rsi && prev_low_rsi < 35.0 {
        return "bullish".to_string();
    }

    // Bearish Divergence Check
    let (prev_high_idx, prev_high_val) = highs[n - lookback..n - 1].iter().enumerate()
        .fold((0, f64::NEG_INFINITY), |(max_idx, max_val), (i, &val)| if val > max_val { (i, val) } else { (max_idx, max_val) });
    let global_prev_high_idx = (n - lookback) + prev_high_idx;
    let prev_high_rsi = rsi_values[global_prev_high_idx];
    if curr_high > prev_high_val && curr_rsi < prev_high_rsi && prev_high_rsi > 65.0 {
        return "bearish".to_string();
    }

    "none".to_string()
}

/// Finds the best limit entry price inside an FVG.
/// Strategy: 'aggressive' = Start of FVG, 'optimal' = 50% of FVG (Equilibrium)
pub fn get_fvg_limit_price(
    zones: &Vec<PriceLevel>,
    current_price: f64,
    direction: &str,
    strategy: &str, // "optimal" or "aggressive"
) -> Option<f64> {
    if zones.is_empty() {
        return None;
    }

    if direction == "long" {
        // Find closest FVG below current price
        if let Some(zone) = zones
            .iter()
            .filter(|z| z.top < current_price)
            .max_by(|a, b| a.top.partial_cmp(&b.top).unwrap())
        {
            return if strategy == "optimal" {
                Some(zone.top - (zone.top - zone.bottom) * 0.5) // 50% Retrace
            } else {
                Some(zone.top) // Aggressive: Enter at top of FVG
            };
        }
    } else {
        // Short
        // Find closest FVG above current price
        if let Some(zone) = zones
            .iter()
            .filter(|z| z.bottom > current_price)
            .min_by(|a, b| a.bottom.partial_cmp(&b.bottom).unwrap())
        {
            return if strategy == "optimal" {
                Some(zone.bottom + (zone.top - zone.bottom) * 0.5) // 50% Retrace
            } else {
                Some(zone.bottom) // Aggressive: Enter at bottom of FVG
            };
        }
    }
    None
}

/// Detects divergence using the current forming candle's data for zero-lag signals.
pub fn detect_realtime_divergence(
    lows: &[f64],
    highs: &[f64],
    rsi_vals: &[f64],
    lookback: usize,
) -> String {
    let n = lows.len();
    if n <= lookback { return "none".to_string(); }

    let current_low = lows[n - 1];
    let current_high = highs[n - 1];
    let current_rsi = rsi_vals[n - 1];

    let (prev_low_idx, &prev_low_val) = lows[n - lookback..n - 1].iter().enumerate().min_by(|a, b| a.1.partial_cmp(b.1).unwrap()).unwrap();
    let prev_low_rsi = rsi_vals[(n - lookback) + prev_low_idx];

    // Aggressive Bullish Check: Price is CURRENTLY breaking the low, but RSI is curling up/higher
    if current_low < prev_low_val && current_rsi > prev_low_rsi + 3.0 { // +3.0 buffer to avoid noise
        return "bullish_realtime".to_string();
    }

    let (prev_high_idx, &prev_high_val) = highs[n - lookback..n - 1].iter().enumerate().max_by(|a, b| a.1.partial_cmp(b.1).unwrap()).unwrap();
    let prev_high_rsi = rsi_vals[(n - lookback) + prev_high_idx];

    // Aggressive Bearish Check: Price is CURRENTLY breaking the high, but RSI is curling down/lower
    if current_high > prev_high_val && current_rsi < prev_high_rsi - 3.0 { // -3.0 buffer to avoid noise
        return "bearish_realtime".to_string();
    }

    "none".to_string()
}

// --- New Swing Functions ---

/// ADX Implementation (compact)
pub fn adx(highs: &[f64], lows: &[f64], closes: &[f64], period: usize) -> Vec<f64> {
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
pub fn chandelier_exit_high(highs: &[f64], atr_vals: &[f64], lookback: usize, atr_mult: f64) -> Option<f64> {
    let n = highs.len();
    if n < lookback || atr_vals.len() < 1 { return None; }
    let hi = highs[n - lookback..n].iter().fold(f64::NEG_INFINITY, |a, &b| a.max(b));
    let last_atr = atr_vals.last().cloned().unwrap_or(0.0);
    Some(hi - atr_mult * last_atr)
}

pub fn chandelier_exit_low(lows: &[f64], atr_vals: &[f64], lookback: usize, atr_mult: f64) -> Option<f64> {
    let n = lows.len();
    if n < lookback || atr_vals.len() < 1 { return None; }
    let lo = lows[n - lookback..n].iter().fold(f64::INFINITY, |a, &b| a.min(b));
    let last_atr = atr_vals.last().cloned().unwrap_or(0.0);
    Some(lo + atr_mult * last_atr)
}

pub struct RawOrderBlock {
    pub top: f64,
    pub bottom: f64,
    pub is_bullish: bool,
    pub creation_index: usize,
    pub bos_index: usize, // NEW: Track when the structure was actually broken
    pub mitigated: bool,
}

/// Scans price history to find unmitigated Order Blocks.
/// `lookback`: How far back to scan for swings (e.g., 100-200 candles).
pub fn find_order_blocks(
    highs: &[f64],
    lows: &[f64],
    _opens: &[f64], // Kept for signature compatibility if needed later
    closes: &[f64],
    lookback: usize, 
) -> Vec<PriceLevel> {
    let len = closes.len();
    if len < 5 { return Vec::new(); }
    
    let start_idx = len.saturating_sub(lookback);
    // Ensure we don't start at 0 because we access i-1
    let start_idx = if start_idx == 0 { 1 } else { start_idx };

    let mut ob_candidates: Vec<RawOrderBlock> = Vec::new();
    
    // 1. Identify Swings & Breaks
    for i in start_idx..(len - 2) {
        
        // --- BULLISH OB LOGIC ---
        let is_swing_low = lows[i] < lows[i-1] && lows[i] < lows[i+1];
        
        if is_swing_low {
            // STEP 1: Find the Structural High to break.
            // We scan backwards from i to find the highest point 
            // before this low was formed (up to a limit, e.g., 20 candles).
            let scan_limit = i.saturating_sub(20); 
            let mut structural_high = highs[i-1];
            
            for k in (scan_limit..i).rev() {
                if highs[k] > structural_high {
                    structural_high = highs[k];
                } else {
                    // Simple heuristic: If we find a high followed by a significantly lower high, 
                    // we might have found the swing point. 
                    // For now, finding the max in the last 10-20 candles is robust enough.
                }
            }
            
            // STEP 2: Look forward for BOS of that Structural High
            for j in (i + 1)..len {
                if closes[j] > structural_high {
                    // BOS Confirmed
                    let ob_top = highs[i]; 
                    let ob_bottom = lows[i];

                    if !ob_candidates.iter().any(|ob| ob.creation_index == i) {
                         ob_candidates.push(RawOrderBlock {
                            top: ob_top,
                            bottom: ob_bottom,
                            is_bullish: true,
                            creation_index: i,
                            bos_index: j,
                            mitigated: false,
                        });
                    }
                    break; 
                }
            }
        }

        // --- BEARISH OB LOGIC ---
        let is_swing_high = highs[i] > highs[i-1] && highs[i] > highs[i+1];
        
        if is_swing_high {
            // Find Structural Low to break (Scan backwards)
            let scan_limit = i.saturating_sub(20);
            let mut structural_low = lows[i-1];
            
            for k in (scan_limit..i).rev() {
                if lows[k] < structural_low {
                    structural_low = lows[k];
                }
            }

            for j in (i + 1)..len {
                if closes[j] < structural_low {
                    // BOS Confirmed
                    let ob_top = highs[i];
                    let ob_bottom = lows[i];

                     if !ob_candidates.iter().any(|ob| ob.creation_index == i) {
                        ob_candidates.push(RawOrderBlock {
                            top: ob_top,
                            bottom: ob_bottom,
                            is_bullish: false,
                            creation_index: i,
                            bos_index: j,
                            mitigated: false,
                        });
                    }
                    break;
                }
            }
        }
    }

    // 2. Check Mitigation (Has price returned to the block?)
    let mut active_zones: Vec<PriceLevel> = Vec::new();
    
    for ob in ob_candidates.iter_mut() {
        // Start checking from the candle AFTER the block was formed
        // FIX: Start checking AFTER the structure break.
        // Price action between creation(i) and BOS(j) is the "Impulse leg" and shouldn't mitigate the block.
        let check_start = ob.bos_index + 1; 
        
        let mut broken = false;
        let mut break_idx = 0;
        
        for k in check_start..len {
            if ob.is_bullish {
                // Invalidated: Price closes below bottom
                if closes[k] < ob.bottom {
                    ob.mitigated = true;
                    broken = true;
                    break_idx = k;
                    break;
                }
                // Mitigated: Price touches the zone (re-entry)
                if lows[k] <= ob.top {
                    ob.mitigated = true;
                    break;
                }
            } else { // Bearish
                // Invalidated
                if closes[k] > ob.top {
                    ob.mitigated = true;
                    broken = true;
                    break_idx = k;
                    break;
                }
                // Mitigated
                if highs[k] >= ob.bottom {
                    ob.mitigated = true;
                    break;
                }
            }
        }

        // 3. Output only valid, unmitigated zones
        if !ob.mitigated {
            active_zones.push(PriceLevel {
                top: ob.top,
                bottom: ob.bottom,
                is_bullish: Some(ob.is_bullish),
            });
        } else if broken {
            // Breaker Block Logic:
            // If an OB is broken, it flips polarity (Support <-> Resistance)
            // We check if the breaker itself is still valid (not reclaimed)
            let mut breaker_valid = true;
            
            for k in (break_idx + 1)..len {
                if ob.is_bullish {
                    // Was Bullish OB -> Now Bearish Breaker (Resistance)
                    // Invalidated if price closes back above the top
                    if closes[k] > ob.top {
                        breaker_valid = false;
                        break;
                    }
                } else {
                    // Was Bearish OB -> Now Bullish Breaker (Support)
                    // Invalidated if price closes back below the bottom
                    if closes[k] < ob.bottom {
                        breaker_valid = false;
                        break;
                    }
                }
            }

            if breaker_valid {
                active_zones.push(PriceLevel {
                    top: ob.top,
                    bottom: ob.bottom,
                    is_bullish: Some(!ob.is_bullish), // Flip polarity
                });
            }
        }
    }

    active_zones
}
