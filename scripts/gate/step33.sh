#!/usr/bin/env bash
# Step 33 Gate：power_grid 与 fin_risk 双行业世界装载、L4 认证、6 万行湖表、PII 脱敏与动态视野验收
set -euo pipefail
source "$(dirname "$0")/lib.sh"

api_as() {
  local tok=$1 m=$2 p=$3 body=${4:-}
  if [[ -n "$body" ]]; then
    curl -sS -X "$m" -H "Authorization: Bearer $tok" -H "Accept: application/json" \
         -H "Content-Type: application/json" "$CORE_BASE_URL$p" -d "$body"
  else
    curl -sS -X "$m" -H "Authorization: Bearer $tok" -H "Accept: application/json" "$CORE_BASE_URL$p"
  fi
}

# 1. Python 静态安全守卫、Ruff 与 mypy --strict 全量核验
python3 "$ROOT/scripts/python-guard.py"
if [[ -x "$ROOT/axpoint-parser/.venv/bin/ruff" ]]; then
  (cd "$ROOT/axpoint-parser" && .venv/bin/ruff check . && .venv/bin/mypy --strict .)
else
  (cd "$ROOT/axpoint-parser" && ruff check . && mypy --strict .)
fi

# 2. 执行双行业世界幂等向量化装载脚本并核验各自单世界耗时 (< 3s)
T_PG_0=$(python3 -c 'import time; print(time.time())')
bash "$ROOT/scripts/seed/showcase/power_grid.sh"
T_PG_MS=$(python3 -c 'import sys, time; print(int((time.time() - float(sys.argv[1])) * 1000))' "$T_PG_0")
echo "info power_grid.sh elapsed_ms=${T_PG_MS}"
[[ "$T_PG_MS" -lt 3000 ]] || _fail "power-grid-seed-under-3s" "elapsed_ms=${T_PG_MS}"

T_FIN_0=$(python3 -c 'import time; print(time.time())')
bash "$ROOT/scripts/seed/showcase/fin_risk.sh"
T_FIN_MS=$(python3 -c 'import sys, time; print(int((time.time() - float(sys.argv[1])) * 1000))' "$T_FIN_0")
echo "info fin_risk.sh elapsed_ms=${T_FIN_MS}"
[[ "$T_FIN_MS" -lt 3000 ]] || _fail "fin-risk-seed-under-3s" "elapsed_ms=${T_FIN_MS}"

# shellcheck disable=SC1091
source "$ROOT/.gate/ids.env"

# 3. 核验两套行业模板包只读预览与 SHA-256 校验全部通过
expect_code "tpl-preview-power-grid-200" 200 GET "/api/axops/templates/power_grid/preview"
expect_code "tpl-preview-fin-risk-200" 200 GET "/api/axops/templates/fin_risk/preview"
PG_PREVIEW=$(api GET /api/axops/templates/power_grid/preview)
FIN_PREVIEW=$(api GET /api/axops/templates/fin_risk/preview)
expect_json "tpl-preview-contracts" "$(jq -n --argjson pg "$PG_PREVIEW" --argjson fn "$FIN_PREVIEW" '{pg:$pg, fn:$fn}')" '
  .pg.manifest.id == "power_grid"
  and (.pg.topology_summary.node_types | length) == 6
  and (.pg.topology_summary.edge_types | length) == 5
  and .fn.manifest.id == "fin_risk"
  and (.fn.topology_summary.node_types | length) == 5
  and (.fn.topology_summary.edge_types | length) == 6
'

# 4. C95：验证 power_grid L4_CERTIFIED、324 节点 / >= 548 边、空间多边形围栏与主变跳闸 deadline_step
CERT_PG=$(api POST /api/xblue/subgraphs/power_grid/certify '{}')
PG_NODE_CNT=$(sql_app default "SELECT count(*) FROM xb_nodes WHERE subgraph_slug = 'power_grid'")
PG_LINK_CNT=$(sql_app default "SELECT count(*) FROM xb_links WHERE subgraph_slug = 'power_grid'")
PG_POLY_CNT=$(sql_app default "SELECT count(*) FROM xb_nodes WHERE subgraph_slug = 'power_grid' AND type_name = 'Substation' AND geo_shape->>'type' = 'Polygon'")
PG_TEL_CNT=$(api POST /api/analytics/sql/run '{"sql":"SELECT COUNT(*) AS cnt FROM grid_feeder_telemetry"}')

expect_json "power-grid-l4-certified" "$(jq -n \
  --argjson c "$CERT_PG" \
  --argjson nc "$PG_NODE_CNT" \
  --argjson lc "$PG_LINK_CNT" \
  --argjson pc "$PG_POLY_CNT" \
  --argjson tc "$PG_TEL_CNT" \
  '{c:$c, nc:$nc, lc:$lc, pc:$pc, tc:$tc}')" '
  .c.cert_level == "L4_CERTIFIED"
  and .c.dead_knowledge_cnt == 0
  and .c.high_fanout_cnt == 0
  and .c.trial_tick_passed == true
  and .nc == 324
  and .lc >= 548
  and .pc == 26
  and ((.tc.rows[0].cnt | tonumber) == 43200)
'

