mod config;
mod db;
mod errors;
mod handlers;
mod middleware;
mod models;
mod services;
mod utils;

/// HTTP 层集成测试。声明在 crate 根，因此可以直接调用 `build_router`。
#[cfg(test)]
mod http_tests;
use axum::{
    body::Body,
    extract::{DefaultBodyLimit, FromRef},
    http::{header, Method, Request, StatusCode},
    response::Response,
    routing::{delete, get, post},
    Router,
};
use sqlx::SqlitePool;
use std::net::SocketAddr;
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

    // into_make_service_with_connect_info：让 handler 能拿到客户端 IP。
    // 登录爆破限流按「用户名 + IP」双维度记账，没有它就只剩一个维度，
    // 攻击者换个来源即可重来。build_router 仍返回 Router<()>——
    // login 用的是 Option<ConnectInfo<_>>，缺少连接信息时降级而非报错，
    // 所以单元测试夹具不需要改动。
    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
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

    // SPA 回退。挂在 `ServeDir` 的 `fallback` 上，于是优先级是
    // 「静态资源 → 回退」，`/assets/*.js`、`favicon.svg` 等照常命中。
    //
    // 用的是 `fallback` 而**不是** `not_found_service`：后者内部是
    // `fallback(SetStatus::new(svc, 404))`，会把回退响应的状态码强行改写成 404。
    // SPA 回退必须返回 200 —— 浏览器拿到 404 仍会渲染 HTML，但前端代码、
    // 监控告警、CDN 缓存规则都会把它当成「页面不存在」，这类误报很难查。
    // `fallback` 保留内层服务自己的状态码。
    //
    // 为什么不能再只给 `/share/*` 挂回退：`/admin`、`/search`、`/trash`、`/shares`
    // 直接访问或刷新会 404。单页内导航看不出来，但「把 /admin 收藏了再点开」
    // 是很自然的动作，公网上这就是一句「你给的链接打不开」。
    //
    // 唯一需要区分的是 `/api/`：未匹配的接口路径必须继续返回 JSON 404，
    // 而不是 index.html —— 否则前端把端点名拼错时会拿到一坨 HTML，
    // axios 解析失败后报出的错误与真实原因毫无关系。
    let spa_fallback = {
        let sd = static_dir.clone();
        move |req: Request<Body>| {
            let sd = sd.clone();
            async move {
                use axum::response::IntoResponse;
                if req.uri().path().starts_with("/api/") {
                    return (
                        StatusCode::NOT_FOUND,
                        axum::Json(serde_json::json!({
                            "success": false,
                            "data": null,
                            "error": "接口不存在"
                        })),
                    )
                        .into_response();
                }
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
                            .unwrap_or_else(|_| {
                                (
                                    StatusCode::INTERNAL_SERVER_ERROR,
                                    "Internal Server Error",
                                )
                                    .into_response()
                            })
                    }
                    Err(_) => (StatusCode::NOT_FOUND, "Not Found").into_response(),
                }
            }
        }
    };

    // 静态文件服务：`ServeDir` 找不到的文件交给上面的 SPA 回退。
    let static_service = ServiceBuilder::new()
        .layer(SetResponseHeaderLayer::if_not_present(
            header::CACHE_CONTROL,
            header::HeaderValue::from_static("no-cache, no-store, must-revalidate"),
        ))
        .service(ServeDir::new(&static_dir).fallback(axum::routing::get(spa_fallback)));

    // 在一个 Router 中构建所有路由，避免合并带来的路由问题
    Router::new()
        // 认证路由
        .route("/api/auth/register", post(handlers::auth::register))
        .route("/api/auth/invite/verify", post(handlers::auth::verify_invite))
        .route("/api/auth/login", post(handlers::auth::login))
        .route("/api/auth/me", get(handlers::auth::me))
        // 媒体访问凭证。缩略图 / 预览图 / 下载链接不能带 Authorization 头，
        // 只能把凭据放进查询串；签发一枚窄口径、短时效的票据来替代 JWT。
        .route("/api/auth/media-ticket", post(handlers::auth::media_ticket))
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
        // 没有 `/api/batch/share`：批次模型下「分享选中的这批东西」就是
        // `POST /api/shares`（body 收 `items`），两个端点会完全同义。
        .route("/api/batch/unshare", post(handlers::batch::batch_unshare))
        // 公开分享路由
        .route("/api/public/shares/:id", get(handlers::share::public_share_access))
        .route("/api/public/shares/:id/verify", post(handlers::share::public_verify_password))
        // 浏览批次内容（根 = 批次里直接包含的条目；带 folder_id = 进入子目录）
        .route("/api/public/shares/:id/items", get(handlers::share::public_share_items))
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
        // 注册邀请码（注册制：管理员发码，客户凭码自助注册）
        .route("/api/admin/invite-codes", get(handlers::admin::list_invite_codes))
        .route("/api/admin/invite-codes", post(handlers::admin::create_invite_codes))
        .route("/api/admin/invite-codes/:id", delete(handlers::admin::delete_invite_code))
        // 孤儿文件清理（手动触发；后台另有 24 小时自动任务）
        .route("/api/admin/gc/cleanup", post(handlers::admin::run_gc_cleanup))
        .route("/api/admin/gc/status", get(handlers::admin::gc_status))
        // 健康检查
        // 存活探针。**必须符合全站统一的 {success, data, error} 信封**——
        // 前端 axios 拦截器（api.js）对所有响应走同一套解析，遇到没有
        // `success` 字段的响应会走兜底分支返回整个 body，调用方拿到的
        // 就不是 `data` 了。之前这个端点返回裸 {"status":"ok"}，
        // 是全站唯一的例外。
        //
        // `data.status` 同时保留，供 Docker HEALTHCHECK 与外部监控判断
        // （curl 只看 HTTP 码，但有些探针会读 body 里的 status 字段）。
        .route(
            "/api/health",
            get(|| async {
                axum::Json(serde_json::json!({
                    "success": true,
                    "data": { "status": "ok" },
                    "error": null
                }))
            }),
        )
        // SPA 回退由 fallback_service 内部处理（ServeDir 找不到就交给 index.html），
        // 不再为 `/share/*` 单独开一条路由。
        .fallback_service(static_service)
        .layer(TraceLayer::new_for_http().make_span_with(default_make_span))
        .layer(cors)
        // 全站安全响应头。放在最外层，静态资源、SPA 回退与 API 响应一并生效。
        //
        // · no-referrer：本页一旦真的跳出到站外，不要把当前 URL 带过去。
        //   缩略图/下载链接的查询串里带着媒体票据，从 Referer 漏出去是最没有
        //   技术含量的一种泄漏方式。
        // · SAMEORIGIN：站内用 <iframe> 展示 PDF，同源放行不受影响；
        //   同时挡住被外站嵌框做点击劫持。
        // · nosniff：媒体接口已单独加过，这里补上其余响应。
        .layer(SetResponseHeaderLayer::overriding(
            header::REFERRER_POLICY,
            header::HeaderValue::from_static("no-referrer"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::X_FRAME_OPTIONS,
            header::HeaderValue::from_static("SAMEORIGIN"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::X_CONTENT_TYPE_OPTIONS,
            header::HeaderValue::from_static("nosniff"),
        ))
        .with_state(state)
}

/// 构造请求日志 span：**抹掉 query string，只留 path**。
///
/// 为什么不能直接用 `TraceLayer::new_for_http()`：它的默认实现把完整 URI
/// （含 `?a=b&c=d`）写进 span 字段。本项目的缩略图/预览图走 `?token=<JWT>`
/// 旁路（`<img src>` 无法携带 Authorization 头，见 frontend/src/api.js），
/// 于是**每加载一次缩略图就把一个 7 天有效期的 bearer token 明文写进日志**，
/// 而 compose 配了 10m×3 轮转，等于把凭据复制三份留在磁盘上。
///
/// 默认 `tower_http=info` 时请求日志是 DEBUG 级、够不着，所以平时看不出来；
/// 但一旦有人为排查故障把 `RUST_LOG` 调成 `debug`，泄露立刻开始且不易察觉。
/// 这里不依赖日志级别——无论怎么调，query 都不会进日志。
fn default_make_span<B>(request: &Request<B>) -> tracing::Span {
    // path_and_query 是最省事也最危险的写法；这里显式只取 path()。
    tracing::info_span!(
        "request",
        method = %request.method(),
        path = %request.uri().path(),
        version = ?request.version(),
    )
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