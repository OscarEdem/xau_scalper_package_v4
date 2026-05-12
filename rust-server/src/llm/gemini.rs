use reqwest::Client;
use serde_json::json;
use anyhow::{anyhow, Result};
use std::time::Duration;
use tokio::time::sleep;
use xau_scalper_server::metrics::AppMetrics;
use futures_util::{Stream, StreamExt};
use std::pin::Pin;
use std::sync::atomic::{AtomicI64, Ordering};
use chrono::Utc;

// --- Circuit Breaker State ---
static LAST_429_TIME: AtomicI64 = AtomicI64::new(0);
static ERROR_COUNT_429: AtomicI64 = AtomicI64::new(0);
const CIRCUIT_BREAKER_THRESHOLD: i64 = 3;
const CIRCUIT_BREAKER_COOLDOWN_SEC: i64 = 600; // 10 minutes

fn check_circuit_breaker() -> Result<()> {
    let now = Utc::now().timestamp();
    let last_429 = LAST_429_TIME.load(Ordering::SeqCst);
    
    if now - last_429 < CIRCUIT_BREAKER_COOLDOWN_SEC {
        if ERROR_COUNT_429.load(Ordering::SeqCst) >= CIRCUIT_BREAKER_THRESHOLD {
            return Err(anyhow!("Gemini API circuit breaker engaged (Too Many 429s). Cooldown: {}s remaining.", CIRCUIT_BREAKER_COOLDOWN_SEC - (now - last_429)));
        }
    } else {
        // Reset count after cooldown period has passed
        if ERROR_COUNT_429.load(Ordering::SeqCst) >= CIRCUIT_BREAKER_THRESHOLD {
            ERROR_COUNT_429.store(0, Ordering::SeqCst);
            tracing::info!("Gemini API circuit breaker reset.");
        }
    }
    Ok(())
}

fn record_429() {
    LAST_429_TIME.store(Utc::now().timestamp(), Ordering::SeqCst);
    ERROR_COUNT_429.fetch_add(1, Ordering::SeqCst);
}

fn record_success() {
    // Only reset if we were below threshold, or just clear it
    ERROR_COUNT_429.store(0, Ordering::SeqCst);
}

// Use the v1beta endpoint for the latest models
const GEMINI_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta/models";

