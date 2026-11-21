#!/usr/bin/env bash
# 用法：backup.sh [FULL|DATABASE|LAKE] [EXISTING_BACKUP_ID]；需要 MIGRATION_DATABASE_URL、AXPOINT_DATA_ROOT、AXPOINT_BACKUP_DIR
set -euo pipefail
: "${MIGRATION_DATABASE_URL:?MIGRATION_DATABASE_URL is required}"
: "${AXPOINT_DATA_ROOT:?AXPOINT_DATA_ROOT is required}"
: "${AXPOINT_BACKUP_DIR:?AXPOINT_BACKUP_DIR is required}"

KIND=${1:-FULL}
EXISTING_ID=${2:-}
case "$KIND" in
  FULL|DATABASE|LAKE) ;;
  *) echo "backup: unsupported kind '$KIND' (expected FULL|DATABASE|LAKE)" >&2; exit 2 ;;
esac

TS=$(date -u +%Y%m%dT%H%M%SZ)
mkdir -p "$AXPOINT_BACKUP_DIR" "$AXPOINT_DATA_ROOT/lake" "$AXPOINT_DATA_ROOT/assets"
DEST="$(cd "$AXPOINT_BACKUP_DIR" && pwd -P)/$TS"
mkdir -p "$DEST/files/lake" "$DEST/files/assets"

if [[ -n "$EXISTING_ID" ]]; then
  ID="$EXISTING_ID"
  psql "$MIGRATION_DATABASE_URL" -v ON_ERROR_STOP=1 -Atqc \
    "UPDATE ax_backups SET status='RUNNING', location='$DEST', started_at=now() WHERE id='$ID'" >/dev/null
else
  ID=$(psql "$MIGRATION_DATABASE_URL" -v ON_ERROR_STOP=1 -Atqc \
    "INSERT INTO ax_backups (backup_kind, status, location) VALUES ('$KIND','RUNNING','$DEST') RETURNING id")
fi

trap 'psql "$MIGRATION_DATABASE_URL" -c "UPDATE ax_backups SET status='"'"'FAILED'"'"', finished_at=now(), error_message='"'"'see logs'"'"' WHERE id='"'"'$ID'"'"'" >/dev/null 2>&1 || true' ERR

if [[ "$KIND" != "LAKE" ]]; then
  pg_dump "$MIGRATION_DATABASE_URL" --format=directory --jobs=4 --file="$DEST/db"
fi

if [[ "$KIND" != "DATABASE" ]]; then
  # 湖目录在数据库中（lake_catalog 模式），数据文件只追加：复制目录与数据文件即可得到一致快照
  LINK_ARGS=()
  if [[ -d "$AXPOINT_BACKUP_DIR/latest/files/lake" ]]; then
    LATEST_LAKE="$(cd "$AXPOINT_BACKUP_DIR/latest/files/lake" && pwd -P)"
    LINK_ARGS=(--link-dest="$LATEST_LAKE")
  fi
  rsync -a "${LINK_ARGS[@]}" "$AXPOINT_DATA_ROOT/lake/" "$DEST/files/lake/"
  rsync -a "$AXPOINT_DATA_ROOT/assets/" "$DEST/files/assets/"
fi

(cd "$DEST" && find . -type f ! -name 'MANIFEST.sha256' -print0 | sort -z | xargs -0 -r sha256sum > MANIFEST.sha256)
MIG=$(psql "$MIGRATION_DATABASE_URL" -v ON_ERROR_STOP=1 -Atqc "SELECT COALESCE(max(version), 0) FROM _sqlx_migrations")
SIZE=$(du -sb "$DEST" | cut -f1)
FILES=$(wc -l < "$DEST/MANIFEST.sha256" | awk '{print $1}')

psql "$MIGRATION_DATABASE_URL" -v ON_ERROR_STOP=1 -Atqc \
  "UPDATE ax_backups SET status='SUCCEEDED', finished_at=now(), size_bytes=$SIZE,
     manifest=jsonb_build_object('migration', $MIG, 'files', $FILES) WHERE id='$ID'" >/dev/null

ln -sfn "$DEST" "$AXPOINT_BACKUP_DIR/latest"
find "$AXPOINT_BACKUP_DIR" -maxdepth 1 -mindepth 1 -type d -mtime +"${AXPOINT_BACKUP_KEEP_DAYS:-14}" -exec rm -rf {} +
echo "backup: $DEST"
