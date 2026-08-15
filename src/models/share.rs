use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct FileShare {
    pub id: String,
    pub file_id: Option<i64>,
    pub folder_id: Option<i64>,
    pub owner_id: i64,
    pub created_at: String,
    pub expires_at: Option<String>,
    pub password_hash: String,
    pub download_count: i64,
    pub max_downloads: Option<i64>,
    pub is_active: i64,
    pub custom_code: Option<String>,
}

/// 用于API响应的分享信息
#[derive(Debug, Serialize)]
pub struct ShareInfo {
    pub id: String,
    pub file_id: Option<i64>,
    pub folder_id: Option<i64>,
    pub file_name: String,
    pub owner_id: i64,
    pub owner_name: String,
    pub created_at: String,
    pub expires_at: Option<String>,
    pub has_password: bool,
    pub download_count: i64,
    pub max_downloads: Option<i64>,
    pub is_active: bool,
    pub is_expired: bool,
    pub share_url: String,
    pub file_type: String,
    pub file_size: i64,
    pub formatted_size: String,
    pub preview_url: Option<String>,
    pub thumb_url: Option<String>,
    pub custom_code: Option<String>,
}
