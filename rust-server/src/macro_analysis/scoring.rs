use crate::indicators::{NewsEvent, MacroCategory};
use serde::{Serialize, Deserialize};

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct MacroScore {
    pub hawkish: i32,
    pub dovish: i32,
    pub risk_off: i32,
}

pub fn score_event(event: &NewsEvent) -> MacroScore {
    let mut score = MacroScore::default();

    match event.category {
        MacroCategory::MonetaryPolicy => {
            score.hawkish += 2;
        }
        MacroCategory::Inflation => {
            score.hawkish += 1;
        }
        MacroCategory::Labor => {
            score.dovish += 1;
        }
        MacroCategory::Risk => {
            score.risk_off += 1;
        }
        _ => {}
    }

    // Impact multiplier
    if event.impact.to_lowercase() == "high" {
        score.hawkish *= 2;
        score.dovish *= 2;
        score.risk_off *= 2;
    }

    score
}