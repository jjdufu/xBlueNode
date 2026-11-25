#!/usr/bin/env bash
# 幂等向量化装载：城市急诊与医疗床位调度（med_dispatch，264 节点 / 508 边 / 18,000 行分诊湖表 / 20,000 时序点）
set -euo pipefail
source "$(dirname "$0")/../../gate/lib.sh"

PACK_DIR="$ROOT/templates/packs/med_dispatch"
PY_BIN="${ROOT}/axpoint-parser/.venv/bin/python"
[[ -x "$PY_BIN" ]] || PY_BIN="python3"

# 1. 创建或更新生产业务子图 med_dispatch 并发布三域三层 Schema
SUBGRAPHS_JSON=$(api GET /api/xblue/subgraphs)
if ! printf '%s' "$SUBGRAPHS_JSON" | jq -e '.[] | select(.slug == "med_dispatch")' >/dev/null 2>&1; then
  api POST /api/xblue/subgraphs '{
    "slug": "med_dispatch",
    "display_name": "城市急诊床位调度",
    "description": "院区统筹、临床救治与120院前急救空间轨迹凸包调度模型",
    "owner_dept": "default",
    "is_dev_sandbox": false,
    "source_template_id": "med_dispatch"
  }' >/dev/null
fi

MED_SCHEMA_RAW=$(cat "$PACK_DIR/schema.json")
api POST "/api/xblue/schema?subgraph=med_dispatch" "$(printf '%s' "$MED_SCHEMA_RAW" | jq '. + {force: true, change_summary: "seed med_dispatch v4 showcase"}')" >/dev/null
api PUT /api/xblue/subgraphs/med_dispatch/competency-suite "$(cat "$PACK_DIR/competency.json")" >/dev/null

