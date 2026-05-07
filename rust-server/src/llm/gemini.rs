use reqwest::Client;
use serde_json::json;
use anyhow::{anyhow, Result};
use std::time::Duration;
use tokio::time::sleep;
use xau_scalper_server::metrics::AppMetrics;
use futures_util::{Stream, StreamExt};
use std::pin::Pin;

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
    // 1. Get API Key
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
            "google_search_retrieval": {}
        }],
        "generationConfig": {
            "response_mime_type": "application/json",
            "response_schema": {
                "type": "object",
                "properties": {
                    "bias": { "type": "string", "enum": ["Bullish", "Bearish", "Neutral"], "description": "Market outlook bias." },
                    "confidence": { "type": "number", "description": "Confidence score from 0.0 to 1.0" },
                    "macro_narrative": { "type": "string", "description": "Detailed institutional narrative." },
                    "high_impact_drivers": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "event": { "type": "string" },
                                "how_it_shapes_direction": { "type": "string" },
                                "date": { "type": "string" }
                            },
                            "required": ["event", "how_it_shapes_direction"]
                        }
                    },
                    "risks": { "type": "array", "items": { "type": "string" } },
                    "targets": {
                        "type": "array",
                        "description": "Optional price coordinates for visual grounding (e.g. liquidity zones, FVG gaps).",
                        "items": {
                            "type": "object",
                            "properties": {
                                "price": { "type": "number" },
                                "label": { "type": "string", "description": "Short label (max 10 chars)" }
                            },
                            "required": ["price", "label"]
                        }
                    }
                },
                "required": ["bias", "confidence", "macro_narrative", "high_impact_drivers", "risks"]
            }
        }
    });

    // Fallback models in order of preference
    // Fallback models prioritizing "Flash Lite" variants for higher RPD/RPM on Free Tier
    let models = [
        "gemini-3.1-flash-lite-preview", // 500 RPD High-Quota Primary
        "gemini-3-flash-preview",        // Secondary Flash
        "gemini-2.5-flash-lite",         // Pro-tier Lite
        "gemini-2.5-flash",              // Pro-tier Standard
        "gemini-2.0-flash",              // Legacy (Active until June 2026)
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
                
                let result_text = json["candidates"]
                    .as_array()
                    .and_then(|c| c.get(0))
                    .and_then(|c| c["content"].as_object())
                    .and_then(|c| c["parts"].as_array())
                    .and_then(|c| c.get(0))
                    .and_then(|c| c["text"].as_str())
                    .ok_or_else(|| anyhow!("Failed to extract text from Gemini response: {:?}", json))?;

                // Parse the inner JSON string returned by Gemini in JSON mode
                let structured_res: serde_json::Value = serde_json::from_str(result_text)
                    .map_err(|e| anyhow!("Failed to parse Gemini structured output: {}. Raw: {}", e, result_text))?;

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
    image_base64: Option<&str>,
    account_info: Option<(f64, f64, f64)>, // (balance, equity, drawdown)
) -> Result<Pin<Box<dyn Stream<Item = Result<String, String>> + Send>>> {
    let api_key = std::env::var("GEMINI_API_KEY")
        .map_err(|e| anyhow!("GEMINI_API_KEY environment variable not set: {}", e))?;

    // We use a high-performance vision-capable model
    let model = "gemini-1.5-flash"; 
    let url = format!("{}/{}:streamGenerateContent?alt=sse", GEMINI_BASE_URL, model);

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
            "google_search_retrieval": {}
        }],
        "generationConfig": {
            "temperature": 0.7,
            "topP": 0.95,
            "topK": 40,
            "maxOutputTokens": 1024,
        }
    });

    let res = client
        .post(&url)
        .header("x-goog-api-key", &api_key)
        .json(&body)
        .send()
        .await
        .map_err(|e| anyhow!("Failed to start stream: {}", e))?;

    if !res.status().is_success() {
        let status = res.status();
        let err = res.text().await.unwrap_or_default();
        return Err(anyhow!("Gemini Stream Error {}: {}", status, err));
    }

    let stream = res.bytes_stream();
    
    // Map the byte stream to text chunks
    let mapped_stream = stream.map(|result| {
        match result {
            Ok(chunk) => {
                let text = String::from_utf8_lossy(&chunk).to_string();
                // SSE format: data: {"candidates": [...]}
                // We need to extract the text from each event
                let mut full_text = String::new();
                for line in text.lines() {
                    if let Some(data) = line.strip_prefix("data: ") {
                        if let Ok(json) = serde_json::from_str::<serde_json::Value>(data) {
                            if let Some(text_part) = json["candidates"]
                                .as_array()
                                .and_then(|c| c.get(0))
                                .and_then(|c| c["content"].as_object())
                                .and_then(|c| c["parts"].as_array())
                                .and_then(|c| c.get(0))
                                .and_then(|c| c["text"].as_str()) {
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
    .filter(|r| futures_util::future::ready(r.is_ok())); // Filter out empty/error chunks for now

    Ok(Box::pin(mapped_stream))
}