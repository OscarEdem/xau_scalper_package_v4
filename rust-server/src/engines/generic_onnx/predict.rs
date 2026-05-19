//! Generic ONNX predictor for feature-based MLP models.
//!
//! This module is the Rust runtime counterpart of the Python training scripts
//! `train_gbm.py` (FeatureMLP) and `train_heston.py` (RegimeMLP).
//!
//! Both models share the same 5-feature input vector, defined identically
//! in Python (`preprocessing.py`) and Rust (this file) to guarantee
//! zero training/inference drift:
//!
//!   | # | Feature      | Description                                |
//!   |---|--------------|---------------------------------------------|
//!   | 0 | roc_3        | 3-bar rate-of-change                       |
//!   | 1 | roc_10       | 10-bar rate-of-change                      |
//!   | 2 | atr_ratio    | ATR(14) / close  (normalised volatility)   |
//!   | 3 | rsi_14       | Wilder RSI(14) mapped to [-1, 1]           |
//!   | 4 | dist_ema20   | (close - EMA(20)) / ATR(14)  (in ATR units)|
//!
//! The model output ∈ (-1, 1) (Tanh activation) is converted back to a
//! "predicted price" so it slots seamlessly into the existing `Predictor`
//! trait and `ensemble_predictor::calculate_bias` arithmetic.

use crate::engines::predictor::Predictor;
use ndarray::Array;
use ort::{
    inputs,
    session::{builder::GraphOptimizationLevel, Session},
    value::Value,
};
use serde::Deserialize;
use std::error::Error;
use std::fs::File;
use std::io::BufReader;
use std::sync::Mutex;

// ---------------------------------------------------------------------------
// Scaler — mirrors the dict saved by preprocessing.compute_features()
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct FeatureScaler {
    mean:       Vec<f64>,
    std:        Vec<f64>,
    n_features: usize,
    /// Only present in FeatureMLP scaler (train_gbm.py).  Ignored for Regime.
    #[serde(default = "default_y_scale")]
    y_scale:    f64,
}

fn default_y_scale() -> f64 { 1.0 }

// ---------------------------------------------------------------------------
// GenericOnnxPredictor
// ---------------------------------------------------------------------------

/// Loads any ONNX model whose input is `[batch, N_FEATURES]` float32 and
/// whose output is `[batch, 1]` float32.
pub struct GenericOnnxPredictor {
    session:    Mutex<Session>,
    scaler:     FeatureScaler,
    /// Fixed confidence reported to the ensemble weighting logic.
    confidence: f64,
}

impl GenericOnnxPredictor {
    pub fn load(model_path: &str, confidence: f64) -> Result<Self, Box<dyn Error>> {
        let session = Session::builder()?
            .with_intra_threads(1)?
            .with_inter_threads(1)?
            .with_optimization_level(GraphOptimizationLevel::Level3)?
            .commit_from_file(model_path)?;

        // Scaler lives next to the .onnx file with suffix `_scaler.json`
        let scaler_path = model_path.replace(".onnx", "_scaler.json");
        let file   = File::open(&scaler_path)
            .map_err(|e| format!("Scaler not found at {}: {}", scaler_path, e))?;
        let scaler: FeatureScaler = serde_json::from_reader(BufReader::new(file))?;

        Ok(Self { session: Mutex::new(session), scaler, confidence })
    }
}

// ---------------------------------------------------------------------------
// Feature computation (mirrors preprocessing.py exactly)
// ---------------------------------------------------------------------------

/// Wilder's EMA helper (alpha = 1/period, seed with first value).
fn ema(values: &[f64], period: usize) -> Vec<f64> {
    if values.is_empty() { return vec![]; }
    let alpha = 1.0 / period as f64;
    let mut out = vec![0.0; values.len()];
    out[0] = values[0];
    for i in 1..values.len() {
        out[i] = alpha * values[i] + (1.0 - alpha) * out[i - 1];
    }
    out
}

/// ATR(14) using Wilder smoothing.
fn atr14(closes: &[f64]) -> Vec<f64> {
    let n = closes.len();
    if n < 2 { return vec![0.0; n]; }

    // True range
    let tr: Vec<f64> = (1..n)
        .map(|i| (closes[i] - closes[i - 1]).abs())  // simplified: we only have closes
        .collect();

    // Seed with simple mean of first 14 TRs, then Wilder smooth
    if tr.len() < 14 {
        return vec![tr.iter().sum::<f64>() / tr.len() as f64; n];
    }
    let alpha = 1.0 / 14.0;
    let seed  = tr[..14].iter().sum::<f64>() / 14.0;
    let mut atr = vec![0.0; n];
    atr[14] = seed;
    for i in 15..n {
        atr[i] = alpha * tr[i - 1] + (1.0 - alpha) * atr[i - 1];
    }
    // Back-fill the leading zeros with the first real value
    let first = atr[14];
    for v in &mut atr[..14] { *v = first; }
    atr
}

