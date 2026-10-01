use std::net::SocketAddr;

use axum::{
    body::Body,
    extract::{ConnectInfo, Path, Query, State},
    http::{header, StatusCode},
    response::Response,
    Json,
};
use tokio_util::io::ReaderStream;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::config::Config;
use crate::errors::AppError;
use crate::middleware::auth::AuthUser;
use crate::models::share::{ItemKind, ShareItemInfo, ShareItemRef};
use crate::services::share_service;
use crate::services::file_service;
use crate::handlers::files;
use crate::utils::login_throttle;
use crate::utils::crypto;
use sqlx::SqlitePool;

/// 受密码保护分享的访问凭证有效期（秒）。默认 2 小时。
const SHARE_TICKET_TTL_SECS: i64 = 2 * 60 * 60;

/// 一个批次最多装多少条目。与批量操作的上限一致。
pub const MAX_SHARE_ITEMS: usize = crate::services::batch_service::MAX_BATCH_SIZE;

/// 从查询参数中提取并校验分享访问凭证。
fn check_share_ticket(config: &Config, share_id: &str, params: &std::collections::HashMap<String, String>) -> bool {
    match params.get("ticket") {
        Some(t) => crypto::verify_share_ticket(config, share_id, t),
        None => false,
    }
}

/// 公开接口的统一门禁：验证分享 + 密码凭证，返回 `(share, unlocked)`。
///
/// 抽出来是因为公开侧现在有四个接口（详情、浏览、下载、媒体），每个都要
/// 走同一条判断。写四遍的话，迟早有一处漏掉密码校验——而那一处就是
/// 「知道链接即可绕过密码」。
async fn open_share(
    pool: &SqlitePool,
    config: &Config,
    share_id: &str,
    params: &std::collections::HashMap<String, String>,
) -> Result<(crate::models::share::FileShare, bool), AppError> {
    let share = share_service::validate_share(pool, share_id).await?;
    let requires_password = !share.password_hash.is_empty();
    let unlocked = !requires_password || check_share_ticket(config, share_id, params);
    Ok((share, unlocked))
}

