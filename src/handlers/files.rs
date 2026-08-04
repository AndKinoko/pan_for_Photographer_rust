use axum::{
    body::Body,
    extract::{Multipart, Path, Query, State},
    http::{header, StatusCode},
    response::Response,
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::Path as StdPath;
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

use crate::config::Config;
use crate::errors::AppError;
use crate::middleware::auth::AuthUser;
use crate::models::file::File;
use crate::services::{file_service, preview_service};
use crate::services::file_service::supports_preview;
use sqlx::SqlitePool;

#[derive(Debug, Deserialize)]
pub struct FileListQuery {
    pub folder_id: Option<i64>,
}

/// GET /api/files?folder_id={id}
pub async fn list_files(
    State(pool): State<SqlitePool>,
    auth: AuthUser,
    Query(query): Query<FileListQuery>,
) -> Result<Json<Value>, AppError> {
    let files = file_service::list_files(&pool, auth.user_id, query.folder_id).await?;
    Ok(Json(json!({
        "success": true,
        "data": files,
        "error": null
    })))
}

/// Pending file data collected during multipart parsing before processing
struct PendingFile {
    file_name: String,
    data: Vec<u8>,
}

/// POST /api/files/upload
pub async fn upload_files(
    State(pool): State<SqlitePool>,
    State(config): State<Config>,
    auth: AuthUser,
    mut multipart: Multipart,
) -> Result<Json<Value>, AppError> {
    let mut folder_id: Option<i64> = None;
    let mut pending_files: Vec<PendingFile> = Vec::new();
    let mut uploaded_files: Vec<Value> = Vec::new();
    let mut errors: Vec<String> = Vec::new();

    // Phase 1: Collect all metadata fields and buffer file data
    while let Ok(Some(field)) = multipart.next_field().await {
        let name = field.name().unwrap_or("").to_string();

        if name == "folder_id" {
            let text = field.text().await.unwrap_or_default();
            if !text.is_empty() {
                folder_id = text.parse::<i64>().ok();
            }
        } else if name == "file" {
            let file_name = field.file_name().unwrap_or("unknown").to_string();
            if file_name.is_empty() {
                continue;
            }

            let data = field.bytes().await.map_err(|_| {
                AppError::BadRequest("读取文件数据失败".into())
            })?;

            pending_files.push(PendingFile { file_name, data: data.to_vec() });
        }
    }

    // Phase 2: Process all buffered files with collected metadata
    for pf in pending_files {
        let file_name = pf.file_name;
        let data = pf.data;

        // Check duplicate
        if file_service::check_duplicates(&pool, auth.user_id, folder_id, &file_name).await? {
            errors.push(format!("文件 \"{}\" 已存在，已跳过", file_name));
            continue;
        }

        // Validate extension
        if let Err(e) = file_service::validate_extension(&file_name) {
            errors.push(e.message().to_string());
            continue;
        }

        // Check size
        if data.len() as u64 > config.max_file_size {
            errors.push(format!("文件 \"{}\" 大小超过限制", file_name));
            continue;
        }

        // Generate stored filename
        let stored_name = file_service::generate_stored_filename(&file_name);
        let user_dir = file_service::user_upload_dir(&config, auth.user_id);
        tokio::fs::create_dir_all(&user_dir).await?;

        let stored_path = format!("user_{}/{}", auth.user_id, stored_name);
        let full_path = config.upload_dir.join(&stored_path);

        // Write file
        let mut file = tokio::fs::File::create(&full_path).await?;
        file.write_all(&data).await?;

        // Determine file type
        let ext = StdPath::new(&file_name)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();

        let size = data.len() as i64;

        // Generate preview (large, 1616x1080) and thumbnail (small, 360x240)
        let (preview_path, thumb_path) = if supports_preview(&ext) {
            let preview_dir = file_service::user_preview_dir(&config, auth.user_id);
            tokio::fs::create_dir_all(&preview_dir).await?;

            let preview_name = format!("{}.jpg", Uuid::new_v4().simple());
            let preview_rel = format!("user_{}/previews/{}", auth.user_id, preview_name);
            let preview_full = config.upload_dir.join(&preview_rel);

            let thumb_name = format!("{}_thumb.jpg", Uuid::new_v4().simple());
            let thumb_rel = format!("user_{}/previews/{}", auth.user_id, thumb_name);
            let thumb_full = config.upload_dir.join(&thumb_rel);

            let preview_result = preview_service::generate_preview(&full_path, &preview_full, &ext).await;
            let thumb_result = preview_service::generate_thumbnail(&full_path, &thumb_full, &ext).await;

            match (preview_result, thumb_result) {
                (Ok(()), Ok(())) => (Some(preview_rel), Some(thumb_rel)),
                (Ok(()), Err(e)) => {
                    tracing::warn!("Thumbnail generation failed: {:?}", e);
                    let _ = tokio::fs::remove_file(&thumb_full).await;
                    (Some(preview_rel), None)
                }
                (Err(e), Ok(())) => {
                    tracing::warn!("Preview generation failed: {:?}", e);
                    let _ = tokio::fs::remove_file(&preview_full).await;
                    (None, Some(thumb_rel))
                }
                (Err(e1), Err(e2)) => {
                    tracing::warn!("Preview generation failed: {:?}, thumbnail: {:?}", e1, e2);
                    let _ = tokio::fs::remove_file(&preview_full).await;
                    let _ = tokio::fs::remove_file(&thumb_full).await;
                    (None, None)
                }
            }
        } else {
            (None, None)
        };

        // Save to database
        let file_record = sqlx::query_as::<_, File>(
            r#"INSERT INTO files (name, original_name, stored_path, preview_path, thumb_path, owner_id, folder_id, size, file_type)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING *"#,
        )
        .bind(&file_name)
        .bind(&file_name)
        .bind(&stored_path)
        .bind(&preview_path)
        .bind(&thumb_path)
        .bind(auth.user_id)
        .bind(folder_id)
        .bind(size)
        .bind(&ext)
        .fetch_one(&pool)
        .await?;

        let info = file_record.to_info();
        uploaded_files.push(serde_json::to_value(info)?);
    }

    if uploaded_files.is_empty() && !errors.is_empty() {
        return Err(AppError::BadRequest(errors.join("; ")));
    }

    Ok(Json(json!({
        "success": true,
        "data": {
            "files": uploaded_files,
            "errors": errors,
            "count": uploaded_files.len(),
        },
        "error": null
    })))
}

/// GET /api/files/:id/download?token=<jwt>
pub async fn download_file(
    State(pool): State<SqlitePool>,
    State(config): State<Config>,
    auth: Option<AuthUser>,
    Path(file_id): Path<i64>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Result<Response, AppError> {
    let user_id = if let Some(user) = auth {
        user.user_id
    } else if let Some(token) = params.get("token") {
        crate::utils::crypto::validate_token(token, &config)
            .map_err(|_| AppError::Unauthorized("无效的访问令牌".into()))?
            .sub
    } else {
        return Err(AppError::Unauthorized("请先登录".into()));
    };

    let file = file_service::get_file(&pool, file_id, user_id).await?;
    let full_path = config.upload_dir.join(&file.stored_path);

    if !tokio::fs::try_exists(&full_path).await.unwrap_or(false) {
        return Err(AppError::NotFound("文件不存在".into()));
    }

    let file_data = tokio::fs::read(&full_path).await?;
    let mime = mime_guess::from_path(&file.original_name)
        .first_or_octet_stream();

    let response = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, mime.as_ref())
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{}\"", file.original_name),
        )
        .body(Body::from(file_data))
        .unwrap();

    Ok(response)
}

