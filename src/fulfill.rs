use crate::AppState;
use axum::{
    extract::{Json, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

/// Payload pushed by the central payment service (ulyn-pay) when a payment
/// settles. Idempotent by `reference`.
#[derive(Debug, Deserialize)]
pub struct FulfillRequest {
    pub reference: String,
    #[serde(default)]
    pub tenant_slug: Option<String>,
    pub external_user_id: String,
    #[serde(default)]
    pub amount: f64,
    pub status: String,
    #[serde(default)]
    pub metadata: Value,
}

fn credits_for_tier(tier: &str) -> i32 {
    match tier.to_lowercase().as_str() {
        "pro_pass" => 60,
        "basic_pass" => 10,
        _ => 0,
    }
}

/// Length-independent constant-time comparison.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// POST /internal/payments/fulfill — grant (SUCCESS) or revoke (REFUNDED) for a
/// user. Authenticated with PAY_FULFILL_SECRET (not the user session).
pub async fn fulfill(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<FulfillRequest>,
) -> impl IntoResponse {
    let expected = std::env::var("PAY_FULFILL_SECRET").unwrap_or_default();
    if expected.trim().is_empty() {
        tracing::error!("PAY_FULFILL_SECRET not configured — rejecting fulfill");
        return (StatusCode::SERVICE_UNAVAILABLE, "fulfill not configured").into_response();
    }
    let provided = headers
        .get("X-Internal-Secret")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !constant_time_eq(provided.as_bytes(), expected.as_bytes()) {
        return (StatusCode::UNAUTHORIZED, "invalid secret").into_response();
    }

    let user_id = match Uuid::parse_str(req.external_user_id.trim()) {
        Ok(u) => u,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid external_user_id").into_response(),
    };

    let tier = req
        .metadata
        .get("target_tier")
        .and_then(|v| v.as_str())
        .or_else(|| req.metadata.get("tier").and_then(|v| v.as_str()))
        .unwrap_or("")
        .to_lowercase();

    tracing::info!(
        "fulfill: {} status={} tenant={:?} user={} tier={}",
        req.reference,
        req.status,
        req.tenant_slug,
        user_id,
        tier
    );

    match req.status.to_uppercase().as_str() {
        "SUCCESS" => apply_success(&state, &req, user_id, &tier).await,
        "REFUNDED" => apply_refund(&state, &req, user_id, &tier).await,
        other => {
            tracing::info!("fulfill: ignoring status {} for {}", other, req.reference);
            (StatusCode::OK, "ignored").into_response()
        }
    }
}

async fn apply_success(
    state: &AppState,
    req: &FulfillRequest,
    user_id: Uuid,
    tier: &str,
) -> axum::response::Response {
    let credits_to_add = credits_for_tier(tier);

    let mut tx = match state.db.begin().await {
        Ok(t) => t,
        Err(e) => {
            tracing::error!("fulfill: tx begin failed: {e}");
            return (StatusCode::INTERNAL_SERVER_ERROR, "db busy").into_response();
        }
    };

    // Idempotency: bail if this reference is already settled.
    let existing_status = sqlx::query_scalar::<_, String>(
        "SELECT status FROM topups WHERE reference = $1 FOR UPDATE",
    )
    .bind(&req.reference)
    .fetch_optional(&mut *tx)
    .await
    .ok()
    .flatten();

    if existing_status.as_deref() == Some("success") {
        let _ = tx.commit().await;
        return (StatusCode::OK, "already fulfilled").into_response();
    }

    // Grant credits + tier (mirrors the legacy webhook: sticky pro_pass, three
    // credit buckets + legacy column). Skip the tier write when no tier was
    // supplied so we never blank it out.
    let user_update = if tier.is_empty() {
        sqlx::query(
            "UPDATE users SET credits = credits + $1, credits_route = credits_route + $1, \
             credits_caption = credits_caption + $1, credits_receipt = credits_receipt + $1, \
             updated_at = NOW() WHERE id = $2",
        )
        .bind(credits_to_add)
        .bind(user_id)
        .execute(&mut *tx)
        .await
    } else {
        sqlx::query(
            "UPDATE users SET credits = credits + $1, credits_route = credits_route + $1, \
             credits_caption = credits_caption + $1, credits_receipt = credits_receipt + $1, \
             tier = CASE WHEN tier = 'pro_pass' THEN 'pro_pass' ELSE $2 END, \
             updated_at = NOW() WHERE id = $3",
        )
        .bind(credits_to_add)
        .bind(tier)
        .bind(user_id)
        .execute(&mut *tx)
        .await
    };
    if let Err(e) = user_update {
        tracing::error!("fulfill: user update failed: {e}");
        let _ = tx.rollback().await;
        return (StatusCode::INTERNAL_SERVER_ERROR, "grant failed").into_response();
    }

    // Record/settle the topup so the reference is idempotent + auditable.
    let upsert = if existing_status.is_some() {
        sqlx::query(
            "UPDATE topups SET status = 'success', payload = $1, updated_at = NOW() WHERE reference = $2",
        )
        .bind(&req.metadata)
        .bind(&req.reference)
        .execute(&mut *tx)
        .await
    } else {
        sqlx::query(
            "INSERT INTO topups (user_id, amount, tier, status, reference, payload) \
             VALUES ($1, $2, $3, 'success', $4, $5)",
        )
        .bind(user_id)
        .bind(req.amount)
        .bind(if tier.is_empty() { "unknown" } else { tier })
        .bind(&req.reference)
        .bind(&req.metadata)
        .execute(&mut *tx)
        .await
    };
    if let Err(e) = upsert {
        tracing::error!("fulfill: topup upsert failed: {e}");
        let _ = tx.rollback().await;
        return (StatusCode::INTERNAL_SERVER_ERROR, "record failed").into_response();
    }

    if let Err(e) = tx.commit().await {
        tracing::error!("fulfill: commit failed: {e}");
        return (StatusCode::INTERNAL_SERVER_ERROR, "commit failed").into_response();
    }

    tracing::info!("fulfill: granted {} (+{} credits) to {}", req.reference, credits_to_add, user_id);
    (StatusCode::OK, "fulfilled").into_response()
}

async fn apply_refund(
    state: &AppState,
    req: &FulfillRequest,
    user_id: Uuid,
    tier: &str,
) -> axum::response::Response {
    // Best-effort claw back of the granted credits. Tier is left as-is (sticky
    // pro_pass makes an automatic downgrade ambiguous) and logged for review.
    let credits = credits_for_tier(tier);

    let already = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM topups WHERE reference = $1 AND status = 'refunded')",
    )
    .bind(&req.reference)
    .fetch_one(&state.db)
    .await
    .unwrap_or(false);
    if already {
        return (StatusCode::OK, "already refunded").into_response();
    }

    if credits > 0 {
        let _ = sqlx::query(
            "UPDATE users SET credits = GREATEST(credits - $1, 0), \
             credits_route = GREATEST(credits_route - $1, 0), \
             credits_caption = GREATEST(credits_caption - $1, 0), \
             credits_receipt = GREATEST(credits_receipt - $1, 0), updated_at = NOW() WHERE id = $2",
        )
        .bind(credits)
        .bind(user_id)
        .execute(&state.db)
        .await;
    }
    tracing::warn!("fulfill: refund {} — tier left unchanged, review manually", req.reference);

    let _ = sqlx::query("UPDATE topups SET status = 'refunded', updated_at = NOW() WHERE reference = $1")
        .bind(&req.reference)
        .execute(&state.db)
        .await;

    (StatusCode::OK, "refunded").into_response()
}
