use askama::Template;
use axum::{
    extract::{Form, State, Path, Json},
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::{mayar, sumopod, harvester, db, auth, AppState};

#[derive(Template)]
#[template(path = "index.html")]
pub struct IndexTemplate {
    pub history: Vec<db::RouteHistoryEntry>,
    pub user: Option<auth::GoogleUser>,
}

// Route Template
#[derive(Template)]
#[template(path = "route.html")]
pub struct RouteTemplate {
    pub _id: String,
    pub route: sumopod::RouteData,
    pub current_date: String,
    pub google_maps_api_key: String,
}

#[derive(Deserialize, Debug)]
pub struct GeneratePayload {
    pub prompt: String,
    pub links: Option<String>,
    pub lat: Option<f64>,
    pub lng: Option<f64>,
}

pub async fn root(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let user = auth::get_current_user(&headers);
    let history = db::get_route_history(&state.db, 5).await.unwrap_or_default();
    let template = IndexTemplate { history, user };
    Html(template.render().unwrap())
}

pub async fn generate(
    State(state): State<AppState>,
    Form(payload): Form<GeneratePayload>,
) -> impl IntoResponse {
    tracing::info!("Generate: Prompt='{}', Links={:?}", payload.prompt, payload.links);

    // If social links were provided, harvest them first and enrich the prompt
    let effective_prompt = if let Some(ref links_str) = payload.links {
        let links: Vec<String> = links_str
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect();

        if !links.is_empty() {
            tracing::info!("Generate: Parallel harvesting {} links", links.len());
            
            let harvest_futures = links.iter().map(|link| {
                let state = state.clone();
                let link = link.clone();
                async move {
                    // Check DB cache first
                    let existing = db::find_gem_by_url(&state.db, &link).await;
                    if let Ok(Some(gem)) = existing {
                        if gem.is_place {
                            if let Some(name) = &gem.name {
                                return Some(format!(
                                    "- {} (Category: {}, Location: {}, Lat: {}, Lng: {}){}{}{}",
                                    name,
                                    gem.category.as_deref().unwrap_or("General"),
                                    gem.location.as_deref().unwrap_or("Unknown"),
                                    gem.lat.unwrap_or(0.0),
                                    gem.lng.unwrap_or(0.0),
                                    format!(", Source: {}", gem.platform),
                                    gem.thumbnail_url.as_deref().map_or("".to_string(), |u| format!(", Thumbnail: {}", u)),
                                    gem.source_url.as_str().is_empty().then(|| "".to_string()).unwrap_or_else(|| format!(", Video URL: {}", gem.source_url))
                                ));
                            }
                        }
                        return None;
                    }

                    // Harvest from social media
                    match harvester::harvest_social(&link).await {
                        Ok(result) => {
                            let analysis = &result.analysis;
                            let metadata = &result.metadata;

                            // Save to DB
                            let ai_json = serde_json::to_value(analysis).ok();
                            let _ = db::upsert_gem(
                                &state.db,
                                &link,
                                &metadata.platform,
                                analysis.name.as_deref(),
                                analysis.category.as_deref(),
                                analysis.description.as_deref(),
                                analysis.location.as_deref(),
                                analysis.lat,
                                analysis.lng,
                                metadata.thumbnail.as_deref(),
                                analysis.is_place,
                                analysis.confidence,
                                ai_json.as_ref(),
                            ).await;

                            if analysis.is_place {
                                if let Some(ref name) = analysis.name {
                                    return Some(format!(
                                        "- {} (Category: {}, Location: {}, Lat: {}, Lng: {}){}{}{}",
                                        name,
                                        analysis.category.as_deref().unwrap_or("General"),
                                        analysis.location.as_deref().unwrap_or("Unknown"),
                                        analysis.lat.unwrap_or(0.0),
                                        analysis.lng.unwrap_or(0.0),
                                        format!(", Source: {}", metadata.platform),
                                        metadata.thumbnail.as_deref().map_or("".to_string(), |u| format!(", Thumbnail: {}", u)),
                                        format!(", Video URL: {}", link)
                                    ));
                                }
                            }
                        }
                        Err(e) => {
                            tracing::error!("Generate: Harvest failed for {}: {}", link, e);
                        }
                    }
                    None
                }
            });

            let results = futures::future::join_all(harvest_futures).await;
            let harvested_names: Vec<String> = results.into_iter().flatten().collect();

            if !harvested_names.is_empty() {
                let gem_list = harvested_names.join("\n");
                let p = format!(
                    "Create a travel route that visits ONLY these specific destinations extracted from social media. These are the REQUIRED stops:\n{}\n\nUser's additional request: {}",
                    gem_list,
                    if payload.prompt.trim().is_empty() { "Please create the best route to visit these places." } else { &payload.prompt }
                );
                tracing::info!("Generate: Prompt enriched with {} harvested places", harvested_names.len());
                p
            } else {
                // Harvesting didn't find any places, fall back
                if payload.prompt.trim().is_empty() {
                    "Provide a general travel itinerary for popular local spots in Indonesia.".to_string()
                } else {
                    format!("Extract destinations from these social links and create a route: {}. Prompt: {}", links_str, payload.prompt)
                }
            }
        } else {
            payload.prompt.clone()
        }
    } else {
        payload.prompt.clone()
    };
    // Inject location context if available
    let final_prompt = if let (Some(lat), Some(lng)) = (payload.lat, payload.lng) {
        format!(
            "{}\n\n[CONTEXT: User is currently at Latitude: {}, Longitude: {}. Please ensure Step 0 of the route is their current location.]",
            effective_prompt, lat, lng
        )
    } else {
        effective_prompt
    };

    match sumopod::generate_route(&final_prompt, &None).await {
        Ok(route_data) => {
            let mock_id = Uuid::new_v4();
            let route_json = serde_json::to_value(&route_data).unwrap_or(serde_json::Value::Null);
            
            // Save to DB History
            let _ = db::save_route(
                &state.db,
                mock_id,
                &route_data.title,
                route_data.steps.first().map(|s| s.location_name.as_str()),
                route_data.steps.len() as i32,
                &route_json,
            ).await;

            let mut headers = HeaderMap::new();
            headers.insert("HX-Redirect", format!("/route/{}", mock_id).parse().unwrap());
            (StatusCode::OK, headers, "Redirecting to map...".to_string())
        }
        Err(e) => {
            tracing::error!("AI Generation Error: {}", e);
            let headers = HeaderMap::new();
            (StatusCode::INTERNAL_SERVER_ERROR, headers, format!("Error generating route: {}", e))
        }
    }
}

pub async fn route_handler(
    State(state): State<AppState>,
    _headers: HeaderMap,
    Path(id_str): Path<String>,
) -> impl IntoResponse {
    let id = match Uuid::parse_str(&id_str) {
        Ok(u) => u,
        Err(_) => return (StatusCode::BAD_REQUEST, "Invalid route ID").into_response(),
    };

    if let Ok(Some(route_json)) = db::find_route_by_id(&state.db, id).await {
        if let Ok(route_data) = serde_json::from_value::<sumopod::RouteData>(route_json) {
            let template = RouteTemplate {
                _id: id_str.clone(),
                route: route_data,
                current_date: chrono::Local::now().format("%d %b %Y").to_string(),
                google_maps_api_key: std::env::var("GOOGLE_MAPS_API_KEY").unwrap_or_default(),
            };
            return Html(template.render().unwrap()).into_response();
        }
    }
    
    (StatusCode::NOT_FOUND, "Route not found").into_response()
}

// =============================================
// Social Link Harvest API
// =============================================

#[derive(Deserialize)]
pub struct HarvestRequest {
    pub url: String,
}

#[derive(Serialize)]
pub struct HarvestApiResponse {
    pub success: bool,
    pub data: Option<Value>,
    pub error: Option<String>,
}

pub async fn harvest(
    State(state): State<AppState>,
    Json(payload): Json<HarvestRequest>,
) -> (StatusCode, Json<HarvestApiResponse>) {
    tracing::info!("HARVEST: Processing URL: {}", payload.url);

    let url = payload.url.trim();

    if !url.contains("tiktok.com") && !url.contains("instagram.com") && !url.contains("ig.me") && !url.contains("vm.tiktok.com") {
        return (
            StatusCode::BAD_REQUEST,
            Json(HarvestApiResponse {
                success: false,
                data: None,
                error: Some("URL harus dari TikTok atau Instagram".to_string()),
            }),
        );
    }

    // Check DB cache
    if let Ok(Some(gem)) = db::find_gem_by_url(&state.db, url).await {
        let data = serde_json::json!({
            "source": "cache",
            "is_place": gem.is_place,
            "name": gem.name,
            "category": gem.category,
            "description": gem.description,
            "location": gem.location,
            "lat": gem.lat,
            "lng": gem.lng,
            "thumbnail_url": gem.thumbnail_url,
            "platform": gem.platform,
            "confidence": gem.confidence,
        });
        return (
            StatusCode::OK,
            Json(HarvestApiResponse { success: true, data: Some(data), error: None }),
        );
    }

    match harvester::harvest_social(url).await {
        Ok(result) => {
            let ai_json = serde_json::to_value(&result.analysis).ok();
            let _ = db::upsert_gem(
                &state.db,
                url,
                &result.metadata.platform,
                result.analysis.name.as_deref(),
                result.analysis.category.as_deref(),
                result.analysis.description.as_deref(),
                result.analysis.location.as_deref(),
                result.analysis.lat,
                result.analysis.lng,
                result.metadata.thumbnail.as_deref(),
                result.analysis.is_place,
                result.analysis.confidence,
                ai_json.as_ref(),
            ).await;

            let data = serde_json::json!({
                "source": "fresh",
                "metadata": {
                    "url": result.metadata.url,
                    "title": result.metadata.title,
                    "description": result.metadata.description,
                    "author": result.metadata.author,
                    "thumbnail": result.metadata.thumbnail,
                    "platform": result.metadata.platform,
                },
                "analysis": {
                    "is_place": result.analysis.is_place,
                    "name": result.analysis.name,
                    "category": result.analysis.category,
                    "description": result.analysis.description,
                    "location": result.analysis.location,
                    "lat": result.analysis.lat,
                    "lng": result.analysis.lng,
                    "tags": result.analysis.tags,
                    "vibe": result.analysis.vibe,
                    "confidence": result.analysis.confidence,
                    "rating": result.analysis.rating,
                    "estimated_price_min": result.analysis.estimated_price_min,
                    "estimated_price_max": result.analysis.estimated_price_max,
                }
            });

            tracing::info!("HARVEST: Success for {}", url);
            (StatusCode::OK, Json(HarvestApiResponse { success: true, data: Some(data), error: None }))
        }
        Err(e) => {
            tracing::error!("HARVEST: Error - {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(HarvestApiResponse {
                    success: false,
                    data: None,
                    error: Some(e),
                }),
            )
        }
    }
}

