# Pan For Photographer

为摄影师团队设计的文件交付与分享服务。Rust + Axum + SQLite 后端，Vue 3 前端，
支持 RAW 格式在线预览（NEF / CR2 / ARW 等）。

> **🔒 隐私提示**：本仓库 **不包含** 任何用户数据、数据库、上传文件、JWT 密钥或管理员密码。
> 所有运行时数据需在首次部署时自行创建。

---

## 功能特性

- **文件管理**：上传、下载、预览、重命名、移动、复制、删除
- **文件夹管理**：新建、重命名、进入/返回、批量移动/复制
- **回收站**：软删除、恢复、永久删除
- **批量操作**：批量移动、复制、删除、分享；单次上限 500 项
- **传输抽屉**：上传队列 + 下载队列，实时进度、取消、重试
- **多媒体预览**：图片、RAW（含内嵌 JPEG 提取）、视频、音频、PDF
- **大文件流式上传**：逐文件 multipart 流式写盘，内存占用与文件大小解耦
- **缩略图异步生成**：后台队列 + 并发闸，上传响应毫秒级返回，缩略图自动补齐
- **磁盘孤儿清理**：周期 GC 对账（孤儿文件 / 超龄 .part / 缺失缩略图重投，失败重投上限 3 次）
- **公开分享**：密码保护、过期时间、下载次数限制
- **邀请制注册**：管理员生成邀请码，客户凭码自助开通账号，码用后即废
- **用户配额**：每用户默认 5GB，管理员可按用户调整；用量含回收站，上传在落盘前校验
- **用户系统**：注册 / 登录 / JWT（7 天有效），管理员面板管理用户（含有效期、配额、角色）
- **交互细节**：隐形全屏拖放上传、移动端顶栏滚动自动收起、侧边栏容量进度条
- **主题切换**：亮色 / 暗色

## 技术栈

- **后端**：Rust、Axum 0.7、Tokio、SQLx (SQLite)、image、uuid、tower-http
- **前端**：Vue 3、Vite 8、Lucide 图标、原生 CSS
- **鉴权**：JWT（HS256）+ bcrypt
- **存储**：本地文件系统 + SQLite（WAL 模式）
- **部署**：Docker 多阶段构建 + Cloudflare Tunnel

## 环境要求

| 场景 | 要求 |
|------|------|
| 容器部署（推荐） | Docker + Docker Compose |
| 裸机部署 | Rust 1.80+、Node.js 20+（仅前端构建需要） |
| 平台 | Windows / macOS / Linux |

---

## 快速开始

### 方式 A：Docker（推荐，公网部署用这个）

```bash
# 1. 准备配置
cp deploy.env.example deploy.env
#    编辑 deploy.env：至少填 TUNNEL_ID 与 SEED_ADMIN_PASSWORD

# 2. 构建镜像并启动
docker build -t pan-for-photographer:latest .
docker compose --env-file deploy.env up -d
```

Windows 下可直接双击 `docker_start_server.bat`，它封装了配置检查、密钥生成、
构建与启动，并提供 `dev` / `undev` / `backup` / `status` 等子命令。

**设计要点**：compose.yaml **刻意不写 `ports:`**——宿主机上因此没有任何入口，
外部流量只能经 Cloudflare Tunnel 进来。安全边界来自网络隔离而非绑定地址。

本机调试需要浏览器访问时：

```bash
docker compose --env-file deploy.env --profile dev up -d app-dev   # 开 127.0.0.1:8000
docker compose --env-file deploy.env --profile dev rm -sf app-dev  # 用完收回
```

> `app-dev` 只绑 `127.0.0.1`，局域网与公网都访问不到。
> **公网部署后不要留着它**——「宿主机无入口」是这套方案的核心前提。

> ⚠️ 容器内 `SERVER_HOST` 必须是 `0.0.0.0`（容器里的 `127.0.0.1` 指向容器自身，
> cloudflared 连不上）。**这与裸机部署正好相反**，裸机应设为 `127.0.0.1`。

### 方式 B：裸机

```bash
# 1. 构建前端（输出到 ../static/）
cd frontend && npm ci && npm run build && cd ..

# 2. 生成 JWT 密钥（必须，缺失或短于 32 字节会 panic）
openssl rand -base64 48 > .secret_key     # Windows PowerShell 见 README 历史版本

# 3. 启动
cargo run --release
```

