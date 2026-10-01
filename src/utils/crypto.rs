use bcrypt::{hash, verify, BcryptError, DEFAULT_COST};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::Semaphore;

use crate::config::Config;
use crate::errors::AppError;

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    pub sub: i64,       // 用户ID
    pub username: String,
    pub exp: usize,     // 过期时间戳
    pub iat: usize,     // 签发时间戳
}

// ===========================================================================
// 密码哈希
// ---------------------------------------------------------------------------
// bcrypt 是**纯 CPU 的同步函数**，`DEFAULT_COST = 12` 下一次约 250ms。
// 直接在 async handler 里调用它，会把一个 tokio worker 线程整段占死——
// worker 数等于 CPU 核数，于是在家用 2~4 核机器上，几次并发登录就能
// 让整个 API 停止响应；而**批量分享给 500 个文件逐个哈希**（每个文件一次）
// 能让服务端一口气停顿约两分钟，这是普通用户点一下就能触发的。
//
// 所以对外一律提供 `*_async`：先进信号量，再进 `spawn_blocking`。
// 信号量限制的是「同时在跑的哈希数」，避免大量并发把阻塞线程池打满。
// ===========================================================================

/// 同时执行的 bcrypt 运算上限。
///
/// 取 4：家用机通常 2~4 核，再多也只是互相抢 CPU；而设为 1 会让并发登录
/// 排队等待，登录延迟随并发数线性增长。4 是「不占满整机」与「不排队」的折中。
static BCRYPT_GATE: Semaphore = Semaphore::const_new(4);

/// 把一次 bcrypt 运算放到阻塞线程池里执行，并限制并发。
async fn run_bcrypt<T, F>(f: F) -> Result<T, AppError>
where
    F: FnOnce() -> Result<T, BcryptError> + Send + 'static,
    T: Send + 'static,
{
    // 许可在 spawn_blocking 全程持有：这正是「限制并发」的落点。
    let _permit = BCRYPT_GATE
        .acquire()
        .await
        .map_err(|_| AppError::Internal("密码服务不可用".into()))?;

    tokio::task::spawn_blocking(f)
        .await
        .map_err(|_| AppError::Internal("密码处理任务异常终止".into()))?
        .map_err(AppError::from)
}

/// 异步哈希密码。**所有 handler 都必须用这个**，不要直接用 `hash_password`。
pub async fn hash_password_async(password: &str) -> Result<String, AppError> {
    let password = password.to_string();
    run_bcrypt(move || hash(&password, DEFAULT_COST)).await
}

/// 异步校验密码。理由同 [`hash_password_async`]。
pub async fn verify_password_async(password: &str, hash: &str) -> Result<bool, AppError> {
    let password = password.to_string();
    let hash = hash.to_string();
    run_bcrypt(move || verify(&password, &hash)).await
}

/// 同步哈希。**仅供启动期（种子管理员）与测试使用**——
/// 那时没有并发请求，阻塞主线程无害。请求路径请用 [`hash_password_async`]。
pub fn hash_password(password: &str) -> Result<String, bcrypt::BcryptError> {
    hash(password, DEFAULT_COST)
}

/// 同步校验。使用范围同 [`hash_password`]（当前只有单元测试用得到）。
#[allow(dead_code)]
pub fn verify_password(password: &str, hash: &str) -> Result<bool, bcrypt::BcryptError> {
    verify(password, hash)
}

/// 为用户生成JWT令牌
pub fn generate_token(user_id: i64, username: &str, config: &Config) -> Result<String, jsonwebtoken::errors::Error> {
    let now = chrono::Utc::now();
    let claims = Claims {
        sub: user_id,
        username: username.to_string(),
        exp: (now + chrono::Duration::hours(24 * 7)).timestamp() as usize,
        iat: now.timestamp() as usize,
    };

    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(&config.jwt_secret),
    )
}

/// 验证并解码JWT令牌
pub fn validate_token(token: &str, config: &Config) -> Result<Claims, jsonwebtoken::errors::Error> {
    decode::<Claims>(
        token,
        &DecodingKey::from_secret(&config.jwt_secret),
        &Validation::default(),
    )
    .map(|data| data.claims)
}

