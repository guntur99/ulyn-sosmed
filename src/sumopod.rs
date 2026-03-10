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
    pub description: String,
    #[serde(default)]
    pub latitude: f64,
    #[serde(default)]
    pub longitude: f64,
    pub rating: f32,
    pub price_range: Option<String>,
    pub thumbnail_url: Option<String>,
    pub video_url: Option<String>,
}

pub async fn generate_route(client: &Client, prompt: &str, lang: &str, weather_info: &Option<String>) -> Result<RouteData, String> {
    let api_key = std::env::var("SUMOPOD_API_KEY").map_err(|_| "Missing SUMOPOD_API_KEY")?;
    let base_url = std::env::var("SUMOPOD_BASE_URL").map_err(|_| "Missing SUMOPOD_BASE_URL")?;
    let model = std::env::var("SUMOPOD_MODEL_SANTUY").unwrap_or_else(|_| "gemini/gemini-2.5-flash-lite".to_string());

    let weather_context = match weather_info {
        Some(w) => format!("IMPORTANT: Current weather is {}. Use this exact string in the 'weather' field and provide tips based on this weather condition.", w),
        None => "Provide a reasonable weather estimate for the area in the 'weather' field.".to_string(),
    };

    let system_prompt = format!("You are an AI travel assistant. Generate a local itinerary or route based on the user's prompt. 
Respond in the language specified: {}.
{}.
Respond ONLY in valid JSON format matching this exact structure:
{{
  \"title\": \"Catchy title for the route\",
  \"weather\": \"24°C, Cerah Berawan\",
  \"steps\": [
    {{
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
        {{
          \"name\": \"Another place\",
          \"address\": \"Nearby address\",
          \"description\": \"Description for candidate...\",
          \"latitude\": -6.89,
          \"longitude\": 107.61,
          \"rating\": 4.5,
          \"price_range\": \"Rp 20k - 40k\",
          \"thumbnail_url\": null,
          \"video_url\": null
        }}
      ]
    }}
  ]
}}
Provide 2-3 candidates (alternative locations) for each main step. Provide around 3-5 major steps.", lang, weather_context);

    let user_content = format!("Prompt: {}", prompt);

    let req_body = ChatRequest {
        model,
        messages: vec![
            ChatMessage { role: "system".to_string(), content: system_prompt },
            ChatMessage { role: "user".to_string(), content: user_content },
        ],
        response_format: Some(ResponseFormat { format_type: "json_object".to_string() }),
    };

    let response = client.post(&base_url)
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", "application/json")
        .json(&req_body)
        .timeout(std::time::Duration::from_secs(45))
        .send()
        .await
        .map_err(|e| format!("Request failed: {}", e))?;

    if !response.status().is_success() {
        let err_text = response.text().await.unwrap_or_default();
        return Err(format!("API Error: {}", err_text));
    }

    let resp_json: ChatResponse = response.json().await.map_err(|e| format!("Failed to parse API response JSON: {}", e))?;
    let content = resp_json.choices.first().map(|c| c.message.content.clone()).unwrap_or_default();

    let cleaned_content = content.trim().strip_prefix("```json").unwrap_or(&content).strip_suffix("```").unwrap_or(&content).trim();

    let route_data: RouteData = serde_json::from_str(cleaned_content).map_err(|e| format!("Failed to parse route data JSON: {} | Content: {}", e, cleaned_content))?;

    Ok(route_data)
}

pub async fn generate_caption(client: &Client, vibe: &str, places: &[String], lang: &str) -> Result<String, String> {
    let api_key = std::env::var("SUMOPOD_API_KEY").map_err(|_| "Missing SUMOPOD_API_KEY")?;
    let base_url = std::env::var("SUMOPOD_BASE_URL").map_err(|_| "Missing SUMOPOD_BASE_URL")?;
    let model = std::env::var("SUMOPOD_MODEL_SANTUY").unwrap_or_else(|_| "gemini/gemini-2.5-flash-lite".to_string());

    let places_str = places.join(", ");
    let system_prompt = format!(
        "You are an expert social media travel influencer. Generate a viral travel caption in the language: {} for a trip to: {}. 
        The vibe should be: {}.
        Keep it engaging, use relevant emojis, and include 3-5 trending hashtags. Respond ONLY with the caption text.",
        lang, places_str, vibe
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
        .timeout(std::time::Duration::from_secs(30))
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
