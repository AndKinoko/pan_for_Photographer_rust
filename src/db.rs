use sqlx::SqlitePool;

/// Initialize the database connection pool and run migrations
pub async fn init_db(database_url: &str) -> Result<SqlitePool, sqlx::Error> {
    let pool = SqlitePool::connect(database_url).await?;

    // Enable WAL mode for better concurrent read performance
    sqlx::query("PRAGMA journal_mode=WAL;")
        .execute(&pool)
        .await?;
    sqlx::query("PRAGMA foreign_keys=ON;")
        .execute(&pool)
        .await?;

    // Run migrations
    run_migrations(&pool).await?;

    Ok(pool)
}

async fn run_migrations(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS users (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            username TEXT NOT NULL UNIQUE,
            password_hash TEXT NOT NULL,
            created_at DATETIME NOT NULL DEFAULT (datetime('now'))
        );
        "#,
    )
    .execute(pool)
    .await?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS folders (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            owner_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            parent_id INTEGER REFERENCES folders(id) ON DELETE CASCADE,
            created_at DATETIME NOT NULL DEFAULT (datetime('now')),
            updated_at DATETIME NOT NULL DEFAULT (datetime('now')),
            UNIQUE(name, owner_id, parent_id)
        );
        "#,
    )
    .execute(pool)
    .await?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS files (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            original_name TEXT NOT NULL,
            stored_path TEXT NOT NULL,
            preview_path TEXT,
            thumb_path TEXT,
            owner_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            folder_id INTEGER REFERENCES folders(id) ON DELETE SET NULL,
            size INTEGER NOT NULL DEFAULT 0,
            file_type TEXT NOT NULL DEFAULT '',
            uploaded_at DATETIME NOT NULL DEFAULT (datetime('now')),
            updated_at DATETIME NOT NULL DEFAULT (datetime('now'))
        );
        "#,
    )
    .execute(pool)
    .await?;

    // Add thumb_path column for existing databases (ignore error if already exists)
    sqlx::query("ALTER TABLE files ADD COLUMN thumb_path TEXT")
        .execute(pool)
        .await
        .ok();

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS file_shares (
            id TEXT PRIMARY KEY,
            file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
            owner_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            created_at DATETIME NOT NULL DEFAULT (datetime('now')),
            expires_at DATETIME,
            password_hash TEXT NOT NULL DEFAULT '',
            download_count INTEGER NOT NULL DEFAULT 0,
            is_active INTEGER NOT NULL DEFAULT 1
        );
        "#,
    )
    .execute(pool)
    .await?;

    tracing::info!("Database migrations completed successfully");
    Ok(())
}