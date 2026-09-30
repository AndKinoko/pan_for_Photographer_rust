use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::config::Config;
use crate::errors::AppError;
use crate::models::file::{File, FileInfo};
use crate::models::folder::Folder;
use crate::utils::pagination::{self, Cursor, Dir, Paged};
use sqlx::SqlitePool;

/// 永不接受上传 / 改名的扩展名（纵深防御的第一层）。
///
/// 注意：黑名单天然不可能穷尽（`.xhtml`、`.shtml`、`.xml`、`.hta` 等都可能渲染成文档）。
/// 真正的安全边界是 [`is_inline_safe`] 的**白名单**——即使某个类型漏过了这里，
/// 只要它不在内联白名单内，响应就会被强制降级为 `attachment`，浏览器不会渲染。
const NEVER_ALLOWED_EXTENSIONS: &[&str] = &[
    ".html", ".htm", ".xhtml", ".xht", ".shtml", ".svg", ".svgz", ".js", ".mjs", ".hta", ".mht",
    ".mhtml",
];

/// 允许以 `inline` 返回、并携带真实 MIME 的扩展名**白名单**。
///
/// 这是防止「上传内容变成同源可执行文档」的根本边界，与上传黑名单相互独立。
/// 不在清单内的类型一律以 `attachment` + `application/octet-stream` 返回。
const INLINE_SAFE_EXTENSIONS: &[&str] = &[
    // 图片与 RAW（本项目主要场景；这些类型浏览器只会当图像解码）
    "jpg", "jpeg", "png", "gif", "bmp", "webp", "avif", "tiff", "tif", "heic", "heif", "nef",
    "cr2", "cr3", "crw", "arw", "sr2", "srf", "dng", "raf", "orf", "rw2", "nrw",
    // 影音
    "mp4", "mov", "m4v", "webm", "mkv", "avi", "mp3", "wav", "flac", "ogg", "aac", "m4a",
    // 文档：浏览器内置查看器渲染，不执行页面脚本
    "pdf",
];

/// 支持预览的图片格式
const IMAGE_FORMATS: &[&str] = &["jpg", "jpeg", "png", "gif", "bmp", "webp", "tiff", "tif"];

/// 支持通过原始处理预览的RAW格式
const RAW_FORMATS: &[&str] = &[
    "nef", "cr2", "cr3", "crw", "arw", "sr2", "srf", "dng", "raf", "orf", "rw2", "nrw",
];

/// 文件列表的排序标识。它同时是游标里携带的「排序名」——
/// 服务端用它校验「上一页的游标」与「本次请求的排序」是否一致，
/// 不一致就报错，而不是静默返回位置无意义的结果。
pub const SORT_UPLOADED_AT: &str = "uploaded_at";

/// 回收站列表的排序标识
pub const SORT_DELETED_AT: &str = "deleted_at";

