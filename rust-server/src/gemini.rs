use reqwest::Client;
use serde_json::json;
use anyhow::{anyhow, Result}; // Changed to anyhow::Result for clarity

// Use the v1beta endpoint for the latest models
const GEMINI_API_URL: &str = 
   "https://generativelanguage.googleapis.com/v1/models/gemini-2.5-flash:generateContent";

pub async fn generate_analysis(
    pair: &str,
    recent_data: &str,
    prompt: &str,
) -> Result<String> {
    // 1. Get API Key
    let api_key = std::env::var("GEMINI_API_KEY")
        .map_err(|e| anyhow!("GEMINI_API_KEY environment variable not set: {}", e))?;
    
    let client = Client::new();

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

    // 3. Send Request (Using Bearer Auth and Error Check)
    let res = client
        .post(GEMINI_API_URL)
        .header("x-goog-api-key", api_key) // Security: Use API Key header
        .json(&body)
        .send()
        .await?;

    // Check for non-success status codes and log body for debugging
    if !res.status().is_success() {
        let status = res.status();
        let error_text = res.text().await.unwrap_or_else(|_| "Unknown error".to_string());
        return Err(anyhow!("Gemini API Error {}: {}", status, error_text));
    }

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

    Ok(result_text)
}