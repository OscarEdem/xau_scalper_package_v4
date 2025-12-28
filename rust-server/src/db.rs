use sqlx::postgres::PgPoolOptions;
use sqlx::{Pool, Postgres, Row};
use tracing::info;
use crate::{EvalResponse, TradingSession, NewsEvent};
use crate::config::TradingSettings;
use std::collections::BTreeSet;
use prometheus::Counter;

pub type DbPool = Pool<Postgres>;

macro_rules! db_retry {
    ($action:expr, $counter:expr) => {{
        let mut attempts = 0;
        let max_attempts = 3;
        loop {
            match $action.await {
                Ok(res) => break Ok(res),
                Err(e) => {
                    attempts += 1;
                    if attempts >= max_attempts {
                        break Err(e);
                    }
                    if let Some(c) = $counter {
                        c.inc();
                    }
                    // Retry on PoolTimedOut or IO errors
                    if matches!(e, sqlx::Error::PoolTimedOut | sqlx::Error::Io(_)) {
                        tracing::warn!("Database operation failed (attempt {}/{}), retrying: {}", attempts, max_attempts, e);
                        tokio::time::sleep(std::time::Duration::from_millis(200 * attempts as u64)).await;
                    } else {
                        break Err(e);
                    }
                }
            }
        }
    }};
}

pub async fn init_db(database_url: &str) -> Result<DbPool, sqlx::Error> {
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(database_url)
        .await?;

    info!("Connected to PostgreSQL.");

    // Basic schema migration
    // Uses the migrations folder embedded in the binary
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await?;

    info!("Database migrations applied.");

    Ok(pool)
}

pub async fn save_signal(pool: &DbPool, signal: &EvalResponse, symbol: &str, retry_counter: Option<&Counter>) -> Result<(), sqlx::Error> {
    db_retry!(sqlx::query(
        "INSERT INTO signals (signal_id, symbol, entry_type, timestamp, data) VALUES ($1, $2, $3, $4, $5) ON CONFLICT (signal_id) DO NOTHING"
    )
    .bind(&signal.signal_id)
    .bind(symbol)
    .bind(&signal.entry_type)
    .bind(chrono::Utc::now().timestamp())
    .bind(serde_json::to_value(signal).unwrap())
    .execute(pool), retry_counter)?;
    Ok(())
}

pub async fn save_session(pool: &DbPool, session: &TradingSession, retry_counter: Option<&Counter>) -> Result<(), sqlx::Error> {
    let data = serde_json::to_value(session).unwrap();
    let symbol = &session.symbol;
    let now = chrono::Utc::now().timestamp();
    db_retry!(sqlx::query(
        "INSERT INTO sessions (symbol, data, updated_at) VALUES ($1, $2, $3) ON CONFLICT (symbol) DO UPDATE SET data = $2, updated_at = $3"
    )
    .bind(symbol)
    .bind(&data)
    .bind(now)
    .execute(pool), retry_counter)?;
    Ok(())
}

pub async fn load_sessions(pool: &DbPool, retry_counter: Option<&Counter>) -> Result<Vec<TradingSession>, sqlx::Error> {
    let rows = db_retry!(sqlx::query("SELECT data FROM sessions").fetch_all(pool), retry_counter)?;
    let mut sessions = Vec::new();
    for row in rows {
        let data: serde_json::Value = row.get("data");
        if let Ok(session) = serde_json::from_value(data) {
            sessions.push(session);
        }
    }
    Ok(sessions)
}

pub async fn cleanup_old_signals(pool: &DbPool, retry_counter: Option<&Counter>) -> Result<u64, sqlx::Error> {
    // 1 year in seconds = 365 * 24 * 60 * 60 = 31536000
    let one_year_ago = chrono::Utc::now().timestamp() - 31536000;
    let result = db_retry!(sqlx::query("DELETE FROM signals WHERE timestamp < $1")
        .bind(one_year_ago)
        .execute(pool), retry_counter)?;
    Ok(result.rows_affected())
}

pub async fn save_settings(pool: &DbPool, settings: &TradingSettings, retry_counter: Option<&Counter>) -> Result<(), sqlx::Error> {
    let data = serde_json::to_value(settings).unwrap();
    db_retry!(sqlx::query("INSERT INTO settings (key, value) VALUES ($1, $2) ON CONFLICT (key) DO UPDATE SET value = $2")
        .bind("trading_settings")
        .bind(&data)
        .execute(pool), retry_counter)?;
    Ok(())
}

pub async fn load_settings(pool: &DbPool, retry_counter: Option<&Counter>) -> Result<Option<TradingSettings>, sqlx::Error> {
    let row = db_retry!(sqlx::query("SELECT value FROM settings WHERE key = $1")
        .bind("trading_settings")
        .fetch_optional(pool), retry_counter)?;

    if let Some(row) = row {
        let data: serde_json::Value = row.get("value");
        let settings: TradingSettings = serde_json::from_value(data).map_err(|e| sqlx::Error::Decode(Box::new(e)))?;
        Ok(Some(settings))
    } else {
        Ok(None)
    }
}

pub async fn save_push_token(pool: &DbPool, token: &str, retry_counter: Option<&Counter>) -> Result<(), sqlx::Error> {
    let now = chrono::Utc::now().timestamp();
    db_retry!(sqlx::query("INSERT INTO push_tokens (token, created_at) VALUES ($1, $2) ON CONFLICT (token) DO NOTHING").bind(token).bind(now).execute(pool), retry_counter)?;
    Ok(())
}

pub async fn load_push_tokens(pool: &DbPool, retry_counter: Option<&Counter>) -> Result<BTreeSet<String>, sqlx::Error> {
    let rows = db_retry!(sqlx::query("SELECT token FROM push_tokens").fetch_all(pool), retry_counter)?;
    Ok(rows.into_iter().map(|r| r.get("token")).collect())
}

pub async fn save_news_events(pool: &DbPool, events: &[NewsEvent], retry_counter: Option<&Counter>) -> Result<(), sqlx::Error> {
    for event in events {
        db_retry!(sqlx::query(
            "INSERT INTO news_events (event, timestamp, currency, impact, data) VALUES ($1, $2, $3, $4, $5) ON CONFLICT (event, timestamp, currency) DO UPDATE SET data = $5"
        )
        .bind(&event.event)
        .bind(event.timestamp)
        .bind(&event.currency)
        .bind(&event.impact)
        .bind(serde_json::to_value(event).unwrap())
        .execute(pool), retry_counter)?;
    }
    Ok(())
}

pub async fn save_analysis_report(pool: &DbPool, symbol: &str, period: &str, report: &serde_json::Value, events_hash: u64, retry_counter: Option<&Counter>) -> Result<(), sqlx::Error> {
    let now = chrono::Utc::now().timestamp();
    db_retry!(sqlx::query(
        "INSERT INTO analysis_reports (symbol, period, created_at, report, events_hash) VALUES ($1, $2, $3, $4, $5)"
    )
    .bind(symbol)
    .bind(period)
    .bind(now)
    .bind(report)
    .bind(events_hash.to_string())
    .execute(pool), retry_counter)?;
    Ok(())
}

pub async fn clear_all_signals(pool: &DbPool, retry_counter: Option<&Counter>) -> Result<u64, sqlx::Error> {
    let result = db_retry!(sqlx::query("DELETE FROM signals").execute(pool), retry_counter)?;
    Ok(result.rows_affected())
}