// ===========================================================================
// 公开分享访问凭证（ticket）
// ---------------------------------------------------------------------------
// 受密码保护的公开分享，在用户通过 /verify 提交正确密码后，服务端签发一个
// 与 share_id 绑定、短时效（默认 2 小时）的签名凭证。下载 / 媒体接口必须
// 携带该凭证才会放行系统内容，从而避免「知道链接即可绕过密码直接下载」。
// 凭证格式："{exp_unix}:{hmac_sha256_hex}"。
// ===========================================================================

const BLOCK_SIZE: usize = 64;

/// HMAC-SHA256，仅依赖已引入的 sha2 与 hex，避免新增 crates 依赖。
fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    let mut key_arr = if key.len() > BLOCK_SIZE {
        Sha256::digest(key).to_vec()
    } else {
        key.to_vec()
    };
    key_arr.resize(BLOCK_SIZE, 0u8);

    let mut ipad = [0u8; BLOCK_SIZE];
    let mut opad = [0u8; BLOCK_SIZE];
    for i in 0..BLOCK_SIZE {
        ipad[i] = key_arr[i] ^ 0x36;
        opad[i] = key_arr[i] ^ 0x5c;
    }

    let mut inner = Sha256::new();
    inner.update(&ipad);
    inner.update(data);
    let inner_hash = inner.finalize();

    let mut outer = Sha256::new();
    outer.update(&opad);
    outer.update(inner_hash);
    let out = outer.finalize();

    let mut res = [0u8; 32];
    res.copy_from_slice(&out);
    res
}

/// 常数时间字符串比较，避免计时侧信道。
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// 为受密码保护的分享签发短时效访问凭证，ttl_secs 为有效期（秒）。
pub fn create_share_ticket(config: &Config, share_id: &str, ttl_secs: i64) -> String {
    let exp = chrono::Utc::now().timestamp() + ttl_secs;
    let payload = share_payload(share_id, exp);
    let mac = hmac_sha256(&config.jwt_secret, payload.as_bytes());
    format!("{}:{}", exp, hex::encode(mac))
}

/// 分享票据的签名载荷。
///
/// **`share:` 前缀不是装饰**：分享码可以由用户自定义（`file_shares.custom_code`），
/// 有人把码设成 `"42"` 是完全合法的。若载荷是裸的 `"{id}:{exp}"`，这条分享的票据
/// 与「user_id = 42 的媒体票据」签名载荷完全相同 —— 先取到媒体票据的人就能拿它
/// 去解开别人的受密码分享。前缀让两套凭证的载荷空间不再重叠。
fn share_payload(share_id: &str, exp: i64) -> String {
    format!("share:{}:{}", share_id, exp)
}

/// 校验分享访问凭证是否有效：绑定指定 share_id、未过期、签名一致。
pub fn verify_share_ticket(config: &Config, share_id: &str, ticket: &str) -> bool {
    let mut parts = ticket.splitn(2, ':');
    let (exp_str, mac_hex) = match (parts.next(), parts.next()) {
        (Some(e), Some(m)) => (e, m),
        _ => return false,
    };
    let exp: i64 = match exp_str.parse() {
        Ok(v) => v,
        Err(_) => return false,
    };
    if exp < chrono::Utc::now().timestamp() {
        return false;
    }
    let payload = share_payload(share_id, exp);
    let mac = hmac_sha256(&config.jwt_secret, payload.as_bytes());
    constant_time_eq(hex::encode(mac).as_bytes(), mac_hex.as_bytes())
}

// ===========================================================================
// 媒体访问凭证（media ticket）
// ---------------------------------------------------------------------------
// `<img src>` 与 `<a download>` 无法携带 Authorization 头，所以缩略图、预览图、
// 下载链接只能把凭据塞进查询串。原先塞的是 **JWT 本身**，于是：
//
//   · 每一次缩略图加载都把一枚 7 天有效期的 bearer token 送进 URL；
//   · URL 会被 Cloudflare / 反向代理的边缘日志、浏览器历史、DevTools 完整记录；
//   · 而 JWT 是全权限凭据 —— 从边缘日志里捞到一枚就能调管理端、删文件。
//
// 媒体票据是为此设计的窄口径替代品：
//   · **只对媒体/下载接口有效**，拿去调任何其它接口一律无效；
//   · 绑定 user_id，但不能用来签发新凭据，也不能改任何数据；
//   · 短时效（默认 2 小时，见 handlers::auth::MEDIA_TICKET_TTL_SECS）。
//
// 授权本身没有被削弱：`resolve_user_id` 解出 user_id 之后，文件归属仍由
// SQL 里的 `owner_id = ?` 把关（见 file_service::get_file），票据只回答
// 「你是谁」，不回答「你能看哪一份」。
// ===========================================================================

