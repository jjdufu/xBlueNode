#!/usr/bin/env bash
# Step 33 Showcase Seed (1/2): `power_grid` 新型电力与配网调度可执行世界幂等秒级装载 (< 3s)
set -euo pipefail
source "$(dirname "$0")/../../gate/lib.sh"

mkdir -p "$ROOT/.gate"
PACK_DIR="$ROOT/templates/packs/power_grid"

upsert_ids_env() {
  local key="$1" val="$2" env_file="$ROOT/.gate/ids.env"
  touch "$env_file"
  if grep -q "^${key}=" "$env_file" 2>/dev/null; then
    local tmp="${env_file}.tmp.$$"
    grep -v "^${key}=" "$env_file" > "$tmp" || true
    printf '%s=%s\n' "$key" "$val" >> "$tmp"
    mv "$tmp" "$env_file"
  else
    printf '%s=%s\n' "$key" "$val" >> "$env_file"
  fi
}

PY_BIN="$ROOT/axpoint-parser/.venv/bin/python"
if [[ ! -x "$PY_BIN" ]]; then
  PY_BIN="python3"
fi

SEED_TMP=$(mktemp -d)
trap 'rm -rf "$SEED_TMP"' EXIT

# 1. 创建或更新 power_grid 子图并发布四域三层 Schema
SUB_LIST=$(api GET /api/xblue/subgraphs)
if ! printf '%s' "$SUB_LIST" | jq -e '(if type == "array" then . else (.items // []) end) | map(select(.slug == "power_grid")) | length > 0' >/dev/null 2>&1; then
  api POST /api/xblue/subgraphs '{
    "slug": "power_grid",
    "display_name": "新型电力与配网调度",
    "description": "电源—输变电—配电—重点负荷四域三层可执行调度模型（多边形围栏与馈线四路径过载传导）",
    "is_dev_sandbox": false
  }' >/dev/null
fi

SCHEMA_PAYLOAD=$(jq -c '. + {"force": true, "change_summary": "publish power_grid world schema"}' "$PACK_DIR/schema.json")
api POST "/api/xblue/schema?subgraph=power_grid" "$SCHEMA_PAYLOAD" >/dev/null

# 2. 并行执行：向量化节点/边/时序/湖表灌装 + 黄金验收题 + 告警规则 + PROD 动作流 + 沙盘元数据准备
"$PY_BIN" "$ROOT/axpoint-parser/scripts/gen_worlds_grid_fin.py" --world power_grid --tenant default > "$SEED_TMP/gen.json" &
GEN_PID=$!

(
  api PUT /api/xblue/subgraphs/power_grid/competency-suite "$(cat "$PACK_DIR/competency.json")" >/dev/null &
  EXISTING_RULES=$(api GET "/api/axflow/rules?limit=200")
  while IFS= read -r rule_json; do
    r_name=$(printf '%s' "$rule_json" | jq -r '.name')
    if ! printf '%s' "$EXISTING_RULES" | jq -e --arg nm "$r_name" '(if type == "array" then . else (.items // []) end) | map(select(.name == $nm)) | length > 0' >/dev/null 2>&1; then
      api POST /api/axflow/rules "$rule_json" >/dev/null &
    fi
  done < <(jq -c '.rules[]' "$PACK_DIR/rules.json")

  PG_FLOW_ID=$(api GET "/api/axflow/flows?limit=200" | jq -r '(if type == "array" then . else (.items // []) end)[] | select(.slug == "power-grid-ess-dispatch") | .id' | head -n 1 || true)
  if [[ -z "$PG_FLOW_ID" || "$PG_FLOW_ID" == "null" ]]; then
    FLOW_BODY=$(jq -c 'del(.key)' "$PACK_DIR/flows/ess_dispatch.json")
    PG_FLOW_ID=$(api POST /api/axflow/flows "$FLOW_BODY" | jq -r '.id')
    api POST "/api/axflow/flows/$PG_FLOW_ID/versions" '{"note":"power_grid ess dispatch v1"}' >/dev/null
    api POST "/api/axflow/flows/$PG_FLOW_ID/deploy" '{"version":1,"environment":"PROD"}' >/dev/null
  fi
  printf '%s' "$PG_FLOW_ID" > "$SEED_TMP/flow_id"

  SB_ID=$(api GET /api/axsim/sandboxes | jq -r '(if type == "array" then . else (.items // []) end)[] | select(.name == "迎峰度夏配网调度推演沙盘") | .id' | head -n 1 || true)
  if [[ -z "$SB_ID" || "$SB_ID" == "null" ]]; then
    SB_ID=$(api POST /api/axsim/sandboxes '{
      "name": "迎峰度夏配网调度推演沙盘",
      "scope": {
        "type_names": ["PowerPlant", "Substation", "DistTransformer", "CriticalLoad"]
      }
    }' | jq -r '.id')
  fi
  printf '%s' "$SB_ID" > "$SEED_TMP/sb_id"
  wait
) &
SETUP_PID=$!

