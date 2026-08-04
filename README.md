# PAN FOR PHOTOGRAPHER - Django to Rust 重构文档

## 目录

1. [项目概述](#1-项目概述)
2. [技术选型说明](#2-技术选型说明)
3. [架构设计](#3-架构设计)
4. [数据模型对照](#4-数据模型对照)
5. [API 接口对照表](#5-api-接口对照表)
6. [性能对比分析](#6-性能对比分析)
7. [迁移指南](#7-迁移指南)

---

## 1. 项目概述

### 1.1 项目背景

PAN FOR PHOTOGRAPHER 是一个面向摄影师群体的局域网文件共享网盘系统。原系统使用 Django 4.2 + SQLite 构建，支持用户注册/登录、文件上传/下载/预览、文件夹管理、文件分享等功能。

### 1.2 重构目标

- 将后端从 Python/Django 迁移至 Rust/Axum，提升并发性能和资源利用率
- 保持前端 SPA 不变（HTML/CSS/JS），仅替换后端 API 层
- 保持数据库 schema 兼容性，支持无缝迁移
- 保持 API 接口响应格式一致

### 1.3 原项目技术栈

| 组件 | 技术 |
|------|------|
| 语言 | Python 3 |
| 框架 | Django 4.2 |
| 数据库 | SQLite 3 |
| 认证 | Django Session Auth |
| 图片处理 | Pillow + rawpy |
| 前端 | Django Templates + Bootstrap 5 |
| 部署 | 单进程 WSGI |

### 1.4 新项目技术栈

| 组件 | 技术 | 版本 |
|------|------|------|
| 语言 | Rust | Edition 2021 |
| 框架 | Axum | 0.7.9 |
| 异步运行时 | Tokio | 1.x |
| 数据库 | SQLx (SQLite) | 0.8 |
| 认证 | JWT (jsonwebtoken) | 9.x |
| 密码哈希 | bcrypt | 0.16 |
| 图片处理 | image | 0.25 |
| 序列化 | Serde + serde_json | 1.x |
| 前端 | 静态 SPA (HTML/CSS/JS) | 不变 |
| 部署 | 单二进制文件 |

---

## 2. 技术选型说明

### 2.1 为什么选择 Rust

| 维度 | Django (Python) | Axum (Rust) | 优势 |
|------|-----------------|-------------|------|
| 并发模型 | 同步/多线程 WSGI | 异步 Tokio 运行时 | 更高并发处理能力 |
| 内存占用 | ~50-200 MB | ~5-20 MB | 显著降低 |
| 启动时间 | 1-3 秒 | <100ms | 即时启动 |
| CPU 密集型操作 | GIL 限制 | 无锁并发 | 图片处理更高效 |
| 类型安全 | 运行时 | 编译时 | 更多错误在编译期捕获 |
| 部署复杂度 | 需 Python 环境 + 依赖 | 单二进制文件 | 部署极简 |

### 2.2 框架选择：Axum

选择 Axum 0.7.9 的原因：
- Tokio 团队官方维护，生态成熟
- 基于 Tower 中间件体系，可组合性强
- 原生 async/await 支持
- 类型安全的 Extractors 机制
- 内置 multipart 文件上传支持

### 2.3 数据库选择：SQLx + SQLite

保持与原项目相同的 SQLite 数据库：
- 零配置，无需独立数据库服务
- 适合局域网/单机部署场景
- SQLx 提供编译时 SQL 检查（可选）和异步操作
- 启用 WAL 模式提升并发读性能

### 2.4 认证方案：JWT 替代 Session

| 维度 | Django Session | JWT |
|------|---------------|-----|
| 状态 | 服务端有状态 | 客户端无状态 |
| 扩展性 | 需共享 Session | 天然分布式 |
| API 友好 | 需 Cookie | Bearer Token |
| 安全性 | CSRF 防护 | 内建签名验证 |

### 2.5 关键决策记录

1. **路径参数语法**：Axum 0.7 使用 `:id` 语法，Axum 0.8 改为 `{id}` 语法。本项目锁定 Axum 0.7.9 以保持稳定性。
2. **JWT 密钥管理**：从文件 `.secret_key` 读取，缺失时 panic 启动，禁止硬编码回退。
3. **数据库连接**：使用 `sqlite:path?mode=rwc` 自动创建数据库文件。
4. **错误处理**：统一 AppError 类型，所有内部错误不暴露文件系统路径。

---

## 3. 架构设计

### 3.1 系统架构图

```
┌─────────────────────────────────────────────────────┐
│                     客户端 (Browser)                  │
│                  static/index.html + app.js           │
└─────────────────────┬───────────────────────────────┘
                      │ HTTP (REST API)
                      ▼
┌─────────────────────────────────────────────────────┐
│                   Axum HTTP Server                    │
│  ┌─────────────────────────────────────────────────┐ │
│  │              Middleware Layer                    │ │
│  │  ┌──────────┐ ┌──────────┐ ┌────────────────┐  │ │
│  │  │   CORS   │ │  Trace   │ │ BodySizeLimit  │  │ │
│  │  └──────────┘ └──────────┘ └────────────────┘  │ │
│  └─────────────────────────────────────────────────┘ │
│  ┌─────────────────────────────────────────────────┐ │
│  │               Router Layer                       │ │
│  │  ┌──────────┐ ┌──────────┐ ┌────────────────┐  │ │
│  │  │   Auth   │ │  Files   │ │    Folders     │  │ │
│  │  │ Handler  │ │ Handler  │ │    Handler     │  │ │
│  │  └──────────┘ └──────────┘ └────────────────┘  │ │
│  │  ┌──────────┐ ┌──────────┐ ┌────────────────┐  │ │
│  │  │  Share   │ │  Search  │ │  Static Files  │  │ │
│  │  │ Handler  │ │ Handler  │ │   (fallback)   │  │ │
│  │  └──────────┘ └──────────┘ └────────────────┘  │ │
│  └─────────────────────────────────────────────────┘ │
│  ┌─────────────────────────────────────────────────┐ │
│  │              Service Layer                       │ │
│  │  ┌──────────┐ ┌──────────┐ ┌────────────────┐  │ │
│  │  │  File    │ │  Folder  │ │    Share       │  │ │
│  │  │ Service  │ │ Service  │ │   Service      │  │ │
│  │  └──────────┘ └──────────┘ └────────────────┘  │ │
│  │  ┌──────────────────────────────────────────┐   │ │
│  │  │           Preview Service                 │   │ │
│  │  └──────────────────────────────────────────┘   │ │
│  └─────────────────────────────────────────────────┘ │
│  ┌─────────────────────────────────────────────────┐ │
│  │               Data Layer                         │ │
│  │  ┌──────────────────────┐ ┌──────────────────┐  │ │
│  │  │      SQLx Pool       │ │   File System    │  │ │
│  │  │     (SQLite WAL)     │ │   (uploads/)     │  │ │
│  │  └──────────────────────┘ └──────────────────┘  │ │
│  └─────────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────────┘
```

### 3.2 项目目录结构

```
pan_for_Photographer/
├── Cargo.toml              # Rust 项目配置 & 依赖
├── .env                    # 环境变量配置
├── .secret_key             # JWT 签名密钥
├── data.db                 # SQLite 数据库（自动创建）
├── REFACTORING_DOCUMENTATION.md  # 本文档
├── static/                 # 前端 SPA 文件
│   ├── index.html          # 主页面
│   ├── style.css           # 样式表
│   └── app.js              # 前端逻辑
├── uploads/                # 用户上传文件目录
│   └── user_{id}/          # 按用户隔离
│       ├── {uuid}.ext      # 存储文件
│       └── previews/       # 预览图
│           └── {uuid}.jpg
└── src/
    ├── main.rs             # 入口 + 路由定义
    ├── config.rs           # 配置加载
    ├── db.rs               # 数据库初始化 & 迁移
    ├── errors.rs           # 统一错误类型
    ├── handlers/           # HTTP 请求处理器
    │   ├── mod.rs
    │   ├── auth.rs         # 注册/登录/获取用户
    │   ├── files.rs        # 文件上传/下载/预览/删除
    │   ├── folders.rs      # 文件夹 CRUD
    │   ├── share.rs        # 分享 CRUD + 公开访问
    │   └── search.rs       # 文件搜索
    ├── middleware/
    │   ├── mod.rs
    │   └── auth.rs         # JWT 认证中间件
    ├── models/             # 数据模型
    │   ├── mod.rs
    │   ├── user.rs         # 用户模型
    │   ├── file.rs         # 文件模型
    │   ├── folder.rs       # 文件夹模型
    │   └── share.rs        # 分享模型
    ├── services/           # 业务逻辑层
    │   ├── mod.rs
    │   ├── file_service.rs
    │   ├── folder_service.rs
    │   ├── share_service.rs
    │   └── preview_service.rs
    └── utils/
        ├── mod.rs
        └── crypto.rs       # JWT & bcrypt 工具
```

### 3.3 请求处理流程

```
Request → CORS Layer → Trace Layer → Body Limit Layer
    → Router (path matching via matchit)
        → Auth Middleware (JWT validation for protected routes)
            → Handler (extract State, Path, Query, Body)
                → Service (business logic + DB operations)
                    → Response (JSON)
```

---

## 4. 数据模型对照

### 4.1 用户表 (users)

| 字段 | Django (User) | Rust (User) | 类型 | 说明 |
|------|--------------|-------------|------|------|
| id | User.id | id | INTEGER PK | 自增主键 |
| username | User.username | username | TEXT UNIQUE | 用户名 |
| password | User.password | password_hash | TEXT | bcrypt 哈希 |
| created_at | - | created_at | DATETIME | 创建时间 |

### 4.2 文件夹表 (folders)

| 字段 | Django (Folder) | Rust (Folder) | 类型 | 说明 |
|------|----------------|---------------|------|------|
| id | Folder.id | id | INTEGER PK | 自增主键 |
| name | Folder.name | name | TEXT | 文件夹名称 |
| owner_id | Folder.owner (FK→User) | owner_id | INTEGER FK | 所有者 |
| parent_id | Folder.parent (FK→self) | parent_id | INTEGER FK | 父文件夹 |
| created_at | Folder.created_at | created_at | DATETIME | 创建时间 |
| updated_at | Folder.updated_at | updated_at | DATETIME | 更新时间 |
| 约束 | unique_together(name, owner, parent) | UNIQUE(name, owner_id, parent_id) | - | 同用户同父目录下唯一 |

### 4.3 文件表 (files)

| 字段 | Django (File) | Rust (File) | 类型 | 说明 |
|------|--------------|-------------|------|------|
| id | File.id | id | INTEGER PK | 自增主键 |
| name | File.name | name | TEXT | 文件名 |
| original_name | File.original_name | original_name | TEXT | 原始文件名 |
| stored_path | File.file (FileField) | stored_path | TEXT | 存储路径 |
| preview_path | File.preview (ImageField) | preview_path | TEXT NULL | 预览图路径 |
| owner_id | File.owner (FK→User) | owner_id | INTEGER FK | 所有者 |
| folder_id | File.folder (FK→Folder) | folder_id | INTEGER FK NULL | 所属文件夹 |
| size | File.size | size | INTEGER | 文件大小(字节) |
| file_type | File.file_type | file_type | TEXT | 扩展名 |
| uploaded_at | File.uploaded_at | uploaded_at | DATETIME | 上传时间 |
| updated_at | File.updated_at | updated_at | DATETIME | 更新时间 |

### 4.4 分享表 (file_shares)

| 字段 | Django (FileShare) | Rust (FileShare) | 类型 | 说明 |
|------|-------------------|-----------------|------|------|
| id | UUIDField | id | TEXT PK | UUID 主键 |
| file_id | FK→File | file_id | INTEGER FK | 分享的文件 |
| owner_id | FK→User | owner_id | INTEGER FK | 分享者 |
| created_at | DateTimeField | created_at | DATETIME | 创建时间 |
| expires_at | DateTimeField NULL | expires_at | DATETIME NULL | 过期时间 |
| password_hash | CharField | password_hash | TEXT | bcrypt 哈希 |
| download_count | IntegerField | download_count | INTEGER | 下载次数 |
| is_active | BooleanField | is_active | INTEGER | 是否有效 |

---

## 5. API 接口对照表

### 5.1 认证接口

| 方法 | 路径 | Django 视图 | Rust 处理器 | 认证 | 说明 |
|------|------|------------|-------------|------|------|
| POST | /api/auth/register | accounts.views | handlers::auth::register | 否 | 用户注册 |
| POST | /api/auth/login | accounts.views | handlers::auth::login | 否 | 用户登录 |
| GET | /api/auth/me | - | handlers::auth::me | JWT | 获取当前用户信息 |

**请求/响应格式**：

```
POST /api/auth/register
Request:  {"username": "string", "password": "string"}
Response: {"success": true, "data": {"token": "jwt", "user": {...}}, "error": null}

POST /api/auth/login
Request:  {"username": "string", "password": "string"}
Response: {"success": true, "data": {"token": "jwt", "user": {...}}, "error": null}

GET /api/auth/me
Headers:  Authorization: Bearer <jwt>
Response: {"success": true, "data": {"id": 1, "username": "...", "created_at": "..."}, "error": null}
```

### 5.2 文件接口

| 方法 | 路径 | Django 视图 | Rust 处理器 | 认证 | 说明 |
|------|------|------------|-------------|------|------|
| GET | /api/files | storage.views.file_list | handlers::files::list_files | JWT | 文件列表 |
| POST | /api/files/upload | storage.views.upload_file | handlers::files::upload_files | JWT | 上传文件 |
| GET | /api/files/:id/download | - | handlers::files::download_file | JWT | 下载文件 |
| GET | /api/files/:id/media | storage.views.serve_media | handlers::files::serve_media | JWT | 媒体预览 |
| DELETE | /api/files/:id | storage.views.delete_file | handlers::files::delete_file | JWT | 删除文件 |

**查询参数**：

```
GET /api/files?folder_id={id}   # 按文件夹筛选
GET /api/files/:id/media?preview=1  # 获取预览图
```

### 5.3 文件夹接口

| 方法 | 路径 | Django 视图 | Rust 处理器 | 认证 | 说明 |
|------|------|------------|-------------|------|------|
| GET | /api/folders | storage.views.folder_list | handlers::folders::list_folders | JWT | 文件夹列表 |
| POST | /api/folders | storage.views.create_folder | handlers::folders::create_folder | JWT | 创建文件夹 |
| DELETE | /api/folders/:id | storage.views.delete_folder | handlers::folders::delete_folder | JWT | 删除文件夹 |

**查询参数**：

```
GET /api/folders?parent_id={id}   # 按父文件夹筛选
```

### 5.4 分享接口

| 方法 | 路径 | Django 视图 | Rust 处理器 | 认证 | 说明 |
|------|------|------------|-------------|------|------|
| GET | /api/shares | share.views.share_list | handlers::share::list_shares | JWT | 我的分享列表 |
| POST | /api/shares | share.views.create_share | handlers::share::create_share | JWT | 创建分享 |
| GET | /api/shares/:id | share.views.share_detail | handlers::share::get_share | JWT | 分享详情 |
| DELETE | /api/shares/:id | share.views.delete_share | handlers::share::delete_share | JWT | 删除分享 |
| GET | /api/public/shares/:id | share.views.share_access | handlers::share::public_share_access | 否 | 公开访问 |
| POST | /api/public/shares/:id/verify | share.views.verify_password | handlers::share::public_verify_password | 否 | 验证密码 |
| GET | /api/public/shares/:id/download | share.views.share_download | handlers::share::public_share_download | 否 | 公开下载 |

### 5.5 搜索接口

| 方法 | 路径 | Django 视图 | Rust 处理器 | 认证 | 说明 |
|------|------|------------|-------------|------|------|
| GET | /api/search | search.views | handlers::search::search_files | JWT | 文件搜索 |

**查询参数**：

```
GET /api/search?q={keyword}&type={file_type}
```

### 5.6 错误响应格式

所有接口统一使用以下错误响应格式：

```json
{
    "success": false,
    "data": null,
    "error": "错误描述信息"
}
```

HTTP 状态码：
- 400: 请求参数错误
- 401: 未认证或认证失败
- 403: 无权限访问
- 404: 资源不存在
- 409: 资源冲突（如重名）
- 410: 资源已过期
- 413: 文件大小超限
- 500: 服务器内部错误

---

## 6. 性能对比分析

### 6.1 基准测试环境

- CPU: 未指定
- 内存: 未指定
- OS: Windows
- 测试工具: 理论分析 + 编译指标

### 6.2 理论性能对比

| 指标 | Django (Python) | Axum (Rust) | 改善 |
|------|-----------------|-------------|------|
| 启动时间 | 1-3 秒 | <100ms | ~20x |
| 基础内存占用 | ~50-200 MB | ~5-20 MB | ~5-10x |
| 并发连接处理 | 受限于 WSGI 线程池 | 异步 I/O 无阻塞 | 显著提升 |
| 请求延迟 (P50) | ~10-50ms | ~1-5ms | ~5-10x |
| 吞吐量 (req/s) | ~500-2000 | ~10000-50000 | ~10-25x |
| 二进制大小 | N/A (需 Python 环境) | ~10-15 MB (release) | 单文件部署 |
| 编译时间 | 即时 | ~30-60s (首次) | 开发时稍慢 |

### 6.3 关键优化点

1. **异步 I/O**：Tokio 运行时提供真正的异步文件操作和数据库查询，消除线程阻塞
2. **零成本抽象**：Rust 的泛型和 trait 在编译后无运行时开销
3. **WAL 模式**：SQLite 启用 WAL 模式，允许并发读写
4. **内存安全**：编译时保证无内存泄漏、无数据竞争
5. **连接池**：SQLx 连接池复用数据库连接，减少连接开销

### 6.4 资源消耗对比

| 资源 | Django | Rust | 说明 |
|------|--------|------|------|
| 运行时依赖 | Python 3 + pip 包 | 无 (静态链接) | Rust 单二进制 |
| 内存基线 | ~50-200 MB | ~5-20 MB | 不含上传文件 |
| CPU 利用率 | 单核受限 (GIL) | 多核并行 | 图片处理更快 |
| 磁盘 I/O | 同步阻塞 | 异步非阻塞 | 上传不阻塞 |

---

## 7. 迁移指南

### 7.1 环境要求

- Rust 1.75+ (MSVC toolchain on Windows)
- Visual Studio Build Tools (Windows)
- 已存在的 `.secret_key` 文件（JWT 签名密钥）

### 7.2 快速启动

```bash
# 1. 进入项目目录
cd F:\RUST\pan_for_Photographer

# 2. 配置环境变量（编辑 .env 文件）
# SERVER_HOST=0.0.0.0
# SERVER_PORT=8000
# DATABASE_PATH=./data.db
# UPLOAD_DIR=./uploads
# JWT_SECRET_KEY_FILE=./.secret_key
# MAX_FILE_SIZE=10737418240

# 3. 确保 .secret_key 文件存在
# 如果不存在，生成一个：powershell -Command "[Convert]::ToBase64String((1..32|%{Get-Random -Max 256}))"

# 4. 编译并运行
cargo run --release

# 5. 访问
# http://localhost:8000
```

### 7.3 从 Django 迁移数据

#### 数据库迁移

Django 和 Rust 项目使用相同的 SQLite schema 结构，可以复用原有数据库文件：

```bash
# 1. 停止 Django 服务
# 2. 复制 Django 数据库到 Rust 项目目录
copy F:\PAN_FOR_ME\db.sqlite3 F:\RUST\pan_for_Photographer\data.db

# 3. 启动 Rust 服务
cd F:\RUST\pan_for_Photographer
cargo run --release
```

**注意**：Django 使用 `auth_user` 表存储用户，密码使用 Django 的 PBKDF2 哈希算法。Rust 项目使用独立的 `users` 表，密码使用 bcrypt 哈希。如需迁移用户数据，需要：
1. 导出 Django 用户数据
2. 使用 bcrypt 重新哈希密码
3. 导入到 Rust 项目的 `users` 表

#### 文件迁移

上传文件存储在 `uploads/` 目录下，目录结构相同：

```
Django:  F:\PAN_FOR_ME\media\user_{id}\{uuid}.ext
Rust:    F:\RUST\pan_for_Photographer\uploads\user_{id}\{uuid}.ext
```

直接复制即可：

```bash
xcopy /E /I F:\PAN_FOR_ME\media\* F:\RUST\pan_for_Photographer\uploads\
```

### 7.4 环境变量说明

| 变量 | 默认值 | 说明 |
|------|--------|------|
| SERVER_HOST | 0.0.0.0 | 监听地址 |
| SERVER_PORT | 8000 | 监听端口 |
| DATABASE_PATH | ./data.db | SQLite 数据库路径 |
| UPLOAD_DIR | ./uploads | 文件上传目录 |
| JWT_SECRET_KEY_FILE | ./.secret_key | JWT 密钥文件路径 |
| MAX_FILE_SIZE | 10737418240 | 最大文件大小(字节, 默认10GB) |

### 7.5 部署建议

**开发环境**：
```bash
cargo run
```

**生产环境**：
```bash
cargo build --release
./target/release/pan_for_photographer.exe
```

**Windows 服务**：
可使用 NSSM (Non-Sucking Service Manager) 注册为 Windows 服务：
```bash
nssm install PanForPhotographer F:\RUST\pan_for_Photographer\target\release\pan_for_photographer.exe
nssm set PanForPhotographer AppDirectory F:\RUST\pan_for_Photographer
nssm start PanForPhotographer
```

### 7.6 前端兼容性

前端 SPA 文件（`static/index.html`, `static/style.css`, `static/app.js`）保持不变。Rust 服务器通过 `ServeDir` 提供静态文件服务，API 路由优先级高于静态文件。

### 7.7 已知限制

1. **RAW 文件预览**：原 Django 项目使用 `rawpy` 库处理 RAW 格式预览，Rust 重构仅支持标准图片格式（jpg, png 等）的预览生成，RAW 格式暂不支持自动预览。
2. **用户数据迁移**：密码哈希算法不同（Django 用 PBKDF2，Rust 用 bcrypt），直接迁移需重新设置密码。
3. **文件扩展名验证**：原 Django 项目无限制，Rust 项目增加了白名单扩展名验证。

### 7.8 故障排查

| 问题 | 可能原因 | 解决方案 |
|------|---------|---------|
| 启动 panic | .secret_key 文件缺失 | 创建 .secret_key 文件 |
| 端口占用 | 8000 端口已被占用 | 修改 SERVER_PORT 或关闭占用进程 |
| 编译失败 | 缺少 MSVC 工具链 | 安装 Visual Studio Build Tools |
| 数据库锁定 | 多进程访问 | 确保只有一个实例运行 |
| 401 认证失败 | Token 过期或无效 | 重新登录获取新 Token |

---

## 附录

### A. 依赖清单

```toml
[dependencies]
axum = { version = "0.7", features = ["multipart"] }
tokio = { version = "1", features = ["full"] }
sqlx = { version = "0.8", features = ["runtime-tokio", "sqlite", "chrono"] }
jsonwebtoken = "9"
bcrypt = "0.16"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
uuid = { version = "1", features = ["v4"] }
image = "0.25"
tower-http = { version = "0.5", features = ["cors", "fs", "trace"] }
tower = "0.4"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
dotenvy = "0.15"
chrono = { version = "0.4", features = ["serde"] }
mime_guess = "2"
futures-util = "0.3"
hex = "0.4"
sha2 = "0.10"
```

### B. 路由总览

```
POST   /api/auth/register
POST   /api/auth/login
GET    /api/auth/me

GET    /api/files?folder_id=
POST   /api/files/upload
GET    /api/files/:id/download
GET    /api/files/:id/media?preview=
DELETE /api/files/:id

GET    /api/folders?parent_id=
POST   /api/folders
DELETE /api/folders/:id

GET    /api/shares
POST   /api/shares
GET    /api/shares/:id
DELETE /api/shares/:id

GET    /api/public/shares/:id
POST   /api/public/shares/:id/verify
GET    /api/public/shares/:id/download

GET    /api/search?q=&type=

GET    /*  →  static/  (静态文件)
```

### C. 变更日志

| 日期 | 版本 | 变更内容 |
|------|------|---------|
| 2026-07-30 | 1.0.0 | 初始 Rust 重构版本，完成 Django → Rust 迁移 |