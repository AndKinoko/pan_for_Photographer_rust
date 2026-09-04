use std::collections::VecDeque;

use crate::errors::AppError;
use crate::models::folder::Folder;
use sqlx::SqlitePool;

/// 列出用户的文件夹（可选按父文件夹过滤）
/// 自动过滤已软删除的文件夹
pub async fn list_folders(
    pool: &SqlitePool,
    owner_id: i64,
    parent_id: Option<i64>,
) -> Result<Vec<Folder>, AppError> {
    let folders = if let Some(pid) = parent_id {
        sqlx::query_as::<_, Folder>(
            "SELECT * FROM folders WHERE owner_id = ? AND parent_id = ? AND deleted_at IS NULL ORDER BY name",
        )
        .bind(owner_id)
        .bind(pid)
        .fetch_all(pool)
        .await?
    } else {
        sqlx::query_as::<_, Folder>(
            "SELECT * FROM folders WHERE owner_id = ? AND parent_id IS NULL AND deleted_at IS NULL ORDER BY name",
        )
        .bind(owner_id)
        .fetch_all(pool)
        .await?
    };

    Ok(folders)
}

/// 重命名文件夹
pub async fn rename_folder(
    pool: &SqlitePool,
    folder_id: i64,
    owner_id: i64,
    new_name: &str,
) -> Result<Folder, AppError> {
    let new_name = new_name.trim();
    if new_name.is_empty() {
        return Err(AppError::BadRequest("文件夹名称不能为空".into()));
    }

    // 获取文件夹并验证所有权
    let folder = sqlx::query_as::<_, Folder>(
        "SELECT * FROM folders WHERE id = ? AND owner_id = ? AND deleted_at IS NULL",
    )
    .bind(folder_id)
    .bind(owner_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| AppError::NotFound("文件夹不存在".into()))?;

    // 检查同名文件夹（排除自身）
    let existing = if let Some(pid) = folder.parent_id {
        sqlx::query_as::<_, Folder>(
            "SELECT * FROM folders WHERE name = ? AND owner_id = ? AND parent_id = ? AND id != ? AND deleted_at IS NULL",
        )
        .bind(new_name)
        .bind(owner_id)
        .bind(pid)
        .bind(folder_id)
        .fetch_optional(pool)
        .await?
    } else {
        sqlx::query_as::<_, Folder>(
            "SELECT * FROM folders WHERE name = ? AND owner_id = ? AND parent_id IS NULL AND id != ? AND deleted_at IS NULL",
        )
        .bind(new_name)
        .bind(owner_id)
        .bind(folder_id)
        .fetch_optional(pool)
        .await?
    };

    if existing.is_some() {
        return Err(AppError::Conflict("同名文件夹已存在".into()));
    }

    let updated = sqlx::query_as::<_, Folder>(
        "UPDATE folders SET name = ?, updated_at = datetime('now') WHERE id = ? AND owner_id = ? RETURNING *",
    )
    .bind(new_name)
    .bind(folder_id)
    .bind(owner_id)
    .fetch_one(pool)
    .await?;

    Ok(updated)
}

/// 软删除文件夹（移入回收站）
/// 递归软删除所有子文件夹和子文件
pub async fn soft_delete_folder(
    pool: &SqlitePool,
    folder_id: i64,
    owner_id: i64,
) -> Result<(), AppError> {
    // 验证文件夹属于当前用户
    let folder = sqlx::query_as::<_, Folder>(
        "SELECT * FROM folders WHERE id = ? AND owner_id = ? AND deleted_at IS NULL",
    )
    .bind(folder_id)
    .bind(owner_id)
    .fetch_optional(pool)
    .await?;

    if folder.is_none() {
        return Err(AppError::NotFound("文件夹不存在".into()));
    }

    // 使用BFS收集所有子文件夹ID
    let mut folder_ids = vec![folder_id];
    let mut queue: VecDeque<i64> = VecDeque::new();
    queue.push_back(folder_id);

    while let Some(current_id) = queue.pop_front() {
        let subfolders: Vec<(i64,)> = sqlx::query_as(
            "SELECT id FROM folders WHERE parent_id = ? AND deleted_at IS NULL",
        )
        .bind(current_id)
        .fetch_all(pool)
        .await?;

        for (sub_id,) in subfolders {
            folder_ids.push(sub_id);
            queue.push_back(sub_id);
        }
    }

    // 开启事务，保证递归软删除要么全部生效、要么全部回滚
    let mut tx = pool.begin().await?;

    // 软删除所有子文件夹中的文件
    for fid in &folder_ids {
        sqlx::query("UPDATE files SET deleted_at = datetime('now') WHERE folder_id = ? AND deleted_at IS NULL")
            .bind(fid)
            .execute(&mut *tx)
            .await?;
    }

    // 软删除所有收集到的文件夹
    for fid in &folder_ids {
        sqlx::query("UPDATE folders SET deleted_at = datetime('now') WHERE id = ? AND deleted_at IS NULL")
            .bind(fid)
            .execute(&mut *tx)
            .await?;
    }

    tx.commit().await?;

    tracing::info!("文件夹已移入回收站: id={}, 含 {} 个子文件夹", folder_id, folder_ids.len());
    Ok(())
}

