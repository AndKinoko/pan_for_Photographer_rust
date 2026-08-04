use std::path::PathBuf;

/// Application configuration loaded from environment variables
#[derive(Clone, Debug)]
pub struct Config {
    pub server_host: String,
    pub server_port: u16,
    pub database_url: String,
    pub upload_dir: PathBuf,
    pub jwt_secret: Vec<u8>,
    pub max_file_size: u64,
}

impl Config {
    pub fn from_env() -> Self {
        // Load .env file if present
        let _ = dotenvy::dotenv();

        let server_host = std::env::var("SERVER_HOST").unwrap_or_else(|_| "0.0.0.0".into());
        let server_port: u16 = std::env::var("SERVER_PORT")
            .unwrap_or_else(|_| "8000".into())
            .parse()
            .expect("SERVER_PORT must be a valid u16");

        let database_path = std::env::var("DATABASE_PATH").unwrap_or_else(|_| "./data.db".into());
        let database_url = format!("sqlite:{}?mode=rwc", database_path);

        // Ensure the database directory exists
        if let Some(parent) = std::path::Path::new(&database_path).parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).ok();
            }
        }

        let upload_dir = std::env::var("UPLOAD_DIR")
            .unwrap_or_else(|_| "./uploads".into())
            .into();
        std::fs::create_dir_all(&upload_dir).ok();

        // Read JWT secret from file; panic if missing
        let jwt_secret_key_file = std::env::var("JWT_SECRET_KEY_FILE")
            .unwrap_or_else(|_| "./.secret_key".into());
        let jwt_secret = std::fs::read_to_string(&jwt_secret_key_file)
            .unwrap_or_else(|_| panic!(
                "JWT secret key file not found at '{}'. Please create it with a secure random string.",
                jwt_secret_key_file
            ))
            .trim()
            .as_bytes()
            .to_vec();

        let max_file_size: u64 = std::env::var("MAX_FILE_SIZE")
            .unwrap_or_else(|_| "10737418240".into())
            .parse()
            .expect("MAX_FILE_SIZE must be a valid u64");

        Config {
            server_host,
            server_port,
            database_url,
            upload_dir,
            jwt_secret,
            max_file_size,
        }
    }
}