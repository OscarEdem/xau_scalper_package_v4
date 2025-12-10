use crate::NewsEvent;
use chrono::{NaiveDate, NaiveTime, Utc};
use reqwest::Client;
use scraper::{Html, Selector};
use serde::Deserialize;
use tracing::{error, info};

#[derive(Deserialize)]
struct CalendarResponse {
    data: String,
}

/// Fetches and parses the economic calendar from Investing.com for the current day.
pub async fn fetch_investing_calendar() -> Vec<NewsEvent> {
    let client = Client::new();
    let today = Utc::now().format("%Y-%m-%d").to_string();

    // These are the form parameters Investing.com's XHR endpoint expects.
    // We are fetching for the whole day.
    let params = [
        ("country[]", "72"), // USA
        ("country[]", "5"),  // UK
        ("country[]", "17"), // Eurozone
        ("country[]", "35"), // Australia
        ("country[]", "25"), // Canada
        ("country[]", "32"), // China
        ("country[]", "6"),  // Germany
        ("country[]", "37"), // New Zealand
        ("country[]", "43"), // Switzerland
        ("country[]", "22"), // Japan
        ("importance[]", "2"),
        ("importance[]", "3"),
        ("dateFrom", &today),
        ("dateTo", &today),
        ("timeZone", "8"), // GMT
    ];

    let res = match client
        .post("https://sslecal2.investing.com/events/get_calendar_by_range")
        .header("x-requested-with", "XMLHttpRequest")
        .header("Origin", "https://www.investing.com")
        .header("Referer", "https://www.investing.com/economic-calendar/")
        .form(&params)
        .send()
        .await
    {
        Ok(res) => res,
        Err(e) => {
            error!("Failed to fetch economic calendar: {}", e);
            return vec![];
        }
    };

    let text = match res.text().await {
        Ok(text) => text,
        Err(e) => {
            error!("Failed to read calendar response body: {}", e);
            return vec![];
        }
    };

    let calendar_response: CalendarResponse = match serde_json::from_str(&text) {
        Ok(json) => json,
        Err(e) => {
            error!("Failed to parse calendar JSON response: {}. Body: {}", e, text);
            return vec![];
        }
    };

    let html_fragment = Html::parse_fragment(&calendar_response.data);
    let row_selector = Selector::parse("tr.js-event-item").unwrap();
    let time_selector = Selector::parse("td.time").unwrap();
    let event_selector = Selector::parse("td.event a").unwrap();
    let country_selector = Selector::parse("td.flagCur span").unwrap();
    let impact_selector = Selector::parse("td.sentiment i").unwrap();

    let mut events = Vec::new();
    let now: NaiveDate = Utc::now().date_naive();

    for row in html_fragment.select(&row_selector) {
        let time_str = row.select(&time_selector).next().map_or("".to_string(), |n| n.inner_html().trim().to_string());
        let event_name = row.select(&event_selector).next().map_or("".to_string(), |n| n.inner_html().trim().to_string());
        let country = row.select(&country_selector).next().map_or("", |n| n.value().attr("title").unwrap_or(""));
        let impact_stars = row.select(&impact_selector).filter(|n| n.value().has_class("grayFullBull", scraper::CaseSensitivity::AsciiCaseInsensitive)).count();
 
        let impact = match impact_stars {
            3 => "high".to_string(),
            2 => "medium".to_string(),
            _ => "low".to_string(),
        };

        if let Ok(time) = NaiveTime::parse_from_str(&time_str, "%H:%M") {
            let datetime = now.and_time(time);
            let timestamp = datetime.and_local_timezone(Utc).unwrap().timestamp();

            events.push(NewsEvent {
                event: event_name,
                timestamp,
                impact: impact,
                country: country.to_string(),
            });
        }
    }
    info!("Fetched and parsed {} news events for today.", events.len());
    events
}