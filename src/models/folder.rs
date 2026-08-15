use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Folder {
    pub id: i64,
    pub name: String,
    pub owner_id: i64,
    pub parent_id: Option<i64>,
    pub created_at: String,
    pub updated_at: String,
    pub deleted_at: Option<String>,
}

/// 文件夹信息，包含文件/子文件夹数量
#[derive(Debug, Serialize)]
pub struct FolderInfo {
    pub id: i64,
    pub name: String,
    pub owner_id: i64,
    pub parent_id: Option<i64>,
    pub created_at: String,
    pub updated_at: String,
    pub file_count: i64,
    pub subfolder_count: i64,
    pub deleted_at: Option<String>,
}
