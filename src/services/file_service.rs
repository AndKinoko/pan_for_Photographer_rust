use std::path::{Path, PathBuf};
use std::time::Duration;
use uuid::Uuid;

use crate::config::Config;
use crate::errors::AppError;
use crate::models::file::{File, FileInfo};
use crate::models::folder::Folder;
use sqlx::SqlitePool;

/// 文件系统操作的最大重试次数
const MAX_RETRY_ATTEMPTS: u32 = 3;
/// 重试之间的基础延迟（毫秒）
const RETRY_BASE_DELAY_MS: u64 = 100;

/// 出于安全考虑被阻止的文件扩展名
const BLOCKED_EXTENSIONS: &[&str] = &[".html", ".htm", ".svg", ".js", ".mjs"];

/// 支持预览的图片格式
const IMAGE_FORMATS: &[&str] = &["jpg", "jpeg", "png", "gif", "bmp", "webp", "tiff", "tif"];

/// 支持通过原始处理预览的RAW格式
const RAW_FORMATS: &[&str] = &[
    "nef", "cr2", "cr3", "crw", "arw", "sr2", "srf", "dng", "raf", "orf", "rw2", "nrw",
];

/// 列出文件夹中的文件（如果 folder_id 为 None，则列出根目录下的文件）
/// 自动过滤已软删除的文件
pub async fn list_files(
    pool: &SqlitePool,
    owner_id: i64,
    folder_id: Option<i64>,
) -> Result<Vec<FileInfo>, AppError> {
    let files = if let Some(fid) = folder_id {
        // 验证文件夹属于当前用户且未删除
        let folder = sqlx::query_as::<_, Folder>(
            "SELECT * FROM folders WHERE id = ? AND owner_id = ? AND deleted_at IS NULL",
        )
        .bind(fid)
        .bind(owner_id)
        .fetch_optional(pool)
        .await?;

        if folder.is_none() {
            return Err(AppError::NotFound("文件夹不存在".into()));
        }

        sqlx::query_as::<_, File>(
            "SELECT * FROM files WHERE owner_id = ? AND folder_id = ? AND deleted_at IS NULL ORDER BY uploaded_at DESC",
        )
        .bind(owner_id)
        .bind(fid)
        .fetch_all(pool)
        .await?
    } else {
        sqlx::query_as::<_, File>(
            "SELECT * FROM files WHERE owner_id = ? AND folder_id IS NULL AND deleted_at IS NULL ORDER BY uploaded_at DESC",
        )
        .bind(owner_id)
        .fetch_all(pool)
        .await?
    };

    Ok(files.into_iter().map(|f| f.to_info()).collect())
}

/// 重命名文件
pub async fn rename_file(
    pool: &SqlitePool,
    file_id: i64,
    owner_id: i64,
    new_name: &str,
) -> Result<File, AppError> {
    let new_name = new_name.trim();
    if new_name.is_empty() {
        return Err(AppError::BadRequest("文件名不能为空".into()));
    }

    // 获取文件并验证所有权
    let file = get_file(pool, file_id, owner_id).await?;

    // 检查同名文件（排除自身）
    let existing = if let Some(fid) = file.folder_id {
        sqlx::query_scalar::<_, i64>(
            "SELECT id FROM files WHERE owner_id = ? AND folder_id = ? AND LOWER(original_name) = LOWER(?) AND id != ? AND deleted_at IS NULL",
        )
        .bind(owner_id)
        .bind(fid)
        .bind(new_name)
        .bind(file_id)
        .fetch_optional(pool)
        .await?
    } else {
        sqlx::query_scalar::<_, i64>(
            "SELECT id FROM files WHERE owner_id = ? AND folder_id IS NULL AND LOWER(original_name) = LOWER(?) AND id != ? AND deleted_at IS NULL",
        )
        .bind(owner_id)
        .bind(new_name)
        .bind(file_id)
        .fetch_optional(pool)
        .await?
    };

    if existing.is_some() {
        return Err(AppError::Conflict("同名文件已存在".into()));
    }

    let updated = sqlx::query_as::<_, File>(
        "UPDATE files SET name = ?, original_name = ?, updated_at = datetime('now') WHERE id = ? AND owner_id = ? RETURNING *",
    )
    .bind(new_name)
    .bind(new_name)
    .bind(file_id)
    .bind(owner_id)
    .fetch_one(pool)
    .await?;

    Ok(updated)
}

