use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use sqlx::SqlitePool;
use tokio::sync::Semaphore;

use crate::config::Config;
use crate::services::preview_service;

/// 孤儿文件（uuid 无对应 DB 行）的老化宽限：仅删除 mtime 超过该时长的文件。
/// 保护上传在途（rename 后尚未 INSERT）与缩略图生成中（已落盘尚未 UPDATE）的窗口。
const ORPHAN_GRACE: Duration = Duration::from_secs(5 * 60); // 5 分钟
/// .part 临时文件的老化宽限：单独放大到 1 小时，避免慢 WiFi 大文件上传被误删。
const PART_GRACE: Duration = Duration::from_secs(60 * 60); // 1 小时

/// 预览生成的最大尝试次数：达到后 GC 不再重投该文件。
///
/// 取 3 是折中——格式不支持 / 文件损坏这类确定性失败连续 3 次即可判定无望，
/// 而磁盘抖动这类临时失败仍有机会在后续轮次恢复。
/// 没有这个上限时，生成失败的文件（preview_path 保持 NULL）会被每一轮 GC
/// 反复解码，配合 RAW 的全量读盘会演变成周期性 OOM。
const MAX_PREVIEW_ATTEMPTS: i64 = 3;

/// 孤儿清理的自动执行间隔：24 小时。
pub const ORPHAN_CLEANUP_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// 启动两个后台任务。
///
/// **为什么拆成两个**：两种职责的合理频率差了一个数量级。
/// - [`retry_missing_previews`]：缩略图生成失败要 reasonably soon 重试，
///   保持 10 分钟。它只查库不扫磁盘，开销极低。
/// - [`cleanup_orphans`]：孤儿只在异常后产生，稀疏跑没影响，
///   改成「管理端按钮手动触发 + 每 24 小时自动」。
///
/// 绑在一起的话，孤儿清理要么太频繁（浪费磁盘 IO），要么缩略图重投太慢
/// （用户盯着空图标等）。所以拆开。
pub fn start(pool: SqlitePool, config: Config, sem: Arc<Semaphore>) {
    // 任务 2 也要用池与配置，而任务 1 会把它们移走，所以先各留一份
    let pool2 = pool.clone();
    let config2 = config.clone();

    // 任务 1：缩略图重投，周期不变
    tokio::spawn(async move {
        let interval = if config.gc_interval_sec > 0 {
            config.gc_interval_sec
        } else {
            600
        };
        tracing::info!("缩略图重投任务已启动，间隔 {} 秒", interval);
        let mut ticker = tokio::time::interval(Duration::from_secs(interval));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            if let Err(e) = retry_missing_previews(&pool, &config, sem.clone()).await {
                tracing::warn!("缩略图重投失败: {:?}", e);
            }
        }
    });

    // 任务 2：孤儿清理，24 小时一次
    tokio::spawn(async move {
        tracing::info!(
            "孤儿清理任务已启动，每 {} 小时自动执行一次（也可在管理端手动触发）",
            ORPHAN_CLEANUP_INTERVAL.as_secs() / 3600
        );
        // 首次等一个完整周期再跑：服务刚启动时磁盘状态本来就是一致的，
        // 立刻扫一遍没有意义。真正的清理由管理端按钮负责。
        loop {
            tokio::time::sleep(ORPHAN_CLEANUP_INTERVAL).await;
            if let Err(e) = cleanup_orphans(&pool2, &config2).await {
                tracing::warn!("孤儿清理失败: {:?}", e);
            }
        }
    });
}

/// 一次孤儿清理的结果，供管理端展示。
#[derive(Debug, Clone, Copy, Default, serde::Serialize)]
pub struct CleanupStats {
    /// 删除的孤立源文件数
    pub orphan_files: usize,
    /// 删除的孤立预览/缩略图数
    pub orphan_previews: usize,
    /// 删除的超龄 .part 临时文件数
    pub stale_parts: usize,
    /// 耗时（毫秒）
    pub elapsed_ms: u128,
}

