use reqwest::Client;
use serde::{Deserialize, Serialize};

#[derive(Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<ChatMessage>,
    response_format: Option<ResponseFormat>,
}

#[derive(Serialize)]
struct ChatMessage {
    role: String,
    content: String,
}

#[derive(Serialize)]
struct ResponseFormat {
    #[serde(rename = "type")]
    format_type: String,
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<ChatChoice>,
}

#[derive(Deserialize)]
struct ChatChoice {
    message: ChatMessageResponse,
}

#[derive(Deserialize)]
struct ChatMessageResponse {
    content: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct RouteData {
    pub title: String,
    pub weather: String,
    pub steps: Vec<RouteStep>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct RouteStep {
    pub location_name: String,
    pub address: Option<String>,
    pub category: String,
    pub tips: String,
    pub rating: Option<f32>,
    pub review_count: Option<String>,
    pub price_range: Option<String>,
    pub description: String,
    pub latitude: f64,
    pub longitude: f64,
    pub source_platform: Option<String>,
    pub thumbnail_url: Option<String>,
    pub video_url: Option<String>,
    pub candidates: Option<Vec<RouteCandidate>>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct RouteCandidate {
    pub name: String,
    pub address: String,
    #[serde(default)]
    pub latitude: f64,
    #[serde(default)]
    pub longitude: f64,
    pub rating: f32,
    pub price_range: Option<String>,
    pub thumbnail_url: Option<String>,
    pub video_url: Option<String>,
}

pub async fn generate_route(prompt: &str, links: &Option<String>) -> Result<RouteData, String> {
    let api_key = std::env::var("SUMOPOD_API_KEY").map_err(|_| "Missing SUMOPOD_API_KEY")?;
    let base_url = std::env::var("SUMOPOD_BASE_URL").map_err(|_| "Missing SUMOPOD_BASE_URL")?;
    let model = std::env::var("SUMOPOD_MODEL_SANTUY").unwrap_or_else(|_| "gemini/gemini-2.5-flash-lite".to_string());

    let client = Client::new();

    let system_prompt = "You are an AI travel assistant. Generate a local itinerary or route based on the user's prompt. 
Respond ONLY in valid JSON format matching this exact structure:
{
  \"title\": \"Catchy title for the route\",
  \"weather\": \"24°C, Cerah Berawan\",
  \"steps\": [
    {
      \"location_name\": \"Name of place\",
      \"address\": \"Full street address\",
      \"category\": \"COFFEE SHOP\",
      \"tips\": \"TIPS KAKAKNYA GEN Z\",
      \"rating\": 4.8,
      \"review_count\": \"2.4k\",
      \"price_range\": \"Rp 25k - 50k\",
      \"description\": \"Brief description...\",
      \"latitude\": -6.88,
      \"longitude\": 107.60,
      \"source_platform\": null,
      \"thumbnail_url\": null,
      \"video_url\": null,
      \"candidates\": [
        {
          \"name\": \"Another place\",
          \"address\": \"Nearby address\",
          \"rating\": 4.5,
          \"price_range\": \"Rp 20k - 40k\",
          \"thumbnail_url\": null,
          \"video_url\": null
        }
      ]
    }
  ]
}
If the user provides social links or harvested data, include the exact `source_platform`, `thumbnail_url`, and `video_url` in the specific route step if matching. 
Provide 2-3 candidates (alternative locations) for each main step. Provide around 3-5 major steps.";

    let user_content = format!("Prompt: {}\nLinks: {}", prompt, links.as_deref().unwrap_or(""));

    let req_body = ChatRequest {
        model,
        messages: vec![
            ChatMessage { role: "system".to_string(), content: system_prompt.to_string() },
            ChatMessage { role: "user".to_string(), content: user_content },
        ],
        // Note: Sumopod (Gemini compatible) might not strictly need JSON object format enforcing, but this is OpenAI standard
        response_format: Some(ResponseFormat { format_type: "json_object".to_string() }),
    };

    let response = client.post(&base_url)
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", "application/json")
        .json(&req_body)
        .send()
        .await
        .map_err(|e| format!("Request failed: {}", e))?;

    if !response.status().is_success() {
        let err_text = response.text().await.unwrap_or_default();
        return Err(format!("API Error: {}", err_text));
    }

    let resp_json: ChatResponse = response.json().await.map_err(|e| format!("Failed to parse API response JSON: {}", e))?;
    let content = resp_json.choices.first().map(|c| c.message.content.clone()).unwrap_or_default();

    // Sometimes AI returns markdown wrapped JSON
    let cleaned_content = content.trim().strip_prefix("```json").unwrap_or(&content).strip_suffix("```").unwrap_or(&content).trim();

    let route_data: RouteData = serde_json::from_str(cleaned_content).map_err(|e| format!("Failed to parse route data JSON: {} | Content: {}", e, cleaned_content))?;

    Ok(route_data)
}

pub async fn generate_caption(vibe: &str, places: &[String]) -> Result<String, String> {
    let api_key = std::env::var("SUMOPOD_API_KEY").map_err(|_| "Missing SUMOPOD_API_KEY")?;
    let base_url = std::env::var("SUMOPOD_BASE_URL").map_err(|_| "Missing SUMOPOD_BASE_URL")?;
    let model = std::env::var("SUMOPOD_MODEL_SANTUY").unwrap_or_else(|_| "gemini/gemini-2.5-flash-lite".to_string());

    let client = Client::new();

    let places_str = places.join(", ");
    let system_prompt = format!(
        "You are an expert social media travel influencer. Generate a viral travel caption in Indonesian for a trip to: {}. 
        The vibe should be: {}.
        Keep it engaging, use relevant emojis, and include 3-5 trending hashtags. Respond ONLY with the caption text.",
        places_str, vibe
    );

    let req_body = ChatRequest {
        model,
        messages: vec![
            ChatMessage { role: "system".to_string(), content: system_prompt },
            ChatMessage { role: "user".to_string(), content: format!("Generate a {} caption for my trip to {}.", vibe, places_str) },
        ],
        response_format: None,
    };

    let response = client.post(&base_url)
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", "application/json")
        .json(&req_body)
        .send()
        .await
        .map_err(|e| format!("Request failed: {}", e))?;

    if !response.status().is_success() {
        let err_text = response.text().await.unwrap_or_default();
        return Err(format!("API Error: {}", err_text));
    }

    let resp_json: ChatResponse = response.json().await.map_err(|e| format!("Failed to parse API response JSON: {}", e))?;
    let content = resp_json.choices.first().map(|c| c.message.content.clone()).unwrap_or_default();

    Ok(content.trim().to_string())
}
