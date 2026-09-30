use axum::{
    extract::{Query, State},
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::errors::AppError;
use crate::middleware::auth::AuthUser;
use crate::models::file::File;
use crate::utils::pagination::{self, Bind, Dir};
use sqlx::SqlitePool;

#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    pub q: String,
    #[serde(rename = "type")]
    pub file_type: Option<String>,
    /// 最小文件大小（字节）
    pub min_size: Option<i64>,
    /// 最大文件大小（字节）
    pub max_size: Option<i64>,
    /// 上传日期起始（YYYY-MM-DD）
    pub date_from: Option<String>,
    /// 上传日期结束（YYYY-MM-DD）
    pub date_to: Option<String>,
    /// 排序字段：name, size, uploaded_at
    #[serde(default = "default_sort")]
    pub sort: String,
    /// 排序方向：asc, desc
    #[serde(default = "default_order")]
    pub order: String,
    /// 每页条数。缺省 100，上限 500
    pub limit: Option<i64>,
    /// 上一页响应里的 `next_cursor`
    pub cursor: Option<String>,
}

fn default_sort() -> String {
    "uploaded_at".to_string()
}

fn default_order() -> String {
    "desc".to_string()
}

/// 空结果的统一响应。抽出来是为了让「关键词为空」这条提前返回的分支
/// 与正常分支的字段完全一致 —— 前端不必为它写第二套解析。
fn empty_response() -> Json<Value> {
    Json(json!({
        "success": true,
        "data": {
            "files": [],
            "folders": [],
            "file_types": [],
            "total_files": 0,
            "total_folders": 0,
            "has_more": false,
            "next_cursor": null,
            "limit": pagination::DEFAULT_LIMIT,
        },
        "error": null
    }))
}

