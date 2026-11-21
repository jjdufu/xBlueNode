#!/usr/bin/env bash
# 用法：restore-drill.sh <备份目录>；在独立的临时 PostgreSQL 实例与临时数据目录中恢复并对账
set -euo pipefail
: "${MIGRATION_DATABASE_URL:?MIGRATION_DATABASE_URL is required}"
for _pg_bin in /usr/lib/postgresql/*/bin; do
  [[ -d "$_pg_bin" ]] && export PATH="$_pg_bin:$PATH"
done

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
SRC_ARG=${1:?backup dir required}
SRC="$(cd "$SRC_ARG" && pwd -P)"

(cd "$SRC" && sha256sum --quiet -c MANIFEST.sha256)

DRILL_ROOT=$(mktemp -d)
DRILL_PG_ROOT=$(mktemp -d)
DRILL_PORT=${AXPOINT_DRILL_PG_PORT:-55432}
USE_DOCKER=0

cleanup() {
  if [[ "$USE_DOCKER" == "1" ]]; then
    docker rm -f axp-drill-pg >/dev/null 2>&1 || true
  elif [[ -f "$DRILL_PG_ROOT/pgdata/postmaster.pid" ]]; then
    pg_ctl -D "$DRILL_PG_ROOT/pgdata" -m immediate stop >/dev/null 2>&1 || true
  fi
  rm -rf "$DRILL_ROOT" "$DRILL_PG_ROOT"
}
trap cleanup EXIT

if command -v docker >/dev/null 2>&1 && docker info >/dev/null 2>&1; then
  USE_DOCKER=1
  docker rm -f axp-drill-pg >/dev/null 2>&1 || true
  DOCKER_VOL_ARGS=()
  if [[ -d "$SRC/db" ]]; then
    DOCKER_VOL_ARGS=(-v "$SRC/db:/dump_db:ro")
  fi
  docker run -d --rm --name axp-drill-pg \
    -e POSTGRES_USER=axpoint \
    -e POSTGRES_PASSWORD=drill \
    -e POSTGRES_DB=xbluenode \
    "${DOCKER_VOL_ARGS[@]}" \
    -p "127.0.0.1:${DRILL_PORT}:5432" \
    pgvector/pgvector:pg16 >/dev/null
  until docker exec axp-drill-pg pg_isready -U axpoint -d xbluenode >/dev/null 2>&1; do
    sleep 1
  done
else
  initdb -D "$DRILL_PG_ROOT/pgdata" -U axpoint --auth=trust --encoding=UTF8 --locale=C >/dev/null
  cat >> "$DRILL_PG_ROOT/pgdata/postgresql.conf" <<CONF
listen_addresses = '127.0.0.1'
port = ${DRILL_PORT}
unix_socket_directories = '/tmp'
max_connections = 60
CONF
  cat > "$DRILL_PG_ROOT/pgdata/pg_hba.conf" <<'HBA'
local   all             all                                     trust
host    all             all             127.0.0.1/32            md5
HBA
  pg_ctl -D "$DRILL_PG_ROOT/pgdata" -l "$DRILL_PG_ROOT/postgres.log" start >/dev/null
  psql -h /tmp -p "$DRILL_PORT" -U axpoint -d postgres -v ON_ERROR_STOP=1 <<'SQL' >/dev/null
ALTER USER axpoint WITH PASSWORD 'drill';
CREATE DATABASE xbluenode OWNER axpoint;
SQL
fi

DRILL_URL="postgres://axpoint:drill@127.0.0.1:${DRILL_PORT}/xbluenode"
psql "$DRILL_URL" -v ON_ERROR_STOP=1 -Atqc "
DO \$\$
BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'axpoint_app') THEN
    CREATE ROLE axpoint_app LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOBYPASSRLS PASSWORD 'drill';
  END IF;
  IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'axpoint_lake') THEN
    CREATE ROLE axpoint_lake LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOBYPASSRLS PASSWORD 'drill';
  END IF;
END \$\$;
" >/dev/null

if [[ -d "$SRC/db" ]]; then
  if [[ "$USE_DOCKER" == "1" ]]; then
    docker exec axp-drill-pg pg_restore \
      --dbname="postgres://axpoint:drill@127.0.0.1:5432/xbluenode" \
      --jobs=4 --no-owner --role=axpoint /dump_db
  else
    pg_restore --dbname="$DRILL_URL" --jobs=4 --no-owner --role=axpoint "$SRC/db"
  fi
fi
if [[ -d "$SRC/files" ]]; then
  cp -a "$SRC/files/." "$DRILL_ROOT/"
fi

PY_BIN="python3"
if [[ -x "$ROOT/axpoint-parser/.venv/bin/python" ]]; then
  PY_BIN="$ROOT/axpoint-parser/.venv/bin/python"
elif command -v python >/dev/null 2>&1 && python -c "import psycopg, duckdb" >/dev/null 2>&1; then
  PY_BIN="python"
fi

"$PY_BIN" "$ROOT/scripts/ops/drill_reconcile.py" \
  --source "$MIGRATION_DATABASE_URL" \
  --target "$DRILL_URL" \
  --lake-root "$DRILL_ROOT" \
  --backup-location "$SRC"

psql "$MIGRATION_DATABASE_URL" -v ON_ERROR_STOP=1 -Atqc \
  "UPDATE ax_backups SET status='VERIFIED', verified_at=now() WHERE location='$SRC'" >/dev/null

echo "restore-drill: OK"