fn media_payload(user_id: i64, exp: i64) -> String {
    format!("media:{}:{}", user_id, exp)
}

/// 为某个用户签发媒体访问凭证。
///
/// 形态 `{exp}:{user_id}:{hmac}`。user_id 明文写在里面是有意的：它不是秘密，
/// 签名才负责证明它没被改过 —— 校验时会用票据自带的 user_id 重算签名，
/// 所以改 user_id 必然导致签名不符。
pub fn create_media_ticket(config: &Config, user_id: i64, ttl_secs: i64) -> String {
    let exp = chrono::Utc::now().timestamp() + ttl_secs;
    let mac = hmac_sha256(&config.jwt_secret, media_payload(user_id, exp).as_bytes());
    format!("{}:{}:{}", exp, user_id, hex::encode(mac))
}

/// 校验媒体访问凭证，返回其绑定的 user_id。
pub fn verify_media_ticket(config: &Config, ticket: &str) -> Option<i64> {
    let mut parts = ticket.splitn(3, ':');
    let (exp_str, user_str, mac_hex) = match (parts.next(), parts.next(), parts.next()) {
        (Some(e), Some(u), Some(m)) => (e, u, m),
        _ => return None,
    };
    let exp: i64 = exp_str.parse().ok()?;
    let user_id: i64 = user_str.parse().ok()?;
    if exp < chrono::Utc::now().timestamp() {
        return None;
    }
    let mac = hmac_sha256(&config.jwt_secret, media_payload(user_id, exp).as_bytes());
    if constant_time_eq(hex::encode(mac).as_bytes(), mac_hex.as_bytes()) {
        Some(user_id)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> Config {
        Config {
            server_host: "127.0.0.1".into(),
            server_port: 0,
            database_url: "sqlite::memory:".into(),
            upload_dir: std::env::temp_dir(),
            static_dir: "static".into(),
            jwt_secret: b"0123456789abcdef0123456789abcdef".to_vec(),
            max_file_size: 1024,
            gc_interval_sec: 0,
        }
    }

    #[test]
    fn password_hash_roundtrip() {
        let hash = hash_password("correct horse battery").unwrap();
        assert_ne!(hash, "correct horse battery");
        assert!(verify_password("correct horse battery", &hash).unwrap());
        assert!(!verify_password("wrong password", &hash).unwrap());
    }

    #[test]
    fn jwt_roundtrip_returns_claims() {
        let config = test_config();
        let token = generate_token(42, "alice", &config).unwrap();
        let claims = validate_token(&token, &config).unwrap();
        assert_eq!(claims.sub, 42);
        assert_eq!(claims.username, "alice");
        assert!(claims.exp > claims.iat);
    }

    #[test]
    fn jwt_rejected_with_wrong_secret() {
        let config = test_config();
        let token = generate_token(1, "bob", &config).unwrap();

        let mut other = test_config();
        other.jwt_secret = b"ffffffffffffffffffffffffffffffff".to_vec();
        assert!(validate_token(&token, &other).is_err());
    }

    #[test]
    fn jwt_rejected_when_expired() {
        let config = test_config();
        let now = chrono::Utc::now();
        let claims = Claims {
            sub: 7,
            username: "carol".into(),
            exp: (now - chrono::Duration::hours(1)).timestamp() as usize,
            iat: (now - chrono::Duration::hours(2)).timestamp() as usize,
        };
        let expired = encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret(&config.jwt_secret),
        )
        .unwrap();
        assert!(validate_token(&expired, &config).is_err());
    }

    #[test]
    fn share_ticket_roundtrip() {
        let config = test_config();
        let ticket = create_share_ticket(&config, "share-abc", 3600);
        assert!(verify_share_ticket(&config, "share-abc", &ticket));
    }

    #[test]
    fn share_ticket_is_bound_to_share_id() {
        let config = test_config();
        let ticket = create_share_ticket(&config, "share-abc", 3600);
        assert!(!verify_share_ticket(&config, "share-xyz", &ticket));
    }

    #[test]
    fn share_ticket_rejects_tampering_and_expiry() {
        let config = test_config();

        // 篡改签名
        let ticket = create_share_ticket(&config, "share-abc", 3600);
        let (exp, _mac) = ticket.split_once(':').unwrap();
        let tampered = format!("{}:{}", exp, "0".repeat(64));
        assert!(!verify_share_ticket(&config, "share-abc", &tampered));

        // 已过期（ttl 为负）
        let expired = create_share_ticket(&config, "share-abc", -10);
        assert!(!verify_share_ticket(&config, "share-abc", &expired));

        // 非法输入
        assert!(!verify_share_ticket(&config, "share-abc", ""));
        assert!(!verify_share_ticket(&config, "share-abc", "garbage"));
        assert!(!verify_share_ticket(&config, "share-abc", "not-a-number:abcd"));
    }

    #[test]
    fn constant_time_eq_matches_expected_semantics() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
        assert!(constant_time_eq(b"", b""));
    }

    #[test]
    fn media_ticket_roundtrip_returns_the_user_id() {
        let config = test_config();
        let ticket = create_media_ticket(&config, 77, 3600);
        assert_eq!(verify_media_ticket(&config, &ticket), Some(77));
    }

    #[test]
    fn media_ticket_rejects_tampering_expiry_and_garbage() {
        let config = test_config();
        let ticket = create_media_ticket(&config, 5, 3600);

        // 改签名
        let (head, _mac) = ticket.rsplit_once(':').unwrap();
        assert_eq!(verify_media_ticket(&config, &format!("{}:{}", head, "0".repeat(64))), None);
        // 改绑定的用户：换 user_id 会改变签名载荷
        let (exp, rest) = ticket.split_once(':').unwrap();
        let (_uid, mac) = rest.split_once(':').unwrap();
        assert_eq!(verify_media_ticket(&config, &format!("{}:{}:{}", exp, 6, mac)), None);
        // 过期
        assert_eq!(verify_media_ticket(&config, &create_media_ticket(&config, 5, -10)), None);
        // 非法输入
        for bad in ["", "garbage", "not-a-number:5:ab", "1:", "1:5"] {
            assert_eq!(verify_media_ticket(&config, bad), None, "{bad} 本应被拒");
        }
    }

    #[test]
    fn media_ticket_is_rejected_by_another_secret() {
        let config = test_config();
        let mut other = test_config();
        other.jwt_secret = b"ffffffffffffffffffffffffffffffff".to_vec();
        let ticket = create_media_ticket(&config, 9, 3600);
        assert_eq!(verify_media_ticket(&other, &ticket), None);
    }

    /// 两套凭证的签名载荷空间不能重叠。
    ///
    /// 自定义分享码允许用户把码设成 `"42"`，而 `"42"` 同样是合法的 user_id。
    /// 若载荷分别是裸的 `"{id}:{exp}"`，两者就会撞在同一个签名上。
    #[test]
    fn share_and_media_tickets_do_not_collide() {
        let config = test_config();
        let exp = chrono::Utc::now().timestamp() + 3600;

        // 手工造一枚「本该是媒体票据」的分享票据：以 share_id="42" 签发
        let share_ticket = create_share_ticket(&config, "42", 3600);

        // 它不能解开 user_id = 42 的媒体通道
        assert_eq!(
            verify_media_ticket(&config, &share_ticket),
            None,
            "分享票据不应能当媒体票据使用"
        );
        // 反向同理：媒体票据不能解开 share_id="42" 的分享
        let media_ticket = create_media_ticket(&config, 42, 3600);
        assert!(!verify_share_ticket(&config, "42", &media_ticket));

        // 两份载荷本身也必须不同
        assert_ne!(share_payload("42", exp), media_payload(42, exp));
    }
}