wait "$GEN_PID"
wait "$SETUP_PID"

read -r PG_DS_ID PP01_ID PP02_ID SUB01_ID SUB01_POLY < <(
  jq -r '[.dataset_id, .refs.pp01_id, .refs.pp02_id, .refs.sub01_id, (.refs.sub01_poly | tojson)] | @tsv' "$SEED_TMP/gen.json"
)
PG_FLOW_ID=$(cat "$SEED_TMP/flow_id")
POWER_GRID_SANDBOX_ID=$(cat "$SEED_TMP/sb_id")

# 刷新 SessionPool DuckLake 快照缓存
api PATCH "/api/axhub/datasets/$PG_DS_ID" '{
  "description": "26座变电站及下辖馈线最近24小时15分钟级电流、有功、线损与电压明细（43,200行）"
}' >/dev/null

# 3. 并行发布五区数字看板 power-grid-cockpit 与预跑主变跳闸沙盘推演（产出 earliest_deadline_step）
(
  EXISTING_DASH=$(api GET /api/xblueview/dashboards/by-slug/power-grid-cockpit 2>/dev/null || echo "{}")
  PG_DASH_ID=$(printf '%s' "$EXISTING_DASH" | jq -r '.id // empty' 2>/dev/null || true)
  PG_DASH_STATUS=$(printf '%s' "$EXISTING_DASH" | jq -r '.status // empty' 2>/dev/null || true)
  if [[ -z "$PG_DASH_ID" || "$PG_DASH_STATUS" != "PUBLISHED" ]]; then
    if [[ -n "$PG_DASH_ID" ]]; then
      api DELETE "/api/xblueview/dashboards/$PG_DASH_ID" >/dev/null 2>&1 || true
    fi
    CREATE_DASH_BODY=$(jq -c '{name, slug, description, theme, canvas}' "$PACK_DIR/dashboards/cockpit.json")
    PG_DASH_ID=$(api POST /api/xblueview/dashboards "$CREATE_DASH_BODY" | jq -r '.id')
    while IFS= read -r w_json; do
      api POST "/api/xblueview/dashboards/$PG_DASH_ID/widgets" "$w_json" >/dev/null &
    done < <(jq -c --arg fid "$PG_FLOW_ID" '
      .widgets[] | if .data_source.kind == "ACTION" then .data_source.flow_id = $fid else . end
    ' "$PACK_DIR/dashboards/cockpit.json")
    wait
    api POST "/api/xblueview/dashboards/$PG_DASH_ID/publish" '{}' >/dev/null
  fi
  printf '%s' "$PG_DASH_ID" > "$SEED_TMP/dash_id"
) &
DASH_PID=$!

(
  EXISTING_SCEN=$(api GET "/api/axsim/sandboxes/${POWER_GRID_SANDBOX_ID}/scenarios")
  HAS_DEADLINE=$(printf '%s' "$EXISTING_SCEN" | jq -r '
    (if type == "array" then . else (.items // []) end)
    | map(select(.status == "SUCCEEDED" and .deadline_report.earliest_deadline_step != null and .deadline_report.earliest_breach_step != null))
    | length
  ')
  if [[ "${HAS_DEADLINE:-0}" -lt 1 ]]; then
    sql_app default "
DELETE FROM ax_sim_scenarios WHERE tenant_id = 'default' AND sandbox_id = '${POWER_GRID_SANDBOX_ID}'::uuid;
DELETE FROM ax_sim_base_links WHERE tenant_id = 'default' AND sandbox_id = '${POWER_GRID_SANDBOX_ID}'::uuid;
DELETE FROM ax_sim_base_nodes WHERE tenant_id = 'default' AND sandbox_id = '${POWER_GRID_SANDBOX_ID}'::uuid;
INSERT INTO ax_sim_base_nodes (
    tenant_id, sandbox_id, node_id, type_name, node_key, title,
    metrics, properties, geo_lng, geo_lat, geo_shape, row_version
)
SELECT 'default', '${POWER_GRID_SANDBOX_ID}'::uuid, id, type_name, node_key, title,
       metrics, properties, geo_lng, geo_lat, geo_shape, row_version
  FROM xb_nodes
 WHERE tenant_id = 'default' AND subgraph_slug = 'power_grid';
INSERT INTO ax_sim_base_links (
    tenant_id, sandbox_id, link_id, link_type, src_node_id, dst_node_id, weight
)
SELECT 'default', '${POWER_GRID_SANDBOX_ID}'::uuid, id, link_type, src_node_id, dst_node_id, weight
  FROM xb_links
 WHERE tenant_id = 'default' AND subgraph_slug = 'power_grid';
UPDATE ax_sandboxes
   SET status = 'READY',
       node_count = (SELECT count(*) FROM ax_sim_base_nodes WHERE sandbox_id = '${POWER_GRID_SANDBOX_ID}'::uuid),
       link_count = (SELECT count(*) FROM ax_sim_base_links WHERE sandbox_id = '${POWER_GRID_SANDBOX_ID}'::uuid),
       updated_at = NOW()
 WHERE tenant_id = 'default' AND id = '${POWER_GRID_SANDBOX_ID}'::uuid;
" >/dev/null

    SCEN_REQ=$(jq -nc \
      --arg pp1 "$PP01_ID" \
      --arg pp2 "$PP02_ID" \
      --arg sub1 "$SUB01_ID" \
      --argjson poly "$SUB01_POLY" '{
        name: "迎峰度夏城北 220kV 主变跳闸推演",
        description: "模拟第3步城北热电/主变突发减供 -120MW 与负载率 >= 92% 自动投储能 +20MW，反推供电围栏内重点负荷母线电压红线最晚干预窗口",
        actions: [],
        params: { mode: "STEPPED", horizon_steps: 12, damping: 1.0 },
        scheduled_events: [
          {
            id: "ev_north_trip_step3",
            name: "第3步城北220kV主变跳闸减供",
            trigger_kind: "AT_STEP",
            at_step: 3,
            actions: [
              { target_node_id: $pp1, metric_key: "generation_mw", mode: "ADD", value: -120 },
              { target_node_id: $sub1, metric_key: "load_ratio", mode: "ADD", value: 0.08 }
            ]
          },
          {
            id: "ev_ess_auto_boost",
            name: "主变负载越限自动投入站内储能",
            trigger_kind: "METRIC_THRESHOLD",
            watch_node_id: $sub1,
            watch_metric: "load_ratio",
            op: "GTE",
            threshold: 0.92,
            cooldown_steps: 2,
            max_fires: 1,
            actions: [
              { target_node_id: $pp2, metric_key: "generation_mw", mode: "ADD", value: 20 }
            ]
          }
        ],
        goal_constraints: [
          {
            id: "goal_north_geofence_voltage",
            name: "城北220kV供电围栏重点负荷母线电压红线",
            target_type_name: "CriticalLoad",
            metric_key: "bus_voltage_pu",
            op: "GTE",
            threshold: 0.93,
            spatial_filter: {
              op: "GEO_WITHIN_POLYGON",
              polygon: $poly
            },
            controllable_node_ids: [$pp1, $pp2]
          }
        ]
      }')
    api POST "/api/axsim/sandboxes/${POWER_GRID_SANDBOX_ID}/scenarios" "$SCEN_REQ" >/dev/null
  fi
) &
SCEN_PID=$!

wait "$DASH_PID"
wait "$SCEN_PID"
PG_DASH_ID=$(cat "$SEED_TMP/dash_id")

upsert_ids_env "POWER_GRID_SANDBOX_ID" "$POWER_GRID_SANDBOX_ID"
upsert_ids_env "POWER_GRID_DATASET_ID" "$PG_DS_ID"
upsert_ids_env "POWER_GRID_DASHBOARD_ID" "$PG_DASH_ID"

# 5. 执行 L4 运行态认证落库
CERT_OUT=$(api POST /api/xblue/subgraphs/power_grid/certify '{}')
CERT_LVL=$(printf '%s' "$CERT_OUT" | jq -r '.cert_level // empty')
if [[ "$CERT_LVL" != "L4_CERTIFIED" ]]; then
  echo "power_grid.sh: expected L4_CERTIFIED, got: $CERT_OUT" >&2
  exit 1
fi

echo "POWER_GRID_SANDBOX_ID=$POWER_GRID_SANDBOX_ID"