/// 软删除文件（移入回收站）
pub async fn soft_delete_file(
    pool: &SqlitePool,
    file_id: i64,
    owner_id: i64,
) -> Result<(), AppError> {
    let result = sqlx::query(
        "UPDATE files SET deleted_at = datetime('now') WHERE id = ? AND owner_id = ? AND deleted_at IS NULL",
    )
    .bind(file_id)
    .bind(owner_id)
    .execute(pool)
    .await?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound("文件不存在或已在回收站中".into()));
    }

    tracing::info!("文件已移入回收站: id={}", file_id);
    Ok(())
}

/// 从回收站恢复文件
pub async fn restore_file(
    pool: &SqlitePool,
    file_id: i64,
    owner_id: i64,
) -> Result<(), AppError> {
    // 获取文件信息以检查其 folder_id 是否仍然有效
    let file = sqlx::query_as::<_, File>(
        "SELECT * FROM files WHERE id = ? AND owner_id = ? AND deleted_at IS NOT NULL",
    )
    .bind(file_id)
    .bind(owner_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| AppError::NotFound("文件不在回收站中".into()))?;

    // 如果文件有 folder_id，检查文件夹是否也被软删除了
    if let Some(fid) = file.folder_id {
        let folder_exists: Option<(i64,)> = sqlx::query_as(
            "SELECT id FROM folders WHERE id = ? AND owner_id = ? AND deleted_at IS NULL",
        )
        .bind(fid)
        .bind(owner_id)
        .fetch_optional(pool)
        .await?;

        if folder_exists.is_none() {
            // 父文件夹也被删除了，将文件移到根目录
            sqlx::query(
                "UPDATE files SET folder_id = NULL, deleted_at = NULL WHERE id = ? AND owner_id = ?",
            )
            .bind(file_id)
            .bind(owner_id)
            .execute(pool)
            .await?;
        } else {
            sqlx::query(
                "UPDATE files SET deleted_at = NULL WHERE id = ? AND owner_id = ?",
            )
            .bind(file_id)
            .bind(owner_id)
            .execute(pool)
            .await?;
        }
    } else {
        sqlx::query(
            "UPDATE files SET deleted_at = NULL WHERE id = ? AND owner_id = ?",
        )
        .bind(file_id)
        .bind(owner_id)
        .execute(pool)
        .await?;
    }

    tracing::info!("文件已从回收站恢复: id={}", file_id);
    Ok(())
}

/// 列出回收站中的文件
pub async fn list_trash_files(
    pool: &SqlitePool,
    owner_id: i64,
) -> Result<Vec<FileInfo>, AppError> {
    let files = sqlx::query_as::<_, File>(
        "SELECT * FROM files WHERE owner_id = ? AND deleted_at IS NOT NULL ORDER BY deleted_at DESC",
    )
    .bind(owner_id)
    .fetch_all(pool)
    .await?;

    Ok(files.into_iter().map(|f| f.to_info()).collect())
}

/// 清空回收站（永久删除所有已软删除的文件）
pub async fn empty_trash(
    pool: &SqlitePool,
    config: &Config,
    owner_id: i64,
) -> Result<usize, AppError> {
    let files: Vec<(i64, String, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT id, stored_path, preview_path, thumb_path FROM files WHERE owner_id = ? AND deleted_at IS NOT NULL",
    )
    .bind(owner_id)
    .fetch_all(pool)
    .await?;

    let count = files.len();
    for (file_id, stored_path, preview_path, thumb_path) in &files {
        // 删除物理文件
        let stored_full = config.upload_dir.join(stored_path);
        let _ = delete_physical_file_with_retry(&stored_full, "源文件").await;

        if let Some(ref pp) = preview_path {
            cleanup_preview_file(config, pp).await;
        }
        if let Some(ref tp) = thumb_path {
            cleanup_preview_file(config, tp).await;
        }

        // 删除数据库记录
        sqlx::query("DELETE FROM files WHERE id = ?")
            .bind(file_id)
            .execute(pool)
            .await?;
    }

    tracing::info!("已清空回收站: {} 个文件被永久删除", count);
    Ok(count)
}

