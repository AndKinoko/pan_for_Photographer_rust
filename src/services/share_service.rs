use chrono::Utc;
use uuid::Uuid;

use crate::errors::AppError;
use crate::models::share::{FileShare, ShareInfo};
use crate::utils::crypto;
use sqlx::SqlitePool;

/// 集中的分享验证——检查存在性、活跃状态、过期时间和下载次数
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

    // 检查下载次数限制
    if let Some(max_dl) = share.max_downloads {
        if share.download_count >= max_dl {
            return Err(AppError::Gone("分享链接的下载次数已用尽".into()));
        }
    }

    Ok(share)
}

/// 创建新的文件/文件夹分享
pub async fn create_share(
    pool: &SqlitePool,
    file_id: Option<i64>,
    folder_id: Option<i64>,
    owner_id: i64,
    expires_hours: Option<i64>,
    password: Option<String>,
    max_downloads: Option<i64>,
    custom_code: Option<String>,
) -> Result<FileShare, AppError> {
    // 验证文件或文件夹存在且属于当前用户
    if let Some(fid) = file_id {
        let exists = sqlx::query_scalar::<_, i64>(
            "SELECT id FROM files WHERE id = ? AND owner_id = ? AND deleted_at IS NULL",
        )
        .bind(fid)
        .bind(owner_id)
        .fetch_optional(pool)
        .await?;

        if exists.is_none() {
            return Err(AppError::NotFound("文件不存在".into()));
        }
    } else if let Some(fid) = folder_id {
        let exists = sqlx::query_scalar::<_, i64>(
            "SELECT id FROM folders WHERE id = ? AND owner_id = ? AND deleted_at IS NULL",
        )
        .bind(fid)
        .bind(owner_id)
        .fetch_optional(pool)
        .await?;

        if exists.is_none() {
            return Err(AppError::NotFound("文件夹不存在".into()));
        }
    } else {
        return Err(AppError::BadRequest("必须指定要分享的文件或文件夹".into()));
    }

    // 生成分享ID或使用自定义码
    let share_id = if let Some(ref code) = custom_code {
        let code = code.trim();
        if code.is_empty() {
            return Err(AppError::BadRequest("自定义分享码不能为空".into()));
        }
        // 检查自定义码是否已存在
        let existing = sqlx::query_scalar::<_, String>(
            "SELECT id FROM file_shares WHERE id = ?",
        )
        .bind(code)
        .fetch_optional(pool)
        .await?;

        if existing.is_some() {
            return Err(AppError::Conflict("该分享码已被使用".into()));
        }
        code.to_string()
    } else {
        Uuid::new_v4().to_string()
    };

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
        r#"INSERT INTO file_shares (id, file_id, folder_id, owner_id, expires_at, password_hash, max_downloads, custom_code)
           VALUES (?, ?, ?, ?, ?, ?, ?, ?) RETURNING *"#,
    )
    .bind(&share_id)
    .bind(file_id)
    .bind(folder_id)
    .bind(owner_id)
    .bind(&expires_at)
    .bind(&password_hash)
    .bind(max_downloads)
    .bind(custom_code.as_ref().map(|s| s.trim()))
    .fetch_one(pool)
    .await?;

    Ok(share)
}

/// 列出用户的所有分享（使用JOIN优化N+1查询）
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

/// 获取单个分享详情（仅限所有者）
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

/// 删除分享（仅限所有者）
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

/// 验证分享密码
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

/// 增加分享的下载计数
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

/// 根据ID获取分享（公开访问，先验证）
pub async fn get_public_share(
    pool: &SqlitePool,
    share_id: &str,
) -> Result<ShareInfo, AppError> {
    let share = validate_share(pool, share_id).await?;
    share_to_info(pool, &share).await
}

/// 将 FileShare 转换为包含关联数据的 ShareInfo
async fn share_to_info(pool: &SqlitePool, share: &FileShare) -> Result<ShareInfo, AppError> {
    // 获取文件或文件夹信息
    let (file_name, file_type, file_size, preview_path, thumb_path, owner_name) = if let Some(fid) = share.file_id {
        let row: Option<(String, String, i64, Option<String>, Option<String>, String)> = sqlx::query_as(
            "SELECT f.original_name, f.file_type, f.size, f.preview_path, f.thumb_path, COALESCE(u.username, '(用户已删除)')
             FROM files f
             LEFT JOIN users u ON u.id = f.owner_id
             WHERE f.id = ?",
        )
        .bind(fid)
        .fetch_optional(pool)
        .await?;

        row.unwrap_or_else(|| ("(文件已删除)".to_string(), "".to_string(), 0, None, None, "(用户已删除)".to_string()))
    } else if let Some(fid) = share.folder_id {
        let row: Option<(String, String)> = sqlx::query_as(
            "SELECT name, COALESCE((SELECT username FROM users WHERE id = folders.owner_id), '(用户已删除)')
             FROM folders WHERE id = ?",
        )
        .bind(fid)
        .fetch_optional(pool)
        .await?;

        let (folder_name, owner) = row.unwrap_or_else(|| ("(文件夹已删除)".to_string(), "(用户已删除)".to_string()));
        (folder_name, "folder".to_string(), 0, None, None, owner)
    } else {
        ("(已删除)".to_string(), "".to_string(), 0, None, None, "(用户已删除)".to_string())
    };

    let is_expired = if let Some(ref expires_at) = share.expires_at {
        let now = Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
        now > *expires_at
    } else {
        false
    };

    let image_formats = ["jpg", "jpeg", "png", "gif", "bmp", "webp", "tiff", "tif"];
    let inline_formats = ["pdf"];
    let ft = file_type.to_lowercase();

    let preview_url = if preview_path.is_some() {
        Some(format!("/api/public/shares/{}/media?preview=1", share.id))
    } else if image_formats.contains(&ft.as_str()) || inline_formats.contains(&ft.as_str()) {
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
        folder_id: share.folder_id,
        file_name,
        owner_id: share.owner_id,
        owner_name,
        created_at: share.created_at.clone(),
        expires_at: share.expires_at.clone(),
        has_password: !share.password_hash.is_empty(),
        download_count: share.download_count,
        max_downloads: share.max_downloads,
        is_active: share.is_active == 1,
        is_expired,
        share_url: format!("/share/{}", share.id),
        file_type,
        file_size,
        formatted_size: crate::models::file::format_file_size(file_size),
        preview_url,
        thumb_url,
        custom_code: share.custom_code.clone(),
    })
}
