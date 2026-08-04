use axum::{
    body::Body,
    extract::{Path, State},
    http::{header, StatusCode},
    response::Response,
    Json,
};
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
    pub file_id: i64,
    pub expires_hours: Option<i64>,
    pub password: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct VerifyShareRequest {
    pub password: String,
}

// ========== Authenticated share endpoints ==========

/// POST /api/shares
pub async fn create_share(
    State(pool): State<SqlitePool>,
    auth: AuthUser,
    Json(req): Json<CreateShareRequest>,
) -> Result<Json<Value>, AppError> {
    let share = share_service::create_share(
        &pool,
        req.file_id,
        auth.user_id,
        req.expires_hours,
        req.password,
    )
    .await?;

    let info = share_service::get_share(&pool, &share.id, auth.user_id).await?;

    Ok(Json(json!({
        "success": true,
        "data": info,
        "error": null
    })))
}

/// GET /api/shares
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

/// GET /api/shares/:id
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

/// DELETE /api/shares/:id
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

// ========== Public share endpoints (no auth required) ==========

/// GET /api/public/shares/:id
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

/// POST /api/public/shares/:id/verify
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

/// GET /api/public/shares/:id/download
pub async fn public_share_download(
    State(pool): State<SqlitePool>,
    State(config): State<Config>,
    Path(share_id): Path<String>,
) -> Result<Response, AppError> {
    // Validate share first
    let share = share_service::validate_share(&pool, &share_id).await?;

    // Get the file
    let file = file_service::get_file_by_id(&pool, share.file_id).await?;
    let full_path = config.upload_dir.join(&file.stored_path);

    if !tokio::fs::try_exists(&full_path).await.unwrap_or(false) {
        return Err(AppError::NotFound("文件不存在".into()));
    }

    // Increment download count
    share_service::increment_download_count(&pool, &share_id).await?;

    let file_data = tokio::fs::read(&full_path).await?;
    let mime = mime_guess::from_path(&file.original_name)
        .first_or_octet_stream();

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, mime.as_ref())
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{}\"", file.original_name),
        )
        .body(Body::from(file_data))
        .unwrap())
}

/// GET /api/public/shares/:id/media?thumb=1&preview=1
pub async fn public_share_media(
    State(pool): State<SqlitePool>,
    State(config): State<Config>,
    Path(share_id): Path<String>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Response, AppError> {
    // Validate share
    let share = share_service::validate_share(&pool, &share_id).await?;

    // Get the file
    let file = file_service::get_file_by_id(&pool, share.file_id).await?;

    let is_thumb = params.get("thumb").map(|v| v.as_str()) == Some("1");
    let is_preview = params.get("preview").map(|v| v.as_str()) == Some("1");

    // Determine which file to serve
    let serve_path = if is_thumb {
        file.thumb_path.as_ref().or(file.preview_path.as_ref())
    } else if is_preview {
        file.preview_path.as_ref()
    } else {
        // Default: serve preview if available, otherwise original
        file.preview_path.as_ref()
    };

    let serve_path = match serve_path {
        Some(p) => p,
        None => {
            // Fallback to original file for image formats
            let ext = std::path::Path::new(&file.original_name)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_lowercase();
            let image_formats = ["jpg", "jpeg", "png", "gif", "bmp", "webp", "tiff", "tif"];
            if image_formats.contains(&ext.as_str()) {
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

    let file_data = tokio::fs::read(&full_path).await?;
    let mime = mime_guess::from_path(serve_path)
        .first_or_octet_stream();

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, mime.as_ref())
        .header(header::CACHE_CONTROL, "public, max-age=3600")
        .body(Body::from(file_data))
        .unwrap())
}