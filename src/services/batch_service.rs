use std::collections::{HashSet, VecDeque};

use crate::config::Config;
use crate::errors::AppError;
use crate::models::batch::*;
use crate::services::file_service;
use crate::services::folder_service;
use sqlx::SqlitePool;

/// Maximum batch size to prevent resource exhaustion
const MAX_BATCH_SIZE: usize = 500;

/// Verify that all file_ids and folder_ids belong to the current user
async fn verify_ownership(
    pool: &SqlitePool,
    user_id: i64,
    file_ids: &[i64],
    folder_ids: &[i64],
) -> Result<(), AppError> {
    if !file_ids.is_empty() {
        let count: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM files WHERE id IN (SELECT value FROM json_each(?)) AND owner_id = ?",
        )
        .bind(serde_json::to_string(file_ids).unwrap_or_default())
        .bind(user_id)
        .fetch_one(pool)
        .await?;

        if count.0 != file_ids.len() as i64 {
            return Err(AppError::Forbidden("部分文件不属于当前用户".into()));
        }
    }

    if !folder_ids.is_empty() {
        let count: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM folders WHERE id IN (SELECT value FROM json_each(?)) AND owner_id = ?",
        )
        .bind(serde_json::to_string(folder_ids).unwrap_or_default())
        .bind(user_id)
        .fetch_one(pool)
        .await?;

        if count.0 != folder_ids.len() as i64 {
            return Err(AppError::Forbidden("部分文件夹不属于当前用户".into()));
        }
    }

    Ok(())
}

/// Collect all descendant folder IDs using BFS
async fn collect_subtree_folder_ids(
    pool: &SqlitePool,
    folder_ids: &[i64],
) -> Result<HashSet<i64>, AppError> {
    let mut all_ids: HashSet<i64> = folder_ids.iter().copied().collect();
    let mut queue: VecDeque<i64> = folder_ids.iter().copied().collect();

    while let Some(current_id) = queue.pop_front() {
        let children: Vec<(i64,)> = sqlx::query_as(
            "SELECT id FROM folders WHERE parent_id = ?",
        )
        .bind(current_id)
        .fetch_all(pool)
        .await?;

        for (child_id,) in children {
            if all_ids.insert(child_id) {
                queue.push_back(child_id);
            }
        }
    }

    Ok(all_ids)
}

/// Generate a unique name when a conflict is detected
fn generate_unique_name(existing_names: &HashSet<String>, original: &str) -> String {
    let (stem, ext) = match original.rfind('.') {
        Some(dot) => (&original[..dot], &original[dot..]),
        None => (original, ""),
    };

    let mut counter = 1;
    loop {
        let candidate = format!("{stem} ({counter}){ext}");
        if !existing_names.contains(&candidate) {
            return candidate;
        }
        counter += 1;
    }
}

