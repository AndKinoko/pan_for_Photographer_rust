use serde::{Deserialize, Serialize};

/// 批量移动/复制操作的请求
#[derive(Debug, Deserialize)]
pub struct BatchMoveCopyRequest {
    pub file_ids: Vec<i64>,
    pub folder_ids: Vec<i64>,
    pub target_folder_id: Option<i64>,
    #[serde(default = "default_conflict_strategy")]
    pub conflict_strategy: String,
}

fn default_conflict_strategy() -> String {
    "rename".to_string()
}

/// 批量删除的请求
#[derive(Debug, Deserialize)]
pub struct BatchDeleteRequest {
    pub file_ids: Vec<i64>,
    pub folder_ids: Vec<i64>,
}

/// 批处理操作中的单个项目结果
#[derive(Debug, Serialize)]
pub struct BatchItemResult {
    pub id: i64,
    pub name: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r#type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub children_count: Option<i64>,
}

/// 批量移动/复制结果的摘要
#[derive(Debug, Serialize)]
pub struct BatchMoveCopyResult {
    pub total: usize,
    pub succeeded: usize,
    pub skipped: usize,
    pub failed: usize,
    pub results: Vec<BatchItemResult>,
}

/// 批量删除结果的摘要
#[derive(Debug, Serialize)]
pub struct BatchDeleteResult {
    pub total: usize,
    pub deleted: usize,
    pub failed: usize,
    pub results: Vec<BatchItemResult>,
}

/// 批量分享的请求
#[derive(Debug, Deserialize)]
pub struct BatchShareRequest {
    pub file_ids: Vec<i64>,
    pub expires_hours: Option<i64>,
    pub password: Option<String>,
}

/// 单个分享结果
#[derive(Debug, Serialize)]
pub struct ShareItemResult {
    pub file_id: i64,
    pub file_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub share_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub share_url: Option<String>,
    pub status: String,
}

/// 批量分享结果的摘要
#[derive(Debug, Serialize)]
pub struct BatchShareResult {
    pub total: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub shares: Vec<ShareItemResult>,
}

/// 批量取消分享的请求
#[derive(Debug, Deserialize)]
pub struct BatchUnshareRequest {
    pub file_ids: Vec<i64>,
}

/// 单个取消分享结果
#[derive(Debug, Serialize)]
pub struct UnshareItemResult {
    pub file_id: i64,
    pub file_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub share_id: Option<String>,
    pub status: String,
}

/// 批量取消分享结果的摘要
#[derive(Debug, Serialize)]
pub struct BatchUnshareResult {
    pub total: usize,
    pub unshared: usize,
    pub failed: usize,
    pub results: Vec<UnshareItemResult>,
}
