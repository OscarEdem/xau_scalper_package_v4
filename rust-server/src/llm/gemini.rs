use reqwest::Client;
use serde_json::json;
use anyhow::{anyhow, Result}; // Changed to anyhow::Result for clarity
use std::time::Duration;
use tokio::time::sleep;
use xau_scalper_server::metrics::AppMetrics;

// Use the v1beta endpoint for the latest models
const GEMINI_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1/models";

pub async fn generate_analysis(
    client: &Client,
    pair: &str,
    recent_data: &str,
    prompt: &str,
    metrics: &AppMetrics,
) -> Result<String> {
    // 1. Get API Key
    let api_key = std::env::var("GEMINI_API_KEY")
        .map_err(|e| anyhow!("GEMINI_API_KEY environment variable not set: {}", e))?;

    // 2. Build Request Body
    let body = json!({
        "contents": [{
            "parts": [{
                "text": format!(
                    "{}\n\nPAIR: {}\nRECENT DATA:\n{}",
                    prompt, pair, recent_data
                )
            }]
        }]
    });

    // Fallback models in order of preference
    let models = ["gemini-2.5-flash", "gemini-2.5-flash-lite", "gemini-3-flash"];
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
                .header("x-goog-api-key", &api_key) // Security: Use API Key header
                .json(&body)
                .send()
                .await;

            let res = match res_result {
                Ok(r) => r,
                Err(e) => {
                    last_error = anyhow!("Request failed for {}: {}", model, e);
                    if attempt >= max_retries { break; } // Try next model
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
                
                // Robustly extract the text from the JSON structure
                let result_text = json["candidates"]
                    .as_array()
                    .and_then(|c| c.get(0))
                    .and_then(|c| c["content"].as_object())
                    .and_then(|c| c["parts"].as_array())
                    .and_then(|c| c.get(0))
                    .and_then(|c| c["text"].as_str())
                    .map(|s| s.to_string())
                    .ok_or_else(|| anyhow!("Failed to parse text from Gemini response: {:?}", json))?;

                metrics.gemini_success_model.with_label_values(&[model]).inc();
                return Ok(result_text);
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