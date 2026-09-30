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
///
/// 字段与 [] 完全一致，外加两个计数—— 靠它们决定
/// 是否显示「N 个文件 · M 个子文件夹」，缺失时会错误地退回显示创建时间。
#[derive(Debug, Serialize, FromRow)]
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
