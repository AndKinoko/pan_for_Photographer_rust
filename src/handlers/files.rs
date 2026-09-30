use axum::{
    body::Body,
    extract::{Multipart, Path, Query, State},
    http::{HeaderMap, header, StatusCode},
    response::Response,
    Json,
};
use futures_util::{StreamExt, TryStreamExt};
use tokio_util::io::ReaderStream;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::{Path as StdPath, PathBuf};
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::sync::Semaphore;
use uuid::Uuid;

use crate::config::Config;
use crate::errors::AppError;
use crate::middleware::auth::AuthUser;
use crate::models::file::File;
use crate::services::{file_service, preview_service};
use crate::utils::pagination;
use sqlx::SqlitePool;

// ===========================================================================
// 下载 / 媒体响应的安全策略
// ---------------------------------------------------------------------------
// 背景：`original_name` 是**用户可改**的（重命名接口），因此不能作为 MIME 推导的
// 可信来源。此前 `serve_media` 末段直接用它取 `Content-Type` 并以 `inline` 返回，
// 形成「上传 x.jpg（内容为 HTML）→ 改名为 x.html → 同源执行脚本」的存储型 XSS。
//
// 现在的边界是 `file_service::is_inline_safe` 的**白名单**：
// 只有图片 / RAW / 影音 / PDF 允许 inline，其余一律降级为 attachment + octet-stream。
// 浏览器对 attachment 只会下载、不会渲染，因此脚本无从执行。
// ===========================================================================

/// 给下载 / 媒体响应统一追加的安全头。
///
/// - `nosniff`：阻止浏览器把响应嗅探成别的类型。
/// - `X-Frame-Options`：阻止被跨站页面嵌框。
///
/// 这里**不**给内联白名单内的类型加 `Content-Security-Policy: sandbox`：
/// PDF 走浏览器内置查看器，加沙箱会让预览直接失效。真正的边界是白名单降级机制，
/// 只有确定要下载的内容才额外补上 CSP 沙箱（见 `serve_media` 末段）。
pub(crate) fn harden(builder: axum::http::response::Builder) -> axum::http::response::Builder {
    builder
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .header("x-frame-options", "SAMEORIGIN")
}

/// 依据扩展名白名单决定内联响应策略，返回 `(content_type, disposition)`。
pub(crate) fn inline_response_policy(original_name: &str) -> (String, &'static str) {
    let ext = file_service::extension_of(original_name);
    if file_service::is_inline_safe(&ext) {
        (
            mime_guess::from_path(original_name)
                .first_or_octet_stream()
                .to_string(),
            "inline",
        )
    } else {
        ("application/octet-stream".to_string(), "attachment")
    }
}

