//! 游标分页（keyset pagination）。
//!
//! ## 为什么不用 `LIMIT`/`OFFSET`
//!
//! 三个列表都按时间倒序排列（文件按 `uploaded_at DESC`、回收站按 `deleted_at DESC`），
//! 而本项目的使用场景就是「一边浏览一边有人在上传」。用 `OFFSET` 时，翻页途中只要有新行
//! 插到列表头部，整个窗口就向后平移一格 —— 第二页的第一条会与第一页的最后一条重复。
//! 游标记的是「上一页最后一条的排序键」，新行插在它前面不影响后续页，
//! 删除也不影响（已取过的页不会回头补偿，但那本来就是「已经看过」的部分）。
//!
//! ## 为什么不用「页码 + 总数跳页」
//!
//! 跳页需要一个稳定的偏移量，而偏移量在并发写入下没有稳定含义。
//! 产品上要的是「继续往下看」，不是「跳到第 7 页」，所以选了游标。
//! 总数仍然返回（见各 handler），只是不用它来定位。
//!
//! ## 游标为什么不需要签名
//!
//! 所有查询的 `WHERE` 里都有 `owner_id = ?`，游标只影响「从这个位置继续」。
//! 篡改游标最多只能改变**自己**列表的起点，取不到别人的数据。
//! 因此不做 HMAC —— 那会让游标无法跨进程重启复用，收益却是零。
//!
//! ## 游标为什么用 hex 而不是 base64 或百分号编码
//!
//! 游标要经过 URL query 参数往返，而排序键可能是中文文件名。base64 与百分号编码
//! 都需要处理转义/补齐/大小写，每一个都是容易写错又难测的地方。hex 只用 `0-9a-f`，
//! 天然 URL 安全、无需转义、可逆且无歧义。代价是游标长一倍，而游标本来就不该被用户读。

use crate::errors::AppError;

/// 未指定 `limit` 时每页返回的条数。
/// 一屏约 20–25 张卡片，100 条约 4–5 屏，首屏 JSON 约 20KB。
pub const DEFAULT_LIMIT: i64 = 100;

/// `limit` 的上限。防止 `?limit=999999999` 把「分页」变成一次全量导出。
pub const MAX_LIMIT: i64 = 500;

/// 排序方向。决定游标条件用 `>` 还是 `<`，以及 `ORDER BY` 的方向。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    Asc,
    Desc,
}

impl Dir {
    pub fn sql(self) -> &'static str {
        match self {
            Dir::Asc => "ASC",
            Dir::Desc => "DESC",
        }
    }

    /// 游标比较用的运算符：升序时「排在游标之后」是 `>`，降序是 `<`。
    pub fn op(self) -> &'static str {
        match self {
            Dir::Asc => ">",
            Dir::Desc => "<",
        }
    }
}

/// 游标里携带的排序键值。
///
/// 必须按列的真实类型绑定，不能一律当字符串：
/// `size` 是 INTEGER，绑成 TEXT 时 SQLite 虽然会按列亲和做隐式转换、
/// 结果通常没错，但那样比较无法使用 `size` 上的索引，而且在不同 SQLite
/// 版本上对类型混合比较的处理需要靠亲和规则推理，属于没必要承担的风险。
#[derive(Debug, Clone, PartialEq)]
pub enum Bind {
    Text(String),
    Int(i64),
}

/// 解析出来的游标载荷。
#[derive(Debug, Clone, PartialEq)]
pub struct Cursor {
    /// 排序标识。必须与本次请求的排序一致，否则拒绝——
    /// 用「按 uploaded_at 取到的游标」去请求「按 name 排序」的下一页，
    /// 语义上是无意义的，静默返回一批乱序结果比报错更糟。
    pub sort: String,
    /// 排序键的原始文本形态（Int 类型就是十进制串）
    pub raw: String,
    /// 并列时的唯一次序依据
    pub id: i64,
}

const CURSOR_VERSION: &str = "v1";

fn to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

fn from_hex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(s.len() / 2);
    for pair in b.chunks(2) {
        let hi = (pair[0] as char).to_digit(16)?;
        let lo = (pair[1] as char).to_digit(16)?;
        out.push((hi * 16 + lo) as u8);
    }
    Some(out)
}

