use std::path::{Path, PathBuf};
use std::time::Duration;
use uuid::Uuid;

use crate::config::Config;
use crate::errors::AppError;
use crate::models::file::{File, FileInfo};
use crate::models::folder::Folder;
use sqlx::SqlitePool;

/// Maximum retry attempts for file system operations
const MAX_RETRY_ATTEMPTS: u32 = 3;
/// Base delay between retries in milliseconds
const RETRY_BASE_DELAY_MS: u64 = 100;

/// Blocked file extensions for security
const BLOCKED_EXTENSIONS: &[&str] = &[".html", ".htm", ".svg", ".js", ".mjs"];

/// Image formats that support preview
const IMAGE_FORMATS: &[&str] = &["jpg", "jpeg", "png", "gif", "bmp", "webp", "tiff", "tif"];

/// RAW formats that support preview via raw processing
const RAW_FORMATS: &[&str] = &[
    "nef", "cr2", "cr3", "crw", "arw", "sr2", "srf", "dng", "raf", "orf", "rw2", "nrw",
];

/// List files in a folder (or root if folder_id is None)
pub async fn list_files(
    pool: &SqlitePool,
    owner_id: i64,
    folder_id: Option<i64>,
) -> Result<Vec<FileInfo>, AppError> {
    let files = if let Some(fid) = folder_id {
        // Verify folder belongs to user
        let folder = sqlx::query_as::<_, Folder>(
            "SELECT * FROM folders WHERE id = ? AND owner_id = ?",
        )
        .bind(fid)
        .bind(owner_id)
        .fetch_optional(pool)
        .await?;

        if folder.is_none() {
            return Err(AppError::NotFound("文件夹不存在".into()));
        }

        sqlx::query_as::<_, File>(
            "SELECT * FROM files WHERE owner_id = ? AND folder_id = ? ORDER BY uploaded_at DESC",
        )
        .bind(owner_id)
        .bind(fid)
        .fetch_all(pool)
        .await?
    } else {
        sqlx::query_as::<_, File>(
            "SELECT * FROM files WHERE owner_id = ? AND folder_id IS NULL ORDER BY uploaded_at DESC",
        )
        .bind(owner_id)
        .fetch_all(pool)
        .await?
    };

    Ok(files.into_iter().map(|f| f.to_info()).collect())
}

/// Get a single file by ID, verifying ownership
pub async fn get_file(
    pool: &SqlitePool,
    file_id: i64,
    owner_id: i64,
) -> Result<File, AppError> {
    sqlx::query_as::<_, File>("SELECT * FROM files WHERE id = ? AND owner_id = ?")
        .bind(file_id)
        .bind(owner_id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| AppError::NotFound("文件不存在".into()))
}

/// Get a file by ID without ownership check (for share access)
pub async fn get_file_by_id(pool: &SqlitePool, file_id: i64) -> Result<File, AppError> {
    sqlx::query_as::<_, File>("SELECT * FROM files WHERE id = ?")
        .bind(file_id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| AppError::NotFound("文件不存在".into()))
}

/// Validate file extension is not blocked
pub fn validate_extension(filename: &str) -> Result<(), AppError> {
    let ext = Path::new(filename)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    let ext_with_dot = format!(".{}", ext);
    if BLOCKED_EXTENSIONS.contains(&ext_with_dot.as_str()) {
        return Err(AppError::BadRequest(format!(
            "文件类型 \"{}\" 不允许上传（存在安全风险）",
            ext_with_dot
        )));
    }
    Ok(())
}

/// Check for duplicate filenames in the same folder (case-insensitive)
pub async fn check_duplicates(
    pool: &SqlitePool,
    owner_id: i64,
    folder_id: Option<i64>,
    filename: &str,
) -> Result<bool, AppError> {
    let existing = if let Some(fid) = folder_id {
        sqlx::query_scalar::<_, String>(
            "SELECT original_name FROM files WHERE owner_id = ? AND folder_id = ? AND LOWER(original_name) = LOWER(?)",
        )
        .bind(owner_id)
        .bind(fid)
        .bind(filename)
        .fetch_optional(pool)
        .await?
    } else {
        sqlx::query_scalar::<_, String>(
            "SELECT original_name FROM files WHERE owner_id = ? AND folder_id IS NULL AND LOWER(original_name) = LOWER(?)",
        )
        .bind(owner_id)
        .bind(filename)
        .fetch_optional(pool)
        .await?
    };

    Ok(existing.is_some())
}

