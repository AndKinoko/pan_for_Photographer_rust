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

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_file(preview: Option<&str>, thumb: Option<&str>) -> File {
        File {
            id: 9,
            name: "DSC_0001.NEF".into(),
            original_name: "DSC_0001.NEF".into(),
            stored_path: "user_1/abc.nef".into(),
            preview_path: preview.map(|s| s.to_string()),
            thumb_path: thumb.map(|s| s.to_string()),
            owner_id: 1,
            folder_id: None,
            size: 25_000_000,
            file_type: "nef".into(),
            uploaded_at: "2026-01-01 00:00:00".into(),
            updated_at: "2026-01-01 00:00:00".into(),
            deleted_at: None,
        }
    }

    #[test]
    fn format_file_size_boundaries() {
        assert_eq!(format_file_size(0), "0 B");
        assert_eq!(format_file_size(1023), "1023 B");
        assert_eq!(format_file_size(1024), "1.0 KB");
        assert_eq!(format_file_size(1024 * 1024 - 1), "1024.0 KB");
        assert_eq!(format_file_size(1024 * 1024), "1.0 MB");
        assert_eq!(format_file_size(1024 * 1024 * 1024 - 1), "1024.0 MB");
        assert_eq!(format_file_size(1024 * 1024 * 1024), "1.0 GB");
    }

    #[test]
    fn to_info_without_preview_hides_media_urls() {
        let info = sample_file(None, None).to_info();
        assert!(!info.has_preview);
        assert!(info.preview_url.is_none());
        assert!(info.thumb_url.is_none());
        assert_eq!(info.download_url, "/api/files/9/download");
        assert_eq!(info.media_url, "/api/files/9/media");
    }

    #[test]
    fn to_info_with_preview_falls_back_thumb_to_preview() {
        let info = sample_file(Some("user_1/previews/a.jpg"), None).to_info();
        assert!(info.has_preview);
        assert_eq!(info.preview_url.as_deref(), Some("/api/files/9/media?preview=1"));
        assert_eq!(info.thumb_url, info.preview_url);
    }

    #[test]
    fn to_info_with_thumb_prefers_thumb_url() {
        let info = sample_file(Some("user_1/previews/a.jpg"), Some("user_1/previews/a_thumb.jpg")).to_info();
        assert_eq!(info.thumb_url.as_deref(), Some("/api/files/9/media?thumb=1"));
    }
}
