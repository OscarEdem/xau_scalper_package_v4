use crate::engines::{gbm::predict::GBM, heston::predict::Heston, lstm::predict::LSTM};
use tracing::warn;

pub trait Predictor {
    /// Predict the next price.
    /// `history`: A slice of recent close prices (e.g., last 50 candles).
    /// `dt`: Time step (e.g., 1.0/24.0 for hourly).
    fn predict(&self, history: &[f64], dt: f64) -> Result<f64, Box<dyn std::error::Error>>;

    /// Optional confidence score (0..1)
    fn confidence(&self) -> Option<f64> {
        None
    }
}

/// Factory function to dynamically load a predictor model based on a string identifier.
pub fn load_predictor(model_type: &str, timeframe: &str) -> Box<dyn Predictor> {
    let model_result: Result<Box<dyn Predictor>, _> = (|| {
        match model_type {
            "gbm" => {
                let path = format!("src/engines/gbm/models/gbm_{}_config.json", timeframe);
                Ok(Box::new(GBM::load(&path)?) as Box<dyn Predictor>)
            }
            "heston" => {
                let path = format!("src/engines/heston/models/heston_{}_config.json", timeframe);
                Ok(Box::new(Heston::load(&path)?) as Box<dyn Predictor>)
            }
            "lstm" => {
                let path = format!("src/engines/lstm/models/lstm_{}.onnx", timeframe);
                Ok(Box::new(LSTM::load(&path)?) as Box<dyn Predictor>)
            }
            _ => {
                warn!("Unknown model type '{}'. Defaulting to 'gbm' for timeframe '{}'.", model_type, timeframe);
                let path = format!("src/engines/gbm/models/gbm_{}_config.json", timeframe);
                Ok(Box::new(GBM::load(&path)?) as Box<dyn Predictor>)
            }
        }
    })();

    match model_result {
        Ok(predictor) => predictor,
        Err(e) => {
            warn!("Failed to load model '{}' for timeframe '{}': {}. Defaulting to GBM.", model_type, timeframe, e);
            let fallback_path = format!("src/engines/gbm/models/gbm_{}_config.json", timeframe);
            Box::new(GBM::load(&fallback_path).expect("Failed to load fallback GBM model"))
        },
    }
}
