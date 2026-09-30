# 部署指南（Cloudflare Tunnel + Docker）

**日期**：2026-09-27
**适用**：家用服务器 / Windows Docker Desktop / Ubuntu

---

## 0. 三分钟速览

```bash
# 1. 准备配置（唯一需要改的文件）
cp deploy.env.example deploy.env
#    编辑 deploy.env：至少填 TUNNEL_ID / TUNNEL_TOKEN 与 CORS_ALLOWED_ORIGINS

# 2. 生成 JWT 密钥（缺了它容器会 panic 重启）
mkdir -p data
docker run --rm -v "$(pwd)/data:/data" alpine \
  sh -c 'head -c 48 /dev/urandom | base64 > /data/secret.key && chmod 600 /data/secret.key'

# 3. 构建并启动
docker compose --env-file deploy.env up -d --build

# 4. 确认没有对外暴露端口（这条是安全的关键）
docker compose ps        # PORTS 列应为空
```

### ⚠️ 还有一步在 Cloudflare 那边，不在本地

`cloudflared` 容器启动后并不知道该把流量转发到哪里——
**源站地址是在 Cloudflare dashboard 里配的**（不在 compose 文件里）。
少了这一步，隧道会启动但所有请求都失败。

Zero Trust → Networks → Tunnels → 点你的隧道 → **Public Hostname** → Add：

| 字段 | 填什么 |
|---|---|
| Subdomain | 随意，例如 `photos` |
| Domain | 你的域名 |
| Type | `HTTP`（**不要选 HTTPS**——源站是明文 HTTP，选 HTTPS 会一直报 525） |
| URL | `app:8000`（服务名 + 端口） |

填错 Type 的症状：访问时返回 **525 SSL Handshake Failed**。
此时 `docker compose logs cloudflared` 里能看到握手错误。

---

## 1. 配置只有一处

**所有配置都在 `deploy.env`**（从 `deploy.env.example` 复制）。compose 通过
`env_file` 读它，`--env-file` 读它，裸机 systemd 也读它。

这样做的原因：先前 `start_server.bat` / `start_publish.bat` / 各处默认值
分散在多个地方，改一个参数要同时改三处，漏一处就出诡异问题。
现在改一处、全部生效。

| 变量 | 说明 |
|---|---|
| `SERVER_HOST` | 容器内**必须** `0.0.0.0`；裸机**必须** `127.0.0.1`。见下方「易错点」 |
| `SERVER_PORT` | 容器内 8000 |
| `DATABASE_PATH` / `UPLOAD_DIR` / `JWT_SECRET_KEY_FILE` | 都在 `/data`（同一个卷） |
| `STATIC_DIR` | `/app/static`（镜像内已构建好） |
| `MAX_FILE_SIZE` | 单文件上限，默认 10GB |
| `CORS_ALLOWED_ORIGINS` | **公网部署必须填自己的域名** |
| `TUNNEL_ID` / `TUNNEL_TOKEN` | 隧道凭据，只在 compose 里用 |

---

## 2. 易错点（每一条都踩过或验证过）

### 容器里必须用 `0.0.0.0`，不是 `127.0.0.1`

容器内的 `127.0.0.1` 指向**容器自身**。`cloudflared` 在另一个容器里，
用它连 `127.0.0.1` 会连到自己。连通性靠 compose 网络 + 服务名 `app:8000`。

**安全性不靠绑定地址，而靠不写 `ports:`。** compose.yaml 里刻意没有
`ports:` ——宿主机上因此没有任何入口。这与裸机部署正好相反：

| | 绑定地址 | 安全边界 |
|---|---|---|
| 裸机 | `127.0.0.1` | 绑定地址 |
| 容器 | `0.0.0.0` + 不映射端口 | 网络隔离 |

### `data.db` / `uploads/` / `secret.key` 必须同一个卷

SQLite 的 `.db`、`-wal`、`-shm` 三个文件拆开存放会丢 WAL。
compose 里用一个 `pan-data` 卷覆盖整个 `/data`。

### `deploy.env` 不要提交

含 `TUNNEL_TOKEN`、`SEED_ADMIN_PASSWORD`。已加进 `.gitignore`，
只提交 `deploy.env.example` 作为模板。

### 用 named volume，不要 bind mount

Windows 上的 bind mount 不支持 `chown`，非 root 进程（UID 10001）
会因权限不足无法写入 `uploads/`。named volume 没有这个问题。

---

## 3. 验证

```bash
# 健康检查
docker compose exec app curl -fsS http://127.0.0.1:8000/api/health

# 看日志
docker compose logs -f app

# 确认没有对外端口（应只显示 8000/tcp 而无 0.0.0.0:xxxx->8000）
docker compose ps
```

**上线前必做**：传一个 150MB 的文件。Cloudflare 对单次请求有 100MB 上限
（Free/Pro，Enterprise 可调），另外还有 125 秒的 Proxy Read Timeout。
这决定了你是否需要分片上传。详见 `DEPLOY_RECHECK.md` §1。

---

## 4. 备份（**不能用 `cp`**）

WAL 模式下已提交的数据可能还在 `-wal` 文件里，而 `cp data.db` 只拷主文件。
实测的两种结果：

- **服务运行中** `cp` → 得到 0 张表的空壳
- **服务刚停、WAL 未 checkpoint** 时 `cp` → 看起来正常（表结构齐全）

第二种更危险：它像一份好备份，实际缺了最近若干次提交，而你不会知道丢了什么。
`.backup` API 读数据库的逻辑视图，无论 WAL 什么状态都给出一致快照。

用镜像里自带的脚本（它还会**校验表数**，防止假备份蒙混过关）：

```bash
docker compose exec app sh /app/backup.sh
docker compose cp app:/data/backups ./backups-$(date +%F)
```

**丢失 `secret.key` 会让所有已签发的 JWT 立即失效**，用户被强制登出。

---

## 5. 日常运维

```bash
docker compose up -d          # 启动
docker compose logs -f        # 看日志
docker compose restart app    # 改完 deploy.env 后重启（配置只在启动时读）
docker compose down           # 停止（-v 会删卷，数据就没了，慎用）
docker compose exec app sh    # 进容器
```

升级：新代码拉下来后 `docker compose up -d --build`。
数据库 schema 由 `src/db.rs` 的 `init_db` 用 `CREATE TABLE IF NOT EXISTS` +
按列比对的方式迁移，不需要单独的迁移步骤。

---

## 6. 还没做的

- **分片上传**：Cloudflare 单请求 100MB 上限是硬约束，前端目前是单请求整文件上传
  （`frontend/src/api.js` 的 `uploadFiles`）。需要大文件不受限就得改成分片。
- **SPA fallback 补全**：`/admin`、`/search`、`/trash` 直接访问或刷新会 404
  （`src/main.rs` 只给 `/share/*` 挂了 fallback）。局域网里容易忽略，公网上会误报「打不开」。
- **TraceLayer 不记 query**：`frontend/src/api.js` 把 JWT 拼进缩略图 URL，
  而 `src/main.rs` 的 `TraceLayer::new_for_http()` 默认记录 URI → **7 天有效期的
  bearer token 明文进日志**。公网部署前应改掉。
- **Cloudflare Access 保护 `/admin`**：Access 默认拒绝，可用路径规则单独保护
  管理端。注意官方警告：**先建 Access application，再配 tunnel route**，
  否则配好隧道前应用对全互联网开放。