/// Batch move files and folders
pub async fn batch_move(
    pool: &SqlitePool,
    config: &Config,
    user_id: i64,
    req: &BatchMoveCopyRequest,
) -> Result<BatchMoveCopyResult, AppError> {
    let total_items = req.file_ids.len() + req.folder_ids.len();
    if total_items == 0 {
        return Err(AppError::BadRequest("请至少选择一个文件或文件夹".into()));
    }
    if total_items > MAX_BATCH_SIZE {
        return Err(AppError::BadRequest(
            format!("单次操作最多 {} 项，当前 {} 项", MAX_BATCH_SIZE, total_items),
        ));
    }

    // Verify ownership
    verify_ownership(pool, user_id, &req.file_ids, &req.folder_ids).await?;

    // Verify target folder exists and belongs to user
    if let Some(tfid) = req.target_folder_id {
        let count: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM folders WHERE id = ? AND owner_id = ?",
        )
        .bind(tfid)
        .bind(user_id)
        .fetch_one(pool)
        .await?;

        if count.0 == 0 {
            return Err(AppError::NotFound("目标文件夹不存在或无权访问".into()));
        }
    }

    // Prevent moving a folder into itself or its subtree
    if !req.folder_ids.is_empty() {
        if let Some(tfid) = req.target_folder_id {
            let subtree = collect_subtree_folder_ids(pool, &req.folder_ids).await?;
            if subtree.contains(&tfid) {
                return Err(AppError::BadRequest(
                    "不能将文件夹移动到自身或子文件夹中".into(),
                ));
            }
        }
    }

    // Collect existing names in the target folder
    let existing_names = if let Some(tfid) = req.target_folder_id {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT original_name FROM files WHERE folder_id = ? AND owner_id = ?",
        )
        .bind(tfid)
        .bind(user_id)
        .fetch_all(pool)
        .await?;
        rows.into_iter().map(|(n,)| n).collect::<HashSet<String>>()
    } else {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT original_name FROM files WHERE folder_id IS NULL AND owner_id = ?",
        )
        .bind(user_id)
        .fetch_all(pool)
        .await?;
        rows.into_iter().map(|(n,)| n).collect::<HashSet<String>>()
    };

    let mut results = Vec::new();
    let mut succeeded = 0;
    let mut skipped = 0;
    let mut failed = 0;
    let mut current_names = existing_names;

    // Process files
    for &file_id in &req.file_ids {
        let file = match file_service::get_file_by_id(pool, file_id).await {
            Ok(f) => f,
            Err(_) => {
                failed += 1;
                results.push(BatchItemResult {
                    id: file_id,
                    name: "(未知)".into(),
                    status: "failed".into(),
                    reason: Some("文件不存在".into()),
                    new_name: None,
                    r#type: Some("file".into()),
                    children_count: None,
                });
                continue;
            }
        };

        let original_name = file.original_name.clone();

        if current_names.contains(&original_name) {
            match req.conflict_strategy.as_str() {
                "skip" => {
                    skipped += 1;
                    results.push(BatchItemResult {
                        id: file_id,
                        name: original_name,
                        status: "skipped".into(),
                        reason: Some("同名文件已存在".into()),
                        new_name: None,
                        r#type: Some("file".into()),
                        children_count: None,
                    });
                    continue;
                }
                "overwrite" => {
                    // Delete existing file in target
                    let target_file: Option<(i64,)> = if let Some(tfid) = req.target_folder_id {
                        sqlx::query_as(
                            "SELECT id FROM files WHERE original_name = ? AND folder_id = ? AND owner_id = ?",
                        )
                        .bind(&original_name)
                        .bind(tfid)
                        .bind(user_id)
                        .fetch_optional(pool)
                        .await?
                    } else {
                        sqlx::query_as(
                            "SELECT id FROM files WHERE original_name = ? AND folder_id IS NULL AND owner_id = ?",
                        )
                        .bind(&original_name)
                        .bind(user_id)
                        .fetch_optional(pool)
                        .await?
                    };
                    if let Some((tid,)) = target_file {
                        let _ = file_service::delete_file(pool, config, tid, user_id).await;
                    }
                    current_names.remove(&original_name);
                }
                "rename" | _ => {
                    let new_name = generate_unique_name(&current_names, &original_name);
                    current_names.insert(new_name.clone());
                    sqlx::query("UPDATE files SET name = ?, original_name = ?, folder_id = ? WHERE id = ?")
                        .bind(&new_name)
                        .bind(&new_name)
                        .bind(req.target_folder_id)
                        .bind(file_id)
                        .execute(pool)
                        .await?;
                    succeeded += 1;
                    results.push(BatchItemResult {
                        id: file_id,
                        name: original_name,
                        status: "renamed".into(),
                        new_name: Some(new_name),
                        reason: None,
                        r#type: Some("file".into()),
                        children_count: None,
                    });
                    continue;
                }
            }
        }

        // Move the file
        sqlx::query("UPDATE files SET folder_id = ? WHERE id = ?")
            .bind(req.target_folder_id)
            .bind(file_id)
            .execute(pool)
            .await?;
        current_names.insert(original_name.clone());
        succeeded += 1;
        results.push(BatchItemResult {
            id: file_id,
            name: original_name,
            status: "moved".into(),
            new_name: None,
            reason: None,
            r#type: Some("file".into()),
            children_count: None,
        });
    }

    // Process folders
    for &folder_id in &req.folder_ids {
        let folder: Option<(String,)> =
            sqlx::query_as("SELECT name FROM folders WHERE id = ?")
                .bind(folder_id)
                .fetch_optional(pool)
                .await?;

        let folder_name = match folder {
            Some((n,)) => n,
            None => {
                failed += 1;
                results.push(BatchItemResult {
                    id: folder_id,
                    name: "(未知)".into(),
                    status: "failed".into(),
                    reason: Some("文件夹不存在".into()),
                    new_name: None,
                    r#type: Some("folder".into()),
                    children_count: None,
                });
                continue;
            }
        };

        sqlx::query("UPDATE folders SET parent_id = ? WHERE id = ?")
            .bind(req.target_folder_id)
            .bind(folder_id)
            .execute(pool)
            .await?;
        succeeded += 1;
        results.push(BatchItemResult {
            id: folder_id,
            name: folder_name,
            status: "moved".into(),
            new_name: None,
            reason: None,
            r#type: Some("folder".into()),
            children_count: None,
        });
    }

    Ok(BatchMoveCopyResult {
        total: total_items,
        succeeded,
        skipped,
        failed,
        results,
    })
}

