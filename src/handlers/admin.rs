use axum::{extract::{Path, State}, Json};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::errors::AppError;
use crate::middleware::admin::AdminUser;
use crate::models::user::{User, UserInfo};
use sqlx::SqlitePool;

#[derive(Debug, Deserialize)]
pub struct UpdateUserRoleRequest {
    pub role: String,
}

/// GET /api/admin/users 获取所有用户列表
pub async fn list_users(
    State(pool): State<SqlitePool>,
    _admin: AdminUser,
) -> Result<Json<Value>, AppError> {
    let users = sqlx::query_as::<_, User>("SELECT * FROM users ORDER BY created_at DESC")
        .fetch_all(&pool)
        .await?;

    let user_infos: Vec<UserInfo> = users.into_iter().map(UserInfo::from).collect();

    Ok(Json(json!({
        "success": true,
        "data": user_infos,
        "error": null
    })))
}

/// PUT /api/admin/users/:id/role 更新用户角色
pub async fn update_user_role(
    State(pool): State<SqlitePool>,
    _admin: AdminUser,
    Path(user_id): Path<i64>,
    Json(req): Json<UpdateUserRoleRequest>,
) -> Result<Json<Value>, AppError> {
    let role = req.role.trim();
    if role != "user" && role != "admin" {
        return Err(AppError::BadRequest("无效的角色，必须是 'user' 或 'admin'".into()));
    }

    let result = sqlx::query("UPDATE users SET role = ? WHERE id = ?")
        .bind(role)
        .bind(user_id)
        .execute(&pool)
        .await?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound("用户不存在".into()));
    }

    let user = sqlx::query_as::<_, User>("SELECT * FROM users WHERE id = ?")
        .bind(user_id)
        .fetch_one(&pool)
        .await?;

    Ok(Json(json!({
        "success": true,
        "data": UserInfo::from(user),
        "error": null
    })))
}

/// DELETE /api/admin/users/:id 删除用户
pub async fn delete_user(
    State(pool): State<SqlitePool>,
    _admin: AdminUser,
    Path(user_id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    // 防止删除自己
    if user_id == _admin.user_id {
        return Err(AppError::BadRequest("不能删除当前登录的管理员账户".into()));
    }

    let result = sqlx::query("DELETE FROM users WHERE id = ?")
        .bind(user_id)
        .execute(&pool)
        .await?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound("用户不存在".into()));
    }

    Ok(Json(json!({
        "success": true,
        "data": null,
        "error": null
    })))
}

/// GET /api/admin/stats 获取系统统计信息
pub async fn get_stats(
    State(pool): State<SqlitePool>,
    _admin: AdminUser,
) -> Result<Json<Value>, AppError> {
    let user_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users")
        .fetch_one(&pool)
        .await?;

    let file_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM files WHERE deleted_at IS NULL")
        .fetch_one(&pool)
        .await?;

    let folder_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM folders WHERE deleted_at IS NULL")
        .fetch_one(&pool)
        .await?;

    let share_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM file_shares WHERE is_active = 1")
        .fetch_one(&pool)
        .await?;

    let total_size: (i64,) = sqlx::query_as("SELECT COALESCE(SUM(size), 0) FROM files WHERE deleted_at IS NULL")
        .fetch_one(&pool)
        .await?;

    let trash_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM files WHERE deleted_at IS NOT NULL")
        .fetch_one(&pool)
        .await?;

    Ok(Json(json!({
        "success": true,
        "data": {
            "users": user_count.0,
            "files": file_count.0,
            "folders": folder_count.0,
            "shares": share_count.0,
            "trash_items": trash_count.0,
            "total_size": total_size.0,
            "formatted_size": crate::models::file::format_file_size(total_size.0),
        },
        "error": null
    })))
}
