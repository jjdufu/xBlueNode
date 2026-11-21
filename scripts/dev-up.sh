#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
for _pg_bin in /usr/lib/postgresql/*/bin; do
  [[ -d "$_pg_bin" ]] && export PATH="$_pg_bin:$PATH"
done
test -f deploy/.env || cp deploy/.env.example deploy/.env

if command -v docker >/dev/null 2>&1 && docker info >/dev/null 2>&1; then
  docker compose -f deploy/docker-compose.yml --env-file deploy/.env up -d postgres
  until docker compose -f deploy/docker-compose.yml exec -T postgres pg_isready -U axpoint -d xbluenode >/dev/null; do sleep 1; done
else
  if ! pg_isready -h 127.0.0.1 -p 5432 -U axpoint -d xbluenode >/dev/null 2>&1; then
    mkdir -p .gate/logs
    rm -rf .gate/pgdata
    initdb -D .gate/pgdata -U axpoint --auth=trust --encoding=UTF8 --locale=C >/dev/null
    cat >> .gate/pgdata/postgresql.conf <<'CONF'
listen_addresses = '127.0.0.1'
port = 5432
unix_socket_directories = '/tmp'
max_connections = 100
CONF
    cat > .gate/pgdata/pg_hba.conf <<'HBA'
local   all             all                                     trust
host    all             all             127.0.0.1/32            md5
HBA
    pg_ctl -D .gate/pgdata -l .gate/logs/postgres.log start >/dev/null
    psql -h /tmp -p 5432 -U axpoint -d postgres -v ON_ERROR_STOP=1 <<'SQL' >/dev/null
ALTER USER axpoint WITH PASSWORD 'axpoint_secret';
CREATE DATABASE xbluenode OWNER axpoint;
\connect xbluenode
CREATE ROLE axpoint_app LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOBYPASSRLS PASSWORD 'axpoint_app_secret';
CREATE ROLE axpoint_lake LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOBYPASSRLS PASSWORD 'axpoint_lake_secret';
SQL
  fi
fi

mkdir -p .gate/jwt/public
if [[ ! -f .gate/jwt/private.pem ]]; then
  openssl genpkey -algorithm ed25519 -out .gate/jwt/private.pem
  openssl pkey -in .gate/jwt/private.pem -pubout -out .gate/jwt/public/1.pem
fi

gen() { python3 -c "import secrets;print(secrets.token_urlsafe(32))"; }
key() { python3 -c "import os,base64;print(base64.b64encode(os.urandom(32)).decode())"; }
if [[ ! -f .env.test ]]; then
  cat > .env.test <<ENV
AXPOINT_ENV=test
MIGRATION_DATABASE_URL=postgres://axpoint:axpoint_secret@127.0.0.1:5432/xbluenode
DATABASE_URL=postgres://axpoint_app:axpoint_app_secret@127.0.0.1:5432/xbluenode
AXPOINT_LAKE_CATALOG_DSN=postgres://axpoint_lake:axpoint_lake_secret@127.0.0.1:5432/xbluenode
AXPOINT_DEV_TOKEN=$(gen)
AXPOINT_INTERNAL_TOKEN=$(gen)
AXPOINT_CREDENTIAL_KEYS=1:$(key)
AXPOINT_MASK_KEYS=1:$(key)
AXPOINT_JWT_PRIVATE_KEY_PATH=$PWD/.gate/jwt/private.pem
AXPOINT_JWT_PUBLIC_KEYS_DIR=$PWD/.gate/jwt/public
AXPOINT_DATA_ROOT=$PWD/.gate/data
AXPOINT_MODEL_PATH=$PWD/.gate/models
AXPOINT_LLM_CASSETTE=$PWD/tests/cassettes
PARSER_BASE_URL=http://127.0.0.1:8001
CORE_BASE_URL=http://127.0.0.1:9995
ENV
fi
set -a
# shellcheck disable=SC1091
source .env.test
set +a
mkdir -p "$AXPOINT_DATA_ROOT"/{lake,assets,exports,tenants} "$AXPOINT_MODEL_PATH" .gate/seeded

(cd axpoint-parser \
  && (uv sync --extra dev 2>/dev/null || (test -d .venv || uv venv .venv; uv pip install -e ".[dev]")) \
  && (test -f charts/package.json && npm --prefix charts ci --omit=dev >/dev/null || true) \
  && .venv/bin/python -c "import importlib.util as u; d=__import__('duckdb').connect() if u.find_spec('duckdb') else None; [d.execute(f'INSTALL {e}') for e in ('ducklake','postgres','mysql_scanner','sqlite_scanner')] if d else None" \
  && .venv/bin/python tests/fixtures/make_fixtures.py)

psql "$MIGRATION_DATABASE_URL" -v ON_ERROR_STOP=1 -c "ALTER ROLE axpoint_app PASSWORD 'axpoint_app_secret'" \
  -c "ALTER ROLE axpoint_lake PASSWORD 'axpoint_lake_secret'" 2>/dev/null || true
sqlx migrate run --source deploy/migrations --database-url "$MIGRATION_DATABASE_URL"
psql "$MIGRATION_DATABASE_URL" -v ON_ERROR_STOP=1 -c "ALTER ROLE axpoint_app PASSWORD 'axpoint_app_secret'" \
  -c "ALTER ROLE axpoint_lake PASSWORD 'axpoint_lake_secret'" >/dev/null
echo "dev-up: OK"
