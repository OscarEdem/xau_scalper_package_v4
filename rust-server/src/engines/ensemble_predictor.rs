use super::predictor_cache::PredictorCache;
use tracing::debug;

/// Calculates a weighted prediction bias from an ensemble of models.
///
/// This function queries multiple predictor models (GBM, Heston, LSTM),
/// weights their predicted price changes by their confidence scores, and
/// returns a single, normalized bias value.
///
/// # Arguments
/// * `predictor_cache` - A cache to load/retrieve predictor models.
/// * `timeframe` - The timeframe for the models (e.g., "m5", "h1").
/// * `closes` - The series of closing prices to use for prediction.
/// * `current_price` - The current price, used as a baseline for predicted change.
/// * `future_periods` - The number of future periods to predict (model-specific).
///
///
/// # Returns
/// A `f64` representing the final weighted prediction bias. A positive value
/// indicates a bullish bias, and a negative value indicates a bearish bias.
pub fn calculate_bias(
    predictor_cache: &PredictorCache,
    timeframe: &str,
    closes: &[f64],
    current_price: f64,
    future_periods: f64,
) -> f64 {
    let model_types = ["gbm", "heston", "lstm"];

    let mut total_confidence = 0.0;
    let mut weighted_prediction_sum = 0.0;
    let mut individual_predictions = Vec::new();

    for &model_type in &model_types {
        let p = predictor_cache.get_or_load(model_type, timeframe);
        
        // Use a safe prediction wrapper to avoid panics on corrupt data
        let pred_price = p.predict(closes, future_periods).unwrap_or(current_price);
        let mut confidence = p.confidence().unwrap_or(0.01); // Min confidence floor

        // Custom Weighting: Boost LSTM influence relative to GBM/Heston
        if model_type == "lstm" {
            confidence *= 2.0; 
        }

        let predicted_change = pred_price - current_price;
        weighted_prediction_sum += predicted_change * confidence;
        total_confidence += confidence;
        individual_predictions.push(format!("{}:{:.4}({:.2})", model_type, predicted_change, confidence));
    }

    debug!(model_biases = %individual_predictions.join(", "), "Ensemble prediction calculated");
    if total_confidence > 0.0 { weighted_prediction_sum / total_confidence } else { 0.0 }
}