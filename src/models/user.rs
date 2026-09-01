use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct User {
    pub id: i64,
    pub username: String,
    #[serde(skip_serializing)]
    pub password_hash: String,
    pub created_at: String,
    pub role: String,
    pub expires_at: Option<String>,
}

/// 公开用户信息（不含密码）
#[derive(Debug, Serialize)]
pub struct UserInfo {
    pub id: i64,
    pub username: String,
    pub created_at: String,
    pub role: String,
    pub expires_at: Option<String>,
}

impl From<User> for UserInfo {
    fn from(u: User) -> Self {
        UserInfo {
            id: u.id,
            username: u.username,
            created_at: u.created_at,
            role: u.role,
            expires_at: u.expires_at,
        }
    }
}