/// Wilder RSI(14) mapped to [-1, 1].
fn rsi14(closes: &[f64]) -> Vec<f64> {
    let n = closes.len();
    if n < 15 { return vec![0.0; n]; }

    let alpha = 1.0 / 14.0;
    let mut avg_gain = 0.0_f64;
    let mut avg_loss = 0.0_f64;

    // Seed with first 14 differences
    for i in 1..=14 {
        let d = closes[i] - closes[i - 1];
        if d > 0.0 { avg_gain += d; } else { avg_loss += d.abs(); }
    }
    avg_gain /= 14.0;
    avg_loss /= 14.0;

    let mut rsi = vec![0.0; n];
    let raw = if avg_loss == 0.0 { 100.0 } else { 100.0 - 100.0 / (1.0 + avg_gain / avg_loss) };
    rsi[14] = (raw / 50.0) - 1.0;

    for i in 15..n {
        let d = closes[i] - closes[i - 1];
        let g = if d > 0.0 { d } else { 0.0 };
        let l = if d < 0.0 { d.abs() } else { 0.0 };
        avg_gain = alpha * g + (1.0 - alpha) * avg_gain;
        avg_loss = alpha * l + (1.0 - alpha) * avg_loss;
        let raw = if avg_loss == 0.0 { 100.0 } else { 100.0 - 100.0 / (1.0 + avg_gain / avg_loss) };
        rsi[i] = (raw / 50.0) - 1.0;
    }
    // Back-fill
    let first = rsi[14];
    for v in &mut rsi[..14] { *v = first; }
    rsi
}

/// Builds the 5-feature vector for the **last bar** in `closes`.
/// Returns `None` if there is insufficient history (need ≥ 21 bars).
fn compute_last_feature(closes: &[f64]) -> Option<[f32; 5]> {
    let n = closes.len();
    if n < 21 { return None; }

    let last  = closes[n - 1];
    let c3    = closes[n - 4];   // 3 bars ago
    let c10   = closes[n - 11];  // 10 bars ago

    let roc3  = if c3  != 0.0 { (last - c3)  / c3  } else { 0.0 };
    let roc10 = if c10 != 0.0 { (last - c10) / c10 } else { 0.0 };

    let atr  = atr14(closes);
    let atr_last = atr[n - 1];
    let atr_ratio = if last != 0.0 { atr_last / last } else { 0.0 };

    let ema20_vec = ema(closes, 20);
    let ema20_last = ema20_vec[n - 1];
    let dist_ema20 = if atr_last != 0.0 { (last - ema20_last) / atr_last } else { 0.0 };

    let rsi_vec  = rsi14(closes);
    let rsi_last = rsi_vec[n - 1];

    Some([roc3 as f32, roc10 as f32, atr_ratio as f32, rsi_last as f32, dist_ema20 as f32])
}

// ---------------------------------------------------------------------------
// Predictor trait impl
// ---------------------------------------------------------------------------

impl Predictor for GenericOnnxPredictor {
    fn predict(&self, closes: &[f64], _future_periods: f64) -> Result<f64, Box<dyn Error>> {
        let current_price = *closes.last().unwrap_or(&0.0);
        if current_price == 0.0 { return Ok(0.0); }

        let raw_features = compute_last_feature(closes)
            .ok_or("Insufficient price history for feature computation (need ≥ 21 bars)")?;

        // Z-score normalisation using the training scaler
        let n = self.scaler.n_features.min(raw_features.len());
        let normalised: Vec<f32> = (0..n)
            .map(|i| {
                let std = if self.scaler.std[i] != 0.0 { self.scaler.std[i] } else { 1.0 };
                ((raw_features[i] as f64 - self.scaler.mean[i]) / std) as f32
            })
            .collect();

        // Build ONNX input tensor [1, N_FEATURES]
        let input_array = Array::from_shape_vec([1, n], normalised)
            .map_err(|e| format!("Feature tensor error: {}", e))?;
        let input_tensor = Value::from_array(input_array)?;
        let inputs = inputs!["input" => input_tensor];

        let mut session_guard = self.session.lock()
            .map_err(|_| "Failed to lock GenericOnnx session")?;
        let outputs = session_guard.run(inputs)
            .map_err(|e| format!("ONNX inference failed: {}", e))?;

        let out_tuple = outputs["output"].try_extract_tensor::<f32>()
            .map_err(|e| format!("Failed to extract output tensor: {}", e))?;
        let raw_output = *out_tuple.1.first()
            .ok_or("Model returned empty prediction")? as f64;

        // Convert the model's normalised output back to a "predicted price".
        // raw_output ∈ (-1, 1)  (Tanh).
        // We scale it by y_scale (std of training targets) and add current_price
        // so that ensemble_predictor's (pred_price - current_price) arithmetic
        // correctly reflects the directional bias.
        let predicted_return = raw_output * self.scaler.y_scale;
        Ok(current_price * (1.0 + predicted_return))
    }

    fn confidence(&self) -> Option<f64> {
        Some(self.confidence)
    }
}
