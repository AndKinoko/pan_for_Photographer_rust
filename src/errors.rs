use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

/// Unified application error type
#[derive(Debug)]
#[allow(dead_code)]
pub enum AppError {
    BadRequest(String),
    Unauthorized(String),
    Forbidden(String),
    NotFound(String),
    Conflict(String),
    Gone(String),
    Internal(String),
    PayloadTooLarge(String),
}

impl AppError {
    pub fn status_code(&self) -> StatusCode {
        match self {
            AppError::BadRequest(_) => StatusCode::BAD_REQUEST,
            AppError::Unauthorized(_) => StatusCode::UNAUTHORIZED,
            AppError::Forbidden(_) => StatusCode::FORBIDDEN,
            AppError::NotFound(_) => StatusCode::NOT_FOUND,
            AppError::Conflict(_) => StatusCode::CONFLICT,
            AppError::Gone(_) => StatusCode::GONE,
            AppError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
            AppError::PayloadTooLarge(_) => StatusCode::PAYLOAD_TOO_LARGE,
        }
    }

    pub fn message(&self) -> &str {
        match self {
            AppError::BadRequest(msg) => msg,
            AppError::Unauthorized(msg) => msg,
            AppError::Forbidden(msg) => msg,
            AppError::NotFound(msg) => msg,
            AppError::Conflict(msg) => msg,
            AppError::Gone(msg) => msg,
            AppError::Internal(_) => "服务器内部错误",
            AppError::PayloadTooLarge(msg) => msg,
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = self.status_code();
        let body = Json(json!({
            "success": false,
            "data": null,
            "error": self.message(),
        }));
        (status, body).into_response()
    }
}

// Convert common errors into AppError
impl From<sqlx::Error> for AppError {
    fn from(e: sqlx::Error) -> Self {
        match e {
            sqlx::Error::RowNotFound => AppError::NotFound("资源不存在".into()),
            _ => {
                tracing::error!("Database error: {:?}", e);
                AppError::Internal("数据库操作失败".into())
            }
        }
    }
}

impl From<bcrypt::BcryptError> for AppError {
    fn from(e: bcrypt::BcryptError) -> Self {
        tracing::error!("Bcrypt error: {:?}", e);
        AppError::Internal("密码处理失败".into())
    }
}

impl From<image::ImageError> for AppError {
    fn from(e: image::ImageError) -> Self {
        tracing::error!("Image processing error: {:?}", e);
        AppError::Internal("图片处理失败".into())
    }
}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        tracing::error!("IO error: {:?}", e);
        AppError::Internal("文件操作失败".into())
    }
}

impl From<serde_json::Error> for AppError {
    fn from(e: serde_json::Error) -> Self {
        tracing::error!("JSON error: {:?}", e);
        AppError::BadRequest("请求数据格式错误".into())
    }
}

impl From<axum::extract::multipart::MultipartError> for AppError {
    fn from(e: axum::extract::multipart::MultipartError) -> Self {
        tracing::error!("Multipart error: {:?}", e);
        AppError::BadRequest("文件上传数据格式错误".into())
    }
}

impl From<jsonwebtoken::errors::Error> for AppError {
    fn from(e: jsonwebtoken::errors::Error) -> Self {
        tracing::error!("JWT error: {:?}", e);
        AppError::Unauthorized("认证失败".into())
    }
}