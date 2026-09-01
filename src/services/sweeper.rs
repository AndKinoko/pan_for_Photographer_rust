use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use sqlx::SqlitePool;
use tokio::sync::Semaphore;

use crate::config::Config;
use crate::services::preview_service;

/// 孤儿文件（uuid 无对应 DB 行）的老化宽限：仅删除 mtime 超过该时长的文件。
/// 保护上传在途（rename 后尚未 INSERT）与缩略图生成中（已落盘尚未 UPDATE）的窗口。
const ORPHAN_GRACE: Duration = Duration::from_secs(5 * 60); // 5 分钟
/// .part 临时文件的老化宽限：单独放大到 1 小时，避免慢 WiFi 大文件上传被误删。
const PART_GRACE: Duration = Duration::from_secs(60 * 60); // 1 小时

/// 周期启动 GC。进程存活期间常驻后台。
pub fn start(pool: SqlitePool, config: Config, sem: Arc<Semaphore>) {
    tokio::spawn(async move {
        let interval = if config.gc_interval_sec > 0 {
            config.gc_interval_sec
        } else {
            600
        };
        tracing::info!("孤儿文件清理器已启动，间隔 {} 秒", interval);
        let mut ticker = tokio::time::interval(Duration::from_secs(interval));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            if let Err(e) = run_once(&pool, &config, sem.clone()).await {
                tracing::warn!("GC 执行失败: {:?}", e);
            }
        }
    });
}

/// 空回收站后手动触发的即时 GC（不等待周期）。
pub async fn run_once(pool: &SqlitePool, config: &Config, sem: Arc<Semaphore>) -> Result<(), sqlx::Error> {
    let t = std::time::Instant::now();

    // 第 1 步：orphan 对账 + .part 清理（阻塞磁盘扫描放 blocking 线程池）
    let upload_dir = config.upload_dir.clone();
    let live_stored: std::collections::HashSet<String> = sqlx::query_scalar(
        "SELECT stored_path FROM files",
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .collect();
    let live_previews: std::collections::HashSet<String> = sqlx::query_scalar(
        "SELECT preview_path FROM files WHERE preview_path IS NOT NULL",
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .collect();
    let live_thumbs: std::collections::HashSet<String> = sqlx::query_scalar(
        "SELECT thumb_path FROM files WHERE thumb_path IS NOT NULL",
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .collect();

    let disk_stats = tokio::task::spawn_blocking(move || {
        let mut deleted_files = 0usize;
        let mut deleted_previews = 0usize;
        let mut deleted_parts = 0usize;

        // 走查 user_* 目录树，收集所有文件相对路径
        let mut disk_stored: Vec<PathBuf> = Vec::new();
        let mut disk_previews: Vec<PathBuf> = Vec::new();
        walk_upload_tree(&upload_dir, &mut disk_stored, &mut disk_previews);

        let is_stale = |p: &Path| file_age_secs(p).unwrap_or(0) >= ORPHAN_GRACE.as_secs();
        let is_part_stale = |p: &Path| file_age_secs(p).unwrap_or(0) >= PART_GRACE.as_secs();

        for rel in disk_stored {
            // 剥掉 upload_dir 前缀得到与 DB stored_path 同构的相对 key
            let Ok(rel_key) = rel.strip_prefix(&upload_dir) else {
                continue;
            };
            let key = rel_key.to_string_lossy().replace('\\', "/");
            if !live_stored.contains(&key) && is_stale(&rel) {
                if std::fs::remove_file(&rel).is_ok() {
                    deleted_files += 1;
                }
            }
        }
        for rel in disk_previews {
            let Ok(rel_key) = rel.strip_prefix(&upload_dir) else {
                continue;
            };
            let key = rel_key.to_string_lossy().replace('\\', "/");
            if !live_previews.contains(&key) && !live_thumbs.contains(&key) && is_stale(&rel) {
                if std::fs::remove_file(&rel).is_ok() {
                    deleted_previews += 1;
                }
            }
        }
        // 清理超龄 .part（覆盖孤儿对账不做它的场景）
        let tmp_root = upload_dir.join(".tmp_incoming");
        if let Ok(rd) = std::fs::read_dir(&tmp_root) {
            for e in rd.flatten() {
                let p = e.path();
                if p.extension().and_then(|s| s.to_str()) == Some("part")
                    && is_part_stale(&p)
                    && std::fs::remove_file(&p).is_ok()
                {
                    deleted_parts += 1;
                }
            }
        }
        (deleted_files, deleted_previews, deleted_parts)
    })
    .await
    .unwrap_or((0, 0, 0));

    // 第 2 步：崩溃恢复 + 补生成 —— preview_path IS NULL 的行重新入队
    // 仅对支持预览的类型重投，避免非图片文件每轮重复空转
    let pending: Vec<(i64, String, String, i64)> = sqlx::query_as(
        "SELECT id, stored_path, file_type, owner_id FROM files WHERE preview_path IS NULL",
    )
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
            let _ = sqlx::query(
                "UPDATE files SET preview_path = ?, thumb_path = ? WHERE id = ?",
            )
            .bind(pv)
            .bind(th)
            .bind(file_id)
            .execute(&pool)
            .await;
        });
    }

    tracing::info!(
        "GC 完成：删除孤立文件 {} 个、孤立预览 {} 个、超龄 .part {} 个，耗时 {:?}",
        disk_stats.0,
        disk_stats.1,
        disk_stats.2,
        t.elapsed()
    );
    Ok(())
}

/// 递归收集 uploads/ 下的源文件（user_*/xxx）与预览文件（user_*/previews/xxx）。
/// 源文件与预览文件按相对路径返回。.tmp_incoming 目录整体跳过（.part 单独处理）。
fn walk_upload_tree(root: &Path, stored: &mut Vec<PathBuf>, previews: &mut Vec<PathBuf>) {
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
                collect_files(&p, previews);
            } else {
                walk_upload_tree(&p, stored, previews);
            }
        } else {
            stored.push(p);
        }
    }
}

fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) {
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            if e.path().is_file() {
                out.push(e.path());
            }
        }
    }
}

/// 文件自 mtime 距今的秒数；失败返回 None
fn file_age_secs(p: &Path) -> Option<u64> {
    let meta = std::fs::metadata(p).ok()?;
    let modified = meta.modified().ok()?;
    let age = SystemTime::now().duration_since(modified).ok()?;
    Some(age.as_secs())
}
