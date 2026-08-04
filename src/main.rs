mod config;
mod db;
mod errors;
mod handlers;
mod middleware;
mod models;
mod services;
mod utils;

use axum::{
    body::Body,
    extract::{DefaultBodyLimit, FromRef},
    http::{Request, StatusCode, header},
    response::Response,
    routing::{delete, get, post},
    Router,
};
use sqlx::SqlitePool;
use tower::ServiceBuilder;
use tower_http::{
    cors::{Any, CorsLayer},
    services::ServeDir,
    set_header::SetResponseHeaderLayer,
    trace::TraceLayer,
};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use crate::config::Config;

/// Combined application state that supports FromRef for sub-state extraction
#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
    pub config: Config,
}

impl FromRef<AppState> for SqlitePool {
    fn from_ref(state: &AppState) -> Self {
        state.pool.clone()
    }
}

impl FromRef<AppState> for Config {
    fn from_ref(state: &AppState) -> Self {
        state.config.clone()
    }
}

#[tokio::main]
async fn main() {
    // Initialize tracing
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "pan_for_photographer=info,tower_http=info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    // Load configuration
    let config = Config::from_env();
    tracing::info!("Configuration loaded");

    // Initialize database
    let pool = db::init_db(&config.database_url)
        .await
        .expect("Failed to initialize database");
    tracing::info!("Database initialized");

    // Build the application
    let state = AppState {
        pool,
        config: config.clone(),
    };

    let router = build_router(state);

    // Start server
    let addr = format!("{}:{}", config.server_host, config.server_port);
    tracing::info!("Server starting at http://{}", addr);

    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .expect("Failed to bind address");

    axum::serve(listener, router)
        .await
        .expect("Server error");
}

fn build_router(state: AppState) -> Router<()> {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    // Static file serving with no-cache headers
    let static_service = ServiceBuilder::new()
        .layer(SetResponseHeaderLayer::if_not_present(
            header::CACHE_CONTROL,
            header::HeaderValue::from_static("no-cache, no-store, must-revalidate"),
        ))
        .service(ServeDir::new("static"));

    // SPA fallback: serve index.html for any non-API route
    async fn spa_fallback(_req: Request<Body>) -> Response<Body> {
        use axum::response::IntoResponse;
        match tokio::fs::read("static/index.html").await {
            Ok(contents) => Response::builder()
                .status(StatusCode::OK)
                .header("content-type", "text/html; charset=utf-8")
                .header("cache-control", "no-cache, no-store, must-revalidate")
                .header("pragma", "no-cache")
                .header("expires", "0")
                .body(Body::from(contents))
                .unwrap(),
            Err(_) => (StatusCode::NOT_FOUND, "Not Found").into_response(),
        }
    }

    // Build all routes directly in a single Router to avoid merge-related routing issues
    Router::new()
        // Auth routes
        .route("/api/auth/register", post(handlers::auth::register))
        .route("/api/auth/login", post(handlers::auth::login))
        .route("/api/auth/me", get(handlers::auth::me))
        // File routes
        .route("/api/files", get(handlers::files::list_files))
        .route("/api/files/upload", post(handlers::files::upload_files))
        .route("/api/files/:id/download", get(handlers::files::download_file))
        .route("/api/files/:id/media", get(handlers::files::serve_media))
        .route("/api/files/:id", delete(handlers::files::delete_file))
        // Folder routes
        .route("/api/folders", get(handlers::folders::list_folders))
        .route("/api/folders", post(handlers::folders::create_folder))
        .route("/api/folders/:id", delete(handlers::folders::delete_folder))
        // Share routes (auth required)
        .route("/api/shares", get(handlers::share::list_shares))
        .route("/api/shares", post(handlers::share::create_share))
        .route("/api/shares/:id", get(handlers::share::get_share))
        .route("/api/shares/:id", delete(handlers::share::delete_share))
        // Batch routes
        .route("/api/batch/move", post(handlers::batch::batch_move))
        .route("/api/batch/copy", post(handlers::batch::batch_copy))
        .route("/api/batch/delete", post(handlers::batch::batch_delete))
        .route("/api/batch/share", post(handlers::batch::batch_share))
        .route("/api/batch/unshare", post(handlers::batch::batch_unshare))
        // Public share routes
        .route("/api/public/shares/:id", get(handlers::share::public_share_access))
        .route("/api/public/shares/:id/verify", post(handlers::share::public_verify_password))
        .route("/api/public/shares/:id/download", get(handlers::share::public_share_download))
        .route("/api/public/shares/:id/media", get(handlers::share::public_share_media))
        // Search routes
        .route("/api/search", get(handlers::search::search_files))
        // SPA: serve index.html for share routes and other SPA paths
        .route("/share/*rest", get(spa_fallback))
        // Static files and SPA fallback
        .fallback_service(static_service)
        .layer(TraceLayer::new_for_http())
        .layer(cors)
        .layer(DefaultBodyLimit::max(state.config.max_file_size as usize))
        .with_state(state)
}