use reqwest::Client;
use serde_json::json;
use anyhow::{anyhow, Result}; // Changed to anyhow::Result for clarity
use std::time::Duration;
use tokio::time::sleep;
use xau_scalper_server::metrics::AppMetrics;

// Use the v1beta endpoint for the latest models
const GEMINI_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta/models";

pub async fn generate_analysis(
    client: &Client,
    pair: &str,
    recent_data: &str,
    prompt: &str,
    metrics: &AppMetrics,
) -> Result<serde_json::Value> {
    // 1. Get API Key
    let api_key = std::env::var("GEMINI_API_KEY")
        .map_err(|e| anyhow!("GEMINI_API_KEY environment variable not set: {}", e))?;

    // 2. Build Request Body with JSON Schema for stability
    let body = json!({
        "contents": [{
            "parts": [{
                "text": format!(
                    "{}\n\nPAIR: {}\nRECENT DATA (Includes current_price for grounding):\n{}",
                    prompt, pair, recent_data
                )
            }]
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
        "gemini-1.5-flash-8b",   // High RPD (500+)
        "gemini-2.0-flash-lite", // New Lite model
        "gemini-2.0-flash",      // Standard Flash
        "gemini-1.5-flash"       // Original Flash
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
            last_error = anyhow!("Gemini API Error {} for {}: {}", status, model, error_text);
            break; // Try next model
            }
        }
    }
    
    Err(last_error)
}