Windows 下双击 `start_server.bat`（等价于 `cargo run --release`）。

默认监听 `0.0.0.0:8000`，访问 `http://localhost:8000`。

---

## 首次配置管理员

管理员由 `SEED_ADMIN_USERNAME` / `SEED_ADMIN_PASSWORD` 决定。
**密码留空则不创建任何管理员**，该提示只出现在容器日志里。

```bash
# docker 部署：编辑 deploy.env 后 docker compose restart app
SEED_ADMIN_USERNAME=admin
SEED_ADMIN_PASSWORD=YourStrongPass!2026
```

若密码留空导致没有管理员，**注册制下无法自助提权**（注册不写 `role`，
数据库默认为 `user`）。补救方式：

```bash
sqlite3 data.db "UPDATE users SET role='admin' WHERE username='你的用户名'"
```

> 源码中不硬编码任何默认账号密码。
> 已存在 admin 时 `seed_admin` 幂等跳过，不会覆盖既有密码。

---

## 邀请制注册

注册接口**必须**携带有效邀请码。管理员在管理端「注册邀请码」区批量生成，
码在客户注册时消费、用后即废。

- 字母表去掉了 `0/O`、`1/I/l` 等手抄易混字符，输入大小写与空格均容错
- **建号与消费码在同一事务内**，并发用同一码不会超发出两个账号
- 注册失败（如用户名已存在）会回滚码的占用，客户可改用户名重试
- 码存明文：它是要抄给客户的凭据，安全性来自「这张表只有管理员能读」

流程：管理员发码 → 客户在登录页点「注册」→ 前端先验码（`/api/auth/invite/verify`，不消费）
→ 填用户名密码完成注册。

---

## 配置（环境变量）

| 变量 | 说明 | 默认值 |
|------|------|--------|
| `SERVER_HOST` | 监听地址。**容器内必须 `0.0.0.0`；裸机应 `127.0.0.1`** | `0.0.0.0` |
| `SERVER_PORT` | 监听端口 | `8000` |
| `DATABASE_PATH` | SQLite 数据库路径 | `./data.db` |
| `UPLOAD_DIR` | 上传文件存储目录 | `./uploads` |
| `STATIC_DIR` | 前端静态目录 | `static` |
| `JWT_SECRET_KEY_FILE` | JWT 密钥文件路径（≥ 32 字节，否则 panic） | `./.secret_key` |
| `MAX_FILE_SIZE` | 单次上传请求体上限（字节） | `10737418240` (10GB) |
| `GC_INTERVAL_SEC` | 孤儿清理周期（秒）。⚠️ 填 0 **不是关闭**，会回落到 600 秒（`src/services/sweeper.rs`） | `600` |
| `SEED_ADMIN_USERNAME` | 初始管理员用户名 | `admin` |
| `SEED_ADMIN_PASSWORD` | 初始管理员密码；**空则不创建** | *(空)* |
| `CORS_ALLOWED_ORIGINS` | 追加的跨域白名单（逗号或空格分隔） | *(空)* |
| `RUST_LOG` | 日志过滤器；**调成 `debug` 会开启 tower_http 请求日志** | `pan_for_photographer=info,tower_http=info` |
| `TUNNEL_ID` | Cloudflare 隧道 ID（**docker compose 缺它会拒绝启动**） | *(无)* |
| `TUNNEL_TOKEN` | 隧道令牌（与 `TUNNEL_ID` 二选一） | *(空)* |

> 配置**只在进程启动时读取**，改完必须重启。
> 裸机部署时 `src/config.rs` 的默认值是**相对路径**（`./data.db`、`static`），
> 用 systemd 时务必给绝对路径并设好 `WorkingDirectory`。

> **时间约定**：数据库与后端内部一律使用 **UTC**（`YYYY-MM-DD HH:MM:SS`，
> 与 SQLite `datetime('now')` 一致）；有效期均按 UTC 比较，前端在展示时转本地时区。

---

## API

共 49 个路由。认证接口除 `health`、注册、登录、验码与公开分享外均需
`Authorization: Bearer <token>`。

### 认证

| 方法 | 路径 | 说明 |
|------|------|------|
| POST | `/api/auth/register` | 注册（**必须带 `invite_code`**） |
| POST | `/api/auth/invite/verify` | 校验邀请码是否可用（**不消费**） |
| POST | `/api/auth/login` | 登录 |
| GET | `/api/auth/me` | 当前用户信息（含 `used_bytes`） |

