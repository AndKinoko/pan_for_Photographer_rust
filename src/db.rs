use sqlx::SqlitePool;

/// 初始化数据库连接池并运行迁移
pub async fn init_db(database_url: &str) -> Result<SqlitePool, sqlx::Error> {
    let pool = SqlitePool::connect(database_url).await?;

    // 启用 WAL 模式以获得更好的并发读取性能
    sqlx::query("PRAGMA journal_mode=WAL;")
        .execute(&pool)
        .await?;
    sqlx::query("PRAGMA foreign_keys=ON;")
        .execute(&pool)
        .await?;

    // 运行迁移
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

    // 为已有数据库添加 thumb_path 列（如果已存在则忽略错误）
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

    // === 新增列迁移（向后兼容，已存在则忽略） ===

    // 用户角色（user / admin）
    sqlx::query("ALTER TABLE users ADD COLUMN role TEXT NOT NULL DEFAULT 'user'")
        .execute(pool)
        .await
        .ok();

    // 文件软删除
    sqlx::query("ALTER TABLE files ADD COLUMN deleted_at DATETIME")
        .execute(pool)
        .await
        .ok();

    // 文件夹软删除
    sqlx::query("ALTER TABLE folders ADD COLUMN deleted_at DATETIME")
        .execute(pool)
        .await
        .ok();

    // 分享：文件夹分享支持
    sqlx::query("ALTER TABLE file_shares ADD COLUMN folder_id INTEGER REFERENCES folders(id) ON DELETE CASCADE")
        .execute(pool)
        .await
        .ok();

    // 分享：最大下载次数限制
    sqlx::query("ALTER TABLE file_shares ADD COLUMN max_downloads INTEGER")
        .execute(pool)
        .await
        .ok();

    // 分享：自定义分享码
    sqlx::query("ALTER TABLE file_shares ADD COLUMN custom_code TEXT")
        .execute(pool)
        .await
        .ok();

    // 为文件搜索添加索引
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_files_owner ON files(owner_id)")
        .execute(pool)
        .await
        .ok();
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_files_folder ON files(folder_id)")
        .execute(pool)
        .await
        .ok();
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_files_deleted ON files(deleted_at)")
        .execute(pool)
        .await
        .ok();
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_folders_deleted ON folders(deleted_at)")
        .execute(pool)
        .await
        .ok();

    tracing::info!("数据库迁移完成");
    Ok(())
}