/// GET /api/files/:id/media?preview=1&token=<jwt>
pub async fn serve_media(
    State(pool): State<SqlitePool>,
    State(config): State<Config>,
    auth: Option<AuthUser>,
    Path(file_id): Path<i64>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Result<Response, AppError> {
    let user_id = if let Some(user) = auth {
        user.user_id
    } else if let Some(token) = params.get("token") {
        crate::utils::crypto::validate_token(token, &config)
            .map_err(|_| AppError::Unauthorized("无效的访问令牌".into()))?
            .sub
    } else {
        return Err(AppError::Unauthorized("请先登录".into()));
    };

    let file = file_service::get_file(&pool, file_id, user_id).await?;
    let is_preview = params.get("preview").map(|v| v == "1").unwrap_or(false);
    let is_thumb = params.get("thumb").map(|v| v == "1").unwrap_or(false);

    // Serve thumbnail (small, 360x240) for file list icons
    if is_thumb {
        if let Some(ref thumb_path) = file.thumb_path {
            let full_path = config.upload_dir.join(thumb_path);
            if tokio::fs::try_exists(&full_path).await.unwrap_or(false) {
                let data = tokio::fs::read(&full_path).await?;
                return Ok(Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "image/jpeg")
                    .body(Body::from(data))
                    .unwrap());
            }
        }
        // Fallback to preview if thumb doesn't exist
        if let Some(ref preview_path) = file.preview_path {
            let full_path = config.upload_dir.join(preview_path);
            if tokio::fs::try_exists(&full_path).await.unwrap_or(false) {
                let data = tokio::fs::read(&full_path).await?;
                return Ok(Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "image/jpeg")
                    .body(Body::from(data))
                    .unwrap());
            }
        }
        return Err(AppError::NotFound("缩略图不存在".into()));
    }

    if is_preview {
        if let Some(ref preview_path) = file.preview_path {
            let full_path = config.upload_dir.join(preview_path);
            if tokio::fs::try_exists(&full_path).await.unwrap_or(false) {
                let data = tokio::fs::read(&full_path).await?;
                return Ok(Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "image/jpeg")
                    .body(Body::from(data))
                    .unwrap());
            }
        }
        return Err(AppError::NotFound("预览文件不存在".into()));
    }

    // Serve original file inline
    let full_path = config.upload_dir.join(&file.stored_path);
    if !tokio::fs::try_exists(&full_path).await.unwrap_or(false) {
        return Err(AppError::NotFound("文件不存在".into()));
    }

    let file_data = tokio::fs::read(&full_path).await?;
    let mime = mime_guess::from_path(&file.original_name)
        .first_or_octet_stream();

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, mime.as_ref())
        .header(
            header::CONTENT_DISPOSITION,
            format!("inline; filename=\"{}\"", file.original_name),
        )
        .body(Body::from(file_data))
        .unwrap())
}

/// DELETE /api/files/:id
pub async fn delete_file(
    State(pool): State<SqlitePool>,
    State(config): State<Config>,
    auth: AuthUser,
    Path(file_id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    file_service::delete_file(&pool, &config, file_id, auth.user_id).await?;
    Ok(Json(json!({
        "success": true,
        "data": null,
        "error": null
    })))
}