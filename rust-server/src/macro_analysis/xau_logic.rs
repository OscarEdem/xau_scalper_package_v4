use std::collections::HashMap;
use super::scoring::MacroScore;
use super::types::Bias;

pub struct XauMacroInputs {
    pub usd_score: MacroScore,
    pub global_risk_off: i32,
}

pub fn resolve_xauusd_bias(
    inputs: &XauMacroInputs,
) -> (Bias, i32) {
    let usd_policy_strength = inputs.usd_score.hawkish - inputs.usd_score.dovish;

    let mut strength = 0;

    // USD policy dominates direction
    if usd_policy_strength >= 3 {
        strength -= 3; // bearish XAU
    } else if usd_policy_strength <= -3 {
        strength += 3; // bullish XAU
    }

    // Risk-off amplifies XAU
    if inputs.global_risk_off >= 3 {
        strength += 2;
    }

    let bias = match strength {
        s if s >= 4 => Bias::StronglyBullish,
        s if s >= 2 => Bias::SlightlyBullish,
        s if s <= -4 => Bias::StronglyBearish,
        s if s <= -2 => Bias::SlightlyBearish,
        _ => Bias::Neutral,
    };

    (bias, strength)
}

pub fn xau_conflict_score(inputs: &XauMacroInputs) -> i32 {
    let usd_policy = inputs.usd_score.hawkish - inputs.usd_score.dovish;

    let mut conflicts = 0;

    // Hawkish USD + risk-off
    if usd_policy >= 3 && inputs.global_risk_off >= 3 {
        conflicts += 1;
    }

    conflicts
}

pub fn resolve_xau_bias(
    scores: &HashMap<String, MacroScore>,
) -> (Bias, Option<String>, u8) {
    let default_score = MacroScore::default();
    let usd_score = scores.get("USD").unwrap_or(&default_score).clone();

    // Global Risk Off (Good for Gold)
    // Summing risk_off from all currencies as a proxy for global sentiment
    let global_risk_off: i32 = scores.values().map(|s| s.risk_off).sum();

    let inputs = XauMacroInputs {
        usd_score,
        global_risk_off,
    };

    let (bias, strength) = resolve_xauusd_bias(&inputs);
    let conflicts = xau_conflict_score(&inputs);

    let stronger = match bias {
        Bias::StronglyBullish | Bias::SlightlyBullish => Some("XAU".to_string()),
        Bias::StronglyBearish | Bias::SlightlyBearish => Some("USD".to_string()),
        Bias::Neutral => None,
    };

    // Confidence Calculation
    // Strength ranges roughly from -3 to +5.
    // Max strength (5) * 18 = 90.
    // Conflict penalty = 25.
    let raw_confidence = (strength.abs() * 18) - (conflicts * 25);
    let confidence = raw_confidence.clamp(0, 100) as u8;

    (bias, stronger, confidence)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_xauusd_bias_hawkish_usd_risk_on() {
        // Scenario: Hawkish USD (Bad for Gold) + Risk On (Bad for Gold)
        // Expect: Strongly Bearish
        let inputs = XauMacroInputs {
            usd_score: MacroScore { hawkish: 5, dovish: 1, risk_off: 0 },
            global_risk_off: 0,
        };
        let (bias, strength) = resolve_xauusd_bias(&inputs);
        assert_eq!(bias, Bias::StronglyBearish);
        assert!(strength <= -4);
    }

    #[test]
    fn test_resolve_xauusd_bias_dovish_usd_risk_off() {
        // Scenario: Dovish USD (Good for Gold) + Risk Off (Good for Gold)
        // Expect: Strongly Bullish
        let inputs = XauMacroInputs {
            usd_score: MacroScore { hawkish: 1, dovish: 5, risk_off: 0 },
            global_risk_off: 5,
        };
        let (bias, strength) = resolve_xauusd_bias(&inputs);
        assert_eq!(bias, Bias::StronglyBullish);
        assert!(strength >= 4);
    }

    #[test]
    fn test_xau_conflict_detection() {
        // Scenario: Hawkish USD (Bearish Gold) BUT Risk Off (Bullish Gold)
        // Expect: Conflict detected
        let inputs = XauMacroInputs {
            usd_score: MacroScore { hawkish: 5, dovish: 0, risk_off: 0 },
            global_risk_off: 5,
        };
        
        let conflicts = xau_conflict_score(&inputs);
        assert_eq!(conflicts, 1);

        // Bias might be neutral or slightly one way depending on exact math, 
        // but confidence should be hit by the conflict.
        let (bias, _) = resolve_xauusd_bias(&inputs);
        // Hawkish USD (-3) + Risk Off (+2) = -1 (Neutral/Slightly Bearish)
        assert!(matches!(bias, Bias::Neutral | Bias::SlightlyBearish));
    }
}