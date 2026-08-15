use axum::{
    body::Body,
    extract::{Path, State},
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
use sqlx::SqlitePool;

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

/// POST /api/public/shares/:id/verify 验证分享密码
pub async fn public_verify_password(
    State(pool): State<SqlitePool>,
    Path(share_id): Path<String>,
    Json(req): Json<VerifyShareRequest>,
) -> Result<Json<Value>, AppError> {
    let valid = share_service::verify_share_password(&pool, &share_id, &req.password).await?;

    if valid {
        Ok(Json(json!({
            "success": true,
            "data": { "verified": true },
            "error": null
        })))
    } else {
        Err(AppError::Unauthorized("密码错误".into()))
    }
}

/// GET /api/public/shares/:id/download 下载公开分享文件
pub async fn public_share_download(
    State(pool): State<SqlitePool>,
    State(config): State<Config>,
    Path(share_id): Path<String>,
) -> Result<Response, AppError> {
    // 先验证分享的有效性
    let share = share_service::validate_share(&pool, &share_id).await?;

    // 获取文件信息（文件夹分享暂不支持直接下载）
    let file_id = share.file_id.ok_or_else(|| {
        AppError::BadRequest("此分享为文件夹分享，暂不支持直接下载".into())
    })?;
    let file = file_service::get_file_by_id(&pool, file_id).await?;
    let full_path = config.upload_dir.join(&file.stored_path);

    if !tokio::fs::try_exists(&full_path).await.unwrap_or(false) {
        return Err(AppError::NotFound("文件不存在".into()));
    }

    // 增加下载次数
    share_service::increment_download_count(&pool, &share_id).await?;

    let file_handle = tokio::fs::File::open(&full_path).await?;
    let stream = ReaderStream::new(file_handle);
    let body = Body::from_stream(stream);
    let mime = mime_guess::from_path(&file.original_name)
        .first_or_octet_stream();

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, mime.as_ref())
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{}\"", file.original_name),
        )
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

    // 获取文件信息（文件夹分享暂不支持媒体预览）
    let file_id = share.file_id.ok_or_else(|| {
        AppError::BadRequest("此分享为文件夹分享，暂不支持媒体预览".into())
    })?;
    let file = file_service::get_file_by_id(&pool, file_id).await?;

    let is_thumb = params.get("thumb").map(|v| v.as_str()) == Some("1");
    let is_preview = params.get("preview").map(|v| v.as_str()) == Some("1");

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
            // 对于图片和 PDF 格式，回退到原始文件
            let ext = std::path::Path::new(&file.original_name)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_lowercase();
            let inline_formats = ["jpg", "jpeg", "png", "gif", "bmp", "webp", "tiff", "tif", "pdf"];
            if inline_formats.contains(&ext.as_str()) {
                &file.stored_path
            } else {
                return Err(AppError::NotFound("预览不可用".into()));
            }
        }
    };

    let full_path = config.upload_dir.join(serve_path);
    if !tokio::fs::try_exists(&full_path).await.unwrap_or(false) {
        return Err(AppError::NotFound("预览文件不存在".into()));
    }

    let file_handle = tokio::fs::File::open(&full_path).await?;
    let stream = ReaderStream::new(file_handle);
    let body = Body::from_stream(stream);
    let mime = mime_guess::from_path(serve_path)
        .first_or_octet_stream();

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, mime.as_ref())
        .header(header::CACHE_CONTROL, "public, max-age=3600")
        .body(body)
        .map_err(|_| AppError::Internal("响应构建失败".into()))?)
}