### 文件与文件夹

| 方法 | 路径 | 说明 |
|------|------|------|
| GET | `/api/files` | 当前文件夹文件（游标分页） |
| POST | `/api/files/upload` | 上传（multipart 流式） |
| GET | `/api/files/:id/download` | 下载 |
| GET | `/api/files/:id/media` | 预览 / 缩略图 / 原图 |
| PUT | `/api/files/:id/rename` | 重命名 |
| DELETE | `/api/files/:id` | 软删除 |
| POST | `/api/files/:id/restore` | 恢复 |
| DELETE | `/api/files/:id/permanent` | 永久删除 |
| GET/POST | `/api/folders` | 文件夹列表 / 新建 |
| PUT | `/api/folders/:id/rename` | 重命名文件夹 |
| DELETE | `/api/folders/:id` | 软删除文件夹 |
| POST | `/api/folders/:id/restore` | 恢复文件夹 |
| DELETE | `/api/folders/:id/permanent` | 永久删除文件夹 |

> `GET /api/folders?parent_id=N` 同时返回 `breadcrumbs`（祖先链）。
> 面包屑**逐跳校验归属**，传入他人文件夹 ID 返回空数组而非其内容。

### 回收站 / 搜索 / 批量

| 方法 | 路径 | 说明 |
|------|------|------|
| GET | `/api/trash` | 回收站内容（游标分页） |
| DELETE | `/api/trash` | 清空回收站（触发即时 GC） |
| GET | `/api/search` | 全局搜索（游标分页） |
| POST | `/api/batch/move` \| `copy` \| `delete` \| `share` \| `unshare` | 批量操作，**单次上限 500** |

### 分享

| 方法 | 路径 | 说明 |
|------|------|------|
| GET/POST | `/api/shares` | 我的分享列表 / 创建 |
| GET/DELETE | `/api/shares/:id` | 分享详情 / 撤销 |
| GET | `/api/public/shares/:id` | 公开分享详情（无需鉴权） |
| POST | `/api/public/shares/:id/verify` | 校验分享密码 |
| GET | `/api/public/shares/:id/download` \| `media` | 公开下载 / 预览 |

> 分享码为 uuid4（122 bit 熵，不可枚举）。**默认不设过期、不限次数、无密码**——
> 建分享时需显式指定，否则等于永久公开访问。文件被软删除后分享链接立即失效。

### 管理员

| 方法 | 路径 | 说明 |
|------|------|------|
| GET/POST | `/api/admin/users` | 用户列表 / 新建用户 |
| PUT | `/api/admin/users/:id` | 编辑用户（账号/密码/有效期/配额） |
| PUT | `/api/admin/users/:id/role` | 改角色 |
| DELETE | `/api/admin/users/:id` | 删除用户（级联清理其文件与分享） |
| GET/POST | `/api/admin/users/:id/folders` | 列出 / 新建该用户的文件夹 |
| GET/POST | `/api/admin/invite-codes` | 邀请码列表 / 批量生成 |
| DELETE | `/api/admin/invite-codes/:id` | 删除邀请码 |
| GET | `/api/admin/stats` | 系统统计 |

### 健康检查

| 方法 | 路径 | 说明 |
|------|------|------|
| GET | `/api/health` | 存活探针，无需鉴权 |

---

## 安全设计

- **JWT 7 天有效**，无服务端吊销机制；`AuthUser` 提取器在验签后**必须再查库**
  确认账号存在且未过期，因此账号到期/删除能立即生效（`src/services/account_guard.rs`）
- **bcrypt**（`DEFAULT_COST = 12`）存密码；密码最小长度 6 位
- **登录限流**：用户名 + 来源 IP 双维度，连续 5 次失败后指数退避（5s 起、
  封顶 15 分钟、30 分钟无活动清零）。**计数在进程内存中，重启清零**
- **上传扩展名黑名单**（`.html/.svg/.js/.hta` 等 11 种）拦截上传与改名；
  真正边界是**预览白名单** `is_inline_safe`——白名单外一律降级为
  `attachment` + `application/octet-stream` + CSP sandbox
- **路径穿越不可达**：磁盘文件名由服务端 `uuid4` 生成，用户输入从不参与路径构造
- **下载额度原子占用**：`UPDATE ... WHERE download_count < max_downloads`，
  并发下不会超发
