# Pan For Photographer

给摄影师用的网盘：把原片交给客户，或者生成一个带密码的分享链接发过去。

Rust + Axum + SQLite 后端，Vue 3 前端。NEF / CR2 / ARW 这些 RAW 能在线预览。

> 本仓库不含任何用户数据、数据库、上传文件、密钥或密码。所有运行时数据在首次部署时由程序自己创建。

---

## 能干什么

- **文件**：上传、下载、预览、重命名、移动、复制、删除
- **文件夹**：新建、重命名、批量移动/复制
- **回收站**：删了能捞回来，也可以彻底删掉
- **批量操作**：一次最多选 500 项，支持移动/复制/删除/分享
- **预览**：图片、RAW（自动提取内嵌预览图）、视频、音频、PDF
- **分享**：可设密码、过期时间、下载次数上限
- **账号**：邀请码注册，每个账号一个配额（默认 5GB），管理员可以改
- **上传**：拖进窗口就行，断网/服务端出错会自动重试
- **界面**：亮色/暗色切换

## 怎么跑

### 用 Docker（推荐）

```bash
cp deploy.env.example deploy.env
# 编辑 deploy.env，至少要填 TUNNEL_ID 和 SEED_ADMIN_PASSWORD

docker compose --env-file deploy.env up -d --build
```

Windows 上可以直接双击 `docker_start_server.bat`，它会检查配置、生成密钥、构建并启动，
还带一个菜单（启动 / 调试 / 日志 / 备份 / 状态）。

**compose.yaml 里故意没写 `ports:`。** 所以宿主机上没有任何入口，流量只能通过
Cloudflare Tunnel 进来——安全边界靠的是网络隔离，不是绑定地址。

想在本机浏览器里看看，就临时开一个调试端口：

```bash
docker compose --env-file deploy.env --profile dev up -d app-dev    # 开 127.0.0.1:8000
docker compose --env-file deploy.env --profile dev rm -sf app-dev   # 用完就收回
```

⚠️ 公网部署后别留着它。

### 不用 Docker

```bash
cd frontend && npm ci && npm run build && cd ..   # 前端产物输出到 static/
openssl rand -base64 48 > .secret_key             # JWT 密钥，必须有
cargo run --release
```

然后打开 `http://localhost:8000`。Windows 上双击 `start_server.bat` 等价。

---

## 第一次要配的东西

**管理员账号**由环境变量决定：

```
SEED_ADMIN_USERNAME=admin
SEED_ADMIN_PASSWORD=换成你的强密码
```

密码留空就**不会**创建管理员，登录页底下的注册也要邀请码，所以进不去管理端。
真发生了就用这条命令补一个：

```bash
sqlite3 data.db "UPDATE users SET role='admin' WHERE username='你的用户名'"
```

**客户账号**不用你手动建。管理端有个「注册邀请码」，生成几个发给客户，
他们在登录页点注册、填码就能自己开通。码用一次就废。

> 程序只在首次启动时创建管理员，之后不会覆盖已有密码。

---

## 配置项

配置只在**启动时**读一次，改完要重启。

| 变量 | 说明 | 默认 |
|---|---|---|
| `SERVER_HOST` | 监听地址。**容器里必须是 `0.0.0.0`；裸机应该是 `127.0.0.1`** | `0.0.0.0` |
| `SERVER_PORT` | 端口 | `8000` |
| `DATABASE_PATH` | SQLite 文件位置 | `./data.db` |
| `UPLOAD_DIR` | 上传文件存哪 | `./uploads` |
| `STATIC_DIR` | 前端静态目录 | `static` |
| `JWT_SECRET_KEY_FILE` | JWT 密钥文件（至少要 32 字节，不然启动就崩） | `./.secret_key` |
| `MAX_FILE_SIZE` | 单个文件上传上限（字节） | `10737418240`（10GB） |
| `GC_INTERVAL_SEC` | 缩略图补生成的间隔（秒） | `600` |
| `SEED_ADMIN_USERNAME` | 初始管理员用户名 | `admin` |
| `SEED_ADMIN_PASSWORD` | 初始管理员密码，**留空就不创建** | 空 |
| `CORS_ALLOWED_ORIGINS` | 额外放行的跨域来源 | 空 |
| `RUST_LOG` | 日志级别 | `pan_for_photographer=info,tower_http=info` |
| `TUNNEL_ID` / `TUNNEL_TOKEN` | Cloudflare 隧道凭据，二选一 | 无 |

> 裸机的默认值是相对路径。用 systemd 时记得给绝对路径，并设好 `WorkingDirectory`。
>
> 数据库里所有时间都是 UTC，前端显示时才转成你本地时区。

---

## 备份

