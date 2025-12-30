use crate::NewsEvent;
use crate::CalendarEvent;
use crate::macro_analysis::classification::classify_event;
use chrono::{Datelike, DateTime, Duration, FixedOffset, NaiveDate, NaiveDateTime, TimeZone, Utc};
use tracing::{debug, info};

/// Maps ForexFactory country codes to ISO currency codes.
pub fn map_country_to_currency(country: &str) -> Option<String> {
    match country.to_uppercase().as_str() {
        "US" | "USA" | "USD" | "UNITED STATES" => Some("USD".to_string()),
        "EU" | "EMU" | "EUR" | "DE" | "FR" | "IT" | "ES" | "EUROZONE" => Some("EUR".to_string()),
        "GB" | "UK" | "GBP" | "GREAT BRITAIN" | "UNITED KINGDOM" => Some("GBP".to_string()),
        "CA" | "CAN" | "CAD" | "CANADA" => Some("CAD".to_string()),
        "AU" | "AUS" | "AUD" | "AUSTRALIA" => Some("AUD".to_string()),
        "NZ" | "NZD" | "NEW ZEALAND" => Some("NZD".to_string()),
        "JP" | "JPN" | "JPY" | "JAPAN" => Some("JPY".to_string()),
        "CH" | "CHF" | "SWITZERLAND" => Some("CHF".to_string()),
        "CN" | "CNY" | "CHINA" => Some("CNY".to_string()),
        _ => None,
    }
}

/// Determines if New York is in Daylight Saving Time for a given naive datetime.
/// DST starts 2nd Sunday in March and ends 1st Sunday in November.
fn is_ny_dst(dt: NaiveDateTime) -> bool {
    let year = dt.year();
    
    // DST starts 2nd Sunday in March at 02:00
    let march_1 = NaiveDate::from_ymd_opt(year, 3, 1).unwrap();
    let days_to_sunday_mar = (7 - march_1.weekday().num_days_from_sunday()) % 7;
    let second_sunday_march = march_1 + Duration::days(days_to_sunday_mar as i64 + 7);
    let dst_start = second_sunday_march.and_hms_opt(2, 0, 0).unwrap();

    // DST ends 1st Sunday in November at 02:00
    let nov_1 = NaiveDate::from_ymd_opt(year, 11, 1).unwrap();
    let days_to_sunday_nov = (7 - nov_1.weekday().num_days_from_sunday()) % 7;
    let first_sunday_nov = nov_1 + Duration::days(days_to_sunday_nov as i64);
    let dst_end = first_sunday_nov.and_hms_opt(2, 0, 0).unwrap();

    dt >= dst_start && dt < dst_end
}

/// Parses ForexFactory datetime (ET) and converts it to UTC timestamp.
pub fn parse_forexfactory_datetime_to_utc(date_str: &str) -> Option<i64> {
    // 1. Try parsing as RFC3339 / ISO 8601 with offset first (e.g., "2025-12-14T16:30:00-05:00")
    if let Ok(dt) = DateTime::parse_from_rfc3339(date_str) {
        return Some(dt.with_timezone(&Utc).timestamp());
    }

    // Try parsing standard formats
    let naive = NaiveDateTime::parse_from_str(date_str, "%Y-%m-%d %H:%M:%S")
        .or_else(|_| NaiveDateTime::parse_from_str(date_str, "%Y-%m-%dT%H:%M:%S"))
        .or_else(|_| NaiveDateTime::parse_from_str(date_str, "%m-%d-%Y %H:%M:%S"))
        .ok()?;

    // Determine offset: EDT is UTC-4, EST is UTC-5
    let is_dst = is_ny_dst(naive);
    let offset_hours = if is_dst { -4 } else { -5 };
    let offset = FixedOffset::east_opt(offset_hours * 3600).unwrap_or(FixedOffset::east_opt(0).unwrap());

    // The naive time is in ET, so we interpret it with that offset
    let dt_with_tz = offset.from_local_datetime(&naive).single()?;
    
    Some(dt_with_tz.with_timezone(&Utc).timestamp())
}

/// Processes raw calendar events: filters by impact, maps currency, and classifies.
pub fn process_calendar_events(events: &[CalendarEvent]) -> Vec<NewsEvent> {
    info!("Processing {} raw calendar events...", events.len());
    let mut relevant_events = Vec::new();

    for event in events {
        // 3. Map country to currency
        let currency = match map_country_to_currency(&event.country) {
            Some(c) => c,
            None => {
                debug!("Skipping event '{}' due to unknown country: '{}'", event.title, event.country);
                continue;
            }
        };

        // 4. Filter by Impact (High or Medium only)
        let impact = event.impact.to_lowercase();
        if impact != "high" && impact != "medium" {
            continue;
        }

        // 5. Timezone correction
        let timestamp = match parse_forexfactory_datetime_to_utc(&event.date) {
            Some(ts) => ts,
            None => {
                debug!("Skipping event '{}' due to date parse failure: '{}'", event.title, event.date);
                continue;
            }
        };

        let category = classify_event(&event.title);

        relevant_events.push(NewsEvent {
            event: event.title.clone(),
            timestamp,
            impact,
            country: event.country.clone(),
            currency,
            forecast: event.forecast.clone(),
            previous: event.previous.clone(),
            actual: event.actual.clone(),
            category,
        });
    }

    // 7. Sort ascending
    relevant_events.sort_by_key(|e| e.timestamp);

    info!("Processed {} global high-impact news events.", relevant_events.len());
    relevant_events
}