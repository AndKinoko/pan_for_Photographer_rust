use axum::{
    body::Body,
    extract::{Multipart, Path, Query, State},
    http::{header, StatusCode},
    response::Response,
    Json,
};
use tokio_util::io::ReaderStream;
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

/// GET /api/files?folder_id={id} 获取文件列表
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

/// 在 multipart 解析过程中收集的待处理文件数据
struct PendingFile {
    file_name: String,
    data: Vec<u8>,
}

/// POST /api/files/upload 上传文件
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

    // 阶段 1：收集所有元数据字段并缓存文件数据
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

    // 阶段 2：使用收集的元数据处理所有缓存的文件
    for pf in pending_files {
        let file_name = pf.file_name;
        let data = pf.data;

        // 检查重复文件
        if file_service::check_duplicates(&pool, auth.user_id, folder_id, &file_name).await? {
            errors.push(format!("文件 \"{}\" 已存在，已跳过", file_name));
            continue;
        }

        // 验证文件扩展名
        if let Err(e) = file_service::validate_extension(&file_name) {
            errors.push(e.message().to_string());
            continue;
        }

        // 检查文件大小
        if data.len() as u64 > config.max_file_size {
            errors.push(format!("文件 \"{}\" 大小超过限制", file_name));
            continue;
        }

        // 生成存储文件名
        let stored_name = file_service::generate_stored_filename(&file_name);
        let user_dir = file_service::user_upload_dir(&config, auth.user_id);
        tokio::fs::create_dir_all(&user_dir).await?;

        let stored_path = format!("user_{}/{}", auth.user_id, stored_name);
        let full_path = config.upload_dir.join(&stored_path);

        // 写入文件
        let mut file = tokio::fs::File::create(&full_path).await?;
        file.write_all(&data).await?;

        // 确定文件类型
        let ext = StdPath::new(&file_name)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();

        let size = data.len() as i64;

        // 生成预览图（大，1616x1080）和缩略图（小，360x240）
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

        // 保存到数据库
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

/// GET /api/files/:id/download?token=<jwt> 下载文件
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

    let file_handle = tokio::fs::File::open(&full_path).await?;
    let stream = ReaderStream::new(file_handle);
    let body = Body::from_stream(stream);
    let mime = mime_guess::from_path(&file.original_name)
        .first_or_octet_stream();

    let response = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, mime.as_ref())
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{}\"", file.original_name),
        )
        .body(body)
        .map_err(|_| AppError::Internal("响应构建失败".into()))?;

    Ok(response)
}

