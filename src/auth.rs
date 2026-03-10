use axum::{
    extract::{Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Redirect},
};
use serde::{Deserialize, Serialize};
use crate::AppState;

// ── Google OAuth URLs ─────────────────────────────────────────
const GOOGLE_AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const GOOGLE_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const GOOGLE_USERINFO_URL: &str = "https://www.googleapis.com/oauth2/v2/userinfo";

// ── Structs ───────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct GoogleUser {
    pub id: String,
    pub email: String,
    pub name: String,
    pub picture: Option<String>,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
}

#[derive(Deserialize)]
pub struct CallbackQuery {
    pub code: Option<String>,
    pub error: Option<String>,
}

// ── Cookie helpers ────────────────────────────────────────────

/// Encode user info as a simple base64 JSON cookie value
pub fn encode_session(user: &GoogleUser) -> String {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;
    let json = serde_json::to_string(user).unwrap_or_default();
    URL_SAFE_NO_PAD.encode(json.as_bytes())
}

/// Decode session cookie back into GoogleUser
pub fn decode_session(cookie_val: &str) -> Option<GoogleUser> {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;
    let bytes = URL_SAFE_NO_PAD.decode(cookie_val).ok()?;
    let json = String::from_utf8(bytes).ok()?;
    serde_json::from_str(&json).ok()
}

/// Extract logged-in user from request headers (Cookie: ulyn_session=...)
pub fn get_current_user(headers: &HeaderMap) -> Option<GoogleUser> {
    let cookie_header = headers.get(header::COOKIE)?.to_str().ok()?;
    for part in cookie_header.split(';') {
        let part = part.trim();
        if let Some(val) = part.strip_prefix("ulyn_session=") {
            return decode_session(val);
        }
    }
    None
}

// ── Handlers ──────────────────────────────────────────────────

/// GET /auth/google → redirect to Google consent screen
pub async fn google_login() -> impl IntoResponse {
    let client_id = std::env::var("GOOGLE_CLIENT_ID").unwrap_or_default();
    let redirect_uri = std::env::var("GOOGLE_REDIRECT_URL").unwrap_or_default();

    let url = format!(
        "{}?client_id={}&redirect_uri={}&response_type=code&scope=openid%20email%20profile&access_type=offline&prompt=select_account",
        GOOGLE_AUTH_URL,
        urlencoding::encode(&client_id),
        urlencoding::encode(&redirect_uri),
    );
    Redirect::to(&url)
}

/// GET /auth/google/callback → exchange code for token, set session cookie
pub async fn google_callback(
    Query(params): Query<CallbackQuery>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    if let Some(err) = &params.error {
        tracing::warn!("Google OAuth error: {}", err);
        return (
            StatusCode::FOUND,
            axum::http::HeaderMap::from_iter(vec![
                (header::LOCATION, "/".parse().unwrap()),
            ]),
            "",
        ).into_response();
    }

    let code = match &params.code {
        Some(c) => c.clone(),
        None => return Redirect::to("/").into_response(),
    };

    let client_id = std::env::var("GOOGLE_CLIENT_ID").unwrap_or_default();
    let client_secret = std::env::var("GOOGLE_CLIENT_SECRET").unwrap_or_default();
    let redirect_uri = std::env::var("GOOGLE_REDIRECT_URL").unwrap_or_default();

    // Exchange code for access token
    let client = reqwest::Client::new();
    let token_res = client
        .post(GOOGLE_TOKEN_URL)
        .form(&[
            ("code", code.as_str()),
            ("client_id", client_id.as_str()),
            ("client_secret", client_secret.as_str()),
            ("redirect_uri", redirect_uri.as_str()),
            ("grant_type", "authorization_code"),
        ])
        .send()
        .await;

    let token_data = match token_res {
        Ok(r) => match r.json::<TokenResponse>().await {
            Ok(t) => t,
            Err(e) => {
                tracing::error!("Token parse error: {}", e);
                return Redirect::to("/").into_response();
            }
        },
        Err(e) => {
            tracing::error!("Token request error: {}", e);
            return Redirect::to("/").into_response();
        }
    };

    // Fetch user info
    let user_res = client
        .get(GOOGLE_USERINFO_URL)
        .bearer_auth(&token_data.access_token)
        .send()
        .await;

    let user: GoogleUser = match user_res {
        Ok(r) => match r.json().await {
            Ok(u) => u,
            Err(e) => {
                tracing::error!("Userinfo parse error: {}", e);
                return Redirect::to("/").into_response();
            }
        },
        Err(e) => {
            tracing::error!("Userinfo request error: {}", e);
            return Redirect::to("/").into_response();
        }
    };

    tracing::info!("Google OAuth login: {} ({})", user.name, user.email);

    // Save/Update user in DB
    let _ = crate::db::upsert_user(
        &state.db,
        &user.email,
        &user.name,
        user.picture.as_deref(),
        &user.id
    ).await;

    // Set session cookie (HttpOnly, SameSite=Lax, 7 days)
    let session_val = encode_session(&user);
    let cookie = format!(
        "ulyn_session={}; Path=/; HttpOnly; SameSite=Lax; Max-Age=604800",
        session_val
    );

    let mut headers = axum::http::HeaderMap::new();
    headers.insert(header::LOCATION, "/".parse().unwrap());
    headers.insert(header::SET_COOKIE, cookie.parse().unwrap());

    (StatusCode::FOUND, headers, "").into_response()
}

/// GET /auth/logout → clear session cookie
pub async fn logout() -> impl IntoResponse {
    let cookie = "ulyn_session=; Path=/; HttpOnly; Max-Age=0";
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(header::LOCATION, "/".parse().unwrap());
    headers.insert(header::SET_COOKIE, cookie.parse().unwrap());
    (StatusCode::FOUND, headers, "")
}