/// 构造 `Content-Disposition` 值。
///
/// 同时给出 `filename=`（ASCII 回退）与 `filename*=UTF-8''…`（RFC 5987），
/// 让中文文件名在各浏览器下都能正确落盘，而不是依赖浏览器对裸 UTF-8 的容错。
/// 入参必须已在入口经 `file_service::sanitize_filename` 处理过（无控制字符、无双引号）。
pub(crate) fn content_disposition(disposition: &str, filename: &str) -> String {
    let ascii_fallback: String = filename
        .chars()
        .map(|c| {
            if c.is_ascii() && c != '"' && c != '\\' {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!(
        "{}; filename=\"{}\"; filename*=UTF-8''{}",
        disposition,
        ascii_fallback,
        percent_encode_utf8(filename)
    )
}

/// RFC 5987 的 percent-encoding：attr-char 之外全部转义。
fn percent_encode_utf8(s: &str) -> String {
    let mut out = String::new();
    for b in s.as_bytes() {
        let c = *b as char;
        let attr_char = c.is_ascii_alphanumeric()
            || matches!(
                c,
                '!' | '#' | '$' | '&' | '+' | '-' | '.' | '^' | '_' | '`' | '|' | '~'
            );
        if attr_char {
            out.push(c);
        } else {
            out.push_str(&format!("%{:02X}", b));
        }
    }
    out
}

#[derive(Debug, Deserialize)]
pub struct FileListQuery {
    pub folder_id: Option<i64>,
    /// 每页条数。缺省 100，上限 500（`utils::pagination`）
    pub limit: Option<i64>,
    /// 上一页响应里的 `next_cursor`
    pub cursor: Option<String>,
}

/// GET /api/files?folder_id={id}&limit={n}&cursor={c} 获取文件列表（游标分页）
///
/// `data` 是 `{ files, total, has_more, next_cursor, limit }`。
/// 文件夹不走这个接口 —— 前端另外并行请求 `/api/folders`，
/// 两个独立排序的列表共用一个游标只会让两边都变复杂。
pub async fn list_files(
    State(pool): State<SqlitePool>,
    auth: AuthUser,
    Query(query): Query<FileListQuery>,
) -> Result<Json<Value>, AppError> {
    let limit = pagination::normalize_limit(query.limit);
    let cursor = pagination::parse_cursor(
        query.cursor.as_deref(),
        file_service::SORT_UPLOADED_AT,
    )?;
    let page =
        file_service::list_files(&pool, auth.user_id, query.folder_id, limit, cursor).await?;

    Ok(Json(json!({
        "success": true,
        "data": {
            "files": page.items,
            "total": page.total,
            "has_more": page.has_more(),
            "next_cursor": page.next_cursor,
            "limit": limit,
        },
        "error": null
    })))
}

/// RAII 守卫：axum 在客户端断开/请求中断时会 drop handler 的 future，
/// 此处 Drop 会立即清理未提交的临时 .part 文件，避免残留半写文件。
struct PartialGuard {
    path: Option<PathBuf>,
}

impl Drop for PartialGuard {
    fn drop(&mut self) {
        if let Some(p) = self.path.take() {
            let _ = std::fs::remove_file(&p);
        }
    }
}

/// multipart 解析过程中已流式写盘的待提交文件（暂存为 .part，提交时 rename）
struct PendingUpload {
    guard: PartialGuard, // 持有临时 .part 路径，rename 提交后置 None
    stored_name: String, // {uuid}.{ext}
    file_name: String,
    size: i64,
    ext: String,
}

/// POST /api/files/upload 上传文件
/// 采用 multipart 流式写盘：对每个 file 字段用 bytes_stream() 逐块写入，
/// 字段内 chunk 计数作为单文件权威限制（max_file_size）；
/// 请求总 Content-Length 做粗预检（> max_file_size 直接 413）。
pub async fn upload_files(
    State(pool): State<SqlitePool>,
    State(config): State<Config>,
    State(sem): State<Arc<Semaphore>>,
    auth: AuthUser,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Result<Json<Value>, AppError> {
    // 粗预检：全请求 Content-Length 超限立即 413，省得传完才被 Limited 掐断。
    // 注意这是全请求总量预算；单文件权威限制由字段内 chunk 计数承担。
    if let Some(cl) = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
    {
        if cl > config.max_file_size {
            return Err(AppError::PayloadTooLarge("上传总量超过限制".into()));
        }
    }

    let mut folder_id: Option<i64> = None;
    let mut explicit_user: Option<i64> = None;
    let mut pending: Vec<PendingUpload> = Vec::new();
    let mut uploaded_files: Vec<Value> = Vec::new();
    let mut errors: Vec<String> = Vec::new();

    // 临时落盘根目录（与 user_* 同一文件系统，rename 原子提交）
    let temp_root = config.upload_dir.join(".tmp_incoming");
    tokio::fs::create_dir_all(&temp_root).await?;

    // 阶段 1：收集元数据字段；对 file 字段连续流式写盘到 .part，不整体缓冲
    //
    // 这里必须用 loop + 显式 match，而不是 `while let Ok(Some(f))`。
    // 后者把解析器的 Err（体积超限、客户端中断、编码错误）与正常的
    // Ok(None)（流结束）当成同一件事：中途出错时循环安静退出，
    // 已写入的 .part 仍会被阶段 2 提交，响应报 success:true，
    // 而客户端以为整个文件都传完了——「报成功但只传了一部分」。
    // 请求整体超限时尤其致命：DefaultBodyLimit 掐断请求体，
    // 走的正是这条 Err 分支。
    loop {
        let field = match multipart.next_field().await {
            Ok(Some(f)) => f,
            // 正常结束
            Ok(None) => break,
            Err(e) => {
                return Err(AppError::BadRequest(format!(
                    "上传中断：{}（已接收的部分不会提交）",
                    e
                )));
            }
        };
        let name = field.name().unwrap_or("").to_string();

        if name == "folder_id" {
            let text = field.text().await.unwrap_or_default();
            if !text.is_empty() {
                folder_id = text.parse::<i64>().ok();
            }
        } else if name == "user_id" {
            let text = field.text().await.unwrap_or_default();
            if !text.is_empty() {
                explicit_user = text.parse::<i64>().ok();
            }
        } else if name == "file" {
            let file_name = field.file_name().unwrap_or("unknown").to_string();
            if file_name.is_empty() {
                continue;
            }

            // 文件名规范化：非法字符（控制字符 / 双引号 / 路径分隔符）在写盘前拦下。
            // 必要性：`original_name` 会被回填进 Content-Disposition 与 MIME 推导，
            // 含 CR/LF 会让 HeaderValue 构造失败（500），含双引号会让下载文件名被截断。
            let file_name = match file_service::sanitize_filename(&file_name) {
                Ok(n) => n,
                Err(e) => {
                    errors.push(e.message().to_string());
                    continue;
                }
            };

            // 扩展名校验：廉价且前置，避免为不合法类型写盘
            if let Err(e) = file_service::validate_extension(&file_name) {
                errors.push(e.message().to_string());
                continue;
            }

            let tmp_path = temp_root.join(format!("{}.part", Uuid::new_v4().simple()));
            let guard = PartialGuard {
                path: Some(tmp_path.clone()),
            };
            let mut out = tokio::fs::File::create(&tmp_path).await?;

            // 字段内 chunk 计数：单文件权威限制 = max_file_size
            let mut size: u64 = 0;
            let mut too_large = false;
            let mut stream = field.into_stream();
            while let Some(chunk) = stream.next().await {
                let chunk =
                    chunk.map_err(|_| AppError::BadRequest("读取文件数据失败".into()))?;
                size += chunk.len() as u64;
                if size > config.max_file_size {
                    too_large = true;
                    break;
                }
                out.write_all(&chunk).await?;
            }

            if too_large {
                errors.push(format!("文件 \"{}\" 大小超过限制", file_name));
                // guard Drop 清理 .part
                continue;
            }

            out.flush().await?;
            drop(out); // 关闭句柄，确保后续 rename 成功

            let ext = StdPath::new(&file_name)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_lowercase()
                .to_string();

            pending.push(PendingUpload {
                guard,
                stored_name: file_service::generate_stored_filename(&file_name),
                file_name,
                size: size as i64,
                ext,
            });
        }
    }

    // 确定上传归属用户：默认当前登录用户；若指定 user_id，则仅管理员可为他人上传
    let owner_id: i64 = if let Some(target) = explicit_user {
        let role: Option<(String,)> = sqlx::query_as("SELECT role FROM users WHERE id = ?")
            .bind(auth.user_id)
            .fetch_optional(&pool)
            .await?;
        if role.as_ref().map(|r| r.0.as_str()) != Some("admin") {
            return Err(AppError::Forbidden("需要管理员权限才能为其他用户上传".into()));
        }
        let exists: Option<(i64,)> = sqlx::query_as("SELECT id FROM users WHERE id = ?")
            .bind(target)
            .fetch_optional(&pool)
            .await?;
        if exists.is_none() {
            return Err(AppError::NotFound("目标用户不存在".into()));
        }
        target
    } else {
        auth.user_id
    };

    // 目标文件夹必须属于**文件的归属用户**（owner_id），且未在回收站中。
    //
    // 为什么不是 auth.user_id：管理员代传时 auth.user_id 是管理员自己，
    // 而 owner_id 在上面的分支里已解析成目标用户。绑 auth.user_id 的话，
    // 管理员为自己客户的文件夹上传时永远查不到——代传功能 100% 不可用。
    //
    // 为什么仍要校验：folder_id 此前是从 multipart 字段直接 parse 出来的，
    // 没有任何校验，于是可以把文件写进他人文件夹；叠加 folder_service 里
    // 原先缺 owner_id 的「按 folder_id 批量删/恢复」，受害者删自己的文件夹
    // 会连带清掉挂上去的记录。两半都已修，这里是入口这一半。
    //
    // 用 owner_id 校验同时也是安全边界：非管理员的 owner_id 恒等于
    // auth.user_id（见上面 explicit_user 的非管理员分支直接 403），
    // 所以这条路径没有放宽任何权限。
    if let Some(fid) = folder_id {
        let owned: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM folders WHERE id = ? AND owner_id = ? AND deleted_at IS NULL",
        )
        .bind(fid)
        .bind(owner_id)
        .fetch_optional(&pool)
        .await?;
        if owned.is_none() {
            return Err(AppError::BadRequest("目标文件夹不存在".into()));
        }
    }

    let (quota_bytes,): (i64,) = sqlx::query_as("SELECT quota_bytes FROM users WHERE id = ?")
        .bind(owner_id)
        .fetch_one(&pool)
        .await?;

    // 阶段 2：提交已流式落盘的待处理文件（.part -> rename 原子提交 + INSERT）
    for pu in pending {
        // 重复检查（此时已能确定 owner/folder）
        if file_service::check_duplicates(&pool, owner_id, folder_id, &pu.file_name).await? {
            errors.push(format!("文件 \"{}\" 已存在，已跳过", pu.file_name));
            // pu 的 guard Drop 移除临时 .part
            continue;
        }

        let user_dir = file_service::user_upload_dir(&config, owner_id);
        tokio::fs::create_dir_all(&user_dir).await?;

        let stored_path = format!("user_{}/{}", owner_id, pu.stored_name);
        let full_path = config.upload_dir.join(&stored_path);

        // 同文件系统下 rename 原子提交
        let src = pu.guard.path.clone().ok_or_else(|| {
            AppError::Internal("内部状态错误：临时文件路径缺失".into())
        })?;
        tokio::fs::rename(&src, &full_path).await?;

        // 配额校验与 INSERT 收进**同一条语句**，由数据库裁决。
        //
        // 原来是「先 SELECT SUM 再判断、然后单独 INSERT」，两个并发请求会读到
        // **同一个** used 值、都判定通过，各自 INSERT——实测 3MB 配额下 8 个
        // 并发 900KB 请求写入了 6.15MB，超配额一倍。
        //
        // 改成 `INSERT ... SELECT ... WHERE used + ? <= quota`：判定和写入在
        // 同一条 SQL 里，SQLite 保证单条语句的原子性，读到的 used 与写入的
        // 行之间不存在窗口。返回 0 行即表示超配额。
        //
        // WHERE 里的 SUM 在 INSERT 执行瞬间求值，与其他写事务互斥（WAL 下
        // 写是全库串行的），因此并发请求会依次看到彼此已提交的文件。
        let file_record = sqlx::query_as::<_, File>(
            r#"
            INSERT INTO files (name, original_name, stored_path, preview_path, thumb_path,
                               owner_id, folder_id, size, file_type)
            SELECT ?, ?, ?, NULL, NULL, ?, ?, ?, ?
            WHERE (SELECT COALESCE(SUM(size), 0) FROM files WHERE owner_id = ?) + ? <= ?
            RETURNING *
            "#,
        )
        .bind(&pu.file_name)
        .bind(&pu.file_name)
        .bind(&stored_path)
        .bind(owner_id)
        .bind(folder_id)
        .bind(pu.size)
        .bind(&pu.ext)
        .bind(owner_id)
        .bind(pu.size)
        .bind(quota_bytes)
        .fetch_optional(&pool)
        .await;

        let file_record = match file_record {
            Ok(Some(r)) => r,
            Ok(None) => {
                // 超配额：物理文件已 rename，但库里没有记录。
                // 立刻删掉它——否则会留下一个 GC 也要等 5 分钟才清理的孤儿。
                let _ = tokio::fs::remove_file(&full_path).await;
                errors.push(format!("文件 \"{}\" 超出网盘配额，已跳过", pu.file_name));
                // pu 的 guard Drop 也会尝试清理（此时 path 已被 rename 走，
                // RemoveFileGuard 找不到文件会静默跳过）
                continue;
            }
            Err(e) => {
                // DB 写入失败：删除刚重命名的物理文件，避免产生无记录孤儿文件；
                // 若本步也失败，则交由周期 GC（sweeper）兜底清理。
                let _ = tokio::fs::remove_file(&full_path).await;
                return Err(e.into());
            }
        };

        // DB 写入成功后才标记守卫勿删，避免 Drop 误删已提交文件
        let mut guard = pu.guard;
        guard.path = None;

        // 后台生成预览图 + 缩略图（spawn_blocking 包裹同步图像处理 + 信号量限并发）
        if file_service::supports_preview(&pu.ext) {
            spawn_preview_task(
                sem.clone(),
                pool.clone(),
                config.clone(),
                owner_id,
                file_record.id,
                full_path,
                pu.ext.clone(),
            );
        }

        let info = file_record.to_info();
        uploaded_files.push(serde_json::to_value(info)?);
    }

    if uploaded_files.is_empty() && !errors.is_empty() {
        return Err(AppError::BadRequest(errors.join("; ")));
    }

    Ok(Json(json!({
        "success": true,
        "data": {
            "files": uploaded_files,
            "errors": errors,
            "count": uploaded_files.len(),
        },
        "error": null
    })))
}

/// 后台生成某文件预览图与缩略图：
/// 先经信号量限并发，再通过 spawn_blocking 跑同步图像处理，最后 UPDATE files 表。
///
/// 丢失/失败不影响已上传文件本身（media 接口已有回退到原图的降级逻辑）。
fn spawn_preview_task(
    sem: Arc<Semaphore>,
    pool: SqlitePool,
    config: Config,
    owner_id: i64,
    file_id: i64,
    src_path: PathBuf,
    ext: String,
) {
    tokio::spawn(async move {
        let Ok(permit) = sem.acquire_owned().await else {
            tracing::warn!("预览并发闸未获许可: file_id={}", file_id);
            return;
        };

        let res = tokio::task::spawn_blocking(move || {
            preview_service::generate_preview_and_thumb(&config, owner_id, &src_path, &ext)
        })
        .await;

        match res {
            Ok((preview_rel, thumb_rel)) => {
                let _ = sqlx::query(
                    "UPDATE files SET preview_path = ?, thumb_path = ? WHERE id = ?",
                )
                .bind(preview_rel)
                .bind(thumb_rel)
                .bind(file_id)
                .execute(&pool)
                .await;
            }
            Err(e) => tracing::warn!("预览后台任务失败: file_id={} err={:?}", file_id, e),
        }
        // permit 于作用域结束自动归还
        drop(permit);
    });
}

/// 解析出「本次请求代表哪个用户」，供 `?token=` 旁路的下载/预览接口使用。
///
/// 这两个接口要支持把令牌放进查询参数，是因为 `<img src>` 与下载链接无法带
/// Authorization 头（`frontend/src/api.js` 的 `authUrl()` 就是这么拼的）。
///
/// 但**令牌在查询参数里不等于可以只验签名**：验签只能证明「这令牌是我们签的、
/// 且没超 7 天」，证明不了「签发者现在还有权限」。账号到期后，旧链接在 7 天内
/// 一直能解出 user_id 而畅通无阻——这正是缺陷 1。
///
/// 所以此处与 `AuthUser` 提取器一样，验签之后必须**再查库**确认账号仍然有效。
/// 两条路径共用 `account_guard`，避免同一套规则各写一份再次跑偏。
async fn resolve_user_id(
    pool: &SqlitePool,
    config: &Config,
    auth: Option<AuthUser>,
    params: &std::collections::HashMap<String, String>,
) -> Result<i64, AppError> {
    let user_id = if let Some(user) = auth {
        user.user_id
    } else if let Some(token) = params.get("token") {
        crate::utils::crypto::validate_token(token, config)
            .map_err(|_| AppError::Unauthorized("无效的访问令牌".into()))?
            .sub
    } else {
        return Err(AppError::Unauthorized("请先登录".into()));
    };

    crate::services::account_guard::require_active_account(pool, user_id).await?;
    Ok(user_id)
}

/// GET /api/files/:id/download?token=<jwt> 下载文件
pub async fn download_file(
    State(pool): State<SqlitePool>,
    State(config): State<Config>,
    auth: Option<AuthUser>,
    Path(file_id): Path<i64>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Result<Response, AppError> {
    let user_id = resolve_user_id(&pool, &config, auth, &params).await?;

    let file = file_service::get_file(&pool, file_id, user_id).await?;
    let full_path = config.upload_dir.join(&file.stored_path);

    if !tokio::fs::try_exists(&full_path).await.unwrap_or(false) {
        return Err(AppError::NotFound("文件不存在".into()));
    }

    let file_handle = tokio::fs::File::open(&full_path).await?;
    let file_size = file_handle.metadata().await.map(|m| m.len()).unwrap_or(0);
    let stream = ReaderStream::new(file_handle);
    let body = Body::from_stream(stream);
    let mime = mime_guess::from_path(&file.original_name)
        .first_or_octet_stream();

    // 下载始终是 attachment，浏览器只会落盘、不会渲染，因此不构成执行面。
    let response = harden(
        Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, mime.as_ref())
            .header(
                header::CONTENT_DISPOSITION,
                content_disposition("attachment", &file.original_name),
            )
            // 显式声明 Content-Length，前端 fetch/axios 才能拿到 total 走真实进度
            .header(header::CONTENT_LENGTH, file_size),
    )
    .body(body)
    .map_err(|_| AppError::Internal("响应构建失败".into()))?;

    Ok(response)
}

/// GET /api/files/:id/media?preview=1&token=<jwt> 提供媒体文件服务
pub async fn serve_media(
    State(pool): State<SqlitePool>,
    State(config): State<Config>,
    auth: Option<AuthUser>,
    Path(file_id): Path<i64>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Result<Response, AppError> {
    let user_id = resolve_user_id(&pool, &config, auth, &params).await?;

    let file = file_service::get_file(&pool, file_id, user_id).await?;
    let is_preview = params.get("preview").map(|v| v == "1").unwrap_or(false);
    let is_thumb = params.get("thumb").map(|v| v == "1").unwrap_or(false);

    // 提供缩略图（小，360x240）用于文件列表图标
    if is_thumb {
        if let Some(ref thumb_path) = file.thumb_path {
            let full_path = config.upload_dir.join(thumb_path);
            if tokio::fs::try_exists(&full_path).await.unwrap_or(false) {
                let file_handle = tokio::fs::File::open(&full_path).await?;
                let stream = ReaderStream::new(file_handle);
                let body = Body::from_stream(stream);
                return Ok(Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "image/jpeg")
                    .body(body)
                    .map_err(|_| AppError::Internal("响应构建失败".into()))?);
            }
        }
        // 如果缩略图不存在，回退到预览图
        if let Some(ref preview_path) = file.preview_path {
            let full_path = config.upload_dir.join(preview_path);
            if tokio::fs::try_exists(&full_path).await.unwrap_or(false) {
                let file_handle = tokio::fs::File::open(&full_path).await?;
                let stream = ReaderStream::new(file_handle);
                let body = Body::from_stream(stream);
                return Ok(Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "image/jpeg")
                    .body(body)
                    .map_err(|_| AppError::Internal("响应构建失败".into()))?);
            }
        }
        return Err(AppError::NotFound("缩略图不存在".into()));
    }

    if is_preview {
        if let Some(ref preview_path) = file.preview_path {
            let full_path = config.upload_dir.join(preview_path);
            if tokio::fs::try_exists(&full_path).await.unwrap_or(false) {
                let file_handle = tokio::fs::File::open(&full_path).await?;
                let stream = ReaderStream::new(file_handle);
                let body = Body::from_stream(stream);
                return Ok(Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "image/jpeg")
                    .body(body)
                    .map_err(|_| AppError::Internal("响应构建失败".into()))?);
            }
        }
        // 预览图文件缺失时，对支持的图片格式回退到原始文件。
        // 这里必须与下面的「提供原始文件」分支共用同一套白名单策略：
        // 只要有一条内联路径绕开策略，就是一条独立的 XSS 入口。
        let ext = file_service::extension_of(&file.original_name);
        let image_formats = ["jpg", "jpeg", "png", "gif", "bmp", "webp", "tiff", "tif"];
        if image_formats.contains(&ext.as_str()) {
            let full_path = config.upload_dir.join(&file.stored_path);
            if tokio::fs::try_exists(&full_path).await.unwrap_or(false) {
                let file_handle = tokio::fs::File::open(&full_path).await?;
                let stream = ReaderStream::new(file_handle);
                let body = Body::from_stream(stream);
                let (content_type, disposition) = inline_response_policy(&file.original_name);
                return harden(
                    Response::builder()
                        .status(StatusCode::OK)
                        .header(header::CONTENT_TYPE, content_type)
                        .header(
                            header::CONTENT_DISPOSITION,
                            content_disposition(disposition, &file.original_name),
                        ),
                )
                .body(body)
                .map_err(|_| AppError::Internal("响应构建失败".into()));
            }
        }
        return Err(AppError::NotFound("预览文件不存在".into()));
    }

    // 提供原始文件。只有扩展名白名单内的类型才允许 inline 预览，
    // 其余一律强制下载——这是防「上传内容变成本源可执行文档」的落点。
    let full_path = config.upload_dir.join(&file.stored_path);
    if !tokio::fs::try_exists(&full_path).await.unwrap_or(false) {
        return Err(AppError::NotFound("文件不存在".into()));
    }

    let file_handle = tokio::fs::File::open(&full_path).await?;
    let stream = ReaderStream::new(file_handle);
    let body = Body::from_stream(stream);

    // 注意：策略只依赖 original_name 的**扩展名判断**，不依赖它推导出的 MIME 可信；
    // 白名单外的类型连 MIME 都不会采信，直接给 octet-stream。
    let (content_type, disposition) = inline_response_policy(&file.original_name);

    let mut builder = harden(
        Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, content_type)
            .header(
                header::CONTENT_DISPOSITION,
                content_disposition(disposition, &file.original_name),
            ),
    );

    // 降级为下载时再补一道 CSP 沙箱作为纵深防御：即便将来有人误把
    // 用户内容声明成 HTML，该响应也不会执行脚本、拿不到源站存储。
    if disposition == "attachment" {
        builder = builder.header("content-security-policy", "sandbox; default-src 'none'");
    }

    builder
        .body(body)
        .map_err(|_| AppError::Internal("响应构建失败".into()))
}

