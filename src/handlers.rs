use askama::Template;
use axum::{
    extract::{Form, State, Path, Json},
    http::{header, HeaderMap, StatusCode},
    response::{Html, IntoResponse},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::{mayar, sumopod, harvester, db, auth, mail, email_templates, weather, AppState};

#[derive(Template)]
#[template(path = "index.html")]
pub struct IndexTemplate {
    pub history: Vec<db::RouteHistoryEntry>,
    pub user: Option<db::User>,
    pub quota: db::QuotaStatus,
}

#[derive(Deserialize)]
pub struct ConsumeQuotaRequest {
    pub feature: String,
}

#[derive(Serialize)]
pub struct QuotaResponse {
    pub success: bool,
    pub status: Option<db::QuotaStatus>,
    pub error: Option<String>,
}

// Route Template
#[derive(Template)]
#[template(path = "route.html")]
pub struct RouteTemplate {
    pub _id: String,
    pub route: sumopod::RouteData,
    pub route_json: String,
    pub current_date: String,
    pub google_maps_api_key: String,
}

#[derive(Deserialize, Debug)]
pub struct GeneratePayload {
    pub prompt: String,
    pub vibe: Option<String>,
    pub vibe_trait: Option<String>,
    pub links: Option<String>,
    pub lat: Option<f64>,
    pub lng: Option<f64>,
    pub lang: Option<String>,
}

/// Helper to get or create a guest session ID from cookies
fn get_or_create_guest_id(headers: &HeaderMap) -> (String, Option<String>) {
    if let Some(cookie_header) = headers.get(header::COOKIE).and_then(|h| h.to_str().ok()) {
        for part in cookie_header.split(';') {
            let part = part.trim();
            if let Some(val) = part.strip_prefix("ulyn_guest_id=") {
                return (format!("guest_{}", val), None);
            }
        }
    }
    
    let new_id = Uuid::new_v4().to_string();
    let cookie = format!("ulyn_guest_id={}; Path=/; HttpOnly; SameSite=Lax; Max-Age=31536000", new_id);
    (format!("guest_{}", new_id), Some(cookie))
}

pub async fn root(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let auth_user = auth::get_current_user(&headers);
    
    let (db_user, user_id, set_cookie) = if let Some(ref u) = auth_user {
        let db_u = db::find_user_by_email(&state.db, &u.email).await.unwrap_or(None);
        (db_u, u.email.clone(), None)
    } else {
        let (gid, cookie) = get_or_create_guest_id(&headers);
        (None, gid, cookie)
    };

    let history = db::get_route_history(&state.db, &user_id, 5).await.unwrap_or_default();
    
    let (db_user_ref, guest_id_ref) = if db_user.is_some() {
        (db_user.as_ref(), None)
    } else {
        (None, Some(user_id.as_str()))
    };

    let quota = db::get_quota_status(&state.db, &state.redis, db_user_ref, guest_id_ref).await
        .unwrap_or(db::QuotaStatus {
            route_used: 0, route_limit: 3,
            caption_used: 0, caption_limit: 3,
            receipt_used: 0, receipt_limit: 3,
        });

    let template = IndexTemplate { history, user: db_user, quota };
    
    let mut response = Html(template.render().unwrap()).into_response();
    if let Some(cookie) = set_cookie {
        response.headers_mut().insert(header::SET_COOKIE, cookie.parse().unwrap());
    }
    response
}

pub async fn generate(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(payload): Form<GeneratePayload>,
) -> impl IntoResponse {
    tracing::info!("Generate: Prompt='{}', Links={:?}", payload.prompt, payload.links);

    // Get user info for quota
    let auth_user = auth::get_current_user(&headers);
    let db_user = if let Some(ref au) = auth_user {
        db::find_user_by_email(&state.db, &au.email).await.unwrap_or(None)
    } else {
        None
    };

    let guest_id = if db_user.is_none() {
        Some(get_or_create_guest_id(&headers).0)
    } else {
        None
    };

    // Check Quota (but don't consume yet)
    match db::check_quota(&state.db, &state.redis, db_user.as_ref(), guest_id.as_deref(), db::FeatureType::Route).await {
        Ok(true) => (),
        Ok(false) => return (StatusCode::FORBIDDEN, HeaderMap::new(), "Kuota harian habis. Silahkan topup atau kembali besok.").into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, HeaderMap::new(), format!("Quota check failed: {}", e)).into_response(),
    }

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
                    match harvester::harvest_social(&state.client, &link).await {
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

    // Inject vibe/mood if available
    let vibe_context = match (&payload.vibe, &payload.vibe_trait) {
        (Some(v), Some(t)) if v == "Custom" && !t.is_empty() => format!("\n[VIBE: {}]", t),
        (Some(v), _) if !v.is_empty() => format!("\n[VIBE: {}]", v),
        _ => "".to_string(),
    };

    let effective_prompt = format!("{}{}", effective_prompt, vibe_context);

    let final_prompt = if let (Some(lat), Some(lng)) = (payload.lat, payload.lng) {
        format!(
            "{}\n\n[CONTEXT: User is currently at Latitude: {}, Longitude: {}. Use this as the starting point for navigation, but do NOT include a dedicated 'Starting Point' or 'Titik Lokasi' step in the steps list, as the UI already displays Step 0 for the user's location.]",
            effective_prompt, lat, lng
        )
    } else {
        effective_prompt
    };

    let lang = payload.lang.clone().unwrap_or_else(|| "id".to_string());

    // Fetch Weather if coordinates are provided
    let weather_info = if let (Some(lat), Some(lng)) = (payload.lat, payload.lng) {
        match weather::get_weather(&state.client, lat, lng, &lang).await {
            Ok(w) => Some(w),
            Err(e) => {
                tracing::warn!("Failed to fetch real-time weather: {}", e);
                None
            }
        }
    } else {
        None
    };

    match sumopod::generate_route(&state.client, &final_prompt, &lang, &weather_info).await {
        Ok(route_data) => {
            let mock_id = Uuid::new_v4();
            let route_json = serde_json::to_value(&route_data).unwrap_or(serde_json::Value::Null);
            
            // Get user_id for history
            let auth_user = auth::get_current_user(&headers);
            let user_id = if let Some(u) = auth_user {
                u.email
            } else {
                get_or_create_guest_id(&headers).0
            };

            // Save to DB History
            let _ = db::save_route(
                &state.db,
                mock_id,
                &user_id,
                &route_data.title,
                route_data.steps.first().map(|s| s.location_name.as_str()),
                route_data.steps.len() as i32,
                &route_json,
            ).await;

            // SUCCESS: Consume Quota now
            let _ = db::consume_quota(&state.db, &state.redis, db_user.as_ref(), guest_id.as_deref(), db::FeatureType::Route).await;

            let mut response_headers = HeaderMap::new();
            if headers.contains_key("HX-Request") {
                response_headers.insert("HX-Redirect", format!("/route/{}", mock_id).parse().unwrap());
            } else {
                response_headers.insert(header::LOCATION, format!("/route/{}", mock_id).parse().unwrap());
                return (StatusCode::SEE_OTHER, response_headers, "Redirecting to map...".to_string()).into_response();
            }
            (StatusCode::OK, response_headers, "Redirecting to map...".to_string()).into_response()
        }
        Err(e) => {
            tracing::error!("AI Generation Error: {}", e);
            let headers = HeaderMap::new();
            (StatusCode::INTERNAL_SERVER_ERROR, headers, format!("Error generating route: {}", e)).into_response()
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
            let route_json_str = serde_json::to_string(&route_data).unwrap_or_else(|_| "{}".to_string());
            let template = RouteTemplate {
                _id: id_str.clone(),
                route: route_data,
                route_json: route_json_str,
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

    match harvester::harvest_social(&state.client, url).await {
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
    pub lang: Option<String>,
}

pub async fn checkout(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(payload): Form<CheckoutPayload>,
) -> impl IntoResponse {
    let auth_user = auth::get_current_user(&headers);
    let user = if let Some(ref au) = auth_user {
        db::find_user_by_email(&state.db, &au.email).await.unwrap_or(None)
    } else {
        None
    };

    if user.is_none() {
        let mut headers = HeaderMap::new();
        headers.insert("HX-Redirect", "/auth/google".parse().unwrap());
        return (StatusCode::OK, headers, "Redirecting to login...").into_response();
    }

    let u = user.unwrap();
    let customer_name = u.name.clone();
    let customer_email = u.email.clone();

    // --- New path: route through the central payment service (ulyn-pay) when
    // ULYN_PAY_URL is configured. ulyn-pay owns the ledger and reference; the
    // topup row here is created later by /internal/payments/fulfill, so we do
    // NOT pre-create one. Flip the env to cut over; unset = legacy Mayar path.
    let pay_url = std::env::var("ULYN_PAY_URL").unwrap_or_default();
    if !pay_url.trim().is_empty() {
        let slug = std::env::var("PAY_TENANT_SLUG").unwrap_or_else(|_| "pro".to_string());
        let secret = std::env::var("PAY_FULFILL_SECRET").unwrap_or_default();
        let redirect_url = std::env::var("APP_BASE_URL").unwrap_or_else(|_| "https://ulyn.pro".to_string());
        let lang = payload.lang.clone().unwrap_or_else(|| "id".to_string());

        let body = serde_json::json!({
            "external_user_id": u.id.to_string(),
            "amount": payload.amount as i64,
            "customer_name": customer_name,
            "customer_email": customer_email,
            "metadata": { "tier": payload.tier, "lang": lang },
            "redirect_url": redirect_url,
        });

        tracing::info!(
            "ulyn-pay request: url={}/payments/create slug={} amount={} user={}",
            pay_url.trim_end_matches('/'), slug, payload.amount as i64, customer_email
        );

        let resp = state.client
            .post(format!("{}/payments/create", pay_url.trim_end_matches('/')))
            .header("X-Tenant-Id", &slug)
            .header("X-Internal-Secret", &secret)
            .json(&body)
            .send()
            .await;

        return match resp {
            Ok(r) if r.status().is_success() => {
                let data: Value = r.json().await.unwrap_or_default();
                // payment_url can be either a string or null in the response
                let link = data.get("payment_url")
                    .and_then(|l| l.as_str())
                    .filter(|s| !s.is_empty());
                match link {
                    Some(url) => {
                        let mut headers = HeaderMap::new();
                        headers.insert("HX-Redirect", url.parse().unwrap());
                        (StatusCode::OK, headers, "Redirecting to payment...").into_response()
                    }
                    None => {
                        tracing::error!("ulyn-pay create: no payment_url in response: {:?}", data);
                        (StatusCode::INTERNAL_SERVER_ERROR, HeaderMap::new(),
                            format!("Gagal membuat link pembayaran (payment_url kosong): {:?}", data)
                        ).into_response()
                    }
                }
            }
            Ok(r) => {
                let st = r.status();
                let txt = r.text().await.unwrap_or_default();
                tracing::error!("ulyn-pay create failed ({}): {}", st, txt);
                (StatusCode::INTERNAL_SERVER_ERROR, HeaderMap::new(),
                    format!("Gagal membuat link pembayaran ({st}): {txt}")
                ).into_response()
            }
            Err(e) => {
                tracing::error!("ulyn-pay create request error: {}", e);
                (StatusCode::INTERNAL_SERVER_ERROR, HeaderMap::new(),
                    format!("Gateway pembayaran tidak terjangkau: {e}")
                ).into_response()
            }
        };
    }

    // --- Legacy path: create the invoice directly via Mayar. ---
    // Generate a UNIQUE invoice number to prevent duplicate key errors
    let unique_suffix = uuid::Uuid::new_v4().to_string()[..4].to_uppercase();
    let invoice_number = format!("ULYN-{}-{}-{}", 
        &u.id.to_string()[..4].to_uppercase(), 
        payload.tier.to_uppercase(),
        unique_suffix
    );

    // Record the topup attempt in database with language preference
    let payload_json = serde_json::json!({
        "lang": payload.lang.unwrap_or_else(|| "id".to_string())
    });
    
    if let Err(e) = db::create_topup(&state.db, u.id, payload.amount, &payload.tier, &invoice_number, Some(&payload_json)).await {
        tracing::error!("Failed to record topup intent: {}", e);
        let headers = HeaderMap::new();
        return (StatusCode::INTERNAL_SERVER_ERROR, headers, "Gagal memproses permintaan pembayaran").into_response();
    }

    let redirect_url = std::env::var("APP_BASE_URL").unwrap_or_else(|_| "https://ulyn.pro".to_string());
    match mayar::create_invoice(&state.client, &customer_name, &customer_email, payload.amount, &invoice_number, &redirect_url).await {
        Ok(payment_url) => {
            let mut headers = HeaderMap::new();
            headers.insert("HX-Redirect", payment_url.parse().unwrap());
            (StatusCode::OK, headers, "Redirecting to payment...").into_response()
        }
        Err(e) => {
            tracing::error!("Payment error: {}", e);
            let headers = HeaderMap::new();
            (StatusCode::INTERNAL_SERVER_ERROR, headers, "Failed to create payment link").into_response()
        }
    }
}

pub async fn payment_callback(
    State(state): State<AppState>,
    body: axum::body::Bytes
) -> impl IntoResponse {
    if let Ok(payload_val) = serde_json::from_slice::<Value>(&body) {
        tracing::info!("[WEBHOOK] Received payload: {}", payload_val);
        let event = payload_val.get("event").and_then(|e| e.as_str()).unwrap_or("");

        if event == "payment.received" || event == "transaction_status_updated" {
            let data = payload_val.get("data").unwrap_or(&payload_val);
            let status = data.get("status").and_then(|s| s.as_str()).unwrap_or("");
            let payment_channel = data.get("payment_channel").and_then(|s| s.as_str());
            
            // The following lines were part of the instruction but refer to variables (db_user, guest_id)
            // not present in this function's scope. They have been omitted to maintain syntactic correctness.
            // // SUCCESS: Consume Quota now
            // let _ = db::consume_quota(&state.db, &state.redis, db_user.as_ref(), guest_id.as_deref()).await;
            
            // Return Redirect to route page or return HTML partial?
            let mut reference = data.get("reference").and_then(|s| s.as_str()).unwrap_or("").to_string();
            if reference.is_empty() {
                if let Some(desc) = data.get("productDescription").and_then(|s| s.as_str()) {
                    // Strip "Purchase " prefix if present
                    if desc.starts_with("Purchase ") {
                        reference = desc.replacen("Purchase ", "", 1).trim().to_string();
                    } else {
                        reference = desc.trim().to_string();
                    }
                }
            }

            if status == "SUCCESS" || status == "PAID" {
                tracing::info!("[WEBHOOK] Processing success for ref: '{}'", reference);
                
                if reference.is_empty() {
                    tracing::warn!("Webhook: SUCCESS/PAID received but reference is missing in payload");
                    return StatusCode::BAD_REQUEST.into_response();
                }
                
                let mut tx = match state.db.begin().await {
                    Ok(t) => t,
                    Err(e) => {
                        tracing::error!("Webhook error: Failed to start transaction: {}", e);
                        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                    }
                };

                // 1. Fetch Topup
                let topup = match db::find_topup_by_reference(&state.db, &reference).await {
                    Ok(Some(t)) => t,
                    Ok(None) => {
                        tracing::warn!("Webhook: Topup ref {} not found", reference);
                        return StatusCode::NOT_FOUND.into_response();
                    }
                    Err(e) => {
                        tracing::error!("Webhook: DB error fetching topup: {}", e);
                        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                    }
                };

                if topup.status == "success" {
                    tracing::info!("Webhook: Topup {} already processed", reference);
                    return StatusCode::OK.into_response();
                }

                // EXTRACT LANG BEFORE OVERWRITING PAYLOAD
                let user_lang = topup.payload.as_ref()
                    .and_then(|p| p.get("lang"))
                    .and_then(|l| l.as_str())
                    .map(|s| s.to_string());

                // Fetch User for email
                let user = match db::find_user_by_id(&state.db, topup.user_id).await {
                    Ok(Some(u)) => {
                        tracing::info!("Webhook: Found user {} (ID: {}) for this topup", u.email, u.id);
                        Some(u)
                    },
                    Ok(None) => {
                        tracing::warn!("Webhook: User ID {} not found in database!", topup.user_id);
                        None
                    },
                    Err(e) => {
                        tracing::error!("Webhook: DB Error fetching user {}: {}", topup.user_id, e);
                        None
                    }
                };

                // 2. Determine Credits (Case-insensitive matching)
                let tier_normalized = topup.tier.to_lowercase();
                let credits_to_add = match tier_normalized.as_str() {
                    "pro_pass" => 60,
                    "basic_pass" => 10,
                    _ => {
                        tracing::warn!("Webhook: Unknown tier detected: '{}'", topup.tier);
                        0
                    }
                };

                tracing::info!("Webhook: Processing tier: '{}', adding {} credits", tier_normalized, credits_to_add);

                // 3. Update User
                // Logic: Only update tier to the new one if the current tier is NOT 'pro_pass'
                // Increment all three feature credit buckets AND legacy credits column.
                tracing::info!("Webhook: Updating credits for user {}", topup.user_id);
                let user_update = sqlx::query(
                    "UPDATE users SET 
                        credits = credits + $1,
                        credits_route = credits_route + $1, 
                        credits_caption = credits_caption + $1, 
                        credits_receipt = credits_receipt + $1, 
                        tier = CASE 
                            WHEN tier = 'pro_pass' THEN 'pro_pass' 
                            ELSE $2 
                        END, 
                        updated_at = NOW() 
                    WHERE id = $3"
                )
                .bind(credits_to_add)
                .bind(&tier_normalized)
                .bind(topup.user_id)
                .execute(&mut *tx)
                .await;

                if let Err(e) = user_update {
                    let _ = tx.rollback().await;
                    tracing::error!("Webhook: Failed to update user credits: {}", e);
                    return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                }

                // 4. Update Topup Status
                let topup_update = sqlx::query(
                    "UPDATE topups SET status = 'success', payload = $1, updated_at = NOW() WHERE id = $2"
                )
                .bind(&payload_val)
                .bind(topup.id)
                .execute(&mut *tx)
                .await;

                if let Err(e) = topup_update {
                    let _ = tx.rollback().await;
                    tracing::error!("Webhook: Failed to update topup status: {}", e);
                    return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                }

                if let Err(e) = tx.commit().await {
                    tracing::error!("Webhook: Failed to commit transaction: {}", e);
                    return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                }

                tracing::info!("Webhook SUCCESS: Added {} credits to user {}", credits_to_add, topup.user_id);

                // 5. Send Email Notification
                if let Some(u) = user {
                    tracing::info!("Webhook: Sending success email to {}", u.email);
                    let email_body = email_templates::get_topup_success_email(
                        &u.name,
                        &topup.amount.to_string(),
                        &tier_normalized, // Used normalized tier for matching
                        &topup.reference,
                        payment_channel.unwrap_or("Mayar"),
                        user_lang.as_deref()
                    );
                    
                    let subject = match user_lang.as_deref() {
                        Some("id") => "Topup Berhasil - Ulyn AI",
                        Some("ja") => "入金完了 - Ulyn AI",
                        Some("ko") => "충전 성공 - Ulyn AI",
                        Some("zh") => "充值成功 - Ulyn AI",
                        Some("ru") => "Пополнение успешно - Ulyn AI",
                        Some("nl") => "Opwaardering geslaagd - Ulyn AI",
                        Some("af") => "Topup Suksesvol - Ulyn AI",
                        Some("ar") => "تم الشحن بنجاح - Ulyn AI",
                        _ => "Topup Successful - Ulyn AI",
                    };

                    let res = mail::send_html_email(Some(&state), &u.email, subject, &email_body).await;
                    if let Err(e) = res {
                        tracing::error!("Webhook: Failed to send success email to {}: {}", u.email, e);
                    } else {
                        tracing::info!("Webhook: Success email sent to {}", u.email);
                    }
                }
            }
        }
    }
    StatusCode::OK.into_response()
}

// =============================================
// Social Content Generation
// =============================================

#[derive(Deserialize)]
pub struct CaptionRequest {
    pub vibe: String,
    pub places: Vec<String>,
    pub lang: Option<String>,
}

#[derive(Serialize)]
pub struct CaptionResponse {
    pub caption: Option<String>,
    pub error: Option<String>,
}

pub async fn generate_caption_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<CaptionRequest>,
) -> (StatusCode, Json<CaptionResponse>) {
    tracing::info!("CAPTION: Vibe={}, Places={:?}", payload.vibe, payload.places);

    // Get user/guest info
    let auth_user = auth::get_current_user(&headers);
    let db_user = if let Some(ref au) = auth_user {
        db::find_user_by_email(&state.db, &au.email).await.unwrap_or(None)
    } else {
        None
    };

    let guest_id = if db_user.is_none() {
        Some(get_or_create_guest_id(&headers).0)
    } else {
        None
    };

    // Check Quota (No pre-deduct)
    match db::check_quota(&state.db, &state.redis, db_user.as_ref(), guest_id.as_deref(), db::FeatureType::Caption).await {
        Ok(true) => (),
        Ok(false) => return (StatusCode::FORBIDDEN, Json(CaptionResponse { caption: None, error: Some("Kuota harian habis. Silahkan topup atau kembali besok.".to_string()) })),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(CaptionResponse { caption: None, error: Some(format!("Quota check failed: {}", e)) })),
    }

    let lang = payload.lang.clone().unwrap_or_else(|| "id".to_string());
    match sumopod::generate_caption(&state.client, &payload.vibe, &payload.places, &lang).await {
        Ok(caption) => {
            // SUCCESS: Consume Quota
            let _ = db::consume_quota(&state.db, &state.redis, db_user.as_ref(), guest_id.as_deref(), db::FeatureType::Caption).await;
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
pub async fn get_quota_status_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> (StatusCode, Json<QuotaResponse>) {
    let auth_user = auth::get_current_user(&headers);
    let db_user = if let Some(ref au) = auth_user {
        db::find_user_by_email(&state.db, &au.email).await.unwrap_or(None)
    } else {
        None
    };

    let guest_id = if db_user.is_none() {
        Some(get_or_create_guest_id(&headers).0)
    } else {
        None
    };

    if let Some(ref u) = db_user {
        tracing::info!("Quota Check: Fetching status for user {}", u.email);
    }

    match db::get_quota_status(&state.db, &state.redis, db_user.as_ref(), guest_id.as_deref()).await {
        Ok(status) => {
            if let Some(ref u) = db_user {
                tracing::info!("Quota Check: SUCCESS for {}. Route limit: {}", u.email, status.route_limit);
            }
            (StatusCode::OK, Json(QuotaResponse { success: true, status: Some(status), error: None }))
        },
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(QuotaResponse { success: false, status: None, error: Some(e) })),
    }
}

pub async fn consume_quota_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<ConsumeQuotaRequest>,
) -> (StatusCode, Json<QuotaResponse>) {
    let auth_user = auth::get_current_user(&headers);
    let db_user = if let Some(ref au) = auth_user {
        db::find_user_by_email(&state.db, &au.email).await.unwrap_or(None)
    } else {
        None
    };

    let guest_id = if db_user.is_none() {
        Some(get_or_create_guest_id(&headers).0)
    } else {
        None
    };

    let feature = match payload.feature.as_str() {
        "route" => db::FeatureType::Route,
        "caption" => db::FeatureType::Caption,
        "receipt" => db::FeatureType::Receipt,
        _ => return (StatusCode::BAD_REQUEST, Json(QuotaResponse { success: false, status: None, error: Some("Invalid feature".to_string()) })),
    };

    match db::check_and_consume_quota(&state.db, &state.redis, db_user.as_ref(), guest_id.as_deref(), feature).await {
        Ok(true) => {
            let status = db::get_quota_status(&state.db, &state.redis, db_user.as_ref(), guest_id.as_deref()).await.ok();
            (StatusCode::OK, Json(QuotaResponse { success: true, status, error: None }))
        },
        Ok(false) => (StatusCode::FORBIDDEN, Json(QuotaResponse { success: false, status: None, error: Some("Kuota harian habis".to_string()) })),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(QuotaResponse { success: false, status: None, error: Some(e) })),
    }
}
