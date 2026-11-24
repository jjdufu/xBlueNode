#!/usr/bin/env bash
# Step 27 Gate：AxQL 免连接跨层编译、SUM_BY_ENTITY 防一对多膨胀、状态驱动动态行级视野与沙盘同源继承验收
set -euo pipefail
source "$(dirname "$0")/lib.sh"

# 1. C85（上）：跨层 OBJECT(...) 免手动 JOIN 查询与无幻觉符号拦截
CUR_SCHEMA=$(api GET "/api/xblue/schema?subgraph=main")
NEXT_SCHEMA=$(echo "$CUR_SCHEMA" | jq '
  del(.subgraph_slug, .version)
  | .node_types = ([.node_types[] | select(.name != "Step27Warehouse" and .name != "Step27Order")] + [
      {
        "name": "Step27Warehouse",
        "display_name": "测试仓库",
        "semantic_layer_tier": "STATIC_FACT",
        "properties": [
          {"name": "region", "display_name": "大区", "data_type": "STRING", "kind": "ATTRIBUTE"},
          {"name": "inventory", "display_name": "库存", "data_type": "FLOAT64", "kind": "METRIC"}
        ],
        "agg_rules": [],
        "derived_rules": []
      },
      {
        "name": "Step27Order",
        "display_name": "测试订单",
        "semantic_layer_tier": "EVENT_DYNAMIC",
        "properties": [
          {"name": "amount", "display_name": "金额", "data_type": "FLOAT64", "kind": "METRIC"}
        ],
        "agg_rules": [],
        "derived_rules": []
      }
    ])
  | .edge_types = ([.edge_types[] | select(.name != "STEP27_FULFILLS")] + [
      {
        "name": "STEP27_FULFILLS",
        "src_type_name": "Step27Warehouse",
        "dst_type_name": "Step27Order",
        "impact_weight": 0.8,
        "cardinality": "ONE_TO_MANY",
        "properties": []
      }
    ])
')
api POST "/api/xblue/schema?subgraph=main" "$NEXT_SCHEMA" >/dev/null

W_NODE=$(api POST /api/xblue/nodes '{"type_name":"Step27Warehouse","node_key":"W-27-1","title":"华东一号仓","properties":{"region":"east"},"metrics":{"inventory":500}}')
O1_NODE=$(api POST /api/xblue/nodes '{"type_name":"Step27Order","node_key":"O-27-1","title":"订单一","metrics":{"amount":120}}')
O2_NODE=$(api POST /api/xblue/nodes '{"type_name":"Step27Order","node_key":"O-27-2","title":"订单二","metrics":{"amount":180}}')
W_ID=$(echo "$W_NODE" | jq -r '.id')
O1_ID=$(echo "$O1_NODE" | jq -r '.id')
O2_ID=$(echo "$O2_NODE" | jq -r '.id')
api POST /api/xblue/links "$(jq -n --arg s "$W_ID" --arg d "$O1_ID" '{link_type:"STEP27_FULFILLS",src_node_id:$s,dst_node_id:$d}')" >/dev/null
api POST /api/xblue/links "$(jq -n --arg s "$W_ID" --arg d "$O2_ID" '{link_type:"STEP27_FULFILLS",src_node_id:$s,dst_node_id:$d}')" >/dev/null

# 校验引用不存在符号返回 400 AXQL_UNMAPPED_SYMBOL
expect_code "axql-unmapped-symbol" 400 POST /api/analytics/axql/compile '{"axql":"SELECT w.non_existent_prop FROM OBJECT(Step27Warehouse) w"}'

AXQL_PLAN=$(api POST /api/analytics/axql/compile '{
  "axql": "SELECT w.node_key, w.region, w.inventory FROM OBJECT(Step27Warehouse) w WHERE w.node_key = '\''W-27-1'\''"
}')
AXQL_OBJ=$(api POST /api/analytics/axql/run '{
  "axql": "SELECT w.node_key, w.region, w.inventory FROM OBJECT(Step27Warehouse) w WHERE w.node_key = '\''W-27-1'\''"
}')
expect_json "axql-cross-tier-compile" "$(jq -n --argjson p "$AXQL_PLAN" --argjson r "$AXQL_OBJ" '{p:$p, r:$r}')" '
  (.p.compiled_sql | length > 0)
  and (.r.row_count == 1)
  and ((.r.rows[0].inventory | tonumber) == 500)
'

# 2. C85（下）：1 个仓库 (inventory=500) 关联 2 笔订单，SUM_BY_ENTITY 精确返回 500（消除 2 倍扇出膨胀）
AXQL_DEDUP=$(api POST /api/analytics/axql/run '{
  "axql": "SELECT SUM_BY_ENTITY(w.inventory BY w) AS exact_inv, COUNT(o.node_key) AS order_cnt FROM MATCH (w:Step27Warehouse)-[:STEP27_FULFILLS]->(o:Step27Order) WHERE w.node_key = '\''W-27-1'\''"
}')
expect_json "sum-by-entity-exact" "$AXQL_DEDUP" '
  (.rows[0].exact_inv | tonumber) == 500
  and (.rows[0].order_cnt | tonumber) == 2
'

# 3. C86：状态驱动动态行级视野（inventory < 100 时对跨事业部观察员自动可见）及 AxSim 沙盘同源继承
OBS_USERNAME="obs27_$(date +%s)"
OBS_PASS="Step27Pass!9876"
api PUT /api/axguard/departments/risk_obs '{"name":"风控观察组"}' >/dev/null
OBS_USER=$(api POST /api/axguard/users "$(jq -n --arg u "$OBS_USERNAME" --arg p "$OBS_PASS" '{
  username: $u, display_name: "跨部风控员", password: $p, dept_id: "risk_obs", clearance: "INTERNAL", roles: ["ANALYST"]
}')")
OBS_UID=$(echo "$OBS_USER" | jq -r '.id')
OBS_LOGIN=$(curl -fsS -X POST "${CORE_BASE_URL}/api/auth/login" -H 'Content-Type: application/json' \
  -d "$(jq -n --arg u "$OBS_USERNAME" --arg p "$OBS_PASS" '{username:$u, password:$p}')")
OBS_TOKEN=$(echo "$OBS_LOGIN" | jq -r '.access_token')

api PUT /api/axguard/row-policy-defaults '{"target_kind":"NODE_TYPE","target_ref":"Step27Warehouse","default_deny":true}' >/dev/null
RP_CREATED=$(api POST /api/axguard/row-policies '{
  "name": "step27_low_inv_escalate",
  "target_kind": "NODE_TYPE",
  "target_ref": "Step27Warehouse",
  "applies_to": {"depts": ["risk_obs"]},
  "predicate": "properties.region == {{user.dept}} OR metrics.inventory < 100",
  "policy_mode": "STATE_DRIVEN",
  "enabled": true
}')
RP_ID=$(echo "$RP_CREATED" | jq -r '.id')

# 初始 inventory=500：跨部门观察员不可见该节点，克隆沙盘亦包含 0 个该类型节点
BEFORE_NODES=$(curl -fsS -H "Authorization: Bearer ${OBS_TOKEN}" "${CORE_BASE_URL}/api/xblue/nodes?type_name=Step27Warehouse")
SB_BEFORE=$(curl -fsS -X POST -H "Authorization: Bearer ${OBS_TOKEN}" -H 'Content-Type: application/json' \
  "${CORE_BASE_URL}/api/axsim/sandboxes" -d '{"name":"step27-sb-before","scope":{"type_names":["Step27Warehouse"]}}')
SB_BEFORE_ID=$(echo "$SB_BEFORE" | jq -r '.id')
SB_BEFORE_GRAPH=$(curl -fsS -H "Authorization: Bearer ${OBS_TOKEN}" "${CORE_BASE_URL}/api/axsim/sandboxes/${SB_BEFORE_ID}/graph")

# 下调实时指标 inventory=60 (<100)：无需手工改权限，观察员立即查见该节点且新建沙盘可见
api PATCH "/api/xblue/nodes/${W_ID}" '{"metrics":{"inventory":60},"metrics_merge":"MERGE"}' >/dev/null
AFTER_NODES=$(curl -fsS -H "Authorization: Bearer ${OBS_TOKEN}" "${CORE_BASE_URL}/api/xblue/nodes?type_name=Step27Warehouse")
expect_json "state-visibility-escalate" "$(jq -n --argjson b "$BEFORE_NODES" --argjson a "$AFTER_NODES" '{b:$b, a:$a}')" '
  ((.b.items | map(select(.node_key == "W-27-1")) | length) == 0)
  and ((.a.items | map(select(.node_key == "W-27-1")) | length) == 1)
'

SB_AFTER=$(curl -fsS -X POST -H "Authorization: Bearer ${OBS_TOKEN}" -H 'Content-Type: application/json' \
  "${CORE_BASE_URL}/api/axsim/sandboxes" -d '{"name":"step27-sb-after","scope":{"type_names":["Step27Warehouse"]}}')
SB_AFTER_ID=$(echo "$SB_AFTER" | jq -r '.id')
SB_AFTER_GRAPH=$(curl -fsS -H "Authorization: Bearer ${OBS_TOKEN}" "${CORE_BASE_URL}/api/axsim/sandboxes/${SB_AFTER_ID}/graph")
expect_json "sim-inherits-state-visibility" "$(jq -n --argjson gb "$SB_BEFORE_GRAPH" --argjson ga "$SB_AFTER_GRAPH" '{gb:$gb, ga:$ga}')" '
  ((.gb.nodes | map(select(.node_key == "W-27-1")) | length) == 0)
  and ((.ga.nodes | map(select(.node_key == "W-27-1")) | length) == 1)
'

# 清理测试策略以免影响后续默认查询
api DELETE "/api/axguard/row-policies/${RP_ID}" >/dev/null || true
api PUT /api/axguard/row-policy-defaults '{"target_kind":"NODE_TYPE","target_ref":"Step27Warehouse","default_deny":false}' >/dev/null || true

gate_pass step27
