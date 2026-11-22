#!/usr/bin/env bash
# Step 20 门禁：AxSim 范围预检、AxQuery 会话显式作用域、xBlueView 五区预设与发布自检、xBlueGraph 模式筛选
set -euo pipefail
source "$(dirname "$0")/lib.sh"

CREATED_DASH_20_ID=""
CREATED_ISOLATED_DS_ID=""
TMP_CSV=""

cleanup_step20() {
  if [[ -n "$CREATED_DASH_20_ID" && "$CREATED_DASH_20_ID" != "null" ]]; then
    api DELETE "/api/xblueview/dashboards/$CREATED_DASH_20_ID" >/dev/null 2>&1 || true
  fi
  if [[ -n "$CREATED_ISOLATED_DS_ID" && "$CREATED_ISOLATED_DS_ID" != "null" ]]; then
    api DELETE "/api/axhub/datasets/$CREATED_ISOLATED_DS_ID" >/dev/null 2>&1 || true
  fi
  if [[ -n "$TMP_CSV" ]]; then
    rm -f "$TMP_CSV" || true
  fi
}
trap cleanup_step20 EXIT

# 自愈基线节点与 SUPPLIES 因果传导规则（兼容单步独立执行或前序门禁重写 Schema）
FACTORY1_NODE_ID="${FACTORY1_NODE_ID:-$(sql_app default "SELECT id FROM xb_nodes WHERE node_key = 'F-1' LIMIT 1")}"
SUPPLIES_PROP=$(sql_app default "SELECT COALESCE(propagation::text, '{}') FROM xb_edge_types WHERE name = 'SUPPLIES' LIMIT 1" || echo "{}")
if [[ -z "$FACTORY1_NODE_ID" || -z "$SUPPLIES_PROP" || "$SUPPLIES_PROP" == "{}" ]]; then
  bash "$ROOT/scripts/seed-demo.sh" 10 >/dev/null
  bash "$ROOT/scripts/seed/10-axsim.sh" >/dev/null
  # shellcheck disable=SC1091
  [[ -f "$ROOT/.gate/ids.env" ]] && source "$ROOT/.gate/ids.env"
  FACTORY1_NODE_ID="${FACTORY1_NODE_ID:-$(sql_app default "SELECT id FROM xb_nodes WHERE node_key = 'F-1' LIMIT 1")}"
fi

# 1. 领域内核与分析作用域单测
(cd "$ROOT/axpoint-core" && \
  cargo test -p axpoint-domain sim::preflight --locked && \
  cargo test -p axpoint-domain view_zones --locked && \
  cargo test -p axpoint-analytics schema_link --locked)

# 2. C73：AxSim 沙盘范围预检（残缺范围告警 vs 完整闭合范围就绪）
# 2a. 仅保留 Factory 类型、排除下游 Warehouse/Order，触发有损告警与缺失关联类型诊断
PREFLIGHT_WARN=$(api POST /api/axsim/sandboxes/preflight '{
  "subgraph_slug": "main",
  "scope": {
    "root_node_ids": ["'"$FACTORY1_NODE_ID"'"],
    "depth": 2,
    "direction": "BOTH",
    "type_names": ["Factory"],
    "link_types": [],
    "max_nodes": 50000
  }
}')
expect_json "sim-preflight-warn" "$PREFLIGHT_WARN" '
  (.readiness_grade == "DEGRADED" or .readiness_grade == "BLOCKED")
  and (.agg_rule_closure_ratio < 1.0 or (.truncated_edge_types | length) > 0)
  and ((.missing_upstream_types | length) > 0 or (.reason_codes | length) > 0)
'

# 2b. 放开全链路相关节点类型与深度 3，因果覆盖与聚合闭合达到 READY
PREFLIGHT_OK=$(api POST /api/axsim/sandboxes/preflight '{
  "subgraph_slug": "main",
  "scope": {
    "root_node_ids": ["'"$FACTORY1_NODE_ID"'"],
    "depth": 3,
    "direction": "BOTH",
    "type_names": [],
    "link_types": [],
    "max_nodes": 50000
  }
}')
expect_json "sim-preflight-closed" "$PREFLIGHT_OK" '
  .readiness_grade == "READY"
  and .estimated_nodes >= 2
  and .estimated_links >= 1
  and .causal_edge_coverage_ratio > 0
  and .agg_rule_closure_ratio >= 0.8
  and (.missing_upstream_types | length) == 0
'

# 3. C74（上）：AxQuery 显式作用域隔离验证
DS_LIST=$(api GET '/api/axhub/datasets?limit=20')
DS_FIRST=$(printf '%s' "$DS_LIST" | jq -r '.items[0].id')
DS_SECOND=$(printf '%s' "$DS_LIST" | jq -r '.items[1].id // empty')

