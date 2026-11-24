#!/usr/bin/env bash
# Step 28 Gate：定时/条件事件队列求解、最晚有效干预窗口 (deadline_step) 反推与纯 Rust 多边形围栏/凸包验收
set -euo pipefail
source "$(dirname "$0")/lib.sh"

# 0. 注册 Step 28 测试模型（跨边延迟 lag_steps = 3）
CUR_SCHEMA=$(api GET "/api/xblue/schema?subgraph=main")
NEXT_SCHEMA=$(echo "$CUR_SCHEMA" | jq '
  del(.subgraph_slug, .version)
  | .node_types = ([.node_types[] | select(.name != "Step28Plant" and .name != "Step28Hub")] + [
      {
        "name": "Step28Plant",
        "display_name": "产能基地",
        "properties": [
          {"name": "output", "display_name": "产能", "data_type": "FLOAT64", "kind": "METRIC"}
        ],
        "agg_rules": [],
        "derived_rules": []
      },
      {
        "name": "Step28Hub",
        "display_name": "区域枢纽仓",
        "properties": [
          {"name": "inventory", "display_name": "库存", "data_type": "FLOAT64", "kind": "METRIC"}
        ],
        "agg_rules": [],
        "derived_rules": []
      }
    ])
  | .edge_types = ([.edge_types[] | select(.name != "STEP28_SUPPLIES")] + [
      {
        "name": "STEP28_SUPPLIES",
        "src_type_name": "Step28Plant",
        "dst_type_name": "Step28Hub",
        "impact_weight": 1.0,
        "cardinality": "ONE_TO_MANY",
        "properties": [],
        "propagation": {
          "rules": [
            {
              "from_metric": "output",
              "to_metric": "inventory",
              "coefficient": 1.0,
              "lag_steps": 3
            }
          ]
        }
      }
    ])
')
api POST "/api/xblue/schema?subgraph=main" "$NEXT_SCHEMA" >/dev/null

# 录入上海、苏州、杭州、深圳 4 个枢纽节点与 1 个上游产能基地
P_NODE=$(api POST /api/xblue/nodes '{"type_name":"Step28Plant","node_key":"P-28-1","title":"华东总厂","metrics":{"output":300},"geo":{"lng":121.20,"lat":31.15}}')
SH_NODE=$(api POST /api/xblue/nodes '{"type_name":"Step28Hub","node_key":"H-28-SH","title":"上海仓","metrics":{"inventory":300},"geo":{"lng":121.4737,"lat":31.2304}}')
SZ_NODE=$(api POST /api/xblue/nodes '{"type_name":"Step28Hub","node_key":"H-28-SUZ","title":"苏州仓","metrics":{"inventory":260},"geo":{"lng":120.5853,"lat":31.2989}}')
HZ_NODE=$(api POST /api/xblue/nodes '{"type_name":"Step28Hub","node_key":"H-28-HZ","title":"杭州仓","metrics":{"inventory":240},"geo":{"lng":120.1551,"lat":30.2741}}')
SZN_NODE=$(api POST /api/xblue/nodes '{"type_name":"Step28Hub","node_key":"H-28-SZX","title":"深圳仓","metrics":{"inventory":280},"geo":{"lng":114.0579,"lat":22.5431}}')

P_ID=$(echo "$P_NODE" | jq -r '.id')
SH_ID=$(echo "$SH_NODE" | jq -r '.id')
api POST /api/xblue/links "$(jq -n --arg s "$P_ID" --arg d "$SH_ID" '{link_type:"STEP28_SUPPLIES",src_node_id:$s,dst_node_id:$d,weight:1.0}')" >/dev/null

# 1. C87（上 & 下）：12 步分步沙盘注入第 4 步减产定时事件（lag=3 → 第 7 步传导至上海仓跌破 200）与库存 < 200 自动补库条件事件
SB_28=$(api POST /api/axsim/sandboxes '{"name":"step28-sim-events","scope":{"type_names":["Step28Plant","Step28Hub"]}}' | jq -r '.id')
SCEN_28=$(api POST "/api/axsim/sandboxes/${SB_28}/scenarios" "$(jq -n --arg p "$P_ID" --arg sh "$SH_ID" '{
  name: "Step28 定时减产与库存红线推演",
  actions: [],
  params: { mode: "STEPPED", horizon_steps: 12, damping: 1.0 },
  scheduled_events: [
    {
      id: "ev_cut_step4",
      name: "第4步突发减产",
      trigger_kind: "AT_STEP",
      at_step: 4,
      actions: [{ target_node_id: $p, metric_key: "output", mode: "ADD", value: -150 }]
    },
    {
      id: "ev_restock_under_200",
      name: "低库存应急补货",
      trigger_kind: "METRIC_THRESHOLD",
      watch_node_id: $sh,
      watch_metric: "inventory",
      op: "LT",
      threshold: 200,
      cooldown_steps: 2,
      max_fires: 1,
      actions: [{ target_node_id: $sh, metric_key: "inventory", mode: "ADD", value: 80 }]
    }
  ],
  goal_constraints: [
    {
      id: "goal_sh_inv_200",
      name: "上海仓安全库存红线",
      target_node_id: $sh,
      metric_key: "inventory",
      op: "GTE",
      threshold: 200,
      controllable_node_ids: [$p]
    }
  ]
}')")

expect_json "sim-scheduled-conditional-events" "$SCEN_28" '
  (.fired_events | map(select(.event_id == "ev_cut_step4" and .step == 4)) | length) == 1
  and (.fired_events | map(select(.event_id == "ev_restock_under_200" and .step == 8)) | length) == 1
'

expect_json "sim-deadline-step" "$SCEN_28" '
  .deadline_report.earliest_breach_step == 7
  and .deadline_report.earliest_deadline_step == 3
  and .deadline_report.status == "ACTIONABLE_WINDOW"
'

# 2. C88：长三角多边形围栏精确命中上海/苏州/杭州 3 仓并排除深圳仓，且凸包面积 > 0；非法几何返回 400 INVALID_GEOMETRY
YRD_POLY='{"type":"Polygon","coordinates":[[[119.5,29.8],[122.2,29.8],[122.2,32.0],[119.5,32.0],[119.5,29.8]]]}'
POLY_NODES=$(api GET "/api/xblue/nodes?type_name=Step28Hub&geo_op=GEO_WITHIN_POLYGON&polygon=$(python3 -c 'import urllib.parse,sys; print(urllib.parse.quote(sys.argv[1]))' "$YRD_POLY")")
expect_json "geo-polygon-within" "$POLY_NODES" '
  ([.items[].node_key] | sort) == ["H-28-HZ", "H-28-SH", "H-28-SUZ"]
'

HULL_RESP=$(api POST /api/xblue/geo/convex-hull '{"type_name":"Step28Hub"}')
expect_json "geo-convex-hull" "$HULL_RESP" '
  .hull.type == "Polygon"
  and .point_count >= 4
  and .area_sq_deg > 0
'

expect_code "invalid-geometry-rejected" 400 GET "/api/xblue/nodes?type_name=Step28Hub&geo_op=GEO_WITHIN_POLYGON&polygon=%7B%22type%22%3A%22Polygon%22%2C%22coordinates%22%3A%5B%5B%5B120%2C30%5D%2C%5B121%2C31%5D%5D%5D%7D"

gate_pass step28