/// Batch copy files
pub async fn batch_copy(
    pool: &SqlitePool,
    config: &Config,
    user_id: i64,
    req: &BatchMoveCopyRequest,
) -> Result<BatchMoveCopyResult, AppError> {
    let total_items = req.file_ids.len();
    if total_items == 0 {
        return Err(AppError::BadRequest("复制操作暂不支持文件夹，请至少选择一个文件".into()));
    }
    if total_items > MAX_BATCH_SIZE {
        return Err(AppError::BadRequest(
            format!("单次操作最多 {} 项，当前 {} 项", MAX_BATCH_SIZE, total_items),
        ));
    }

    verify_ownership(pool, user_id, &req.file_ids, &[]).await?;

    // Verify target folder
    if let Some(tfid) = req.target_folder_id {
        let count: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM folders WHERE id = ? AND owner_id = ?",
        )
        .bind(tfid)
        .bind(user_id)
        .fetch_one(pool)
        .await?;

        if count.0 == 0 {
            return Err(AppError::NotFound("目标文件夹不存在或无权访问".into()));
        }
    }

    // Collect existing names in target
    let existing_names = if let Some(tfid) = req.target_folder_id {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT original_name FROM files WHERE folder_id = ? AND owner_id = ?",
        )
        .bind(tfid)
        .bind(user_id)
        .fetch_all(pool)
        .await?;
        rows.into_iter().map(|(n,)| n).collect::<HashSet<String>>()
    } else {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT original_name FROM files WHERE folder_id IS NULL AND owner_id = ?",
        )
        .bind(user_id)
        .fetch_all(pool)
        .await?;
        rows.into_iter().map(|(n,)| n).collect::<HashSet<String>>()
    };

    let mut results = Vec::new();
    let mut succeeded = 0;
    let mut skipped = 0;
    let mut failed = 0;
    let mut current_names = existing_names;

    for &file_id in &req.file_ids {
        let file = match file_service::get_file_by_id(pool, file_id).await {
            Ok(f) => f,
            Err(_) => {
                failed += 1;
                results.push(BatchItemResult {
                    id: file_id,
                    name: "(未知)".into(),
                    status: "failed".into(),
                    reason: Some("文件不存在".into()),
                    new_name: None,
                    r#type: Some("file".into()),
                    children_count: None,
                });
                continue;
            }
        };

        let original_name = file.original_name.clone();
        let mut new_name = original_name.clone();

        if current_names.contains(&original_name) {
            match req.conflict_strategy.as_str() {
                "skip" => {
                    skipped += 1;
                    results.push(BatchItemResult {
                        id: file_id,
                        name: original_name,
                        status: "skipped".into(),
                        reason: Some("同名文件已存在".into()),
                        new_name: None,
                        r#type: Some("file".into()),
                        children_count: None,
                    });
                    continue;
                }
                "overwrite" => {
                    let target_file: Option<(i64,)> = if let Some(tfid) = req.target_folder_id {
                        sqlx::query_as(
                            "SELECT id FROM files WHERE original_name = ? AND folder_id = ? AND owner_id = ?",
                        )
                        .bind(&original_name)
                        .bind(tfid)
                        .bind(user_id)
                        .fetch_optional(pool)
                        .await?
                    } else {
                        sqlx::query_as(
                            "SELECT id FROM files WHERE original_name = ? AND folder_id IS NULL AND owner_id = ?",
                        )
                        .bind(&original_name)
                        .bind(user_id)
                        .fetch_optional(pool)
                        .await?
                    };
                    if let Some((tid,)) = target_file {
                        let _ = file_service::delete_file(pool, config, tid, user_id).await;
                    }
                    current_names.remove(&original_name);
                }
                "rename" | _ => {
                    new_name = generate_unique_name(&current_names, &original_name);
                }
            }
        }

        // Copy: insert new file record with same stored_path but new name
        sqlx::query(
            "INSERT INTO files (name, original_name, stored_path, preview_path, thumb_path, owner_id, folder_id, size, file_type) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&new_name)
        .bind(&new_name)
        .bind(&file.stored_path)
        .bind(&file.preview_path)
        .bind(&file.thumb_path)
        .bind(user_id)
        .bind(req.target_folder_id)
        .bind(file.size)
        .bind(&file.file_type)
        .execute(pool)
        .await?;

        current_names.insert(new_name.clone());
        let name_changed = new_name != original_name;
        succeeded += 1;
        results.push(BatchItemResult {
            id: file_id,
            name: original_name,
            status: "copied".into(),
            new_name: if name_changed { Some(new_name) } else { None },
            reason: None,
            r#type: Some("file".into()),
            children_count: None,
        });
    }

    Ok(BatchMoveCopyResult {
        total: total_items,
        succeeded,
        skipped,
        failed,
        results,
    })
}

