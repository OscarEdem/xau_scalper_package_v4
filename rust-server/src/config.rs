use config::{Config, ConfigError, File, Environment};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Settings {
    pub server: ServerSettings,
    pub trading: TradingSettings,
    pub paths: PathSettings,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ServerSettings {
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Deserialize, Serialize, Clone, ToSchema)]
pub struct TradingSettings {
    pub max_buffer_size: usize,
    #[serde(default = "default_sync_threshold")]
    pub sync_threshold: usize,
    #[serde(default)]
    pub scalp: ScalpSettings,
    #[serde(default)]
    pub swing: SwingSettings,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct PathSettings {
    pub push_tokens_file: String,
    pub models_dir: String,
    pub sessions_dir: String,
}

fn default_sync_threshold() -> usize {
    100
}

impl Default for TradingSettings {
    fn default() -> Self {
        Self {
            max_buffer_size: 500,
            sync_threshold: 100,
            scalp: Default::default(),
            swing: Default::default(),
        }
    }
}

impl TradingSettings {
    pub fn validate(&self) -> Result<(), String> {
        if self.max_buffer_size < 100 {
            return Err(format!("max_buffer_size must be at least 100, got {}", self.max_buffer_size));
        }
        self.scalp.validate()?;
        self.swing.validate()?;
        Ok(())
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, ToSchema)]
pub struct ScalpSettings {
    pub min_data_len: usize,
    pub atr_period: usize,
    pub kalman_period: usize,
    pub base_kalman_threshold: f64,
    pub base_m1_surge_threshold: f64,
    pub min_conviction: f64,
    pub max_limit_dist_atr_mult: f64,
    pub htf_bias_weight: f64,
    pub ensemble_weight: f64,
    pub flow_surge_confluence_boost: f64,
    pub inducement_weight: f64,
    pub risk_reward_ratio_tp1: f64,
    pub risk_reward_ratio_tp2: f64,
    pub sl_atr_multiplier_inducement: f64,
    pub sl_atr_multiplier_flow: f64,
    pub logistic_scale: f64,
    pub logistic_offset: f64,
    pub limit_order_expiration: u64,
    pub time_stop_seconds: u32,
    pub news_pre_event_block_minutes: i64,
    pub news_post_event_block_minutes: i64,
    pub news_guard_atr_spike_multiplier: f64,
}

impl Default for ScalpSettings {
    fn default() -> Self {
        Self {
            min_data_len: 60,
            atr_period: 14,
            kalman_period: 20,
            base_kalman_threshold: 0.12,
            base_m1_surge_threshold: 0.9,
            min_conviction: 50.0,
            max_limit_dist_atr_mult: 0.6,
            htf_bias_weight: 0.35,
            ensemble_weight: 0.25,
            flow_surge_confluence_boost: 0.15,
            inducement_weight: 0.6,
            risk_reward_ratio_tp1: 1.25,
            risk_reward_ratio_tp2: 2.5,
            sl_atr_multiplier_inducement: 1.0,
            sl_atr_multiplier_flow: 2.0,
            logistic_scale: 6.0,
            logistic_offset: 0.6,
            limit_order_expiration: 180,
            time_stop_seconds: 3600,
            news_pre_event_block_minutes: 30,
            news_post_event_block_minutes: 15,
            news_guard_atr_spike_multiplier: 2.5,
        }
    }
}

impl ScalpSettings {
    pub fn validate(&self) -> Result<(), String> {
        if self.min_conviction < 0.0 || self.min_conviction > 100.0 {
            return Err(format!("Scalp min_conviction must be between 0.0 and 100.0, got {}", self.min_conviction));
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, ToSchema)]
pub struct SwingSettings {
    pub min_data_len: usize,
    pub atr_period: usize,
    pub conviction_threshold: f64,
    pub htf_bias_weight: f64,
    pub sfp_weight_mult: f64,
    pub displacement_weight_mult: f64,
    pub fvg_weight: f64,
    pub ensemble_weight_mult: f64,
    pub risk_reward_ratio_tp1: f64,
    pub risk_reward_ratio_tp2: f64,
    pub sl_atr_buffer: f64,
    pub limit_order_expiration: u64,
    pub time_stop_seconds: u32,
    pub reentry_bounce_threshold: f64,
    pub reentry_invalidation_threshold: f64,
    pub reentry_expiration_seconds: i64,
    pub reentry_sl_pct: f64,
    pub reentry_tp1_pct: f64,
    pub reentry_tp2_pct: f64,
    pub htf_bias_daily_weight: f64,
    pub htf_bias_h4_weight: f64,
    pub news_pre_event_block_minutes: i64,
    pub news_post_event_block_minutes: i64,
    pub news_guard_atr_spike_multiplier: f64,
}

impl Default for SwingSettings {
    fn default() -> Self {
        Self {
            min_data_len: 60,
            atr_period: 14,
            conviction_threshold: 50.0,
            htf_bias_weight: 30.0,
            sfp_weight_mult: 0.5,
            displacement_weight_mult: 0.4,
            fvg_weight: 20.0,
            ensemble_weight_mult: 15.0,
            risk_reward_ratio_tp1: 2.0,
            risk_reward_ratio_tp2: 4.0,
            sl_atr_buffer: 0.25,
            limit_order_expiration: 3600,
            time_stop_seconds: 86400,
            reentry_bounce_threshold: 0.0005,
            reentry_invalidation_threshold: 0.0010,
            reentry_expiration_seconds: 14400,
            reentry_sl_pct: 0.001,
            reentry_tp1_pct: 0.005,
            reentry_tp2_pct: 0.01,
            htf_bias_daily_weight: 0.6,
            htf_bias_h4_weight: 0.4,
            news_pre_event_block_minutes: 60,
            news_post_event_block_minutes: 30,
            news_guard_atr_spike_multiplier: 2.0,
        }
    }
}

impl SwingSettings {
    pub fn validate(&self) -> Result<(), String> {
        if self.conviction_threshold < 0.0 || self.conviction_threshold > 100.0 {
            return Err(format!("Swing conviction_threshold must be between 0.0 and 100.0, got {}", self.conviction_threshold));
        }
        Ok(())
    }
}

impl Settings {
    pub fn new() -> Result<Self, ConfigError> {
        let s = Config::builder()
            .set_default("server.host", "0.0.0.0")?
            .set_default("server.port", 3000)?
            .set_default("trading.max_buffer_size", 500)?
            .set_default("paths.push_tokens_file", "push_tokens.json")?
            .set_default("paths.models_dir", "./models/")?
            .set_default("paths.sessions_dir", "./sessions_data")?
            // Look for config.toml in the current directory
            .add_source(File::with_name("config").required(false))
            // Allow override with env vars (e.g. APP_SERVER__PORT=3000)
            .add_source(Environment::with_prefix("APP").separator("__"))
            .build()?;

        s.try_deserialize()
    }
}