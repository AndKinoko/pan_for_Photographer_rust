use axum::{
    async_trait,
    extract::{FromRef, FromRequestParts},
    http::{header, request::Parts, StatusCode},
    Json,
};
use serde_json::json;

use crate::config::Config;
use crate::utils::crypto::validate_token;
use crate::errors::AppError;
use sqlx::SqlitePool;

/// 提取的已验证管理员用户信息
#[derive(Debug, Clone)]
pub struct AdminUser {
    pub user_id: i64,
    pub username: String,
}

/// 从 Authorization 头验证 JWT 并检查管理员角色的 Axum 提取器
#[async_trait]
impl<S> FromRequestParts<S> for AdminUser
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

        let token = match auth_header {
            Some(t) => t,
            None => {
                return Err((
                    StatusCode::UNAUTHORIZED,
                    Json(json!({
                        "success": false,
                        "data": null,
                        "error": "认证失败"
                    })),
                ));
            }
        };

        let claims = match validate_token(token, &config) {
            Ok(c) => c,
            Err(_) => {
                return Err((
                    StatusCode::UNAUTHORIZED,
                    Json(json!({
                        "success": false,
                        "data": null,
                        "error": "认证失败"
                    })),
                ));
            }
        };

        // 检查用户角色
        let role: Option<(String,)> = sqlx::query_as(
            "SELECT role FROM users WHERE id = ?",
        )
        .bind(claims.sub)
        .fetch_optional(&pool)
        .await
        .map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "success": false,
                    "data": null,
                    "error": "服务器内部错误"
                })),
            )
        })?;

        match role {
            Some((r,)) if r == "admin" => Ok(AdminUser {
                user_id: claims.sub,
                username: claims.username,
            }),
            _ => Err((
                StatusCode::FORBIDDEN,
                Json(json!({
                    "success": false,
                    "data": null,
                    "error": "需要管理员权限"
                })),
            )),
        }
    }
}

impl From<AdminUser> for AppError {
    fn from(_: AdminUser) -> Self {
        AppError::Forbidden("需要管理员权限".into())
    }
}
