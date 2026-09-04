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

    // 用户有效期（可空，NULL 表示永久有效）
    sqlx::query("ALTER TABLE users ADD COLUMN expires_at DATETIME")
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

/// 确保超级管理员存在（幂等）。仅首次创建，不覆盖已存在的密码/角色。
///
/// 账号与初始密码从环境变量读取，避免硬编码在源码中泄露：
///   - `SEED_ADMIN_USERNAME`：默认 `"admin"`
///   - `SEED_ADMIN_PASSWORD`：默认空；若为空则**仅在数据库尚无任何 admin 时**打印警告并跳过创建，
///     由首个启动者通过 `cargo run -- admin create` 或数据库直接操作完成初始化。
pub async fn seed_admin(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    let username =
        std::env::var("SEED_ADMIN_USERNAME").unwrap_or_else(|_| "admin".to_string());
    let password = std::env::var("SEED_ADMIN_PASSWORD").unwrap_or_default();

    // 仅在数据库里"完全没有 admin"时尝试创建；已存在则什么都不做（保证幂等、不覆盖）。
    let admin_exists: Option<i64> =
        sqlx::query_scalar("SELECT id FROM users WHERE role = 'admin' LIMIT 1")
            .fetch_optional(pool)
            .await?;
    if admin_exists.is_some() {
        tracing::info!("已存在 admin 账户，跳过种子创建");
        return Ok(());
    }

    if password.is_empty() {
        tracing::warn!(
            "数据库中无 admin 账户，且未设置 SEED_ADMIN_PASSWORD。\n\
             请通过以下方式之一初始化第一个管理员：\n\
             1. 设置 SEED_ADMIN_USERNAME / SEED_ADMIN_PASSWORD 环境变量后重启；\n\
             2. 手动 INSERT 一个 admin 账户；\n\
             3. 调用 /api/auth/register 注册后由数据库手动 UPDATE role='admin'。"
        );
        return Ok(());
    }

    let password_hash = crate::utils::crypto::hash_password(&password)
        .map_err(|_| sqlx::Error::ColumnIndexOutOfBounds { index: 0, len: 1 })?;

    sqlx::query(
        r#"INSERT INTO users (username, password_hash, role)
           VALUES (?, ?, 'admin')
           ON CONFLICT(username) DO NOTHING"#,
    )
    .bind(&username)
    .bind(&password_hash)
    .execute(pool)
    .await?;

    tracing::info!("已创建 admin 账户 '{}'，请尽快登录后修改密码", username);
    Ok(())
}