/// 孤儿文件对账 + 超龄 .part 清理。**低频**：手动按钮 + 每 24 小时一次。
///
/// 孤儿只在异常路径后产生（进程被杀、用户被删、rename 后未 INSERT），
/// 不是正常产物，所以稀疏清理足够——孤儿在磁盘上多放 24 小时没有任何危害。
///
/// 与 [`retry_missing_previews`] 分开的原因见 [`start`] 的注释。
pub async fn cleanup_orphans(pool: &SqlitePool, config: &Config) -> Result<CleanupStats, sqlx::Error> {
    let t = std::time::Instant::now();

    // 阻塞磁盘扫描放 blocking 线程池
    let upload_dir = config.upload_dir.clone();

    // **先扫磁盘、再查库**，而不是反过来。
    //
    // 原实现是「SELECT stored_path/preview_path/thumb_path FROM files」
    // 无 WHERE 无分页，把整张表拉进三个 HashSet。10 万文件时每轮 GC 要
    // 在内存里放三份全量路径（约数 MB × 3），而且跑的频率是每 600 秒。
    //
    // 对账的判据本质是「磁盘上有哪些文件，库里有没有对应记录」，
    // 所以更省的一侧永远是磁盘——那部分数据我们本来就要读。
    // 于是改成：扫出磁盘候选集后，用 `json_each(?)` 把它交给数据库做
    // 反连接，只把**孤儿**（磁盘有、库里无）带回内存。
    // 内存占用从 O(库中文件数) 降到 O(孤儿数)，后者通常是个位数。
    // 先在 blocking 线程里扫磁盘，只收集「可能超龄」的候选（含 mtime）。
    // 这里不做孤儿判定——那需要查库，不能在 blocking 闭包里做异步 IO。
    // 闭包内要用 upload_dir 走查，之后删除孤儿时还要用，所以克隆一份进去。
    let scan_root = upload_dir.clone();
    let (disk_stored, disk_previews, deleted_parts) =
        tokio::task::spawn_blocking(move || {
            let mut stored: Vec<(String, std::time::SystemTime)> = Vec::new();
            let mut previews: Vec<(String, std::time::SystemTime)> = Vec::new();
            walk_upload_tree(&scan_root, &scan_root, &mut stored, &mut previews);

            // 只留超龄的：未超龄的本轮不可能被删，早点丢弃能显著缩小
            // 后面传给数据库的候选集。
            let cutoff = std::time::SystemTime::now()
                .checked_sub(ORPHAN_GRACE)
                .unwrap_or(std::time::UNIX_EPOCH);
            stored.retain(|(_, mtime)| *mtime <= cutoff);
            previews.retain(|(_, mtime)| *mtime <= cutoff);

            // 超龄 .part：直接删，不必查库（它们从来不入库）
            let mut parts = 0usize;
            let part_cutoff = std::time::SystemTime::now()
                .checked_sub(PART_GRACE)
                .unwrap_or(std::time::UNIX_EPOCH);
            let tmp_root = scan_root.join(".tmp_incoming");
            if let Ok(rd) = std::fs::read_dir(&tmp_root) {
                for e in rd.flatten() {
                    let p = e.path();
                    let stale = p
                        .metadata()
                        .and_then(|m| m.modified())
                        .map(|m| m <= part_cutoff)
                        .unwrap_or(false);
                    if p.extension().and_then(|s| s.to_str()) == Some("part")
                        && stale
                        && std::fs::remove_file(&p).is_ok()
                    {
                        parts += 1;
                    }
                }
            }
            (stored, previews, parts)
        })
        .await
        .unwrap_or_default();

    // 把磁盘候选集交给数据库做反连接：只取「磁盘有、库里无」的孤儿。
    // 相比原来把整张 files 表拉进三个 HashSet，内存占用从 O(库中文件数)
    // 降到 O(孤儿数)——后者通常是 0。
    let orphan_stored: Vec<String> = if disk_stored.is_empty() {
        Vec::new()
    } else {
        let keys: Vec<String> = disk_stored.iter().map(|(k, _)| k.clone()).collect();
        sqlx::query_scalar(
            r#"
            SELECT d.value FROM json_each(?) AS d
            WHERE NOT EXISTS (
                SELECT 1 FROM files f
                WHERE f.stored_path = d.value
                   OR f.preview_path = d.value
                   OR f.thumb_path = d.value
            )
            "#,
        )
        .bind(serde_json::to_string(&keys).unwrap_or_else(|_| "[]".into()))
        .fetch_all(pool)
        .await?
    };

    let orphan_previews: Vec<String> = if disk_previews.is_empty() {
        Vec::new()
    } else {
        let keys: Vec<String> = disk_previews.iter().map(|(k, _)| k.clone()).collect();
        sqlx::query_scalar(
            r#"
            SELECT d.value FROM json_each(?) AS d
            WHERE NOT EXISTS (
                SELECT 1 FROM files f
                WHERE f.preview_path = d.value OR f.thumb_path = d.value
            )
            "#,
        )
        .bind(serde_json::to_string(&keys).unwrap_or_else(|_| "[]".into()))
        .fetch_all(pool)
        .await?
    };

    // 删除孤儿：此时已确认无主，删了不会误伤。
    let mut deleted_files = 0usize;
    let mut deleted_previews = 0usize;
    for key in &orphan_stored {
        if std::fs::remove_file(upload_dir.join(key)).is_ok() {
            deleted_files += 1;
        }
    }
    for key in &orphan_previews {
        if std::fs::remove_file(upload_dir.join(key)).is_ok() {
            deleted_previews += 1;
        }
    }
    tracing::info!(
        "孤儿清理完成：孤立文件 {} 个、孤立预览 {} 个、超龄 .part {} 个，耗时 {:?}",
        deleted_files,
        deleted_previews,
        deleted_parts,
        t.elapsed()
    );

    Ok(CleanupStats {
        orphan_files: deleted_files,
        orphan_previews: deleted_previews,
        stale_parts: deleted_parts,
        elapsed_ms: t.elapsed().as_millis(),
    })
}