impl Cursor {
    pub fn new(sort: &str, key: &str, id: i64) -> Self {
        Cursor {
            sort: sort.to_string(),
            raw: key.to_string(),
            id,
        }
    }

    /// 排序键为文本列时取值（uploaded_at / deleted_at / name）
    pub fn as_text(&self) -> String {
        self.raw.clone()
    }

    /// 排序键为整数列时取值（size）
    pub fn as_int(&self) -> Result<i64, AppError> {
        self.raw
            .parse::<i64>()
            .map_err(|_| AppError::BadRequest("分页游标已损坏，请重新加载列表".into()))
    }

    pub fn encode(&self) -> String {
        // 值放在最后一段：排序键可能是文件名，里面出现 `.` 也不会破坏结构
        format!(
            "{}.{}.{}.{}",
            CURSOR_VERSION,
            self.sort,
            self.id,
            to_hex(self.raw.as_bytes())
        )
    }

    pub fn decode(s: &str) -> Result<Cursor, AppError> {
        let bad = || AppError::BadRequest("分页游标格式不正确，请重新加载列表".into());

        // splitn(4) 让最后一段吃掉剩余的所有内容（hex 里不会出现 `.`，
        // 用 splitn 是为了万一将来分隔符变化时行为仍然确定）
        let parts: Vec<&str> = s.splitn(4, '.').collect();
        if parts.len() != 4 || parts[0] != CURSOR_VERSION {
            return Err(bad());
        }
        let id = parts[2].parse::<i64>().map_err(|_| bad())?;
        let bytes = from_hex(parts[3]).ok_or_else(bad)?;
        let raw = String::from_utf8(bytes).map_err(|_| bad())?;

        Ok(Cursor {
            sort: parts[1].to_string(),
            raw,
            id,
        })
    }
}

/// 归一化 `limit`。`None`、0、负数都视为「未指定」，并夹到 [`MAX_LIMIT`]。
pub fn normalize_limit(raw: Option<i64>) -> i64 {
    match raw {
        Some(n) if n > 0 => n.min(MAX_LIMIT),
        _ => DEFAULT_LIMIT,
    }
}

/// 解析可选游标，并校验它与本次排序一致。
///
/// `expected_sort` 必须来自本模块/调用方的常量白名单，不能是用户输入。
pub fn parse_cursor(raw: Option<&str>, expected_sort: &str) -> Result<Option<Cursor>, AppError> {
    let Some(s) = raw.filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    let c = Cursor::decode(s)?;
    if c.sort != expected_sort {
        return Err(AppError::BadRequest(
            "分页游标与当前排序方式不匹配，请重新加载列表".into(),
        ));
    }
    Ok(Some(c))
}

/// 追加 keyset 条件。
///
/// `col` 必须来自调用方的常量白名单（`"uploaded_at"` / `"deleted_at"` / `"size"` /
/// `"name"`），绝不能是用户输入 —— 这里是字符串拼接。
///
/// **`id` 这一层不能省。** `uploaded_at` 是 `datetime('now')`，只有秒精度，
/// 而批量上传 200 个文件会落在同一秒里。只按 `uploaded_at` 比较时，
/// 并列的那些行没有确定的先后，翻页会漏项或重复。
pub fn push_cursor_condition(sql: &mut String, col: &str, dir: Dir) {
    let op = dir.op();
    sql.push_str(&format!(" AND ({col} {op} ? OR ({col} = ? AND id {op} ?))"));
}

/// `ORDER BY` 子句。同样必须带上 `id` 兜底，理由见 [`push_cursor_condition`]。
pub fn order_by(col: &str, dir: Dir) -> String {
    format!(" ORDER BY {col} {}, id {}", dir.sql(), dir.sql())
}

/// 把一页结果切成「这一页」与「下一页的游标」。
///
/// 约定：查询时取 `limit + 1` 条，多出来的那条只用来判断还有没有下一页，
/// 不返回给客户端。这样不需要额外的 `COUNT` 就能知道 `has_more`。
///
/// 返回 `(本页, next_cursor)`；`next_cursor` 为 `None` 表示已到末尾。
pub fn split_page<T, F>(
    mut rows: Vec<T>,
    limit: i64,
    sort: &str,
    key_of: F,
) -> (Vec<T>, Option<String>)
where
    F: Fn(&T) -> (String, i64),
{
    let has_more = rows.len() as i64 > limit;
    if has_more {
        rows.truncate(limit as usize);
    }
    let next = if has_more {
        rows.last().map(|last| {
            let (key, id) = key_of(last);
            Cursor::new(sort, &key, id).encode()
        })
    } else {
        None
    };
    (rows, next)
}