/// 根据ID获取单个文件，验证所有权（排除已删除的文件）
pub async fn get_file(
    pool: &SqlitePool,
    file_id: i64,
    owner_id: i64,
) -> Result<File, AppError> {
    sqlx::query_as::<_, File>("SELECT * FROM files WHERE id = ? AND owner_id = ? AND deleted_at IS NULL")
        .bind(file_id)
        .bind(owner_id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| AppError::NotFound("文件不存在".into()))
}

/// 根据ID获取文件，不验证所有权（用于分享访问）
pub async fn get_file_by_id(pool: &SqlitePool, file_id: i64) -> Result<File, AppError> {
    sqlx::query_as::<_, File>("SELECT * FROM files WHERE id = ?")
        .bind(file_id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| AppError::NotFound("文件不存在".into()))
}

/// 根据ID获取单个文件，验证所有权（包含已删除的文件，用于永久删除）
pub async fn get_file_include_deleted(
    pool: &SqlitePool,
    file_id: i64,
    owner_id: i64,
) -> Result<File, AppError> {
    sqlx::query_as::<_, File>("SELECT * FROM files WHERE id = ? AND owner_id = ?")
        .bind(file_id)
        .bind(owner_id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| AppError::NotFound("文件不存在".into()))
}

/// 验证文件扩展名未被阻止
pub fn validate_extension(filename: &str) -> Result<(), AppError> {
    let ext = Path::new(filename)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    let ext_with_dot = format!(".{}", ext);
    if BLOCKED_EXTENSIONS.contains(&ext_with_dot.as_str()) {
        return Err(AppError::BadRequest(format!(
            "文件类型 \"{}\" 不允许上传（存在安全风险）",
            ext_with_dot
        )));
    }
    Ok(())
}

/// 检查同一文件夹中是否存在重复文件名（不区分大小写）
pub async fn check_duplicates(
    pool: &SqlitePool,
    owner_id: i64,
    folder_id: Option<i64>,
    filename: &str,
) -> Result<bool, AppError> {
    let existing = if let Some(fid) = folder_id {
        sqlx::query_scalar::<_, String>(
            "SELECT original_name FROM files WHERE owner_id = ? AND folder_id = ? AND LOWER(original_name) = LOWER(?) AND deleted_at IS NULL",
        )
        .bind(owner_id)
        .bind(fid)
        .bind(filename)
        .fetch_optional(pool)
        .await?
    } else {
        sqlx::query_scalar::<_, String>(
            "SELECT original_name FROM files WHERE owner_id = ? AND folder_id IS NULL AND LOWER(original_name) = LOWER(?) AND deleted_at IS NULL",
        )
        .bind(owner_id)
        .bind(filename)
        .fetch_optional(pool)
        .await?
    };

    Ok(existing.is_some())
}

