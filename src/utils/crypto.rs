use bcrypt::{hash, verify, DEFAULT_COST};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};

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