- **文件名净化**：拒绝 `..`、路径分隔符、控制字符（含 CR/LF）、双引号
- **错误响应不泄露内部信息**：SQL 错误、文件路径、bcrypt 内部错误一律不返回客户端
- **请求日志不含 query string**：`TraceLayer` 自定义 span 只记 `method`/`path`，
  `?token=` 旁路不会让 JWT 落进日志（与 `RUST_LOG` 级别无关）
- **CORS 白名单**：无通配符、无 `allow_credentials` 组合风险

## 已知限制

- **上传不分片**：前端单请求整文件上传。Cloudflare 单请求上限 100MB（Free/Pro），
  超过这个大小的原图传不上去——`MAX_FILE_SIZE` 配再大也没用
- **SPA 深链 404**：`/admin`、`/search`、`/trash`、`/shares` 直接访问或刷新返回 404，
  只有 `/share/*` 挂了 fallback。局域网里易忽略，公网上会误报「打不开」
- **无令牌吊销**：JWT 泄漏后 7 天内无法作废；改密码也不吊销已签发的令牌
- **无自助改密**：改密码只能走管理端 `PUT /api/admin/users/:id`，
  即用户**改自己的密码也得用管理员账号操作**
- **限流计数重启清零**：单实例可接受，多副本部署会失效
- **登录限流的 IP 维度在 Tunnel 下失真**：拿到的是 cloudflared 容器 IP，
  全站用户共享一个桶（一次针对 `admin` 的爆破会让所有人集体退避）。
  建议在 Cloudflare 侧对 `/api/auth/*` 加 WAF Rate Limiting 规则
- **前端无自动化测试**：`package.json` 只有 `dev` / `build` / `preview`

---

## 日常运维

Windows 下双击 `docker_start_server.bat` 会打开交互式菜单（启动 / 本机调试 /
停止 / 日志 / 重新构建 / 备份 / 状态 / 清理资源 / 收回调试）。也可以带参数调用：

```bash
docker compose --env-file deploy.env <命令>
```

### 孤儿文件清理

孤儿文件是「磁盘上有、数据库里没有记录」的文件，只在异常路径后产生——进程被强杀、
用户被删除、上传在 rename 与 INSERT 之间中断。它们会持续占用磁盘，需要清理。

**两种触发方式**：

| 方式 | 说明 |
|---|---|
| 管理端按钮 | 「系统统计」区块下方 → **立即清理孤儿文件**，点完立即执行并返回本次删了多少 |
| 自动 | 后台每 **24 小时**跑一次 |

清理会删除三类文件：孤立的源文件、孤立的预览/缩略图、超龄 1 小时的 `.part` 临时文件。
有 5 分钟宽限期保护「上传在途」和「缩略图生成中」的文件，不会误删。

> 「上次执行时间」不落盘——服务重启后重新计 24 小时是符合预期的，
> 重启后手动点一次即可。

### 缩略图重投

与孤儿清理分开运行，**保持 10 分钟一次**（`GC_INTERVAL_SEC`）。它只补生成
`preview_path IS NULL` 的文件，不扫磁盘，开销极低。

两者必须分开：缩略图生成失败（磁盘抖动、内存不足）需要尽快重试，拖到 24 小时
会让用户盯着空图标等一整天；而孤儿清理是纯磁盘操作，稀疏跑没影响。

单个文件最多重试 3 次（`MAX_PREVIEW_ATTEMPTS`）——没有这个上限时，损坏的文件
会被每轮反复解码，配合 RAW 的全量读盘会演变成周期性 OOM。

### 清空回收站后的即时清理

用户清空回收站时会**立即触发一次**孤儿清理，不等 24 小时——刚删完 20GB
需要马上释放磁盘。

## 备份

**不能用 `cp` 拷数据库。** WAL 模式下已提交的数据可能还在 `-wal` 文件里，
`cp data.db` 会得到 0 张表的空壳，或更危险的「看着正常但缺最近提交」的副本。
用 `sqlite3` 的 `.backup` API，它读逻辑视图，任意 WAL 状态下都给出��致快照。

```bash
# 容器部署：镜像自带脚本，还会校验表数防止假备份蒙混过关
docker compose --env-file deploy.env exec app sh /app/backup.sh
docker compose --env-file deploy.env cp app:/data/backups ./backups-$(date +%F)
```

`data.db`、`uploads/`、`secret.key` **三者必须一起备份**。
丢失 `secret.key` 会让所有已签发的 JWT 立即失效（用户被强制登出）。

