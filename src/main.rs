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
    http::{header, Method, Request, StatusCode},
    response::Response,
    routing::{delete, get, post},
    Router,
};
use sqlx::SqlitePool;
use std::sync::Arc;
use tokio::sync::Semaphore;
use tokio_util::io::ReaderStream;
use tower::ServiceBuilder;
use tower_http::{
    cors::{AllowOrigin, CorsLayer},
    services::ServeDir,
    set_header::SetResponseHeaderLayer,
    trace::TraceLayer,
};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use crate::config::Config;

/// 组合应用状态，支持通过 FromRef 提取子状态
#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
    pub config: Config,
    /// 缩略图/预览图后台生成的并发闸，限制同时运行的图片解码任务数
    pub preview_semaphore: Arc<Semaphore>,
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

impl FromRef<AppState> for Arc<Semaphore> {
    fn from_ref(state: &AppState) -> Self {
        state.preview_semaphore.clone()
    }
}

#[tokio::main]
async fn main() {
    // 初始化 tracing 日志
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "pan_for_photographer=info,tower_http=info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    // 加载配置
    let config = Config::from_env();
    tracing::info!("配置已加载");

    // 初始化数据库
    let pool = db::init_db(&config.database_url)
        .await
        .expect("数据库初始化失败");
    tracing::info!("数据库已初始化");

    // 确保超级管理员存在
    db::seed_admin(&pool).await.expect("种子管理员初始化失败");
    tracing::info!("超级管理员已就绪");

    // 后台缩略图任务并发上限：permits = 2
    let preview_semaphore = Arc::new(Semaphore::new(2));

    // 启动孤儿文件清理器（周期 GC）
    crate::services::sweeper::start(pool.clone(), config.clone(), preview_semaphore.clone());

    // 构建应用
    let state = AppState {
        pool,
        config: config.clone(),
        preview_semaphore,
    };

    let router = build_router(state);

    // 启动服务器（若 server_host 是 IPv6 地址，自动补上方括号）
    let host = config.server_host;
    let host = if host.contains(':') && !host.starts_with('[') {
        format!("[{}]", host)
    } else {
        host
    };
    let addr = format!("{}:{}", host, config.server_port);
    tracing::info!("服务器正在启动，地址为 http://{}", addr);

    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .expect("绑定地址失败");

    axum::serve(listener, router)
        .await
        .expect("服务器运行错误");
}

