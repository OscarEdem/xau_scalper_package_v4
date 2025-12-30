// src/external_feeds.rs
use crate::state::ApplicationStateWithTicks;
use xau_scalper_server::CalendarEvent;
use xau_scalper_server::NewsItem;
use std::sync::Arc;
use xau_scalper_server::news_fetcher::process_calendar_events;
use tokio::sync::broadcast;
use tokio::time::{sleep, Duration, Instant};
use reqwest::Client;
use crate::db;

// --- Configuration ---
const CALENDAR_URL: &str = "https://nfs.faireconomy.media/ff_calendar_thisweek.json";
const CALENDAR_CACHE_DURATION: Duration = Duration::from_secs(15 * 60); // 15 minutes
const RSS_POLL_INTERVAL: Duration = Duration::from_secs(5 * 60); // 5 minutes

const RSS_FEEDS: &[(&str, &str)] = &[
    ("World News", "https://investing.com/rss/news_287.rss"),
    ("Commodities", "https://investing.com/rss/news_11.rss"),
    ("Indicators", "https://investing.com/rss/news_95.rss"),
    ("Economy", "https://investing.com/rss/news_14.rss"),
];

// --- Service ---

pub fn spawn_external_feeds_task(
    state: Arc<ApplicationStateWithTicks>, // State passed for future integration (e.g. DB storage)
    mut shutdown: broadcast::Receiver<()>,
) {
    tokio::spawn(async move {
        tracing::info!("External Market News & Calendar service started.");
        
        let client = Client::builder()
            .user_agent("Mozilla/5.0 (compatible; XAU_Scalper_Bot/4.0)")
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_default();

        // --- Load persisted RSS news on startup ---
        match db::load_rss_news(&state.inner.db, Some(&state.inner.metrics.db_retries_total)).await {
            Ok(items) => {
                tracing::info!("Loaded {} RSS news items from DB.", items.len());
                *state.inner.external_rss_news.lock().await = items;
            },
            Err(e) => tracing::warn!("Failed to load RSS news from DB: {}", e),
        }

        let mut last_calendar_fetch = Instant::now().checked_sub(CALENDAR_CACHE_DURATION * 2).unwrap_or(Instant::now());
        let mut last_rss_fetch = Instant::now().checked_sub(RSS_POLL_INTERVAL * 2).unwrap_or(Instant::now());
        

        loop {
            tokio::select! {
                _ = shutdown.recv() => {
                    tracing::info!("External feeds task shutting down.");
                    break;
                }
                _ = sleep(Duration::from_secs(60)) => {
                    // 1. Handle Calendar (with Guardrails & Caching)
                    if last_calendar_fetch.elapsed() >= CALENDAR_CACHE_DURATION {
                        match fetch_calendar(&client).await {
                            Ok(events) => {
                                tracing::info!("Fetched {} economic calendar events.", events.len());
                                // 1. Update Raw Cache (for API)
                                *state.inner.external_calendar_events.lock().await = events.clone();
                                
                                // 2. Process for Trading Engine (Filter High/Medium impact, map currency)
                                let processed_events = process_calendar_events(&events);
                                *state.inner.news_events.lock().await = processed_events;
                                
                                // 3. Save to DB
                                let db = state.inner.db.clone();
                                let metrics = state.inner.metrics.clone();
                                let events_to_save = state.inner.news_events.lock().await.clone();
                                tokio::spawn(async move { let _ = crate::db::save_news_events(&db, &events_to_save, Some(&metrics.db_retries_total)).await; });

                                last_calendar_fetch = Instant::now();
                            },
                            Err(e) => {
                                tracing::error!("Failed to fetch Economic Calendar: {}. Using cached data ({} events) if available.", e, state.inner.external_calendar_events.lock().await.len());
                                // Backoff strategy:
                                // If we failed (especially 429), wait 5 minutes before retrying instead of 1 minute.
                                // CALENDAR_CACHE_DURATION is 15 mins. We set last_fetch so that it expires in 5 mins.
                                // last_fetch = now - (15min - 5min) = now - 10min.
                                let backoff = CALENDAR_CACHE_DURATION.saturating_sub(Duration::from_secs(300));
                                last_calendar_fetch = Instant::now().checked_sub(backoff).unwrap_or(Instant::now());
                            }
                        }
                    }

                    // 2. Handle RSS Feeds
                    if last_rss_fetch.elapsed() >= RSS_POLL_INTERVAL {
                        let mut aggregated_news = Vec::new();
                        let mut seen_links = std::collections::HashSet::new();
                        for (category, url) in RSS_FEEDS {
                            match fetch_rss(&client, url, category).await {
                                Ok(news) => {
                                    tracing::info!("Fetched {} news items for {}.", news.len(), category);
                                    for item in news {
                                        if seen_links.insert(item.link.clone()) {
                                            aggregated_news.push(item);
                                        }
                                    }
                                },
                                Err(e) => {
                                    tracing::error!("Failed to fetch RSS {}: {}", category, e);
                                }
                            }
                            // Be gentle with requests
                            sleep(Duration::from_secs(2)).await;
                        }
                        
                        // Replace the state with the fresh batch to avoid duplicates/infinite growth
                        if !aggregated_news.is_empty() {
                            *state.inner.external_rss_news.lock().await = aggregated_news;
                            
                            // --- Persist to DB ---
                            let db = state.inner.db.clone();
                            let metrics = state.inner.metrics.clone();
                            let news_to_save = state.inner.external_rss_news.lock().await.clone();
                            tokio::spawn(async move {
                                if let Err(e) = db::save_rss_news(&db, &news_to_save, Some(&metrics.db_retries_total)).await {
                                    tracing::error!("Failed to save RSS news to DB: {}", e);
                                }
                            });
                        }
                        last_rss_fetch = Instant::now();
                    }
                }
            }
        }
    });
}