/// Batch delete files and folders
pub async fn batch_delete(
    pool: &SqlitePool,
    config: &Config,
    user_id: i64,
    req: &BatchDeleteRequest,
) -> Result<BatchDeleteResult, AppError> {
    let total_items = req.file_ids.len() + req.folder_ids.len();
    if total_items == 0 {
        return Err(AppError::BadRequest("请至少选择一个文件或文件夹".into()));
    }
    if total_items > MAX_BATCH_SIZE {
        return Err(AppError::BadRequest(
            format!("单次操作最多 {} 项，当前 {} 项", MAX_BATCH_SIZE, total_items),
        ));
    }

    verify_ownership(pool, user_id, &req.file_ids, &req.folder_ids).await?;

    let mut results = Vec::new();
    let mut deleted = 0;
    let mut failed = 0;

    // Delete files
    for &file_id in &req.file_ids {
        match file_service::delete_file(pool, config, file_id, user_id).await {
            Ok(()) => {
                let file_name = "(已删除)".to_string();
                deleted += 1;
                results.push(BatchItemResult {
                    id: file_id,
                    name: file_name,
                    status: "deleted".into(),
                    new_name: None,
                    reason: None,
                    r#type: Some("file".into()),
                    children_count: None,
                });
            }
            Err(e) => {
                failed += 1;
                results.push(BatchItemResult {
                    id: file_id,
                    name: "(未知)".into(),
                    status: "failed".into(),
                    reason: Some(e.message().to_string()),
                    new_name: None,
                    r#type: Some("file".into()),
                    children_count: None,
                });
            }
        }
    }

    // Delete folders
    for &folder_id in &req.folder_ids {
        match folder_service::delete_folder(pool, config, folder_id, user_id).await {
            Ok(()) => {
                deleted += 1;
                results.push(BatchItemResult {
                    id: folder_id,
                    name: "(已删除)".into(),
                    status: "deleted".into(),
                    new_name: None,
                    reason: None,
                    r#type: Some("folder".into()),
                    children_count: None,
                });
            }
            Err(e) => {
                failed += 1;
                results.push(BatchItemResult {
                    id: folder_id,
                    name: "(未知)".into(),
                    status: "failed".into(),
                    reason: Some(e.message().to_string()),
                    new_name: None,
                    r#type: Some("folder".into()),
                    children_count: None,
                });
            }
        }
    }

    Ok(BatchDeleteResult {
        total: total_items,
        deleted,
        failed,
        results,
    })
}