**别用 `cp` 拷数据库。** 开了 WAL 模式，已提交的数据可能还在 `-wal` 文件里，
`cp data.db` 拿到的要么是空壳，要么是「看着正常但缺最近几次提交」的假备份，
后者更危险——你不会知道自己丢了什么。

用镜像里带的脚本，它走 SQLite 的 `.backup`，还会校验表数：

```bash
docker compose --env-file deploy.env exec app sh /app/backup.sh
docker compose --env-file deploy.env cp app:/data/backups ./backups-$(date +%F)
```

`data.db`、`uploads/`、`secret.key` **三个要一起备份**。
把 `secret.key` 弄丢的话，所有人都会被强制登出。

---

## 磁盘清理

程序会自己收拾两种垃圾：

- **孤儿文件**（磁盘上有、数据库里没记录，只在进程被强杀之类的异常后才出现）：
  管理端有个「立即清理」按钮，另外每 24 小时自动跑一次。
- **缩略图没生成出来的文件**：每 10 分钟重试一次，最多试 3 次。

用户清空回收站时会立刻触发一次清理，不用等 24 小时。

正在上传、正在生成缩略图的文件有 1 小时保护期，不会被误删。

---

## 已知的坑

- **大于 100MB 的文件传不上去。** 这是 Cloudflare 免费版单请求的上限，不是本程序的
  限制——`MAX_FILE_SIZE` 调再大也没用。要传更大的原片就得改成分片上传，目前没做。
- **没有令牌吊销。** JWT 有效期 7 天，改密码也不会让已发出的令牌失效。
- **不能自己改密码。** 只能让管理员在后台改。
- **限流的 IP 维度在 Tunnel 下不准。** 拿到的是 cloudflared 容器的 IP，所有人共用一个
  计数器。建议在 Cloudflare 那边给 `/api/auth/*` 加一条 WAF 限流规则。

---

## 仓库里有什么、没什么

仓库是**纯源码**，克隆下来约 1 MB、91 个文件。不含任何运行时数据。

**必须提交（少了就构建不了）**

| 文件 | 少了会怎样 |
|---|---|
| `Cargo.lock` | binary crate，锁文件保证依赖版本可复现 |
| `frontend/package-lock.json` | `npm ci` 和 Docker 前端阶段都依赖它 |
| `frontend/`（除 `node_modules/`） | 前端源码与 `vite.config.js` |
| `src/` | 后端源码 |
| `Dockerfile` / `compose.yaml` / `deploy.env.example` | 容器化部署 |
| `backup.sh` | 会被 `COPY` 进镜像 |

**不提交（`.gitignore` 里）**

`target/`、`node_modules/`、**`static/`**、`uploads/`、各类 `*.db`、
`.secret_key`、`deploy.env`、`docs/`。

> ⚠️ `static/` 是前端构建产物，**故意不入库**。所以裸机启动必须先
> `npm run build` 生成它，否则后端虽然能编译但打开页面是 404
> （`ServeDir` 找不到文件，走 SPA 回退也没东西可回退）。
> Docker 不受影响——镜像构建时会现跑一次前端构建。

---

## 项目结构

```
├── src/                    后端
│   ├── handlers/           HTTP 接口（认证 / 文件 / 文件夹 / 分享 / 批量 / 管理端 / 搜索）
│   ├── middleware/         登录与管理员权限校验
│   ├── services/           业务逻辑（文件、文件夹、分享、邀请码、预览生成、磁盘清理）
│   ├── utils/              加密、时间、分页、登录限流
│   ├── db.rs               建表与迁移
│   ├── main.rs             入口、路由
│   └── http_tests.rs       HTTP 层集成测试
├── frontend/               Vue 3 源码（npm run build 输出到 static/）
├── Dockerfile              三阶段构建：前端 → 后端 → 运行镜像
├── compose.yaml            app + cloudflared
├── deploy.env.example      配置模板，复制成 deploy.env 再填
├── backup.sh               备份脚本（已装进镜像）
├── docker_start_server.bat Windows 下的 Docker 启动菜单
├── start_server.bat        裸机启动
└── DEPLOY.md               Cloudflare Tunnel 部署指南
```

---

## 开发

```bash
cargo test        # 172 个测试（85 个 #[test] + 87 个 #[tokio::test]）
```

前端还没有自动化测试，只有 `npm run dev` / `build` / `preview`。

调试时改前端：

```bash
cd frontend && npm run dev    # Vite 开发服务器，/api 自动代理到 localhost:8000
```

---

## 部署

公网部署看 [DEPLOY.md](DEPLOY.md)，里面写了 Cloudflare Tunnel 怎么配、
常见错误怎么排查（比如填错 Type 会一直报 525）。

---

## 许可证

私有项目，还没加 LICENSE 文件。如果要开源或分发，请先补一个（MIT / Apache-2.0 等）。