fn build_router(state: AppState) -> Router<()> {
    // 只放行受信任的来源，反射任意请求 Origin 会放宽同源策略，造成跨站可利用面。
    let cors = CorsLayer::new()
        .allow_origin(allowed_cors_origins())
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers([header::CONTENT_TYPE, header::AUTHORIZATION]);

    let static_dir = state.config.static_dir.clone();

    // 静态文件服务，添加无缓存响应头
    let static_service = ServiceBuilder::new()
        .layer(SetResponseHeaderLayer::if_not_present(
            header::CACHE_CONTROL,
            header::HeaderValue::from_static("no-cache, no-store, must-revalidate"),
        ))
        .service(ServeDir::new(&static_dir));

    // SPA 回退：对非 API 路由返回 index.html
    let spa_fallback = {
        let sd = static_dir.clone();
        async move |_req: Request<Body>| -> Response<Body> {
            use axum::response::IntoResponse;
            match tokio::fs::File::open(std::path::Path::new(&sd).join("index.html")).await {
            Ok(file) => {
                let stream = ReaderStream::new(file);
                let body = Body::from_stream(stream);
                Response::builder()
                    .status(StatusCode::OK)
                    .header("content-type", "text/html; charset=utf-8")
                    .header("cache-control", "no-cache, no-store, must-revalidate")
                    .header("pragma", "no-cache")
                    .header("expires", "0")
                    .body(body)
                    .unwrap_or_else(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error").into_response())
            }
            Err(_) => (StatusCode::NOT_FOUND, "Not Found").into_response(),
        }
        }
    };

    // 在一个 Router 中构建所有路由，避免合并带来的路由问题
    Router::new()
        // 认证路由
        .route("/api/auth/register", post(handlers::auth::register))
        .route("/api/auth/login", post(handlers::auth::login))
        .route("/api/auth/me", get(handlers::auth::me))
        // 文件路由
        .route("/api/files", get(handlers::files::list_files))
        // 仅上传路由放开大数据量请求体上限；其余接口保持 axum 默认较小限制，缩小 DoS 面
        .route(
            "/api/files/upload",
            post(handlers::files::upload_files)
                .layer(DefaultBodyLimit::max(state.config.max_file_size as usize)),
        )
        .route("/api/files/:id/download", get(handlers::files::download_file))
        .route("/api/files/:id/media", get(handlers::files::serve_media))
        .route("/api/files/:id", delete(handlers::files::delete_file))
        .route("/api/files/:id/rename", axum::routing::put(handlers::files::rename_file))
        .route("/api/files/:id/restore", post(handlers::files::restore_file))
        .route("/api/files/:id/permanent", delete(handlers::files::permanent_delete_file))
        // 文件夹路由
        .route("/api/folders", get(handlers::folders::list_folders))
        .route("/api/folders", post(handlers::folders::create_folder))
        .route("/api/folders/:id", delete(handlers::folders::delete_folder))
        .route("/api/folders/:id/rename", axum::routing::put(handlers::folders::rename_folder))
        .route("/api/folders/:id/restore", post(handlers::folders::restore_folder))
        .route("/api/folders/:id/permanent", delete(handlers::folders::permanent_delete_folder))
        // 回收站路由
        .route("/api/trash", get(handlers::files::list_trash))
        .route("/api/trash", delete(handlers::files::empty_trash))
        // 分享路由（需要认证）
        .route("/api/shares", get(handlers::share::list_shares))
        .route("/api/shares", post(handlers::share::create_share))
        .route("/api/shares/:id", get(handlers::share::get_share))
        .route("/api/shares/:id", delete(handlers::share::delete_share))
        // 批量操作路由
        .route("/api/batch/move", post(handlers::batch::batch_move))
        .route("/api/batch/copy", post(handlers::batch::batch_copy))
        .route("/api/batch/delete", post(handlers::batch::batch_delete))
        .route("/api/batch/share", post(handlers::batch::batch_share))
        .route("/api/batch/unshare", post(handlers::batch::batch_unshare))
        // 公开分享路由
        .route("/api/public/shares/:id", get(handlers::share::public_share_access))
        .route("/api/public/shares/:id/verify", post(handlers::share::public_verify_password))
        .route("/api/public/shares/:id/download", get(handlers::share::public_share_download))
        .route("/api/public/shares/:id/media", get(handlers::share::public_share_media))
        // 搜索路由
        .route("/api/search", get(handlers::search::search_files))
        // 管理员路由
        .route("/api/admin/users", get(handlers::admin::list_users))
        .route("/api/admin/users", post(handlers::admin::create_user))
        .route("/api/admin/users/:id", delete(handlers::admin::delete_user))
        .route("/api/admin/users/:id", axum::routing::put(handlers::admin::update_user))
        .route("/api/admin/users/:id/role", axum::routing::put(handlers::admin::update_user_role))
        .route("/api/admin/users/:id/folders", get(handlers::admin::admin_list_user_folders))
        .route("/api/admin/users/:id/folders", post(handlers::admin::admin_create_user_folder))
        .route("/api/admin/stats", get(handlers::admin::get_stats))
        // 健康检查
        .route("/api/health", get(|| async { axum::Json(serde_json::json!({"status": "ok"})) }))
        // SPA：为分享路由及其他 SPA 路径提供 index.html
        .route("/share/*rest", get(spa_fallback))
        // 静态文件及 SPA 回退
        .fallback_service(static_service)
        .layer(TraceLayer::new_for_http())
        .layer(cors)
        .with_state(state)
}

/// 构建 CORS 白名单来源：来自环境变量 CORS_ALLOWED_ORIGINS（空格或逗号分隔），
/// 并始终包含本地开发 / 双端口发布常用来源，便于局域网与开发模式使用。
fn allowed_cors_origins() -> AllowOrigin {
    let mut origins: Vec<header::HeaderValue> = vec![];

    // 环境变量追加
    if let Ok(env) = std::env::var("CORS_ALLOWED_ORIGINS") {
        for part in env.split([',', ' ']) {
            let part = part.trim();
            if !part.is_empty() {
                if let Ok(v) = header::HeaderValue::from_str(part) {
                    origins.push(v);
                }
            }
        }
    }

    // 兜底的本地常用来源（覆盖默认单端口、Vite 开发、双端口发布）
    let defaults = [
        "http://localhost:100",
        "http://127.0.0.1:100",
        "http://localhost:8000",
        "http://127.0.0.1:8000",
        "http://localhost:8001",
        "http://127.0.0.1:8001",
        "http://localhost:8002",
        "http://127.0.0.1:8002",
        "http://localhost:5173",
        "http://127.0.0.1:5173",
    ];
    for o in defaults {
        if let Ok(v) = header::HeaderValue::from_str(o) {
            origins.push(v);
        }
    }

    AllowOrigin::list(origins)
}