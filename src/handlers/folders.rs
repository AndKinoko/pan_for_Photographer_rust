use axum::{extract::{Path, Query, State}, Json};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::config::Config;
use crate::errors::AppError;
use crate::middleware::auth::AuthUser;
use crate::services::folder_service;
use sqlx::SqlitePool;

#[derive(Debug, Deserialize)]
pub struct FolderListQuery {
    pub parent_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct CreateFolderRequest {
    pub name: String,
    pub parent_id: Option<i64>,
}

/// GET /api/folders?parent_id={id}
pub async fn list_folders(
    State(pool): State<SqlitePool>,
    auth: AuthUser,
    Query(query): Query<FolderListQuery>,
) -> Result<Json<Value>, AppError> {
    let folders = folder_service::list_folders(&pool, auth.user_id, query.parent_id).await?;

    // Get breadcrumbs if parent_id is specified
    let breadcrumbs = if let Some(pid) = query.parent_id {
        folder_service::get_breadcrumbs(&pool, pid).await.unwrap_or_default()
    } else {
        Vec::new()
    };

    Ok(Json(json!({
        "success": true,
        "data": {
            "folders": folders,
            "breadcrumbs": breadcrumbs,
        },
        "error": null
    })))
}

/// POST /api/folders
pub async fn create_folder(
    State(pool): State<SqlitePool>,
    auth: AuthUser,
    Json(req): Json<CreateFolderRequest>,
) -> Result<Json<Value>, AppError> {
    if req.name.trim().is_empty() {
        return Err(AppError::BadRequest("文件夹名称不能为空".into()));
    }

    let folder = folder_service::create_folder(
        &pool,
        auth.user_id,
        req.name.trim(),
        req.parent_id,
    )
    .await?;

    Ok(Json(json!({
        "success": true,
        "data": folder,
        "error": null
    })))
}

/// DELETE /api/folders/:id
pub async fn delete_folder(
    State(pool): State<SqlitePool>,
    State(config): State<Config>,
    auth: AuthUser,
    Path(folder_id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    folder_service::delete_folder(&pool, &config, folder_id, auth.user_id).await?;
    Ok(Json(json!({
        "success": true,
        "data": null,
        "error": null
    })))
}