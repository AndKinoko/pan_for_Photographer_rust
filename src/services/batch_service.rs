use std::collections::{HashSet, VecDeque};

use crate::errors::AppError;
use crate::models::batch::*;
use crate::models::file::File;
use crate::services::file_service;
use crate::services::folder_service;
use sqlx::SqlitePool;

/// 最大批处理大小，防止资源耗尽
const MAX_BATCH_SIZE: usize = 500;

/// 验证所有 file_ids 和 folder_ids 都属于当前用户
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

/// 使用BFS收集所有后代文件夹ID
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

/// 检测到冲突时生成唯一名称
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

/// 批量移动文件和文件夹
pub async fn batch_move(
    pool: &SqlitePool,
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

    // 验证所有权
    verify_ownership(pool, user_id, &req.file_ids, &req.folder_ids).await?;

    // 验证目标文件夹存在且属于当前用户
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

    // 防止将文件夹移动到自身或其子文件夹中
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

    // 收集目标文件夹中的现有名称
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

    // 开启事务，保证整批移动要么全部生效、要么全部回滚。
    // SQLite 为单写者，事务内逐条 UPDATE 串行执行即可。
    let mut tx = pool.begin().await?;

    let mut results = Vec::new();
    let mut succeeded = 0;
    let mut skipped = 0;
    let mut failed = 0;
    let mut current_names = existing_names;

    // 处理文件
    for &file_id in &req.file_ids {
        let file: Option<File> = sqlx::query_as("SELECT * FROM files WHERE id = ?")
            .bind(file_id)
            .fetch_optional(&mut *tx)
            .await?;

        let file = match file {
            Some(f) => f,
            None => {
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
                    // 排除自身（原地移动时目标即自身），其余同名目标做软删除，
                    // 保留在回收站可恢复，避免不可逆的数据丢失。
                    let target_file: Option<(i64,)> = if let Some(tfid) = req.target_folder_id {
                        sqlx::query_as(
                            "SELECT id FROM files WHERE original_name = ? AND folder_id = ? AND owner_id = ? AND id != ?",
                        )
                        .bind(&original_name)
                        .bind(tfid)
                        .bind(user_id)
                        .bind(file_id)
                        .fetch_optional(&mut *tx)
                        .await?
                    } else {
                        sqlx::query_as(
                            "SELECT id FROM files WHERE original_name = ? AND folder_id IS NULL AND owner_id = ? AND id != ?",
                        )
                        .bind(&original_name)
                        .bind(user_id)
                        .bind(file_id)
                        .fetch_optional(&mut *tx)
                        .await?
                    };
                    if let Some((tid,)) = target_file {
                        sqlx::query(
                            "UPDATE files SET deleted_at = datetime('now') WHERE id = ? AND owner_id = ? AND deleted_at IS NULL",
                        )
                        .bind(tid)
                        .bind(user_id)
                        .execute(&mut *tx)
                        .await?;
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
                        .execute(&mut *tx)
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

        // 移动文件
        sqlx::query("UPDATE files SET folder_id = ? WHERE id = ?")
            .bind(req.target_folder_id)
            .bind(file_id)
            .execute(&mut *tx)
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

    // 处理文件夹
    for &folder_id in &req.folder_ids {
        let folder_row: Option<(String,)> =
            sqlx::query_as("SELECT name FROM folders WHERE id = ?")
                .bind(folder_id)
                .fetch_optional(&mut *tx)
                .await?;

        let folder_name = match folder_row {
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
            .execute(&mut *tx)
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

    tx.commit().await?;

    Ok(BatchMoveCopyResult {
        total: total_items,
        succeeded,
        skipped,
        failed,
        results,
    })
}

/// 批量复制文件
pub async fn batch_copy(
    pool: &SqlitePool,
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

    verify_ownership(pool, user_id, &req.file_ids, &req.folder_ids).await?;

    // 验证目标文件夹
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

    // 防止复制到自身的子文件夹中：若目标位于任一被复制文件夹的子树内，
    // BFS 会对已复制的内容再次复制，产生指数级重复记录并耗尽磁盘/空间。
    if !req.folder_ids.is_empty() {
        if let Some(tfid) = req.target_folder_id {
            let subtree = collect_subtree_folder_ids(pool, &req.folder_ids).await?;
            if subtree.contains(&tfid) {
                return Err(AppError::BadRequest(
                    "不能复制到自身或其子文件夹中".into(),
                ));
            }
        }
    }

    // 收集目标中的现有名称
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

    // 收集目标中的现有文件夹名（用于文件夹复制时的冲突检测）
    let existing_folder_names = if let Some(tfid) = req.target_folder_id {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT name FROM folders WHERE parent_id = ? AND owner_id = ? AND deleted_at IS NULL",
        )
        .bind(tfid)
        .bind(user_id)
        .fetch_all(pool)
        .await?;
        rows.into_iter().map(|(n,)| n).collect::<HashSet<String>>()
    } else {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT name FROM folders WHERE parent_id IS NULL AND owner_id = ? AND deleted_at IS NULL",
        )
        .bind(user_id)
        .fetch_all(pool)
        .await?;
        rows.into_iter().map(|(n,)| n).collect::<HashSet<String>>()
    };

    // 开启事务，保证整批复制要么全部落库、要么全部回滚（避免残留半套副本）。
    let mut tx = pool.begin().await?;

    let mut results = Vec::new();
    let mut succeeded = 0;
    let mut skipped = 0;
    let mut failed = 0;
    let mut current_names = existing_names;
    let mut target_folder_names = existing_folder_names;

    for &file_id in &req.file_ids {
        let file: Option<File> = sqlx::query_as("SELECT * FROM files WHERE id = ? AND deleted_at IS NULL")
            .bind(file_id)
            .fetch_optional(&mut *tx)
            .await?;

        let file = match file {
            Some(f) => f,
            None => {
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
                    // 目标同名文件软删除，保留在回收站可恢复
                    let target_file: Option<(i64,)> = if let Some(tfid) = req.target_folder_id {
                        sqlx::query_as(
                            "SELECT id FROM files WHERE original_name = ? AND folder_id = ? AND owner_id = ? AND deleted_at IS NULL",
                        )
                        .bind(&original_name)
                        .bind(tfid)
                        .bind(user_id)
                        .fetch_optional(&mut *tx)
                        .await?
                    } else {
                        sqlx::query_as(
                            "SELECT id FROM files WHERE original_name = ? AND folder_id IS NULL AND owner_id = ? AND deleted_at IS NULL",
                        )
                        .bind(&original_name)
                        .bind(user_id)
                        .fetch_optional(&mut *tx)
                        .await?
                    };
                    if let Some((tid,)) = target_file {
                        sqlx::query(
                            "UPDATE files SET deleted_at = datetime('now') WHERE id = ? AND owner_id = ? AND deleted_at IS NULL",
                        )
                        .bind(tid)
                        .bind(user_id)
                        .execute(&mut *tx)
                        .await?;
                    }
                    current_names.remove(&original_name);
                }
                "rename" | _ => {
                    new_name = generate_unique_name(&current_names, &original_name);
                }
            }
        }

        // 复制：插入新文件记录，使用相同的存储路径但新名称
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
        .execute(&mut *tx)
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

    // 复制文件夹（使用 BFS 遍历源文件夹树，在目标位置递归创建副本）
    for &folder_id in &req.folder_ids {
        let folder: Option<(String,)> =
            sqlx::query_as("SELECT name FROM folders WHERE id = ? AND deleted_at IS NULL")
                .bind(folder_id)
                .fetch_optional(&mut *tx)
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

        // 解决目标目录中的名称冲突（同时检查文件名和文件夹名）
        let mut combined_names = current_names.clone();
        combined_names.extend(target_folder_names.iter().cloned());
        let new_folder_name = if combined_names.contains(&folder_name) {
            generate_unique_name(&combined_names, &folder_name)
        } else {
            folder_name.clone()
        };

        // 在目标位置创建新文件夹
        let (new_root_id,): (i64,) = sqlx::query_as(
            "INSERT INTO folders (name, owner_id, parent_id) VALUES (?, ?, ?) RETURNING id",
        )
        .bind(&new_folder_name)
        .bind(user_id)
        .bind(req.target_folder_id)
        .fetch_one(&mut *tx)
        .await?;

        target_folder_names.insert(new_folder_name.clone());
        current_names.insert(new_folder_name.clone());

        // BFS 遍历源文件夹树，复制子文件夹和文件
        let mut queue: VecDeque<(i64, i64)> = VecDeque::new();
        queue.push_back((folder_id, new_root_id));
        let mut children_count: i64 = 0;

        while let Some((src_id, dst_id)) = queue.pop_front() {
            // 跟踪目标文件夹中已有的名称（新文件夹为空，但仍用于检测源中可能的同名）
            let mut dst_names: HashSet<String> = HashSet::new();

            // 复制当前源文件夹中的所有文件
            let files: Vec<File> = sqlx::query_as::<_, File>(
                "SELECT * FROM files WHERE folder_id = ? AND deleted_at IS NULL",
            )
            .bind(src_id)
            .fetch_all(&mut *tx)
            .await?;

            for f in files {
                let mut file_name = f.original_name.clone();
                if dst_names.contains(&file_name) {
                    file_name = generate_unique_name(&dst_names, &file_name);
                }
                sqlx::query(
                    "INSERT INTO files (name, original_name, stored_path, preview_path, thumb_path, owner_id, folder_id, size, file_type) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
                )
                .bind(&file_name)
                .bind(&file_name)
                .bind(&f.stored_path)
                .bind(&f.preview_path)
                .bind(&f.thumb_path)
                .bind(user_id)
                .bind(dst_id)
                .bind(f.size)
                .bind(&f.file_type)
                .execute(&mut *tx)
                .await?;
                dst_names.insert(file_name);
                children_count += 1;
            }

            // 创建子文件夹并加入队列继续遍历
            let subfolders: Vec<(i64, String)> = sqlx::query_as(
                "SELECT id, name FROM folders WHERE parent_id = ? AND deleted_at IS NULL",
            )
            .bind(src_id)
            .fetch_all(&mut *tx)
            .await?;

            for (sub_id, sub_name) in subfolders {
                let mut new_sub_name = sub_name.clone();
                if dst_names.contains(&new_sub_name) {
                    new_sub_name = generate_unique_name(&dst_names, &new_sub_name);
                }
                let (new_sub_id,): (i64,) = sqlx::query_as(
                    "INSERT INTO folders (name, owner_id, parent_id) VALUES (?, ?, ?) RETURNING id",
                )
                .bind(&new_sub_name)
                .bind(user_id)
                .bind(dst_id)
                .fetch_one(&mut *tx)
                .await?;
                dst_names.insert(new_sub_name);
                queue.push_back((sub_id, new_sub_id));
                children_count += 1;
            }
        }

        succeeded += 1;
        results.push(BatchItemResult {
            id: folder_id,
            name: folder_name.clone(),
            status: "copied".into(),
            new_name: if new_folder_name != folder_name {
                Some(new_folder_name)
            } else {
                None
            },
            reason: None,
            r#type: Some("folder".into()),
            children_count: Some(children_count),
        });
    }

    tx.commit().await?;

    Ok(BatchMoveCopyResult {
        total: total_items,
        succeeded,
        skipped,
        failed,
        results,
    })
}

/// 批量删除文件和文件夹（软删除，移入回收站）
pub async fn batch_delete(
    pool: &SqlitePool,
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

    // 软删除文件（移入回收站，与单文件删除行为一致）
    for &file_id in &req.file_ids {
        match file_service::soft_delete_file(pool, file_id, user_id).await {
            Ok(()) => {
                deleted += 1;
                results.push(BatchItemResult {
                    id: file_id,
                    name: "(已移入回收站)".to_string(),
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

    // 软删除文件夹（移入回收站，与单文件夹删除行为一致）
    for &folder_id in &req.folder_ids {
        match folder_service::soft_delete_folder(pool, folder_id, user_id).await {
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