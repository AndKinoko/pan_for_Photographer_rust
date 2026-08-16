# Pan For Photographer

一款为摄影师团队设计的局域网文件共享服务端，使用 Rust + Axum + SQLite 构建高性能后端，Vue 3 + Vite 构建现代化前端。

## 功能特性

- **文件管理**：上传、下载、预览、重命名、移动、复制、删除
- **文件夹管理**：新建、重命名、进入/返回、批量移动/复制
- **回收站**：软删除、恢复、永久删除
- **批量操作**：支持文件和文件夹的批量移动、复制、删除、分享
- **多媒体预览**：图片（含 RAW 格式）、视频、音频、PDF 在线预览，自动生成缩略图
- **公开分享**：生成分享链接，支持密码保护和过期时间
- **用户系统**：注册、登录、JWT 鉴权，管理员面板可管理用户
- **全局搜索**：按关键词、类型、时间、大小筛选文件
- **主题切换**：支持亮色/暗色模式

## 技术栈

- **后端**：Rust、Axum、Tokio、SQLx (SQLite)、image、uuid
- **前端**：Vue 3、Vite、原生 CSS
- **鉴权**：JWT + bcrypt
- **存储**：本地文件系统 + SQLite

## 环境要求

- Rust 1.80+
- Node.js 20+
- SQLite（使用 SQLx 内联迁移，无需单独安装）

## 快速开始

### 1. 克隆仓库并安装依赖

```bash
git clone <仓库地址>
cd pan_for_Photographer

# 安装前端依赖
cd frontend
npm install
cd ..

# 构建后端
cargo build --release
```

### 2. 构建前端静态资源

```bash
cd frontend
npm run build
cd ..
```

构建产物会输出到项目根目录的 `static/` 文件夹，由 Rust 后端直接托管。

### 3. 配置环境变量

复制示例配置（如果需要）并编辑 `.env`：

```bash
# .env
PORT=8000
DATABASE_URL=sqlite:./app.db
UPLOAD_DIR=./uploads
JWT_SECRET=your-super-secret-key
MAX_FILE_SIZE=104857600
RUST_LOG=info
```

| 变量 | 说明 | 默认值 |
|------|------|--------|
| `PORT` | 服务监听端口 | `8000` |
| `DATABASE_URL` | SQLite 数据库地址 | `sqlite:./app.db` |
| `UPLOAD_DIR` | 用户上传文件存储目录 | `./uploads` |
| `JWT_SECRET` | JWT 签名密钥 | 自动生成到 `.secret_key` |
| `MAX_FILE_SIZE` | 最大允许上传字节数 | `104857600` (100MB) |
| `RUST_LOG` | 日志级别 | `info` |

> **注意**：`.env`、数据库文件、`uploads/`、`static/` 均已加入 `.gitignore`，不会被提交。

### 4. 启动服务

```bash
cargo run --release
```

访问 http://localhost:8000 即可使用。

## 开发模式

### 后端热重载

```bash
cargo watch -x run
```

### 前端独立开发

```bash
cd frontend
npm run dev
```

前端开发服务器会代理 `/api` 请求到 `http://localhost:8000`。

## 项目结构

```
pan_for_Photographer/
├── src/                    # Rust 后端源码
│   ├── handlers/           # HTTP 请求处理器
│   ├── services/           # 业务逻辑层
│   ├── models/             # 数据模型
│   ├── db.rs               # 数据库连接与迁移
│   ├── config.rs           # 应用配置
│   ├── errors.rs           # 错误定义
│   └── main.rs             # 入口与路由
├── frontend/               # Vue 前端源码
│   ├── src/
│   │   ├── views/          # 页面视图
│   │   ├── components/     # 可复用组件
│   │   ├── composables/    # 组合式函数
│   │   ├── api.js          # API 封装
│   │   ├── router.js       # 路由配置
│   │   └── style.css       # 全局样式
│   └── vite.config.js
├── static/                 # 前端构建产物（自动生成）
├── uploads/                # 用户上传文件（自动生成）
├── .gitignore
├── Cargo.toml
└── README.md
```

## 常用 API 速览

| 方法 | 路径 | 说明 |
|------|------|------|
| POST | `/api/auth/register` | 用户注册 |
| POST | `/api/auth/login` | 用户登录 |
| GET  | `/api/files` | 列出当前文件夹文件与子文件夹 |
| POST | `/api/files/upload` | 上传文件 |
| GET  | `/api/files/:id/download` | 下载文件 |
| GET  | `/api/files/:id/media` | 预览/缩略图 |
| GET  | `/api/folders` | 列出子文件夹 |
| POST | `/api/folders` | 新建文件夹 |
| GET  | `/api/trash` | 回收站列表 |
| POST | `/api/trash/restore` | 恢复文件/文件夹 |
| DELETE | `/api/trash/:id` | 永久删除 |
| GET  | `/api/search` | 全局搜索 |
| GET  | `/api/public/shares/:id` | 公开分享详情 |
| GET  | `/api/public/shares/:id/media` | 公开分享媒体预览 |
| GET  | `/api/admin/users` | 管理员用户列表 |

完整接口定义请参考 `src/handlers/` 与 `src/main.rs`。

## 贡献

欢迎提交 Issue 和 Pull Request。

## 许可证

[MIT](LICENSE)
