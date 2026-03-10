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
            credits INTEGER NOT NULL DEFAULT 10,
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
// --- QUOTA SYSTEM ---

#[derive(Debug, Clone, Copy)]
pub enum FeatureType {
    Route,
    #[allow(dead_code)]
    Receipt,
    Caption,
}

impl FeatureType {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Route => "route",
            Self::Receipt => "receipt",
            Self::Caption => "caption",
        }
    }
}

/// Check and consume quota for a user (guest or auth)
pub async fn check_and_consume_quota(
    pool: &sqlx::PgPool,
    redis: &redis::Client,
    user: Option<&User>,
    guest_id: Option<&str>,
    feature: FeatureType,
) -> Result<bool, String> {
    let tier = user.map(|u| u.tier.as_str()).unwrap_or("guest");
    let limit = match tier {
        "pro_pass" => 30,
        "basic_pass" => 15,
        "free" => 9,
        _ => 3,
    };

    let user_id = if let Some(u) = user {
        u.email.clone()
    } else if let Some(gid) = guest_id {
        gid.to_string()
    } else {
        return Err("No user identification provided".into());
    };

    let date = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let redis_key = format!("usage:{}:{}:{}", user_id, feature.as_str(), date);

    let mut conn = redis.get_multiplexed_tokio_connection().await
        .map_err(|e| format!("Redis connection error: {}", e))?;

    // Check usage in Redis
    let current_usage: i32 = redis::cmd("GET").arg(&redis_key).query_async::<Option<i32>>(&mut conn)
        .await
        .map_err(|e| format!("Redis GET error: {}", e))?
        .unwrap_or(0);

    if current_usage >= limit {
        return Ok(false);
    }

    // Increment usage in Redis
    let _: () = redis::cmd("INCR").arg(&redis_key).query_async(&mut conn)
        .await
        .map_err(|e| format!("Redis INCR error: {}", e))?;
    
    // Set expiry if new key (24h)
    if current_usage == 0 {
        let _: () = redis::cmd("EXPIRE").arg(&redis_key).arg(86400).query_async(&mut conn).await.unwrap_or_default();
    }

    // If authenticated user, also decrement credits if applicable
    if let Some(u) = user {
        if u.credits > 0 {
            let _ = sqlx::query("UPDATE users SET credits = credits - 1 WHERE id = $1")
                .bind(u.id)
                .execute(pool)
                .await;
        }
    }

    Ok(true)
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

/// Get the current quota status for a user/guest
pub async fn get_quota_status(
    redis: &redis::Client,
    user: Option<&User>,
    guest_id: Option<&str>,
) -> Result<QuotaStatus, String> {
    let tier = user.map(|u| u.tier.as_str()).unwrap_or("guest");
    
    // Limits based on tier
    let (route_limit, caption_limit, receipt_limit) = match tier {
        "pro_pass" => (30, 30, 50),
        "basic_pass" => (15, 10, 20),
        "free" => (9, 5, 10),
        _ => (3, 3, 3), // Guest
    };

    let user_identifier = if let Some(u) = user {
        u.email.clone()
    } else if let Some(gid) = guest_id {
        gid.to_string()
    } else {
        return Err("No user identification provided".into());
    };

    let date = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let mut conn = redis.get_multiplexed_tokio_connection().await
        .map_err(|e| format!("Redis connection error: {}", e))?;

    let route_used: i32 = redis::cmd("GET")
        .arg(format!("usage:{}:route:{}", user_identifier, date))
        .query_async::<Option<i32>>(&mut conn).await.unwrap_or(Some(0)).unwrap_or(0);
    let caption_used: i32 = redis::cmd("GET")
        .arg(format!("usage:{}:caption:{}", user_identifier, date))
        .query_async::<Option<i32>>(&mut conn).await.unwrap_or(Some(0)).unwrap_or(0);
    let receipt_used: i32 = redis::cmd("GET")
        .arg(format!("usage:{}:receipt:{}", user_identifier, date))
        .query_async::<Option<i32>>(&mut conn).await.unwrap_or(Some(0)).unwrap_or(0);

    Ok(QuotaStatus {
        route_used,
        route_limit,
        caption_used,
        caption_limit,
        receipt_used,
        receipt_limit,
    })
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
