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
use std::sync::Mutex; // <--- FIX 1: Import Mutex

#[derive(Deserialize)]
struct ScalerParams {
    mean: f64,
    std: f64,
}

pub struct LSTM {
    // <--- FIX 2: Wrap Session in Mutex
    // This allows us to modify the session (run inference) 
    // even when we only hold an immutable reference (&self) to LSTM.
    session: Mutex<Session>, 
    scaler: ScalerParams,
}

impl LSTM {
    pub fn load(model_path: &str) -> Result<Self, Box<dyn Error>> {
        let session = Session::builder()?
            .with_optimization_level(GraphOptimizationLevel::Level3)?
            .commit_from_file(model_path)?;

        let scaler_path = model_path.replace(".onnx", "_scaler.json");
        let file = File::open(&scaler_path)
            .map_err(|e| format!("Scaler config not found at {}: {}", scaler_path, e))?;
        let scaler: ScalerParams = serde_json::from_reader(BufReader::new(file))?;

        Ok(LSTM { 
            session: Mutex::new(session), // <--- FIX 3: Initialize the Mutex
            scaler 
        })
    }
}

impl Predictor for LSTM {
    fn predict(&self, history: &[f64], _future_periods: f64) -> Result<f64, Box<dyn Error>> {
        const SEQ_LEN: usize = 50;

        if history.len() < SEQ_LEN {
            return Ok(history.last().cloned().unwrap_or(0.0));
        }

        let recent_data = &history[history.len() - SEQ_LEN..];
        
        let normalized: Vec<f32> = recent_data
            .iter()
            .map(|&x| ((x - self.scaler.mean) / self.scaler.std) as f32)
            .collect();

        // Shape: [Batch Size = 1, Sequence Length = 50, Features = 1]
        let input_array = Array::from_shape_vec([1, SEQ_LEN, 1], normalized)
            .map_err(|e| format!("Failed to create input array: {}", e))?;

        // Pass 'input_array' directly (Ownership transfer)
        let input_tensor = Value::from_array(input_array)?;

        // inputs! returns a Vec, so we DO NOT use '?'
        let inputs = inputs!["input" => input_tensor]; 
        
        // --- FIX 4: Lock the Mutex ---
        // We lock the session to get exclusive, mutable access to it.
        // This satisfies the requirement for session.run(&mut self, ...)
        let mut session_guard = self.session.lock().map_err(|_| "Failed to lock LSTM session")?;
        
        let outputs = session_guard.run(inputs)
            .map_err(|e| format!("ONNX Inference failed: {}", e))?;

        let output_tuple = outputs["output"].try_extract_tensor::<f32>()
            .map_err(|e| format!("Failed to extract output tensor: {}", e))?;
        
        let output_slice = output_tuple.1;
        
        let raw_output = *output_slice.first().ok_or("Model returned empty prediction")? as f64;

        let last_price = *recent_data.last().unwrap_or(&0.0);
        let predicted_price = last_price * raw_output.exp();

        Ok(predicted_price)
    }

    fn confidence(&self) -> Option<f64> {
        Some(0.85) 
    }
}