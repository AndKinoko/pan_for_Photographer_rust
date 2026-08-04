use axum::{
    async_trait,
    extract::{FromRef, FromRequestParts},
    http::{header, request::Parts, StatusCode},
    Json,
};
use serde_json::json;

use crate::config::Config;
use crate::utils::crypto::validate_token;

/// Extracted authenticated user info
#[derive(Debug, Clone)]
pub struct AuthUser {
    pub user_id: i64,
    #[allow(dead_code)]
    pub username: String,
}

/// Axum extractor that validates JWT from Authorization header
#[async_trait]
impl<S> FromRequestParts<S> for AuthUser
where
    Config: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = (StatusCode, Json<serde_json::Value>);

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let config = Config::from_ref(state);
        let auth_header = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "));

        match auth_header {
            Some(token) => match validate_token(token, &config) {
                Ok(claims) => Ok(AuthUser {
                    user_id: claims.sub,
                    username: claims.username,
                }),
                Err(_) => Err((
                    StatusCode::UNAUTHORIZED,
                    Json(json!({
                        "success": false,
                        "data": null,
                        "error": "认证失败"
                    })),
                )),
            },
            None => Err((
                StatusCode::UNAUTHORIZED,
                Json(json!({
                    "success": false,
                    "data": null,
                    "error": "认证失败"
                })),
            )),
        }
    }
}