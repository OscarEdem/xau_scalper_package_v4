use crate::engines::predictor::Predictor;
use rand_distr::{Distribution, Normal};
use serde::Deserialize;
use std::fs;

#[derive(Deserialize)]
pub struct HestonConfig {
    pub kappa: f64, // Mean reversion speed
    pub theta: f64, // Long term variance
    pub sigma: f64, // Volatility of volatility
    pub rho: f64,   // Correlation between asset and volatility
    pub v0: f64,    // Initial variance
}

pub struct Heston {
    config: HestonConfig,
}

impl Heston {
    pub fn load(path: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let data = fs::read_to_string(path)?;
        let config: HestonConfig = serde_json::from_str(&data)?;
        Ok(Heston { config })
    }
}

impl Predictor for Heston {
    fn predict(&self, history: &[f64], dt: f64) -> Result<f64, Box<dyn std::error::Error>> {
        let current_price = *history.last().unwrap_or(&0.0);
        if current_price == 0.0 {
            return Ok(0.0);
        }

        let mut rng = rand::thread_rng();
        let normal = Normal::new(0.0, 1.0).unwrap();

        // Generate two correlated random variables (Z1, Z2)
        let z1 = normal.sample(&mut rng);
        let z_uncorrelated = normal.sample(&mut rng);
        let z2 = self.config.rho * z1 + (1.0 - self.config.rho.powi(2)).sqrt() * z_uncorrelated;

        // 1. Update Variance (v_t)
        // dv = kappa(theta - v)dt + sigma*sqrt(v)*dW2
        // We use max(0.0) to prevent negative variance
        let v_prev = self.config.v0;
        let dv = self.config.kappa * (self.config.theta - v_prev) * dt + self.config.sigma * v_prev.sqrt() * dt.sqrt() * z2;
        let v_new = (v_prev + dv).max(0.00001);

        // 2. Update Price (S_t)
        // dS = sqrt(v)*S*dW1 (assuming 0 drift for simple simulation, or add mu)
        let ds = v_new.sqrt() * current_price * dt.sqrt() * z1;

        Ok(current_price + ds)
    }

    fn confidence(&self) -> Option<f64> {
        Some(0.65)
    }
}
