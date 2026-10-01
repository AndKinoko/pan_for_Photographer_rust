use serde::{Deserialize, Serialize};
use sqlx::FromRow;

/// 批次里的条目类型。
///
/// 用枚举而不是裸字符串：`share_items.item_type` 上有 CHECK 约束只允许
/// `'file'` / `'folder'`，在 Rust 侧也照着约束收口，就不用到处防「数据库里
/// 出现了第三种值」——那种分支写出来也没法测。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ItemKind {
    File,
    Folder,
}

impl ItemKind {
    /// 落库用的字面量，必须与 `share_items` 的 CHECK 约束一致。
    pub fn as_str(self) -> &'static str {
        match self {
            ItemKind::File => "file",
            ItemKind::Folder => "folder",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "file" => Some(ItemKind::File),
            "folder" => Some(ItemKind::Folder),
            _ => None,
        }
    }
}

/// `file_shares` 的一行 —— 只有**批次本身**的属性。
///
/// 指向什么内容不在这里，在 [`ShareItem`]：一个批次可以同时装多个文件和多个
/// 文件夹。历史上这里是 `file_id` + `folder_id` 两个互斥的可空列，
/// 只能表达「一个分享 → 一个目标」。
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct FileShare {
    pub id: String,
    pub owner_id: i64,
    pub created_at: String,
    pub expires_at: Option<String>,
    pub password_hash: String,
    pub download_count: i64,
    pub max_downloads: Option<i64>,
    pub is_active: i64,
    pub custom_code: Option<String>,
}

/// 批次 ↔ 内容 的关联行。
#[derive(Debug, Clone, FromRow)]
pub struct ShareItem {
    pub share_id: String,
    pub item_type: String,
    pub item_id: i64,
}

impl ShareItem {
    pub fn kind(&self) -> Option<ItemKind> {
        ItemKind::parse(&self.item_type)
    }
}

/// 对外表示的一个条目。
///
/// 文件与文件夹用同一个结构体，靠 `item_type` 区分。前端 `FileCard` 本来就
/// 同时渲染这两种（它靠 `kind` prop 分支），所以这里对齐它的输入形状，
/// 而不是拆成两个结构体让前端再合并一次。
#[derive(Debug, Serialize)]
pub struct ShareItemInfo {
    /// `"file"` 或 `"folder"`
    pub item_type: String,
    pub id: i64,
    pub name: String,
    /// 文件字节数；文件夹为 0（文件夹的递归体积见 `ShareInfo::total_size`）
    pub size: i64,
    pub formatted_size: String,
    /// 扩展名（小写）；文件夹为 `"folder"`
    pub file_type: String,
    /// 仅文件有
    pub uploaded_at: Option<String>,
    pub has_preview: bool,
    /// 未解锁（受密码保护且访客尚未验码）时一律为 `None`
    pub preview_url: Option<String>,
    pub thumb_url: Option<String>,
    /// 仅文件有；文件夹为 `None`（先点进去再挑里面的文件下）
    pub download_url: Option<String>,
    /// 仅文件夹有：直接子级计数
    pub file_count: Option<i64>,
    pub subfolder_count: Option<i64>,
}

/// 请求里指定的一个批次条目。
///
/// `type` 是 JSON 字段名（`type` 在 Rust 里是关键字，所以字段叫 `item_type`）。
/// 这里刻意不在反序列化阶段就拒绝非法值——收口放在业务层做，
/// 这样错误信息能统一成「条目类型必须是 file 或 folder」，
/// 而不是 serde 抛出的 `unknown variant` 之类。
#[derive(Debug, Clone, Deserialize)]
pub struct ShareItemRef {
    #[serde(rename = "type")]
    pub item_type: String,
    pub id: i64,
}

impl ShareItemRef {
    pub fn kind(&self) -> Option<ItemKind> {
        ItemKind::parse(self.item_type.trim())
    }
}

/// 公开侧的面包屑项。
///
/// 与 `models::folder::Folder` 分开定义，是因为公开侧**只需要** id 和 name：
/// 多带 `owner_id` / `created_at` / `deleted_at` 之类字段没有用处，
/// 而每一个多余字段都是一次「以后有人顺手用了它」的机会。
#[derive(Debug, Serialize)]
pub struct Crumb {
    pub id: i64,
    pub name: String,
}

/// 分享（批次）的对外表示。
#[derive(Debug, Serialize)]
pub struct ShareInfo {
    pub id: String,
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
    pub custom_code: Option<String>,

    /// 批次顶层包含的条目。受密码保护且未解锁时不带媒体 URL。
    pub items: Vec<ShareItemInfo>,
    /// 顶层条目总数（= `items.len()`，单独给一份让前端不必再算）
    pub item_count: i64,
    /// 顶层文件数 / 文件夹数
    pub file_count: i64,
    pub folder_count: i64,

    /// 递归统计：批次内所有文件夹展开后的文件总数。
    /// 列表接口也会算（一次查询覆盖所有分享），因为「3 个文件夹 · 47 个文件」
    /// 这种摘要是列表页唯一能说清楚「这批交付了什么」的东西。
    pub total_file_count: i64,
    /// 递归文件夹总数（含批次里直接列出的那些）
    pub total_folder_count: i64,
    /// 递归总字节。
    ///
    /// 与 `file_service::used_bytes` 的口径**故意不同**：那个统计的是
    /// 「这个用户占了磁盘多少」，所以按 `stored_path` 去重（批量复制产生的
    /// 副本共享物理文件，不能重复计）；这里回答的是「客户要下走多少东西」，
    /// 批次里列了几个文件就算几个。两个问题不同，不该强行统一。
    pub total_size: i64,
    pub formatted_size: String,
}
