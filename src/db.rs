use sqlx::PgPool;
use uuid::Uuid;

/// Ensure the harvested_gems table exists
pub async fn run_migrations(pool: &PgPool) -> Result<(), sqlx::Error> {
    sqlx::query(r#"
        CREATE TABLE IF NOT EXISTS harvested_gems (
            id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
            source_url TEXT UNIQUE NOT NULL,
            platform TEXT NOT NULL DEFAULT 'unknown',
            name TEXT,
            category TEXT,
            description TEXT,
            location TEXT,
            lat DOUBLE PRECISION,
            lng DOUBLE PRECISION,
            thumbnail_url TEXT,
            is_place BOOLEAN NOT NULL DEFAULT false,
            confidence DOUBLE PRECISION NOT NULL DEFAULT 0.0,
            ai_analysis JSONB,
            created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
        );
    "#).execute(pool).await?;

    sqlx::query(r#"
        CREATE TABLE IF NOT EXISTS users (
            id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
            email TEXT UNIQUE NOT NULL,
            name TEXT NOT NULL,
            picture TEXT,
            google_id TEXT,
            credits INTEGER NOT NULL DEFAULT 7,
            tier TEXT NOT NULL DEFAULT 'free',
            created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
            updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
        );
    "#).execute(pool).await?;

    sqlx::query(r#"
        CREATE TABLE IF NOT EXISTS topups (
            id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
            user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            amount DOUBLE PRECISION NOT NULL,
            tier TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'pending',
            reference TEXT UNIQUE NOT NULL,
            payload JSONB,
            created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
            updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
        );
    "#).execute(pool).await?;

    sqlx::query(r#"
        CREATE TABLE IF NOT EXISTS route_history (
            id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
            user_id TEXT NOT NULL DEFAULT 'global',
            title TEXT NOT NULL,
            first_location TEXT,
            step_count INTEGER NOT NULL DEFAULT 0,
            route_json JSONB NOT NULL,
            created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
        );
    "#).execute(pool).await?;

    sqlx::query(r#"
        DO $$
        BEGIN
            IF NOT EXISTS (SELECT 1 FROM information_schema.columns WHERE table_name='route_history' AND column_name='user_id') THEN
                ALTER TABLE route_history ADD COLUMN user_id TEXT NOT NULL DEFAULT 'global';
            END IF;
        END
        $$;
    "#).execute(pool).await?;

    tracing::info!("DB: Migrations complete");
    Ok(())
}

/// Find a user by email
pub async fn find_user_by_email(pool: &PgPool, email: &str) -> Result<Option<User>, sqlx::Error> {
    sqlx::query_as::<_, User>("SELECT * FROM users WHERE email = $1")
        .bind(email)
        .fetch_optional(pool)
        .await
}

/// Create or update user after Google Login
pub async fn upsert_user(
    pool: &PgPool,
    email: &str,
    name: &str,
    picture: Option<&str>,
    google_id: &str,
) -> Result<User, sqlx::Error> {
    sqlx::query_as::<_, User>(
        r#"
        INSERT INTO users (email, name, picture, google_id, updated_at)
        VALUES ($1, $2, $3, $4, NOW())
        ON CONFLICT (email) DO UPDATE SET
            name = EXCLUDED.name,
            picture = EXCLUDED.picture,
            google_id = EXCLUDED.google_id,
            updated_at = NOW()
        RETURNING *
        "#
    )
    .bind(email)
    .bind(name)
    .bind(picture)
    .bind(google_id)
    .fetch_one(pool)
    .await
}

/// Find a user by ID
pub async fn find_user_by_id(pool: &PgPool, id: Uuid) -> Result<Option<User>, sqlx::Error> {
    sqlx::query_as::<_, User>("SELECT * FROM users WHERE id = $1")
        .bind(id)
        .fetch_optional(pool)
        .await
}

/// Create a topup record
pub async fn create_topup(
    pool: &PgPool,
    user_id: Uuid,
    amount: f64,
    tier: &str,
    reference: &str,
    payload: Option<&serde_json::Value>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO topups (user_id, amount, tier, reference, payload)
        VALUES ($1, $2, $3, $4, $5)
        "#
    )
    .bind(user_id)
    .bind(amount)
    .bind(tier)
    .bind(reference)
    .bind(payload)
    .execute(pool)
    .await?;
    Ok(())
}

#[derive(sqlx::FromRow, serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct User {
    pub id: Uuid,
    pub email: String,
    pub name: String,
    pub picture: Option<String>,
    pub google_id: Option<String>,
    pub credits: i32,
    pub tier: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize, sqlx::FromRow, Clone)]
pub struct Topup {
    pub id: Uuid,
    pub user_id: Uuid,
    pub amount: f64,
    pub tier: String,
    pub status: String,
    pub reference: String,
    pub payload: Option<serde_json::Value>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// Cleanup guest routes older than 24 hours
pub async fn cleanup_guest_routes(pool: &PgPool) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        DELETE FROM route_history
        WHERE user_id LIKE 'guest_%'
        AND created_at < NOW() - INTERVAL '1 day'
        "#
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// Save a generated route to history
pub async fn save_route(
    pool: &PgPool,
    id: Uuid,
    user_id: &str,
    title: &str,
    first_location: Option<&str>,
    step_count: i32,
    route_json: &serde_json::Value,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO route_history (id, user_id, title, first_location, step_count, route_json)
        VALUES ($1, $2, $3, $4, $5, $6)
        "#
    )
    .bind(id)
    .bind(user_id)
    .bind(title)
    .bind(first_location)
    .bind(step_count)
    .bind(route_json)
    .execute(pool)
    .await?;

    Ok(())
}

/// Get latest route history
pub async fn get_route_history(pool: &PgPool, user_id: &str, limit: i32) -> Result<Vec<RouteHistoryEntry>, sqlx::Error> {
    // Attempt cleanup
    let _ = cleanup_guest_routes(pool).await;

    let rows = sqlx::query_as::<_, RouteHistoryEntry>(
        r#"
        SELECT id, title, first_location, step_count, created_at
        FROM route_history
        WHERE user_id = $1
        ORDER BY created_at DESC
        LIMIT $2
        "#
    )
    .bind(user_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(rows)
}

/// Find a topup record by reference
pub async fn find_topup_by_reference(pool: &PgPool, reference: &str) -> Result<Option<Topup>, sqlx::Error> {
    sqlx::query_as::<_, Topup>("SELECT * FROM topups WHERE reference = $1")
        .bind(reference)
        .fetch_optional(pool)
        .await
}
// --- QUOTA SYSTEM ---

#[derive(Debug, Clone, Copy)]
pub enum FeatureType {
    Route,
    #[allow(dead_code)]
    Receipt,
    Caption,
}


/// Check and consume quota for a user (guest or auth).
/// Now uses a lifetime credit system for users and a lifetime trial for guests.
pub async fn check_and_consume_quota(
    pool: &sqlx::PgPool,
    redis: &redis::Client,
    user: Option<&User>,
    guest_id: Option<&str>,
    _feature: FeatureType, // Feature logic is now unified into general credits
) -> Result<bool, String> {
    if let Some(u) = user {
        // Authenticated users use credits from DB
        let row_user = sqlx::query_as::<_, User>("SELECT * FROM users WHERE id = $1")
            .bind(u.id)
            .fetch_one(pool)
            .await
            .map_err(|e| format!("DB Error: {}", e))?;

        if row_user.credits <= 0 {
            return Ok(false);
        }

        sqlx::query("UPDATE users SET credits = credits - 1 WHERE id = $1")
            .bind(u.id)
            .execute(pool)
            .await
            .map_err(|e| format!("DB Update Error: {}", e))?;
        
        return Ok(true);
    } else if let Some(gid) = guest_id {
        // Guests use a lifetime Redis key (no date)
        let redis_key = format!("guest_usage:{}", gid);
        let limit = 3;

        let mut conn = redis.get_multiplexed_tokio_connection().await
            .map_err(|e| format!("Redis connection error: {}", e))?;

        let current_usage: i32 = redis::cmd("GET").arg(&redis_key).query_async::<Option<i32>>(&mut conn)
            .await
            .map_err(|e| format!("Redis GET error: {}", e))?
            .unwrap_or(0);

        if current_usage >= limit {
            return Ok(false);
        }

        let _: () = redis::cmd("INCR").arg(&redis_key).query_async(&mut conn)
            .await
            .map_err(|e| format!("Redis INCR error: {}", e))?;
        
        return Ok(true);
    }

    Err("No user identification provided".into())
}

#[derive(serde::Serialize)]
pub struct QuotaStatus {
    pub route_used: i32,
    pub route_limit: i32,
    pub caption_used: i32,
    pub caption_limit: i32,
    pub receipt_used: i32,
    pub receipt_limit: i32,
}

/// Get the current quota status for a user/guest.
/// Simplified to show remaining total credits.
pub async fn get_quota_status(
    pool: &sqlx::PgPool,
    redis: &redis::Client,
    user: Option<&User>,
    guest_id: Option<&str>,
) -> Result<QuotaStatus, String> {
    if let Some(u) = user {
        // Authenticated user: get latest credits from DB
        let latest: (i32,) = sqlx::query_as("SELECT credits FROM users WHERE id = $1")
            .bind(u.id)
            .fetch_one(pool)
            .await
            .map_err(|e| format!("DB query error: {}", e))?;

        // In the new system, used/limit is per action, but we can show it as remaining
        return Ok(QuotaStatus {
            route_used: 0, 
            route_limit: latest.0,
            caption_used: 0,
            caption_limit: latest.0,
            receipt_used: 0,
            receipt_limit: latest.0,
        });
    } else if let Some(gid) = guest_id {
        let redis_key = format!("guest_usage:{}", gid);
        let mut conn = redis.get_multiplexed_tokio_connection().await
            .map_err(|e| format!("Redis connection error: {}", e))?;

        let used: i32 = redis::cmd("GET").arg(&redis_key).query_async::<Option<i32>>(&mut conn)
            .await.unwrap_or(Some(0)).unwrap_or(0);
        
        let limit = 3;

        return Ok(QuotaStatus {
            route_used: used,
            route_limit: limit,
            caption_used: used,
            caption_limit: limit,
            receipt_used: used,
            receipt_limit: limit,
        });
    }

    Err("No identification".into())
}
/// Find a route by ID
pub async fn find_route_by_id(pool: &PgPool, id: Uuid) -> Result<Option<serde_json::Value>, sqlx::Error> {
    let row: Option<(serde_json::Value,)> = sqlx::query_as(
        "SELECT route_json FROM route_history WHERE id = $1"
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| r.0))
}

#[derive(sqlx::FromRow, serde::Serialize)]
pub struct RouteHistoryEntry {
    pub id: Uuid,
    pub title: String,
    pub first_location: Option<String>,
    pub step_count: i32,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Upsert a harvested social link result into the database
pub async fn upsert_gem(
    pool: &PgPool,
    source_url: &str,
    platform: &str,
    name: Option<&str>,
    category: Option<&str>,
    description: Option<&str>,
    location: Option<&str>,
    lat: Option<f64>,
    lng: Option<f64>,
    thumbnail_url: Option<&str>,
    is_place: bool,
    confidence: f64,
    ai_analysis: Option<&serde_json::Value>,
) -> Result<Uuid, sqlx::Error> {
    let row: (Uuid,) = sqlx::query_as(
        r#"
        INSERT INTO harvested_gems (
            source_url, platform, name, category, description, location,
            lat, lng, thumbnail_url, is_place, confidence, ai_analysis, created_at
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
        ON CONFLICT (source_url) DO UPDATE SET
            platform = EXCLUDED.platform,
            name = EXCLUDED.name,
            category = EXCLUDED.category,
            description = EXCLUDED.description,
            location = EXCLUDED.location,
            lat = EXCLUDED.lat,
            lng = EXCLUDED.lng,
            thumbnail_url = EXCLUDED.thumbnail_url,
            is_place = EXCLUDED.is_place,
            confidence = EXCLUDED.confidence,
            ai_analysis = EXCLUDED.ai_analysis
        RETURNING id
        "#
    )
    .bind(source_url)
    .bind(platform)
    .bind(name)
    .bind(category)
    .bind(description)
    .bind(location)
    .bind(lat)
    .bind(lng)
    .bind(thumbnail_url)
    .bind(is_place)
    .bind(confidence)
    .bind(ai_analysis)
    .bind(chrono::Utc::now())
    .fetch_one(pool)
    .await?;

    Ok(row.0)
}

/// Look up an existing harvested gem by URL
pub async fn find_gem_by_url(
    pool: &PgPool,
    source_url: &str,
) -> Result<Option<crate::harvester::HarvestedGem>, sqlx::Error> {
    let row = sqlx::query_as::<_, crate::harvester::HarvestedGem>(
        r#"
        SELECT id, source_url, platform, name, category, description, location,
               lat, lng, thumbnail_url, is_place, confidence, ai_analysis, created_at
        FROM harvested_gems
        WHERE source_url = $1
        "#
    )
    .bind(source_url)
    .fetch_optional(pool)
    .await?;

    Ok(row)
}
