use std::collections::VecDeque;

use crate::config::Config;
use crate::errors::AppError;
use crate::models::folder::Folder;
use crate::services::file_service;
use sqlx::SqlitePool;

/// List folders for a user (optionally filtered by parent)
pub async fn list_folders(
    pool: &SqlitePool,
    owner_id: i64,
    parent_id: Option<i64>,
) -> Result<Vec<Folder>, AppError> {
    let folders = if let Some(pid) = parent_id {
        sqlx::query_as::<_, Folder>(
            "SELECT * FROM folders WHERE owner_id = ? AND parent_id = ? ORDER BY name",
        )
        .bind(owner_id)
        .bind(pid)
        .fetch_all(pool)
        .await?
    } else {
        sqlx::query_as::<_, Folder>(
            "SELECT * FROM folders WHERE owner_id = ? AND parent_id IS NULL ORDER BY name",
        )
        .bind(owner_id)
        .fetch_all(pool)
        .await?
    };

    Ok(folders)
}

/// Create a new folder
pub async fn create_folder(
    pool: &SqlitePool,
    owner_id: i64,
    name: &str,
    parent_id: Option<i64>,
) -> Result<Folder, AppError> {
    // Validate parent belongs to user if specified
    if let Some(pid) = parent_id {
        let parent = sqlx::query_as::<_, Folder>(
            "SELECT * FROM folders WHERE id = ? AND owner_id = ?",
        )
        .bind(pid)
        .bind(owner_id)
        .fetch_optional(pool)
        .await?;

        if parent.is_none() {
            return Err(AppError::NotFound("父文件夹不存在".into()));
        }
    }

    // Check for duplicate name
    let existing = if let Some(pid) = parent_id {
        sqlx::query_as::<_, Folder>(
            "SELECT * FROM folders WHERE name = ? AND owner_id = ? AND parent_id = ?",
        )
        .bind(name)
        .bind(owner_id)
        .bind(pid)
        .fetch_optional(pool)
        .await?
    } else {
        sqlx::query_as::<_, Folder>(
            "SELECT * FROM folders WHERE name = ? AND owner_id = ? AND parent_id IS NULL",
        )
        .bind(name)
        .bind(owner_id)
        .fetch_optional(pool)
        .await?
    };

    if existing.is_some() {
        return Err(AppError::Conflict("同名文件夹已存在".into()));
    }

    let folder = sqlx::query_as::<_, Folder>(
        "INSERT INTO folders (name, owner_id, parent_id) VALUES (?, ?, ?) RETURNING *",
    )
    .bind(name)
    .bind(owner_id)
    .bind(parent_id)
    .fetch_one(pool)
    .await?;

    Ok(folder)
}

/// Delete a folder and all its contents recursively using iterative BFS
pub async fn delete_folder(
    pool: &SqlitePool,
    config: &Config,
    folder_id: i64,
    owner_id: i64,
) -> Result<(), AppError> {
    // Verify folder belongs to user
    let folder = sqlx::query_as::<_, Folder>(
        "SELECT * FROM folders WHERE id = ? AND owner_id = ?",
    )
    .bind(folder_id)
    .bind(owner_id)
    .fetch_optional(pool)
    .await?;

    if folder.is_none() {
        return Err(AppError::NotFound("文件夹不存在".into()));
    }

    // Collect all subfolder IDs using iterative BFS
    let mut folder_ids = vec![folder_id];
    let mut queue: VecDeque<i64> = VecDeque::new();
    queue.push_back(folder_id);

    while let Some(current_id) = queue.pop_front() {
        let subfolders: Vec<(i64,)> = sqlx::query_as(
            "SELECT id FROM folders WHERE parent_id = ?",
        )
        .bind(current_id)
        .fetch_all(pool)
        .await?;

        for (sub_id,) in subfolders {
            folder_ids.push(sub_id);
            queue.push_back(sub_id);
        }
    }

    // Delete all files in all collected folders with retry and logging
    for fid in &folder_ids {
        let files: Vec<(i64, String, Option<String>, Option<String>)> = sqlx::query_as(
            "SELECT id, stored_path, preview_path, thumb_path FROM files WHERE folder_id = ?",
        )
        .bind(fid)
        .fetch_all(pool)
        .await?;

        let file_count = files.len();
        tracing::info!(
            "Deleting {} files from folder id={}",
            file_count,
            fid
        );

        for (file_id, stored_path, preview_path, thumb_path) in files {
            let stored_full = config.upload_dir.join(&stored_path);
            // Use retry-based deletion for source files
            match file_service::delete_physical_file_with_retry(&stored_full, "源文件").await {
                Ok(()) => {}
                Err(e) => {
                    tracing::warn!(
                        "Source file cleanup failed during folder deletion (non-fatal): {} - {}",
                        stored_full.display(),
                        e.message()
                    );
                }
            }

            if let Some(ref pp) = preview_path {
                file_service::cleanup_preview_file(config, pp).await;
            }

            if let Some(ref tp) = thumb_path {
                file_service::cleanup_preview_file(config, tp).await;
            }

            // Delete DB record
            sqlx::query("DELETE FROM files WHERE id = ?")
                .bind(file_id)
                .execute(pool)
                .await?;
        }
    }

    // Delete folders in reverse order (children first) via cascading or manual
    // SQLite foreign keys with ON DELETE CASCADE should handle this,
    // but we delete explicitly for safety
    for fid in folder_ids.iter().rev() {
        sqlx::query("DELETE FROM folders WHERE id = ?")
            .bind(fid)
            .execute(pool)
            .await?;
    }

    Ok(())
}

/// Get breadcrumb path for a folder
pub async fn get_breadcrumbs(
    pool: &SqlitePool,
    folder_id: i64,
) -> Result<Vec<Folder>, AppError> {
    let mut breadcrumbs = Vec::new();
    let mut current_id = Some(folder_id);

    while let Some(cid) = current_id {
        let folder = sqlx::query_as::<_, Folder>("SELECT * FROM folders WHERE id = ?")
            .bind(cid)
            .fetch_optional(pool)
            .await?;

        if let Some(f) = folder {
            current_id = f.parent_id;
            breadcrumbs.push(f);
        } else {
            break;
        }
    }

    breadcrumbs.reverse();
    Ok(breadcrumbs)
}