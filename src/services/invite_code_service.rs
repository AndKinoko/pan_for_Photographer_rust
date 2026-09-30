//! 注册邀请码：管理员生成、注册时消费、用后即废。
//!
//! 存在的理由有两条，缺一不可：
//! 1. **公网 DoS**——公开注册接口在新用户默认 5GB 配额下，等于允许任何人
//!    无限创建账号吃满磁盘。限流能缓解，但根治靠「能不能注册」这件事本身。
//! 2. **交付语义**——摄影师的网盘里装的是客户照片，「谁来用这个网盘」
//!    本该由摄影师决定，而不是任何拿到域名的人自助注册。
//!
//! 设计上的三个取舍：
//! - **码存明文不存哈希**。它不是密码，是要抄进微信发给客户的凭据，
//!   摄影师需要在管理界面重新看到它。安全性来自「这张表只有管理员能读」。
//! - **消费与建号必须同一个事务**。否则并发用同一个码注册会超发，
//!   和下载额度用 `UPDATE ... WHERE count < max` 原子占用是同一个道理。
//! - **错误信息区分「不存在 / 用完 / 过期」**。这些都是摄影师自己生成的码，
//!   客户拿错码时需要知道是哪种情况才好反馈，统一一句「无效」只会让人猜。

use serde::Serialize;
use sqlx::SqlitePool;

use crate::errors::AppError;
use crate::models::user::User;
use crate::utils::time;

/// 邀请码字母表：去掉 `0/O`、`1/I/l` 这类手抄最容易认错的字符。
/// 摄影师要把它念给对方或贴在聊天窗口里，可用性直接决定会不会被输错。
const CODE_ALPHABET: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";
const CODE_LEN: usize = 10;

/// 一次生成 `count` 个码。批量是主用法：交付时常常要一次给几个客户。
pub async fn create_codes(
    pool: &SqlitePool,
    created_by: i64,
    count: i64,
    max_uses: i64,
    expires_hours: Option<i64>,
    note: &str,
) -> Result<Vec<InviteCode>, AppError> {
    if count < 1 {
        return Err(AppError::BadRequest("生成数量至少为 1".into()));
    }
    if count > 100 {
        return Err(AppError::BadRequest("一次最多生成 100 个".into()));
    }
    if max_uses < 1 {
        return Err(AppError::BadRequest("每个码至少允许使用 1 次".into()));
    }
    if let Some(h) = expires_hours {
        if h <= 0 {
            return Err(AppError::BadRequest("有效小时数必须大于 0".into()));
        }
    }
    if note.len() > 200 {
        return Err(AppError::BadRequest("备注不能超过 200 字".into()));
    }

    let expires_at = expires_hours.map(time::utc_string_after_hours);
    let mut out = Vec::new();

    for _ in 0..count {
        // UNIQUE 冲突只可能是随机碰撞（字母表 31 字符、取 10 位），
        // 重试几次即可，不值得为它引入整批事务的复杂度。
        let mut inserted = None;
        for _ in 0..8 {
            let code = random_code();
            let res = sqlx::query_as::<_, InviteCode>(
                r#"
                INSERT INTO invite_codes (code, created_by, expires_at, max_uses, note)
                VALUES (?, ?, ?, ?, ?)
                RETURNING *
                "#,
            )
            .bind(&code)
            .bind(created_by)
            .bind(expires_at.as_deref())
            .bind(max_uses)
            .bind(note.trim())
            .fetch_one(pool)
            .await;

            match res {
                Ok(row) => {
                    inserted = Some(row);
                    break;
                }
                Err(sqlx::Error::Database(e)) if e.is_unique_violation() => continue,
                Err(e) => return Err(AppError::from(e)),
            }
        }
        match inserted {
            Some(row) => out.push(row),
            None => {
                return Err(AppError::Internal(
                    "生成邀请码失败：连续多次随机碰撞，请重试".into(),
                ))
            }
        }
    }

    Ok(out)
}

