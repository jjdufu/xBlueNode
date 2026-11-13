#!/usr/bin/env bash
# 首次初始化数据目录时由 postgres 镜像执行：创建应用角色与湖目录角色并设置口令
set -euo pipefail
psql -v ON_ERROR_STOP=1 --username "$POSTGRES_USER" --dbname "$POSTGRES_DB" \
  -v app_pw="${AXPOINT_APP_DB_PASSWORD}" -v lake_pw="${AXPOINT_LAKE_DB_PASSWORD}" <<'SQL'
SELECT 'CREATE ROLE axpoint_app LOGIN NOBYPASSRLS' WHERE NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname='axpoint_app')\gexec
SELECT 'CREATE ROLE axpoint_lake LOGIN NOBYPASSRLS' WHERE NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname='axpoint_lake')\gexec
ALTER ROLE axpoint_app PASSWORD :'app_pw';
ALTER ROLE axpoint_lake PASSWORD :'lake_pw';
SQL