async fn fetch_calendar(client: &Client) -> Result<Vec<CalendarEvent>, Box<dyn std::error::Error + Send + Sync>> {
    let resp = client.get(CALENDAR_URL).send().await?;
    if !resp.status().is_success() {
        return Err(format!("Status code: {}", resp.status()).into());
    }
    let events = resp.json::<Vec<CalendarEvent>>().await?;
    Ok(events)
}

async fn fetch_rss(client: &Client, url: &str, source: &str) -> Result<Vec<NewsItem>, Box<dyn std::error::Error + Send + Sync>> {
    let resp = client.get(url).send().await?;
    let xml = resp.text().await?;
    
    // Simple XML parsing to avoid adding 'rss' crate dependency if not present.
    // If you have the 'rss' crate, it is recommended to use it instead.
    let mut items = Vec::new();
    
    // Split by <item>
    let chunks: Vec<&str> = xml.split("<item>").collect();
    for chunk in chunks.iter().skip(1) { // Skip header
        if let Some(end_idx) = chunk.find("</item>") {
            let content = &chunk[..end_idx];
            
            let title = extract_tag(content, "title").unwrap_or_default();
            let link = extract_tag(content, "link").unwrap_or_default();
            let pub_date = extract_tag(content, "pubDate").unwrap_or_default();
            let image_url = extract_attribute(content, "enclosure", "url");
            let author = extract_tag(content, "author");

            if !title.is_empty() {
                items.push(NewsItem {
                    title,
                    link,
                    pub_date,
                    source: source.to_string(),
                    image_url,
                    author,
                });
            }
        }
    }
    
    Ok(items)
}

fn extract_tag(content: &str, tag: &str) -> Option<String> {
    let open_tag = format!("<{}>", tag);
    let close_tag = format!("</{}>", tag);
    
    let start = content.find(&open_tag)?;
    let content_after_start = content.get(start..)?;
    let end_relative = content_after_start.find(&close_tag)?;
    let end = start + end_relative;
    let value_start = start + open_tag.len();

    if value_start <= end {
        if let Some(raw) = content.get(value_start..end) {
            // Handle CDATA if present
            if let Some(cdata_start) = raw.find("<![CDATA[") {
                if let Some(cdata_end) = raw.find("]]>") {
                    return Some(raw[cdata_start + 9..cdata_end].trim().to_string());
                }
            }
            return Some(raw.trim().to_string());
        }
    }
    None
}

fn extract_attribute(content: &str, tag: &str, attr: &str) -> Option<String> {
    let tag_start = format!("<{}", tag);
    let start = content.find(&tag_start)?;
    let content_after_start = content.get(start..)?;
    let end = content_after_start.find('>')?;
    
    if let Some(tag_content) = content.get(start..start+end) {
            let attr_search = format!("{}=\"", attr);
            if let Some(attr_start) = tag_content.find(&attr_search) {
                let val_start = attr_start + attr_search.len();
                if let Some(val_end) = tag_content.get(val_start..)?.find('"') {
                    return tag_content.get(val_start..val_start+val_end).map(|s| s.to_string());
                }
            }
            // Try single quotes
             let attr_search_sq = format!("{}='", attr);
            if let Some(attr_start) = tag_content.find(&attr_search_sq) {
                let val_start = attr_start + attr_search_sq.len();
                if let Some(val_end) = tag_content.get(val_start..)?.find('\'') {
                    return tag_content.get(val_start..val_start+val_end).map(|s| s.to_string());
                }
            }
    }
    None
}
