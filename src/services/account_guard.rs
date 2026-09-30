//! 账号状态校验：把「这个用户现在还能不能以自己的身份操作」集中到一处。
//!
//! ## 为什么要集中
//!
//! 校验 JWT 签名（`crypto::validate_token`）只能证明「这个令牌是我们签的、
//! 且没超过 7 天有效期」，**不能**证明「签发这个令牌的用户现在还有权限」：
//!
//! - 账号到期（`users.expires_at` 已过）后，手里的令牌在 7 天内一直能解出 user_id；
//! - 账号被删除后同理（`sub` 指向的用户已不存在）。
//!
//! `middleware::auth` 的 `AuthUser` 提取器早就做了查库校验，但历史上
//! `?token=` 这条旁路（缩略图、下载链接的 `authUrl()` 会把令牌拼进 URL）
//! 直接调 `validate_token` 拿 `sub` 就放行，绕开了这道校验。
//! 两条路径实现同一件事却各写一份，差异迟早再次发生——所以提取到这里共用。

use sqlx::SqlitePool;

use crate::errors::AppError;

/// 账号状态的判定结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountStatus {
    /// 存在且未过期，可正常操作
    Active,
    /// 已过期（`expires_at` 不为 NULL 且已过）
    Expired,
    /// 用户不存在
    NotFound,
}

/// 查库判定账号是否仍然可用。
///
/// 存储与比较口径统一走 `utils::time`（全库 UTC，`YYYY-MM-DD HH:MM:SS`）。
pub async fn account_status(pool: &SqlitePool, user_id: i64) -> Result<AccountStatus, sqlx::Error> {
    // 单条查询同时覆盖「用户不存在」与「已过期」两种情况。
    let expires_at: Option<String> =
        sqlx::query_scalar("SELECT expires_at FROM users WHERE id = ?")
            .bind(user_id)
            .fetch_optional(pool)
            .await?;

    let Some(expires_at) = expires_at else {
        return Ok(AccountStatus::NotFound);
    };

    // expires_at 为 NULL 表示永久有效。
    //
    // 这里必须先判空：`fetch_optional` 作用在可空列上，SQL NULL 会被映射成
    // `Some("")`（String 的 Default），而不是 None。若不拦住空串，
    // `is_expired_utc("")` 会走字符串比较分支把「永久有效账号」判成已过期，
    // 结果是所有正常用户全部被锁在门外。已由 null_expires_at_means_never_expires 覆盖。
    if !expires_at.is_empty() && crate::utils::time::is_expired_utc(&expires_at) {
        return Ok(AccountStatus::Expired);
    }

    Ok(AccountStatus::Active)
}

/// 校验账号仍可用，否则返回对应的 `AppError`。
///
/// 与 [`account_status`] 的区别只在于把「判定」翻译成「错误」，
/// 让各 handler 直接 `?` 即可，不需要重复写 match。
pub async fn require_active_account(pool: &SqlitePool, user_id: i64) -> Result<(), AppError> {
    match account_status(pool, user_id).await {
        Ok(AccountStatus::Active) => Ok(()),
        Ok(AccountStatus::Expired) => Err(AppError::Unauthorized(
            "账号已过期，请联系管理员续期".into(),
        )),
        // 令牌签名有效但用户已不存在：与「认证失败」同等对待，
        // 不透露「这个用户曾经存在」这一信息。
        Ok(AccountStatus::NotFound) | Err(_) => Err(AppError::Unauthorized("认证失败".into())),
    }
}

/// 校验账号仍可用，且角色为管理员。
///
/// 供 `middleware::admin` 的 `AdminUser` 提取器使用——它此前只
/// `SELECT role`，漏掉了有效期，已过期管理员仍能管理全部用户。
pub async fn require_active_admin(pool: &SqlitePool, user_id: i64) -> Result<(), AppError> {
    require_active_account(pool, user_id).await?;

    let role: Option<String> = sqlx::query_scalar("SELECT role FROM users WHERE id = ?")
        .bind(user_id)
        .fetch_optional(pool)
        .await
        .map_err(|_| AppError::Internal("服务器内部错误".into()))?;

    match role.as_deref() {
        Some("admin") => Ok(()),
        _ => Err(AppError::Forbidden("需要管理员权限".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    async fn fixture() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE users (
                id INTEGER PRIMARY KEY,
                username TEXT NOT NULL UNIQUE,
                role TEXT NOT NULL DEFAULT 'user',
                expires_at DATETIME
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    async fn insert(pool: &SqlitePool, id: i64, role: &str, expires_at: Option<&str>) {
        sqlx::query("INSERT INTO users (id, username, role, expires_at) VALUES (?, ?, ?, ?)")
            .bind(id)
            .bind(format!("u{id}"))
            .bind(role)
            .bind(expires_at)
            .execute(pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn null_expires_at_means_never_expires() {
        let pool = fixture().await;
        insert(&pool, 1, "user", None).await;
        assert_eq!(account_status(&pool, 1).await.unwrap(), AccountStatus::Active);
        assert!(require_active_account(&pool, 1).await.is_ok());
    }

    #[tokio::test]
    async fn past_expires_at_is_expired() {
        let pool = fixture().await;
        insert(&pool, 2, "user", Some("2000-01-01 00:00:00")).await;
        assert_eq!(account_status(&pool, 2).await.unwrap(), AccountStatus::Expired);
        let err = require_active_account(&pool, 2).await.unwrap_err();
        assert!(matches!(err, AppError::Unauthorized(_)));
    }

    #[tokio::test]
    async fn future_expires_at_is_active() {
        let pool = fixture().await;
        insert(&pool, 3, "user", Some("2999-01-01 00:00:00")).await;
        assert_eq!(account_status(&pool, 3).await.unwrap(), AccountStatus::Active);
    }

    #[tokio::test]
    async fn missing_user_is_not_found_and_rejected() {
        let pool = fixture().await;
        assert_eq!(account_status(&pool, 99).await.unwrap(), AccountStatus::NotFound);
        assert!(require_active_account(&pool, 99).await.is_err());
    }

    #[tokio::test]
    async fn admin_check_rejects_expired_admin() {
        let pool = fixture().await;
        // 这正是缺陷 6：role 是 admin，但有效期已过
        insert(&pool, 4, "admin", Some("2000-01-01 00:00:00")).await;
        let err = require_active_admin(&pool, 4).await.unwrap_err();
        assert!(matches!(err, AppError::Unauthorized(_)), "过期管理员应被拒");
    }

    #[tokio::test]
    async fn admin_check_rejects_active_non_admin() {
        let pool = fixture().await;
        insert(&pool, 5, "user", None).await;
        let err = require_active_admin(&pool, 5).await.unwrap_err();
        assert!(matches!(err, AppError::Forbidden(_)));
    }

    #[tokio::test]
    async fn admin_check_accepts_active_admin() {
        let pool = fixture().await;
        insert(&pool, 6, "admin", None).await;
        assert!(require_active_admin(&pool, 6).await.is_ok());
    }
}
