#!/usr/bin/env bash
# Step 34 Gate：semi_supply 与 med_dispatch 双行业世界装载、模板预览、L4 认证、AxQL 跨层防膨胀与急救车凸包验收
set -euo pipefail
source "$(dirname "$0")/lib.sh"

# 0. Python 静态守卫 + Ruff + Mypy --strict 全量检查
python3 "$ROOT/scripts/python-guard.py"
if [[ -x "$ROOT/axpoint-parser/.venv/bin/ruff" && -x "$ROOT/axpoint-parser/.venv/bin/mypy" ]]; then
  (cd "$ROOT/axpoint-parser" && .venv/bin/ruff check . && .venv/bin/mypy --strict .)
fi

# 1. 幂等装载双行业仿真世界并校验模板包 SHA-256 预览接口返回 200
bash "$ROOT/scripts/seed/showcase/semi_supply.sh"
bash "$ROOT/scripts/seed/showcase/med_dispatch.sh"

expect_code "template-preview-semi-supply-200" 200 GET /api/axops/templates/semi_supply/preview
expect_code "template-preview-med-dispatch-200" 200 GET /api/axops/templates/med_dispatch/preview

# 2. C97（上）：核验 semi_supply L4_CERTIFIED、4 大业务域复合泳道 × 三层语义结构、30,000 行传感湖表与免连接 AxQL SUM_BY_ENTITY 防膨胀
CERT_SEMI=$(api POST /api/xblue/subgraphs/semi_supply/certify '{}')
SCHEMA_SEMI=$(api GET "/api/xblue/schema?subgraph=semi_supply")
expect_json "semi-supply-l4-and-swimlanes" "$(jq -n --argjson c "$CERT_SEMI" --argjson s "$SCHEMA_SEMI" '{c:$c, s:$s}')" '
  .c.cert_level == "L4_CERTIFIED"
  and .c.dead_knowledge_cnt == 0
  and .c.high_fanout_cnt == 0
  and .c.trial_tick_passed == true
  and (.s.node_types | length) == 8
  and (([.s.node_types[].domain_group] | unique | sort) == ["制造域", "履约域", "物流域", "采购域"])
  and (([.s.node_types[].semantic_layer_tier] | unique | sort) == ["CUBE_ROLLUP", "EVENT_DYNAMIC", "STATIC_FACT"])
'

SEMI_CNT=$(api POST /api/analytics/sql/run '{"sql":"SELECT COUNT(*) AS cnt FROM semi_lot_telemetry"}')
expect_json "semi-supply-30k-lake-rows" "$SEMI_CNT" '(.rows[0].cnt | tonumber) >= 30000'

AXQL_SEMI=$(api POST /api/analytics/axql/run '{
  "subgraph_slug": "semi_supply",
  "axql": "SELECT SUM_BY_ENTITY(f.wafer_starts_wpm BY f) AS exact_wpm, COUNT(l.node_key) AS lot_cnt FROM MATCH (f:WaferFab)-[:RUNS_LOT]->(l:WaferLotEvent) WHERE f.node_key IN ('\''FAB-01'\'', '\''FAB-02'\'')"
}')
expect_json "semi-supply-axql-sum-by-entity" "$AXQL_SEMI" '
  .plan.fanout_protected == true
  and (.rows[0].exact_wpm | tonumber) == 80000
  and (.rows[0].lot_cnt | tonumber) == 5
'

# 3. C97（下）：核验 med_dispatch L4_CERTIFIED、18,000 行急诊分诊明细、五区看板动作回写与 120 急救车空间凸包
CERT_MED=$(api POST /api/xblue/subgraphs/med_dispatch/certify '{}')
MED_CNT=$(api POST /api/analytics/sql/run '{"sql":"SELECT COUNT(*) AS cnt FROM med_triage_events"}')
expect_json "med-dispatch-18k-lake-rows" "$MED_CNT" '(.rows[0].cnt | tonumber) >= 18000'

HULL_MED=$(api POST /api/xblue/geo/convex-hull '{"subgraph":"med_dispatch","type_name":"AmbulanceUnit"}')
expect_json "med-dispatch-convex-hull-and-l4" "$(jq -n --argjson c "$CERT_MED" --argjson h "$HULL_MED" '{c:$c, h:$h}')" '
  .c.cert_level == "L4_CERTIFIED"
  and .c.dead_knowledge_cnt == 0
  and .c.high_fanout_cnt == 0
  and .c.trial_tick_passed == true
  and .h.point_count >= 50
  and .h.hull.type == "Polygon"
  and .h.area_sq_deg > 0
'

# 核验 activate_surge_beds 对象动作契约无审批阻塞原子更新 total_free_beds 与 surge_level
SURGE_ACT=$(api POST /api/xblue/objects/HospitalCampus/HOSP-01/actions/activate_surge_beds "$(jq -n --arg ik "step34-surge-$(date +%s)-$$" '{inputs:{added_beds:12,target_surge_level:2},idempotency_key:$ik}')")
expect_json "med-dispatch-activate-surge-beds" "$SURGE_ACT" '
  .status == "APPLIED"
  and (.node.metrics.surge_level == 2)
  and (.node.metrics.total_free_beds >= 50)
'

gate_pass step34
