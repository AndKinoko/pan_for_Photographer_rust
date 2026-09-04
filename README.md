# Pan For Photographer

为摄影师团队设计的局域网文件共享服务。Rust + Axum + SQLite 高性能后端，Vue 3 前端，支持 RAW 格式在线预览（NEF / CR2 / ARW 等）。

> **🔒 隐私提示**：本仓库 **不包含** 任何用户数据、数据库、上传文件、JWT 密钥或默认管理员密码。所有运行时数据需在首次部署时自行创建。

---

## 功能特性

- **文件管理**：上传、下载、预览、重命名、移动、复制、删除
- **文件夹管理**：新建、重命名、进入/返回、批量移动/复制
- **回收站**：软删除、恢复、永久删除
- **批量操作**：批量移动、复制、删除、分享、下载；全选 / 反选
- **全局传输抽屉**：上传队列 + 下载队列，实时进度、取消、重试
- **多媒体预览**：图片、RAW（含内嵌 JPEG 提取）、视频、音频、PDF
- **大文件流式上传**：逐文件 multipart 流式写盘，内存占用与文件大小解耦
- **缩略图异步生成**：后台队列 + 并发闸（permits=2），上传响应毫秒级返回，缩略图自动补齐
- **磁盘孤儿清理**：周期 GC 对账（孤儿文件 / 超龄 .part / 缺失缩略图重投）
- **公开分享**：密码保护、过期时间、下载次数限制
- **用户系统**：注册 / 登录 / JWT，管理员面板管理用户（含账号有效期）
- **主题切换**：亮色 / 暗色

## 技术栈

- **后端**：Rust、Axum、Tokio、SQLx (SQLite)、image、uuid、tokio-util
- **前端**：Vue 3、Vite、原生 CSS
- **鉴权**：JWT + bcrypt
- **存储**：本地文件系统 + SQLite

## 环境要求

- Rust 1.80+
- Node.js 20+（仅前端构建需要）
- Windows / macOS / Linux

---

## 快速开始（推荐 `start_server.bat`）

### 1. 克隆与构建前端

```bash
git clone <repo-url> pan_for_Photographer_rust
cd pan_for_Photographer_rust/frontend
npm install
npm run build       # 输出到 ../static/（已被 .gitignore）
```

> `start_server.bat` 会自动执行第 1 步 + 启动后端；如果只改了前端代码，再次双击 `start_server.bat` 即可刷新。

### 2. 配置 JWT 密钥（必须）

```bash
# Windows PowerShell
Set-Content -Path .secret_key -Value 'put-a-long-random-string-here' -NoNewline

# Linux / macOS
echo -n 'put-a-long-random-string-here' > .secret_key
```

> 密钥文件在 `.gitignore` 中，永不会被提交。**首次启动前必须创建**，否则后端会 panic。

### 3. （可选）配置初始管理员

`start_server.bat` 默认会调用 `db::seed_admin`。可通过以下环境变量控制：

| 变量 | 默认 | 说明 |
|------|------|------|
| `SEED_ADMIN_USERNAME` | `admin` | 首次启动时创建的 admin 用户名 |
| `SEED_ADMIN_PASSWORD` | *(空)* | 首次启动时创建的 admin 密码；**空则跳过创建并打印警告** |

#### 方式 A：通过环境变量创建（推荐）

```bat
:: Windows
set SEED_ADMIN_USERNAME=admin
set SEED_ADMIN_PASSWORD=YourStrongPass!2026
start_server.bat
```

```bash
# Linux / macOS
export SEED_ADMIN_USERNAME=admin
export SEED_ADMIN_PASSWORD='YourStrongPass!2026'
./run.sh
```

#### 方式 B：手动初始化

若不想设置环境变量：
1. 启动服务（无 admin 账户，仅警告）
2. 用 `POST /api/auth/register` 注册任意账户
3. 用 `sqlite3 data.db "UPDATE users SET role='admin' WHERE username='你的用户名'"` 提升为 admin

> 源码中 **不再硬编码** 默认账号密码，请务必按需设置。

### 4. 启动

```bat
:: Windows
start_server.bat
```

```bash
# Linux / macOS
cargo run --release
```

默认配置：
- 监听：`SERVER_HOST=::`（IPv6 双栈，同时兼容 IPv4/IPv6 访问）+ `SERVER_PORT=0100`
- 访问地址：`http://localhost:100`
- 静态目录：`static`（即上一步构建的前端）
- 上传目录：`./uploads`，数据库：`./data.db`

### 5. 登录

使用第 3 步设置的 admin 账户登录；首次登录后请尽快修改密码。

---

## 双端口发布（`start_publish.bat`，可选）

管理端功能已并入 Vue 主站（`/admin`），不再需要独立管理前端；普通用户交付端 `static_user/` 保留（供客户只读取片）。

- `8001`：影像交付端（`static_user/`）——客户只读：登录 / 浏览 / 预览 / 下载
- `0100`：统一 Vue 前端（`static/`）——完整功能 + 管理端（admin 登录后访问 `/admin`：用户管理 / 有效期 / 新建·编辑用户 / 为指定用户上传原图 / 系统统计）