/// Generate a unique stored filename: {uuid}.{ext}
pub fn generate_stored_filename(original_name: &str) -> String {
    let ext = Path::new(original_name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("bin");
    format!("{}.{}", Uuid::new_v4().simple(), ext)
}

/// Get the user's upload directory path
pub fn user_upload_dir(config: &Config, user_id: i64) -> PathBuf {
    config.upload_dir.join(format!("user_{}", user_id))
}

/// Get the user's preview directory path
pub fn user_preview_dir(config: &Config, user_id: i64) -> PathBuf {
    config.upload_dir.join(format!("user_{}", user_id)).join("previews")
}

/// Check if file type supports preview
pub fn supports_preview(file_type: &str) -> bool {
    let ft = file_type.to_lowercase();
    IMAGE_FORMATS.contains(&ft.as_str()) || RAW_FORMATS.contains(&ft.as_str())
}

/// Delete a physical file with retry logic.
/// Returns Ok(()) if the file was deleted or didn't exist.
/// Returns Err if all retry attempts failed.
pub(crate) async fn delete_physical_file_with_retry(file_path: &Path, description: &str) -> Result<(), AppError> {
    if !tokio::fs::try_exists(file_path).await.unwrap_or(false) {
        tracing::debug!("{} does not exist, skipping deletion: {}", description, file_path.display());
        return Ok(());
    }

    let mut last_error = None;
    for attempt in 0..MAX_RETRY_ATTEMPTS {
        match tokio::fs::remove_file(file_path).await {
            Ok(()) => {
                tracing::info!(
                    "{} deleted successfully (attempt {}): {}",
                    description,
                    attempt + 1,
                    file_path.display()
                );
                return Ok(());
            }
            Err(e) => {
                last_error = Some(e);
                if attempt < MAX_RETRY_ATTEMPTS - 1 {
                    let delay = Duration::from_millis(RETRY_BASE_DELAY_MS * (1 << attempt));
                    tracing::warn!(
                        "{} deletion failed (attempt {}), retrying in {:?}: {} - error: {}",
                        description,
                        attempt + 1,
                        delay,
                        file_path.display(),
                        last_error.as_ref().unwrap()
                    );
                    tokio::time::sleep(delay).await;
                }
            }
        }
    }

    let err = last_error.unwrap();
    tracing::error!(
        "{} deletion failed after {} attempts: {} - error: {}",
        description,
        MAX_RETRY_ATTEMPTS,
        file_path.display(),
        err
    );
    Err(AppError::Internal(format!(
        "删除{}失败: {}",
        description, err
    )))
}

/// Clean up a preview file and its parent directory if empty.
/// This is a best-effort operation; failures are logged but not propagated.
pub(crate) async fn cleanup_preview_file(config: &Config, preview_path: &str) {
    let preview_full = config.upload_dir.join(preview_path);

    match delete_physical_file_with_retry(&preview_full, "预览文件").await {
        Ok(()) => {
            // Try to clean up empty parent directory
            if let Some(parent) = preview_full.parent() {
                match tokio::fs::read_dir(parent).await {
                    Ok(mut entries) => {
                        if entries.next_entry().await.ok().flatten().is_none() {
                            // Directory is empty, remove it
                            match tokio::fs::remove_dir(parent).await {
                                Ok(()) => {
                                    tracing::info!(
                                        "Removed empty preview directory: {}",
                                        parent.display()
                                    );
                                }
                                Err(e) => {
                                    tracing::debug!(
                                        "Could not remove preview directory (may not be empty): {} - {}",
                                        parent.display(),
                                        e
                                    );
                                }
                            }
                        }
                    }
                    Err(e) => {
                        tracing::debug!(
                            "Could not read preview directory: {} - {}",
                            parent.display(),
                            e
                        );
                    }
                }
            }
        }
        Err(e) => {
            tracing::warn!(
                "Preview file cleanup failed (non-fatal): {} - {}",
                preview_full.display(),
                e.message()
            );
        }
    }
}

/// Clean up a stored (source) file with retry.
/// This is a best-effort operation; failures are logged but not propagated.
async fn cleanup_stored_file(config: &Config, stored_path: &str) {
    let stored_full = config.upload_dir.join(stored_path);

    match delete_physical_file_with_retry(&stored_full, "源文件").await {
        Ok(()) => {}
        Err(e) => {
            tracing::warn!(
                "Source file cleanup failed (non-fatal): {} - {}",
                stored_full.display(),
                e.message()
            );
        }
    }
}

/// Delete a file record and its associated physical files (source + preview).
///
/// Deletion strategy:
/// 1. Delete the database record in a transaction (ensures atomicity at the DB level)
/// 2. Delete physical files (source + preview) with retry — best-effort, non-fatal on failure
///
/// This ensures the user sees the file as deleted immediately, while physical cleanup
/// happens asynchronously with retry. Failed physical deletions are logged for monitoring.
pub async fn delete_file(
    pool: &SqlitePool,
    config: &Config,
    file_id: i64,
    owner_id: i64,
) -> Result<(), AppError> {
    // Fetch the file record first to get stored_path and preview_path
    let file = get_file(pool, file_id, owner_id).await?;

    let stored_path = file.stored_path.clone();
    let preview_path = file.preview_path.clone();
    let thumb_path = file.thumb_path.clone();

    tracing::info!(
        "Starting deletion of file id={}, stored_path={}, has_preview={}, has_thumb={}",
        file_id,
        stored_path,
        preview_path.is_some(),
        thumb_path.is_some()
    );

    // Step 1: Delete the database record in a transaction
    // This is the atomic step — once committed, the file is "deleted" from the user's perspective
    let mut tx = pool.begin().await?;
    let result = sqlx::query("DELETE FROM files WHERE id = ? AND owner_id = ?")
        .bind(file_id)
        .bind(owner_id)
        .execute(&mut *tx)
        .await?;

    if result.rows_affected() == 0 {
        // This shouldn't happen since we just fetched the file, but handle it defensively
        tx.rollback().await?;
        return Err(AppError::NotFound("文件不存在".into()));
    }

    tx.commit().await?;
    tracing::info!("Database record deleted for file id={}", file_id);

    // Step 2: Delete physical files (best-effort with retry)
    // These are spawned as concurrent tasks for efficiency
    let cleanup_stored = cleanup_stored_file(config, &stored_path);
    let cleanup_preview = async {
        if let Some(ref pp) = preview_path {
            cleanup_preview_file(config, pp).await;
        }
    };
    let cleanup_thumb = async {
        if let Some(ref tp) = thumb_path {
            cleanup_preview_file(config, tp).await;
        }
    };

    tokio::join!(cleanup_stored, cleanup_preview, cleanup_thumb);

    Ok(())
}

/// Clean up orphaned preview/thumbnail files whose source file no longer exists on disk.
/// This is a maintenance function that can be called periodically or on startup.
#[allow(dead_code)]
pub async fn cleanup_orphaned_previews(
    pool: &SqlitePool,
    config: &Config,
) -> Result<usize, AppError> {
    // Find all database records that have preview or thumbnail paths
    let records: Vec<(String, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT stored_path, preview_path, thumb_path FROM files WHERE preview_path IS NOT NULL OR thumb_path IS NOT NULL",
    )
    .fetch_all(pool)
    .await?;

    let mut cleaned = 0;
    for (stored_path, preview_path, thumb_path) in &records {
        let stored_full = config.upload_dir.join(stored_path);
        // If the source file is missing, the preview/thumbnail are orphaned
        if !tokio::fs::try_exists(&stored_full).await.unwrap_or(false) {
            if let Some(ref pp) = preview_path {
                let full_path = config.upload_dir.join(pp);
                if tokio::fs::try_exists(&full_path).await.unwrap_or(false) {
                    match delete_physical_file_with_retry(&full_path, "孤立预览文件").await {
                        Ok(()) => cleaned += 1,
                        Err(e) => tracing::warn!("Failed to clean orphaned preview: {} - {}", full_path.display(), e.message()),
                    }
                }
            }
            if let Some(ref tp) = thumb_path {
                let full_path = config.upload_dir.join(tp);
                if tokio::fs::try_exists(&full_path).await.unwrap_or(false) {
                    match delete_physical_file_with_retry(&full_path, "孤立缩略图文件").await {
                        Ok(()) => cleaned += 1,
                        Err(e) => tracing::warn!("Failed to clean orphaned thumbnail: {} - {}", full_path.display(), e.message()),
                    }
                }
            }
        }
    }

    if cleaned > 0 {
        tracing::info!("Cleaned up {} orphaned preview/thumbnail files", cleaned);
    }
    Ok(cleaned)
}

