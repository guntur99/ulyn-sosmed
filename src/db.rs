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
        CREATE TABLE IF NOT EXISTS route_history (
            id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
            title TEXT NOT NULL,
            first_location TEXT,
            step_count INTEGER NOT NULL DEFAULT 0,
            route_json JSONB NOT NULL,
            created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
        );
    "#).execute(pool).await?;

    tracing::info!("DB: Migrations complete");
    Ok(())
}

/// Save a generated route to history
pub async fn save_route(
    pool: &PgPool,
    id: Uuid,
    title: &str,
    first_location: Option<&str>,
    step_count: i32,
    route_json: &serde_json::Value,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO route_history (id, title, first_location, step_count, route_json)
        VALUES ($1, $2, $3, $4, $5)
        "#
    )
    .bind(id)
    .bind(title)
    .bind(first_location)
    .bind(step_count)
    .bind(route_json)
    .execute(pool)
    .await?;

    Ok(())
}

/// Get latest route history
pub async fn get_route_history(pool: &PgPool, limit: i32) -> Result<Vec<RouteHistoryEntry>, sqlx::Error> {
    let rows = sqlx::query_as::<_, RouteHistoryEntry>(
        r#"
        SELECT id, title, first_location, step_count, created_at
        FROM route_history
        ORDER BY created_at DESC
        LIMIT $1
        "#
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(rows)
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
