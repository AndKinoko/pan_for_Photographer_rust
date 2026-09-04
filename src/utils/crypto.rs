use bcrypt::{hash, verify, DEFAULT_COST};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::config::Config;

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    pub sub: i64,       // 用户ID
    pub username: String,
    pub exp: usize,     // 过期时间戳
    pub iat: usize,     // 签发时间戳
}

/// 使用bcrypt对密码进行哈希
pub fn hash_password(password: &str) -> Result<String, bcrypt::BcryptError> {
    hash(password, DEFAULT_COST)
}

/// 验证密码与bcrypt哈希是否匹配
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
    let payload = format!("{}:{}", share_id, exp);
    let mac = hmac_sha256(&config.jwt_secret, payload.as_bytes());
    format!("{}:{}", exp, hex::encode(mac))
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
    let payload = format!("{}:{}", share_id, exp);
    let mac = hmac_sha256(&config.jwt_secret, payload.as_bytes());
    constant_time_eq(hex::encode(mac).as_bytes(), mac_hex.as_bytes())
}