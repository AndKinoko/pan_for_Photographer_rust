use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::config::Config;
use crate::errors::AppError;
use crate::models::file::{File, FileInfo};
use crate::models::folder::Folder;
use sqlx::SqlitePool;

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

/// 清空回收站（永久删除所有已软删除的文件记录）
/// 磁盘清理交由周期 GC（sweeper）统一处理。
pub async fn empty_trash(
    pool: &SqlitePool,
    owner_id: i64,
) -> Result<usize, AppError> {
    let result = sqlx::query(
        "DELETE FROM files WHERE owner_id = ? AND deleted_at IS NOT NULL",
    )
    .bind(owner_id)
    .execute(pool)
    .await?;

    let count = result.rows_affected() as usize;
    tracing::info!("已清空回收站: {} 个文件记录被永久删除", count);
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

/// 检查文件类型是否支持预览
pub fn supports_preview(file_type: &str) -> bool {
    let ft = file_type.to_lowercase();
    IMAGE_FORMATS.contains(&ft.as_str()) || RAW_FORMATS.contains(&ft.as_str())
}

/// 删除文件记录（从回收站永久删除）。
/// 磁盘清理交由周期 GC（sweeper）统一处理。
pub async fn delete_file(
    pool: &SqlitePool,
    file_id: i64,
    owner_id: i64,
) -> Result<(), AppError> {
    // 先获取文件记录以验证所有权（支持已软删除的文件）
    let file = get_file_include_deleted(pool, file_id, owner_id).await?;

    let result = sqlx::query("DELETE FROM files WHERE id = ? AND owner_id = ?")
        .bind(file_id)
        .bind(owner_id)
        .execute(pool)
        .await?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound("文件不存在".into()));
    }

    tracing::info!("文件记录已永久删除: id={}", file.id);
    Ok(())
}

