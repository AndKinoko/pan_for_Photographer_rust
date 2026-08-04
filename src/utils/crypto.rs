use bcrypt::{hash, verify, DEFAULT_COST};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};

use crate::config::Config;

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    pub sub: i64,       // user id
    pub username: String,
    pub exp: usize,     // expiry timestamp
    pub iat: usize,     // issued at timestamp
}

/// Hash a password using bcrypt
pub fn hash_password(password: &str) -> Result<String, bcrypt::BcryptError> {
    hash(password, DEFAULT_COST)
}

/// Verify a password against a bcrypt hash
pub fn verify_password(password: &str, hash: &str) -> Result<bool, bcrypt::BcryptError> {
    verify(password, hash)
}

/// Generate a JWT token for a user
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

/// Validate and decode a JWT token
pub fn validate_token(token: &str, config: &Config) -> Result<Claims, jsonwebtoken::errors::Error> {
    decode::<Claims>(
        token,
        &DecodingKey::from_secret(&config.jwt_secret),
        &Validation::default(),
    )
    .map(|data| data.claims)
}