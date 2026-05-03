use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use crate::indicators::NewsEvent;
use super::scoring::MacroScore;

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
pub enum Bias {
    StronglyBullish,
    SlightlyBullish,
    Neutral,
    SlightlyBearish,
    StronglyBearish,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct MacroOutlook {
    pub pair: String,
    pub horizon: String,
    pub bias: Bias,
    pub stronger_currency: Option<String>,
    pub high_impact_drivers: Vec<String>,
    pub macro_narrative: String,
    pub technical_signals: Vec<String>,
    pub risks: Vec<String>,
    pub confidence: u8, // 0–100
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TechnicalSignal {
    pub signal_type: String,
    pub confidence: f64,
    pub price_level: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MacroContext {
    pub pair: String,
    pub horizon: String,
    pub base_currency: String,
    pub quote_currency: String,
    pub base_score: MacroScore,
    pub quote_score: MacroScore,
    pub bias: Bias,
    pub stronger_currency: Option<String>,
    pub confidence: u8,
    pub global_risk_off: i32,
    pub high_impact_events: Vec<NewsEvent>,
    pub technical_levels: Vec<String>,
    pub swing_signals: Vec<TechnicalSignal>,
    pub htf_bias: String,
    pub fvg_zones: Vec<String>,
    pub news_headlines: Vec<String>,
}