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
}

/// GET /api/search?q={query}&type={file_type}
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

    // Search files
    let files = if let Some(ref ft) = query.file_type {
        if ft.is_empty() {
            sqlx::query_as::<_, File>(
                "SELECT * FROM files WHERE owner_id = ? AND (name LIKE ? OR original_name LIKE ?) ORDER BY uploaded_at DESC",
            )
            .bind(auth.user_id)
            .bind(&search_term)
            .bind(&search_term)
            .fetch_all(&pool)
            .await?
        } else {
            sqlx::query_as::<_, File>(
                "SELECT * FROM files WHERE owner_id = ? AND file_type = ? AND (name LIKE ? OR original_name LIKE ?) ORDER BY uploaded_at DESC",
            )
            .bind(auth.user_id)
            .bind(ft)
            .bind(&search_term)
            .bind(&search_term)
            .fetch_all(&pool)
            .await?
        }
    } else {
        sqlx::query_as::<_, File>(
            "SELECT * FROM files WHERE owner_id = ? AND (name LIKE ? OR original_name LIKE ?) ORDER BY uploaded_at DESC",
        )
        .bind(auth.user_id)
        .bind(&search_term)
        .bind(&search_term)
        .fetch_all(&pool)
        .await?
    };

    // Search folders
    let folders = sqlx::query_as::<_, Folder>(
        "SELECT * FROM folders WHERE owner_id = ? AND name LIKE ? ORDER BY name",
    )
    .bind(auth.user_id)
    .bind(&search_term)
    .fetch_all(&pool)
    .await?;

    // Get distinct file types for filter
    let file_types: Vec<(String,)> = sqlx::query_as(
        "SELECT DISTINCT file_type FROM files WHERE owner_id = ? ORDER BY file_type",
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