/// 列出文件夹中的文件（如果 folder_id 为 None，则列出根目录下的文件）
/// 自动过滤已软删除的文件。
///
/// 游标分页（keyset）。取 `limit + 1` 条来判断是否还有下一页，
/// 因此不需要额外查询就能给出 `next_cursor`；`total` 是一次廉价的索引 COUNT。
///
/// **排序必须带 `id` 兜底。** `uploaded_at` 是 `datetime('now')`，只有秒精度，
/// 而一次上传 200 个文件会全部落在同一秒里；只按 `uploaded_at` 排序时并列行的
/// 先后不确定，翻页会漏项或重复。加了 `id DESC` 之后顺序才是全序。
pub async fn list_files(
    pool: &SqlitePool,
    owner_id: i64,
    folder_id: Option<i64>,
    limit: i64,
    cursor: Option<Cursor>,
) -> Result<Paged<FileInfo>, AppError> {
    if let Some(fid) = folder_id {
        // 验证文件夹属于当前用户且未删除
        let folder = sqlx::query_as::<_, Folder>(
            "SELECT * FROM folders WHERE id = ? AND owner_id = ? AND deleted_at IS NULL",
        )
        .bind(fid)
        .bind(owner_id)
        .fetch_optional(pool)
        .await?;

        if folder.is_none() {
            return Err(AppError::NotFound("文件夹不存在".into()));
        }
    }

    let total: i64 = if let Some(fid) = folder_id {
        sqlx::query_scalar(
            "SELECT COUNT(*) FROM files WHERE owner_id = ? AND folder_id = ? AND deleted_at IS NULL",
        )
        .bind(owner_id)
        .bind(fid)
        .fetch_one(pool)
        .await?
    } else {
        sqlx::query_scalar(
            "SELECT COUNT(*) FROM files WHERE owner_id = ? AND folder_id IS NULL AND deleted_at IS NULL",
        )
        .bind(owner_id)
        .fetch_one(pool)
        .await?
    };

    let mut sql = String::from("SELECT * FROM files WHERE owner_id = ?");
    if folder_id.is_some() {
        sql.push_str(" AND folder_id = ?");
    } else {
        sql.push_str(" AND folder_id IS NULL");
    }
    sql.push_str(" AND deleted_at IS NULL");
    if cursor.is_some() {
        pagination::push_cursor_condition(&mut sql, SORT_UPLOADED_AT, Dir::Desc);
    }
    sql.push_str(&pagination::order_by(SORT_UPLOADED_AT, Dir::Desc));
    sql.push_str(" LIMIT ?");

    // 绑定顺序必须与上面 `?` 的出现顺序一致
    let mut q = sqlx::query_as::<_, File>(&sql).bind(owner_id);
    if let Some(fid) = folder_id {
        q = q.bind(fid);
    }
    if let Some(c) = &cursor {
        let v = c.as_text();
        q = q.bind(v.clone()).bind(v).bind(c.id);
    }
    let rows = q.bind(limit + 1).fetch_all(pool).await?;

    let (rows, next_cursor) =
        pagination::split_page(rows, limit, SORT_UPLOADED_AT, |f| (f.uploaded_at.clone(), f.id));

    Ok(Paged {
        items: rows.into_iter().map(|f| f.to_info()).collect(),
        total,
        next_cursor,
    })
}

/// 重命名文件
pub async fn rename_file(
    pool: &SqlitePool,
    file_id: i64,
    owner_id: i64,
    new_name: &str,
) -> Result<File, AppError> {
    // 规范化 + 校验（长度/路径分隔符/控制字符/双引号）
    let new_name = sanitize_filename(new_name)?;
    // 与上传路径同一套扩展名校验。
    // 这一步不能省：否则可以「上传 x.jpg（内容为 HTML）→ 改名为 x.html」绕过上传校验，
    // 再经由内联媒体接口把用户内容变成同源可执行文档。
    validate_extension(&new_name)?;

    // 获取文件并验证所有权
    let file = get_file(pool, file_id, owner_id).await?;

    // 检查同名文件（排除自身）
    let existing = if let Some(fid) = file.folder_id {
        sqlx::query_scalar::<_, i64>(
            "SELECT id FROM files WHERE owner_id = ? AND folder_id = ? AND LOWER(original_name) = LOWER(?) AND id != ? AND deleted_at IS NULL",
        )
        .bind(owner_id)
        .bind(fid)
        .bind(&new_name)
        .bind(file_id)
        .fetch_optional(pool)
        .await?
    } else {
        sqlx::query_scalar::<_, i64>(
            "SELECT id FROM files WHERE owner_id = ? AND folder_id IS NULL AND LOWER(original_name) = LOWER(?) AND id != ? AND deleted_at IS NULL",
        )
        .bind(owner_id)
        .bind(&new_name)
        .bind(file_id)
        .fetch_optional(pool)
        .await?
    };

    if existing.is_some() {
        return Err(AppError::Conflict("同名文件已存在".into()));
    }

    let updated = sqlx::query_as::<_, File>(
        "UPDATE files SET name = ?, original_name = ?, updated_at = datetime('now') WHERE id = ? AND owner_id = ? RETURNING *",
    )
    .bind(&new_name)
    .bind(&new_name)
    .bind(file_id)
    .bind(owner_id)
    .fetch_one(pool)
    .await?;

    Ok(updated)
}