// =============================================
// Payment Logic
// =============================================

#[derive(Deserialize)]
pub struct CheckoutPayload {
    pub tier: String,
    pub amount: f64,
}

pub async fn checkout(Form(payload): Form<CheckoutPayload>) -> impl IntoResponse {
    let customer_name = "Guest User".to_string();
    let customer_email = "guest@ulyn.fun".to_string();
    let invoice_number = format!("SOSMED-{}", payload.tier.to_uppercase());

    match mayar::create_invoice(&customer_name, &customer_email, payload.amount, &invoice_number).await {
        Ok(payment_url) => {
            let mut headers = HeaderMap::new();
            headers.insert("HX-Redirect", payment_url.parse().unwrap());
            (StatusCode::OK, headers, "Redirecting to payment...")
        }
        Err(e) => {
            tracing::error!("Payment error: {}", e);
            let headers = HeaderMap::new();
            (StatusCode::INTERNAL_SERVER_ERROR, headers, "Failed to create payment link")
        }
    }
}

pub async fn payment_callback(body: axum::body::Bytes) -> impl IntoResponse {
    if let Ok(payload_val) = serde_json::from_slice::<Value>(&body) {
        let event = payload_val.get("event").and_then(|e| e.as_str()).unwrap_or("");

        if event == "payment.received" || event == "transaction_status_updated" {
            let data = payload_val.get("data").unwrap_or(&payload_val);
            let status = data.get("status").and_then(|s| s.as_str()).unwrap_or("");

            if status == "SUCCESS" || status == "PAID" {
                tracing::info!("[WEBHOOK] Payment Success received: {:?}", payload_val);
            }
        }
    }
    StatusCode::OK
}

// =============================================
// Social Content Generation
// =============================================

#[derive(Deserialize)]
pub struct CaptionRequest {
    pub vibe: String,
    pub places: Vec<String>,
}

#[derive(Serialize)]
pub struct CaptionResponse {
    pub caption: Option<String>,
    pub error: Option<String>,
}

pub async fn generate_caption_handler(
    Json(payload): Json<CaptionRequest>,
) -> (StatusCode, Json<CaptionResponse>) {
    tracing::info!("CAPTION: Vibe={}, Places={:?}", payload.vibe, payload.places);

    match sumopod::generate_caption(&payload.vibe, &payload.places).await {
        Ok(caption) => {
            (StatusCode::OK, Json(CaptionResponse { caption: Some(caption), error: None }))
        }
        Err(e) => {
            tracing::error!("CAPTION: AI Error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(CaptionResponse {
                    caption: None,
                    error: Some(e),
                }),
            )
        }
    }
}