/// 生成唯一的存储文件名：{uuid}.{ext}
pub fn generate_stored_filename(original_name: &str) -> String {
    let ext = Path::new(original_name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("bin");
    format!("{}.{}", Uuid::new_v4().simple(), ext)
}

/// 获取用户的上传目录路径
pub fn user_upload_dir(config: &Config, user_id: i64) -> PathBuf {
    config.upload_dir.join(format!("user_{}", user_id))
}

/// 获取用户的预览目录路径
pub fn user_preview_dir(config: &Config, user_id: i64) -> PathBuf {
    config.upload_dir.join(format!("user_{}", user_id)).join("previews")
}

/// 检查文件类型是否支持预览
pub fn supports_preview(file_type: &str) -> bool {
    let ft = file_type.to_lowercase();
    IMAGE_FORMATS.contains(&ft.as_str()) || RAW_FORMATS.contains(&ft.as_str())
}

/// 使用重试逻辑删除物理文件。
/// 如果文件已被删除或不存在，返回 Ok(())。
/// 如果所有重试均失败，返回 Err。
pub(crate) async fn delete_physical_file_with_retry(file_path: &Path, description: &str) -> Result<(), AppError> {
    if !tokio::fs::try_exists(file_path).await.unwrap_or(false) {
        tracing::debug!("{}不存在，跳过删除: {}", description, file_path.display());
        return Ok(());
    }

    let mut last_error = None;
    for attempt in 0..MAX_RETRY_ATTEMPTS {
        match tokio::fs::remove_file(file_path).await {
            Ok(()) => {
                tracing::info!(
                    "{}删除成功（尝试第 {} 次）: {}",
                    description,
                    attempt + 1,
                    file_path.display()
                );
                return Ok(());
            }
            Err(e) => {
                last_error = Some(e);
                if attempt < MAX_RETRY_ATTEMPTS - 1 {
                    let delay = Duration::from_millis(RETRY_BASE_DELAY_MS * (1 << attempt));
                    if let Some(ref err) = last_error {
                        tracing::warn!(
                            "{}删除失败（尝试第 {} 次），{} 毫秒后重试: {} - 错误: {}",
                            description,
                            attempt + 1,
                            delay.as_millis(),
                            file_path.display(),
                            err
                        );
                    }
                    tokio::time::sleep(delay).await;
                }
            }
        }
    }

    let err = last_error.unwrap_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::Other, "未知错误")
    });
    tracing::error!(
        "{}删除失败，已重试 {} 次: {} - 错误: {}",
        description,
        MAX_RETRY_ATTEMPTS,
        file_path.display(),
        err
    );
    Err(AppError::Internal(format!(
        "删除{}失败: {}",
        description, err
    )))
}

/// 清理预览文件及其父目录（如果为空）。
/// 这是尽力而为的操作；失败会被记录但不会传播。
pub(crate) async fn cleanup_preview_file(config: &Config, preview_path: &str) {
    let preview_full = config.upload_dir.join(preview_path);

    match delete_physical_file_with_retry(&preview_full, "预览文件").await {
        Ok(()) => {
            // 尝试清理空的父目录
            if let Some(parent) = preview_full.parent() {
                match tokio::fs::read_dir(parent).await {
                    Ok(mut entries) => {
                        if entries.next_entry().await.ok().flatten().is_none() {
                            // 目录为空，删除它
                            match tokio::fs::remove_dir(parent).await {
                                Ok(()) => {
                                    tracing::info!(
                                        "Removed empty preview directory: {}",
                                        parent.display()
                                    );
                                }
                                Err(e) => {
                                    tracing::debug!(
                                        "Could not remove preview directory (may not be empty): {} - {}",
                                        parent.display(),
                                        e
                                    );
                                }
                            }
                        }
                    }
                    Err(e) => {
                        tracing::debug!(
                            "Could not read preview directory: {} - {}",
                            parent.display(),
                            e
                        );
                    }
                }
            }
        }
        Err(e) => {
            tracing::warn!(
                "Preview file cleanup failed (non-fatal): {} - {}",
                preview_full.display(),
                e.message()
            );
        }
    }
}

/// 清理存储的（源）文件，带重试。
/// 这是尽力而为的操作；失败会被记录但不会传播。
async fn cleanup_stored_file(config: &Config, stored_path: &str) {
    let stored_full = config.upload_dir.join(stored_path);

    match delete_physical_file_with_retry(&stored_full, "源文件").await {
        Ok(()) => {}
        Err(e) => {
            tracing::warn!(
                "Source file cleanup failed (non-fatal): {} - {}",
                stored_full.display(),
                e.message()
            );
        }
    }
}