# 创建一张仅用于隔离对照的独立文件数据集（若当前不足 2 张表）
if [[ -z "$DS_SECOND" ]]; then
  mkdir -p "$ROOT/.gate"
  TMP_CSV="$ROOT/.gate/step20_isolated_secret.csv"
  printf 'secret_sku_code,isolated_vault_qty\nSKU-Z999,8421\n' > "$TMP_CSV"
  UP_ID=$(curl -sS -X POST -H "Authorization: Bearer $AXPOINT_DEV_TOKEN" \
    -F "file=@${TMP_CSV};type=text/csv" "$CORE_BASE_URL/api/axhub/uploads" | jq -r .id)
  DS_SECOND=$(api POST /api/axhub/datasets "{
    \"name\": \"step20_isolated_vault\",
    \"display_name\": \"隔离对照仓\",
    \"source_kind\": \"FILE_TABLE\",
    \"source_ref\": {\"upload_id\": \"$UP_ID\", \"sheet\": 0}
  }" | jq -r .id)
  CREATED_ISOLATED_DS_ID="$DS_SECOND"
  wait_dataset_idle "$DS_SECOND"
fi
DS_SECOND_NAME=$(api GET "/api/axhub/datasets/$DS_SECOND" | jq -r .name)

SCOPED_OUT=$(api POST /api/chat/query "{
  \"query\": \"统计表 ${DS_SECOND_NAME} 中的记录总数\",
  \"mode\": \"DATA\",
  \"scope\": {
    \"dataset_ids\": [\"$DS_FIRST\"],
    \"subgraph_slug\": \"main\"
  }
}")
expect_json "scoped-ask-isolated" "$SCOPED_OUT" "
  (.applied_scope.dataset_ids == [\"$DS_FIRST\"])
  and (((.sql // \"\") | contains(\"$DS_SECOND_NAME\")) | not)
  and ((.rows | length) == 0 or .grounding_mode == \"ABSTAINED\" or .grounding_mode == \"CLARIFICATION\")
"

# 4. C74（中）：xBlueView 五区语义驾驶舱预设生成
SLUG_20="step20-five-zone-$(date +%s)"
DASH_20=$(api POST /api/xblueview/dashboards "{
  \"name\": \"五区语义联调驾驶舱\",
  \"slug\": \"$SLUG_20\",
  \"theme\": \"dark\",
  \"zone_layout\": \"STANDARD_FIVE_ZONE\"
}")
DASH_20_ID=$(printf '%s' "$DASH_20" | jq -r .id)
CREATED_DASH_20_ID="$DASH_20_ID"
DASH_20_DETAIL=$(api GET "/api/xblueview/dashboards/$DASH_20_ID")
expect_json "view-zone-preset" "$DASH_20_DETAIL" '
  ((.dashboard.canvas // .canvas).cols == 12)
  and ((.dashboard.canvas // .canvas).zone_layout == "STANDARD_FIVE_ZONE")
  and (([.widgets[].options.semantic_zone] | unique | sort) == ["CONTROL","DECISION","INSIGHT","KPI","SCENE"])
  and ([.widgets[] | (.position.x >= 0 and (.position.x + .position.w) <= 12)] | all)
'

# 5. C74（下）：xBlueView 发布前自检阻断失效组件，修复后放行发布
BROKEN_W=$(api POST "/api/xblueview/dashboards/$DASH_20_ID/widgets" '{
  "widget_kind": "ACTION_BUTTON",
  "title": "下发调拨",
  "position": {"x": 8, "y": 19, "w": 4, "h": 2},
  "data_source": {
    "kind": "ACTION",
    "action_type": "RUN_FLOW",
    "flow_id": null
  },
  "options": {"semantic_zone": "DECISION"}
}' | jq -r .id)

PRE_BLOCKED=$(api POST "/api/xblueview/dashboards/$DASH_20_ID/preflight" '{}')
expect_code "view-publish-blocked-http" 422 POST "/api/xblueview/dashboards/$DASH_20_ID/publish" '{}'

# 删除失效动作组件后再次自检并放行发布
api DELETE "/api/xblueview/widgets/$BROKEN_W" >/dev/null
PRE_AFTER=$(api POST "/api/xblueview/dashboards/$DASH_20_ID/preflight" '{}')
expect_json "view-publish-preflight" "$PRE_BLOCKED" '
  .status == "BLOCKED"
  and .blocking_count >= 1
  and ([.issues[].code] | index("UNCONFIGURED_ACTION") != null)
'
expect_json "view-preflight-cleared" "$PRE_AFTER" '.blocking_count == 0 and (.status == "PASS" or .status == "WARN")'
expect_code "view-publish-ok" 200 POST "/api/xblueview/dashboards/$DASH_20_ID/publish" '{}'

# 6. xBlueGraph 三元图模式过滤核验
EDGE_SAMPLE=$(api GET '/api/xblue/graph/subgraph?limit=50')
SAMPLE_LINK_TYPE=$(printf '%s' "$EDGE_SAMPLE" | jq -r '.links[0].link_type // empty')
if [[ -n "$SAMPLE_LINK_TYPE" ]]; then
  PAT_RESP=$(api GET "/api/xblue/graph/subgraph?edge_type=${SAMPLE_LINK_TYPE}&limit=100")
  expect_json "graph-pattern-filter" "$PAT_RESP" "
    ([.links[].link_type] | all(. == \"$SAMPLE_LINK_TYPE\"))
  "
fi

gate_pass step20
