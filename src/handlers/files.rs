use axum::{
    body::Body,
    extract::{Multipart, Path, Query, State},
    http::{HeaderMap, header, StatusCode},
    response::Response,
    Json,
};
use futures_util::{StreamExt, TryStreamExt};
use tokio_util::io::ReaderStream;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::{Path as StdPath, PathBuf};
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::sync::Semaphore;
use uuid::Uuid;

use crate::config::Config;
use crate::errors::AppError;
use crate::middleware::auth::AuthUser;
use crate::models::file::File;
use crate::services::{file_service, preview_service};
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

/// RAII 守卫：axum 在客户端断开/请求中断时会 drop handler 的 future，
/// 此处 Drop 会立即清理未提交的临时 .part 文件，避免残留半写文件。
struct PartialGuard {
    path: Option<PathBuf>,
}

impl Drop for PartialGuard {
    fn drop(&mut self) {
        if let Some(p) = self.path.take() {
            let _ = std::fs::remove_file(&p);
        }
    }
}

/// multipart 解析过程中已流式写盘的待提交文件（暂存为 .part，提交时 rename）
struct PendingUpload {
    guard: PartialGuard, // 持有临时 .part 路径，rename 提交后置 None
    stored_name: String, // {uuid}.{ext}
    file_name: String,
    size: i64,
    ext: String,
}

