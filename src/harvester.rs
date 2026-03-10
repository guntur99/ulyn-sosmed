use reqwest::Client;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone, sqlx::FromRow)]
pub struct HarvestedGem {
    pub id: uuid::Uuid,
    pub source_url: String,
    pub platform: String,
    pub name: Option<String>,
    pub category: Option<String>,
    pub description: Option<String>,
    pub location: Option<String>,
    pub lat: Option<f64>,
    pub lng: Option<f64>,
    pub thumbnail_url: Option<String>,
    pub is_place: bool,
    pub confidence: f64,
    pub ai_analysis: Option<serde_json::Value>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SocialMetadata {
    pub url: String,
    pub title: String,
    pub description: String,
    pub author: Option<String>,
    pub thumbnail: Option<String>,
    pub video_url: Option<String>,
    pub platform: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HarvesterAnalysis {
    pub is_place: bool,
    pub name: Option<String>,
    pub category: Option<String>,
    pub description: Option<String>,
    pub location: Option<String>,
    pub lat: Option<f64>,
    pub lng: Option<f64>,
    pub tags: Option<Vec<String>>,
    pub estimated_price_min: Option<i64>,
    pub estimated_price_max: Option<i64>,
    pub vibe: Option<String>,
    pub best_time: Option<String>,
    pub rating: Option<f64>,
    pub confidence: f64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HarvestResult {
    pub metadata: SocialMetadata,
    pub analysis: HarvesterAnalysis,
}

/// OEmbed response from TikTok
#[derive(Debug, Deserialize)]
struct TikTokOEmbed {
    title: Option<String>,
    author_name: Option<String>,
    author_unique_id: Option<String>,
    thumbnail_url: Option<String>,
}

/// Fetch metadata from TikTok using the official OEmbed API
pub async fn fetch_tiktok_metadata(client: &Client, url: &str) -> Result<SocialMetadata, String> {
    tracing::info!("HARVESTER: Fetching TikTok OEmbed metadata from {}", url);

    let oembed_url = format!("https://www.tiktok.com/oembed?url={}", urlencoding::encode(url));

    let response = client.get(&oembed_url)
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| format!("OEmbed Fetch Error: {}", e))?;

    if !response.status().is_success() {
        return Err(format!("OEmbed HTTP Error: {}", response.status()));
    }

    let oembed: TikTokOEmbed = response.json().await
        .map_err(|e| format!("OEmbed Parse Error: {}", e))?;

    let title = oembed.title.unwrap_or_default();
    let author = oembed.author_unique_id
        .map(|id| format!("@{}", id))
        .or(oembed.author_name);

    let metadata = SocialMetadata {
        url: url.to_string(),
        title: title.clone(),
        description: title,
        author,
        thumbnail: oembed.thumbnail_url,
        video_url: Some(url.to_string()),
        platform: "tiktok".to_string(),
    };

    Ok(metadata)
}

/// Fetch metadata from Instagram using a simple scraper fallback
pub async fn fetch_instagram_metadata(client: &Client, url: &str) -> Result<SocialMetadata, String> {
    tracing::info!("HARVESTER: Fetching Instagram metadata from {}", url);

    let response = client.get(url)
        .header("User-Agent", "facebookexternalhit/1.1")
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| format!("Instagram Fetch Error: {}", e))?;

    let html = response.text().await.map_err(|e| format!("Text Parse Error: {}", e))?;
    let document = scraper::Html::parse_document(&html);

    let og_title = scraper::Selector::parse("meta[property='og:title']").unwrap();
    let og_description = scraper::Selector::parse("meta[property='og:description']").unwrap();
    let og_image = scraper::Selector::parse("meta[property='og:image']").unwrap();
    let meta_description = scraper::Selector::parse("meta[name='description']").unwrap();

    let title = document.select(&og_title)
        .filter_map(|el| el.value().attr("content"))
        .next()
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            document.select(&scraper::Selector::parse("title").unwrap())
                .next()
                .map(|el| el.inner_html())
                .unwrap_or_else(|| "Instagram Post".to_string())
        });

    let description = document.select(&og_description)
        .filter_map(|el| el.value().attr("content"))
        .next()
        .or_else(|| document.select(&meta_description).filter_map(|el| el.value().attr("content")).next())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "".to_string());

    let thumbnail = document.select(&og_image)
        .filter_map(|el| el.value().attr("content"))
        .next()
        .map(|s| s.to_string());

    let mut final_title = title;
    let mut final_description = description;

    if final_title == "Instagram" || final_title == "Login • Instagram" {
        final_title = "Instagram Post (Protected)".to_string();
    }

    if final_description.contains("Welcome back to Instagram") || final_description.contains("Sign in to check out") {
        final_description = "".to_string();
    }

    Ok(SocialMetadata {
        url: url.to_string(),
        title: final_title,
        description: final_description,
        author: None,
        thumbnail,
        video_url: Some(url.to_string()),
        platform: "instagram".to_string(),
    })
}

