//! 全局时间约定：后端与数据库一律使用 UTC，存储格式为 `"YYYY-MM-DD HH:MM:SS"`
//! （与 SQLite `datetime('now')` 一致）；时区转换只发生在前端展示/输入边界。
//!
//! 历史上用户有效期使用 `chrono::Local` 比较、分享有效期使用 `Utc` 比较，
//! 两者混用会在服务器与浏览器时区不一致时产生偏差，现统一收敛到本模块。

use chrono::{Duration, NaiveDateTime, Utc};

/// 数据库时间字符串格式（UTC）。
pub const DATETIME_FMT: &str = "%Y-%m-%d %H:%M:%S";

/// 当前 UTC 时间字符串，格式与 SQLite `datetime('now')` 一致。
pub fn now_utc_string() -> String {
    Utc::now().format(DATETIME_FMT).to_string()
}

/// 解析 UTC 时间字符串；兼容 `YYYY-MM-DD HH:MM:SS`、`YYYY-MM-DDTHH:MM:SS`
/// 与 `YYYY-MM-DDTHH:MM`（前端 datetime-local 直传的形态）。
pub fn parse_utc(s: &str) -> Option<NaiveDateTime> {
    let s = s.trim();
    NaiveDateTime::parse_from_str(s, DATETIME_FMT)
        .or_else(|_| NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S"))
        .or_else(|_| NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M"))
        .ok()
}

/// 当前时间之后 `hours` 小时的 UTC 时间字符串（用于生成分享过期时间）。
pub fn utc_string_after_hours(hours: i64) -> String {
    (Utc::now() + Duration::hours(hours))
        .format(DATETIME_FMT)
        .to_string()
}

/// 判断过期时间是否已到（统一按 UTC 比较）。
///
/// - 可解析时按真实时间比较；
/// - 不可解析时退化为字符串比较，保持历史宽松语义（不误伤手工写入的脏数据）。
pub fn is_expired_utc(expires_at: &str) -> bool {
    match parse_utc(expires_at) {
        Some(t) => t <= Utc::now().naive_utc(),
        None => expires_at.trim() <= now_utc_string().as_str(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn now_utc_string_matches_sqlite_format() {
        let now = now_utc_string();
        assert_eq!(now.len(), 19, "格式必须为 YYYY-MM-DD HH:MM:SS");
        assert!(parse_utc(&now).is_some());
        assert_eq!(now.as_bytes()[10], b' ');
    }

    #[test]
    fn parse_utc_accepts_supported_shapes() {
        assert!(parse_utc("2026-01-02 03:04:05").is_some());
        assert!(parse_utc("2026-01-02T03:04:05").is_some());
        assert!(parse_utc("2026-01-02T03:04").is_some());
        assert!(parse_utc("  2026-01-02 03:04:05  ").is_some());
        assert!(parse_utc("not-a-date").is_none());
        assert!(parse_utc("").is_none());
    }

    #[test]
    fn is_expired_utc_detects_past_and_future() {
        assert!(is_expired_utc("2000-01-01 00:00:00"));
        assert!(!is_expired_utc("2999-01-01 00:00:00"));
        // 与存储格式一致，但字符串比较同样成立
        assert!(is_expired_utc("1999-12-31T23:59"));
    }

    #[test]
    fn is_expired_utc_handles_garbage_without_panic() {
        let _ = is_expired_utc("");
        let _ = is_expired_utc("garbage");
    }

    #[test]
    fn utc_string_after_hours_is_in_the_future() {
        let future = utc_string_after_hours(2);
        assert!(!is_expired_utc(&future));
        let past = utc_string_after_hours(-2);
        assert!(is_expired_utc(&past));
    }
}