/// 软删除文件（移入回收站）
pub async fn soft_delete_file(
    pool: &SqlitePool,
    file_id: i64,
    owner_id: i64,
) -> Result<(), AppError> {
    let result = sqlx::query(
        "UPDATE files SET deleted_at = datetime('now') WHERE id = ? AND owner_id = ? AND deleted_at IS NULL",
    )
    .bind(file_id)
    .bind(owner_id)
    .execute(pool)
    .await?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound("文件不存在或已在回收站中".into()));
    }

    tracing::info!("文件已移入回收站: id={}", file_id);
    Ok(())
}

/// 从回收站恢复文件
pub async fn restore_file(
    pool: &SqlitePool,
    file_id: i64,
    owner_id: i64,
) -> Result<(), AppError> {
    // 获取文件信息以检查其 folder_id 是否仍然有效
    let file = sqlx::query_as::<_, File>(
        "SELECT * FROM files WHERE id = ? AND owner_id = ? AND deleted_at IS NOT NULL",
    )
    .bind(file_id)
    .bind(owner_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| AppError::NotFound("文件不在回收站中".into()))?;

    // 如果文件有 folder_id，检查文件夹是否也被软删除了
    if let Some(fid) = file.folder_id {
        let folder_exists: Option<(i64,)> = sqlx::query_as(
            "SELECT id FROM folders WHERE id = ? AND owner_id = ? AND deleted_at IS NULL",
        )
        .bind(fid)
        .bind(owner_id)
        .fetch_optional(pool)
        .await?;

        if folder_exists.is_none() {
            // 父文件夹也被删除了，将文件移到根目录
            sqlx::query(
                "UPDATE files SET folder_id = NULL, deleted_at = NULL WHERE id = ? AND owner_id = ?",
            )
            .bind(file_id)
            .bind(owner_id)
            .execute(pool)
            .await?;
        } else {
            sqlx::query(
                "UPDATE files SET deleted_at = NULL WHERE id = ? AND owner_id = ?",
            )
            .bind(file_id)
            .bind(owner_id)
            .execute(pool)
            .await?;
        }
    } else {
        sqlx::query(
            "UPDATE files SET deleted_at = NULL WHERE id = ? AND owner_id = ?",
        )
        .bind(file_id)
        .bind(owner_id)
        .execute(pool)
        .await?;
    }

    tracing::info!("文件已从回收站恢复: id={}", file_id);
    Ok(())
}

/// 列出回收站中的文件
/// 回收站里的文件，按删除时间倒序，游标分页。
pub async fn list_trash_files(
    pool: &SqlitePool,
    owner_id: i64,
    limit: i64,
    cursor: Option<Cursor>,
) -> Result<Paged<FileInfo>, AppError> {
    let total: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM files WHERE owner_id = ? AND deleted_at IS NOT NULL")
            .bind(owner_id)
            .fetch_one(pool)
            .await?;

    let mut sql =
        String::from("SELECT * FROM files WHERE owner_id = ? AND deleted_at IS NOT NULL");
    if cursor.is_some() {
        pagination::push_cursor_condition(&mut sql, SORT_DELETED_AT, Dir::Desc);
    }
    sql.push_str(&pagination::order_by(SORT_DELETED_AT, Dir::Desc));
    sql.push_str(" LIMIT ?");

    let mut q = sqlx::query_as::<_, File>(&sql).bind(owner_id);
    if let Some(c) = &cursor {
        let v = c.as_text();
        q = q.bind(v.clone()).bind(v).bind(c.id);
    }
    let rows = q.bind(limit + 1).fetch_all(pool).await?;

    let (rows, next_cursor) = pagination::split_page(rows, limit, SORT_DELETED_AT, |f| {
        // WHERE 里的 `deleted_at IS NOT NULL` 保证了这里是 Some。
        // 用 expect 而不是 `unwrap_or_default()`：空串当游标会让下一页的
        // `deleted_at < ''` 匹配不到任何行 —— 前端会以为「已经到底」，
        // 于是静默少显示一批文件。这种失败必须响亮。
        (
            f.deleted_at
                .clone()
                .expect("回收站查询的 WHERE 已排除 deleted_at IS NULL"),
            f.id,
        )
    });

    Ok(Paged {
        items: rows.into_iter().map(|f| f.to_info()).collect(),
        total,
        next_cursor,
    })
}

