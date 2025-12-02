pub mod indicators;
pub mod engines;
pub mod session;
pub use indicators::*;
pub use session::*;

/// A 1D Kalman Filter for price smoothing and estimation.
#[derive(Debug, Clone)]
pub struct KalmanFilter {
    pub gain: f64,
    pub estimate: f64,
    pub error_estimate: f64,
    pub error_measure: f64,
    pub q: f64, // Process noise
}

impl KalmanFilter {
    pub fn new(initial_price: f64, process_noise: f64, measurement_noise: f64) -> Self {
        Self {
            gain: 0.0,
            estimate: initial_price,
            error_estimate: 1.0,
            error_measure: measurement_noise,
            q: process_noise,
        }
    }

    pub fn update(&mut self, measurement: f64) -> f64 {
        // Prediction Update
        let prediction = self.estimate;
        self.error_estimate += self.q;

        // Measurement Update
        self.gain = self.error_estimate / (self.error_estimate + self.error_measure);
        self.estimate = prediction + self.gain * (measurement - prediction);
        self.error_estimate = (1.0 - self.gain) * self.error_estimate;

        self.estimate
    }
}

/// Runs a Kalman filter over a price series and returns the final estimate and its slope (velocity).
pub fn kalman_slope(prices: &[f64], window: usize, process_noise: f64, measurement_noise: f64) -> (f64, f64) {
    let n = prices.len();
    if n < window { return (0.0, 0.0); }

    // Noise parameters are now passed in.
    let mut kf = KalmanFilter::new(prices[n - window], process_noise, measurement_noise);

    let mut last_est = 0.0;
    let mut prev_est = 0.0;

    for &price in prices.iter().skip(n - window) {
        prev_est = last_est;
        last_est = kf.update(price);
    }

    (last_est, last_est - prev_est)
}

/// Detects liquidity traps where price sweeps a recent high/low but fails to close beyond it.
pub fn detect_inducement(highs: &[f64], lows: &[f64], closes: &[f64], lookback: usize) -> String {
    let n = closes.len();
    if n < lookback + 2 { return "none".to_string(); }

    let current_low = lows[n - 1];
    let current_high = highs[n - 1];
    let current_close = closes[n - 1];

    let &prev_low = lows[n - lookback..n - 1].iter().min_by(|a, b| a.partial_cmp(b).unwrap()).unwrap();
    let &prev_high = highs[n - lookback..n - 1].iter().max_by(|a, b| a.partial_cmp(b).unwrap()).unwrap();

    if current_low < prev_low && current_close > prev_low { "bullish_inducement".to_string() }
    else if current_high > prev_high && current_close < prev_high { "bearish_inducement".to_string() }
    else { "none".to_string() }
}