PG_SCENARIOS=$(api GET "/api/axsim/sandboxes/${POWER_GRID_SANDBOX_ID}/scenarios")
expect_json "power-grid-feeder-deadline" "$PG_SCENARIOS" '
  (if type == "array" then . else (.items // []) end) as $items
  | ($items | length) >= 1
  and ($items[0].status == "SUCCEEDED")
  and ($items[0].deadline_report.earliest_deadline_step != null)
  and ($items[0].deadline_report.earliest_breach_step != null)
'

# 5. C96：验证 fin_risk L4_CERTIFIED、1,210 节点 / >= 4,260 边、60,000 行对公流水湖表、三道 PII 脱敏与跨分行动态行级视野
CERT_FIN=$(api POST /api/xblue/subgraphs/fin_risk/certify '{}')
FIN_NODE_CNT=$(sql_app default "SELECT count(*) FROM xb_nodes WHERE subgraph_slug = 'fin_risk'")
FIN_LINK_CNT=$(sql_app default "SELECT count(*) FROM xb_links WHERE subgraph_slug = 'fin_risk'")
expect_json "fin-risk-l4-certified" "$(jq -n \
  --argjson c "$CERT_FIN" \
  --argjson nc "$FIN_NODE_CNT" \
  --argjson lc "$FIN_LINK_CNT" \
  '{c:$c, nc:$nc, lc:$lc}')" '
  .c.cert_level == "L4_CERTIFIED"
  and .c.dead_knowledge_cnt == 0
  and .c.high_fanout_cnt == 0
  and .c.trial_tick_passed == true
  and .nc == 1210
  and .lc >= 4260
'

WIRE_CNT=$(api POST /api/analytics/sql/run '{"sql":"SELECT COUNT(*) AS cnt FROM fin_corp_wire_transfers"}')
PIVOT_RES=$(api POST /api/analytics/pivot '{
  "dataset": "fin_corp_wire_transfers",
  "rows": [{"field": "branch_dept"}],
  "columns": [{"field": "risk_tier"}],
  "values": [{"field": "transfer_amt", "agg": "SUM"}, {"field": "tx_count", "agg": "SUM"}],
  "grand_total": true
}')
expect_json "fin-risk-60k-lake-rows" "$(jq -n --argjson w "$WIRE_CNT" --argjson p "$PIVOT_RES" '{w:$w, p:$p}')" '
  ((.w.rows[0].cnt | tonumber) >= 60000)
  and ((.p.row_headers | length) >= 6)
'

ANALYST_LOGIN=$(curl -fsS -X POST "${CORE_BASE_URL}/api/auth/login" \
  -H "Content-Type: application/json" \
  -d "$(jq -nc --arg u "$FIN_ANALYST_SH_USERNAME" --arg p "$FIN_ANALYST_SH_PASSWORD" '{tenant:"default", username:$u, password:$p}')")
ANALYST_TOKEN=$(printf '%s' "$ANALYST_LOGIN" | jq -r '.access_token')

ANALYST_TMP=$(mktemp -d)
trap 'rm -rf "$ANALYST_TMP"' EXIT
api_as "$ANALYST_TOKEN" GET "/api/xblue/nodes?subgraph=fin_risk&type_name=BeneficialOwner&limit=20" > "$ANALYST_TMP/bo.json"
api_as "$ANALYST_TOKEN" GET "/api/xblue/nodes?subgraph=fin_risk&type_name=LegalEntity&limit=1000" > "$ANALYST_TMP/le.json"
api_as "$ANALYST_TOKEN" POST "/api/analytics/sql/run" '{"sql":"SELECT operator_phone, payee_card_no FROM fin_corp_wire_transfers LIMIT 5"}' > "$ANALYST_TMP/wp.json"

PII_RLS_SUMMARY=$(jq -n \
  --slurpfile bo "$ANALYST_TMP/bo.json" \
  --slurpfile le "$ANALYST_TMP/le.json" \
  --slurpfile wp "$ANALYST_TMP/wp.json" '
  ($bo[0]) as $b
  | ($le[0]) as $l
  | ($wp[0]) as $w
  | {
      bo_cnt: ($b.items | length),
      bo_all_masked: ($b.items | all(
        ((.properties.full_name | tostring) | contains("*"))
        and ((.properties.mobile_phone | tostring) | contains("*"))
        and ((.properties.national_id | tostring) | contains("*"))
        and ((.properties.settlement_card | tostring) | contains("*"))
      )),
      wp_cnt: ($w.rows | length),
      wp_all_masked: ($w.rows | all(
        ((.operator_phone | tostring) | contains("*"))
        and ((.payee_card_no | tostring) | contains("*"))
      )),
      sh_healthy_cnt: ($l.items | map(select(.properties.branch_dept == "sh_branch" and (.metrics.overdue_ratio < 0.05))) | length),
      other_overdue_cnt: ($l.items | map(select(.properties.branch_dept != "sh_branch" and (.metrics.overdue_ratio >= 0.05))) | length),
      other_healthy_leaked: ($l.items | map(select(.properties.branch_dept != "sh_branch" and (.metrics.overdue_ratio < 0.05))) | length)
    }
')

expect_json "fin-risk-pii-and-dynamic-rls" "$PII_RLS_SUMMARY" '
  .bo_cnt == 20
  and .bo_all_masked == true
  and .wp_cnt == 5
  and .wp_all_masked == true
  and .sh_healthy_cnt >= 10
  and .other_overdue_cnt >= 10
  and .other_healthy_leaked == 0
'

gate_pass step33
