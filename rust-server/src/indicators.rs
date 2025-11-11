use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
pub struct EvalRequest {
    pub symbol: String,
    pub timeframe: String,
    pub closes: Vec<f64>,
    pub highs: Vec<f64>, // Needed for ATR
    pub lows: Vec<f64>,  // Needed for ATR
    pub rsi_period: Option<usize>,
    pub ema_fast: Option<usize>,
    pub ema_slow: Option<usize>,
    pub atr_period: Option<usize>, // ATR period
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