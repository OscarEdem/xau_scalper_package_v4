use crate::engines::predictor::Predictor;
use rand_distr::{Distribution, Normal};
use serde::Deserialize;
use std::fs;

#[derive(Deserialize)]
pub struct GBMConfig {
    #[serde(alias = "drift")]
    pub mu: f64,
    #[serde(alias = "volatility")]
    pub sigma: f64,
}

pub struct GBM {
    config: GBMConfig,
}

impl GBM {
    pub fn load(path: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let data = fs::read_to_string(path)?;
        let config: GBMConfig = serde_json::from_str(&data)?;
        Ok(GBM { config })
    }
}

impl Predictor for GBM {
    fn predict(&self, history: &[f64], dt: f64) -> Result<f64, Box<dyn std::error::Error>> {
        let current_price = *history.last().unwrap_or(&0.0);
        if current_price == 0.0 { return Ok(0.0); }

        // Simple GBM step: S(t+1) = S(t) * exp((mu - 0.5*sigma^2)*dt + sigma*sqrt(dt)*Z)
        let normal = Normal::new(0.0, 1.0).unwrap();
        let z = normal.sample(&mut rand::thread_rng()); // Standard normal random variable
        Ok(current_price * ((self.config.mu - 0.5 * self.config.sigma.powi(2)) * dt
            + self.config.sigma * dt.sqrt() * z)
            .exp())
    }

    fn confidence(&self) -> Option<f64> {
        Some(0.5) // GBM is baseline, weak directional confidence
    }
}
