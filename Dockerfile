# ============================================================================
# 多阶段构建：前端 → 后端 → 运行时
# ============================================================================
#
# 三个阶段各有分工：
#   frontend —— 产出 static/（Vue 构建产物，Rust 用 ServeDir 提供）
#   backend  —— 编译静态链接的 Rust 二进制
#   runtime  —— 只带运行期真正需要的东西，非 root 运行
#
# 为什么要分阶段而不是一个镜像装 Node 和 Rust：
# 交付照片的服务器不需要几百 MB 的构建工具链，
# 而 `static/` 是 gitignore 的产物（见 .gitignore），
# 镜像里必须现构建——不能指望仓库里带一份。

# ───────────────────────── 阶段 1：前端 ─────────────────────────
FROM node:22-bookworm-slim AS frontend
WORKDIR /build/frontend

# 先只拷 manifest，让依赖层能被缓存：源码改动不会触发重装依赖
COPY frontend/package.json frontend/package-lock.json ./
# 用 npm ci 而不是 npm install：严格按 lock 文件装，可复现
RUN npm ci

COPY frontend/ ./
# vite.config.js 里 outDir 指向 ../static
RUN npm run build

# ───────────────────────── 阶段 2：后端 ─────────────────────────
FROM rust:1-bookworm AS backend
WORKDIR /src

# 依赖预热层：先用空 main.rs 把所有依赖编译成 rlib，
# 之后拷入真实源码只需重编本 crate，不必重编两百多个依赖。
#
# 已实测：touch src/main.rs 后重编，cargo 只重编本 crate 一个，
# 其余依赖 rlib 全部命中缓存 —— 正是这一层想要的效果。
#
# -j $(nproc) 是刻意的：cargo 默认就按核数并行，但在 2 核家用小主机上
# 编译 image（AVIF/zune_jpeg 等）那批 crate 时峰值内存很高，
# 并行度过高会被 OOM killer 干掉 —— 这是自托管构建最常见的翻车方式。
# 如果你的机器内存 < 4G，可以显式改成 -j 2。
COPY Cargo.toml Cargo.lock ./
RUN mkdir -p src \
 && echo 'fn main() {}' > src/main.rs \
 && cargo build --release --locked -j "$(nproc)" \
 && rm -rf src

COPY src/ ./src
# 触碰 main.rs 强制重新编译本 crate（依赖层已经命中缓存）
RUN touch src/main.rs && cargo build --release --locked -j "$(nproc)"

# ───────────────────────── 阶段 3：运行时 ─────────────────────────
FROM debian:bookworm-slim

# curl 供 HEALTHCHECK 使用；ca-certificates 让 HTTP 客户端能验证 TLS。
# sqlite3 不是运行时依赖，但备份必须用它——WAL 模式下直接 cp data.db
# 会得到 1 页空壳（实测），所以「.backup」是唯一安全的路径，
# 得让容器里真的有这个工具，而不是在文档里写一条跑不通的命令。
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl sqlite3 \
 && rm -rf /var/lib/apt/lists/*

# 非 root 运行。UID 固定 10001，避免与宿主上其他容器的用户冲突。
RUN groupadd -g 10001 pan \
 && useradd -u 10001 -g pan -m -d /app pan

WORKDIR /app

COPY --from=backend /src/target/release/pan_for_photographer /app/pan
# 前端构建产物。STATIC_DIR 指向这里。
COPY --from=frontend /build/static /app/static
# 备份脚本。放进镜像而不是让人手敲 —— 备份命令里的引号转义很容易出错，
# 而「备份了却恢复不出来」恰恰是最坏的故障形态，不能靠手敲。
COPY backup.sh /app/backup.sh
RUN chmod +x /app/backup.sh

# 数据目录：数据库、上传文件、JWT 密钥都放在这里，映射成一个卷。
# 三个必须在同一卷——SQLite 的 .db / -wal / -shm 拆开存放会丢 WAL。
RUN mkdir -p /data/uploads && chown -R pan:pan /data /app

USER pan

# 所有配置都有默认值，实际值由 compose 的 env_file 注入。
# 这里的取值只保证 `docker run` 单独跑也能起得来。
ENV SERVER_HOST=0.0.0.0 \
    SERVER_PORT=8000 \
    DATABASE_PATH=/data/data.db \
    UPLOAD_DIR=/data/uploads \
    STATIC_DIR=/app/static \
    JWT_SECRET_KEY_FILE=/data/secret.key \
    MAX_FILE_SIZE=10737418240 \
    GC_INTERVAL_SEC=600 \
    RUST_LOG=info

EXPOSE 8000

# /api/health 是无鉴权的存活探针（见 src/main.rs）。
# --fail 让非 2xx 返回非零退出码，这正是 HEALTHCHECK 需要的语义。
#
# 端口必须从环境变量取，不能写死 8000：deploy.env 是用户会改的地方，
# 端口一改，写死的探针就永远打错端口 → 容器永远 unhealthy →
# compose 的 `depends_on: service_healthy` 让 cloudflared 永不启动。
# HEALTHCHECK 的 CMD 走 shell，因此 ${SERVER_PORT:-8000} 会被展开
# （若改成 exec 形式就不会展开，这是这里必须用 shell 的原因）。
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD curl -fsS "http://127.0.0.1:${SERVER_PORT:-8000}/api/health" || exit 1

CMD ["/app/pan"]
