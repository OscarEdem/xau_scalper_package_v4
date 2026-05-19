use std::error::Error;
use super::{lstm, generic_onnx};
use std::path::Path;

/// A common trait for all prediction models.
/// This allows different models (FeatureMLP, RegimeMLP, LSTM) to be used interchangeably.
pub trait Predictor: Send + Sync {
    /// Predicts the price for a number of future periods.
    fn predict(&self, data: &[f64], future_periods: f64) -> Result<f64, Box<dyn Error>>;
    /// Returns the confidence of the model, if available.
    fn confidence(&self) -> Option<f64>;
}

/// Factory function to load a specific predictor model.
///
/// | model_type    | File loaded                           | Replaces |
/// |---------------|---------------------------------------|----------|
/// | feature_mlp   | `feature_mlp_{tf}.onnx` + scaler     | GBM      |
/// | regime_mlp    | `regime_mlp_{tf}.onnx`  + scaler     | Heston   |
/// | lstm          | `lstm_{tf}.onnx` + scaler            | (kept)   |
pub fn load_predictor(model_type: &str, timeframe: &str, models_dir: &str) -> Result<Box<dyn Predictor>, Box<dyn Error>> {
    let base_path = Path::new(models_dir);
    match model_type {
        "feature_mlp" => {
            // FeatureMLP: replaces GBM.
            // Maps 5 technical features → next-bar log-return prediction.
            // Fully deterministic — no stochastic noise.
            let path = base_path.join(format!("feature_mlp_{}.onnx", timeframe));
            let model = generic_onnx::predict::GenericOnnxPredictor::load(
                path.to_str().ok_or("Invalid path")?,
                0.70, // confidence: higher than old GBM's 0.50, lower than LSTM's 0.85
            )?;
            Ok(Box::new(model))
        }
        "regime_mlp" => {
            // RegimeMLP: replaces Heston.
            // Classifies market regime (trending / sideways) as a continuous bias ∈ (-1,1).
            // Fully deterministic — no stochastic variance process.
            let path = base_path.join(format!("regime_mlp_{}.onnx", timeframe));
            let model = generic_onnx::predict::GenericOnnxPredictor::load(
                path.to_str().ok_or("Invalid path")?,
                0.60, // confidence: regime signals are lower-resolution than direct return forecasts
            )?;
            Ok(Box::new(model))
        }
        "lstm" => {
            // LSTM: sequence model — unchanged.
            let path = base_path.join(format!("lstm_{}.onnx", timeframe));
            let lstm_model = lstm::predict::LSTM::load(path.to_str().ok_or("Invalid path")?)?;
            Ok(Box::new(lstm_model))
        }
        _ => Err(format!("Unknown predictor model type: {}", model_type).into()),
    }
}

/// A no-op predictor used as a safe fallback when a real model cannot be loaded.
#[derive(Default)]
pub struct NoopPredictor;

impl Predictor for NoopPredictor {
    fn predict(&self, _data: &[f64], _future_periods: f64) -> Result<f64, Box<dyn Error>> {
        Err("No predictor available".into())
    }

    fn confidence(&self) -> Option<f64> {
        None
    }
}