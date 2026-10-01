use std::collections::HashMap;

use uuid::Uuid;

use crate::errors::AppError;
use crate::models::share::{FileShare, ItemKind, ShareInfo, ShareItem, ShareItemInfo};
use crate::utils::crypto;
use sqlx::SqlitePool;

/// 一个批次的递归统计。
#[derive(Debug, Default, Clone, Copy)]
struct ShareStats {
    /// 展开所有文件夹后的文件总数
    total_file_count: i64,
    /// 递归文件夹总数（含批次里直接列出的那些）
    total_folder_count: i64,
    /// 这些文件加起来多少字节
    total_size: i64,
}

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
        if crate::utils::time::is_expired_utc(expires_at) {
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

/// 创建一批分享（批次）。
///
/// `items` 是「一批条目」而不是单个目标：批次里可以有多个文件、多个文件夹，
/// 也可以两者混装。所以落库是往关联表插多行，而不是往 `file_shares` 上写
/// 一两个外键列。
///
/// `password_hash` 是**已经算好的** bcrypt 哈希（None 表示无密码），
/// 本函数不再自己调 bcrypt：那是 250ms 的同步 CPU 运算，不能在这个 async
/// 函数里裸调（见 `utils::crypto`）；而且一个批次只对应一个口令，调用方
/// 哈希一次即可，不存在被放大的余地。
pub async fn create_share(
    pool: &SqlitePool,
    items: &[(ItemKind, i64)],
    owner_id: i64,
    expires_hours: Option<i64>,
    password_hash: Option<String>,
    max_downloads: Option<i64>,
    custom_code: Option<String>,
) -> Result<FileShare, AppError> {
    if items.is_empty() {
        return Err(AppError::BadRequest("必须至少选择一个文件或文件夹".into()));
    }

    // 去重后数量必须一致：同一项出现两次没有意义，而且会让「顶层条目数」
    // 与实际内容对不上（界面显示 5 项、实际只有 4 项）。
    let mut unique: Vec<(ItemKind, i64)> = items.to_vec();
    unique.sort_by_key(|(k, id)| (k.as_str(), *id));
    unique.dedup();
    if unique.len() != items.len() {
        return Err(AppError::BadRequest("同一项不能重复添加".into()));
    }

    verify_ownership(pool, owner_id, &unique).await?;

    let share_id = if let Some(ref code) = custom_code {
        let code = code.trim();
        if code.is_empty() {
            return Err(AppError::BadRequest("自定义分享码不能为空".into()));
        }
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

    let password_hash = password_hash.unwrap_or_default();

    let expires_at = match expires_hours {
        Some(hours) if hours > 0 => Some(crate::utils::time::utc_string_after_hours(hours)),
        _ => None,
    };

    let mut tx = pool.begin().await?;

    let share = sqlx::query_as::<_, FileShare>(
        r#"INSERT INTO file_shares
               (id, owner_id, expires_at, password_hash, max_downloads, custom_code)
           VALUES (?, ?, ?, ?, ?, ?) RETURNING *"#,
    )
    .bind(&share_id)
    .bind(owner_id)
    .bind(&expires_at)
    .bind(&password_hash)
    .bind(max_downloads)
    .bind(custom_code.as_ref().map(|s| s.trim()))
    .fetch_one(&mut *tx)
    .await?;

    // 建批次与写条目必须同一个事务：否则可能出现「分享存在但空无一物」，
    // 客户打开链接只看到一片空白，而这种状态在库里看不出任何异常。
    for (kind, id) in &unique {
        sqlx::query(
            "INSERT INTO share_items (share_id, item_type, item_id) VALUES (?, ?, ?)",
        )
        .bind(&share_id)
        .bind(kind.as_str())
        .bind(id)
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;
    Ok(share)
}

/// 校验这批条目的归属，**一次 SQL 查完全部**。
///
/// 全有或全无：任何一项不属于当前用户或已进回收站，整批拒绝，不做部分成功。
/// 半批分享出去比直接报错更难排查——用户以为自己分享了 10 张，实际只有 8 张，
/// 而且没有任何提示。
async fn verify_ownership(
    pool: &SqlitePool,
    owner_id: i64,
    items: &[(ItemKind, i64)],
) -> Result<(), AppError> {
    let file_ids: Vec<i64> = items
        .iter()
        .filter(|(k, _)| *k == ItemKind::File)
        .map(|(_, id)| *id)
        .collect();
    let folder_ids: Vec<i64> = items
        .iter()
        .filter(|(k, _)| *k == ItemKind::Folder)
        .map(|(_, id)| *id)
        .collect();

    if !file_ids.is_empty() {
        let (found,): (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM files
              WHERE id IN (SELECT value FROM json_each(?)) AND owner_id = ? AND deleted_at IS NULL",
        )
        .bind(serde_json::to_string(&file_ids).unwrap_or_else(|_| "[]".into()))
        .bind(owner_id)
        .fetch_one(pool)
        .await?;

        if found != file_ids.len() as i64 {
            return Err(AppError::NotFound(
                "部分文件不存在、不属于你，或已在回收站中".into(),
            ));
        }
    }

    if !folder_ids.is_empty() {
        let (found,): (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM folders
              WHERE id IN (SELECT value FROM json_each(?)) AND owner_id = ? AND deleted_at IS NULL",
        )
        .bind(serde_json::to_string(&folder_ids).unwrap_or_else(|_| "[]".into()))
        .bind(owner_id)
        .fetch_one(pool)
        .await?;

        if found != folder_ids.len() as i64 {
            return Err(AppError::NotFound(
                "部分文件夹不存在、不属于你，或已在回收站中".into(),
            ));
        }
    }

    Ok(())
}

/// 列出用户的所有分享（批次）。
///
/// 条目元信息用**常数条 SQL** 取回（两条 IN 查询 + 两条统计），与分享数量无关。
/// 原先这里是 `for share { share_to_info(share).await }` —— 每个分享一次查询，
/// 而注释还写着「使用 JOIN 优化 N+1 查询」，与实现完全相反。
pub async fn list_shares(pool: &SqlitePool, owner_id: i64) -> Result<Vec<ShareInfo>, AppError> {
    let shares = sqlx::query_as::<_, FileShare>(
        "SELECT * FROM file_shares WHERE owner_id = ? ORDER BY created_at DESC",
    )
    .bind(owner_id)
    .fetch_all(pool)
    .await?;

    build_infos(pool, shares, true, false).await
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

    Ok(build_infos(pool, vec![share], true, false)
        .await?
        .pop()
        .expect("刚传入一个元素"))
}

/// 根据ID获取分享（公开访问，先验证）。
///
/// `unlocked` 表示「本次请求是否已经通过了密码校验」（见
/// `handlers::share::public_share_access`）。它只影响**是否下发媒体 URL**，
/// 不影响分享本身是否可访问 —— 那是 `validate_share` 的职责。
pub async fn get_public_share(
    pool: &SqlitePool,
    share_id: &str,
    unlocked: bool,
) -> Result<ShareInfo, AppError> {
    let share = validate_share(pool, share_id).await?;
    Ok(build_infos(pool, vec![share], unlocked, true)
        .await?
        .pop()
        .expect("刚传入一个元素"))
}

/// 删除分享。`share_items` 由外键 `ON DELETE CASCADE` 一并清理。
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

/// 停用所有包含这些条目的活跃分享，返回被停用的 `(item_id, 项名, share_id)`。
///
/// 批次模型下「取消分享某个文件」的语义变了：一个文件可能同时出现在好几个
/// 批次里，而分享是**批次级**的——没法只把其中一个文件从某个批次里摘掉。
/// 所以这里的含义是「把这些项所在的分享整体停用」，前端必须在确认框里
/// 把这一点说清楚，否则用户会以为只取消了一个文件。
pub async fn deactivate_shares_containing(
    pool: &SqlitePool,
    owner_id: i64,
    items: &[(ItemKind, i64)],
) -> Result<Vec<(i64, String, String)>, AppError> {
    let mut touched: Vec<(i64, String, String)> = Vec::new();

    for (kind, id) in items {
        let share_rows: Vec<(String,)> = sqlx::query_as(
            "SELECT fs.id FROM file_shares fs
               JOIN share_items si ON si.share_id = fs.id
              WHERE fs.owner_id = ? AND fs.is_active = 1
                AND si.item_type = ? AND si.item_id = ?",
        )
        .bind(owner_id)
        .bind(kind.as_str())
        .bind(id)
        .fetch_all(pool)
        .await?;

        let name_row: Option<(String,)> = if *kind == ItemKind::File {
            sqlx::query_as("SELECT original_name FROM files WHERE id = ? AND owner_id = ?")
                .bind(id)
                .bind(owner_id)
                .fetch_optional(pool)
                .await?
        } else {
            sqlx::query_as("SELECT name FROM folders WHERE id = ? AND owner_id = ?")
                .bind(id)
                .bind(owner_id)
                .fetch_optional(pool)
                .await?
        };
        let name = name_row
            .map(|(n,)| n)
            .unwrap_or_else(|| "(未知)".to_string());

        for (share_id,) in share_rows {
            sqlx::query("UPDATE file_shares SET is_active = 0 WHERE id = ? AND owner_id = ?")
                .bind(&share_id)
                .bind(owner_id)
                .execute(pool)
                .await?;
            touched.push((*id, name.clone(), share_id));
        }
    }

    Ok(touched)
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

    // 走异步入口：bcrypt 是 250ms 的同步 CPU 运算，裸调会占死一个 tokio worker。
    // 这个接口是**无鉴权**的（拿到分享链接即可调用），正是最不该阻塞的地方。
    crypto::verify_password_async(password, &share.password_hash).await
}

/// 占用一次下载额度：检查剩余次数并在**同一个事务里**自增。
///
/// 为什么不用「先 validate_share 检查、再单独 increment」：
/// 那两步分属两个查询、两个事务。并发 N 个请求会同时读到
/// `download_count = max - 1`，各自判定「还有额度」，然后全部自增——
///
/// **限额分享实际发出 max + (N-1) 次下载。**
///
/// 这里用 `UPDATE ... WHERE download_count < max_downloads` 让数据库自己裁决：
/// 受影响行数为 0 就说明额度已尽，直接拒绝。检查与占用因此是原子的，
/// 不依赖应用层加锁。
///
/// 批次模型下的口径：**按文件扣，每下一个文件扣一次**。`max_downloads = 10`
/// 的含义是「客户最多能下载 10 个文件」——比「整个批次共用一个池」直观得多，
/// 后者 1 次额度就能把整包拉走，限制等于没有。
///
/// 调用方应当**先确认文件可读再占用**——顺序反了会把额度记在没真正发出的
/// 下载上。进目录、看缩略图、看预览都不扣（派生图不算「交付」）。
pub async fn consume_download_slot(pool: &SqlitePool, share_id: &str) -> Result<(), AppError> {
    // max_downloads 为 NULL 表示不限次数，此时只自增、不做拦截。
    let affected = sqlx::query(
        "UPDATE file_shares SET download_count = download_count + 1
         WHERE id = ?
           AND (max_downloads IS NULL OR download_count < max_downloads)",
    )
    .bind(share_id)
    .execute(pool)
    .await?
    .rows_affected();

    if affected == 0 {
        // 两种可能：分享不存在，或额度已尽。
        // 刻意不区分——区分开就成了探测分享码是否有效的旁路。
        return Err(AppError::Gone("分享链接的下载次数已用尽".into()));
    }
    Ok(())
}

// ===========================================================================
// 授权：请求的条目是否落在批次范围内
// ---------------------------------------------------------------------------
// 这是整个批次功能**唯一的越权面**。批次把「一个分享指向一个目标」放宽成
// 「一个分享指向一批目标」，客户端于是可以指定 file_id / folder_id 来请求
// 具体内容——这个指定若不校验，拿到任意一个分享链接就等于拿到了该账号的
// **全部文件**。
//
// 判定规则：
//   · 文件：直接列在批次里，**或**位于批次内某个文件夹的子树里（任意深度）
//   · 文件夹：是该子树的成员（含批次里直接列出的那个根目录本身）
//
// 用递归 CTE 而不是 `folder_service` 里那三处逐层 BFS —— 后者每层一次查询，
// 目录深了就是 N 次往返，而这是**每个下载请求都要跑**的热路径。
// ===========================================================================

/// 递归 CTE：批次内所有文件夹的整棵子树（含根目录本身）。
///
/// 每一跳都带 `owner_id` 是纵深防御：子树靠 `parent_id` 串联，理论上不会
/// 跨用户，但一旦上游出现缺口，这里少一层约束就是「把别人的文件夹也划进
/// 授权范围」。与 `soft_delete_folder` 里同一个理由。
const SUBTREE_CTE: &str = r#"
WITH RECURSIVE subtree(id) AS (
    SELECT si.item_id
      FROM share_items si
      JOIN folders f ON f.id = si.item_id
     WHERE si.share_id = ?1 AND si.item_type = 'folder'
       AND f.owner_id = ?2 AND f.deleted_at IS NULL
    UNION
    SELECT f.id
      FROM folders f
      JOIN subtree s ON f.parent_id = s.id
     WHERE f.owner_id = ?2 AND f.deleted_at IS NULL
)
"#;

/// 这个文件是否在批次的授权范围内。
pub async fn file_in_share(
    pool: &SqlitePool,
    share_id: &str,
    owner_id: i64,
    file_id: i64,
) -> Result<bool, AppError> {
    let sql = format!(
        r#"{SUBTREE_CTE}
        SELECT
          EXISTS (SELECT 1 FROM share_items si
                   WHERE si.share_id = ?1 AND si.item_type = 'file' AND si.item_id = ?3)
          OR EXISTS (SELECT 1 FROM files f
                      WHERE f.id = ?3 AND f.owner_id = ?2 AND f.deleted_at IS NULL
                        AND f.folder_id IN (SELECT id FROM subtree))"#
    );

    let (ok,): (i64,) = sqlx::query_as(&sql)
        .bind(share_id)
        .bind(owner_id)
        .bind(file_id)
        .fetch_one(pool)
        .await?;

    Ok(ok != 0)
}

/// 这个文件夹是否在批次的授权范围内（含批次里直接列出的那些根目录）。
pub async fn folder_in_share(
    pool: &SqlitePool,
    share_id: &str,
    owner_id: i64,
    folder_id: i64,
) -> Result<bool, AppError> {
    let sql = format!("{SUBTREE_CTE} SELECT EXISTS (SELECT 1 FROM subtree WHERE id = ?3)");

    let (ok,): (i64,) = sqlx::query_as(&sql)
        .bind(share_id)
        .bind(owner_id)
        .bind(folder_id)
        .fetch_one(pool)
        .await?;

    Ok(ok != 0)
}

/// 公开侧的面包屑：从**批次根**到 `folder_id`（含两端）。
///
/// 不能用 `folder_service::get_breadcrumbs`：它一路走到账号根目录，会把
/// 批次范围之外的祖先目录名一并暴露给客户。交付场景里那些名字常含客户姓名
/// 或拍摄活动（`客户A / 婚礼 / 精修` 里的 `客户A` 就不该出现在这个只分享了
/// `婚礼` 的链接上）。
///
/// 往上走，直到遇到一个「直接列在批次里的文件夹」为止——那一层就是根。
pub async fn breadcrumbs_from_share(
    pool: &SqlitePool,
    share_id: &str,
    owner_id: i64,
    folder_id: i64,
) -> Result<Vec<crate::models::share::Crumb>, AppError> {
    let mut chain: Vec<crate::models::share::Crumb> = Vec::new();
    let mut current = Some(folder_id);
    // 深度上限：数据库一旦出现 parent_id 成环（理论上不该有，但没约束拦着），
    // 没有它就是一个把请求线程挂死的死循环。
    let mut depth = 0usize;

    while let Some(cid) = current {
        if depth >= MAX_BREADCRUMB_DEPTH {
            tracing::warn!("面包屑深度超限，可能存在环: folder_id={}", folder_id);
            break;
        }
        depth += 1;

        let row: Option<(i64, String, Option<i64>)> = sqlx::query_as(
            "SELECT id, name, parent_id FROM folders
              WHERE id = ? AND owner_id = ? AND deleted_at IS NULL",
        )
        .bind(cid)
        .bind(owner_id)
        .fetch_optional(pool)
        .await?;

        let Some((id, name, parent)) = row else { break };
        chain.push(crate::models::share::Crumb { id, name });

        if is_share_root(pool, share_id, id).await? {
            break;
        }
        current = parent;
    }

    chain.reverse();
    Ok(chain)
}

/// 目录深度上限。目录树正常不会超过十几层，64 纯属防御。
const MAX_BREADCRUMB_DEPTH: usize = 64;

async fn is_share_root(
    pool: &SqlitePool,
    share_id: &str,
    folder_id: i64,
) -> Result<bool, AppError> {
    let (n,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM share_items
          WHERE share_id = ? AND item_type = 'folder' AND item_id = ?",
    )
    .bind(share_id)
    .bind(folder_id)
    .fetch_one(pool)
    .await?;
    Ok(n > 0)
}

// ===========================================================================
// 内部：元信息装载与拼装
// ===========================================================================

/// 条目指向的对象。查不到就是「已删除 / 已硬删」。
#[derive(Debug, Default, Clone)]
struct ItemMeta {
    name: String,
    file_type: String,
    size: i64,
    uploaded_at: Option<String>,
    preview_path: Option<String>,
    thumb_path: Option<String>,
    /// 文件夹专有：直接子级计数
    file_count: i64,
    subfolder_count: i64,
}

/// 文件行：`(id, 原名, 类型, 大小, 上传时间, 预览路径, 缩略图路径)`。
/// 抽成别名是因为这段元组写在 `query_as` 的类型参数里，直接内联会顶到
/// clippy 的复杂度阈值，可读性也差。
type FileMetaRow = (i64, String, String, i64, Option<String>, Option<String>, Option<String>);

/// 一次取回这批条目涉及的全部文件/文件夹元信息。
async fn load_item_metas(
    pool: &SqlitePool,
    refs: &[(ItemKind, i64)],
) -> Result<HashMap<(ItemKind, i64), ItemMeta>, AppError> {
    let mut out: HashMap<(ItemKind, i64), ItemMeta> = HashMap::new();

    let file_ids: Vec<i64> = refs
        .iter()
        .filter(|(k, _)| *k == ItemKind::File)
        .map(|(_, id)| *id)
        .collect();
    if !file_ids.is_empty() {
        let rows: Vec<FileMetaRow> = sqlx::query_as(
                "SELECT id, original_name, file_type, size, uploaded_at, preview_path, thumb_path
                   FROM files
                  WHERE id IN (SELECT value FROM json_each(?)) AND deleted_at IS NULL",
            )
            .bind(serde_json::to_string(&file_ids).unwrap_or_else(|_| "[]".into()))
            .fetch_all(pool)
            .await?;

        for (id, name, file_type, size, uploaded_at, preview_path, thumb_path) in rows {
            out.insert(
                (ItemKind::File, id),
                ItemMeta {
                    name,
                    file_type,
                    size,
                    uploaded_at,
                    preview_path,
                    thumb_path,
                    ..Default::default()
                },
            );
        }
    }

    let folder_ids: Vec<i64> = refs
        .iter()
        .filter(|(k, _)| *k == ItemKind::Folder)
        .map(|(_, id)| *id)
        .collect();
    if !folder_ids.is_empty() {
        // 计数用一条 SQL 带聚合子查询取，而不是逐个文件夹再查一次。
        let rows: Vec<(i64, String, i64, i64)> = sqlx::query_as(
            "SELECT f.id, f.name,
                    COALESCE(fc.cnt, 0) AS file_count,
                    COALESCE(sc.cnt, 0) AS subfolder_count
               FROM folders f
               LEFT JOIN (SELECT folder_id, COUNT(*) AS cnt FROM files
                           WHERE deleted_at IS NULL AND folder_id IS NOT NULL
                           GROUP BY folder_id) fc ON fc.folder_id = f.id
               LEFT JOIN (SELECT parent_id AS pid, COUNT(*) AS cnt FROM folders
                           WHERE deleted_at IS NULL AND parent_id IS NOT NULL
                           GROUP BY parent_id) sc ON sc.pid = f.id
              WHERE f.id IN (SELECT value FROM json_each(?)) AND f.deleted_at IS NULL",
        )
        .bind(serde_json::to_string(&folder_ids).unwrap_or_else(|_| "[]".into()))
        .fetch_all(pool)
        .await?;

        for (id, name, file_count, subfolder_count) in rows {
            out.insert(
                (ItemKind::Folder, id),
                ItemMeta {
                    name,
                    file_type: "folder".to_string(),
                    file_count,
                    subfolder_count,
                    ..Default::default()
                },
            );
        }
    }

    Ok(out)
}

/// 这些批次的递归统计。**两条 SQL 覆盖全部批次**，与分享数量无关。
async fn load_stats(
    pool: &SqlitePool,
    share_ids: &[String],
) -> Result<HashMap<String, ShareStats>, AppError> {
    let mut out: HashMap<String, ShareStats> = HashMap::new();
    if share_ids.is_empty() {
        return Ok(out);
    }
    let ids = serde_json::to_string(share_ids).unwrap_or_else(|_| "[]".into());

    // 子树（文件夹）与「范围内的文件」都只保留未删除的行：已进回收站的内容
    // 对客户不可见，也不该计入「这批交付了什么」。
    //
    // `UNION` 而不是 `UNION ALL`：同一个文件既被直接列出、又落在某个被列出的
    // 文件夹里时只算一次，否则条目数会虚高。
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        r#"
        WITH RECURSIVE subtree(share_id, folder_id) AS (
            SELECT si.share_id, si.item_id
              FROM share_items si
              JOIN folders f ON f.id = si.item_id
             WHERE si.item_type = 'folder' AND f.deleted_at IS NULL
            UNION
            SELECT s.share_id, f.id
              FROM folders f JOIN subtree s ON f.parent_id = s.folder_id
             WHERE f.deleted_at IS NULL
        ),
        scoped(share_id, file_id) AS (
            SELECT si.share_id, si.item_id
              FROM share_items si
              JOIN files f ON f.id = si.item_id
             WHERE si.item_type = 'file' AND f.deleted_at IS NULL
            UNION
            SELECT s.share_id, f.id
              FROM files f JOIN subtree s ON f.folder_id = s.folder_id
             WHERE f.deleted_at IS NULL
        )
        SELECT sc.share_id, COUNT(*) AS file_count, COALESCE(SUM(f.size), 0) AS total_size
          FROM scoped sc JOIN files f ON f.id = sc.file_id
         WHERE sc.share_id IN (SELECT value FROM json_each(?))
         GROUP BY sc.share_id
        "#,
    )
    .bind(&ids)
    .fetch_all(pool)
    .await?;

    for (share_id, total_file_count, total_size) in rows {
        let e = out.entry(share_id).or_default();
        e.total_file_count = total_file_count;
        e.total_size = total_size;
    }

    let folder_rows: Vec<(String, i64)> = sqlx::query_as(
        r#"
        WITH RECURSIVE subtree(share_id, folder_id) AS (
            SELECT si.share_id, si.item_id
              FROM share_items si
              JOIN folders f ON f.id = si.item_id
             WHERE si.item_type = 'folder' AND f.deleted_at IS NULL
            UNION
            SELECT s.share_id, f.id
              FROM folders f JOIN subtree s ON f.parent_id = s.folder_id
             WHERE f.deleted_at IS NULL
        )
        SELECT share_id, COUNT(*) FROM subtree
         WHERE share_id IN (SELECT value FROM json_each(?))
         GROUP BY share_id
        "#,
    )
    .bind(&ids)
    .fetch_all(pool)
    .await?;

    for (share_id, n) in folder_rows {
        out.entry(share_id).or_default().total_folder_count = n;
    }

    Ok(out)
}

/// 把一批 `FileShare` 拼成对外结构。调用方给什么就返回什么，顺序保持一致。
///
/// `unlocked`：为 false 时不下发任何媒体 URL（受密码保护且访客尚未验码）。
/// `hide_missing`：条目指向的对象已被删除时，公开侧直接过滤掉（客户不该看到
/// 「(已删除)」这种幽灵行）；所有者侧保留占位，让他知道这批东西坏了。
async fn build_infos(
    pool: &SqlitePool,
    shares: Vec<FileShare>,
    unlocked: bool,
    hide_missing: bool,
) -> Result<Vec<ShareInfo>, AppError> {
    if shares.is_empty() {
        return Ok(Vec::new());
    }

    let share_ids: Vec<String> = shares.iter().map(|s| s.id.clone()).collect();
    let ids = serde_json::to_string(&share_ids).unwrap_or_else(|_| "[]".into());

    // 一次取回所有批次的条目
    let mut items_by_share: HashMap<String, Vec<ShareItem>> = HashMap::new();
    let all_items: Vec<ShareItem> = sqlx::query_as(
        "SELECT * FROM share_items WHERE share_id IN (SELECT value FROM json_each(?))
          ORDER BY CASE item_type WHEN 'folder' THEN 0 ELSE 1 END, item_id",
    )
    .bind(&ids)
    .fetch_all(pool)
    .await?;
    for item in all_items {
        items_by_share
            .entry(item.share_id.clone())
            .or_default()
            .push(item);
    }

    // 一次取回所有条目指向的对象
    let refs: Vec<(ItemKind, i64)> = items_by_share
        .values()
        .flatten()
        .filter_map(|i| i.kind().map(|k| (k, i.item_id)))
        .collect();
    let metas = load_item_metas(pool, &refs).await?;
    let stats = load_stats(pool, &share_ids).await?;

    // 所有者名：一次查完，避免每个分享一条查询
    let owner_ids: Vec<i64> = {
        let mut v: Vec<i64> = shares.iter().map(|s| s.owner_id).collect();
        v.sort_unstable();
        v.dedup();
        v
    };
    let owner_rows: Vec<(i64, String)> = sqlx::query_as(
        "SELECT id, username FROM users WHERE id IN (SELECT value FROM json_each(?))",
    )
    .bind(serde_json::to_string(&owner_ids).unwrap_or_else(|_| "[]".into()))
    .fetch_all(pool)
    .await?;
    let owner_names: HashMap<i64, String> = owner_rows.into_iter().collect();

    let mut out = Vec::with_capacity(shares.len());
    for share in shares {
        let empty = Vec::new();
        let share_items = items_by_share.get(&share.id).unwrap_or(&empty);
        let st = stats.get(&share.id).copied().unwrap_or_default();
        let owner_name = owner_names
            .get(&share.owner_id)
            .cloned()
            .unwrap_or_else(|| "(用户已删除)".to_string());

        let mut infos: Vec<ShareItemInfo> = Vec::with_capacity(share_items.len());
        let (mut file_count, mut folder_count) = (0i64, 0i64);

        for item in share_items {
            let Some(kind) = item.kind() else { continue };

            match metas.get(&(kind, item.item_id)) {
                Some(meta) => {
                    if kind == ItemKind::File {
                        file_count += 1;
                    } else {
                        folder_count += 1;
                    }
                    infos.push(share_item_info(&share.id, kind, item.item_id, meta, unlocked));
                }
                None if hide_missing => continue,
                None => infos.push(ShareItemInfo {
                    item_type: kind.as_str().to_string(),
                    id: item.item_id,
                    name: "(已删除)".to_string(),
                    size: 0,
                    formatted_size: crate::models::file::format_file_size(0),
                    file_type: if kind == ItemKind::Folder {
                        "folder".to_string()
                    } else {
                        String::new()
                    },
                    uploaded_at: None,
                    has_preview: false,
                    preview_url: None,
                    thumb_url: None,
                    download_url: None,
                    file_count: None,
                    subfolder_count: None,
                }),
            }
        }

        let item_count = infos.len() as i64;
        out.push(ShareInfo {
            id: share.id.clone(),
            owner_id: share.owner_id,
            owner_name,
            created_at: share.created_at.clone(),
            expires_at: share.expires_at.clone(),
            has_password: !share.password_hash.is_empty(),
            download_count: share.download_count,
            max_downloads: share.max_downloads,
            is_active: share.is_active == 1,
            is_expired: share
                .expires_at
                .as_deref()
                .map(crate::utils::time::is_expired_utc)
                .unwrap_or(false),
            share_url: format!("/share/{}", share.id),
            custom_code: share.custom_code.clone(),
            items: infos,
            item_count,
            file_count,
            folder_count,
            total_file_count: st.total_file_count,
            total_folder_count: st.total_folder_count,
            total_size: st.total_size,
            formatted_size: crate::models::file::format_file_size(st.total_size),
        });
    }

    Ok(out)
}

/// 单个条目的对外表示。`unlocked` 决定是否下发媒体地址。
fn share_item_info(
    share_id: &str,
    kind: ItemKind,
    id: i64,
    meta: &ItemMeta,
    unlocked: bool,
) -> ShareItemInfo {
    if kind == ItemKind::Folder {
        return ShareItemInfo {
            item_type: kind.as_str().to_string(),
            id,
            name: meta.name.clone(),
            size: 0,
            formatted_size: crate::models::file::format_file_size(0),
            file_type: "folder".to_string(),
            uploaded_at: None,
            has_preview: false,
            preview_url: None,
            thumb_url: None,
            download_url: None,
            file_count: Some(meta.file_count),
            subfolder_count: Some(meta.subfolder_count),
        };
    }

    // 与 `files::inline_response_policy` 同一套白名单判断：白名单外的类型
    // 不给预览地址（给了也只会被降级成下载，白白多一次请求）。
    let ext = crate::services::file_service::extension_of(&meta.name);
    let inline_safe = crate::services::file_service::is_inline_safe(&ext);

    // 缩略图缺失时回退到预览图，与 `FileInfo::to_info` 一致。
    let thumb_path = meta.thumb_path.as_ref().or(meta.preview_path.as_ref());
    let (preview_url, thumb_url) = if !unlocked || (!meta.preview_path.is_some() && !inline_safe) {
        (None, None)
    } else {
        let preview = Some(format!(
            "/api/public/shares/{}/media?file_id={}&preview=1",
            share_id, id
        ));
        let thumb = if thumb_path.is_some() {
            Some(format!(
                "/api/public/shares/{}/media?file_id={}&thumb=1",
                share_id, id
            ))
        } else if inline_safe {
            preview.clone()
        } else {
            None
        };
        (preview, thumb)
    };

    ShareItemInfo {
        item_type: kind.as_str().to_string(),
        id,
        name: meta.name.clone(),
        size: meta.size,
        formatted_size: crate::models::file::format_file_size(meta.size),
        file_type: meta.file_type.clone(),
        uploaded_at: meta.uploaded_at.clone(),
        has_preview: meta.preview_path.is_some(),
        preview_url,
        thumb_url,
        // 下载地址与是否解锁无关：它本身不是凭据（要带 ticket 才放行），
        // 而不解锁时前端根本不会走到下载按钮。
        download_url: Some(format!(
            "/api/public/shares/{}/download?file_id={}",
            share_id, id
        )),
        file_count: None,
        subfolder_count: None,
    }
}