/// GET /api/search?q={query}&type={file_type}&min_size={bytes}&max_size={bytes}&date_from={date}&date_to={date}&sort={field}&order={dir}&limit={n}&cursor={c}
///
/// 搜索文件和文件夹，支持组合筛选和排序，文件侧游标分页。
///
/// **排序标识含方向。** 游标里带的是 `{字段}_{方向}`（如 `uploaded_at_desc`），
/// 而不是只用字段名：`uploaded_at ASC` 与 `DESC` 的遍历方向相反，
/// 若只校验字段名，客户端把方向从降序改成升序后旧游标仍会通过校验，
/// 服务端就会按升序去比较一个降序取来的位置，返回一批位置错误的结果。
pub async fn search_files(
    State(pool): State<SqlitePool>,
    auth: AuthUser,
    Query(query): Query<SearchQuery>,
) -> Result<Json<Value>, AppError> {
    if query.q.trim().is_empty() {
        return Ok(empty_response());
    }

    let limit = pagination::normalize_limit(query.limit);

    // 排序字段白名单（防注入：绝不把 query.sort 直接拼进 SQL）
    let sort_field = match query.sort.as_str() {
        "name" => "name",
        "size" => "size",
        // 其余（含默认的 uploaded_at 与任何未知取值）一律按上传时间排序
        _ => "uploaded_at",
    };
    let dir = if query.order.eq_ignore_ascii_case("asc") {
        Dir::Asc
    } else {
        Dir::Desc
    };
    let sort_token = format!("{}_{}", sort_field, dir.sql().to_lowercase());
    let cursor = pagination::parse_cursor(query.cursor.as_deref(), &sort_token)?;

    let search_term = format!("%{}%", query.q.trim());

    // WHERE 片段与绑定值**只构建一次**，COUNT 与取数共用。
    // 两边各写一遍的话，一旦只改一处，total 与 items 就会对不上，
    // 表现为「共 300 张」但翻到底只有 200 张 —— 而且不会有任何报错。
    let mut where_sql = String::from(
        " FROM files WHERE owner_id = ? AND deleted_at IS NULL AND (name LIKE ? OR original_name LIKE ?)",
    );
    let mut binds: Vec<Bind> = Vec::new();

    if let Some(ref ft) = query.file_type {
        if !ft.is_empty() {
            where_sql.push_str(" AND file_type = ?");
            binds.push(Bind::Text(ft.clone()));
        }
    }
    // size 是 INTEGER 列，按整数绑定而不是字符串：
    // 结果虽然靠类型亲和通常也对，但那样比较用不上索引
    if let Some(min_sz) = query.min_size {
        where_sql.push_str(" AND size >= ?");
        binds.push(Bind::Int(min_sz));
    }
    if let Some(max_sz) = query.max_size {
        where_sql.push_str(" AND size <= ?");
        binds.push(Bind::Int(max_sz));
    }
    if let Some(ref df) = query.date_from {
        where_sql.push_str(" AND date(uploaded_at) >= date(?)");
        binds.push(Bind::Text(df.clone()));
    }
    if let Some(ref dt) = query.date_to {
        where_sql.push_str(" AND date(uploaded_at) <= date(?)");
        binds.push(Bind::Text(dt.clone()));
    }

    // ---- 总数 ----
    let count_sql = format!("SELECT COUNT(*){where_sql}");
    let mut cq = sqlx::query_scalar::<_, i64>(&count_sql)
        .bind(auth.user_id)
        .bind(&search_term)
        .bind(&search_term);
    for b in &binds {
        cq = match b {
            Bind::Text(s) => cq.bind(s.clone()),
            Bind::Int(i) => cq.bind(*i),
        };
    }
    let total_files = cq.fetch_one(&pool).await?;

    // ---- 取一页 ----
    let mut sql = format!("SELECT *{where_sql}");
    if cursor.is_some() {
        pagination::push_cursor_condition(&mut sql, sort_field, dir);
    }
    // order_by 会额外带上 `id {dir}`：排序键并列时的先后必须确定，
    // 否则 keyset 分页会漏项或重复（同秒上传的批量文件就是这个情况）
    sql.push_str(&pagination::order_by(sort_field, dir));
    sql.push_str(" LIMIT ?");

    let mut q = sqlx::query_as::<_, File>(&sql)
        .bind(auth.user_id)
        .bind(&search_term)
        .bind(&search_term);
    for b in &binds {
        q = match b {
            Bind::Text(s) => q.bind(s.clone()),
            Bind::Int(i) => q.bind(*i),
        };
    }
    if let Some(c) = &cursor {
        // 游标值的类型必须与排序键的列类型一致，否则比较走不到索引
        match sort_field {
            "size" => {
                let v = c.as_int()?;
                q = q.bind(v).bind(v).bind(c.id);
            }
            _ => {
                let v = c.as_text();
                q = q.bind(v.clone()).bind(v).bind(c.id);
            }
        }
    }
    let rows = q.bind(limit + 1).fetch_all(&pool).await?;

    let (rows, next_cursor) = pagination::split_page(rows, limit, &sort_token, |f| {
        let key = match sort_field {
            "size" => f.size.to_string(),
            "name" => f.name.clone(),
            _ => f.uploaded_at.clone(),
        };
        (key, f.id)
    });

    let file_infos: Vec<_> = rows.into_iter().map(|f| f.to_info()).collect();

    // 搜索文件夹（不分页，理由同 file_service 里的说明）
    // 同样带 file_count / subfolder_count——搜索结果里的文件夹复用
    // FileCard 组件渲染，缺了计数会退回显示创建时间。
    let folders = sqlx::query_as::<_, crate::models::folder::FolderInfo>(
        r#"
        SELECT f.id, f.name, f.owner_id, f.parent_id, f.created_at, f.updated_at, f.deleted_at,
               COALESCE(fc.cnt, 0) AS file_count,
               COALESCE(sc.cnt, 0) AS subfolder_count
        FROM folders f
        LEFT JOIN (
            SELECT folder_id, COUNT(*) AS cnt FROM files
            WHERE deleted_at IS NULL AND folder_id IS NOT NULL GROUP BY folder_id
        ) fc ON fc.folder_id = f.id
        LEFT JOIN (
            SELECT parent_id AS pid, COUNT(*) AS cnt FROM folders
            WHERE deleted_at IS NULL AND parent_id IS NOT NULL GROUP BY parent_id
        ) sc ON sc.pid = f.id
        WHERE f.owner_id = ? AND f.deleted_at IS NULL AND f.name LIKE ?
        ORDER BY f.name, f.id
        "#,
    )
    .bind(auth.user_id)
    .bind(&search_term)
    .fetch_all(&pool)
    .await?;
    let total_folders = folders.len() as i64;

    // 用于筛选下拉框的可选文件类型。
    // 每页都返回（而不是只在第一页给）：前端拿到空数组会把下拉框清空，
    // 「第二页之后筛选框突然没选项了」是个很难查的 bug，
    // 用一次廉价的 DISTINCT 扫描换掉这个风险是划算的。
    let file_types: Vec<(String,)> = sqlx::query_as(
        "SELECT DISTINCT file_type FROM files WHERE owner_id = ? AND deleted_at IS NULL ORDER BY file_type",
    )
    .bind(auth.user_id)
    .fetch_all(&pool)
    .await?;

    Ok(Json(json!({
        "success": true,
        "data": {
            "files": file_infos,
            "folders": folders,
            "file_types": file_types.into_iter().map(|(t,)| t).collect::<Vec<_>>(),
            "total_files": total_files,
            "total_folders": total_folders,
            "has_more": next_cursor.is_some(),
            "next_cursor": next_cursor,
            "limit": limit,
        },
        "error": null
    })))
}