> 两端口共用同一数据库与上传目录。超管账号由 `SEED_ADMIN_USERNAME` / `SEED_ADMIN_PASSWORD` 环境变量控制（首次启动时设置）。

---

## 配置（环境变量）

| 变量 | 说明 | 默认值 |
|------|------|--------|
| `SERVER_HOST` | 监听地址；`::` 为 IPv6 双栈，`0.0.0.0` 仅 IPv4 | `0.0.0.0` |
| `SERVER_PORT` | 监听端口（避开浏览器受限端口） | `8000` |
| `DATABASE_PATH` | SQLite 数据库文件路径 | `./data.db` |
| `UPLOAD_DIR` | 用户上传文件存储目录 | `./uploads` |
| `STATIC_DIR` | 前端静态文件目录 | `static` |
| `JWT_SECRET_KEY_FILE` | JWT 密钥文件路径 | `./.secret_key` |
| `SEED_ADMIN_USERNAME` | 首次启动时创建的 admin 用户名 | `admin` |
| `SEED_ADMIN_PASSWORD` | 首次启动时创建的 admin 密码 | *(空)* |
| `MAX_FILE_SIZE` | 单文件最大字节数 | `10737418240` (10GB) |
| `GC_INTERVAL_SEC` | 磁盘孤儿清理周期（秒），0 表示不清理 | `600` |

---

## 前端开发模式

```bash
cd frontend
npm run dev
```

Vite 开发服务器会代理 `/api` 到后端，配合 `cargo run` 使用。开发模式下 `SERVER_PORT` 改为非 100 的端口（如 `8001`）。

---

## 项目结构

```
pan_for_Photographer/
├── src/                    # Rust 后端源码
│   ├── handlers/           # HTTP 请求处理器
│   ├── middleware/         # JWT / 管理员鉴权
│   ├── models/             # 数据模型
│   ├── services/           # 业务逻辑（preview_service、sweeper、batch）
│   ├── config.rs           # 环境变量配置
│   ├── db.rs               # 数据库迁移与种子管理员
│   └── main.rs             # 入口、路由、后台任务
├── frontend/               # Vue 3 前端源码（npm run build → ../static/）
├── static_user/            # 影像交付端（保留） - 只读：登录/浏览/预览/下载，供客户取片
├── start_server.bat        # 默认启动脚本（单端口 + Vue 前端）
├── start_publish.bat       # 发布脚本：交付端(static_user) + 统一前端(static)
├── .gitignore
└── README.md
```

> **不提交到 git**（参见 `.gitignore`）：
> - `target/` — Rust 编译产物
> - `frontend/node_modules/`、`frontend/.vite/`、`frontend/dist/`
> - `static/` — 前端构建产物（由 `npm run build` 重新生成）
> - `uploads/` — 用户上传文件（含隐私内容）
> - `*.db`、`*.db-wal` 等 — SQLite 数据库
> - `.secret_key` — JWT 密钥
> - `.env`、`.env.local` — 环境变量
> - `.trae/` — IDE 本地工作目录（含个人规划文档）

---

## 常用 API 速览

| 方法 | 路径 | 说明 |
|------|------|------|
| POST | `/api/auth/register` | 用户注册 |
| POST | `/api/auth/login` | 用户登录 |
| GET  | `/api/auth/me` | 当前用户信息 |
| GET  | `/api/files` | 列出当前文件夹文件 |
| POST | `/api/files/upload` | 上传文件（multipart 流式） |
| GET  | `/api/files/:id/download` | 下载文件 |
| GET  | `/api/files/:id/media` | 预览 / 缩略图 / 原图 |
| 删除 | `/api/files/:id` | 软删除 |
| POST | `/api/files/:id/restore` | 恢复 |
| 删除 | `/api/files/:id/permanent` | 永久删除 |
| 删除 | `/api/trash` | 清空回收站（并触发即时 GC） |
| GET  | `/api/folders` | 文件夹列表 |
| GET  | `/api/search` | 全局搜索 |
| GET  | `/api/public/shares/:id` | 公开分享详情 |
| GET  | `/api/admin/users` | 管理员用户列表 |

## 存储与后台任务说明

- **上传**：multipart 字段以 chunk 流式写入 `uploads/.tmp_incoming/*.part`，完成后原子重命名为 `user_<id>/`，INSERT 数据库后立即响应；超限文件在流内计数阶段即被拒绝（413）。
- **缩略图**：插入时 `preview_path/thumb_path` 为 NULL，后台任务（`spawn_blocking` + 信号量限并发）生成后 UPDATE。生成完成前前端自动轮询补齐。
- **孤儿清理（GC）**：周期扫描 `uploads/`，对账数据库——孤儿源文件/预览（超 5 分钟）、超龄 `.part`（超 1 小时）会被删除；`preview_path IS NULL` 的图片行自动重新入队补生成。

## 许可证

[MIT](LICENSE)
