# Pan For Photographer

为摄影师团队设计的局域网文件共享服务。Rust + Axum + SQLite 高性能后端，Vue 3 前端，支持 RAW 格式在线预览（NEF / CR2 / ARW 等）。

## 功能特性

- **文件管理**：上传、下载、预览、重命名、移动、复制、删除
- **文件夹管理**：新建、重命名、进入/返回、批量移动/复制
- **回收站**：软删除、恢复、永久删除
- **批量操作**：文件和文件夹的批量移动、复制、删除、分享
- **多媒体预览**：图片、RAW（含内嵌 JPEG 提取）、视频、音频、PDF
- **大文件流式上传**：逐文件 multipart 流式写盘，内存占用与文件大小解耦（常数级）
- **缩略图异步生成**：后台队列 + 并发闸（permits=2），上传响应毫秒级返回，缩略图自动补齐
- **磁盘孤儿清理**：周期 GC 对账（孤儿文件 / 超龄 .part / 缺失缩略图重投）
- **公开分享**：密码保护、过期时间
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
- Windows（提供 `.bat` 启动脚本）

## 快速开始（默认使用 start_server.bat）

### 1. 构建前端

```bash
cd frontend
npm install
npm run build
```

构建产物输出到项目根目录的 `static/` 文件夹（已在 `.gitignore` 中，不提交）。

### 2. 准备 JWT 密钥

后端启动需要一个密钥文件，内容为任意安全随机字符串：

```
# Windows PowerShell
Set-Content -Path .secret_key -Value 'put-a-long-random-string-here' -NoNewline
```

### 3. 启动服务

双击或执行 `start_server.bat`：

```bat
start_server.bat
```

默认配置：

- 监听：`SERVER_HOST=::`（IPv6 双栈，同时兼容 IPv4/IPv6 访问）+ `SERVER_PORT=0100`
- 访问地址：`http://localhost:100`
- 静态目录：`static`（即上一步构建的前端）
- 上传目录：`./uploads`，数据库：`./data.db`

### 4. 登录

管理员账号（首次启动自动创建）：**AKIHANA / ljyljy**

## 配置（环境变量）

| 变量 | 说明 | 默认值 |
|------|------|--------|
| `SERVER_HOST` | 监听地址；`::` 为 IPv6 双栈，`0.0.0.0` 仅 IPv4 | `0.0.0.0` |
| `SERVER_PORT` | 监听端口（避开浏览器受限端口，如 101/102 等） | `8000` |
| `DATABASE_PATH` | SQLite 数据库文件路径 | `./data.db` |
| `UPLOAD_DIR` | 用户上传文件存储目录 | `./uploads` |
| `STATIC_DIR` | 前端静态文件目录 | `static` |
| `JWT_SECRET_KEY_FILE` | JWT 密钥文件路径（必须存在） | `./.secret_key` |
| `MAX_FILE_SIZE` | 单文件最大字节数（同时也是单请求总量预算） | `10737418240` (10GB) |
| `GC_INTERVAL_SEC` | 磁盘孤儿清理周期（秒），0 表示不清理 | `600` |

## 前端开发模式

```bash
cd frontend
npm run dev
```

Vite 开发服务器会代理 `/api` 到后端，配合 `cargo run` 使用。开发模式下 `SERVER_PORT` 改为非 100 的端口（如 `8001`），避免与 100 冲突。

## 双端口发布模式（可选，start_publish.bat）

除默认的 `start_server.bat` 外，还提供 `start_publish.bat` 双端口启动：

- `8001`：普通用户前端（`static_user/`）
- `8002`：管理端前端（`static_admin/`）

> 注意：`static_user/`、`static_admin/` 属于非默认资产，已在 `.gitignore` 中忽略、不随仓库提交。若需要双端口模式，请从你的本地副本保留这两个目录。

## 项目结构

```
pan_for_Photographer/
├── src/                    # Rust 后端源码
│   ├── handlers/           # HTTP 请求处理器
│   ├── middleware/         # JWT / 管理员鉴权
│   ├── models/             # 数据模型
│   ├── services/           # 业务逻辑（含 preview_service、sweeper）
│   ├── config.rs           # 环境变量配置
│   ├── db.rs               # 数据库迁移与种子管理员
│   └── main.rs             # 入口、路由、后台任务
├── frontend/               # Vue 前端源码（npm run build → ../static/）
├── static/                 # 前端构建产物（自动生成，不提交）
├── uploads/                # 用户上传文件（自动生成，不提交）
├── start_server.bat        # 默认启动脚本（单端口 + Vue 前端）
├── start_publish.bat       # 可选双端口启动脚本
├── .gitignore
└── README.md
```

## 常用 API 速览

| 方法 | 路径 | 说明 |
|------|------|------|
| POST | `/api/auth/register` | 用户注册 |
| POST | `/api/auth/login` | 用户登录 |
| GET  | `/api/files` | 列出当前文件夹文件 |
| POST | `/api/files/upload` | 上传文件（multipart 流式） |
| GET  | `/api/files/:id/download` | 下载文件 |
| GET  | `/api/files/:id/media` | 预览 / 缩略图 / 原图 |
| DELETE | `/api/files/:id` | 软删除 |
| DELETE | `/api/files/:id/permanent` | 永久删除 |
| DELETE | `/api/trash` | 清空回收站（并触发即时 GC） |
| GET  | `/api/folders` | 文件夹列表 |
| GET  | `/api/search` | 全局搜索 |
| GET  | `/api/public/shares/:id` | 公开分享详情 |
| GET  | `/api/admin/users` | 管理员用户列表 |

## 存储与后台任务说明

- **上传**：multipart 字段以 chunk 流式写入 `uploads/.tmp_incoming/*.part`，完成后 `rename` 原子提交到 `user_<id>/`，随后 INSERT 数据库并立即响应；超限文件在流内计数阶段即被拒绝（413）。
- **缩略图**：插入时 `preview_path/thumb_path` 为 NULL，后台任务（`spawn_blocking` + 信号量限并发）生成后 UPDATE。生成完成前前端会自动轮询补齐，无需手动刷新。
- **孤儿清理（GC）**：周期扫描 `uploads/`，对账数据库——孤儿源文件/预览（超 5 分钟）、超龄 `.part`（超 1 小时）会被删除；`preview_path IS NULL` 的图片行自动重新入队补生成。

## 许可证

[MIT](LICENSE)