# 同步合并对象与关系类型至企业主图以支撑急诊看板预检、应急加床回写与推演沙盘运行
CUR_MAIN=$(api GET "/api/xblue/schema?subgraph=main")
MED_NAMES='["AmbulanceUnit","HospitalCampus","WardCapacityCube","ClinicalDept","LifeSupportUnit","TriageVisit"]'
MED_EDGES='["TRANSPORTS_TO_CAMPUS","HAS_CLINICAL_DEPT","OPERATES_DEVICE","RECORDS_TRIAGE","SUMMARIZES_WARD"]'
MERGED_MAIN=$(jq -n \
  --argjson cur "$CUR_MAIN" \
  --argjson med "$MED_SCHEMA_RAW" \
  --argjson nnames "$MED_NAMES" \
  --argjson enames "$MED_EDGES" '
  {
    node_types: (([ $cur.node_types[] | select((.name as $n | $nnames | index($n)) == null) ]) + $med.node_types),
    edge_types: (([ $cur.edge_types[] | select((.name as $e | $enames | index($e)) == null) ]) + $med.edge_types),
    mappings: ($cur.mappings // []),
    force: true,
    change_summary: "merge med_dispatch types"
  }
')
api POST "/api/xblue/schema?subgraph=main" "$MERGED_MAIN" >/dev/null

# 2. 向量化写入 18,000 行 DuckLake 分诊表、264 节点（含 64 辆带坐标急救车）、508 边与 20,000 时序点
SEED_OUT=$(PYTHONPATH="$ROOT/axpoint-parser" "$PY_BIN" "$ROOT/axpoint-parser/scripts/gen_worlds_semi_med.py" --world med_dispatch --tenant default)
read -r HOSP01_ID DEPT01_ID AMB01_ID < <(
  printf '%s' "$SEED_OUT" | jq -r '[.main_anchors.hosp01_id, .main_anchors.dept01_id, .main_anchors.amb01_id] | @tsv'
)

# 3. 预置 AxSim 冬季急诊重症高峰床位挤兑沙盘与推演场景
SB_NAME="冬季急诊重症高峰床位挤兑沙盘"
MED_SB_ID=$(api GET /api/axsim/sandboxes | jq -r --arg n "$SB_NAME" '.[] | select(.name == $n) | .id' | head -n 1)
if [[ -z "$MED_SB_ID" || "$MED_SB_ID" == "null" ]]; then
  MED_SB_ID=$(api POST /api/axsim/sandboxes "$(jq -n --arg n "$SB_NAME" '{
    name: $n,
    scope: {
      type_names: ["AmbulanceUnit", "HospitalCampus", "ClinicalDept", "LifeSupportUnit"],
      link_types: ["TRANSPORTS_TO_CAMPUS", "HAS_CLINICAL_DEPT", "OPERATES_DEVICE"]
    }
  }')" | jq -r '.id')
else
  api POST "/api/axsim/sandboxes/${MED_SB_ID}/refresh-base" '{}' >/dev/null || true
fi

SCEN_NAME="寒潮重症呼衰转运激增与机动加床推演"
EXISTING_SCEN=$(api GET "/api/axsim/sandboxes/${MED_SB_ID}/scenarios" | jq -r --arg s "$SCEN_NAME" 'if type == "array" then (.[] | select(.name == $s) | .id) else (.items[]? | select(.name == $s) | .id) end' | head -n 1)
if [[ -z "$EXISTING_SCEN" || "$EXISTING_SCEN" == "null" ]]; then
  api POST "/api/axsim/sandboxes/${MED_SB_ID}/scenarios" "$(jq -n \
    --arg s "$SCEN_NAME" \
    --arg amb "$AMB01_ID" \
    --arg hosp "$HOSP01_ID" '{
      name: $s,
      actions: [],
      params: { mode: "STEPPED", horizon_steps: 10, damping: 0.9 },
      scheduled_events: [
        {
          id: "ev_winter_surge_step2",
          name: "第2步寒潮夜间120重症转运量激增",
          trigger_kind: "AT_STEP",
          at_step: 2,
          actions: [{ target_node_id: $amb, metric_key: "dispatch_load", mode: "ADD", value: 14 }]
        },
        {
          id: "ev_auto_surge_beds",
          name: "空床跌破20张自动启用机动病区加床",
          trigger_kind: "METRIC_THRESHOLD",
          watch_node_id: $hosp,
          watch_metric: "total_free_beds",
          op: "LT",
          threshold: 20,
          cooldown_steps: 2,
          max_fires: 1,
          actions: [{ target_node_id: $hosp, metric_key: "total_free_beds", mode: "ADD", value: 24 }]
        }
      ],
      goal_constraints: [
        {
          id: "goal_hosp_free_beds_floor",
          name: "总院区急诊机动空床安全红线",
          target_node_id: $hosp,
          metric_key: "total_free_beds",
          op: "GTE",
          threshold: 20,
          controllable_node_ids: [$amb, $hosp]
        }
      ]
    }')" >/dev/null
fi

# 4. 发布五区急诊态势看板 med-dispatch-cockpit（含 120 急救车凸包地图与一键启用应急加床按钮）
EXISTING_DASH=$(api GET /api/xblueview/dashboards/by-slug/med-dispatch-cockpit 2>/dev/null || true)
MED_DASH_ID=$(printf '%s' "$EXISTING_DASH" | jq -r 'if type == "object" then (.id // empty) else empty end' 2>/dev/null || true)
if [[ -z "$MED_DASH_ID" || "$MED_DASH_ID" == "null" ]]; then
  COCKPIT_DEF=$(cat "$PACK_DIR/dashboards/cockpit.json")
  MED_DASH_ID=$(api POST /api/xblueview/dashboards "$(printf '%s' "$COCKPIT_DEF" | jq '{
    name: .name,
    slug: .slug,
    description: .description,
    theme: .theme,
    canvas: .canvas,
    zone_layout: "STANDARD_FIVE_ZONE"
  }')" | jq -r '.id')

  CUR_DETAIL=$(api GET "/api/xblueview/dashboards/${MED_DASH_ID}")
  for WID in $(printf '%s' "$CUR_DETAIL" | jq -r '.widgets[]?.id'); do
    api DELETE "/api/xblueview/widgets/${WID}" >/dev/null
  done

  WIDGETS_ARRAY=$(printf '%s' "$COCKPIT_DEF" | jq --arg hosp_id "$HOSP01_ID" '
    .widgets | .[9] = {
      widget_kind: "ACTION_BUTTON",
      title: "启用应急加床",
      position: { x: 8, y: 14, w: 4, h: 5 },
      data_source: {
        kind: "ACTION",
        action_type: "WRITE_NODE",
        node_id: $hosp_id,
        target_kind: "METRIC",
        key: "total_free_beds",
        input_schema: {
          type: "object",
          required: ["value"],
          properties: {
            value: { type: "number", title: "扩容后总空床数", default: 62 }
          }
        }
      },
      encoding: {},
      options: {
        semantic_zone: "DECISION",
        button_label: "启用应急加床",
        skip_confirm: true
      },
      drill: [],
      refresh_mode: "LIVE",
      refresh_seconds: 30
    }
  ')
  COUNT_W=$(printf '%s' "$WIDGETS_ARRAY" | jq 'length')
  for (( idx = 0; idx < COUNT_W; idx++ )); do
    W_BODY=$(printf '%s' "$WIDGETS_ARRAY" | jq ".[$idx]")
    api POST "/api/xblueview/dashboards/${MED_DASH_ID}/widgets" "$W_BODY" >/dev/null
  done
  api POST "/api/xblueview/dashboards/${MED_DASH_ID}/publish" '{}' >/dev/null
elif [[ "$(printf '%s' "$EXISTING_DASH" | jq -r '.status // "DRAFT"')" != "PUBLISHED" ]]; then
  api POST "/api/xblueview/dashboards/${MED_DASH_ID}/publish" '{}' >/dev/null
fi

# 留出 LISTEN/NOTIFY 窗口以便 DatasetSyncedSubscriber 刷新 DuckDB 会话池缓存
sleep 0.05

# 5. 执行子图运行态 L4 认证落库
CERT_MED=$(api POST /api/xblue/subgraphs/med_dispatch/certify '{}')
CERT_LEVEL=$(printf '%s' "$CERT_MED" | jq -r '.cert_level')
if [[ "$CERT_LEVEL" != "L4_CERTIFIED" ]]; then
  echo "med_dispatch certification did not reach L4_CERTIFIED: $CERT_MED" >&2
  exit 1
fi

echo "MED_DISPATCH_SANDBOX_ID=${MED_SB_ID}"
echo "MED_DISPATCH_DASHBOARD_ID=${MED_DASH_ID}"
echo "MED_DISPATCH_DEPT01_ID=${DEPT01_ID}"
