use crate::NewsEvent;
use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};
use reqwest::Client;
use serde::Deserialize;
use tracing::{error, info};

/// Represents the structure of a single event from the Forex Factory JSON endpoint.
#[derive(Deserialize, Debug)]
struct ForexFactoryEvent {
    title: String,
    country: String,
    date: String, // e.g., "2025-12-10 13:30:00"
    impact: String,
    // We don't need forecast, previous, or actual for now, but they are available.
    forecast: Option<String>,
    previous: Option<String>,
    actual: Option<String>,
}

/// Fetches and parses the economic calendar from the stable Forex Factory JSON endpoint.
/// This function is tailored to find high-impact USD news relevant to XAU/USD trading.
pub async fn fetch_investing_calendar() -> Vec<NewsEvent> {
    let client = match Client::builder()
        .user_agent("xau_scalper_ml/1.0")
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            error!("Failed to build reqwest client: {}", e);
            return vec![];
        }
    };

    // This public endpoint has been stable for years and is used by many trading tools.
    let url = "https://nfs.faireconomy.media/ff_calendar_thisweek.json";
    
    let events: Vec<ForexFactoryEvent> = match client.get(url).send().await {
        Ok(res) => match res.json().await {
            Ok(json) => json,
            Err(e) => {
                error!("Failed to parse Forex Factory JSON response: {}", e);
                return vec![];
            }
        },
        Err(e) => {
            error!("Failed to fetch Forex Factory calendar: {}", e);
            return vec![];
        }
    };

    let now = Utc::now();
    let mut gold_relevant_events = Vec::new();

    // Major currencies to track
    let major_currencies = ["USD", "EUR", "GBP", "JPY", "AUD", "CAD", "CHF", "NZD", "CNY"];

    for event in events {
        // Filter by impact and major currency
        if !major_currencies.contains(&event.country.as_str()) {
            continue;
        }
        
        if event.impact != "High" && event.impact != "Medium" {
            continue;
        }

        // Parse datetime. The feed usually provides "YYYY-MM-DD HH:MM:SS" without offset.
        // We assume UTC as this specific feed (nfs.faireconomy.media) is typically UTC.
        let datetime_utc = DateTime::parse_from_rfc3339(&event.date)
            .map(|dt| dt.with_timezone(&Utc))
            .or_else(|_| {
                NaiveDateTime::parse_from_str(&event.date, "%Y-%m-%d %H:%M:%S")
                    .map(|naive| Utc.from_utc_datetime(&naive))
            })
            .unwrap_or_else(|e| {
                error!("Failed to parse date '{}': {}", event.date, e);
                Utc::now()
            });

        // Only consider future events (or events within the last 5 minutes, allowing for delays).
        if (datetime_utc - now).num_minutes() < -5 {
            continue;
        }

        // Add event
            let timestamp = datetime_utc.timestamp();
            gold_relevant_events.push(NewsEvent {
                event: event.title,
                timestamp,
                impact: event.impact.to_lowercase(), // Standardize to "high", "medium", "low"
                country: event.country,
                forecast: event.forecast,
                previous: event.previous,
                actual: event.actual,
            });
    }

    // Sort by time ascending for chronological order.
    gold_relevant_events.sort_by_key(|e| e.timestamp);

    info!("Found and parsed {} high-impact USD news events relevant for XAU/USD.", gold_relevant_events.len());
    gold_relevant_events
}