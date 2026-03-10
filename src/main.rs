use axum::{
    routing::{get, post},
    Router,
};
use sqlx::postgres::PgPoolOptions;
use std::net::SocketAddr;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use std::time::Duration;

mod handlers;
mod mayar;
mod sumopod;
mod harvester;
mod auth;
mod db;
mod storage;
mod mail;
mod email_templates;
mod weather;

#[derive(Clone)]
pub struct AppState {
    pub routes: Arc<RwLock<HashMap<String, sumopod::RouteData>>>,
    pub db: sqlx::PgPool,
    pub redis: redis::Client,
    pub client: reqwest::Client,
}

#[tokio::main]
async fn main() {
    // load env variables
    dotenvy::dotenv().ok();

    // initialize tracing
    tracing_subscriber::fmt::init();

    // Connect to PostgreSQL (Durable Config for Railway Proxy)
    let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    
    use std::str::FromStr;
    let connect_options = sqlx::postgres::PgConnectOptions::from_str(&database_url)
        .expect("Invalid DATABASE_URL")
        .statement_cache_capacity(0); // Fixed for Railway/PgBouncer

    let pool = PgPoolOptions::new()
        .max_connections(3) // Lowered to reduce proxy handshake pressure
        .min_connections(0)
        .acquire_timeout(Duration::from_secs(120))
        .idle_timeout(Duration::from_secs(60))
        .connect_lazy_with(connect_options);

    tracing::info!("DB: PostgreSQL Pool initialized (Lazy)");

    // Connect to Redis with Retry Logic
    let redis_url = std::env::var("REDIS_URL").expect("REDIS_URL must be set");
    let redis_client = redis::Client::open(redis_url).expect("Failed to connect to Redis");
    
    let mut redis_connection_success = false;
    for i in 1..=3 {
        tracing::info!("DB: Connecting to Redis (Attempt {}/3)...", i);
        match redis_client.get_multiplexed_tokio_connection().await {
            Ok(_) => {
                tracing::info!("DB: Connected to Redis");
                redis_connection_success = true;
                break;
            }
            Err(e) => {
                tracing::warn!("DB: Redis connection attempt {} failed: {}. Retrying in 5s...", i, e);
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        }
    }

    if !redis_connection_success {
        panic!("DB: All Redis connection attempts failed. Please check your REDIS_URL/Redis status.");
    }

    if !redis_connection_success {
        panic!("DB: All Redis connection attempts failed. Please check your REDIS_URL/Redis status.");
    }

    // Run migrations with Retry Logic (Railway Proxy Resilience)
    let skip_migrations = std::env::var("SKIP_MIGRATIONS").map(|v| v == "true").unwrap_or(false);
    let mut migration_success = false;
    
    if skip_migrations {
        tracing::info!("DB: SKIP_MIGRATIONS is true. Skipping migrations.");
        migration_success = true;
    } else {
        for i in 1..=3 {
            tracing::info!("DB: Running migrations (Attempt {}/3)...", i);
            match db::run_migrations(&pool).await {
                Ok(_) => {
                    tracing::info!("DB: Migrations complete");
                    migration_success = true;
                    break;
                }
                Err(e) => {
                    tracing::warn!("DB: Migration attempt {} failed: {}. Retrying in 5s...", i, e);
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
            }
        }
    }

    if !migration_success {
        panic!("DB: All migration attempts failed. Please check your Railway Proxy/Database status.");
    }

    let state = AppState {
        routes: Arc::new(RwLock::new(HashMap::new())),
        db: pool,
        redis: redis_client,
        client: reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .pool_idle_timeout(Duration::from_secs(90))
            .build()
            .expect("Failed to create reqwest client"),
    };

    // build our application with a route
    let app = Router::new()
        .nest_service("/static", tower_http::services::ServeDir::new("static"))
        .route("/", get(handlers::root))
        .route("/route/{id}", get(handlers::route_handler))
        .route("/api/generate", post(handlers::generate))
        .route("/api/harvest", post(handlers::harvest))
        .route("/api/caption", post(handlers::generate_caption_handler))
        .route("/api/quota/status", get(handlers::get_quota_status_handler))
        .route("/api/quota/consume", post(handlers::consume_quota_handler))
        .route("/api/checkout", post(handlers::checkout))
        .route("/api/payment/callback", post(handlers::payment_callback))
        .route("/auth/google", get(auth::google_login))
        .route("/auth/google/callback", get(auth::google_callback))
        .route("/auth/logout", get(auth::logout))
        .with_state(state);

    // run our app with hyper
    // Railway injects $PORT; fallback to 3005 for local dev
    let port: u16 = std::env::var("PORT")
        .unwrap_or_else(|_| "3005".to_string())
        .parse()
        .expect("PORT must be a valid number");
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    println!("listening on {}", addr);
    axum::serve(listener, app).await.unwrap();
}
