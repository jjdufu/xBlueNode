#!/usr/bin/env bash
# Step 35 Gate：十万级多行业秒级装载（< 8s）、98 项能力溯源与 Phase 4 六段全旅程 E2E 终验
set -euo pipefail
source "$(dirname "$0")/lib.sh"

# 1. 全套 G1 静态守卫与 C01..C98 能力溯源零差集校验
if [[ -n "${NAMING_DENYLIST:-}" || -f "$ROOT/.env" ]]; then
  bash "$ROOT/scripts/naming-guard.sh"
fi
python3 "$ROOT/scripts/lint-guard.py"
python3 "$ROOT/scripts/web-guard.py"
python3 "$ROOT/scripts/python-guard.py"
python3 "$ROOT/scripts/hallucination-guard.py"
python3 "$ROOT/scripts/docs-and-plans-guard.py"
python3 "$ROOT/scripts/check_routes.py" \
  --routes "$ROOT/axpoint-core/crates/axpoint-server/src/routes" \
  --index "$ROOT/docs/standards/A4-API-CONTRACT-INDEX.md"
python3 "$ROOT/scripts/check_traceability.py" \
  "$ROOT/docs/standards/A7-REQUIREMENTS-TRACEABILITY.md" \
  "$ROOT/scripts/gate"
echo "ASSERT traceability-98-ok PASS"

# 2. 后台装载接口 RBAC（401 / 403 PERMISSION_DENIED）与不可篡改审计链集成单测
(cd "$ROOT/axpoint-core" && cargo test -p axpoint-server --test step35_showcase_api --locked)

# 3. 计时运行向量化总装载器（硬红线 <= 8s）并核验全域规模、>= 5 套 PUBLISHED 看板、main 无孤立边与四世界 L4_CERTIFIED
T0=$(date +%s)
T0_MS=$(python3 -c 'import time; print(int(time.time() * 1000))')
bash "$ROOT/scripts/seed-showcase.sh"
ELAPSED=$(( $(date +%s) - T0 ))
ELAPSED_MS=$(( $(python3 -c 'import time; print(int(time.time() * 1000))') - T0_MS ))
echo "info seed-showcase.sh elapsed_s=${ELAPSED} elapsed_ms=${ELAPSED_MS}"
(( ELAPSED <= 8 )) || _fail "showcase-seed-under-8s" "elapsed_s=${ELAPSED} elapsed_ms=${ELAPSED_MS}"
(( ELAPSED_MS <= 8000 )) || _fail "showcase-seed-under-8000ms" "elapsed_ms=${ELAPSED_MS}"

PULSE=$(api GET /api/axops/pulse)
expect_json "showcase-145k-seeded-fast" "$PULSE" '
  .totals.node_count >= 2300
  and .totals.link_count >= 6500
  and .totals.lake_total_rows >= 145000
  and .totals.metric_points_24h >= 80000
  and (.subgraphs | length) >= 5
  and ((.subgraphs | map(select(.slug == "power_grid"))[0].cert_level) == "L4_CERTIFIED")
  and ((.subgraphs | map(select(.slug == "fin_risk"))[0].cert_level) == "L4_CERTIFIED")
  and ((.subgraphs | map(select(.slug == "semi_supply"))[0].cert_level) == "L4_CERTIFIED")
  and ((.subgraphs | map(select(.slug == "med_dispatch"))[0].cert_level) == "L4_CERTIFIED")
'

DASHBOARDS=$(api GET /api/xblueview/dashboards)
expect_json "showcase-5-published-dashboards" "$DASHBOARDS" '
  (if type == "array" then . else (.items // []) end) as $dl
  | (($dl | map(select(.status == "PUBLISHED")) | length) >= 5)
  and ([ "power-grid-cockpit", "fin-risk-cockpit", "semi-supply-cockpit", "med-dispatch-cockpit" ]
       | all(. as $slug | (($dl | map(select(.slug == $slug and .status == "PUBLISHED")) | length) == 1)))
'

MAIN_SCHEMA=$(api GET "/api/xblue/schema?subgraph=main")
expect_json "showcase-main-schema-no-orphan-edges" "$MAIN_SCHEMA" '
  (.node_types | map(.name)) as $mn
  | (.node_types | length) >= 25
  and ([ .edge_types[] | ((.src_type_name as $s | $mn | index($s)) != null and (.dst_type_name as $d | $mn | index($d)) != null) ] | all)
'

# 核验 POST /api/axops/demo/showcase/load 二次幂等调用 HTTP 200、< 8s 返回与 TEMPLATE_INSTALL 审计落库
T_API_0=$(python3 -c 'import time; print(int(time.time() * 1000))')
SHOWCASE_API_RAW=$(curl -sS -w '\n%{http_code}' -X POST \
  -H "Authorization: Bearer $AXPOINT_DEV_TOKEN" \
  -H "Accept: application/json" \
  -H "Content-Type: application/json" \
  "$CORE_BASE_URL/api/axops/demo/showcase/load" -d '{}')
T_API_MS=$(( $(python3 -c 'import time; print(int(time.time() * 1000))') - T_API_0 ))
SHOWCASE_API_CODE=$(printf '%s\n' "$SHOWCASE_API_RAW" | tail -n 1)
SHOWCASE_API_BODY=$(printf '%s\n' "$SHOWCASE_API_RAW" | sed '$d')
echo "info showcase-load-api wall_ms=${T_API_MS} body_elapsed_ms=$(printf '%s' "$SHOWCASE_API_BODY" | jq -r '.elapsed_ms // -1')"
expect_eq "showcase-load-api-200" "$SHOWCASE_API_CODE" "200"
(( T_API_MS < 8000 )) || _fail "showcase-load-api-under-8000ms" "wall_ms=${T_API_MS}"
expect_json "showcase-load-api-contract" "$SHOWCASE_API_BODY" '
  .loaded_subgraphs == ["power_grid", "fin_risk", "semi_supply", "med_dispatch"]
  and .node_count >= 2300
  and .link_count >= 6500
  and .lake_total_rows >= 145000
  and .metric_points_24h >= 80000
  and .elapsed_ms < 8000
'

AUDIT_CNT=$(sql_owner "SELECT count(*) FROM sec_audit_logs WHERE tenant_id = 'default' AND action = 'TEMPLATE_INSTALL' AND resource_ref = 'TEMPLATE:v4_industry_showcase'")
[[ "${AUDIT_CNT:-0}" -ge 1 ]] || _fail "showcase-load-audit-chain" "AUDIT_CNT=${AUDIT_CNT:-0}"
echo "ok  showcase-load-audit-chain"

# 4. Phase 4 六段全旅程 Playwright E2E（独立桩模式 + AXPOINT_E2E_LIVE=1 真实后端双轨全绿）
PW_PORT_STUB="${PLAYWRIGHT_PORT:-$(( 4173 + ${WT_SLOT:-0} * 100 ))}"
PW_PORT_LIVE="$(( PW_PORT_STUB + 1 ))"

(cd "$ROOT/xbluenode-web" && CI=1 AXPOINT_E2E_LIVE=0 PLAYWRIGHT_PORT="$PW_PORT_STUB" npx playwright test e2e/v4-modeling-and-worlds.spec.ts)
(cd "$ROOT/xbluenode-web" && AXPOINT_E2E_LIVE=1 PLAYWRIGHT_PORT="$PW_PORT_LIVE" npx playwright test e2e/v4-modeling-and-worlds.spec.ts)
echo "ASSERT v4-journey-pass PASS"

gate_pass step35
