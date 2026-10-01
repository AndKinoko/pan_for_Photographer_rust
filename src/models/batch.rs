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

/// 批量取消分享的请求。
///
/// 批次模型下一个文件可能同时出现在好几个分享里，而分享是**批次级**的——
/// 没法只把其中一个文件从某个批次里摘掉。所以这里的语义是
/// 「把这些项所在的分享整体停用」，前端必须在确认框里说清楚。
#[derive(Debug, Deserialize)]
pub struct BatchUnshareRequest {
    pub items: Vec<crate::models::share::ShareItemRef>,
}

/// 单个条目的取消分享结果
#[derive(Debug, Serialize)]
pub struct UnshareItemResult {
    pub id: i64,
    pub item_type: String,
    pub name: String,
    /// 被停用的分享。一个条目可能牵动多个批次。
    pub share_ids: Vec<String>,
    /// `unshared`（至少停用了一个分享）或 `not_found`（该条目没有任何活跃分享）
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