/// POST /api/files/upload 上传文件
/// 采用 multipart 流式写盘：对每个 file 字段用 bytes_stream() 逐块写入，
/// 字段内 chunk 计数作为单文件权威限制（max_file_size）；
/// 请求总 Content-Length 做粗预检（> max_file_size 直接 413）。
pub async fn upload_files(
    State(pool): State<SqlitePool>,
    State(config): State<Config>,
    State(sem): State<Arc<Semaphore>>,
    auth: AuthUser,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Result<Json<Value>, AppError> {
    // 粗预检：全请求 Content-Length 超限立即 413，省得传完才被 Limited 掐断。
    // 注意这是全请求总量预算；单文件权威限制由字段内 chunk 计数承担。
    if let Some(cl) = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
    {
        if cl > config.max_file_size {
            return Err(AppError::PayloadTooLarge("上传总量超过限制".into()));
        }
    }

    let mut folder_id: Option<i64> = None;
    let mut explicit_user: Option<i64> = None;
    let mut pending: Vec<PendingUpload> = Vec::new();
    let mut uploaded_files: Vec<Value> = Vec::new();
    let mut errors: Vec<String> = Vec::new();

    // 临时落盘根目录（与 user_* 同一文件系统，rename 原子提交）
    let temp_root = config.upload_dir.join(".tmp_incoming");
    tokio::fs::create_dir_all(&temp_root).await?;

    // 阶段 1：收集元数据字段；对 file 字段连续流式写盘到 .part，不整体缓冲
    while let Ok(Some(field)) = multipart.next_field().await {
        let name = field.name().unwrap_or("").to_string();

        if name == "folder_id" {
            let text = field.text().await.unwrap_or_default();
            if !text.is_empty() {
                folder_id = text.parse::<i64>().ok();
            }
        } else if name == "user_id" {
            let text = field.text().await.unwrap_or_default();
            if !text.is_empty() {
                explicit_user = text.parse::<i64>().ok();
            }
        } else if name == "file" {
            let file_name = field.file_name().unwrap_or("unknown").to_string();
            if file_name.is_empty() {
                continue;
            }

            // 扩展名校验：廉价且前置，避免为不合法类型写盘
            if let Err(e) = file_service::validate_extension(&file_name) {
                errors.push(e.message().to_string());
                continue;
            }

            let tmp_path = temp_root.join(format!("{}.part", Uuid::new_v4().simple()));
            let guard = PartialGuard {
                path: Some(tmp_path.clone()),
            };
            let mut out = tokio::fs::File::create(&tmp_path).await?;

            // 字段内 chunk 计数：单文件权威限制 = max_file_size
            let mut size: u64 = 0;
            let mut too_large = false;
            let mut stream = field.into_stream();
            while let Some(chunk) = stream.next().await {
                let chunk =
                    chunk.map_err(|_| AppError::BadRequest("读取文件数据失败".into()))?;
                size += chunk.len() as u64;
                if size > config.max_file_size {
                    too_large = true;
                    break;
                }
                out.write_all(&chunk).await?;
            }

            if too_large {
                errors.push(format!("文件 \"{}\" 大小超过限制", file_name));
                // guard Drop 清理 .part
                continue;
            }

            out.flush().await?;
            drop(out); // 关闭句柄，确保后续 rename 成功

            let ext = StdPath::new(&file_name)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_lowercase()
                .to_string();

            pending.push(PendingUpload {
                guard,
                stored_name: file_service::generate_stored_filename(&file_name),
                file_name,
                size: size as i64,
                ext,
            });
        }
    }

    // 确定上传归属用户：默认当前登录用户；若指定 user_id，则仅管理员可为他人上传
    let owner_id: i64 = if let Some(target) = explicit_user {
        let role: Option<(String,)> = sqlx::query_as("SELECT role FROM users WHERE id = ?")
            .bind(auth.user_id)
            .fetch_optional(&pool)
            .await?;
        if role.as_ref().map(|r| r.0.as_str()) != Some("admin") {
            return Err(AppError::Forbidden("需要管理员权限才能为其他用户上传".into()));
        }
        let exists: Option<(i64,)> = sqlx::query_as("SELECT id FROM users WHERE id = ?")
            .bind(target)
            .fetch_optional(&pool)
            .await?;
        if exists.is_none() {
            return Err(AppError::NotFound("目标用户不存在".into()));
        }
        target
    } else {
        auth.user_id
    };

    // 阶段 2：提交已流式落盘的待处理文件（.part -> rename 原子提交 + INSERT）
    for pu in pending {
        // 重复检查（此时已能确定 owner/folder）
        if file_service::check_duplicates(&pool, owner_id, folder_id, &pu.file_name).await? {
            errors.push(format!("文件 \"{}\" 已存在，已跳过", pu.file_name));
            // pu 的 guard Drop 移除临时 .part
            continue;
        }

        let user_dir = file_service::user_upload_dir(&config, owner_id);
        tokio::fs::create_dir_all(&user_dir).await?;

        let stored_path = format!("user_{}/{}", owner_id, pu.stored_name);
        let full_path = config.upload_dir.join(&stored_path);

        // 同文件系统下 rename 原子提交
        let src = pu.guard.path.clone().ok_or_else(|| {
            AppError::Internal("内部状态错误：临时文件路径缺失".into())
        })?;
        tokio::fs::rename(&src, &full_path).await?;

        // 先写库（preview_path/thumb_path = NULL），缩略图在后台异步补齐
        let insert = sqlx::query_as::<_, File>(
            r#"INSERT INTO files (name, original_name, stored_path, preview_path, thumb_path, owner_id, folder_id, size, file_type)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING *"#,
        )
        .bind(&pu.file_name)
        .bind(&pu.file_name)
        .bind(&stored_path)
        .bind(Option::<String>::None) // preview_path 由后台任务补齐
        .bind(Option::<String>::None) // thumb_path 由后台任务补齐
        .bind(owner_id)
        .bind(folder_id)
        .bind(pu.size)
        .bind(&pu.ext)
        .fetch_one(&pool)
        .await;

        let file_record = match insert {
            Ok(r) => r,
            Err(e) => {
                // DB 写入失败：删除刚重命名的物理文件，避免产生无记录孤儿文件；
                // 若本步也失败，则交由周期 GC（sweeper）兜底清理。
                let _ = tokio::fs::remove_file(&full_path).await;
                return Err(e.into());
            }
        };

        // DB 写入成功后才标记守卫勿删，避免 Drop 误删已提交文件
        let mut guard = pu.guard;
        guard.path = None;

        // 后台生成预览图 + 缩略图（spawn_blocking 包裹同步图像处理 + 信号量限并发）
        if file_service::supports_preview(&pu.ext) {
            spawn_preview_task(
                sem.clone(),
                pool.clone(),
                config.clone(),
                owner_id,
                file_record.id,
                full_path,
                pu.ext.clone(),
            );
        }

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

/// 后台生成某文件预览图与缩略图：
/// 先经信号量限并发，再通过 spawn_blocking 跑同步图像处理，最后 UPDATE files 表。
///
/// 丢失/失败不影响已上传文件本身（media 接口已有回退到原图的降级逻辑）。
fn spawn_preview_task(
    sem: Arc<Semaphore>,
    pool: SqlitePool,
    config: Config,
    owner_id: i64,
    file_id: i64,
    src_path: PathBuf,
    ext: String,
) {
    tokio::spawn(async move {
        let Ok(permit) = sem.acquire_owned().await else {
            tracing::warn!("预览并发闸未获许可: file_id={}", file_id);
            return;
        };

        let res = tokio::task::spawn_blocking(move || {
            preview_service::generate_preview_and_thumb(&config, owner_id, &src_path, &ext)
        })
        .await;

        match res {
            Ok((preview_rel, thumb_rel)) => {
                let _ = sqlx::query(
                    "UPDATE files SET preview_path = ?, thumb_path = ? WHERE id = ?",
                )
                .bind(preview_rel)
                .bind(thumb_rel)
                .bind(file_id)
                .execute(&pool)
                .await;
            }
            Err(e) => tracing::warn!("预览后台任务失败: file_id={} err={:?}", file_id, e),
        }
        // permit 于作用域结束自动归还
        drop(permit);
    });
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
    let file_size = file_handle.metadata().await.map(|m| m.len()).unwrap_or(0);
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
        // 显式声明 Content-Length，前端 fetch/axios 才能拿到 total 走真实进度
        .header(header::CONTENT_LENGTH, file_size)
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
                    .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
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
        // 防止响应内容被嗅探成 text/html，堵塞预览相关的 MIME 混淆 XSS
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
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
    auth: AuthUser,
    Path(file_id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    file_service::delete_file(&pool, file_id, auth.user_id).await?;
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
    State(sem): State<Arc<Semaphore>>,
    auth: AuthUser,
) -> Result<Json<Value>, AppError> {
    let file_count = file_service::empty_trash(&pool, auth.user_id).await?;

    // 触发一次即时 GC，立即释放本次硬删产生的磁盘空间（无需等周期任务）
    tokio::spawn(async move {
        if let Err(e) = crate::services::sweeper::run_once(&pool, &config, sem.clone()).await {
            tracing::warn!("清空回收站后的即时 GC 失败: {:?}", e);
        }
    });

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
