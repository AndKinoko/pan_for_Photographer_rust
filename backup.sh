#!/bin/sh
# ============================================================================
# 备份 photographer 网盘的全部状态
# ============================================================================
#
# 为什么需要这个脚本：SQLite 在 WAL 模式下，已提交的数据可能还在
# `data.db-wal` 里，而 `cp data.db` 只拷主文件。
#
# 实测到的两种结果：
#   · 服务运行中 cp —— 拿到 0 张表的空壳（连表结构都没有）
#   · 服务刚停、WAL 尚未 checkpoint 时 cp —— 看起来正常（5 张表齐全）
# 第二种**更危险**：它看起来是份好备份，实际缺了最近若干次提交，
# 而你不会知道自己丢了什么。`.backup` API 读的是数据库的逻辑视图，
# 无论 WAL 处于什么状态，拷出的都是一致的快照。
#
# 用法：
#   docker compose exec app sh /app/backup.sh          # 备份到 /data/backups
#   docker compose cp app:/data/backups ./backups      # 取回宿主机
#
# 三个必须一起备的东西：数据库、上传文件、JWT 密钥。
# 丢 secret.key = 所有已签发的 JWT 立即失效 = 全员被强制登出。
# ============================================================================

set -eu

DATA_DIR=/data
OUT_DIR="$DATA_DIR/backups"
STAMP="$(date +%Y%m%d-%H%M%S)"

mkdir -p "$OUT_DIR"

# ── 1. 数据库：.backup 会把 WAL 里的内容一并纳入，是在线安全的 ──
#     不加 .backup 而直接 cp 得到的是空壳，原因见文件头。
sqlite3 "$DATA_DIR/data.db" ".backup '$OUT_DIR/data.db.$STAMP'"

# 立即校验备份可读且表齐全 —— 备份完不验证等于没备
TABLES=$(sqlite3 "$OUT_DIR/data.db.$STAMP" \
         "SELECT COUNT(*) FROM sqlite_master WHERE type='table'")
if [ "$TABLES" -lt 4 ]; then
    echo "备份校验失败：只备份到 $TABLES 张表（预期至少 4），已中止" >&2
    exit 1
fi

# ── 2. 上传文件：原照片不能丢 ──
tar -czf "$OUT_DIR/uploads.tgz.$STAMP" -C "$DATA_DIR/uploads" .

# ── 3. JWT 密钥：单独存一份，权限收紧 ──
cp "$DATA_DIR/secret.key" "$OUT_DIR/secret.key.$STAMP"
chmod 600 "$OUT_DIR/secret.key.$STAMP"

# 密钥不出现在归档里，但数据库和照片在同一个 tar 里会很大，
# 所以这里分文件存放，取回时按需组合。
echo "备份完成（$STAMP）："
ls -lh "$OUT_DIR"/*"$STAMP" | awk '{print "  " $5 "\t" $9}'
echo
echo "提示：这三份文件必须一起保存，缺一不可。"
