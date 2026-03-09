use axum::{
    routing::{get, post},
    Router,
};
use sqlx::postgres::PgPoolOptions;
use std::net::SocketAddr;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

mod handlers;
mod mayar;
mod sumopod;
mod harvester;
mod db;
mod storage;

#[derive(Clone)]
pub struct AppState {
    pub routes: Arc<RwLock<HashMap<String, sumopod::RouteData>>>,
    pub db: sqlx::PgPool,
}

#[tokio::main]
async fn main() {
    // load env variables
    dotenvy::dotenv().ok();

    // initialize tracing
    tracing_subscriber::fmt::init();

    // Connect to PostgreSQL
    let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&database_url)
        .await
        .expect("Failed to connect to PostgreSQL");

    tracing::info!("DB: Connected to PostgreSQL");

    // Connect to Redis
    let redis_url = std::env::var("REDIS_URL").expect("REDIS_URL must be set");
    let redis_client = redis::Client::open(redis_url).expect("Failed to connect to Redis");
    let _redis_conn = redis_client
        .get_multiplexed_tokio_connection()
        .await
        .expect("Failed to get Redis connection");
    tracing::info!("DB: Connected to Redis");

    // Run migrations (create tables if not exist)
    db::run_migrations(&pool).await.expect("DB migration failed");

    let state = AppState {
        routes: Arc::new(RwLock::new(HashMap::new())),
        db: pool,
    };

    // build our application with a route
    let app = Router::new()
        .route("/", get(handlers::root))
        .route("/route/{id}", get(handlers::route_handler))
        .route("/api/generate", post(handlers::generate))
        .route("/api/harvest", post(handlers::harvest))
        .route("/api/caption", post(handlers::generate_caption_handler))
        .route("/api/checkout", post(handlers::checkout))
        .route("/api/payment/callback", post(handlers::payment_callback))
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
