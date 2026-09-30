use std::net::SocketAddr;

use axum::{
    extract::{ConnectInfo, State},
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::config::Config;
use crate::errors::AppError;
use crate::middleware::auth::AuthUser;
use crate::models::user::{User, UserInfo};
use crate::services::invite_code_service;
use crate::utils::crypto;
use sqlx::SqlitePool;

#[derive(Debug, Deserialize)]
pub struct RegisterRequest {
    pub username: String,
    pub password: String,
    /// 邀请码。注册制下必填——用 Option 只是为了让「缺码」返回 400 而不是 422。
    pub invite_code: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct VerifyInviteRequest {
    pub invite_code: String,
}

#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

/// POST /api/auth/invite/verify 校验邀请码是否可用（**不消费**）
///
/// 注册页第一步调用：客户拿到码先验一次，通过了再让他填用户名密码。
/// 不做这步的话，用户要填完整页表单才被告知「码无效」。
pub async fn verify_invite(
    State(pool): State<SqlitePool>,
    Json(req): Json<VerifyInviteRequest>,
) -> Result<Json<Value>, AppError> {
    invite_code_service::validate_code(&pool, &req.invite_code).await?;

    Ok(Json(json!({
        "success": true,
        "data": { "valid": true },
        "error": null
    })))
}

/// POST /api/auth/register 注册（邀请制）
///
/// 公开注册在公网是磁盘 DoS 的入口：新用户默认 5GB 配额，任意人都能
/// 无限注册把磁盘吃满。同时交付场景本就该由摄影师决定「谁能进这个网盘」。
/// 因此改为必须持有管理员生成的邀请码，且**建号与消费码在同一事务内**
/// （见 `invite_code_service::register_with_invite`），并发用同一个码
/// 不会超发出两个账号。
pub async fn register(
    State(pool): State<SqlitePool>,
    State(config): State<Config>,
    Json(req): Json<RegisterRequest>,
) -> Result<Json<Value>, AppError> {
    if req.username.trim().is_empty() {
        return Err(AppError::BadRequest("用户名不能为空".into()));
    }
    if req.password.len() < 6 {
        return Err(AppError::BadRequest("密码长度至少6位".into()));
    }

    // bcrypt 很贵（DEFAULT_COST=12，约 250ms/次）。**放在验码之后**：
    // 拿一个不存在的码反复注册，不该让服务端每次都付一次哈希成本。
    let code = req
        .invite_code
        .as_deref()
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .ok_or_else(|| AppError::BadRequest("需要邀请码才能注册".into()))?;

    invite_code_service::validate_code(&pool, code).await?;

    let password_hash = crypto::hash_password(&req.password)?;

    let user = invite_code_service::register_with_invite(
        &pool,
        code,
        req.username.trim(),
        &password_hash,
    )
    .await?;

    let token = crypto::generate_token(user.id, &user.username, &config)?;
    Ok(Json(json!({
        "success": true,
        "data": {
            "token": token,
            "user": UserInfo::from(user),
        },
        "error": null
    })))
}

/// POST /api/auth/login 登录
pub async fn login(
    State(pool): State<SqlitePool>,
    State(config): State<Config>,
    // Option 而非必填：axum 在未启用 into_make_service_with_connect_info 时
    // 不会提供连接信息，此时降级为「未知来源」——限流仍按用户名维度生效，
    // 只是失去 IP 这一维。测试夹具也是走这条路径。
    client: Option<ConnectInfo<SocketAddr>>,
    Json(req): Json<LoginRequest>,
) -> Result<Json<Value>, AppError> {
    let ip = client.map(|ConnectInfo(a)| a.ip().to_string()).unwrap_or_default();

    // 限流检查：按「用户名 + 来源 IP」双维度，任一退避中即拒绝。
    // 放在查库之前，让爆破在触发 bcrypt 之前就被挡住——bcrypt 很贵，
    // 不限流时每个请求都要付一次校验成本。
    if let Some(wait) = crate::utils::login_throttle::check(&req.username, &ip) {
        return Err(AppError::TooManyRequests(format!(
            "登录失败次数过多，请在 {} 秒后重试",
            wait
        )));
    }

    let user = sqlx::query_as::<_, User>("SELECT * FROM users WHERE username = ?")
        .bind(&req.username)
        .fetch_optional(&pool)
        .await?;

    // 用户不存在与密码错误返回同一句话：分两句就成了用户名枚举工具。
    let Some(user) = user else {
        crate::utils::login_throttle::record_failure(&req.username, &ip);
        return Err(AppError::Unauthorized("用户名或密码错误".into()));
    };

    let valid = crypto::verify_password(&req.password, &user.password_hash)?;
    if !valid {
        crate::utils::login_throttle::record_failure(&req.username, &ip);
        return Err(AppError::Unauthorized("用户名或密码错误".into()));
    }

    // 密码正确才清空计数——上面两处失败已各自记过一次。
    crate::utils::login_throttle::record_success(&req.username, &ip);

    // 校验账号有效期：expires_at 已过则拒绝登录（NULL 表示永久有效）
    // 统一按 UTC 比较（存储与比较口径见 utils::time）
    if let Some(ref exp) = user.expires_at {
        if crate::utils::time::is_expired_utc(exp) {
            return Err(AppError::Unauthorized(format!(
                "账号已过期（{} UTC），请联系管理员续期",
                exp.trim()
            )));
        }
    }

    let token = crypto::generate_token(user.id, &user.username, &config)?;

    Ok(Json(json!({
        "success": true,
        "data": {
            "token": token,
            "user": UserInfo::from(user),
        },
        "error": null
    })))
}

/// GET /api/auth/me 获取当前用户信息
pub async fn me(
    State(pool): State<SqlitePool>,
    auth: AuthUser,
) -> Result<Json<Value>, AppError> {
    let user = sqlx::query_as::<_, User>("SELECT * FROM users WHERE id = ?")
        .bind(auth.user_id)
        .fetch_optional(&pool)
        .await?
        .ok_or_else(|| AppError::NotFound("用户不存在".into()))?;

    // 已用容量（含回收站，与配额校验口径一致），供前端展示
    let (used_bytes,): (i64,) =
        sqlx::query_as("SELECT COALESCE(SUM(size), 0) FROM files WHERE owner_id = ?")
            .bind(user.id)
            .fetch_one(&pool)
            .await?;

    let mut data = serde_json::to_value(UserInfo::from(user))?;
    if let Some(obj) = data.as_object_mut() {
        obj.insert("used_bytes".into(), json!(used_bytes));
    }

    Ok(Json(json!({
        "success": true,
        "data": data,
        "error": null
    })))
}