/// 受密码保护的分享，没带有效票据就**拒绝系统内容**。
///
/// `open_share` 只算出 `unlocked` 而不自己拦截，是有意的：详情接口在未解锁时
/// 仍要返回分享的元信息（叫什么、有几个文件、要不要输密码），只是不下发媒体
/// URL；而下载、媒体、浏览内容必须直接拒绝。两种行为不该揉进一个函数，
/// 否则「详情页也 401」会让前端连密码框都显示不出来。
///
/// **但下载/媒体/浏览这三条路径必须显式调用它。** 拆开之后，漏掉一次调用
/// 就是「知道链接即可绕过密码直接拿原片」——原来的代码把校验写在每个接口里，
/// 重构时极容易漏，这条注释就是给下一次改动的人看的。
fn require_unlocked(share: &crate::models::share::FileShare, unlocked: bool) -> Result<(), AppError> {
    if !share.password_hash.is_empty() && !unlocked {
        return Err(AppError::Unauthorized("需要访问密码".into()));
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct CreateShareRequest {
    /// 批次内容。文件与文件夹可以混装。
    pub items: Vec<ShareItemRef>,
    pub expires_hours: Option<i64>,
    pub password: Option<String>,
    pub max_downloads: Option<i64>,
    pub custom_code: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct VerifyShareRequest {
    pub password: String,
}

/// 公开侧浏览用的查询参数。
#[derive(Debug, Deserialize)]
pub struct PublicBrowseQuery {
    /// 省略 = 批次根（列出批次里直接包含的条目）
    pub folder_id: Option<i64>,
    pub limit: Option<i64>,
    pub cursor: Option<String>,
    /// 受密码保护分享的访问凭证
    pub ticket: Option<String>,
}

// ========== 需要认证的分享接口 ==========

/// 把请求里的明文口令哈希成入库形态（None / 空串 = 无密码）。
///
/// 单独抽出来是为了让**批量分享只哈希一次**：同一个口令给一整个批次用，
/// 逐个条目哈希等于把 250ms 放大 N 倍（见 `hash_password_async` 的说明）。
pub(crate) async fn hash_share_password(
    password: Option<&str>,
) -> Result<Option<String>, AppError> {
    match password {
        Some(p) if !p.is_empty() => Ok(Some(crypto::hash_password_async(p).await?)),
        _ => Ok(None),
    }
}

/// POST /api/shares 创建分享批次
///
/// 收的是 `items` 而不是单个 `file_id`：一个分享就是一个批次，里面可以同时
/// 装多个文件和多个文件夹。历史上这里收 `file_id` / `folder_id` 两个互斥字段，
/// 一个分享只能指向一个目标。
pub async fn create_share(
    State(pool): State<SqlitePool>,
    auth: AuthUser,
    Json(req): Json<CreateShareRequest>,
) -> Result<Json<Value>, AppError> {
    if req.items.is_empty() {
        return Err(AppError::BadRequest("请至少选择一个文件或文件夹".into()));
    }
    if req.items.len() > MAX_SHARE_ITEMS {
        return Err(AppError::BadRequest(format!(
            "单次最多分享 {} 项，当前 {} 项",
            MAX_SHARE_ITEMS,
            req.items.len()
        )));
    }

    // 先把类型字符串收口成枚举，非法值直接拒绝而不是静默跳过——
    // 「我选了 10 项，分享里只有 8 项」是最难查的一类问题。
    let mut items: Vec<(ItemKind, i64)> = Vec::with_capacity(req.items.len());
    for it in &req.items {
        let kind = it
            .kind()
            .ok_or_else(|| AppError::BadRequest("条目类型必须是 file 或 folder".into()))?;
        items.push((kind, it.id));
    }

    let password_hash = hash_share_password(req.password.as_deref()).await?;

    let share = share_service::create_share(
        &pool,
        &items,
        auth.user_id,
        req.expires_hours,
        password_hash,
        req.max_downloads,
        req.custom_code,
    )
    .await?;

    let info = share_service::get_share(&pool, &share.id, auth.user_id).await?;

    Ok(Json(json!({
        "success": true,
        "data": info,
        "error": null
    })))
}

/// GET /api/shares 获取分享列表
pub async fn list_shares(
    State(pool): State<SqlitePool>,
    auth: AuthUser,
) -> Result<Json<Value>, AppError> {
    let shares = share_service::list_shares(&pool, auth.user_id).await?;
    Ok(Json(json!({
        "success": true,
        "data": shares,
        "error": null
    })))
}

/// GET /api/shares/:id 获取分享详情
pub async fn get_share(
    State(pool): State<SqlitePool>,
    auth: AuthUser,
    Path(share_id): Path<String>,
) -> Result<Json<Value>, AppError> {
    let share = share_service::get_share(&pool, &share_id, auth.user_id).await?;
    Ok(Json(json!({
        "success": true,
        "data": share,
        "error": null
    })))
}

/// DELETE /api/shares/:id 删除分享
pub async fn delete_share(
    State(pool): State<SqlitePool>,
    auth: AuthUser,
    Path(share_id): Path<String>,
) -> Result<Json<Value>, AppError> {
    share_service::delete_share(&pool, &share_id, auth.user_id).await?;
    Ok(Json(json!({
        "success": true,
        "data": null,
        "error": null
    })))
}

// ========== 公开分享接口（无需认证） ==========

/// GET /api/public/shares/:id 访问公开分享
///
/// `?ticket=` 用于受密码保护的分享：访客在 `/verify` 拿到票据后，前端会带着它
/// 重新拉一次详情——只有带了有效票据，响应里才会出现媒体 URL。
///
/// **这是「验码后重新 load」这条前端流程真正的落点。** 此前该判断写成了
/// 「分享有密码 → 不下发 URL」，与票据无关，于是受密码保护的分享无论用户
/// 输没输对密码，预览区永远是空的。
pub async fn public_share_access(
    State(pool): State<SqlitePool>,
    State(config): State<Config>,
    Path(share_id): Path<String>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<Value>, AppError> {
    let (_share, unlocked) = open_share(&pool, &config, &share_id, &params).await?;
    let info = share_service::get_public_share(&pool, &share_id, unlocked).await?;
    Ok(Json(json!({
        "success": true,
        "data": info,
        "error": null
    })))
}

/// POST /api/public/shares/:id/verify 验证分享密码并签发访问凭证
///
/// **限流的必要性**：本接口**无鉴权**（拿到分享链接即可调用），而每次调用
/// 都会执行一次 bcrypt 校验（`DEFAULT_COST=12`，约 250ms CPU）。没有限流时
/// 攻击者可以拿任意一个受密码保护的分享码无限次爆破口令，同时把服务端 CPU
/// 打满——典型的计算放大 DoS。实测：12 次错误口令串行耗时 22.9 秒，
/// 且全程无任何 429。
///
/// 复用 `login_throttle` 的退避机制（5 次失败后指数退避、封顶 15 分钟），
/// 但**只启用分享码这一维度**、不带 IP——理由见函数内注释。
/// 检查放在 bcrypt **之前**，让爆破在付出哈希成本之前就被挡下：
/// 实测被限流后响应从约 250ms 降到 2ms。
pub async fn public_verify_password(
    State(pool): State<SqlitePool>,
    State(config): State<Config>,
    client: Option<ConnectInfo<SocketAddr>>,
    Path(share_id): Path<String>,
    Json(req): Json<VerifyShareRequest>,
) -> Result<Json<Value>, AppError> {
    let ip = client
        .map(|ConnectInfo(a)| a.ip().to_string())
        .unwrap_or_default();

    // **只按分享码维度计数，不带 IP。**
    //
    // 登录限流是「用户名 + IP」双维度，但这里必须去掉 IP 维度：
    // 访客与攻击者通常来自同一个出口 IP（家庭宽带 / Cloudflare Tunnel 后
    // 更是有大量请求来自少数几个 IP）。若带 IP 维度，一次针对 A 分享码的
    // 爆破会让**同 IP 下所有访客都无法访问任何受密码保护的分享**——
    // 攻击者只需打满一个码，就能把该出口的全部客户锁在门外。
    //
    // 只按分享码计数的残余风险是「换码重置」，但分享码是 122 bit 熵的
    // uuid4（除非摄影师自定义了可枚举的 custom_code），攻击者手上没有
    // 别的码可换；真正的防线是「码本身不可猜」，限流只是防住
    // 「已经拿到码的人在猜口令」这一种情况。
    let _ = &ip;
    if let Some(wait) = login_throttle::check(&share_id, "") {
        return Err(AppError::TooManyRequests(format!(
            "口令尝试次数过多，请在 {} 秒后重试",
            wait
        )));
    }

    let valid = share_service::verify_share_password(&pool, &share_id, &req.password).await?;

    if valid {
        // 口令正确：清空该分享的失败计数，并签发短时效访问凭证
        login_throttle::record_success(&share_id, "");
        let ticket = crypto::create_share_ticket(&config, &share_id, SHARE_TICKET_TTL_SECS);
        Ok(Json(json!({
            "success": true,
            "data": { "verified": true, "ticket": ticket },
            "error": null
        })))
    } else {
        login_throttle::record_failure(&share_id, "");
        Err(AppError::Unauthorized("密码错误".into()))
    }
}

/// GET /api/public/shares/:id/items 浏览批次内容
///
/// 不传 `folder_id` → 列出批次里直接包含的条目（根）。
/// 传 `folder_id` → 列出该文件夹的直接子级，**前提是它落在批次授权范围内**。
///
/// 授权判定见 `share_service::folder_in_share`。这一条不能省：不做校验的话，
/// 拿到任意一个分享链接就等于拿到了该账号的全部文件夹。
pub async fn public_share_items(
    State(pool): State<SqlitePool>,
    State(config): State<Config>,
    Path(share_id): Path<String>,
    Query(query): Query<PublicBrowseQuery>,
) -> Result<Json<Value>, AppError> {
    let params = std::collections::HashMap::from_iter(
        query
            .ticket
            .clone()
            .map(|t| ("ticket".to_string(), t)),
    );
    let (share, unlocked) = open_share(&pool, &config, &share_id, &params).await?;
    // 受密码保护时，**连目录结构都不给看**——否则不输密码就能遍历出
    // 这一批里有哪些文件夹、叫什么名字。
    require_unlocked(&share, unlocked)?;

    let info = share_service::get_public_share(&pool, &share_id, unlocked).await?;
    let owner_id = info.owner_id;

    // ---- 根：批次里直接包含的条目 ----
    //
    // 不分页：批次的条目数在创建时就被限制在 MAX_SHARE_ITEMS，一次给完
    // 反而比给前端配一套游标状态机更省事，也不会出现「根目录还要加载更多」。
    let Some(folder_id) = query.folder_id else {
        return Ok(Json(json!({
            "success": true,
            "data": {
                "folder_id": null,
                "breadcrumbs": [],
                "items": info.items,
                "total": info.items.len(),
                "has_more": false,
                "next_cursor": null,
            },
            "error": null
        })));
    };

    // ---- 子目录：先过授权 ----
    if !share_service::folder_in_share(&pool, &share_id, owner_id, folder_id).await? {
        // 与「文件夹不存在」返回同一个 404，不区分二者——
        // 区分开就成了「探测某个 folder_id 是否属于该账号」的旁路。
        return Err(AppError::NotFound("文件夹不存在".into()));
    }

    let limit = crate::utils::pagination::normalize_limit(query.limit);
    let cursor = crate::utils::pagination::parse_cursor(
        query.cursor.as_deref(),
        file_service::SORT_UPLOADED_AT,
    )?;

    // 复用主列表那两套查询。传的是**分享所有者的** owner_id，而上一行的
    // 子树的根就是这个 owner 的文件夹，归属校验由 `folder_in_share` 完成。
    let page = file_service::list_files(&pool, owner_id, Some(folder_id), limit, cursor).await?;
    let folders = crate::services::folder_service::list_folders(&pool, owner_id, Some(folder_id))
        .await?;

    // 先把分页字段取出来，再消费 `page.items`——`has_more()` 借的是
    // `next_cursor`，而 `into_iter()` 会把整个结构体移走。
    let total = page.total;
    let has_more = page.has_more();
    let next_cursor = page.next_cursor;

    let mut items: Vec<ShareItemInfo> = folders
        .into_iter()
        .map(|f| folder_item(&share_id, f))
        .collect();
    items.extend(page.items.into_iter().map(|f| file_item(&share_id, f, unlocked)));

    let crumbs = share_service::breadcrumbs_from_share(&pool, &share_id, owner_id, folder_id).await?;

    Ok(Json(json!({
        "success": true,
        "data": {
            "folder_id": folder_id,
            "breadcrumbs": crumbs,
            "items": items,
            "total": total,
            "has_more": has_more,
            "next_cursor": next_cursor,
        },
        "error": null
    })))
}

/// 把主列表的文件夹转成批次条目。计数沿用 `FolderInfo` 已有的统计。
fn folder_item(share_id: &str, f: crate::models::folder::FolderInfo) -> ShareItemInfo {
    let _ = share_id;
    ShareItemInfo {
        item_type: "folder".to_string(),
        id: f.id,
        name: f.name,
        size: 0,
        formatted_size: crate::models::file::format_file_size(0),
        file_type: "folder".to_string(),
        uploaded_at: None,
        has_preview: false,
        preview_url: None,
        thumb_url: None,
        download_url: None,
        file_count: Some(f.file_count),
        subfolder_count: Some(f.subfolder_count),
    }
}

/// 把主列表的文件转成批次条目，媒体地址指向**公开分享**的接口。
///
/// 注意这里用的是 `/api/public/shares/...` 而不是 `/api/files/...`：
/// 客户端没有登录态，`/api/files/*` 对它一律 401。
fn file_item(
    share_id: &str,
    f: crate::models::file::FileInfo,
    unlocked: bool,
) -> ShareItemInfo {
    let ext = file_service::extension_of(&f.original_name);
    let inline_safe = file_service::is_inline_safe(&ext);

    let (preview_url, thumb_url) = if !unlocked || (!f.has_preview && !inline_safe) {
        (None, None)
    } else {
        let preview = Some(format!(
            "/api/public/shares/{}/media?file_id={}&preview=1",
            share_id, f.id
        ));
        let thumb = if f.thumb_url.is_some() || f.has_preview {
            Some(format!(
                "/api/public/shares/{}/media?file_id={}&thumb=1",
                share_id, f.id
            ))
        } else {
            preview.clone()
        };
        (preview, thumb)
    };

    ShareItemInfo {
        item_type: "file".to_string(),
        id: f.id,
        name: f.original_name.clone(),
        size: f.size,
        formatted_size: f.formatted_size,
        file_type: f.file_type,
        uploaded_at: Some(f.uploaded_at),
        has_preview: f.has_preview,
        preview_url,
        thumb_url,
        download_url: Some(format!(
            "/api/public/shares/{}/download?file_id={}",
            share_id, f.id
        )),
        file_count: None,
        subfolder_count: None,
    }
}

/// GET /api/public/shares/:id/download?file_id=N 下载公开分享里的某个文件
pub async fn public_share_download(
    State(pool): State<SqlitePool>,
    State(config): State<Config>,
    Path(share_id): Path<String>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Result<Response, AppError> {
    let (share, unlocked) = open_share(&pool, &config, &share_id, &params).await?;
    require_unlocked(&share, unlocked)?;

    let file_id = require_file_id(&params)?;

    // **授权**：这个文件必须真的在批次范围内。少了这一步，随便改一下 URL 里的
    // file_id 就能把该账号的任意文件下载下来——批次分享会直接变成全账号下载器。
    if !share_service::file_in_share(&pool, &share_id, share.owner_id, file_id).await? {
        return Err(AppError::NotFound("文件不存在".into()));
    }

    let file = file_service::get_file_by_id(&pool, file_id).await?;
    let full_path = config.upload_dir.join(&file.stored_path);

    if !tokio::fs::try_exists(&full_path).await.unwrap_or(false) {
        return Err(AppError::NotFound("文件不存在".into()));
    }

    // 占用下载额度放在**文件确认可读之后、开流之前**。
    //
    // 原实现在开流之前就自增，且与 validate_share 的检查分属两个事务：
    //   · 并发下所有请求会同时读到「还有额度」而全部放行，max_downloads 被超发；
    //   · 客户端中途取消也会消耗掉一次额度（那时字节还没传出去多少）。
    // 现在 consume_download_slot 把「检查+占用」收进同一条 UPDATE，由数据库裁决，
    // 天然并发安全；而放在 try_exists 之后，文件真不存在时不会白白扣掉额度。
    //
    // 批次模型下按**文件**扣：下一个文件扣一次。`max_downloads = 10` 的含义
    // 是「客户最多能下载 10 个文件」。
    share_service::consume_download_slot(&pool, &share_id).await?;

    let file_handle = tokio::fs::File::open(&full_path).await?;
    let file_size = file_handle.metadata().await.map(|m| m.len()).unwrap_or(0);
    let stream = ReaderStream::new(file_handle);
    let body = Body::from_stream(stream);
    let mime = mime_guess::from_path(&file.original_name)
        .first_or_octet_stream();

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, mime.as_ref())
        // 走 files 的统一构造：同时给 ASCII 回退与 RFC 5987 的 filename*，
        // 否则中文文件名在部分浏览器下会乱码或被截断。
        .header(
            header::CONTENT_DISPOSITION,
            files::content_disposition("attachment", &file.original_name),
        )
        // 让前端能拿到 total 走真实进度
        .header(header::CONTENT_LENGTH, file_size)
        .body(body)
        .map_err(|_| AppError::Internal("响应构建失败".into()))?)
}

/// GET /api/public/shares/:id/media?file_id=N&thumb=1&preview=1 公开分享的媒体
pub async fn public_share_media(
    State(pool): State<SqlitePool>,
    State(config): State<Config>,
    Path(share_id): Path<String>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Result<Response, AppError> {
    let (share, unlocked) = open_share(&pool, &config, &share_id, &params).await?;
    require_unlocked(&share, unlocked)?;

    let file_id = require_file_id(&params)?;

    // 与下载同一条授权判定。写宽一点就等于把整个账号暴露出去。
    if !share_service::file_in_share(&pool, &share_id, share.owner_id, file_id).await? {
        return Err(AppError::NotFound("文件不存在".into()));
    }

    let file = file_service::get_file_by_id(&pool, file_id).await?;

    let is_thumb = params.get("thumb").map(|v| v.as_str()) == Some("1");
    let is_preview = params.get("preview").map(|v| v.as_str()) == Some("1");

    // 响应头策略**以用户的原始文件名为准**，与认证后的 serve_media 完全一致。
    //
    // 之前这里用 serve_path 推导 MIME，且不设 Content-Disposition（等价于
    // inline 渲染），而 serve_path 在回退分支里是 stored_path —— 后缀来自
    // 用户可控的 original_name。后果是：上传一张内容为 HTML 的 .jpg，
    // 改名成 .xml / .txt 之类未被黑名单覆盖的后缀后创建公开分享，
    // 访客访问 /media 会拿到 Content-Type: text/xml + 内联渲染。
    // nosniff 拦不住这种情况（text/xml 是精确 MIME），而这是**无鉴权接口**。
    //
    // 真正的边界是 files::is_inline_safe 白名单：不在表内的一律降级为
    // attachment + application/octet-stream，浏览器只落盘、不渲染。
    let (content_type, disposition) = files::inline_response_policy(&file.original_name);

    // 确定要提供哪个文件，以及**提供的是不是原片**。
    let serve_path = if is_thumb {
        file.thumb_path.as_ref().or(file.preview_path.as_ref())
    } else if is_preview {
        file.preview_path.as_ref()
    } else {
        // 默认：优先提供预览图，否则使用原始文件
        file.preview_path.as_ref()
    };

    let (serve_path, serving_original) = match serve_path {
        Some(p) => (p, false),
        None => {
            // 没有预览图时回退到原文件。**先按白名单判断能不能内联**：
            // 不可内联的类型直接拒绝，而不是返回一个必然被降级下载的响应。
            if disposition == "attachment" {
                return Err(AppError::NotFound("预览不可用".into()));
            }
            (&file.stored_path, true)
        }
    };

    let full_path = config.upload_dir.join(serve_path);
    if !tokio::fs::try_exists(&full_path).await.unwrap_or(false) {
        return Err(AppError::NotFound("预览文件不存在".into()));
    }

    // **回退到原片时也要扣额度。**
    //
    // 原先只有 `/download` 会占用额度，`/media` 全程不计数；而只要
    // `preview_path` 为空（刚上传还没生成、RAW 抽帧失败、或视频/PDF 这类
    // 本就不支持预览的类型），这条回退分支就会把**完整原片**吐出去。
    // 于是一个设了「最多下载 1 次」的分享，任何拿到链接的人都能通过
    // `/media` 无限次拿走原图，而 `download_count` 始终是 0。
    if serving_original {
        share_service::consume_download_slot(&pool, &share_id).await?;
    }

    let file_handle = tokio::fs::File::open(&full_path).await?;
    let stream = ReaderStream::new(file_handle);
    let body = Body::from_stream(stream);

    let builder = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(
            header::CONTENT_DISPOSITION,
            files::content_disposition(disposition, &file.original_name),
        )
        .header(header::CACHE_CONTROL, "public, max-age=3600");

    // 降级为下载的内容额外加 CSP 沙箱。与 serve_media 同一套理由：
    // attachment 响应浏览器不会渲染，但沙箱是第二道保险，且不影响
    // 白名单内类型的正常预览。
    let builder = if disposition == "attachment" {
        builder.header("content-security-policy", "sandbox; default-src 'none'")
    } else {
        builder
    };

    files::harden(builder)
        .body(body)
        .map_err(|_| AppError::Internal("响应构建失败".into()))
}

/// 公开侧必须显式指定 `file_id`——批次里没有「唯一的那个文件」了。
fn require_file_id(
    params: &std::collections::HashMap<String, String>,
) -> Result<i64, AppError> {
    params
        .get("file_id")
        .and_then(|v| v.parse::<i64>().ok())
        .ok_or_else(|| AppError::BadRequest("缺少 file_id 参数".into()))
}
