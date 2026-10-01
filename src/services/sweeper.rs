use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sqlx::SqlitePool;
use tokio::sync::Semaphore;

use crate::config::Config;
use crate::services::preview_service;

/// 孤儿文件（uuid 无对应 DB 行）的老化宽限：**仅删除 mtime 超过该时长的文件**。
///
/// 这个宽限期是**唯一**在保护上传提交窗口的东西，所以取值必须大于
/// 「最慢一次上传的耗时」，否则会把刚传完的文件当成孤儿删掉：
///
/// 上传流程是 `File::create(.part)` → 流式写入 → `rename` 到最终路径 → `INSERT`。
/// `rename` 保留原 mtime，即 **mtime 等于上传开始时刻**。于是只要上传耗时超过
/// 宽限期，`rename` 完成的那一刻文件就已经「超龄」；若 GC 恰好在此刻扫过磁盘、
/// 又在 INSERT 之前查库，这个文件就会被判定为孤儿删除 —— 随后 INSERT 成功，
/// 留下一条指向不存在文件的记录（下载 404、预览永久失败）。
///
/// 原先取 5 分钟，只覆盖了「5 分钟内传完」的情形，而家用宽带上一个 10GB 原片
/// 传 20 分钟以上很常见。取 1 小时可以覆盖 10GB @ 约 22 Mbit/s 及以上的上行；
/// 再慢的上传仍有窗口，代价则是孤儿多留一小时 —— 而孤儿清理本来 24 小时才跑一次，
/// 这点延迟毫无影响。
const ORPHAN_GRACE: Duration = Duration::from_secs(60 * 60); // 1 小时
/// .part 临时文件的老化宽限：单独放大到 1 小时，避免慢 WiFi 大文件上传被误删。
const PART_GRACE: Duration = Duration::from_secs(60 * 60); // 1 小时

/// 两次**主动**孤儿清理之间的最小间隔。
///
/// 主动清理（管理端按钮 / 清空回收站后的即时清理）每次都要遍历整个 uploads/
/// 目录树，是纯 IO 开销。没有这道闸时，任何已发出的账号循环调
/// `DELETE /api/trash` 就能让服务端持续全盘扫描。60 秒足够让「刚删完想立刻
/// 回收空间」的体验不受影响，同时把放大倍数压到 1/60 以下。
const MIN_CLEANUP_INTERVAL: Duration = Duration::from_secs(60);

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
///
/// **调用方几乎总是该用 [`cleanup_orphans_throttled`]。** 本函数自身不做任何
/// 并发或频率保护，直接暴露出去等于把「全盘扫一遍」变成一个可被任意账号
/// 无限触发的操作。
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

/// 主动孤儿清理的统一入口：**全局互斥 + 最小间隔**。
///
/// 管理端的清理按钮与用户侧「清空回收站后的即时清理」都走这里，
/// 两者此前各自为政：管理端有 `AtomicBool` 互斥（还写了注释说明理由），
/// 用户侧却裸调 `cleanup_orphans`，于是任意已发出的账号都能循环触发全盘扫描。
///
/// 返回 `None` 表示本次跳过，原因有二，调用方不必区分：
/// · 已有一次清理在跑（互斥）；
/// · 距上次清理不足 [`MIN_CLEANUP_INTERVAL`]。
pub async fn cleanup_orphans_throttled(
    pool: &SqlitePool,
    config: &Config,
) -> Option<Result<CleanupStats, sqlx::Error>> {
    // 互斥先抢，抢不到立刻返回——不能等，等就等于把请求排成队继续打满 IO。
    if CLEANUP_RUNNING.swap(true, Ordering::SeqCst) {
        return None;
    }
    // 无论怎么退出（含 panic 展开）都要把标志放回去，否则清理入口会被永久锁死。
    let _running = RunningGuard;

    {
        let last = LAST_CLEANUP.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(t) = *last {
            if t.elapsed() < MIN_CLEANUP_INTERVAL {
                return None;
            }
        }
    }

    let result = cleanup_orphans(pool, config).await;

    // 用「开始时刻」还是「结束时刻」记账差别不大，这里记结束时刻：
    // 一次扫描跑很久时，下一次至少还能在它结束后 60 秒开始。
    *LAST_CLEANUP.lock().unwrap_or_else(|e| e.into_inner()) = Some(std::time::Instant::now());
    Some(result)
}

static CLEANUP_RUNNING: AtomicBool = AtomicBool::new(false);
static LAST_CLEANUP: Mutex<Option<std::time::Instant>> = Mutex::new(None);

/// 作用域结束时复位互斥标志。
struct RunningGuard;

impl Drop for RunningGuard {
    fn drop(&mut self) {
        CLEANUP_RUNNING.store(false, Ordering::SeqCst);
    }
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
    //
    // 另外两条过滤都不能省：
    // · `deleted_at IS NULL`——回收站里的文件不该再花 CPU 生成预览，用户已经
    //   不要它了；它们还留在库里的唯一原因是「可恢复」。
    // · 支持的类型白名单——视频/压缩包这类永远不支持预览的类型（`preview_path`
    //   恒为 NULL）此前每 10 分钟就被整表捞出来一次，在 `supports_preview`
    //   里再被逐条跳过。判定下沉到 SQL，几万条视频时省下的是每次全表扫描。
    //   白名单随 `file_service` 走，两边不会各自漂移。
    let previewable = serde_json::to_string(&crate::services::file_service::previewable_types())
        .unwrap_or_else(|_| "[]".into());
    let pending: Vec<(i64, String, String, i64)> = sqlx::query_as(
        "SELECT id, stored_path, file_type, owner_id FROM files \
         WHERE preview_path IS NULL AND preview_attempts < ? AND deleted_at IS NULL \
           AND lower(file_type) IN (SELECT value FROM json_each(?))",
    )
    .bind(MAX_PREVIEW_ATTEMPTS)
    .bind(&previewable)
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