/// 列出所有邀请码，供管理界面展示。
pub async fn list_codes(pool: &SqlitePool) -> Result<Vec<InviteCode>, AppError> {
    Ok(sqlx::query_as::<_, InviteCode>("SELECT * FROM invite_codes ORDER BY id DESC")
        .fetch_all(pool)
        .await?)
}

/// 删除一个邀请码。
///
/// 删已使用的码只是让列表干净，不影响已创建的账号——账号归属由
/// `users` 表决定，删掉记录不会把人变成「无来源的用户」。
pub async fn delete_code(pool: &SqlitePool, id: i64) -> Result<(), AppError> {
    let res = sqlx::query("DELETE FROM invite_codes WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::NotFound("邀请码不存在".into()));
    }
    Ok(())
}

/// 校验一个邀请码此刻是否可用于注册（**不消费**）。
///
/// 注册页要先验码、再让用户填用户名密码——否则用户填完一整页才被告知
/// 「码无效」，白填一遍。这是「先验后用」的第一步。
pub async fn validate_code(pool: &SqlitePool, code: &str) -> Result<InviteCode, AppError> {
    let row = sqlx::query_as::<_, InviteCode>("SELECT * FROM invite_codes WHERE code = ?")
        .bind(normalize(code))
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| AppError::BadRequest("邀请码不存在".into()))?;

    check_usable(&row)?;
    Ok(row)
}

/// 建号并消费邀请码，**在同一个事务里**。
///
/// 这是注册路径上唯一允许创建普通用户的入口。事务保证「码被占用」与
/// 「账号被创建」要么都成功要么都回滚，避免出现「码消耗了但账号没建成」
/// 或「同一码建出两个账号」。
///
/// 占用用带条件的 UPDATE（`used_count < max_uses AND 未过期`）而不是
/// 先 SELECT 后 UPDATE：后者在并发下会让两个请求同时通过检查。
pub async fn register_with_invite(
    pool: &SqlitePool,
    code: &str,
    username: &str,
    password_hash: &str,
) -> Result<User, AppError> {
    let code = normalize(code);
    let mut tx = pool.begin().await?;

    // 原子占用：条件 UPDATE 本身就完成了「检查 + 占用」两件事。
    let claimed = sqlx::query(
        r#"
        UPDATE invite_codes
        SET used_count = used_count + 1,
            used_at = datetime('now')
        WHERE code = ?
          AND used_count < max_uses
          AND (expires_at IS NULL OR expires_at > datetime('now'))
        "#,
    )
    .bind(&code)
    .execute(&mut *tx)
    .await?;

    if claimed.rows_affected() == 0 {
        // 条件没命中，需要区分具体原因给用户一句有用的话。
        // 事务尚未做任何写入，直接回滚即可。
        let existing = sqlx::query_as::<_, InviteCode>(
            "SELECT * FROM invite_codes WHERE code = ?",
        )
        .bind(&code)
        .fetch_optional(&mut *tx)
        .await?;

        let err = match existing {
            None => AppError::BadRequest("邀请码不存在".into()),
            Some(ref r) => check_usable(r).unwrap_err(),
        };
        tx.rollback().await?;
        return Err(err);
    }

    // 建号。注意不写 role —— 走数据库默认的 'user'，
    // 自助注册无法提权成管理员（见 models/schema 的 role 列）。
    let created = sqlx::query_as::<_, User>(
        "INSERT INTO users (username, password_hash) VALUES (?, ?) RETURNING *",
    )
    .bind(username)
    .bind(password_hash)
    .fetch_one(&mut *tx)
    .await;

    let user = match created {
        Ok(u) => u,
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
            tx.rollback().await?;
            return Err(AppError::Conflict("用户名已存在".into()));
        }
        Err(e) => {
            let e = AppError::from(e);
            tx.rollback().await?;
            return Err(e);
        }
    };

    // 回填「谁用了这个码」：管理界面要能据此回答「这个客户是谁」。
    sqlx::query("UPDATE invite_codes SET used_by = ? WHERE code = ?")
        .bind(user.id)
        .bind(&code)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;
    Ok(user)
}

