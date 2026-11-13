#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "$0")/lib.sh"

# 0. 门禁断言函数自测
bash "$(dirname "$0")/lib_test.sh"

# 1. 迁移无待执行
expect_eq "migrations-applied" \
  "$(sqlx migrate info --source deploy/migrations --database-url "$MIGRATION_DATABASE_URL" | awk '/pending/{c++} END{print c+0}')" "0"

# 2. 身份边界
expect_code "health-public" 200 GET_NOAUTH /api/health
expect_code "stats-requires-token" 401 GET_NOAUTH /api/stats
expect_code "stats-with-token" 200 GET /api/stats
expect_json "me-is-dev" "$(api GET /api/auth/me)" '.tenant_id == "default" and (.roles | index("ADMIN") != null)'
expect_code "tenant-param-rejected" 400 POST /api/documents/ingest \
  '{"tenant_id":"x","title":"t","source_name":"t.md","content":"c"}'

# 3. 错误体与回退
expect_json "api-404-json" "$(api GET /api/does-not-exist)" '.error_code == "ROUTE_NOT_FOUND" and (.trace_id | length > 0)'
expect_eq "spa-fallback" "$(curl -s -o /dev/null -w '%{http_code}' "$CORE_BASE_URL/graph")" "200"
ASSET_FILE="$(find xbluenode-web/dist/assets -maxdepth 1 -type f -printf '%f\n' | sed -n '1p')"
expect_eq "assets-immutable" \
  "$(curl -sI "$CORE_BASE_URL/assets/$ASSET_FILE" | awk 'BEGIN{IGNORECASE=1} /^cache-control:.*immutable/{c=1} END{print c+0}')" "1"

# 4. 行级安全
bash scripts/seed-demo.sh 00
expect_eq "rls-no-tenant" "$(sql_app '' 'SELECT count(*) FROM xb_nodes')" "0"
expect_eq "rls-default" "$(sql_app default 'SELECT count(*) >= 3 FROM xb_nodes')" "t"
expect_eq "rls-other-tenant" "$(sql_app t_other 'SELECT count(*) FROM xb_nodes')" "0"

# 5. Sidecar 内部令牌
expect_eq "parser-requires-internal-token" \
  "$(curl -s -o /dev/null -w '%{http_code}' -F file=@axpoint-parser/tests/fixtures/plain.txt "$PARSER_BASE_URL/parse")" "401"

# 6. 指标
expect_eq "metrics" "$(curl -s http://127.0.0.1:9464/metrics | awk '/^axpoint_http_requests_total/{c=1} END{print c+0}')" "1"

# 7. 单测与冒烟
(cd axpoint-core && cargo test --workspace -q)
(cd axpoint-parser && pytest -q tests)
(cd xbluenode-web && npx playwright test e2e/smoke.spec.ts)

gate_pass step00
