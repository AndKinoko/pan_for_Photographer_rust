use std::collections::VecDeque;

use crate::errors::AppError;
use crate::models::folder::{Folder, FolderInfo};
use sqlx::SqlitePool;

/// 列出用户的文件夹（可选按父文件夹过滤），**带文件数与子文件夹数**。
/// 自动过滤已软删除的文件夹。
///
/// 为什么返回 `FolderInfo` 而不是 `Folder`：前端的文件夹卡片要显示
/// 「N 个文件 · M 个子文件夹」（`FileCard.vue` 读 `file_count` /
/// `subfolder_count`），而 `Folder` 结构体里没有这两个字段，于是
/// `FileCard.vue` 的判断恒为 false、所有卡片都掉进「显示创建时间」的
/// 兜底分支。`FolderInfo` 早就定义好了却一直没人用。
///
/// 计数用**一条 SQL + LEFT JOIN + 聚合子查询**取，而不是在 Rust 里
/// 逐个文件夹再查一次（N+1：100 个文件夹 = 101 次查询）。
/// 只统计未软删除的记录，与列表本身的过滤口径一致。
pub async fn list_folders(
    pool: &SqlitePool,
    owner_id: i64,
    parent_id: Option<i64>,
) -> Result<Vec<FolderInfo>, AppError> {
    let folders = sqlx::query_as::<_, FolderInfo>(
        r#"
        SELECT f.id, f.name, f.owner_id, f.parent_id, f.created_at, f.updated_at, f.deleted_at,
               COALESCE(fc.cnt, 0) AS file_count,
               COALESCE(sc.cnt, 0) AS subfolder_count
        FROM folders f
        LEFT JOIN (
            SELECT folder_id, COUNT(*) AS cnt FROM files
            WHERE deleted_at IS NULL AND folder_id IS NOT NULL
            GROUP BY folder_id
        ) fc ON fc.folder_id = f.id
        LEFT JOIN (
            SELECT parent_id AS pid, COUNT(*) AS cnt FROM folders
            WHERE deleted_at IS NULL AND parent_id IS NOT NULL
            GROUP BY parent_id
        ) sc ON sc.pid = f.id
        WHERE f.owner_id = ? AND f.deleted_at IS NULL
          AND ((? IS NOT NULL AND f.parent_id = ?) OR (? IS NULL AND f.parent_id IS NULL))
        ORDER BY f.name
        "#,
    )
    .bind(owner_id)
    .bind(parent_id)
    .bind(parent_id)
    .bind(parent_id)
    .fetch_all(pool)
    .await?;

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
        // owner_id 是纵深防御：folder_ids 来自本用户的文件夹树，理论上已是自己的，
        // 但 SQL 自身不带归属约束时，一旦上游判定出缺口就会连带删掉别人的文件。
        sqlx::query("UPDATE files SET deleted_at = datetime('now') WHERE folder_id = ? AND owner_id = ? AND deleted_at IS NULL")
            .bind(fid)
            .bind(owner_id)
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
        sqlx::query("UPDATE files SET deleted_at = NULL WHERE folder_id = ? AND owner_id = ?")
            .bind(fid)
            .bind(owner_id)
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
///
/// 同样返回 `FolderInfo`（带计数）：回收站页面复用同一套 `FileCard` 组件，
/// 缺了 `file_count` / `subfolder_count` 会退回显示创建时间。
/// 这里的计数口径是「**未被软删除**的文件/子文件夹」——已删的同级不该
/// 再给用户一个「里面还有 N 个」的错觉。
pub async fn list_trash_folders(
    pool: &SqlitePool,
    owner_id: i64,
) -> Result<Vec<FolderInfo>, AppError> {
    let folders = sqlx::query_as::<_, FolderInfo>(
        r#"
        SELECT f.id, f.name, f.owner_id, f.parent_id, f.created_at, f.updated_at, f.deleted_at,
               COALESCE(fc.cnt, 0) AS file_count,
               COALESCE(sc.cnt, 0) AS subfolder_count
        FROM folders f
        LEFT JOIN (
            SELECT folder_id, COUNT(*) AS cnt FROM files
            WHERE deleted_at IS NULL AND folder_id IS NOT NULL
            GROUP BY folder_id
        ) fc ON fc.folder_id = f.id
        LEFT JOIN (
            SELECT parent_id AS pid, COUNT(*) AS cnt FROM folders
            WHERE deleted_at IS NULL AND parent_id IS NOT NULL
            GROUP BY parent_id
        ) sc ON sc.pid = f.id
        WHERE f.owner_id = ? AND f.deleted_at IS NOT NULL
        ORDER BY f.deleted_at DESC, f.id DESC
        "#,
    )
    .bind(owner_id)
    .fetch_all(pool)
    .await?;

    Ok(folders)
}

/// 回收站里的文件夹总数。与 [`list_trash_folders`] 条件一致，
/// 用于在响应里给出总数（前端显示「共 N 项」）。
///
/// 文件夹不分页：一次查询能返回几百个目录的场景在本项目里不成立
/// （交付场景下同一层目录通常是个位数），而给两个独立排序的列表各配一套游标
/// 会让接口和前端状态机都复杂一倍。真正需要分页的是照片。
pub async fn count_trash_folders(pool: &SqlitePool, owner_id: i64) -> Result<i64, AppError> {
    let n: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM folders WHERE owner_id = ? AND deleted_at IS NOT NULL")
            .bind(owner_id)
            .fetch_one(pool)
            .await?;
    Ok(n)
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
        sqlx::query("DELETE FROM files WHERE folder_id = ? AND owner_id = ?")
            .bind(fid)
            .bind(owner_id)
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
///
/// `owner_id` 不是可选的：面包屑沿着 parent_id 一路向上，**中途任何一跳
/// 都不能跨用户**。少了它，任意登录用户只要构造 `parent_id=<他人文件夹ID>`
/// 就能拿到对方整条祖先链（文件夹名在交付场景里常含客户姓名），
/// 而 folders.id 是稠密自增整数，遍历成本几乎为零。
/// 归属不符时返回空 vec——与 `list_folders`「查不到就是查不到」的语义一致，
/// 不区分「不存在」和「不属于你」，避免变成归属探测工具。
pub async fn get_breadcrumbs(
    pool: &SqlitePool,
    owner_id: i64,
    folder_id: i64,
) -> Result<Vec<Folder>, AppError> {
    let mut breadcrumbs = Vec::new();
    let mut current_id = Some(folder_id);

    while let Some(cid) = current_id {
        let folder = sqlx::query_as::<_, Folder>(
            "SELECT * FROM folders WHERE id = ? AND owner_id = ? AND deleted_at IS NULL",
        )
        .bind(cid)
        .bind(owner_id)
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