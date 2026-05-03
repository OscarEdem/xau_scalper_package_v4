use std::error::Error;
use super::{gbm, heston}; // lstm disabled due to linker issues
use std::path::Path;

/// A common trait for all prediction models.
/// This allows different models (GBM, Heston, LSTM) to be used interchangeably.
pub trait Predictor: Send + Sync {
    /// Predicts the price for a number of future periods.
    fn predict(&self, data: &[f64], future_periods: f64) -> Result<f64, Box<dyn Error>>;
    /// Returns the confidence of the model, if available.
    fn confidence(&self) -> Option<f64>;
}

/// Factory function to load a specific predictor model.
pub fn load_predictor(model_type: &str, timeframe: &str, models_dir: &str) -> Result<Box<dyn Predictor>, Box<dyn Error>> {
    let base_path = Path::new(models_dir);
    match model_type {
        "gbm" => {
            // GBM models are stored as JSON configs named like `gbm_h1_config.json` in `/app/models`.
            let path = base_path.join(format!("gbm_{}_config.json", timeframe));
            let gbm = gbm::predict::GBM::load(path.to_str().ok_or("Invalid path")?)?;
            Ok(Box::new(gbm))
        }
       "heston" => {
            // Heston models are stored as JSON configs named like `heston_h1_config.json` in `/app/models`.
            let path = base_path.join(format!("heston_{}_config.json", timeframe));
            let heston = heston::predict::Heston::load(path.to_str().ok_or("Invalid path")?)?;
            Ok(Box::new(heston))
        }
        "lstm" => {
            // LSTM models are disabled due to ort-sys linker issues on Windows.
            // Using NoopPredictor as fallback.
            Ok(Box::new(NoopPredictor::default()))
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