/// 删除文件记录及其关联的物理文件（源文件 + 预览）。
///
/// 删除策略：
/// 1. 在事务中删除数据库记录（确保数据库级别的原子性）
/// 2. 带重试删除物理文件（源文件 + 预览）—— 尽力而为，失败不致命
///
/// 这确保用户立即看到文件已删除，同时物理清理以重试方式异步进行。
/// 失败的物理删除会被记录用于监控。
pub async fn delete_file(
    pool: &SqlitePool,
    config: &Config,
    file_id: i64,
    owner_id: i64,
) -> Result<(), AppError> {
    // 首先获取文件记录以获取 stored_path 和 preview_path
    // 使用 get_file_include_deleted 以支持永久删除回收站中的文件
    let file = get_file_include_deleted(pool, file_id, owner_id).await?;

    let stored_path = file.stored_path.clone();
    let preview_path = file.preview_path.clone();
    let thumb_path = file.thumb_path.clone();

    tracing::info!(
        "Starting deletion of file id={}, stored_path={}, has_preview={}, has_thumb={}",
        file_id,
        stored_path,
        preview_path.is_some(),
        thumb_path.is_some()
    );

    // 步骤1：在事务中删除数据库记录
    // 这是原子步骤——一旦提交，从用户视角看文件就已"删除"
    let mut tx = pool.begin().await?;
    let result = sqlx::query("DELETE FROM files WHERE id = ? AND owner_id = ?")
        .bind(file_id)
        .bind(owner_id)
        .execute(&mut *tx)
        .await?;

    if result.rows_affected() == 0 {
        // 这种情况不应该发生，因为我们刚刚获取了文件，但防御性地处理
        tx.rollback().await?;
        return Err(AppError::NotFound("文件不存在".into()));
    }

    tx.commit().await?;
    tracing::info!("Database record deleted for file id={}", file_id);

    // 步骤2：删除物理文件（尽力而为，带重试）
    // 这些作为并发任务启动以提高效率
    let cleanup_stored = cleanup_stored_file(config, &stored_path);
    let cleanup_preview = async {
        if let Some(ref pp) = preview_path {
            cleanup_preview_file(config, pp).await;
        }
    };
    let cleanup_thumb = async {
        if let Some(ref tp) = thumb_path {
            cleanup_preview_file(config, tp).await;
        }
    };

    tokio::join!(cleanup_stored, cleanup_preview, cleanup_thumb);

    Ok(())
}

/// 清理源文件在磁盘上已不存在的孤立预览/缩略图文件。
/// 这是一个维护函数，可以定期或在启动时调用。
#[allow(dead_code)]
pub async fn cleanup_orphaned_previews(
    pool: &SqlitePool,
    config: &Config,
) -> Result<usize, AppError> {
    // 查找所有包含预览或缩略图路径的数据库记录
    let records: Vec<(String, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT stored_path, preview_path, thumb_path FROM files WHERE preview_path IS NOT NULL OR thumb_path IS NOT NULL",
    )
    .fetch_all(pool)
    .await?;

    let mut cleaned = 0;
    for (stored_path, preview_path, thumb_path) in &records {
        let stored_full = config.upload_dir.join(stored_path);
        // 如果源文件缺失，则预览/缩略图是孤立的
        if !tokio::fs::try_exists(&stored_full).await.unwrap_or(false) {
            if let Some(ref pp) = preview_path {
                let full_path = config.upload_dir.join(pp);
                if tokio::fs::try_exists(&full_path).await.unwrap_or(false) {
                    match delete_physical_file_with_retry(&full_path, "孤立预览文件").await {
                        Ok(()) => cleaned += 1,
                        Err(e) => tracing::warn!("Failed to clean orphaned preview: {} - {}", full_path.display(), e.message()),
                    }
                }
            }
            if let Some(ref tp) = thumb_path {
                let full_path = config.upload_dir.join(tp);
                if tokio::fs::try_exists(&full_path).await.unwrap_or(false) {
                    match delete_physical_file_with_retry(&full_path, "孤立缩略图文件").await {
                        Ok(()) => cleaned += 1,
                        Err(e) => tracing::warn!("Failed to clean orphaned thumbnail: {} - {}", full_path.display(), e.message()),
                    }
                }
            }
        }
    }

    if cleaned > 0 {
        tracing::info!("Cleaned up {} orphaned preview/thumbnail files", cleaned);
    }
    Ok(cleaned)
}

