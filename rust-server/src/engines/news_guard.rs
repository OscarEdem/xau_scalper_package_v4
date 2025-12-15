use crate::NewsEvent;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// The result of a guard evaluation, indicating if trading is allowed.
#[derive(Debug, Serialize, Deserialize, ToSchema, Clone)]
pub struct GuardResult {
    pub allowed: bool,
    pub reason: Option<String>,
}

/// Evaluates if the current time is within a blocked window around scheduled news events.
///
/// # Arguments
/// * `current_timestamp` - The current time in Unix seconds.
/// * `symbol` - The trading symbol (e.g., "XAUUSD").
/// * `upcoming_events` - A list of scheduled news events.
/// * `pre_event_block_minutes` - How many minutes before a high-impact event to block trading.
/// * `post_event_block_minutes` - How many minutes after a high-impact event to block trading.
///
/// # Logic
/// - High-impact events use the full block time.
/// - Medium-impact events use half the block time.
/// - Low-impact events are ignored.
pub fn evaluate_news_guard(
    symbol: &str,
    current_timestamp: i64,
    upcoming_events: &[NewsEvent],
    pre_event_block_minutes: i64,
    post_event_block_minutes: i64,
) -> GuardResult {
    // Extract base and quote from symbol (e.g., XAUUSD -> XAU, USD)
    let base = if symbol.len() >= 3 { &symbol[0..3] } else { "" };
    let quote = if symbol.len() >= 6 { &symbol[3..6] } else { "" };

    for event in upcoming_events {
        // Double-check: Only block if the event currency matches base or quote
        if event.currency != base && event.currency != quote {
            continue;
        }

        let (pre_block, post_block) = match event.impact.as_str() {
            "high" => (pre_event_block_minutes * 60, post_event_block_minutes * 60),
            "medium" => (
                (pre_event_block_minutes / 2) * 60,
                (post_event_block_minutes / 2) * 60,
            ),
            _ => (0, 0), // "low" impact or other, no block
        };

        if pre_block == 0 && post_block == 0 {
            continue;
        }

        let time_to_event = event.timestamp - current_timestamp;

        // Check pre-event window (event is in the future)
        if time_to_event > 0 && time_to_event <= pre_block {
            return GuardResult {
                allowed: false,
                reason: Some(format!(
                    "Blocked: Approaching {} impact event '{}'",
                    event.impact, event.event
                )),
            };
        }

        // Check post-event window (event is in the past)
        if time_to_event < 0 && time_to_event.abs() <= post_block {
            return GuardResult {
                allowed: false,
                reason: Some(format!(
                    "Blocked: Post-event volatility after {} impact event '{}'",
                    event.impact, event.event
                )),
            };
        }
    }

    GuardResult {
        allowed: true,
        reason: None,
    }
}

/// Evaluates if the current volatility regime is safe for trading.
///
/// # Arguments
/// * `atr_values` - A series of recent ATR values.
/// * `adx_values` - A series of recent ADX values.
/// * `atr_multiplier` - The factor to detect a spike (e.g., 2.0 means ATR doubled).
/// * `adx_threshold` - The minimum ADX value required for a trending market.
pub fn volatility_guard(
    atr_values: &[f64],
    adx_values: &[f64],
    atr_multiplier: f64,
    adx_threshold: f64,
) -> GuardResult {
    let n_atr = atr_values.len();
    if n_atr >= 2 {
        let last_atr = atr_values[n_atr - 1];
        let prev_atr = atr_values[n_atr - 2];
        if prev_atr > 0.0 && last_atr > prev_atr * atr_multiplier {
            return GuardResult {
                allowed: false,
                reason: Some("Blocked: Volatility Spike Detected".to_string()),
            };
        }
    }

    if let Some(&last_adx) = adx_values.last() {
        if last_adx < adx_threshold {
            return GuardResult {
                allowed: false,
                reason: Some("Blocked: Low Trend Strength (ADX)".to_string()),
            };
        }
    }

    GuardResult {
        allowed: true,
        reason: None,
    }
}

/// A combined wrapper that runs both the news and volatility guards.
/// If any guard returns a "blocked" result, the combined result is blocked.
pub fn combined_guard(
    symbol: &str,
    current_timestamp: i64,
    upcoming_events: &[NewsEvent],
    pre_event_block_minutes: i64,
    post_event_block_minutes: i64,
    atr_values: &[f64],
    adx_values: &[f64],
    atr_multiplier: f64,
    adx_threshold: f64,
) -> GuardResult {
    let news_guard_result = evaluate_news_guard(
        symbol,
        current_timestamp,
        upcoming_events,
        pre_event_block_minutes,
        post_event_block_minutes,
    );
    if !news_guard_result.allowed {
        return news_guard_result;
    }

    let vol_guard_result =
        volatility_guard(atr_values, adx_values, atr_multiplier, adx_threshold);
    if !vol_guard_result.allowed {
        return vol_guard_result;
    }

    GuardResult {
        allowed: true,
        reason: None,
    }
}