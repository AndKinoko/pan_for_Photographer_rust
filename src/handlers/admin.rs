use axum::{extract::{Path, Query, State}, Json};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::errors::AppError;
use crate::middleware::admin::AdminUser;
use crate::models::user::{User, UserInfo};
use crate::config::Config;
use crate::services::folder_service;
use crate::services::sweeper;
use crate::services::invite_code_service;
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
    pub quota_bytes: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateUserRequest {
    pub username: Option<String>,
    pub password: Option<String>,
    pub role: Option<String>,
    pub expires_at: Option<Option<String>>,
    pub quota_bytes: Option<i64>,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_expires_handles_empty_values() {
        assert_eq!(normalize_expires(None), None);
        assert_eq!(normalize_expires(Some(String::new())), None);
        assert_eq!(normalize_expires(Some("   ".to_string())), None);
    }

    #[test]
    fn normalize_expires_date_only_means_end_of_day() {
        assert_eq!(
            normalize_expires(Some("2026-12-31".to_string())),
            Some("2026-12-31 23:59:59".to_string())
        );
    }

    #[test]
    fn normalize_expires_datetime_local_shapes() {
        assert_eq!(
            normalize_expires(Some("2026-12-31T08:30".to_string())),
            Some("2026-12-31 08:30:00".to_string())
        );
        assert_eq!(
            normalize_expires(Some("2026-12-31 08:30:15".to_string())),
            Some("2026-12-31 08:30:15".to_string())
        );
    }
}

