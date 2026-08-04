use axum::{extract::State, Json};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::config::Config;
use crate::errors::AppError;
use crate::middleware::auth::AuthUser;
use crate::models::user::{User, UserInfo};
use crate::utils::crypto;
use sqlx::SqlitePool;

#[derive(Debug, Deserialize)]
pub struct RegisterRequest {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

/// POST /api/auth/register
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

    let password_hash = crypto::hash_password(&req.password)?;

    let result = sqlx::query_as::<_, User>(
        "INSERT INTO users (username, password_hash) VALUES (?, ?) RETURNING *",
    )
    .bind(req.username.trim())
    .bind(&password_hash)
    .fetch_one(&pool)
    .await;

    match result {
        Ok(user) => {
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
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
            Err(AppError::Conflict("用户名已存在".into()))
        }
        Err(e) => Err(AppError::from(e)),
    }
}

/// POST /api/auth/login
pub async fn login(
    State(pool): State<SqlitePool>,
    State(config): State<Config>,
    Json(req): Json<LoginRequest>,
) -> Result<Json<Value>, AppError> {
    let user = sqlx::query_as::<_, User>("SELECT * FROM users WHERE username = ?")
        .bind(&req.username)
        .fetch_optional(&pool)
        .await?
        .ok_or_else(|| AppError::Unauthorized("用户名或密码错误".into()))?;

    let valid = crypto::verify_password(&req.password, &user.password_hash)?;
    if !valid {
        return Err(AppError::Unauthorized("用户名或密码错误".into()));
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

/// GET /api/auth/me
pub async fn me(
    State(pool): State<SqlitePool>,
    auth: AuthUser,
) -> Result<Json<Value>, AppError> {
    let user = sqlx::query_as::<_, User>("SELECT * FROM users WHERE id = ?")
        .bind(auth.user_id)
        .fetch_optional(&pool)
        .await?
        .ok_or_else(|| AppError::NotFound("用户不存在".into()))?;

    Ok(Json(json!({
        "success": true,
        "data": UserInfo::from(user),
        "error": null
    })))
}