/// Analyze social content using Sumopod AI to determine if it's a hidden gem
pub async fn analyze_content(client: &Client, metadata: &SocialMetadata) -> Result<HarvesterAnalysis, String> {
    tracing::info!("HARVESTER: Analyzing content from {} ({})", metadata.url, metadata.platform);

    let api_key = std::env::var("SUMOPOD_API_KEY").map_err(|_| "Missing SUMOPOD_API_KEY".to_string())?;
    let base_url = std::env::var("SUMOPOD_BASE_URL").map_err(|_| "Missing SUMOPOD_BASE_URL".to_string())?;
    let model = std::env::var("SUMOPOD_MODEL_SANTUY").unwrap_or_else(|_| "gemini/gemini-2.5-flash-lite".to_string());

    let prompt = format!(
        r#"Kamu adalah AI analis pariwisata global. Tugasmu menganalisis konten {} dan menentukan apakah video ini tentang sebuah tempat wisata / hidden gem / kuliner yang bisa dikunjungi.

KONTEN {}:
- Judul/Caption: {}
- Deskripsi: {}
- Author: {}

TUGAS:
1. Tentukan apakah ini konten tentang sebuah TEMPAT (cafe, wisata alam, kuliner, dll).
2. Jika YA, ekstrak informasi berikut.
3. Estimasi "Rating" (1.0 - 4.8) berdasarkan antusiasme dan sentimen di caption.
4. Jika TIDAK, set is_place = false.

RESPONS WAJIB DALAM JSON:
{{
  "is_place": true/false,
  "rating": 4.8,
  "name": "Nama tempat",
  "category": "culinary/nature/activity/cafe/beach/mountain/waterfall/temple",
  "description": "Deskripsi singkat",
  "location": "Kota, Provinsi",
  "lat": null atau angka,
  "lng": null atau angka,
  "tags": ["tag1", "tag2"],
  "estimated_price_min": angka,
  "estimated_price_max": angka,
  "vibe": "aesthetic",
  "best_time": "Siang",
  "confidence": 0.95
}}"#,
        metadata.platform.to_uppercase(),
        metadata.platform.to_uppercase(),
        metadata.title,
        metadata.description,
        metadata.author.as_deref().unwrap_or("Unknown")
    );

    let req_body = serde_json::json!({
        "model": model,
        "messages": [
            {"role": "system", "content": "You are an AI that analyzes social media content to identify travel destinations. Always respond with valid JSON only."},
            {"role": "user", "content": prompt}
        ],
        "response_format": {"type": "json_object"}
    });

    let response = client.post(&base_url)
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", "application/json")
        .json(&req_body)
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| format!("AI request failed: {}", e))?;

    if !response.status().is_success() {
        return Err(format!("AI API error: {}", response.status()));
    }

    let resp_json: serde_json::Value = response.json().await.map_err(|e| format!("Failed to parse AI response: {}", e))?;
    let content = resp_json["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("{}");

    let cleaned = content.trim()
        .strip_prefix("```json").unwrap_or(content)
        .strip_suffix("```").unwrap_or(content)
        .trim();

    let analysis: HarvesterAnalysis = serde_json::from_str(cleaned)
        .map_err(|e| format!("Failed to parse analysis JSON: {} | Content: {}", e, cleaned))?;

    tracing::info!("HARVESTER: AI analysis complete: is_place={}", analysis.is_place);
    Ok(analysis)
}

/// Full harvest pipeline: fetch metadata + analyze
pub async fn harvest_social(client: &Client, url: &str) -> Result<HarvestResult, String> {
    let mut metadata = if url.contains("tiktok.com") || url.contains("vm.tiktok.com") {
        fetch_tiktok_metadata(client, url).await?
    } else if url.contains("instagram.com") || url.contains("ig.me") {
        fetch_instagram_metadata(client, url).await?
    } else {
        return Err("Platform tidak didukung. Gunakan link TikTok atau Instagram.".to_string());
    };

    // Parallelize AI analysis and thumbnail processing
    let metadata_clone = metadata.clone();
    let client_clone = client.clone();
    
    let analysis_future = analyze_content(client, &metadata);
    
    // Thumbnail job (optional, can fail silently or log)
    let thumbnail_future = async move {
        if let Some(thumb_url) = &metadata_clone.thumbnail {
            tracing::info!("HARVESTER: Downloading thumbnail from {}", thumb_url);
            if let Ok(resp) = client_clone.get(thumb_url).timeout(std::time::Duration::from_secs(10)).send().await {
                let content_type = resp.headers().get("content-type")
                    .and_then(|h| h.to_str().ok())
                    .unwrap_or("image/jpeg")
                    .to_string();
                
                if let Ok(bytes) = resp.bytes().await {
                    let bytes_vec = bytes.to_vec();
                    let file_ext = if content_type.contains("png") { "png" } else { "jpg" };
                    let filename = format!("harvester/thumb_{}_{}.{}", metadata_clone.platform, uuid::Uuid::new_v4(), file_ext);
                    
                    match crate::storage::upload_to_spaces(&filename, bytes_vec, &content_type).await {
                        Ok(s3_path) => {
                            let cdn_url = format!("https://ekaputratour.sgp1.digitaloceanspaces.com/{}", s3_path);
                            tracing::info!("HARVESTER: Uploaded thumbnail to S3: {}", cdn_url);
                            return Some(cdn_url);
                        }
                        Err(e) => {
                            tracing::error!("HARVESTER: Failed to upload thumbnail to S3: {}", e);
                        }
                    }
                }
            }
        }
        None
    };

    let (analysis_res, thumbnail_res) = tokio::join!(analysis_future, thumbnail_future);
    
    let analysis = analysis_res?;
    if let Some(new_thumb) = thumbnail_res {
        metadata.thumbnail = Some(new_thumb);
    }

    Ok(HarvestResult { metadata, analysis })
}