/// DELETE /api/files/:id 删除文件（软删除，移入回收站）
pub async fn delete_file(
    State(pool): State<SqlitePool>,
    auth: AuthUser,
    Path(file_id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    file_service::soft_delete_file(&pool, file_id, auth.user_id).await?;
    Ok(Json(json!({
        "success": true,
        "data": null,
        "error": null
    })))
}

/// PUT /api/files/:id/rename 重命名文件
pub async fn rename_file(
    State(pool): State<SqlitePool>,
    auth: AuthUser,
    Path(file_id): Path<i64>,
    Json(req): Json<RenameRequest>,
) -> Result<Json<Value>, AppError> {
    let file = file_service::rename_file(&pool, file_id, auth.user_id, &req.name).await?;
    Ok(Json(json!({
        "success": true,
        "data": file.to_info(),
        "error": null
    })))
}

/// POST /api/files/:id/restore 从回收站恢复文件
pub async fn restore_file(
    State(pool): State<SqlitePool>,
    auth: AuthUser,
    Path(file_id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    file_service::restore_file(&pool, file_id, auth.user_id).await?;
    Ok(Json(json!({
        "success": true,
        "data": null,
        "error": null
    })))
}

/// DELETE /api/files/:id/permanent 永久删除文件（从回收站）
pub async fn permanent_delete_file(
    State(pool): State<SqlitePool>,
    auth: AuthUser,
    Path(file_id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    file_service::delete_file(&pool, file_id, auth.user_id).await?;
    Ok(Json(json!({
        "success": true,
        "data": null,
        "error": null
    })))
}

/// GET /api/trash 获取回收站内容（文件+文件夹）
/// 回收站列表的分页参数。与 `FileListQuery` 分开定义是因为这里没有 folder_id
/// —— 回收站是跨目录的扁平列表，多一个用不上的字段只会让调用方困惑。
#[derive(Debug, Deserialize)]
pub struct TrashListQuery {
    pub limit: Option<i64>,
    pub cursor: Option<String>,
}

/// GET /api/trash?limit={n}&cursor={c} 获取回收站（游标分页）
///
/// 文件分页，文件夹不分页（理由见 `folder_service::count_trash_folders`）。
/// `total_files` / `total_folders` 分开给，因为前端「共 N 项」要把两者相加，
/// 而分页只作用于文件。
pub async fn list_trash(
    State(pool): State<SqlitePool>,
    auth: AuthUser,
    Query(query): Query<TrashListQuery>,
) -> Result<Json<Value>, AppError> {
    let limit = pagination::normalize_limit(query.limit);
    let cursor =
        pagination::parse_cursor(query.cursor.as_deref(), file_service::SORT_DELETED_AT)?;
    let files = file_service::list_trash_files(&pool, auth.user_id, limit, cursor).await?;
    let folders = crate::services::folder_service::list_trash_folders(&pool, auth.user_id).await?;
    let total_folders =
        crate::services::folder_service::count_trash_folders(&pool, auth.user_id).await?;

    Ok(Json(json!({
        "success": true,
        "data": {
            "files": files.items,
            "folders": folders,
            "total_files": files.total,
            "total_folders": total_folders,
            "has_more": files.has_more(),
            "next_cursor": files.next_cursor,
            "limit": limit,
        },
        "error": null
    })))
}

/// DELETE /api/trash 清空回收站
pub async fn empty_trash(
    State(pool): State<SqlitePool>,
    State(config): State<Config>,
    // 提取器仍需声明（axum 靠它注入 AppState），但孤儿清理不涉及解码，
    // 用不到信号量，所以命名加下划线表示「有意不使用」
    _sem: State<Arc<Semaphore>>,
    auth: AuthUser,
) -> Result<Json<Value>, AppError> {
    let file_count = file_service::empty_trash(&pool, auth.user_id).await?;

    // 触发一次即时 GC，立即释放本次硬删产生的磁盘空间（无需等周期任务）
    tokio::spawn(async move {
        // 只跑孤儿对账，不重投缩略图——用户刚删完的磁盘空间要立刻释放
        if let Err(e) = crate::services::sweeper::cleanup_orphans(&pool, &config).await {
            tracing::warn!("清空回收站后的即时清理失败: {:?}", e);
        }
    });

    Ok(Json(json!({
        "success": true,
        "data": {
            "deleted_count": file_count,
        },
        "error": null
    })))
}

#[derive(Debug, Deserialize)]
pub struct RenameRequest {
    pub name: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 回归测试：这条链路此前构成存储型 XSS
    /// （上传 x.jpg（内容为 HTML）→ 改名为 x.html → serve_media 以内联 text/html 返回）。
    #[test]
    fn inline_policy_forces_attachment_for_renderable_types() {
        for name in [
            "x.html", "x.HTML", "x.xhtml", "x.svg", "x.js", "x.txt", "x.zip", "x.xml", "noext",
        ] {
            let (ct, disp) = inline_response_policy(name);
            assert_eq!(disp, "attachment", "{} 必须降级为下载", name);
            assert_eq!(
                ct, "application/octet-stream",
                "{} 不该采信推导出的 MIME",
                name
            );
        }
    }

    #[test]
    fn inline_policy_allows_whitelisted_media() {
        assert_eq!(
            inline_response_policy("a.jpg"),
            ("image/jpeg".to_string(), "inline")
        );
        assert_eq!(
            inline_response_policy("a.PNG"),
            ("image/png".to_string(), "inline")
        );
        assert_eq!(
            inline_response_policy("a.mp4"),
            ("video/mp4".to_string(), "inline")
        );
        assert_eq!(
            inline_response_policy("a.pdf"),
            ("application/pdf".to_string(), "inline")
        );
        // PDF 是唯一允许内联的非媒体类型，FilePreview 依赖它做 PDF 预览
        assert_eq!(inline_response_policy("a.nef").1, "inline");
    }

    /// 白名单内的图片即使改名成 jpg，保护来自「MIME + nosniff」而非白名单本身：
    /// 浏览器会按 image/jpeg 解码失败，不会当作 HTML 执行。
    #[test]
    fn inline_policy_keeps_images_inline() {
        let (ct, disp) = inline_response_policy("evil.jpg");
        assert_eq!(disp, "inline");
        assert_eq!(ct, "image/jpeg");
    }

    #[test]
    fn content_disposition_uses_rfc5987_for_non_ascii() {
        let v = content_disposition("attachment", "婚礼精修.jpg");
        assert!(v.starts_with("attachment; filename=\""), "{}", v);
        // ASCII 回退：非 ASCII 逐字符替换为下划线
        assert!(v.contains("filename=\"____.jpg\""), "{}", v);
        // RFC 5987 形式保留完整信息
        assert!(v.contains("filename*=UTF-8''%E5%A9%9A%E7%A4%BC%E7%B2%BE%E4%BF%AE.jpg"), "{}", v);
    }

    #[test]
    fn content_disposition_keeps_ascii_names_intact() {
        let v = content_disposition("inline", "DSC_0001.NEF");
        assert!(v.contains("filename=\"DSC_0001.NEF\""), "{}", v);
        assert!(v.contains("filename*=UTF-8''DSC_0001.NEF"), "{}", v);
    }

    #[test]
    fn percent_encode_utf8_escapes_reserved_bytes() {
        assert_eq!(percent_encode_utf8("abc-_.~"), "abc-_.~");
        assert_eq!(percent_encode_utf8("a b"), "a%20b");
        assert_eq!(percent_encode_utf8("a\"b"), "a%22b");
        assert_eq!(percent_encode_utf8("婚"), "%E5%A9%9A");
    }
}