---

## 测试

```bash
cargo test          # 133 个测试，约 11 秒
```

覆盖：时间工具（UTC 约定）、JWT / 密码哈希 / 分享凭证、文件名与扩展名校验、
图片解码、批量重名、数据库迁移、种子管理员幂等性、分页游标、
邀请码全链路（发码 → 验码 → 注册 → 失效 → 事务回滚）、
以及一组 HTTP 层集成测试——跨用户越权、注册必须带码、下载额度并发不超发、
上传吞错、日志不含令牌等。

前端暂无自动化测试。

---

## 项目结构

```
pan_for_Photographer_rust/
├── src/                        # Rust 后端
│   ├── handlers/               # HTTP 处理器
│   │   ├── auth.rs             #   注册（邀请制）/ 登录 / 验码
│   │   ├── files.rs            #   上传 / 下载 / 预览 / 回收站
│   │   ├── folders.rs          #   文件夹
│   │   ├── share.rs            #   分享与公开分享
│   │   ├── batch.rs            #   批量操作
│   │   ├── admin.rs            #   管理端（含邀请码管理）
│   │   └── search.rs
│   ├── middleware/             # AuthUser / AdminUser 提取器
│   ├── models/                 # 数据模型
│   ├── services/
│   │   ├── file_service.rs     #   扩展名策略 / 预览白名单 / 路径构造
│   │   ├── folder_service.rs
│   │   ├── share_service.rs
│   │   ├── invite_code_service.rs  # 邀请码生成与事务化消费
│   │   ├── account_guard.rs    #   账号有效期校验
│   │   ├── preview_service.rs  #   缩略图 / RAW 内嵌 JPEG
│   │   └── sweeper.rs          #   孤儿清理（手动+24h）/ 缩略图重投（10min）
│   ├── utils/                  # crypto / time / pagination / login_throttle
│   ├── db.rs                   # 建表、迁移、种子管理员
│   ├── config.rs               # 环境变量
│   ├── main.rs                 # 入口、路由、静态服务
│   └── http_tests.rs           # HTTP 层集成测试
├── frontend/                   # Vue 3 源码（npm run build → ../static/）
├── static_user/                # 独立交付端（客户只读取片）
├── Dockerfile                  # 三阶段：frontend → backend → runtime
├── compose.yaml                # app + cloudflared（刻意不映射端口）
├── deploy.env.example          # 配置模板（复制成 deploy.env 后填写）
├── backup.sh                   # 备份脚本（装在镜像里）
├── docker_start_server.bat     # 容器化启动（交互式菜单）
├── start_server.bat            # 裸机启动
├── start_publish.bat           # 双端口发布（交付端 + 统一前端）
├── DEPLOY.md                   # Cloudflare Tunnel 部署指南
├── .gitignore
└── README.md
```

> **不提交到 git**（详见 `.gitignore`）：
>
> | 类别 | 内容 |
> |---|---|
> | 编译产物 | `target/`、`frontend/node_modules/`、`static/` |
> | 运行时数据 | `uploads/`、`*.db` / `*.db-wal` / `*.db-shm`、`.secret_key` |
> | 凭据 | `deploy.env`、`.env`（只提交 `.example` 模板） |
> | 备份 | `*.snapshot`、`backups*/`、`migrate_backup/`、`cleanup_backup/`、`*.tgz` |
> | 过程性文档 | `docs/`、`TEST_REPORT_*.md`、`PLAN_*.md`、`.trae/` |
> | 本机工具配置 | `.claude/`、`.vscode/`、`.idea/` |
> | 废弃残留 | `src/srcok/`、`static_admin/` |
> | 大体积媒体 | `*.nef` / `*.cr2` / `*.arw` 等 RAW 原片 |
>
> `docs/` 里是本机的工作记录（审查报告、修复计划、测试报表），记录的是
> 「当时怎么做的」，对使用者没有指导意义——当前状态看本 README 与 DEPLOY.md 即可。

## 相关文档

| 文档 | 用途 |
|---|---|
| [DEPLOY.md](DEPLOY.md) | Cloudflare Tunnel 部署指南（首次部署前必读） |

日常运维命令见本 README 的「日常运维」章节。

## 许可证

本项目为私有项目，尚未附加 LICENSE 文件。若要开源或分发，
请先补上明确的许可证文件（MIT / Apache-2.0 等）。