/// GET /api/files/:id/media?preview=1&token=<jwt> 提供媒体文件服务
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

    // 提供缩略图（小，360x240）用于文件列表图标
    if is_thumb {
        if let Some(ref thumb_path) = file.thumb_path {
            let full_path = config.upload_dir.join(thumb_path);
            if tokio::fs::try_exists(&full_path).await.unwrap_or(false) {
                let file_handle = tokio::fs::File::open(&full_path).await?;
                let stream = ReaderStream::new(file_handle);
                let body = Body::from_stream(stream);
                return Ok(Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "image/jpeg")
                    .body(body)
                    .map_err(|_| AppError::Internal("响应构建失败".into()))?);
            }
        }
        // 如果缩略图不存在，回退到预览图
        if let Some(ref preview_path) = file.preview_path {
            let full_path = config.upload_dir.join(preview_path);
            if tokio::fs::try_exists(&full_path).await.unwrap_or(false) {
                let file_handle = tokio::fs::File::open(&full_path).await?;
                let stream = ReaderStream::new(file_handle);
                let body = Body::from_stream(stream);
                return Ok(Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "image/jpeg")
                    .body(body)
                    .map_err(|_| AppError::Internal("响应构建失败".into()))?);
            }
        }
        return Err(AppError::NotFound("缩略图不存在".into()));
    }

    if is_preview {
        if let Some(ref preview_path) = file.preview_path {
            let full_path = config.upload_dir.join(preview_path);
            if tokio::fs::try_exists(&full_path).await.unwrap_or(false) {
                let file_handle = tokio::fs::File::open(&full_path).await?;
                let stream = ReaderStream::new(file_handle);
                let body = Body::from_stream(stream);
                return Ok(Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "image/jpeg")
                    .body(body)
                    .map_err(|_| AppError::Internal("响应构建失败".into()))?);
            }
        }
        // 预览图文件缺失时，对支持的图片格式回退到原始文件
        let ext = std::path::Path::new(&file.original_name)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();
        let image_formats = ["jpg", "jpeg", "png", "gif", "bmp", "webp", "tiff", "tif"];
        if image_formats.contains(&ext.as_str()) {
            let full_path = config.upload_dir.join(&file.stored_path);
            if tokio::fs::try_exists(&full_path).await.unwrap_or(false) {
                let file_handle = tokio::fs::File::open(&full_path).await?;
                let stream = ReaderStream::new(file_handle);
                let body = Body::from_stream(stream);
                let mime = mime_guess::from_path(&file.original_name)
                    .first_or_octet_stream();
                return Ok(Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, mime.as_ref())
                    .body(body)
                    .map_err(|_| AppError::Internal("响应构建失败".into()))?);
            }
        }
        return Err(AppError::NotFound("预览文件不存在".into()));
    }

    // 提供原始文件内联显示
    let full_path = config.upload_dir.join(&file.stored_path);
    if !tokio::fs::try_exists(&full_path).await.unwrap_or(false) {
        return Err(AppError::NotFound("文件不存在".into()));
    }

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
            format!("inline; filename=\"{}\"", file.original_name),
        )
        .body(body)
        .map_err(|_| AppError::Internal("响应构建失败".into()))?)
}

/// DELETE /api/files/:id 删除文件（软删除，移入回收站）
pub async fn delete_file(
    State(pool): State<SqlitePool>,
    auth: AuthUser,
    Path(file_id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    file_service::soft_delete_file(&pool, file_id, auth.user_id).await?;
    Ok(Json(json!({
        "success": true,
        "data": null,
        "error": null
    })))
}

/// PUT /api/files/:id/rename 重命名文件
pub async fn rename_file(
    State(pool): State<SqlitePool>,
    auth: AuthUser,
    Path(file_id): Path<i64>,
    Json(req): Json<RenameRequest>,
) -> Result<Json<Value>, AppError> {
    let file = file_service::rename_file(&pool, file_id, auth.user_id, &req.name).await?;
    Ok(Json(json!({
        "success": true,
        "data": file.to_info(),
        "error": null
    })))
}

/// POST /api/files/:id/restore 从回收站恢复文件
pub async fn restore_file(
    State(pool): State<SqlitePool>,
    auth: AuthUser,
    Path(file_id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    file_service::restore_file(&pool, file_id, auth.user_id).await?;
    Ok(Json(json!({
        "success": true,
        "data": null,
        "error": null
    })))
}

/// DELETE /api/files/:id/permanent 永久删除文件（从回收站）
pub async fn permanent_delete_file(
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

/// GET /api/trash 获取回收站内容（文件+文件夹）
pub async fn list_trash(
    State(pool): State<SqlitePool>,
    auth: AuthUser,
) -> Result<Json<Value>, AppError> {
    let files = file_service::list_trash_files(&pool, auth.user_id).await?;
    let folders = crate::services::folder_service::list_trash_folders(&pool, auth.user_id).await?;
    Ok(Json(json!({
        "success": true,
        "data": {
            "files": files,
            "folders": folders,
        },
        "error": null
    })))
}

/// DELETE /api/trash 清空回收站
pub async fn empty_trash(
    State(pool): State<SqlitePool>,
    State(config): State<Config>,
    auth: AuthUser,
) -> Result<Json<Value>, AppError> {
    let file_count = file_service::empty_trash(&pool, &config, auth.user_id).await?;
    Ok(Json(json!({
        "success": true,
        "data": {
            "deleted_count": file_count,
        },
        "error": null
    })))
}

#[derive(Debug, Deserialize)]
pub struct RenameRequest {
    pub name: String,
}
