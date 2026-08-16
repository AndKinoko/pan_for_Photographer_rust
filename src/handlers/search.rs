use axum::{extract::{Query, State}, Json};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::errors::AppError;
use crate::middleware::auth::AuthUser;
use crate::models::file::File;
use crate::models::folder::Folder;
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
}

fn default_sort() -> String {
    "uploaded_at".to_string()
}

fn default_order() -> String {
    "desc".to_string()
}

/// GET /api/search?q={query}&type={file_type}&min_size={bytes}&max_size={bytes}&date_from={date}&date_to={date}&sort={field}&order={dir}
/// 搜索文件和文件夹，支持组合筛选和排序
pub async fn search_files(
    State(pool): State<SqlitePool>,
    auth: AuthUser,
    Query(query): Query<SearchQuery>,
) -> Result<Json<Value>, AppError> {
    if query.q.trim().is_empty() {
        return Ok(Json(json!({
            "success": true,
            "data": {
                "files": [],
                "folders": [],
                "file_types": [],
            },
            "error": null
        })));
    }

    let search_term = format!("%{}%", query.q.trim());

    // 构建排序子句（防注入：白名单校验）
    let sort_clause = match query.sort.as_str() {
        "name" => "name",
        "size" => "size",
        "uploaded_at" | _ => "uploaded_at",
    };
    let order_clause = if query.order.eq_ignore_ascii_case("asc") {
        "ASC"
    } else {
        "DESC"
    };

    // 构建文件搜索SQL（动态条件组合）
    let mut sql = String::from(
        "SELECT * FROM files WHERE owner_id = ? AND deleted_at IS NULL AND (name LIKE ? OR original_name LIKE ?)",
    );
    let mut binds: Vec<String> = Vec::new();

    if let Some(ref ft) = query.file_type {
        if !ft.is_empty() {
            sql.push_str(" AND file_type = ?");
            binds.push(ft.clone());
        }
    }
    if let Some(min_sz) = query.min_size {
        sql.push_str(" AND size >= ?");
        binds.push(min_sz.to_string());
    }
    if let Some(max_sz) = query.max_size {
        sql.push_str(" AND size <= ?");
        binds.push(max_sz.to_string());
    }
    if let Some(ref df) = query.date_from {
        sql.push_str(" AND date(uploaded_at) >= date(?)");
        binds.push(df.clone());
    }
    if let Some(ref dt) = query.date_to {
        sql.push_str(" AND date(uploaded_at) <= date(?)");
        binds.push(dt.clone());
    }

    sql.push_str(&format!(" ORDER BY {} {}", sort_clause, order_clause));

    // 执行文件搜索
    let mut q = sqlx::query_as::<_, File>(&sql)
        .bind(auth.user_id)
        .bind(&search_term)
        .bind(&search_term);

    for b in &binds {
        q = q.bind(b);
    }

    let files = q.fetch_all(&pool).await?;

    // 搜索文件夹
    let folders = sqlx::query_as::<_, Folder>(
        "SELECT * FROM folders WHERE owner_id = ? AND deleted_at IS NULL AND name LIKE ? ORDER BY name",
    )
    .bind(auth.user_id)
    .bind(&search_term)
    .fetch_all(&pool)
    .await?;

    // 获取不同的文件类型用于筛选
    let file_types: Vec<(String,)> = sqlx::query_as(
        "SELECT DISTINCT file_type FROM files WHERE owner_id = ? AND deleted_at IS NULL ORDER BY file_type",
    )
    .bind(auth.user_id)
    .fetch_all(&pool)
    .await?;

    let file_infos: Vec<_> = files.into_iter().map(|f| f.to_info()).collect();

    Ok(Json(json!({
        "success": true,
        "data": {
            "files": file_infos,
            "folders": folders,
            "file_types": file_types.into_iter().map(|(t,)| t).collect::<Vec<_>>(),
        },
        "error": null
    })))
}
