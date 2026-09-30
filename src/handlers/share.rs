use std::net::SocketAddr;

use axum::{
    body::Body,
    extract::{ConnectInfo, Path, Query, State},
    http::{header, StatusCode},
    response::Response,
    Json,
};
use tokio_util::io::ReaderStream;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::config::Config;
use crate::errors::AppError;
use crate::middleware::auth::AuthUser;
use crate::services::share_service;
use crate::services::file_service;
use crate::handlers::files;
use crate::utils::login_throttle;
use crate::utils::crypto;
use sqlx::SqlitePool;

/// 受密码保护分享的访问凭证有效期（秒）。默认 2 小时。
const SHARE_TICKET_TTL_SECS: i64 = 2 * 60 * 60;

/// 从查询参数中提取并校验分享访问凭证。
fn check_share_ticket(config: &Config, share_id: &str, params: &std::collections::HashMap<String, String>) -> bool {
    match params.get("ticket") {
        Some(t) => crypto::verify_share_ticket(config, share_id, t),
        None => false,
    }
}

#[derive(Debug, Deserialize)]
pub struct CreateShareRequest {
    pub file_id: Option<i64>,
    pub folder_id: Option<i64>,
    pub expires_hours: Option<i64>,
    pub password: Option<String>,
    pub max_downloads: Option<i64>,
    pub custom_code: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct VerifyShareRequest {
    pub password: String,
}

// ========== 需要认证的分享接口 ==========

/// POST /api/shares 创建分享
pub async fn create_share(
    State(pool): State<SqlitePool>,
    auth: AuthUser,
    Json(req): Json<CreateShareRequest>,
) -> Result<Json<Value>, AppError> {
    let share = share_service::create_share(
        &pool,
        req.file_id,
        req.folder_id,
        auth.user_id,
        req.expires_hours,
        req.password,
        req.max_downloads,
        req.custom_code,
    )
    .await?;

    let info = share_service::get_share(&pool, &share.id, auth.user_id).await?;

    Ok(Json(json!({
        "success": true,
        "data": info,
        "error": null
    })))
}

/// GET /api/shares 获取分享列表
pub async fn list_shares(
    State(pool): State<SqlitePool>,
    auth: AuthUser,
) -> Result<Json<Value>, AppError> {
    let shares = share_service::list_shares(&pool, auth.user_id).await?;
    Ok(Json(json!({
        "success": true,
        "data": shares,
        "error": null
    })))
}

/// GET /api/shares/:id 获取分享详情
pub async fn get_share(
    State(pool): State<SqlitePool>,
    auth: AuthUser,
    Path(share_id): Path<String>,
) -> Result<Json<Value>, AppError> {
    let share = share_service::get_share(&pool, &share_id, auth.user_id).await?;
    Ok(Json(json!({
        "success": true,
        "data": share,
        "error": null
    })))
}

/// DELETE /api/shares/:id 删除分享
pub async fn delete_share(
    State(pool): State<SqlitePool>,
    auth: AuthUser,
    Path(share_id): Path<String>,
) -> Result<Json<Value>, AppError> {
    share_service::delete_share(&pool, &share_id, auth.user_id).await?;
    Ok(Json(json!({
        "success": true,
        "data": null,
        "error": null
    })))
}

// ========== 公开分享接口（无需认证） ==========

/// GET /api/public/shares/:id 访问公开分享
pub async fn public_share_access(
    State(pool): State<SqlitePool>,
    Path(share_id): Path<String>,
) -> Result<Json<Value>, AppError> {
    let share = share_service::get_public_share(&pool, &share_id).await?;
    Ok(Json(json!({
        "success": true,
        "data": share,
        "error": null
    })))
}

/// POST /api/public/shares/:id/verify 验证分享密码并签发访问凭证
///
/// **限流的必要性**：本接口**无鉴权**（拿到分享链接即可调用），而每次调用
/// 都会执行一次 bcrypt 校验（`DEFAULT_COST=12`，约 250ms CPU）。没有限流时
/// 攻击者可以拿任意一个受密码保护的分享码无限次爆破口令，同时把服务端 CPU
/// 打满——典型的计算放大 DoS。实测：12 次错误口令串行耗时 22.9 秒，
/// 且全程无任何 429。
///
/// 复用 `login_throttle` 的退避机制（5 次失败后指数退避、封顶 15 分钟），
/// 但**只启用分享码这一维度**、不带 IP——理由见函数内注释。
/// 检查放在 bcrypt **之前**，让爆破在付出哈希成本之前就被挡下：
/// 实测被限流后响应从约 250ms 降到 2ms。
pub async fn public_verify_password(
    State(pool): State<SqlitePool>,
    State(config): State<Config>,
    client: Option<ConnectInfo<SocketAddr>>,
    Path(share_id): Path<String>,
    Json(req): Json<VerifyShareRequest>,
) -> Result<Json<Value>, AppError> {
    let ip = client
        .map(|ConnectInfo(a)| a.ip().to_string())
        .unwrap_or_default();

    // **只按分享码维度计数，不带 IP。**
    //
    // 登录限流是「用户名 + IP」双维度，但这里必须去掉 IP 维度：
    // 访客与攻击者通常来自同一个出口 IP（家庭宽带 / Cloudflare Tunnel 后
    // 更是有大量请求来自少数几个 IP）。若带 IP 维度，一次针对 A 分享码的
    // 爆破会让**同 IP 下所有访客都无法访问任何受密码保护的分享**——
    // 攻击者只需打满一个码，就能把该出口的全部客户锁在门外。
    //
    // 只按分享码计数的残余风险是「换码重置」，但分享码是 122 bit 熵的
    // uuid4（除非摄影师自定义了可枚举的 custom_code），攻击者手上没有
    // 别的码可换；真正的防线是「码本身不可猜」，限流只是防住
    // 「已经拿到码的人在猜口令」这一种情况。
    let _ = &ip;
    if let Some(wait) = login_throttle::check(&share_id, "") {
        return Err(AppError::TooManyRequests(format!(
            "口令尝试次数过多，请在 {} 秒后重试",
            wait
        )));
    }

    let valid = share_service::verify_share_password(&pool, &share_id, &req.password).await?;

    if valid {
        // 口令正确：清空该分享的失败计数，并签发短时效访问凭证
        login_throttle::record_success(&share_id, "");
        let ticket = crypto::create_share_ticket(&config, &share_id, SHARE_TICKET_TTL_SECS);
        Ok(Json(json!({
            "success": true,
            "data": { "verified": true, "ticket": ticket },
            "error": null
        })))
    } else {
        login_throttle::record_failure(&share_id, "");
        Err(AppError::Unauthorized("密码错误".into()))
    }
}

/// GET /api/public/shares/:id/download 下载公开分享文件
pub async fn public_share_download(
    State(pool): State<SqlitePool>,
    State(config): State<Config>,
    Path(share_id): Path<String>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Result<Response, AppError> {
    // 先验证分享的有效性
    let share = share_service::validate_share(&pool, &share_id).await?;

    // 密码保护的分享必须提供有效访问凭证，否则拒绝系统内容
    if !share.password_hash.is_empty() && !check_share_ticket(&config, &share_id, &params) {
        return Err(AppError::Unauthorized("需要访问密码".into()));
    }

    // 获取文件信息（文件夹分享暂不支持直接下载）
    let file_id = share.file_id.ok_or_else(|| {
        AppError::BadRequest("此分享为文件夹分享，暂不支持直接下载".into())
    })?;
    let file = file_service::get_file_by_id(&pool, file_id).await?;
    let full_path = config.upload_dir.join(&file.stored_path);

    if !tokio::fs::try_exists(&full_path).await.unwrap_or(false) {
        return Err(AppError::NotFound("文件不存在".into()));
    }

    // 占用下载额度放在**文件确认可读之后、开流之前**。
    //
    // 原实现在开流之前就自增，且与 validate_share 的检查分属两个事务：
    //   · 并发下所有请求会同时读到「还有额度」而全部放行，max_downloads 被超发；
    //   · 客户端中途取消也会消耗掉一次额度（那时字节还没传出去多少）。
    // 现在 consume_download_slot 把「检查+占用」收进同一条 UPDATE，由数据库裁决，
    // 天然并发安全；而放在 try_exists 之后，文件真不存在时不会白白扣掉额度。
    share_service::consume_download_slot(&pool, &share_id).await?;

    let file_handle = tokio::fs::File::open(&full_path).await?;
    let file_size = file_handle.metadata().await.map(|m| m.len()).unwrap_or(0);
    let stream = ReaderStream::new(file_handle);
    let body = Body::from_stream(stream);
    let mime = mime_guess::from_path(&file.original_name)
        .first_or_octet_stream();

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, mime.as_ref())
        // 走 files 的统一构造：同时给 ASCII 回退与 RFC 5987 的 filename*，
        // 否则中文文件名在部分浏览器下会乱码或被截断。原先这里是裸
        // format!，与认证后的 download 行为不一致。
        .header(
            header::CONTENT_DISPOSITION,
            files::content_disposition("attachment", &file.original_name),
        )
        // 让前端能拿到 total 走真实进度
        .header(header::CONTENT_LENGTH, file_size)
        .body(body)
        .map_err(|_| AppError::Internal("响应构建失败".into()))?)
}

