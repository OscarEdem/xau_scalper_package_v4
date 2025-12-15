use std::collections::HashMap;
use super::scoring::MacroScore;
use super::types::Bias;

pub fn resolve_pair_bias(
    base: &str,
    quote: &str,
    scores: &HashMap<String, MacroScore>,
) -> (Bias, Option<String>, u8) {
    let default_score = MacroScore::default();
    let base_score = scores.get(base).unwrap_or(&default_score);
    let quote_score = scores.get(quote).unwrap_or(&default_score);

    let base_strength = base_score.hawkish - base_score.dovish;
    let quote_strength = quote_score.hawkish - quote_score.dovish;

    let diff = base_strength - quote_strength;

    let (bias, stronger) = match diff {
        d if d >= 4 => (Bias::StronglyBullish, Some(base.to_string())),
        d if d >= 2 => (Bias::SlightlyBullish, Some(base.to_string())),
        d if d <= -4 => (Bias::StronglyBearish, Some(quote.to_string())),
        d if d <= -2 => (Bias::SlightlyBearish, Some(quote.to_string())),
        _ => (Bias::Neutral, None),
    };

    // Confidence Calculation: (abs(diff) * 10) - (conflicting_signals * 5)
    let base_conflict = std::cmp::min(base_score.hawkish, base_score.dovish);
    let quote_conflict = std::cmp::min(quote_score.hawkish, quote_score.dovish);
    
    let raw_confidence = (diff.abs() * 10) - ((base_conflict + quote_conflict) * 5);
    let confidence = raw_confidence.clamp(0, 100) as u8;

    (bias, stronger, confidence)
}