pub async fn generate_analysis(
    client: &Client,
    pair: &str,
    recent_data: &str,
    prompt: &str,
    metrics: &AppMetrics,
    image_base64: Option<&str>,
    account_info: Option<(f64, f64, f64)>, // (balance, equity, drawdown)
) -> Result<serde_json::Value> {
    // 1. Check Circuit Breaker
    check_circuit_breaker()?;

    // 2. Get API Key
    let api_key = std::env::var("GEMINI_API_KEY")
        .map_err(|e| anyhow!("GEMINI_API_KEY environment variable not set: {}", e))?;

    // 2. Build Request Body with JSON Schema for stability
    let mut system_info = String::new();
    if let Some((bal, eq, dd)) = account_info {
        system_info = format!(
            "\n[LIVE TERMINAL STATE - MANDATORY PRIORITY]\n- Current Balance: ${:.2}\n- Current Equity: ${:.2}\n- Live Drawdown: {:.2}%\nNote: These values are fetched directly from the MT5 terminal and represent your actual live standing, ignoring any static default settings.\n",
            bal, eq, dd
        );
    }

    let mut parts = vec![json!({
        "text": format!(
            "{}{}\n\nPAIR: {}\nRECENT DATA (Includes current_price for grounding):\n{}",
            prompt, system_info, pair, recent_data
        )
    })];

    if let Some(img) = image_base64 {
        parts.push(json!({
            "inline_data": {
                "mime_type": "image/png",
                "data": img
            }
        }));
    }

    let body = json!({
        "contents": [{
            "parts": parts
        }],
        "tools": [{
            "google_search": {}
        }],
        "generationConfig": {
            "temperature": 0.4,
            "topP": 0.9,
            "maxOutputTokens": 2048,
        }
    });

    // Free-tier GA models only (Gemini 2.5 and 2.0 families)
    let models = [
        "gemini-2.5-flash-lite",  // Free tier: fastest, highest RPM/RPD
        "gemini-2.5-flash",       // Free tier: higher quality
        "gemini-2.0-flash",       // Free tier: solid fallback
    ];
    let mut last_error = anyhow!("No models available");

    for model in models {
        let url = format!("{}/{}:generateContent", GEMINI_BASE_URL, model);
        let mut attempt = 0;
        let max_retries = 3;
        let mut backoff = Duration::from_secs(2);

        loop {
            // 3. Send Request
            let res_result = client
                .post(&url)
                .header("x-goog-api-key", &api_key)
                .json(&body)
                .send()
                .await;

            let res = match res_result {
                Ok(r) => r,
                Err(e) => {
                    last_error = anyhow!("Request failed for {}: {}", model, e);
                    if attempt >= max_retries { break; }
                    tracing::warn!("Request failed for {}: {}. Retrying...", model, e);
                    sleep(backoff).await;
                    attempt += 1;
                    backoff *= 2;
                    continue;
                }
            };

            if res.status().is_success() {
                // 4. Parse Response
                let json: serde_json::Value = res.json().await?;
                
                // Extract text from the first candidate — search all parts for a text entry
                // (google_search grounding can insert non-text parts before the actual text)
                let candidate_parts = json.get("candidates")
                    .and_then(|c| c.as_array())
                    .and_then(|c| c.get(0))
                    .and_then(|c| c.get("content"))
                    .and_then(|c| c.get("parts"))
                    .and_then(|c| c.as_array());

                let result_text = candidate_parts
                    .and_then(|parts| {
                        parts.iter().find_map(|p| p.get("text").and_then(|t| t.as_str()))
                    })
                    .ok_or_else(|| anyhow!("Failed to extract text from Gemini response: {:?}", json))?;

                // Parse the inner JSON string returned by Gemini in JSON mode
                let structured_res: serde_json::Value = match serde_json::from_str(result_text) {
                    Ok(json) => json,
                    Err(e) => {
                        // Fallback: try to extract from ```json ... ```
                        if let Some(start) = result_text.find("```json") {
                            let content = &result_text[start + 7..];
                            if let Some(end) = content.find("```") {
                                let json_str = content[..end].trim();
                                match serde_json::from_str(json_str) {
                                    Ok(json) => {
                                        tracing::info!(model = %model, "Successfully extracted JSON from markdown block.");
                                        json
                                    },
                                    Err(e2) => {
                                        tracing::error!(model = %model, error = %e2, "Failed to parse JSON even after markdown extraction.");
                                        return Err(anyhow!("Failed to parse JSON from markdown: {}", e2));
                                    }
                                }
                            } else {
                                return Err(anyhow!("Found ```json but no closing ```"));
                            }
                        } else if let Some(start) = result_text.find('{') {
                             if let Some(end) = result_text.rfind('}') {
                                 let json_str = &result_text[start..end+1];
                                 match serde_json::from_str(json_str) {
                                     Ok(json) => {
                                         tracing::info!(model = %model, "Successfully extracted JSON from raw text braces.");
                                         json
                                     },
                                     Err(e2) => {
                                         tracing::error!(model = %model, error = %e2, "Failed to parse JSON from braces.");
                                         return Err(anyhow!("Failed to parse JSON by braces: {}", e2));
                                     }
                                 }
                             } else {
                                 return Err(anyhow!("Failed to parse structured output and no braces found: {}", e));
                             }
                        } else {
                            tracing::error!(model = %model, error = %e, raw_text = %result_text, "Gemini output is not JSON and no fallback found.");
                            return Err(anyhow!("Failed to parse Gemini structured output: {}. Raw: {}", e, result_text));
                        }
                    }
                };

                metrics.gemini_success_model.with_label_values(&[model]).inc();
                return Ok(structured_res);
            } else if res.status() == reqwest::StatusCode::TOO_MANY_REQUESTS || res.status() == reqwest::StatusCode::SERVICE_UNAVAILABLE {
            let status = res.status();
            if attempt >= max_retries {
                if status == reqwest::StatusCode::TOO_MANY_REQUESTS { metrics.gemini_429_errors.inc(); }
                let error_text = res.text().await.unwrap_or_else(|_| "Unknown error".to_string());
                last_error = anyhow!("Gemini API Error {} (Max retries reached) for {}: {}", status, model, error_text);
                break; // Try next model
            }
            
            tracing::warn!("Gemini API error ({}) for {}. Retrying in {:?}...", status, model, backoff);
            if status == reqwest::StatusCode::TOO_MANY_REQUESTS { metrics.gemini_429_errors.inc(); }
            sleep(backoff).await;
            attempt += 1;
            backoff *= 2;
        } else {
            let status = res.status();
            let error_text = res.text().await.unwrap_or_else(|_| "Unknown error".to_string());
            tracing::warn!("Model '{}' skipped. Status: {}. Error: {}", model, status, error_text);
            last_error = anyhow!("Gemini API Error {} for {}: {}", status, model, error_text);
            break; // Try next model
        }
        }
    }
    
    Err(last_error)
}