/// 清空回收站（永久删除所有已软删除的文件记录）
/// 磁盘清理交由周期 GC（sweeper）统一处理。
pub async fn empty_trash(
    pool: &SqlitePool,
    owner_id: i64,
) -> Result<usize, AppError> {
    let result = sqlx::query(
        "DELETE FROM files WHERE owner_id = ? AND deleted_at IS NOT NULL",
    )
    .bind(owner_id)
    .execute(pool)
    .await?;

    let count = result.rows_affected() as usize;
    tracing::info!("已清空回收站: {} 个文件记录被永久删除", count);
    Ok(count)
}

/// 根据ID获取单个文件，验证所有权（排除已删除的文件）
pub async fn get_file(
    pool: &SqlitePool,
    file_id: i64,
    owner_id: i64,
) -> Result<File, AppError> {
    sqlx::query_as::<_, File>("SELECT * FROM files WHERE id = ? AND owner_id = ? AND deleted_at IS NULL")
        .bind(file_id)
        .bind(owner_id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| AppError::NotFound("文件不存在".into()))
}

/// 根据ID获取文件，不验证所有权（用于公开分享访问）。
///
/// **不验证所有权 ≠ 不过滤软删除。** 分享是「持有链接即可访问」，
/// 所以这里唯一需要额外把关的就是文件本身是否还在：文件进回收站后，
/// 分享链接必须一并失效——否则就是「我删了但客户还能下载」，
/// 在交付场景里这是一次真实的交付事故，而不只是越权问题。
pub async fn get_file_by_id(pool: &SqlitePool, file_id: i64) -> Result<File, AppError> {
    sqlx::query_as::<_, File>(
        "SELECT * FROM files WHERE id = ? AND deleted_at IS NULL",
    )
    .bind(file_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| AppError::NotFound("文件不存在或已删除".into()))
}