/// 邀请码的对外表示。
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct InviteCode {
    pub id: i64,
    pub code: String,
    pub created_by: Option<i64>,
    pub created_at: String,
    pub expires_at: Option<String>,
    pub max_uses: i64,
    pub used_count: i64,
    pub used_by: Option<i64>,
    pub used_at: Option<String>,
    pub note: String,
}

impl InviteCode {
    /// 是否还能使用（未用完且未过期）。管理界面拿它决定是否置灰。
    pub fn is_usable(&self) -> bool {
        check_usable(self).is_ok()
    }
}

/// 校验一个已取出的邀请码当前是否可用，把原因说清楚。
fn check_usable(row: &InviteCode) -> Result<(), AppError> {
    if row.used_count >= row.max_uses {
        return Err(AppError::BadRequest("邀请码已被使用".into()));
    }
    if let Some(ref exp) = row.expires_at {
        if time::is_expired_utc(exp) {
            return Err(AppError::BadRequest("邀请码已过期".into()));
        }
    }
    Ok(())
}

/// 归一化用户输入：去空白、转大写。
///
/// 让客户可以照着截图输 `a1b2c3`，哪怕是手抄时带上了空格或小写。
/// 这是把「抄写摩擦」从注册成功率里扣掉。
fn normalize(code: &str) -> String {
    code.trim().to_uppercase()
}

/// 生成一个随机邀请码。
///
/// 熵来自 `uuid::Uuid::new_v4()`——它内部走操作系统 CSPRNG
/// （Linux 的 `getrandom(2)`、Windows 的 `BCryptGenRandom`），
/// 而 uuid 本来就是本项目的既有依赖，不引入新的信任假设。
/// 122 bit 熵远超 10 位字母表所需，只取其中若干位做均匀映射。
fn random_code() -> String {
    let bytes = uuid::Uuid::new_v4();
    (0..CODE_LEN)
        .map(|i| {
            // 逐字节取，避免只用 uuid 的 4 个低位字节（熵浪费）
            let b = bytes.as_bytes()[i % 16];
            CODE_ALPHABET[b as usize % CODE_ALPHABET.len()] as char
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_code_has_expected_shape() {
        let c = random_code();
        assert_eq!(c.len(), CODE_LEN, "长度必须固定，否则抄写容易出错");
        assert!(
            c.chars().all(|ch| CODE_ALPHABET.contains(&(ch as u8))),
            "只能使用去掉了易混字符的字母表: {c}"
        );
    }

    #[test]
    fn random_codes_do_not_repeat_in_a_small_batch() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..500 {
            assert!(seen.insert(random_code()), "短码空间下不应连续碰撞");
        }
    }

    #[test]
    fn alphabet_excludes_ambiguous_glyphs() {
        // 手抄场景里最容易认错的几个字符
        for bad in ['0', 'O', '1', 'I', 'l'] {
            assert!(
                !CODE_ALPHABET.contains(&(bad as u8)),
                "{bad} 不应出现在字母表里"
            );
        }
    }

    #[test]
    fn normalize_accepts_pasted_variants() {
        assert_eq!(normalize("  ab-cd  "), "AB-CD");
        assert_eq!(normalize("AbCd"), "ABCD");
    }

    #[test]
    fn check_usable_distinguishes_exhausted_from_expired() {
        let mut row = InviteCode {
            id: 1,
            code: "AAAAAAAAAA".into(),
            created_by: Some(1),
            created_at: "2026-01-01 00:00:00".into(),
            expires_at: None,
            max_uses: 1,
            used_count: 1,
            used_by: None,
            used_at: None,
            note: String::new(),
        };
        assert!(!row.is_usable());
        assert!(
            check_usable(&row).unwrap_err().message().contains("已被使用"),
            "用完后应说明已被使用"
        );

        row.used_count = 0;
        row.expires_at = Some("2000-01-01 00:00:00".into());
        assert!(!row.is_usable());
        assert!(
            check_usable(&row).unwrap_err().message().contains("过期"),
            "过期与用完是不同的原因，要分开提示"
        );

        row.expires_at = Some("2999-01-01 00:00:00".into());
        assert!(row.is_usable());
    }
}
