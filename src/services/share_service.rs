use chrono::Utc;
use uuid::Uuid;

use crate::errors::AppError;
use crate::models::share::{FileShare, ShareInfo};
use crate::utils::crypto;
use sqlx::SqlitePool;

/// Centralized share validation - checks existence, active status, and expiry
pub async fn validate_share(
    pool: &SqlitePool,
    share_id: &str,
) -> Result<FileShare, AppError> {
    let share = sqlx::query_as::<_, FileShare>(
        "SELECT * FROM file_shares WHERE id = ?",
    )
    .bind(share_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| AppError::NotFound("分享链接不存在".into()))?;

    if share.is_active == 0 {
        return Err(AppError::Gone("分享链接已失效".into()));
    }

    if let Some(ref expires_at) = share.expires_at {
        let now = Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
        if now > *expires_at {
            return Err(AppError::Gone("分享链接已过期".into()));
        }
    }

    Ok(share)
}

/// Create a new file share
pub async fn create_share(
    pool: &SqlitePool,
    file_id: i64,
    owner_id: i64,
    expires_hours: Option<i64>,
    password: Option<String>,
) -> Result<FileShare, AppError> {
    // Verify file exists and belongs to user
    let file = sqlx::query_scalar::<_, i64>(
        "SELECT id FROM files WHERE id = ? AND owner_id = ?",
    )
    .bind(file_id)
    .bind(owner_id)
    .fetch_optional(pool)
    .await?;

    if file.is_none() {
        return Err(AppError::NotFound("文件不存在".into()));
    }

    let share_id = Uuid::new_v4().to_string();
    let password_hash = if let Some(ref pwd) = password {
        if pwd.is_empty() {
            String::new()
        } else {
            crypto::hash_password(pwd)?
        }
    } else {
        String::new()
    };

    let expires_at = if let Some(hours) = expires_hours {
        if hours > 0 {
            let expiry = Utc::now() + chrono::Duration::hours(hours);
            Some(expiry.format("%Y-%m-%d %H:%M:%S").to_string())
        } else {
            None
        }
    } else {
        None
    };

    let share = sqlx::query_as::<_, FileShare>(
        r#"INSERT INTO file_shares (id, file_id, owner_id, expires_at, password_hash)
           VALUES (?, ?, ?, ?, ?) RETURNING *"#,
    )
    .bind(&share_id)
    .bind(file_id)
    .bind(owner_id)
    .bind(&expires_at)
    .bind(&password_hash)
    .fetch_one(pool)
    .await?;

    Ok(share)
}

/// List all shares for a user
pub async fn list_shares(
    pool: &SqlitePool,
    owner_id: i64,
) -> Result<Vec<ShareInfo>, AppError> {
    let shares = sqlx::query_as::<_, FileShare>(
        "SELECT * FROM file_shares WHERE owner_id = ? ORDER BY created_at DESC",
    )
    .bind(owner_id)
    .fetch_all(pool)
    .await?;

    let mut result = Vec::new();
    for share in shares {
        result.push(share_to_info(pool, &share).await?);
    }

    Ok(result)
}

/// Get a single share detail (owner only)
pub async fn get_share(
    pool: &SqlitePool,
    share_id: &str,
    owner_id: i64,
) -> Result<ShareInfo, AppError> {
    let share = sqlx::query_as::<_, FileShare>(
        "SELECT * FROM file_shares WHERE id = ? AND owner_id = ?",
    )
    .bind(share_id)
    .bind(owner_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| AppError::NotFound("分享链接不存在".into()))?;

    share_to_info(pool, &share).await
}

/// Delete a share (owner only)
pub async fn delete_share(
    pool: &SqlitePool,
    share_id: &str,
    owner_id: i64,
) -> Result<(), AppError> {
    let result = sqlx::query("DELETE FROM file_shares WHERE id = ? AND owner_id = ?")
        .bind(share_id)
        .bind(owner_id)
        .execute(pool)
        .await?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound("分享链接不存在".into()));
    }

    Ok(())
}

/// Verify share password
pub async fn verify_share_password(
    pool: &SqlitePool,
    share_id: &str,
    password: &str,
) -> Result<bool, AppError> {
    let share = validate_share(pool, share_id).await?;

    if share.password_hash.is_empty() {
        return Ok(true);
    }

    crypto::verify_password(password, &share.password_hash)
        .map_err(|_| AppError::Internal("密码验证失败".into()))
}

/// Increment download count for a share
pub async fn increment_download_count(
    pool: &SqlitePool,
    share_id: &str,
) -> Result<(), AppError> {
    sqlx::query("UPDATE file_shares SET download_count = download_count + 1 WHERE id = ?")
        .bind(share_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Get a share by ID (public access, validates first)
pub async fn get_public_share(
    pool: &SqlitePool,
    share_id: &str,
) -> Result<ShareInfo, AppError> {
    let share = validate_share(pool, share_id).await?;
    share_to_info(pool, &share).await
}

/// Convert FileShare to ShareInfo with joined data
async fn share_to_info(pool: &SqlitePool, share: &FileShare) -> Result<ShareInfo, AppError> {
    let row: Option<(String, String, i64, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT original_name, file_type, size, preview_path, thumb_path FROM files WHERE id = ?",
    )
    .bind(share.file_id)
    .fetch_optional(pool)
    .await?;

    let (file_name, file_type, file_size, preview_path, thumb_path) =
        row.unwrap_or_else(|| ("(文件已删除)".to_string(), "".to_string(), 0, None, None));

    let (owner_name,): (String,) = sqlx::query_as(
        "SELECT username FROM users WHERE id = ?",
    )
    .bind(share.owner_id)
    .fetch_optional(pool)
    .await?
    .unwrap_or_else(|| ("(用户已删除)".to_string(),));

    let is_expired = if let Some(ref expires_at) = share.expires_at {
        let now = Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
        now > *expires_at
    } else {
        false
    };

    let image_formats = ["jpg", "jpeg", "png", "gif", "bmp", "webp", "tiff", "tif"];
    let ft = file_type.to_lowercase();

    let preview_url = if preview_path.is_some() {
        Some(format!("/api/public/shares/{}/media?preview=1", share.id))
    } else if image_formats.contains(&ft.as_str()) {
        Some(format!("/api/public/shares/{}/media", share.id))
    } else {
        None
    };

    let thumb_url = if thumb_path.is_some() {
        Some(format!("/api/public/shares/{}/media?thumb=1", share.id))
    } else if preview_url.is_some() {
        preview_url.clone()
    } else if image_formats.contains(&ft.as_str()) {
        Some(format!("/api/public/shares/{}/media", share.id))
    } else {
        None
    };

    Ok(ShareInfo {
        id: share.id.clone(),
        file_id: share.file_id,
        file_name,
        owner_id: share.owner_id,
        owner_name,
        created_at: share.created_at.clone(),
        expires_at: share.expires_at.clone(),
        has_password: !share.password_hash.is_empty(),
        download_count: share.download_count,
        is_active: share.is_active == 1,
        is_expired,
        share_url: format!("/share/{}", share.id),
        file_type,
        file_size,
        formatted_size: crate::models::file::format_file_size(file_size),
        preview_url,
        thumb_url,
    })
}