/// 根据ID获取单个文件，验证所有权（包含已删除的文件，用于永久删除）
pub async fn get_file_include_deleted(
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

/// 提取小写扩展名（不含点）；无扩展名返回空串。
pub fn extension_of(filename: &str) -> String {
    Path::new(filename)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase()
}

/// 验证文件扩展名不在「永不允许」清单内。
/// 上传（`validate_extension` 在 upload 阶段调用）与改名两条路径都必须调用。
pub fn validate_extension(filename: &str) -> Result<(), AppError> {
    let ext_with_dot = format!(".{}", extension_of(filename));
    if NEVER_ALLOWED_EXTENSIONS.contains(&ext_with_dot.as_str()) {
        return Err(AppError::BadRequest(format!(
            "文件类型 \"{}\" 不允许使用（存在安全风险）",
            ext_with_dot
        )));
    }
    Ok(())
}

/// 该扩展名是否允许以 `inline` 返回并携带真实 MIME。
///
/// 这是防止用户上传内容变成本源可执行文档的**根本边界**：任何不在白名单内的类型，
/// 响应都会被降级为 `attachment` + `application/octet-stream`，浏览器只会下载、
/// 不会渲染——因此即使 MIME 推导逻辑将来出错，也不会执行脚本。
pub fn is_inline_safe(ext: &str) -> bool {
    let e = ext.trim_start_matches('.').to_lowercase();
    INLINE_SAFE_EXTENSIONS.contains(&e.as_str())
}

/// 规范化并校验用户提交的文件名（重命名路径）。
///
/// 拒绝：空 / 全空白、`.` 与 `..`、路径分隔符、控制字符（含 CR/LF——会破坏 HTTP 头，
/// 使 `HeaderValue` 构造失败并返回 500）与双引号（会破坏 `Content-Disposition` 的
/// quoted-string，导致下载文件名被截断）；长度上限 255 字符。
///
/// 存储名由服务端生成（`user_{id}/{uuid}.{ext}`），所以这里不是为了防路径穿越，
/// 而是为了不让非法字符进入 `original_name`——它会被回填进响应头与 MIME 推导逻辑。
pub fn sanitize_filename(raw: &str) -> Result<String, AppError> {
    let name = raw.trim();
    if name.is_empty() {
        return Err(AppError::BadRequest("文件名不能为空".into()));
    }
    if name == "." || name == ".." {
        return Err(AppError::BadRequest("文件名不合法".into()));
    }
    if name.chars().count() > 255 {
        return Err(AppError::BadRequest(
            "文件名过长（最多 255 个字符）".into(),
        ));
    }
    if name.contains('/') || name.contains('\\') {
        return Err(AppError::BadRequest("文件名不能包含路径分隔符".into()));
    }
    if name.chars().any(|c| c.is_control() || c == '"') {
        return Err(AppError::BadRequest(
            "文件名不能包含控制字符或双引号".into(),
        ));
    }
    Ok(name.to_string())
}

/// 检查同一文件夹中是否存在重复文件名（不区分大小写）
pub async fn check_duplicates(
    pool: &SqlitePool,
    owner_id: i64,
    folder_id: Option<i64>,
    filename: &str,
) -> Result<bool, AppError> {
    let existing = if let Some(fid) = folder_id {
        sqlx::query_scalar::<_, String>(
            "SELECT original_name FROM files WHERE owner_id = ? AND folder_id = ? AND LOWER(original_name) = LOWER(?) AND deleted_at IS NULL",
        )
        .bind(owner_id)
        .bind(fid)
        .bind(filename)
        .fetch_optional(pool)
        .await?
    } else {
        sqlx::query_scalar::<_, String>(
            "SELECT original_name FROM files WHERE owner_id = ? AND folder_id IS NULL AND LOWER(original_name) = LOWER(?) AND deleted_at IS NULL",
        )
        .bind(owner_id)
        .bind(filename)
        .fetch_optional(pool)
        .await?
    };

    Ok(existing.is_some())
}

/// 生成唯一的存储文件名：{uuid}.{ext}
pub fn generate_stored_filename(original_name: &str) -> String {
    let ext = Path::new(original_name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("bin");
    format!("{}.{}", Uuid::new_v4().simple(), ext)
}

/// 获取用户的上传目录路径
pub fn user_upload_dir(config: &Config, user_id: i64) -> PathBuf {
    config.upload_dir.join(format!("user_{}", user_id))
}

/// 检查文件类型是否支持预览
pub fn supports_preview(file_type: &str) -> bool {
    let ft = file_type.to_lowercase();
    IMAGE_FORMATS.contains(&ft.as_str()) || RAW_FORMATS.contains(&ft.as_str())
}

/// 删除文件记录（从回收站永久删除）。
/// 磁盘清理交由周期 GC（sweeper）统一处理。
pub async fn delete_file(
    pool: &SqlitePool,
    file_id: i64,
    owner_id: i64,
) -> Result<(), AppError> {
    // 先获取文件记录以验证所有权（支持已软删除的文件）
    let file = get_file_include_deleted(pool, file_id, owner_id).await?;

    let result = sqlx::query("DELETE FROM files WHERE id = ? AND owner_id = ?")
        .bind(file_id)
        .bind(owner_id)
        .execute(pool)
        .await?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound("文件不存在".into()));
    }

    tracing::info!("文件记录已永久删除: id={}", file.id);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> Config {
        Config {
            server_host: "127.0.0.1".into(),
            server_port: 0,
            database_url: "sqlite::memory:".into(),
            upload_dir: std::env::temp_dir().join("pan_test_uploads"),
            static_dir: "static".into(),
            jwt_secret: b"0123456789abcdef0123456789abcdef".to_vec(),
            max_file_size: 1024,
            gc_interval_sec: 0,
        }
    }

    #[test]
    fn validate_extension_blocks_dangerous_types() {
        for name in ["evil.html", "evil.HTM", "x.svg", "a.js", "b.mjs", "c.SVG"] {
            assert!(
                validate_extension(name).is_err(),
                "{} 应被阻止",
                name
            );
        }
    }

    #[test]
    fn validate_extension_allows_media_and_unknown_types() {
        for name in ["photo.NEF", "raw.cr2", "clip.mp4", "doc.pdf", "noext", "archive.tar.gz"] {
            assert!(
                validate_extension(name).is_ok(),
                "{} 应被允许",
                name
            );
        }
    }

    #[test]
    fn generate_stored_filename_keeps_extension_and_is_unique() {
        let a = generate_stored_filename("DSC_0001.NEF");
        let b = generate_stored_filename("DSC_0001.NEF");
        assert!(a.ends_with(".NEF"));
        assert!(b.ends_with(".NEF"));
        assert_ne!(a, b, "存储名必须唯一");
        // uuid simple 形态为 32 位十六进制
        let stem = a.trim_end_matches(".NEF");
        assert_eq!(stem.len(), 32);
        assert!(stem.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn generate_stored_filename_defaults_to_bin() {
        let name = generate_stored_filename("no_extension");
        assert!(name.ends_with(".bin"));
    }

    #[test]
    fn supports_preview_covers_images_and_raw() {
        assert!(supports_preview("jpg"));
        assert!(supports_preview("NEF"));
        assert!(supports_preview("tiff"));
        assert!(!supports_preview("mp4"));
        assert!(!supports_preview("pdf"));
        assert!(!supports_preview(""));
    }

    #[test]
    fn user_upload_dir_is_per_user() {
        let config = test_config();
        let dir = user_upload_dir(&config, 42);
        assert!(dir.ends_with("user_42"));
        assert!(dir.starts_with(&config.upload_dir));
    }

    #[test]
    fn validate_extension_blocks_newly_added_renderable_types() {
        // 这些类型都能被浏览器当作文档渲染，必须在黑名单的纵深防御层被拦住
        for name in [
            "a.xhtml", "b.XHTML", "c.shtml", "d.svgz", "e.hta", "f.mht", "g.mhtml", "h.xht",
        ] {
            assert!(validate_extension(name).is_err(), "{} 应被阻止", name);
        }
    }

    #[test]
    fn inline_safe_is_a_whitelist_not_a_denylist() {
        // 白名单内（本项目的常见交付类型）
        for ext in ["jpg", "JPG", ".png", "nef", "cr3", "arw", "mp4", "mov", "pdf", "tiff"] {
            assert!(is_inline_safe(ext), "{} 应在内联白名单内", ext);
        }
        // 白名单外：即便不在上传黑名单里，也不允许 inline 返回
        for ext in ["html", "xhtml", "svg", "js", "txt", "xml", "exe", "zip", ""] {
            assert!(!is_inline_safe(ext), "{} 不应在内联白名单内", ext);
        }
    }

    #[test]
    fn sanitize_filename_rejects_dangerous_input() {
        assert!(sanitize_filename("").is_err());
        assert!(sanitize_filename("   ").is_err());
        assert!(sanitize_filename(".").is_err());
        assert!(sanitize_filename("..").is_err());
        assert!(sanitize_filename("../etc/passwd").is_err());
        assert!(sanitize_filename("a/b.jpg").is_err());
        assert!(sanitize_filename("a\\b.jpg").is_err());
        // CR/LF/NUL 会破坏 HTTP 头
        assert!(sanitize_filename("a\r\nX-Evil: 1.jpg").is_err());
        assert!(sanitize_filename("a\u{0}b.jpg").is_err());
        // 双引号会破坏 Content-Disposition 的 quoted-string
        assert!(sanitize_filename("a\"b.jpg").is_err());
        assert!(sanitize_filename(&"x".repeat(256)).is_err());
    }

    #[test]
    fn sanitize_filename_accepts_normal_names() {
        assert_eq!(
            sanitize_filename("  DSC_0001.NEF  ").unwrap(),
            "DSC_0001.NEF"
        );
        // 中文文件名必须放行：这是本项目的主要使用场景
        assert_eq!(
            sanitize_filename("婚礼精修_001.nef").unwrap(),
            "婚礼精修_001.nef"
        );
        assert_eq!(sanitize_filename("a (1).jpg").unwrap(), "a (1).jpg");
    }

    #[test]
    fn extension_of_lowercases_and_handles_missing() {
        assert_eq!(extension_of("A.JPG"), "jpg");
        assert_eq!(extension_of("archive.tar.gz"), "gz");
        assert_eq!(extension_of("noext"), "");
    }
}

