use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
pub struct EvalRequest {
    pub symbol: String,
    pub timeframe: String,
    pub closes: Vec<f64>,
    pub highs: Vec<f64>, // Needed for ATR
    pub lows: Vec<f64>,  // Needed for ATR
    pub m5_closes: Vec<f64>, // New: M5 data for trend confirmation
    pub m5_highs: Vec<f64>,
    pub m5_lows: Vec<f64>,
    pub rsi_period: Option<usize>,
    pub ema_fast: Option<usize>,
    pub ema_slow: Option<usize>,
    pub atr_period: Option<usize>, // ATR period
    pub sma_period: Option<usize>, // New: SMA period for trend filtering
    pub stoch_k_period: Option<usize>, // New: Stochastic K period
    pub stoch_d_period: Option<usize>, // New: Stochastic D period
    pub stoch_slowing: Option<usize>,  // New: Stochastic slowing period
    pub tp_pips: Option<f64>,
    pub sl_pips: Option<f64>,
    pub sl_atr_multiplier: Option<f64>,
    pub tp_atr_multiplier: Option<f64>,
}

#[derive(Serialize)]
pub struct EvalResponse {
    pub action_advice: String,
    pub tp_pips: f64,
    pub sl_pips: f64,
    pub reason: String,
    pub rsi: f64,
    pub ema_fast_last: f64,
    pub ema_slow_last: f64,
    pub atr: f64,
    pub sma_last: f64,
    pub stoch_k_last: f64, // New: Return the last Stochastic %K value
    pub stoch_d_last: f64, // New: Return the last Stochastic %D value
    pub conviction_score: u8, // New: Signal conviction score
}

#[derive(Serialize, Deserialize, Clone, Debug)]
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