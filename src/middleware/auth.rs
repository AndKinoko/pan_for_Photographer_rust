use axum::{
    async_trait,
    extract::{FromRef, FromRequestParts},
    http::{header, request::Parts, StatusCode},
    Json,
};
use serde_json::json;
use sqlx::SqlitePool;

use crate::config::Config;
use crate::utils::crypto::validate_token;

/// 提取的已验证用户信息
#[derive(Debug, Clone)]
pub struct AuthUser {
    pub user_id: i64,
    #[allow(dead_code)]
    pub username: String,
}

fn rejection(msg: &str) -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({
            "success": false,
            "data": null,
            "error": msg
        })),
    )
}

/// 从 Authorization 头验证 JWT 的 Axum 提取器。
/// 同时校验账号有效期（expires_at，NULL 表示永久有效）。
#[async_trait]
impl<S> FromRequestParts<S> for AuthUser
where
    Config: FromRef<S>,
    SqlitePool: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = (StatusCode, Json<serde_json::Value>);

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let config = Config::from_ref(state);
        let pool = SqlitePool::from_ref(state);
        let auth_header = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "));

        let token = auth_header.ok_or_else(|| rejection("认证失败"))?;
        let claims = validate_token(token, &config).map_err(|_| rejection("认证失败"))?;

        // 校验账号有效期：expires_at 已过则拒绝（NULL 表示永久有效）
        let expires_at: Option<Option<String>> =
            sqlx::query_scalar("SELECT expires_at FROM users WHERE id = ?")
                .bind(claims.sub)
                .fetch_optional(&pool)
                .await
                .map_err(|_| rejection("认证失败"))?
                .ok_or_else(|| rejection("认证失败"))?;

        if let Some(Some(exp)) = expires_at {
            let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
            if exp.trim() <= now.as_str() {
                return Err(rejection("账号已过期，请联系管理员续期"));
            }
        }

        Ok(AuthUser {
            user_id: claims.sub,
            username: claims.username,
        })
    }
}