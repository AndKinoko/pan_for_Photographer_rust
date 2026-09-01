use axum::{extract::{Path, Query, State}, Json};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::errors::AppError;
use crate::middleware::admin::AdminUser;
use crate::models::user::{User, UserInfo};
use crate::services::folder_service;
use sqlx::SqlitePool;

#[derive(Debug, Deserialize)]
pub struct UpdateUserRoleRequest {
    pub role: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateUserRequest {
    pub username: String,
    pub password: String,
    pub role: Option<String>,
    pub expires_at: Option<Option<String>>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateUserRequest {
    pub username: Option<String>,
    pub password: Option<String>,
    pub role: Option<String>,
    pub expires_at: Option<Option<String>>,
}

/// 归一化有效期字符串为 "YYYY-MM-DD HH:MM:SS" 格式；日期仅当天的视为截止 23:59:59。
fn normalize_expires(v: Option<String>) -> Option<String> {
    let raw = v?.trim().to_string();
    if raw.is_empty() {
        return None;
    }
    let norm = raw.replace('T', " ");
    if norm.len() == 10 {
        return Some(format!("{} 23:59:59", norm));
    }
    if norm.len() == 16 {
        return Some(format!("{}:00", norm));
    }
    Some(norm)
}

/// 将用户记录扩展为带统计信息与「原图」文件夹的管理端视图。
async fn build_admin_user(pool: &SqlitePool, user: User) -> Result<serde_json::Value, AppError> {
    let (file_count,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM files WHERE owner_id = ? AND deleted_at IS NULL",
    )
    .bind(user.id)
    .fetch_one(pool)
    .await?;

    let original_folder_id: Option<(i64,)> = sqlx::query_as(
        "SELECT id FROM folders WHERE owner_id = ? AND name = '原图' AND parent_id IS NULL AND deleted_at IS NULL",
    )
    .bind(user.id)
    .fetch_optional(pool)
    .await?;

    let info = UserInfo::from(user);
    Ok(serde_json::json!({
        "id": info.id,
        "username": info.username,
        "role": info.role,
        "created_at": info.created_at,
        "expires_at": info.expires_at,
        "file_count": file_count,
        "original_folder_id": original_folder_id.map(|(id,)| id),
    }))
}

/// GET /api/admin/users 获取所有用户列表
pub async fn list_users(
    State(pool): State<SqlitePool>,
    _admin: AdminUser,
) -> Result<Json<Value>, AppError> {
    let users = sqlx::query_as::<_, User>("SELECT * FROM users ORDER BY created_at DESC")
        .fetch_all(&pool)
        .await?;

    let mut list = Vec::new();
    for u in users {
        list.push(build_admin_user(&pool, u).await?);
    }

    Ok(Json(json!({
        "success": true,
        "data": list,
        "error": null
    })))
}

/// POST /api/admin/users 新建普通用户（自动创建「原图」文件夹）
pub async fn create_user(
    State(pool): State<SqlitePool>,
    _admin: AdminUser,
    Json(req): Json<CreateUserRequest>,
) -> Result<Json<Value>, AppError> {
    let username = req.username.trim().to_string();
    if username.is_empty() {
        return Err(AppError::BadRequest("用户名不能为空".into()));
    }
    if req.password.len() < 6 {
        return Err(AppError::BadRequest("密码长度至少6位".into()));
    }
    let role = req.role.as_deref().unwrap_or("user").trim();
    if role != "user" && role != "admin" {
        return Err(AppError::BadRequest("无效的角色，必须是 'user' 或 'admin'".into()));
    }

    let password_hash = crate::utils::crypto::hash_password(&req.password)?;
    let expires_at = normalize_expires(req.expires_at.flatten());

    let user = sqlx::query_as::<_, User>(
        "INSERT INTO users (username, password_hash, role, expires_at) VALUES (?, ?, ?, ?) RETURNING *",
    )
    .bind(&username)
    .bind(&password_hash)
    .bind(role)
    .bind(&expires_at)
    .fetch_one(&pool)
    .await
    .map_err(|e| match e {
        sqlx::Error::Database(de) if de.is_unique_violation() => {
            AppError::Conflict("用户名已存在".into())
        }
        other => AppError::from(other),
    })?;

    // 自动为新建普通用户创建根目录下的「原图」文件夹
    let original_folder = folder_service::create_folder(&pool, user.id, "原图", None)
        .await
        .ok();

    let folded = build_admin_user(&pool, user).await?;
    let mut value = folded;
    if let Some(obj) = value.as_object_mut() {
        obj.insert(
            "original_folder_id".into(),
            serde_json::json!(original_folder.as_ref().map(|f| f.id)),
        );
    }

    Ok(Json(json!({
        "success": true,
        "data": value,
        "error": null
    })))
}

/// PUT /api/admin/users/:id 更新普通用户（账号/密码/角色/有效期）
pub async fn update_user(
    State(pool): State<SqlitePool>,
    _admin: AdminUser,
    Path(user_id): Path<i64>,
    Json(req): Json<UpdateUserRequest>,
) -> Result<Json<Value>, AppError> {
    let mut user = sqlx::query_as::<_, User>("SELECT * FROM users WHERE id = ?")
        .bind(user_id)
        .fetch_optional(&pool)
        .await?
        .ok_or_else(|| AppError::NotFound("用户不存在".into()))?;

    if let Some(ref u) = req.username {
        let name = u.trim();
        if name.is_empty() {
            return Err(AppError::BadRequest("用户名不能为空".into()));
        }
        user.username = name.to_string();
    }
    if let Some(ref p) = req.password {
        if p.len() < 6 {
            return Err(AppError::BadRequest("密码长度至少6位".into()));
        }
        user.password_hash = crate::utils::crypto::hash_password(p)?;
    }
    if let Some(ref r) = req.role {
        let role = r.trim();
        if role != "user" && role != "admin" {
            return Err(AppError::BadRequest("无效的角色，必须是 'user' 或 'admin'".into()));
        }
        user.role = role.to_string();
    }
    // expires_at 提供时（Some(_)）更新；提供 null 表示清除有效期；未提供保持原样
    if let Some(v) = req.expires_at {
        user.expires_at = normalize_expires(v);
    }

    let updated = sqlx::query_as::<_, User>(
        "UPDATE users SET username = ?, password_hash = ?, role = ?, expires_at = ?, created_at = created_at WHERE id = ? RETURNING *",
    )
    .bind(&user.username)
    .bind(&user.password_hash)
    .bind(&user.role)
    .bind(&user.expires_at)
    .bind(user_id)
    .fetch_one(&pool)
    .await
    .map_err(|e| match e {
        sqlx::Error::Database(de) if de.is_unique_violation() => {
            AppError::Conflict("用户名已存在".into())
        }
        other => AppError::from(other),
    })?;

    let value = build_admin_user(&pool, updated).await?;
    Ok(Json(json!({
        "success": true,
        "data": value,
        "error": null
    })))
}

/// 修改用户角色（保留：兼容原有接口）
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

#[derive(Debug, Deserialize)]
pub struct AdminFolderListQuery {
    pub parent_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct AdminCreateFolderRequest {
    pub name: String,
    pub parent_id: Option<i64>,
}

/// 确认目标用户存在，供管理员代操作接口复用
async fn ensure_user_exists(pool: &SqlitePool, user_id: i64) -> Result<(), AppError> {
    let exists: Option<(i64,)> = sqlx::query_as("SELECT id FROM users WHERE id = ?")
        .bind(user_id)
        .fetch_optional(pool)
        .await?;
    if exists.is_none() {
        return Err(AppError::NotFound("目标用户不存在".into()));
    }
    Ok(())
}

/// GET /api/admin/users/:id/folders?parent_id= 列出某普通用户的文件夹（管理端代查）
pub async fn admin_list_user_folders(
    State(pool): State<SqlitePool>,
    _admin: AdminUser,
    Path(user_id): Path<i64>,
    Query(query): Query<AdminFolderListQuery>,
) -> Result<Json<Value>, AppError> {
    ensure_user_exists(&pool, user_id).await?;
    let folders = folder_service::list_folders(&pool, user_id, query.parent_id).await?;
    Ok(Json(json!({
        "success": true,
        "data": { "folders": folders },
        "error": null
    })))
}

/// POST /api/admin/users/:id/folders 为某普通用户新建文件夹
pub async fn admin_create_user_folder(
    State(pool): State<SqlitePool>,
    _admin: AdminUser,
    Path(user_id): Path<i64>,
    Json(req): Json<AdminCreateFolderRequest>,
) -> Result<Json<Value>, AppError> {
    let name = req.name.trim().to_string();
    if name.is_empty() {
        return Err(AppError::BadRequest("文件夹名称不能为空".into()));
    }
    ensure_user_exists(&pool, user_id).await?;
    let folder = folder_service::create_folder(&pool, user_id, &name, req.parent_id).await?;
    Ok(Json(json!({
        "success": true,
        "data": folder,
        "error": null
    })))
}
