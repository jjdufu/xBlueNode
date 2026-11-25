#!/usr/bin/env bash
# 幂等向量化装载：半导体全球多级供应链（semi_supply，526 节点 / 1,350 边 / 30,000 行传感湖表 / 16,800 时序点）
set -euo pipefail
source "$(dirname "$0")/../../gate/lib.sh"

PACK_DIR="$ROOT/templates/packs/semi_supply"
PY_BIN="${ROOT}/axpoint-parser/.venv/bin/python"
[[ -x "$PY_BIN" ]] || PY_BIN="python3"

# 1. 创建或更新生产业务子图 semi_supply 并发布四域三层 Schema
SUBGRAPHS_JSON=$(api GET /api/xblue/subgraphs)
if ! printf '%s' "$SUBGRAPHS_JSON" | jq -e '.[] | select(.slug == "semi_supply")' >/dev/null 2>&1; then
  api POST /api/xblue/subgraphs '{
    "slug": "semi_supply",
    "display_name": "半导体全球供应链",
    "description": "采购、制造、物流、履约四域泳道与晶圆批次/良率三层折叠模型",
    "owner_dept": "default",
    "is_dev_sandbox": false,
    "source_template_id": "semi_supply"
  }' >/dev/null
fi

SEMI_SCHEMA_RAW=$(cat "$PACK_DIR/schema.json")
api POST "/api/xblue/schema?subgraph=semi_supply" "$(printf '%s' "$SEMI_SCHEMA_RAW" | jq '. + {force: true, change_summary: "seed semi_supply v4 showcase"}')" >/dev/null
api PUT /api/xblue/subgraphs/semi_supply/competency-suite "$(cat "$PACK_DIR/competency.json")" >/dev/null

