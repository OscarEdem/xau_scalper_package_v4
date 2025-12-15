use crate::indicators::NewsEvent;
use super::types::{MacroContext, TechnicalSignal};
use super::scoring::MacroScore;
use super::aggregate::aggregate_scores;
use super::pair_logic::resolve_pair_bias;
use super::xau_logic::resolve_xau_bias;

pub fn build_context(
    pair: &str,
    horizon: &str,
    events: &[NewsEvent],
    technical_levels: Vec<String>,
    swing_signals: Vec<TechnicalSignal>,
    htf_bias: String,
    fvg_zones: Vec<String>,
) -> MacroContext {
    // 1. Extract base/quote
    let base = if pair.len() >= 3 { &pair[0..3] } else { "" };
    let quote = if pair.len() >= 6 { &pair[3..6] } else { "" };

    // 2. Aggregate scores
    let scores = aggregate_scores(events);

    // 3. Calculate Global Risk-Off Score (Sum of all risk_off scores)
    let global_risk_off: i32 = scores.values().map(|s| s.risk_off).sum();

    // 4. Resolve Bias
    let (bias, stronger, confidence) = if pair == "XAUUSD" {
        resolve_xau_bias(&scores)
    } else {
        resolve_pair_bias(base, quote, &scores)
    };

    // 5. Get individual scores for context
    let default_score = MacroScore::default();
    let base_score = scores.get(base).unwrap_or(&default_score).clone();
    let quote_score = scores.get(quote).unwrap_or(&default_score).clone();

    MacroContext {
        pair: pair.to_string(),
        horizon: horizon.to_string(),
        base_currency: base.to_string(),
        quote_currency: quote.to_string(),
        base_score,
        quote_score,
        bias,
        stronger_currency: stronger,
        confidence,
        global_risk_off,
        high_impact_events: events.to_vec(),
        technical_levels,
        swing_signals,
        htf_bias,
        fvg_zones,
    }
}