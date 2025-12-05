use crate::engines::predictor::Predictor;
use ndarray::Array;
use ort::{
    inputs,
    session::{builder::GraphOptimizationLevel, Session},
    value::Value, // <--- REQUIRED IMPORT
    Result as OrtResult,
};
use serde::Deserialize;
use std::fs::File;
use std::io::BufReader;

#[derive(Deserialize)]
struct ScalerParams {
    mean: f64,
    std: f64,
}

pub struct LSTM {
    session: Session,
    scaler: ScalerParams,
}

impl LSTM {
    pub fn load(model_path: &str) -> OrtResult<Self> {
        let session = Session::builder()?
            .with_optimization_level(GraphOptimizationLevel::Level3)?
            .commit_from_file(model_path)?;

        let scaler_path = model_path.replace(".onnx", "_scaler.json");
        let reader = BufReader::new(File::open(&scaler_path).expect("Scaler config not found"));
        let scaler: ScalerParams = serde_json::from_reader(reader).expect("Failed to parse scaler");

        Ok(LSTM { session, scaler })
    }
}

impl Predictor for LSTM {
    fn predict(&self, history: &[f64], _dt: f64) -> f64 { // Changed return type to f64 to match Trait
        const SEQ_LEN: usize = 50;

        if history.len() < SEQ_LEN {
            return history.last().cloned().unwrap_or(0.0);
        }

        // 1. Prepare Data
        let recent_data = &history[history.len() - SEQ_LEN..];
        
        // 2. Normalize
        let normalized: Vec<f32> = recent_data
            .iter()
            .map(|&x| ((x - self.scaler.mean) / self.scaler.std) as f32)
            .collect();

        // 3. Create ndarray
        let input_array = Array::from_shape_vec((1, SEQ_LEN, 1), normalized)
            .expect("Failed to create input array");

        // 4. Create ORT Tensor (FIX A: Explicit Value creation)
        let input_tensor = Value::from_array(input_array.view())
            .expect("Failed to create ORT Tensor");

        // 5. Run Inference (FIX B: Correct macro usage)
        // inputs! returns a Vec, so we do NOT use '?' here.
        let inputs = inputs!["input" => input_tensor]; 
        let outputs = self.session.run(inputs).expect("Inference failed");

        // 6. Extract Output (FIX C: Robust Iterator extraction)
        // We use try_extract_tensor to get an ArrayView, then .iter().next()
        // This avoids the 'to_slice' / 'non-primitive cast' issues entirely.
        let output_view = outputs["output"]
            .try_extract_tensor::<f32>()
            .expect("Failed to extract tensor");
        
        let raw_output = *output_view.iter().next().unwrap_or(&0.0) as f64;

        // 7. Denormalize
        let last_price = recent_data.last().unwrap();
        last_price * raw_output.exp()
    }

    fn confidence(&self) -> Option<f64> {
        Some(0.85) 
    }
}