/// 一页结果。`total` 是该过滤条件下的总条数（不受分页影响），
/// 前端拿它显示「共 N 张」并判断是否还有内容。
#[derive(Debug, Clone)]
pub struct Paged<T> {
    pub items: Vec<T>,
    pub total: i64,
    /// 为 `None` 表示已到末尾
    pub next_cursor: Option<String>,
}

impl<T> Paged<T> {
    pub fn has_more(&self) -> bool {
        self.next_cursor.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limit_defaults_and_clamps() {
        assert_eq!(normalize_limit(None), DEFAULT_LIMIT);
        assert_eq!(normalize_limit(Some(0)), DEFAULT_LIMIT);
        assert_eq!(normalize_limit(Some(-5)), DEFAULT_LIMIT);
        assert_eq!(normalize_limit(Some(50)), 50);
        assert_eq!(normalize_limit(Some(MAX_LIMIT)), MAX_LIMIT);
        // 超过上限必须被夹住，否则 ?limit=999999 就等于取消了分页
        assert_eq!(normalize_limit(Some(MAX_LIMIT + 1)), MAX_LIMIT);
        assert_eq!(normalize_limit(Some(i64::MAX)), MAX_LIMIT);
    }

    #[test]
    fn cursor_roundtrip_ascii() {
        let c = Cursor::new("uploaded_at", "2026-09-26 13:00:00", 42);
        let enc = c.encode();
        assert_eq!(enc, "v1.uploaded_at.42.323032362d30392d32362031333a30303a3030");
        let dec = Cursor::decode(&enc).unwrap();
        assert_eq!(dec, c);
        assert_eq!(dec.as_text(), "2026-09-26 13:00:00");
    }

    #[test]
    fn cursor_roundtrip_unicode_name_with_dots() {
        // 中文名 + 文件名里的点号：值放在最后一段，点号不影响解析
        let c = Cursor::new("name", "婚礼跟拍 2026.09.26 精选.NEF", 7);
        let dec = Cursor::decode(&c.encode()).unwrap();
        assert_eq!(dec, c);
        assert_eq!(dec.as_text(), "婚礼跟拍 2026.09.26 精选.NEF");
    }

    #[test]
    fn cursor_is_url_safe() {
        let enc = Cursor::new("name", "婚礼跟拍 2026.09.26 精选.NEF", 7).encode();
        // 只允许 0-9a-zA-Z._- ，否则放进 query 参数还要再转义
        assert!(
            enc.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-'),
            "游标含非法字符: {enc}"
        );
    }

    #[test]
    fn cursor_int_roundtrip() {
        let c = Cursor::new("size", &(25_000_000i64).to_string(), 3);
        let dec = Cursor::decode(&c.encode()).unwrap();
        assert_eq!(dec.as_int().unwrap(), 25_000_000);
    }

    #[test]
    fn malformed_cursors_are_rejected() {
        for bad in [
            "",
            "v1",
            "v1.uploaded_at",
            "v1.uploaded_at.42",
            "v2.uploaded_at.42.61",       // 版本不符
            "v1.uploaded_at.abc.61",      // id 不是数字
            "v1.uploaded_at.42.6",        // hex 长度为奇数
            "v1.uploaded_at.42.zz",       // 非 hex 字符
            "v1.uploaded_at.42.ff",       // 非法 UTF-8
        ] {
            assert!(
                Cursor::decode(bad).is_err(),
                "本应被拒绝的游标被接受了: {bad:?}"
            );
        }
    }

    #[test]
    fn cursor_sort_must_match_request() {
        let enc = Cursor::new("uploaded_at", "2026-09-26 13:00:00", 1).encode();
        // 同一个游标用在同一排序上没问题
        assert!(parse_cursor(Some(&enc), "uploaded_at").unwrap().is_some());
        // 换了排序就必须拒绝，否则会静默返回一批位置无意义的结果
        assert!(parse_cursor(Some(&enc), "name").is_err());
        assert!(parse_cursor(Some(&enc), "size").is_err());
    }

    #[test]
    fn absent_cursor_is_none() {
        assert!(parse_cursor(None, "uploaded_at").unwrap().is_none());
        // 前端把「没有更多」表达成空串，要当「没有游标」而不是报错
        assert!(parse_cursor(Some(""), "uploaded_at").unwrap().is_none());
    }

    #[test]
    fn cursor_condition_carries_id_tiebreaker() {
        // 只在片段本身上计数，避免把调用方 SQL 里已有的 `?` 一并算进来
        let mut frag = String::new();
        push_cursor_condition(&mut frag, "uploaded_at", Dir::Desc);
        // 三个占位符：值比较、值相等判断、id 比较
        assert_eq!(frag.matches('?').count(), 3, "片段: {frag}");
        assert!(frag.contains("uploaded_at < ?"));
        assert!(frag.contains("uploaded_at = ? AND id < ?"));

        // 升序时运算符反向
        let mut frag = String::new();
        push_cursor_condition(&mut frag, "name", Dir::Asc);
        assert!(frag.contains("name > ?"));
        assert!(frag.contains("name = ? AND id > ?"));

        // **整段必须被括号包住。** 搜索的 WHERE 末尾是
        // `... AND (name LIKE ? OR original_name LIKE ?)`，
        // 若游标条件不包裹，就成了 `... AND (a OR b) AND c OR d`，
        // 末尾的 `OR (col = ? AND id < ?)` 会绕过 owner_id 等所有前置条件 ——
        // 那是越权级别的错误，而不是简单的排序错误。
        for dir in [Dir::Asc, Dir::Desc] {
            let mut frag = String::new();
            push_cursor_condition(&mut frag, "uploaded_at", dir);
            assert!(
                frag.starts_with(" AND (") && frag.ends_with(')'),
                "{dir:?} 的游标片段没有被括号包裹: {frag}"
            );
            // 括号必须配平
            assert_eq!(
                frag.matches('(').count(),
                frag.matches(')').count(),
                "括号不配平: {frag}"
            );
        }
    }

    #[test]
    fn order_by_is_total() {
        // 必须同时按排序键和 id 排序，否则并列行的顺序不确定，
        // 而 keyset 分页的正确性完全依赖「顺序是全序」
        assert_eq!(
            order_by("uploaded_at", Dir::Desc),
            " ORDER BY uploaded_at DESC, id DESC"
        );
        assert_eq!(order_by("name", Dir::Asc), " ORDER BY name ASC, id ASC");
    }

    #[test]
    fn split_page_marks_more_and_builds_cursor() {
        // 取 limit+1 条 → 说明还有下一页；返回 limit 条
        let rows: Vec<(String, i64)> = (0..4)
            .map(|i| (format!("t{i}"), 100 - i))
            .collect();
        let (page, next) = split_page(rows, 3, "uploaded_at", |r| (r.0.clone(), r.1));
        assert_eq!(page.len(), 3);
        assert_eq!(page.last().unwrap().1, 98);
        let c = Cursor::decode(next.as_deref().unwrap()).unwrap();
        assert_eq!(c.id, 98);
        assert_eq!(c.as_text(), "t2");

        // 不足 limit+1 → 已到末尾
        let rows: Vec<(String, i64)> = vec![("a".into(), 1), ("b".into(), 2)];
        let (page, next) = split_page(rows, 3, "uploaded_at", |r| (r.0.clone(), r.1));
        assert_eq!(page.len(), 2);
        assert!(next.is_none());
    }

    #[test]
    fn split_page_exactly_limit_is_the_end() {
        // 恰好 limit 条：因为查询取的是 limit+1 条，返回条数 ≤ limit 就说明没有更多了。
        // 多取那一条正是为了不做额外 COUNT 也能确定「是否到底」——
        // 返回 limit 条时既可能是「刚好最后一页」也可能是「还有下一页」，
        // 只有多取一条才能把两者区分开。这里属于前者。
        let rows: Vec<(String, i64)> = (0..3).map(|i| (format!("t{i}"), i)).collect();
        let (page, next) = split_page(rows, 3, "uploaded_at", |r| (r.0.clone(), r.1));
        assert_eq!(page.len(), 3);
        assert!(next.is_none());
    }
}