/// GET /api/public/shares/:id/media?thumb=1&preview=1 提供公开分享媒体文件
pub async fn public_share_media(
    State(pool): State<SqlitePool>,
    State(config): State<Config>,
    Path(share_id): Path<String>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Response, AppError> {
    // 验证分享的有效性
    let share = share_service::validate_share(&pool, &share_id).await?;

    // 密码保护的分享必须提供有效访问凭证，否则拒绝系统内容
    if !share.password_hash.is_empty() && !check_share_ticket(&config, &share_id, &params) {
        return Err(AppError::Unauthorized("需要访问密码".into()));
    }

    // 获取文件信息（文件夹分享暂不支持媒体预览）
    let file_id = share.file_id.ok_or_else(|| {
        AppError::BadRequest("此分享为文件夹分享，暂不支持媒体预览".into())
    })?;
    let file = file_service::get_file_by_id(&pool, file_id).await?;

    let is_thumb = params.get("thumb").map(|v| v.as_str()) == Some("1");
    let is_preview = params.get("preview").map(|v| v.as_str()) == Some("1");

    // 响应头策略**以用户的原始文件名为准**，与认证后的 serve_media 完全一致。
    //
    // 之前这里用 serve_path 推导 MIME，且不设 Content-Disposition（等价于
    // inline 渲染），而 serve_path 在回退分支里是 stored_path —— 后缀来自
    // 用户可控的 original_name。后果是：上传一张内容为 HTML 的 .jpg，
    // 改名成 .xml / .txt 之类未被黑名单覆盖的后缀后创建公开分享，
    // 访客访问 /media 会拿到 Content-Type: text/xml + 内联渲染。
    // nosniff 拦不住这种情况（text/xml 是精确 MIME），而这是**无鉴权接口**。
    //
    // 真正的边界是 files::is_inline_safe 白名单：不在表内的一律降级为
    // attachment + application/octet-stream，浏览器只落盘、不渲染。
    let (content_type, disposition) = files::inline_response_policy(&file.original_name);

    // 确定要提供哪个文件
    let serve_path = if is_thumb {
        file.thumb_path.as_ref().or(file.preview_path.as_ref())
    } else if is_preview {
        file.preview_path.as_ref()
    } else {
        // 默认：优先提供预览图，否则使用原始文件
        file.preview_path.as_ref()
    };

    let serve_path = match serve_path {
        Some(p) => p,
        None => {
            // 没有预览图时回退到原文件。**先按白名单判断能不能内联**：
            // 不可内联的类型直接拒绝，而不是返回一个必然被降级下载的响应。
            if disposition == "attachment" {
                return Err(AppError::NotFound("预览不可用".into()));
            }
            &file.stored_path
        }
    };

    let full_path = config.upload_dir.join(serve_path);
    if !tokio::fs::try_exists(&full_path).await.unwrap_or(false) {
        return Err(AppError::NotFound("预览文件不存在".into()));
    }

    let file_handle = tokio::fs::File::open(&full_path).await?;
    let stream = ReaderStream::new(file_handle);
    let body = Body::from_stream(stream);

    let builder = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(
            header::CONTENT_DISPOSITION,
            files::content_disposition(disposition, &file.original_name),
        )
        .header(header::CACHE_CONTROL, "public, max-age=3600");

    // 降级为下载的内容额外加 CSP 沙箱。与 serve_media 同一套理由：
    // attachment 响应浏览器不会渲染，但沙箱是第二道保险，且不影响
    // 白名单内类型的正常预览。
    let builder = if disposition == "attachment" {
        builder.header("content-security-policy", "sandbox; default-src 'none'")
    } else {
        builder
    };

    files::harden(builder)
        .body(body)
        .map_err(|_| AppError::Internal("响应构建失败".into()))
}