# 同步合并对象与关系类型至企业主图以支撑数字看板预检、动作契约与推演沙盘运行
CUR_MAIN=$(api GET "/api/xblue/schema?subgraph=main")
SEMI_NAMES='["SpecialtyVendor","WaferFab","LithoTool","WaferLotEvent","YieldRollupCube","CustomsPort","RegionalHub","AutoOemOrder"]'
SEMI_EDGES='["SUPPLIES_MATERIAL","EQUIPPED_WITH","RUNS_LOT","SUMMARIZES_YIELD","SHIPS_VIA_PORT","DISPATCHES_TO_HUB","DELIVERS_ORDER"]'
MERGED_MAIN=$(jq -n \
  --argjson cur "$CUR_MAIN" \
  --argjson semi "$SEMI_SCHEMA_RAW" \
  --argjson nnames "$SEMI_NAMES" \
  --argjson enames "$SEMI_EDGES" '
  {
    node_types: (([ $cur.node_types[] | select((.name as $n | $nnames | index($n)) == null) ]) + $semi.node_types),
    edge_types: (([ $cur.edge_types[] | select((.name as $e | $enames | index($e)) == null) ]) + $semi.edge_types),
    mappings: ($cur.mappings // []),
    force: true,
    change_summary: "merge semi_supply types"
  }
')
api POST "/api/xblue/schema?subgraph=main" "$MERGED_MAIN" >/dev/null

# 2. 向量化写入 30,000 行 DuckLake 湖表、526 节点、1,350 边与 16,800 时序点
SEED_OUT=$(PYTHONPATH="$ROOT/axpoint-parser" "$PY_BIN" "$ROOT/axpoint-parser/scripts/gen_worlds_semi_med.py" --world semi_supply --tenant default)
read -r FAB01_ID PORT01_ID HUB01_ID OEM01_ID < <(
  printf '%s' "$SEED_OUT" | jq -r '[.main_anchors.fab01_id, .main_anchors.port01_id, .main_anchors.hub01_id, .main_anchors.oem01_id] | @tsv'
)

# 3. 预置 AxSim 晶圆断供与清关延误沙盘及 STEPPED 红线推演场景
SB_NAME="半导体晶圆断供与清关延误沙盘"
SEMI_SB_ID=$(api GET /api/axsim/sandboxes | jq -r --arg n "$SB_NAME" '.[] | select(.name == $n) | .id' | head -n 1)
if [[ -z "$SEMI_SB_ID" || "$SEMI_SB_ID" == "null" ]]; then
  SEMI_SB_ID=$(api POST /api/axsim/sandboxes "$(jq -n --arg n "$SB_NAME" '{
    name: $n,
    scope: {
      type_names: ["WaferFab", "LithoTool", "CustomsPort", "RegionalHub", "AutoOemOrder"],
      link_types: ["EQUIPPED_WITH", "SHIPS_VIA_PORT", "DISPATCHES_TO_HUB", "DELIVERS_ORDER"]
    }
  }')" | jq -r '.id')
else
  api POST "/api/axsim/sandboxes/${SEMI_SB_ID}/refresh-base" '{}' >/dev/null || true
fi

SCEN_NAME="口岸清关拥堵与车规芯片库存红线推演"
EXISTING_SCEN=$(api GET "/api/axsim/sandboxes/${SEMI_SB_ID}/scenarios" | jq -r --arg s "$SCEN_NAME" 'if type == "array" then (.[] | select(.name == $s) | .id) else (.items[]? | select(.name == $s) | .id) end' | head -n 1)
if [[ -z "$EXISTING_SCEN" || "$EXISTING_SCEN" == "null" ]]; then
  api POST "/api/axsim/sandboxes/${SEMI_SB_ID}/scenarios" "$(jq -n \
    --arg s "$SCEN_NAME" \
    --arg port "$PORT01_ID" \
    --arg hub "$HUB01_ID" \
    --arg fab "$FAB01_ID" '{
      name: $s,
      actions: [],
      params: { mode: "STEPPED", horizon_steps: 10, damping: 0.9 },
      scheduled_events: [
        {
          id: "ev_port_delay_step2",
          name: "第2步跨国口岸清关查验升级",
          trigger_kind: "AT_STEP",
          at_step: 2,
          actions: [{ target_node_id: $port, metric_key: "clearance_hours", mode: "ADD", value: 36 }]
        },
        {
          id: "ev_air_charter_relief",
          name: "分拨仓低库存触发包机补库",
          trigger_kind: "METRIC_THRESHOLD",
          watch_node_id: $hub,
          watch_metric: "chip_inventory_k",
          op: "LT",
          threshold: 260,
          cooldown_steps: 2,
          max_fires: 1,
          actions: [{ target_node_id: $hub, metric_key: "chip_inventory_k", mode: "ADD", value: 65 }]
        }
      ],
      goal_constraints: [
        {
          id: "goal_hub_chip_floor",
          name: "华东分拨仓车规芯片安全库存底线",
          target_node_id: $hub,
          metric_key: "chip_inventory_k",
          op: "GTE",
          threshold: 250,
          controllable_node_ids: [$port, $fab]
        }
      ]
    }')" >/dev/null
fi

# 4. 发布五区供应链数字看板 semi-supply-cockpit（含一键启用包机快通动作按钮）
EXISTING_DASH=$(api GET /api/xblueview/dashboards/by-slug/semi-supply-cockpit 2>/dev/null || true)
SEMI_DASH_ID=$(printf '%s' "$EXISTING_DASH" | jq -r 'if type == "object" then (.id // empty) else empty end' 2>/dev/null || true)
if [[ -z "$SEMI_DASH_ID" || "$SEMI_DASH_ID" == "null" ]]; then
  COCKPIT_DEF=$(cat "$PACK_DIR/dashboards/cockpit.json")
  SEMI_DASH_ID=$(api POST /api/xblueview/dashboards "$(printf '%s' "$COCKPIT_DEF" | jq '{
    name: .name,
    slug: .slug,
    description: .description,
    theme: .theme,
    canvas: .canvas,
    zone_layout: "STANDARD_FIVE_ZONE"
  }')" | jq -r '.id')

  # 替换默认生成的占位组件为精确业务绑定组件（含第10个启用包机快通动作按钮）
  CUR_DETAIL=$(api GET "/api/xblueview/dashboards/${SEMI_DASH_ID}")
  for WID in $(printf '%s' "$CUR_DETAIL" | jq -r '.widgets[]?.id'); do
    api DELETE "/api/xblueview/widgets/${WID}" >/dev/null
  done

  WIDGETS_ARRAY=$(printf '%s' "$COCKPIT_DEF" | jq --arg port_id "$PORT01_ID" '
    .widgets | .[9] = {
      widget_kind: "ACTION_BUTTON",
      title: "启用恒温包机快通",
      position: { x: 8, y: 14, w: 4, h: 5 },
      data_source: {
        kind: "ACTION",
        action_type: "WRITE_NODE",
        node_id: $port_id,
        target_kind: "METRIC",
        key: "clearance_hours",
        input_schema: {
          type: "object",
          required: ["value"],
          properties: {
            value: { type: "number", title: "目标清关耗时", default: 12 }
          }
        }
      },
      encoding: {},
      options: {
        semantic_zone: "DECISION",
        button_label: "启用包机快通",
        skip_confirm: true
      },
      drill: [],
      refresh_mode: "LIVE",
      refresh_seconds: 60
    }
  ')
  COUNT_W=$(printf '%s' "$WIDGETS_ARRAY" | jq 'length')
  for (( idx = 0; idx < COUNT_W; idx++ )); do
    W_BODY=$(printf '%s' "$WIDGETS_ARRAY" | jq ".[$idx]")
    api POST "/api/xblueview/dashboards/${SEMI_DASH_ID}/widgets" "$W_BODY" >/dev/null
  done
  api POST "/api/xblueview/dashboards/${SEMI_DASH_ID}/publish" '{}' >/dev/null
elif [[ "$(printf '%s' "$EXISTING_DASH" | jq -r '.status // "DRAFT"')" != "PUBLISHED" ]]; then
  api POST "/api/xblueview/dashboards/${SEMI_DASH_ID}/publish" '{}' >/dev/null
fi

# 留出 LISTEN/NOTIFY 窗口以便 DatasetSyncedSubscriber 刷新 DuckDB 会话池缓存
sleep 0.05

# 5. 执行子图运行态 L4 认证落库
CERT_SEMI=$(api POST /api/xblue/subgraphs/semi_supply/certify '{}')
CERT_LEVEL=$(printf '%s' "$CERT_SEMI" | jq -r '.cert_level')
if [[ "$CERT_LEVEL" != "L4_CERTIFIED" ]]; then
  echo "semi_supply certification did not reach L4_CERTIFIED: $CERT_SEMI" >&2
  exit 1
fi

echo "SEMI_SUPPLY_SANDBOX_ID=${SEMI_SB_ID}"
echo "SEMI_SUPPLY_DASHBOARD_ID=${SEMI_DASH_ID}"
echo "SEMI_SUPPLY_OEM01_ID=${OEM01_ID}"