/// 从回收站恢复文件夹
pub async fn restore_folder(
    pool: &SqlitePool,
    folder_id: i64,
    owner_id: i64,
) -> Result<(), AppError> {
    // 验证文件夹在回收站中
    let folder = sqlx::query_as::<_, Folder>(
        "SELECT * FROM folders WHERE id = ? AND owner_id = ? AND deleted_at IS NOT NULL",
    )
    .bind(folder_id)
    .bind(owner_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| AppError::NotFound("文件夹不在回收站中".into()))?;

    // 检查父文件夹是否也被删除了
    let restore_to_root = if let Some(pid) = folder.parent_id {
        let parent_exists: Option<(i64,)> = sqlx::query_as(
            "SELECT id FROM folders WHERE id = ? AND owner_id = ? AND deleted_at IS NULL",
        )
        .bind(pid)
        .bind(owner_id)
        .fetch_optional(pool)
        .await?;

        parent_exists.is_none()
    } else {
        false
    };

    // 使用BFS收集所有子文件夹ID（包括已删除的）
    let mut folder_ids = vec![folder_id];
    let mut queue: VecDeque<i64> = VecDeque::new();
    queue.push_back(folder_id);

    while let Some(current_id) = queue.pop_front() {
        let subfolders: Vec<(i64,)> = sqlx::query_as(
            "SELECT id FROM folders WHERE parent_id = ? AND deleted_at IS NOT NULL",
        )
        .bind(current_id)
        .fetch_all(pool)
        .await?;

        for (sub_id,) in subfolders {
            folder_ids.push(sub_id);
            queue.push_back(sub_id);
        }
    }

    // 开启事务，保证恢复流程（含父目录迁移）整体生效
    let mut tx = pool.begin().await?;

    // 恢复所有文件夹
    for fid in &folder_ids {
        sqlx::query("UPDATE folders SET deleted_at = NULL WHERE id = ?")
            .bind(fid)
            .execute(&mut *tx)
            .await?;
    }

    // 恢复所有文件夹中的文件
    for fid in &folder_ids {
        sqlx::query("UPDATE files SET deleted_at = NULL WHERE folder_id = ?")
            .bind(fid)
            .execute(&mut *tx)
            .await?;
    }

    // 如果父文件夹被删除了，将文件夹移到根目录
    if restore_to_root {
        sqlx::query("UPDATE folders SET parent_id = NULL WHERE id = ?")
            .bind(folder_id)
            .execute(&mut *tx)
            .await?;
    }

    tx.commit().await?;

    tracing::info!("文件夹已从回收站恢复: id={}", folder_id);
    Ok(())
}

/// 列出回收站中的文件夹
pub async fn list_trash_folders(
    pool: &SqlitePool,
    owner_id: i64,
) -> Result<Vec<Folder>, AppError> {
    let folders = sqlx::query_as::<_, Folder>(
        "SELECT * FROM folders WHERE owner_id = ? AND deleted_at IS NOT NULL ORDER BY deleted_at DESC",
    )
    .bind(owner_id)
    .fetch_all(pool)
    .await?;

    Ok(folders)
}

/// 永久删除回收站中的文件夹（及其子文件夹和文件）
/// 磁盘清理交由周期 GC（sweeper）统一处理。
pub async fn permanently_delete_folder(
    pool: &SqlitePool,
    folder_id: i64,
    owner_id: i64,
) -> Result<(), AppError> {
    // 验证文件夹属于当前用户
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

    // 使用BFS收集所有子文件夹ID
    let mut folder_ids = vec![folder_id];
    let mut queue: VecDeque<i64> = VecDeque::new();
    queue.push_back(folder_id);

    while let Some(current_id) = queue.pop_front() {
        let subfolders: Vec<(i64,)> = sqlx::query_as(
            "SELECT id FROM folders WHERE parent_id = ? AND owner_id = ?",
        )
        .bind(current_id)
        .bind(owner_id)
        .fetch_all(pool)
        .await?;

        for (sub_id,) in subfolders {
            folder_ids.push(sub_id);
            queue.push_back(sub_id);
        }
    }

    // 开启事务，保证文件记录与文件夹记录删除要么全部成功、要么全部回滚
    let mut tx = pool.begin().await?;

    // 删除所有文件夹中的文件记录（物理文件交由 GC 处理）
    for fid in &folder_ids {
        sqlx::query("DELETE FROM files WHERE folder_id = ?")
            .bind(fid)
            .execute(&mut *tx)
            .await?;
    }

    // 删除文件夹记录
    for fid in folder_ids.iter().rev() {
        sqlx::query("DELETE FROM folders WHERE id = ?")
            .bind(fid)
            .execute(&mut *tx)
            .await?;
    }

    tx.commit().await?;

    tracing::info!("文件夹已永久删除: id={}", folder_id);
    Ok(())
}

/// 创建新文件夹
pub async fn create_folder(
    pool: &SqlitePool,
    owner_id: i64,
    name: &str,
    parent_id: Option<i64>,
) -> Result<Folder, AppError> {
    // 如果指定了父文件夹，验证其属于当前用户
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

    // 检查是否存在同名文件夹（排除已删除的）
    let existing = if let Some(pid) = parent_id {
        sqlx::query_as::<_, Folder>(
            "SELECT * FROM folders WHERE name = ? AND owner_id = ? AND parent_id = ? AND deleted_at IS NULL",
        )
        .bind(name)
        .bind(owner_id)
        .bind(pid)
        .fetch_optional(pool)
        .await?
    } else {
        sqlx::query_as::<_, Folder>(
            "SELECT * FROM folders WHERE name = ? AND owner_id = ? AND parent_id IS NULL AND deleted_at IS NULL",
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

/// 获取文件夹的面包屑导航路径
pub async fn get_breadcrumbs(
    pool: &SqlitePool,
    folder_id: i64,
) -> Result<Vec<Folder>, AppError> {
    let mut breadcrumbs = Vec::new();
    let mut current_id = Some(folder_id);

    while let Some(cid) = current_id {
        let folder = sqlx::query_as::<_, Folder>("SELECT * FROM folders WHERE id = ? AND deleted_at IS NULL")
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