/// 将用户记录扩展为带统计信息与「原图」文件夹的管理端视图。
async fn build_admin_user(pool: &SqlitePool, user: User) -> Result<serde_json::Value, AppError> {
    let (file_count,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM files WHERE owner_id = ? AND deleted_at IS NULL",
    )
    .bind(user.id)
    .fetch_one(pool)
    .await?;

    // 用量含回收站（软删除文件仍占磁盘），与配额校验口径一致
    let (used_bytes,): (i64,) =
        sqlx::query_as("SELECT COALESCE(SUM(size), 0) FROM files WHERE owner_id = ?")
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
        "quota_bytes": info.quota_bytes,
        "used_bytes": used_bytes,
        "formatted_used": crate::models::file::format_file_size(used_bytes),
        "usage_percent": if info.quota_bytes > 0 {
            ((used_bytes as f64 / info.quota_bytes as f64) * 1000.0).round() / 10.0
        } else {
            100.0
        },
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
    let quota_bytes = match req.quota_bytes {
        Some(v) if v < 0 => return Err(AppError::BadRequest("配额不能为负数".into())),
        Some(v) => v,
        None => 5_368_709_120,
    };

    let user = sqlx::query_as::<_, User>(
        "INSERT INTO users (username, password_hash, role, expires_at, quota_bytes) VALUES (?, ?, ?, ?, ?) RETURNING *",
    )
    .bind(&username)
    .bind(&password_hash)
    .bind(role)
    .bind(&expires_at)
    .bind(quota_bytes)
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
    if let Some(q) = req.quota_bytes {
        if q < 0 {
            return Err(AppError::BadRequest("配额不能为负数".into()));
        }
        user.quota_bytes = q;
    }

    let updated = sqlx::query_as::<_, User>(
        "UPDATE users SET username = ?, password_hash = ?, role = ?, expires_at = ?, quota_bytes = ?, created_at = created_at WHERE id = ? RETURNING *",
    )
    .bind(&user.username)
    .bind(&user.password_hash)
    .bind(&user.role)
    .bind(&user.expires_at)
    .bind(user.quota_bytes)
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

// ===========================================================================
// 注册邀请码
// ---------------------------------------------------------------------------
// 注册改为邀请制后，摄影师需要一个自助渠道发码给客户。这些接口全部由
// AdminUser 提取器守护——它会验签 + 查库确认账号未过期 + 校验 role，
// 所以过期管理员的令牌在这里同样进不来（见 middleware/admin.rs）。
// ===========================================================================

#[derive(Debug, Deserialize)]
pub struct CreateInviteCodesRequest {
    /// 一次生成几个。批量是主用法：交付常常要一次给几个客户。
    pub count: Option<i64>,
    /// 每个码能被用几次。默认 1，即「一码一人」。
    pub max_uses: Option<i64>,
    /// 有效期（小时）。省略或 null 表示永不过期。
    pub expires_hours: Option<i64>,
    /// 备注，方便摄影师记「这个码给谁了」。会显示在管理列表里。
    pub note: Option<String>,
}

/// GET /api/admin/invite-codes 列出全部邀请码
pub async fn list_invite_codes(
    State(pool): State<SqlitePool>,
    _admin: AdminUser,
) -> Result<Json<Value>, AppError> {
    let codes = invite_code_service::list_codes(&pool).await?;

    // 附带「谁用了这个码」的用户名，避免摄影师在两个页面之间来回对照。
    let mut items = Vec::with_capacity(codes.len());
    for c in codes {
        let used_by_username: Option<String> = match c.used_by {
            Some(uid) => sqlx::query_scalar("SELECT username FROM users WHERE id = ?")
                .bind(uid)
                .fetch_optional(&pool)
                .await?,
            None => None,
        };
        let mut v = serde_json::to_value(&c)?;
        if let Some(obj) = v.as_object_mut() {
            obj.insert("usable".into(), json!(c.is_usable()));
            obj.insert("used_by_username".into(), json!(used_by_username));
        }
        items.push(v);
    }

    Ok(Json(json!({
        "success": true,
        "data": { "codes": items },
        "error": null
    })))
}

/// POST /api/admin/invite-codes 生成邀请码
pub async fn create_invite_codes(
    State(pool): State<SqlitePool>,
    admin: AdminUser,
    Json(req): Json<CreateInviteCodesRequest>,
) -> Result<Json<Value>, AppError> {
    let codes = invite_code_service::create_codes(
        &pool,
        admin.user_id,
        req.count.unwrap_or(1),
        req.max_uses.unwrap_or(1),
        req.expires_hours,
        req.note.as_deref().unwrap_or(""),
    )
    .await?;

    Ok(Json(json!({
        "success": true,
        "data": { "codes": codes },
        "error": null
    })))
}

/// DELETE /api/admin/invite-codes/:id 删除邀请码
///
/// 删已使用的码只是让列表干净，不影响已创建的账号——
/// 账号归属由 users 表决定，删掉记录不会把人变成「无来源的用户」。
pub async fn delete_invite_code(
    State(pool): State<SqlitePool>,
    _admin: AdminUser,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    invite_code_service::delete_code(&pool, id).await?;
    Ok(Json(json!({
        "success": true,
        "data": null,
        "error": null
    })))
}

// ===========================================================================
// 孤儿文件清理（手动触发）
// ---------------------------------------------------------------------------
// 孤儿文件只在异常路径后产生（进程被杀、用户被删、rename 后未 INSERT），
// 不是正常产物，所以清理频率可以很低。后台任务每 24 小时自动跑一次，
// 这里再提供一个手动入口——运维发现磁盘占用异常时可以立刻处理。
//
// 全部由 AdminUser 守护：它会验签 + 查库确认账号未过期 + 校验 role，
// 所以过期管理员的令牌在这里同样进不来。
// ===========================================================================

/// POST /api/admin/gc/cleanup 立即执行一次孤儿清理
pub async fn run_gc_cleanup(
    State(pool): State<SqlitePool>,
    _admin: AdminUser,
    State(config): State<Config>,
) -> Result<Json<Value>, AppError> {
    // 防止并发点击：两个清理同时跑会互相争抢同一批文件
    // （虽然 ORPHAN_GRACE 宽限期能兜住大部分情况，但没必要冒这个险）
    static RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if RUNNING.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return Err(AppError::TooManyRequests("上一次清理还在进行中，请稍候".into()));
    }

    let result = sweeper::cleanup_orphans(&pool, &config).await;
    RUNNING.store(false, std::sync::atomic::Ordering::SeqCst);

    let stats = result?;
    Ok(Json(json!({
        "success": true,
        "data": stats,
        "error": null
    })))
}

/// GET /api/admin/gc/status 孤儿清理任务的状态
///
/// 只报告后台任务的**配置**，不报告「上次执行时间」——那个时间没有落盘
/// （重启后重新计 24 小时是符合预期的：重启后手动点一次即可）。
pub async fn gc_status(
    _admin: AdminUser,
) -> Result<Json<Value>, AppError> {
    Ok(Json(json!({
        "success": true,
        "data": {
            // 后台自动执行的间隔（小时）
            "auto_interval_hours": sweeper::ORPHAN_CLEANUP_INTERVAL.as_secs() / 3600,
            // 缩略图重投仍保持独立的高频周期
            "preview_retry_note": "缩略图重投仍按 GC_INTERVAL_SEC 独立运行，不受此间隔影响"
        },
        "error": null
    })))
}
