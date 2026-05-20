use config::{Config, ConfigError, File, Environment};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Settings {
    pub server: ServerSettings,
    pub trading: TradingSettings,
    pub paths: PathSettings,
    #[serde(default)]
    pub database: DatabaseSettings,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ServerSettings {
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct DatabaseSettings {
    pub url: String,
}

impl Default for DatabaseSettings {
    fn default() -> Self {
        Self {
            url: "".to_string(),
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, ToSchema)]
pub struct RiskSettings {
    #[serde(default = "default_account_equity")]
    pub account_equity: f64,
    #[serde(default = "default_risk_per_trade_pct")]
    pub risk_per_trade_pct: f64,
    #[serde(default = "default_xauusd_lot_point_value")]
    pub xauusd_lot_point_value: f64, // Value of a 1.0 price move for a 1.0 lot size trade. For XAUUSD, this is typically $100.
}

fn default_account_equity() -> f64 { 100000.0 }
fn default_risk_per_trade_pct() -> f64 { 0.01 } // 1%
fn default_xauusd_lot_point_value() -> f64 { 100.0 }

impl Default for RiskSettings {
    fn default() -> Self {
        Self {
            account_equity: default_account_equity(),
            risk_per_trade_pct: default_risk_per_trade_pct(),
            xauusd_lot_point_value: default_xauusd_lot_point_value(),
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, ToSchema)]
pub struct TradingSettings {
    pub max_buffer_size: usize,
    #[serde(default = "default_sync_threshold")]
    pub sync_threshold: usize,
    #[serde(default)]
    pub risk: RiskSettings,
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
            risk: Default::default(),
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
        if self.risk.risk_per_trade_pct <= 0.0 || self.risk.risk_per_trade_pct > 0.1 {
             return Err(format!("risk_per_trade_pct should be between 0.0 and 0.1 (10%), got {}", self.risk.risk_per_trade_pct));
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
    // Mode Classification Thresholds
    pub momentum_kalman_threshold: f64,
    pub momentum_m1_threshold: f64,

    // Fade Strategy
    pub fade_sma_period: usize,
    pub fade_std_dev_mult: f64,
    pub fade_parabolic_atr_mult: f64,
    pub fade_sl_atr_mult: f64,
    pub fade_tp1_atr_mult: f64,
    pub fade_max_adx: f64,

    // Momentum Strategy
    pub momentum_risk_atr_mult: f64,
    pub momentum_tp1_atr_mult: f64,
    pub momentum_tp2_atr_mult: f64,

    // Pullback Strategy
    pub pullback_sl_atr_mult: f64,
    pub pullback_tp1_atr_mult: f64,
    pub pullback_tp2_atr_mult: f64,

    // Safety & Scoring
    pub min_sl_atr_mult: f64,
    pub max_sl_atr_mult: f64,
    pub kalman_weight: f64,
    pub m1_surge_weight: f64,

    // Session Toggles
    #[serde(default)]
    pub allow_asia_trading: bool,
    #[serde(default)]
    pub allow_london_open_momentum: bool,
    #[serde(default)]
    pub allow_ny_late_momentum: bool,
    pub m1_roc_period: usize,
    pub m1_atr_conversion_div: f64,
    pub vol_regime_clamp_min: f64,
    pub vol_regime_clamp_max: f64,
    pub flow_threshold_mult: f64,
    pub inducement_opposing_reduction: f64,
    pub momentum_min_risk_atr: f64,
    #[serde(default)]
    pub filter_scalp_by_swing: bool,
    #[serde(default = "default_push_notifications_enabled")]
    pub push_notifications_enabled: bool,
    #[serde(default = "default_push_notification_threshold")]
    pub push_notification_threshold: f64,
    // (swing evaluation knobs moved to `SwingSettings`)
    #[serde(default = "default_pullback_entry_displacement_atr")]
    pub pullback_entry_displacement_atr: f64,
    #[serde(default = "default_momentum_require_ml_confluence")]
    pub momentum_require_ml_confluence: bool,

    // Daily Circuit Breaker
    /// Maximum number of signals the scalp engine may emit per UTC day.
    /// When hit, the engine goes silent until the next UTC midnight.
    #[serde(default = "default_max_scalp_signals_per_day")]
    pub max_signals_per_day: usize,
    /// Master switch for the daily circuit breaker.
    #[serde(default = "default_circuit_breaker_enabled")]
    pub circuit_breaker_enabled: bool,
}

impl Default for ScalpSettings {
    fn default() -> Self {
        Self {
            min_data_len: 60,
            atr_period: 14,
            kalman_period: 20,
            base_kalman_threshold: 0.08, // Lowered to catch trends earlier
            base_m1_surge_threshold: 0.20, // Sniper: Catch smaller initial impulses (0.2 ATR)
            min_conviction: 60.0,
            max_limit_dist_atr_mult: 0.6,
            htf_bias_weight: 0.35,
            ensemble_weight: 0.25,
            flow_surge_confluence_boost: 0.15,
            inducement_weight: 0.6,
            risk_reward_ratio_tp1: 1.25,
            risk_reward_ratio_tp2: 2.5,
            sl_atr_multiplier_inducement: 0.6,
            sl_atr_multiplier_flow: 1.2,
            logistic_scale: 2.93,
            logistic_offset: 0.75,
            limit_order_expiration: 180,
            time_stop_seconds: 3600,
            news_pre_event_block_minutes: 30,
            news_post_event_block_minutes: 15,
            news_guard_atr_spike_multiplier: 2.5,
            momentum_kalman_threshold: 0.7,
            momentum_m1_threshold: 0.6,
            
            fade_sma_period: 20,
            fade_std_dev_mult: 3.0,
            fade_parabolic_atr_mult: 0.8,
            fade_sl_atr_mult: 0.3,
            fade_tp1_atr_mult: 1.0,
            fade_max_adx: 30.0,
            momentum_risk_atr_mult: 0.24,
            momentum_tp1_atr_mult: 1.5, // Widened from 0.7
            momentum_tp2_atr_mult: 3.0, // Widened from 1.3
            pullback_tp1_atr_mult: 1.0, // Widened from 0.5
            pullback_tp2_atr_mult: 2.0, // Widened from 1.0
            // FIXED: Raised from 0.09 → 0.30 ATR. The original 0.09 ATR stop
            // (~$0.30–$0.80 on XAUUSD) was smaller than typical broker spread
            // and would be hit by spread widening alone on market execution.
            pullback_sl_atr_mult: 0.30,
            min_sl_atr_mult: 0.15,
            max_sl_atr_mult: 1.5,
            kalman_weight: 0.6,
            m1_surge_weight: 0.35,
            
            allow_asia_trading: false,
            allow_london_open_momentum: false,
            allow_ny_late_momentum: false,
            m1_roc_period: 3, // FIXED: Require 3-bar sustained M1 move (was 1 — single-bar noise)
            m1_atr_conversion_div: 5.0,
            vol_regime_clamp_min: 0.5,
            vol_regime_clamp_max: 2.0,
            flow_threshold_mult: 0.5,
            inducement_opposing_reduction: 0.5,
            momentum_min_risk_atr: 0.21,
            filter_scalp_by_swing: true, // ENABLED: Only scalp in the direction of H1 swing bias
            push_notifications_enabled: true,
            push_notification_threshold: 60.0,
            pullback_entry_displacement_atr: 0.30, // RAISED: Require deeper pullbacks to fair value (was 0.20)
            momentum_require_ml_confluence: true,
            // note: swing evaluation knobs belong to `SwingSettings`
            max_signals_per_day: 8, // Unified with swing engine
            circuit_breaker_enabled: true,
        }
    }
}

fn default_pullback_entry_displacement_atr() -> f64 { 0.20 }
fn default_momentum_require_ml_confluence() -> bool { true }
fn default_max_scalp_signals_per_day() -> usize { default_max_signals_per_day() }
fn default_circuit_breaker_enabled() -> bool { true }
fn default_eval_on_h1_only() -> bool { false }
fn default_eval_atr_multiplier() -> f64 { 1.5 }
fn default_eval_struct_margin_atr() -> f64 { 0.5 }
fn default_eval_m15_check_enabled() -> bool { true }

impl ScalpSettings {
    pub fn validate(&self) -> Result<(), String> {
        if self.min_conviction < 0.0 || self.min_conviction > 100.0 {
            return Err(format!("Scalp min_conviction must be between 0.0 and 100.0, got {}", self.min_conviction));
        }
        if self.push_notification_threshold < 0.0 || self.push_notification_threshold > 100.0 {
            return Err(format!("Scalp push_notification_threshold must be between 0.0 and 100.0, got {}", self.push_notification_threshold));
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
    pub adx_period: usize,
    pub atr_avg_lookback: usize,
    pub swing_lookback: usize,
    pub swing_neighbors: usize,
    pub ensemble_h1_weight: f64,
    pub ensemble_d1_weight: f64,
    pub displacement_atr_mult: f64,
    pub displacement_strength_bonus: f64,
    pub displacement_rsi_period: usize,
    pub displacement_rsi_threshold_bull: f64,
    pub displacement_rsi_threshold_bear: f64,
    pub displacement_rsi_bonus: f64,
    pub fvg_lookback: usize,
    #[serde(default = "default_fractal_guard_enabled")]
    pub fractal_guard_enabled: bool,
    #[serde(default = "default_m15_swing_lookback")]
    pub m15_swing_lookback: usize,
    #[serde(default = "default_m5_swing_lookback")]
    pub m5_swing_lookback: usize,
    #[serde(default = "default_m30_swing_lookback")]
    pub m30_swing_lookback: usize,
    #[serde(default = "default_fractal_penalty")]
    pub fractal_penalty_score: f64,
    #[serde(default = "default_push_notifications_enabled")]
    pub push_notifications_enabled: bool,
    #[serde(default = "default_push_notification_threshold")]
    pub push_notification_threshold: f64,
    // Evaluation tuning knobs
    #[serde(default = "default_eval_on_h1_only")]
    pub eval_on_h1_only: bool,
    #[serde(default = "default_eval_atr_multiplier")]
    pub eval_atr_multiplier: f64,
    #[serde(default = "default_eval_struct_margin_atr")]
    pub eval_struct_margin_atr: f64,
    #[serde(default = "default_eval_m15_check_enabled")]
    pub eval_m15_check_enabled: bool,

    // Daily Circuit Breaker
    /// Maximum swing signals per UTC day before silencing the engine.
    #[serde(default = "default_max_swing_signals_per_day")]
    pub max_signals_per_day: usize,
    /// Master switch for the swing daily circuit breaker.
    #[serde(default = "default_circuit_breaker_enabled")]
    pub circuit_breaker_enabled: bool,
}

impl Default for SwingSettings {
    fn default() -> Self {
        Self {
            min_data_len: 60,
            atr_period: 14,
            conviction_threshold: 62.0, // RAISED: Require at least 2-3 confluence factors (was 50.0)
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
            adx_period: 14,
            atr_avg_lookback: 50,
            swing_lookback: 60,
            swing_neighbors: 3,
            ensemble_h1_weight: 0.7,
            ensemble_d1_weight: 0.3,
            displacement_atr_mult: 0.7,
            displacement_strength_bonus: 50.0,
            displacement_rsi_period: 14,
            displacement_rsi_threshold_bull: 55.0,
            displacement_rsi_threshold_bear: 45.0,
            displacement_rsi_bonus: 30.0,
            fvg_lookback: 10,
            fractal_guard_enabled: true,
            m15_swing_lookback: 6,
            m5_swing_lookback: 60,
            m30_swing_lookback: 30,
            fractal_penalty_score: 25.0,
            push_notifications_enabled: true,
            push_notification_threshold: 60.0,
            eval_on_h1_only: false,
            eval_atr_multiplier: 1.5,
            eval_struct_margin_atr: 0.5,
            eval_m15_check_enabled: true,
            max_signals_per_day: 8, // Unified with scalp engine
            circuit_breaker_enabled: true,
        }
    }
}

fn default_fractal_guard_enabled() -> bool { true }
fn default_m15_swing_lookback() -> usize { 6 }
fn default_m5_swing_lookback() -> usize { 60 }
fn default_m30_swing_lookback() -> usize { 30 }
fn default_fractal_penalty() -> f64 { 25.0 }
fn default_push_notifications_enabled() -> bool { true }
fn default_push_notification_threshold() -> f64 { 60.0 }
/// Single source of truth for both scalp and swing daily signal cap.
/// Override via API: POST /settings  or env: APP__TRADING__SCALP__MAX_SIGNALS_PER_DAY
fn default_max_signals_per_day() -> usize { 8 }
fn default_max_swing_signals_per_day() -> usize { default_max_signals_per_day() }

impl SwingSettings {
    pub fn validate(&self) -> Result<(), String> {
        if self.conviction_threshold < 0.0 || self.conviction_threshold > 100.0 {
            return Err(format!("Swing conviction_threshold must be between 0.0 and 100.0, got {}", self.conviction_threshold));
        }
        if self.push_notification_threshold < 0.0 || self.push_notification_threshold > 100.0 {
            return Err(format!("Swing push_notification_threshold must be between 0.0 and 100.0, got {}", self.push_notification_threshold));
        }
        Ok(())
    }
}

impl Settings {
    pub fn new() -> Result<Self, ConfigError> {
        // Check for DATABASE_URL environment variable (standard on Render)
        let database_url = std::env::var("DATABASE_URL").unwrap_or_default();

        let s = Config::builder()
            .set_default("server.host", "0.0.0.0")?
            .set_default("server.port", 3000)?
            .set_default("trading.max_buffer_size", 500)?
            .set_default("paths.push_tokens_file", "push_tokens.json")?
            .set_default("paths.models_dir", "./models/")?
            .set_default("paths.sessions_dir", "./sessions_data")?
            .set_default("database.url", database_url)?
            // Look for config.toml in the current directory
            .add_source(File::with_name("config").required(false))
            // Look for config.toml in the standard Render secrets directory
            .add_source(File::with_name("/etc/secrets/config").required(false))
            // Look for config.toml in the persistent data directory (to load API updates on restart)
            .add_source(File::with_name("/data/sessions/config").required(false)) 
            .add_source(File::with_name("./sessions_data/config").required(false))
            // Allow override with env vars (e.g. APP_SERVER__PORT=3000)
            .add_source(Environment::with_prefix("APP").separator("__"))
            .build()?;

        s.try_deserialize()
    }
}