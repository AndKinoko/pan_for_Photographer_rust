use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct File {
    pub id: i64,
    pub name: String,
    pub original_name: String,
    pub stored_path: String,
    pub preview_path: Option<String>,
    pub thumb_path: Option<String>,
    pub owner_id: i64,
    pub folder_id: Option<i64>,
    pub size: i64,
    pub file_type: String,
    pub uploaded_at: String,
    pub updated_at: String,
    pub deleted_at: Option<String>,
}

/// 文件信息，包含格式化后的文件大小
#[derive(Debug, Serialize)]
pub struct FileInfo {
    pub id: i64,
    pub name: String,
    pub original_name: String,
    pub owner_id: i64,
    pub folder_id: Option<i64>,
    pub size: i64,
    pub formatted_size: String,
    pub file_type: String,
    pub uploaded_at: String,
    pub updated_at: String,
    pub has_preview: bool,
    pub preview_url: Option<String>,
    pub thumb_url: Option<String>,
    pub download_url: String,
    pub media_url: String,
    pub deleted_at: Option<String>,
}

impl File {
    pub fn to_info(&self) -> FileInfo {
        // preview/thumb 未生成（NULL）时返回 None，让前端显示文件图标/占位，
        // 避免回退到原始大图导致全量下载。后台生成完成后由列表刷新补上。
        let has_preview = self.preview_path.is_some();
        let preview_url = if self.preview_path.is_some() {
            Some(format!("/api/files/{}/media?preview=1", self.id))
        } else {
            None
        };

        let thumb_url = if self.thumb_path.is_some() {
            Some(format!("/api/files/{}/media?thumb=1", self.id))
        } else if let Some(pv) = &preview_url {
            // 对于没有缩略图的旧文件，回退到 preview_url
            Some(pv.clone())
        } else {
            None
        };

        FileInfo {
            id: self.id,
            name: self.name.clone(),
            original_name: self.original_name.clone(),
            owner_id: self.owner_id,
            folder_id: self.folder_id,
            size: self.size,
            formatted_size: format_file_size(self.size),
            file_type: self.file_type.clone(),
            uploaded_at: self.uploaded_at.clone(),
            updated_at: self.updated_at.clone(),
            has_preview,
            preview_url,
            thumb_url,
            download_url: format!("/api/files/{}/download", self.id),
            media_url: format!("/api/files/{}/media", self.id),
            deleted_at: self.deleted_at.clone(),
        }
    }
}

pub fn format_file_size(size: i64) -> String {
    if size < 1024 {
        format!("{} B", size)
    } else if size < 1024 * 1024 {
        format!("{:.1} KB", size as f64 / 1024.0)
    } else if size < 1024 * 1024 * 1024 {
        format!("{:.1} MB", size as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1} GB", size as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}