/// 补生成缺失的缩略图。**高频**：每 10 分钟一次，只查库不扫磁盘。
///
/// 与 [`cleanup_orphans`] 分开的理由：缩略图生成失败（磁盘抖动、内存不足）
/// 需要 reasonably soon 重试，拖到 24 小时会让用户盯着空图标等一整天。
/// 而孤儿清理是纯磁盘操作，稀疏跑没影响。两者绑在一起必然有一方不舒服。
pub async fn retry_missing_previews(
    pool: &SqlitePool,
    config: &Config,
    sem: Arc<Semaphore>,
) -> Result<usize, sqlx::Error> {
    let mut enqueued = 0usize;
    // 崩溃恢复 + 补生成 —— preview_path IS NULL 的行重新入队。
    // 只对「支持预览」且「未超过尝试上限」的行重投：
    // 生成失败时 preview_path 会保持 NULL，若不设上限，同一个失败文件会被每轮 GC
    // 反复解码（历史上配合 RAW 的全量读盘会变成周期性 OOM）。
    let pending: Vec<(i64, String, String, i64)> = sqlx::query_as(
        "SELECT id, stored_path, file_type, owner_id FROM files \
         WHERE preview_path IS NULL AND preview_attempts < ?",
    )
    .bind(MAX_PREVIEW_ATTEMPTS)
    .fetch_all(pool)
    .await?;
    if !pending.is_empty() {
        tracing::info!("GC 检测到 {} 个待生成缩略图的文件", pending.len());
    }
    for (file_id, stored, ft, owner) in pending {
        if !crate::services::file_service::supports_preview(&ft) {
            continue;
        }
        // 逐个 spawn（与上传共用 preview_semaphore 限并发）
        let Ok(permit) = sem.clone().acquire_owned().await else {
            break;
        };
        enqueued += 1;

        // 先把尝试次数记上再做活：即便进程在生成途中被杀，
        // 这次尝试也已计入，不会形成「崩溃 → 重启 → 立刻再试」的死循环。
        let _ = sqlx::query("UPDATE files SET preview_attempts = preview_attempts + 1 WHERE id = ?")
            .bind(file_id)
            .execute(pool)
            .await;

        let pool = pool.clone();
        let config = config.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let src = config.upload_dir.join(&stored);
            let res = tokio::task::spawn_blocking(move || {
                preview_service::generate_preview_and_thumb(&config, owner, &src, &ft)
            })
            .await;
            let Ok((pv, th)) = res else {
                return;
            };

            if pv.is_none() {
                // preview_path 仍为 NULL，下一轮仍会被选中。这里**不**清零尝试次数，
                // 让它继续累积到上限为止——否则「pv 永远失败、th 永远成功」的组合
                // 会因为每轮都清零而变成无限重试。
                if th.is_some() {
                    let _ = sqlx::query("UPDATE files SET thumb_path = ? WHERE id = ?")
                        .bind(th)
                        .bind(file_id)
                        .execute(&pool)
                        .await;
                }
                return;
            }

            // 成功（preview 已生成，行会离开待处理集合）→ 计数清零
            let _ = sqlx::query(
                "UPDATE files SET preview_path = ?, thumb_path = ?, preview_attempts = 0 WHERE id = ?",
            )
            .bind(pv)
            .bind(th)
            .bind(file_id)
            .execute(&pool)
            .await;
        });
    }

    tracing::info!("缩略图重投：本次入队 {} 个", enqueued);
    Ok(enqueued)
}

