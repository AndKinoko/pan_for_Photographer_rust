use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct User {
    pub id: i64,
    pub username: String,
    #[serde(skip_serializing)]
    pub password_hash: String,
    pub created_at: String,
}

/// Public user info (no password)
#[derive(Debug, Serialize)]
pub struct UserInfo {
    pub id: i64,
    pub username: String,
    pub created_at: String,
}

impl From<User> for UserInfo {
    fn from(u: User) -> Self {
        UserInfo {
            id: u.id,
            username: u.username,
            created_at: u.created_at,
        }
    }
}