/// Streams content from Gemini, allowing for real-time "typing" effect in the UI.
/// This version is optimized for the Chat Assistant.
pub async fn stream_generate_content(
    client: &Client,
    pair: &str,
    recent_data: &str,
    prompt: &str,
    metrics: &crate::state::AppMetrics,
    image_base64: Option<&str>,
    account_info: Option<(f64, f64, f64)>, // (balance, equity, drawdown)
) -> Result<Pin<Box<dyn Stream<Item = Result<String, String>> + Send>>> {
    metrics.llm_requests_total.inc();
    let api_key = std::env::var("GEMINI_API_KEY")
        .map_err(|e| anyhow!("GEMINI_API_KEY environment variable not set: {}", e))?;

    // Using your high-quota specialized model tier

    let mut system_info = String::new();
    if let Some((bal, eq, dd)) = account_info {
        system_info = format!(
            "\n[LIVE TERMINAL STATE - MANDATORY PRIORITY]\n- Current Balance: ${:.2}\n- Current Equity: ${:.2}\n- Live Drawdown: {:.2}%\nNote: These values are fetched directly from the MT5 terminal and represent your actual live standing, ignoring any static default settings.\n",
            bal, eq, dd
        );
    }

    let mut parts = vec![json!({
        "text": format!(
            "{}{}\n\nPAIR: {}\nRECENT DATA:\n{}",
            prompt, system_info, pair, recent_data
        )
    })];

    if let Some(img) = image_base64 {
        parts.push(json!({
            "inline_data": {
                "mime_type": "image/png",
                "data": img
            }
        }));
    }

    let body = json!({
        "contents": [{
            "parts": parts
        }],
        "tools": [{
            "google_search": {}
        }],
        "generationConfig": {
            "temperature": 0.7,
            "topP": 0.95,
            "topK": 40,
            "maxOutputTokens": 1024,
        }
    });

    // Free-tier GA models only for streaming
    let models = [
        "gemini-2.5-flash-lite",
        "gemini-2.5-flash",
        "gemini-2.0-flash",
    ];
    let mut last_res = None;

    for model in models {
        let url = format!("{}/{}:streamGenerateContent?alt=sse", GEMINI_BASE_URL, model);
        
        let res = client
            .post(&url)
            .header("x-goog-api-key", &api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| anyhow!("Failed to start stream: {}", e))?;

        if res.status().is_success() {
            let stream = res.bytes_stream();
            
            let mapped_stream = stream.map(|result| {
                match result {
                    Ok(chunk) => {
                        let text = String::from_utf8_lossy(&chunk).to_string();
                        let mut full_text = String::new();
                        for line in text.lines() {
                            if let Some(data) = line.strip_prefix("data: ") {
                                if let Ok(json) = serde_json::from_str::<serde_json::Value>(data) {
                                    if let Some(text_part) = json.get("candidates")
                                        .and_then(|c| c.as_array())
                                        .and_then(|c| c.get(0))
                                        .and_then(|c| c.get("content"))
                                        .and_then(|c| c.get("parts"))
                                        .and_then(|c| c.as_array())
                                        .and_then(|c| c.get(0))
                                        .and_then(|c| c.get("text"))
                                        .and_then(|c| c.as_str()) {
                                        full_text.push_str(text_part);
                                    }
                                }
                            }
                        }
                        if full_text.is_empty() {
                            Err("Empty chunk".to_string())
                        } else {
                            Ok(full_text)
                        }
                    }
                    Err(e) => Err(format!("Stream error: {}", e))
                }
            })
            .filter(|r| futures_util::future::ready(r.is_ok()));

            return Ok(Box::pin(mapped_stream));
        } else {
            let status = res.status();
            let err_body = res.text().await.unwrap_or_default();
            tracing::warn!("Streaming failed for model {}: {} - {}", model, status, err_body);
            last_res = Some(anyhow!("Gemini Stream Error {}: {}", status, err_body));
        }
    }

    Err(last_res.unwrap_or_else(|| anyhow!("No models available for streaming in your tier")))
}