/// 递归收集 uploads/ 下的源文件（user_*/xxx）与预览文件（user_*/previews/xxx）。
/// 源文件与预览文件按相对路径返回。.tmp_incoming 目录整体跳过（.part 单独处理）。
/// 递归走查 uploads 树，收集「相对路径 + mtime」。
///
/// 直接产出相对路径（而非完整 `PathBuf`）是因为调用方要把它交给
/// `json_each` 传给数据库做反连接——那是与 `files.stored_path`
/// 同构的相对 key。多存一个 `upload_dir` 前缀只是浪费。
///
/// mtime 一并带出，让调用方能在**不额外 stat 一次**的前提下筛掉未超龄的。
fn walk_upload_tree(
    root: &Path,
    base: &Path,
    stored: &mut Vec<(String, std::time::SystemTime)>,
    previews: &mut Vec<(String, std::time::SystemTime)>,
) {
    let Ok(rd) = std::fs::read_dir(root) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name == ".tmp_incoming" {
            continue; // 临时目录不参与孤儿对账（.part 有独立宽限）
        }
        if p.is_dir() {
            // 预览子目录单独归拢，避免与源文件混淆
            if name == "previews" {
                collect_files(&p, base, previews);
            } else {
                walk_upload_tree(&p, base, stored, previews);
            }
        } else if let (Some(key), Ok(md)) = (rel_key(&p, base), std::fs::metadata(&p)) {
            if let Ok(mtime) = md.modified() {
                stored.push((key, mtime));
            }
        }
    }
}

fn collect_files(
    dir: &Path,
    base: &Path,
    out: &mut Vec<(String, std::time::SystemTime)>,
) {
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_file() {
                if let (Some(key), Ok(md)) = (rel_key(&p, base), std::fs::metadata(&p)) {
                    if let Ok(mtime) = md.modified() {
                        out.push((key, mtime));
                    }
                }
            }
        }
    }
}

/// 把绝对路径转成与 `files.stored_path` 同构的相对 key（正斜杠分隔）。
/// 剥不掉前缀（不在 base 之下）时返回 None。
fn rel_key(p: &Path, base: &Path) -> Option<String> {
    p.strip_prefix(base)
        .ok()
        .map(|r| r.to_string_lossy().replace('\\', "/"))
}


