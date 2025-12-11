use config::{Config, ConfigError, File, Environment};
use serde::Deserialize;

#[derive(Debug, Deserialize, Clone)]
pub struct Settings {
    pub server: ServerSettings,
    pub trading: TradingSettings,
    pub paths: PathSettings,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ServerSettings {
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Deserialize, Clone)]
pub struct TradingSettings {
    pub max_buffer_size: usize,
}

#[derive(Debug, Deserialize, Clone)]
pub struct PathSettings {
    pub push_tokens_file: String,
    pub models_dir: String,
}

impl Settings {
    pub fn new() -> Result<Self, ConfigError> {
        let s = Config::builder()
            .set_default("server.host", "0.0.0.0")?
            .set_default("server.port", 3000)?
            .set_default("trading.max_buffer_size", 500)?
            .set_default("paths.push_tokens_file", "push_tokens.json")?
            .set_default("paths.models_dir", "./models/")?
            // Look for config.toml in the current directory
            .add_source(File::with_name("config").required(false))
            // Allow override with env vars (e.g. APP_SERVER__PORT=3000)
            .add_source(Environment::with_prefix("APP").separator("__"))
            .build()?;

        s.try_deserialize()
    }
}