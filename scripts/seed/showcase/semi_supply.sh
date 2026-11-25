#!/usr/bin/env bash
# 幂等向量化装载：半导体全球多级供应链（semi_supply，526 节点 / 1,350 边 / 30,000 行传感湖表 / 16,800 时序点）
set -euo pipefail

_CALLER_CORE_BASE_URL="${CORE_BASE_URL:-}"
_CALLER_PARSER_BASE_URL="${PARSER_BASE_URL:-}"
_CALLER_DEV_TOKEN="${AXPOINT_DEV_TOKEN:-}"
_CALLER_DB_URL="${DATABASE_URL:-}"
_CALLER_MIG_DB_URL="${MIGRATION_DATABASE_URL:-}"
_CALLER_LAKE_DSN="${AXPOINT_LAKE_CATALOG_DSN:-}"
_CALLER_DATA_ROOT="${AXPOINT_DATA_ROOT:-}"

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
if [[ -f "$ROOT/.env.test" ]]; then
  # shellcheck disable=SC1091
  source "$ROOT/scripts/gate/lib.sh"
else
  [[ -d "$ROOT/axpoint-parser/.venv/bin" ]] && export PATH="$ROOT/axpoint-parser/.venv/bin:$PATH"
  if [[ -z "${DATABASE_URL:-}" && -f "$ROOT/.env" ]]; then
    set -a
    # shellcheck disable=SC1091
    source "$ROOT/.env"
    set +a
  fi
fi

[[ -n "$_CALLER_CORE_BASE_URL" ]] && export CORE_BASE_URL="$_CALLER_CORE_BASE_URL"
[[ -n "$_CALLER_PARSER_BASE_URL" ]] && export PARSER_BASE_URL="$_CALLER_PARSER_BASE_URL"
[[ -n "$_CALLER_DEV_TOKEN" ]] && export AXPOINT_DEV_TOKEN="$_CALLER_DEV_TOKEN"
[[ -n "$_CALLER_DB_URL" ]] && export DATABASE_URL="$_CALLER_DB_URL"
[[ -n "$_CALLER_MIG_DB_URL" ]] && export MIGRATION_DATABASE_URL="$_CALLER_MIG_DB_URL"
[[ -n "$_CALLER_LAKE_DSN" ]] && export AXPOINT_LAKE_CATALOG_DSN="$_CALLER_LAKE_DSN"
[[ -n "$_CALLER_DATA_ROOT" ]] && export AXPOINT_DATA_ROOT="$_CALLER_DATA_ROOT"

CORE_BASE_URL="${CORE_BASE_URL:-http://127.0.0.1:9995}"
SHOWCASE_TENANT_ID="${SHOWCASE_TENANT_ID:-default}"
if [[ ! "$SHOWCASE_TENANT_ID" =~ ^[a-z][a-z0-9_]{0,62}$ ]]; then
  echo "semi_supply.sh: invalid SHOWCASE_TENANT_ID: $SHOWCASE_TENANT_ID" >&2
  exit 1
fi
: "${AXPOINT_DEV_TOKEN:?AXPOINT_DEV_TOKEN is required}"

mkdir -p "$ROOT/.gate"
PACK_DIR="$ROOT/templates/packs/semi_supply"
PY_BIN="${ROOT}/axpoint-parser/.venv/bin/python"
[[ -x "$PY_BIN" ]] || PY_BIN="python3"

api_ok() {
  local m=$1 p=$2 body=${3:-} raw code resp
  if [[ -n "$body" ]]; then
    raw=$(curl -sS -w '\n%{http_code}' -X "$m" \
      -H "Authorization: Bearer $AXPOINT_DEV_TOKEN" \
      -H "Accept: application/json" \
      -H "Content-Type: application/json" \
      "$CORE_BASE_URL$p" -d "$body")
  else
    raw=$(curl -sS -w '\n%{http_code}' -X "$m" \
      -H "Authorization: Bearer $AXPOINT_DEV_TOKEN" \
      -H "Accept: application/json" \
      "$CORE_BASE_URL$p")
  fi
  code="${raw##*$'\n'}"
  resp="${raw%$'\n'*}"
  if [[ ! "$code" =~ ^2[0-9][0-9]$ ]]; then
    echo "semi_supply.sh: HTTP ${code} on ${m} ${p}: ${resp}" >&2
    return 1
  fi
  printf '%s' "$resp"
}

api_try_get() {
  local p=$1 raw code resp
  raw=$(curl -sS -w '\n%{http_code}' -X GET \
    -H "Authorization: Bearer $AXPOINT_DEV_TOKEN" \
    -H "Accept: application/json" \
    "$CORE_BASE_URL$p")
  code="${raw##*$'\n'}"
  resp="${raw%$'\n'*}"
  if [[ "$code" == "404" ]]; then
    printf '{}'
    return 0
  fi
  if [[ ! "$code" =~ ^2[0-9][0-9]$ ]]; then
    echo "semi_supply.sh: HTTP ${code} on GET ${p}: ${resp}" >&2
    return 1
  fi
  printf '%s' "$resp"
}

wait_pids() {
  local _pid
  for _pid in "$@"; do
    wait "$_pid"
  done
}

upsert_ids_env() {
  local key="$1" val="$2" env_file="$ROOT/.gate/ids.env"
  local lock_dir="${env_file}.lock"
  for _ in $(seq 1 50); do
    mkdir "$lock_dir" 2>/dev/null && break
    sleep 0.02
  done
  touch "$env_file"
  local tmp="${env_file}.tmp.$$"
  grep -v "^${key}=" "$env_file" > "$tmp" 2>/dev/null || true
  printf '%s=%s\n' "$key" "$val" >> "$tmp"
  mv "$tmp" "$env_file"
  rmdir "$lock_dir" 2>/dev/null || true
}

SEED_TMP=$(mktemp -d)
trap 'rm -rf "$SEED_TMP"' EXIT

# 0. 严格真数据校验的幂等快路径（SHOWCASE_FORCE_REBUILD=1 时强制全量重建）
if [[ "${SHOWCASE_FORCE_REBUILD:-0}" != "1" ]]; then
  FP_PIDS=()
  api_ok GET /api/axops/pulse > "$SEED_TMP/fp_pulse.json" & FP_PIDS+=("$!")
  api_ok GET "/api/xblue/schema?subgraph=main" > "$SEED_TMP/fp_main.json" & FP_PIDS+=("$!")
  api_try_get /api/xblueview/dashboards/by-slug/semi-supply-cockpit > "$SEED_TMP/fp_dash.json" & FP_PIDS+=("$!")
  api_ok GET /api/axsim/sandboxes > "$SEED_TMP/fp_sb.json" & FP_PIDS+=("$!")
  api_ok GET "/api/axhub/datasets?limit=200" > "$SEED_TMP/fp_ds.json" & FP_PIDS+=("$!")
  api_ok GET "/api/xblue/nodes?subgraph=main&type_name=AutoOemOrder&limit=20" > "$SEED_TMP/fp_oem.json" & FP_PIDS+=("$!")
  if wait_pids "${FP_PIDS[@]}" 2>/dev/null; then
    if jq -ne \
      --slurpfile pulse "$SEED_TMP/fp_pulse.json" \
      --slurpfile main "$SEED_TMP/fp_main.json" \
      --slurpfile dash "$SEED_TMP/fp_dash.json" \
      --slurpfile sb "$SEED_TMP/fp_sb.json" \
      --slurpfile ds "$SEED_TMP/fp_ds.json" \
      --slurpfile oem "$SEED_TMP/fp_oem.json" \
      --slurpfile pack "$PACK_DIR/schema.json" '
      ($pulse[0].subgraphs | map(select(.slug == "semi_supply"))[0]) as $sm
      | ($main[0].node_types | map(.name)) as $mn
      | ($main[0].edge_types | map(.name)) as $me
      | ((if ($ds[0] | type) == "array" then $ds[0] else ($ds[0].items // []) end) | map(select(.name == "semi_lot_telemetry"))[0]) as $sds
      | ((if ($sb[0] | type) == "array" then $sb[0] else ($sb[0].items // []) end) | map(select(.name == "半导体晶圆断供与清关延误沙盘"))[0]) as $ssb
      | (($oem[0].items // []) | map(select(.node_key == "OEM-001"))[0]) as $oem01
      | ($sm != null and $sm.cert_level == "L4_CERTIFIED" and $sm.node_count >= 520 and $sm.link_count >= 1340)
      and ($sds != null and ($sds.row_count // 0) >= 30000)
      and ($ssb != null and ($ssb.id // "") != "")
      and ($oem01 != null and ($oem01.id // "") != "")
      and ($dash[0].status == "PUBLISHED")
      and ([ $pack[0].node_types[].name ] | all(. as $n | ($mn | index($n)) != null))
      and ([ $pack[0].edge_types[].name ] | all(. as $e | ($me | index($e)) != null))
      and ([ $main[0].edge_types[] | ((.src_type_name as $s | $mn | index($s)) != null and (.dst_type_name as $d | $mn | index($d)) != null) ] | all)
    ' >/dev/null 2>&1; then
      FP_SB_ID=$(jq -r '(if type == "array" then . else (.items // []) end) | map(select(.name == "半导体晶圆断供与清关延误沙盘"))[0].id' "$SEED_TMP/fp_sb.json")
      FP_DASH_ID=$(jq -r '.id' "$SEED_TMP/fp_dash.json")
      FP_OEM01_ID=$(jq -r '(.items // []) | map(select(.node_key == "OEM-001"))[0].id' "$SEED_TMP/fp_oem.json")
      if [[ "$SHOWCASE_TENANT_ID" == "default" ]]; then
        upsert_ids_env "SEMI_SUPPLY_SANDBOX_ID" "$FP_SB_ID"
        upsert_ids_env "SEMI_SUPPLY_DASHBOARD_ID" "$FP_DASH_ID"
        upsert_ids_env "SEMI_SUPPLY_OEM01_ID" "$FP_OEM01_ID"
      fi
      echo "SEMI_SUPPLY_SANDBOX_ID=${FP_SB_ID}"
      echo "SEMI_SUPPLY_DASHBOARD_ID=${FP_DASH_ID}"
      echo "SEMI_SUPPLY_OEM01_ID=${FP_OEM01_ID}"
      exit 0
    fi
  fi
fi

# 1. 创建或更新生产业务子图 semi_supply 并发布四域三层 Schema
SUBGRAPHS_JSON=$(api_ok GET /api/xblue/subgraphs)
if ! printf '%s' "$SUBGRAPHS_JSON" | jq -e '(if type == "array" then . else (.items // []) end) | map(select(.slug == "semi_supply")) | length > 0' >/dev/null 2>&1; then
  api_ok POST /api/xblue/subgraphs '{
    "slug": "semi_supply",
    "display_name": "半导体全球供应链",
    "description": "采购、制造、物流、履约四域泳道与晶圆批次/良率三层折叠模型",
    "owner_dept": "default",
    "is_dev_sandbox": false,
    "source_template_id": "semi_supply"
  }' >/dev/null
fi

SEMI_SCHEMA_RAW=$(cat "$PACK_DIR/schema.json")
S1_PIDS=()
api_ok POST "/api/xblue/schema?subgraph=semi_supply" "$(printf '%s' "$SEMI_SCHEMA_RAW" | jq -c '. + {force: true, change_summary: "seed semi_supply v4 showcase"}')" >/dev/null &
S1_PIDS+=("$!")
api_ok PUT /api/xblue/subgraphs/semi_supply/competency-suite "$(cat "$PACK_DIR/competency.json")" >/dev/null &
S1_PIDS+=("$!")

# 同步成对合并对象与关系类型至企业主图（同时自动剥离 main 中任何历史无源/宿孤立边）
(
  CUR_MAIN=$(api_ok GET "/api/xblue/schema?subgraph=main")
  SEMI_NAMES='["SpecialtyVendor","WaferFab","LithoTool","WaferLotEvent","YieldRollupCube","CustomsPort","RegionalHub","AutoOemOrder"]'
  SEMI_EDGES='["SUPPLIES_MATERIAL","EQUIPPED_WITH","RUNS_LOT","SUMMARIZES_YIELD","SHIPS_VIA_PORT","DISPATCHES_TO_HUB","DELIVERS_ORDER"]'
  MERGED_MAIN=$(jq -nc \
    --argjson cur "$CUR_MAIN" \
    --argjson semi "$SEMI_SCHEMA_RAW" \
    --argjson nnames "$SEMI_NAMES" \
    --argjson enames "$SEMI_EDGES" '
    (([ $cur.node_types[] | select((.name as $n | $nnames | index($n)) == null) ]) + $semi.node_types) as $all_nodes
    | ($all_nodes | map(.name)) as $valid_nnames
    | {
        node_types: $all_nodes,
        edge_types: (
          ([ $cur.edge_types[]
             | select((.name as $e | $enames | index($e)) == null)
             | select((.src_type_name as $s | $valid_nnames | index($s)) != null and (.dst_type_name as $d | $valid_nnames | index($d)) != null)
           ]) + $semi.edge_types
        ),
        mappings: ($cur.mappings // []),
        force: true,
        change_summary: "merge semi_supply types"
      }
  ')
  api_ok POST "/api/xblue/schema?subgraph=main" "$MERGED_MAIN" >/dev/null
) &
S1_PIDS+=("$!")
wait_pids "${S1_PIDS[@]}"

# 2. 向量化写入 30,000 行 DuckLake 湖表、526 节点、1,350 边与 16,800 时序点
SEED_OUT=$(PYTHONPATH="$ROOT/axpoint-parser" "$PY_BIN" "$ROOT/axpoint-parser/scripts/gen_worlds_semi_med.py" --world semi_supply --tenant "$SHOWCASE_TENANT_ID")
read -r SEMI_DS_ID FAB01_ID PORT01_ID HUB01_ID OEM01_ID < <(
  printf '%s' "$SEED_OUT" | jq -r '[.dataset_id, .main_anchors.fab01_id, .main_anchors.port01_id, .main_anchors.hub01_id, .main_anchors.oem01_id] | @tsv'
)

api_ok PATCH "/api/axhub/datasets/$SEMI_DS_ID" '{
  "description": "晶圆流片批次与关键光刻机台腔室温度、缺陷密度30,000行遥测明细"
}' >/dev/null

# 3 & 4. 并行配置 AxSim 晶圆断供沙盘推演与发布五区供应链数字看板 semi-supply-cockpit
(
  SB_NAME="半导体晶圆断供与清关延误沙盘"
  SEMI_SB_ID=$(api_ok GET /api/axsim/sandboxes | jq -r --arg n "$SB_NAME" '(if type == "array" then . else (.items // []) end)[] | select(.name == $n) | .id' | head -n 1 || true)
  if [[ -z "$SEMI_SB_ID" || "$SEMI_SB_ID" == "null" ]]; then
    SEMI_SB_ID=$(api_ok POST /api/axsim/sandboxes "$(jq -nc --arg n "$SB_NAME" '{
      name: $n,
      scope: {
        type_names: ["WaferFab", "LithoTool", "CustomsPort", "RegionalHub", "AutoOemOrder"],
        link_types: ["EQUIPPED_WITH", "SHIPS_VIA_PORT", "DISPATCHES_TO_HUB", "DELIVERS_ORDER"]
      }
    }')" | jq -r '.id')
  else
    api_ok POST "/api/axsim/sandboxes/${SEMI_SB_ID}/refresh-base" '{}' >/dev/null
  fi

  SCEN_NAME="口岸清关拥堵与车规芯片库存红线推演"
  EXISTING_SCEN=$(api_ok GET "/api/axsim/sandboxes/${SEMI_SB_ID}/scenarios" | jq -r --arg s "$SCEN_NAME" 'if type == "array" then (.[] | select(.name == $s) | .id) else (.items[]? | select(.name == $s) | .id) end' | head -n 1 || true)
  if [[ -n "$EXISTING_SCEN" && "$EXISTING_SCEN" != "null" && "${SHOWCASE_FORCE_REBUILD:-0}" == "1" ]]; then
    api_ok DELETE "/api/axsim/scenarios/${EXISTING_SCEN}" >/dev/null
    EXISTING_SCEN=""
  fi
  if [[ -z "$EXISTING_SCEN" || "$EXISTING_SCEN" == "null" ]]; then
    api_ok POST "/api/axsim/sandboxes/${SEMI_SB_ID}/scenarios" "$(jq -nc \
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
  printf '%s' "$SEMI_SB_ID" > "$SEED_TMP/sb_id"
) &
SB_PID=$!

(
  EXISTING_DASH=$(api_try_get /api/xblueview/dashboards/by-slug/semi-supply-cockpit)
  SEMI_DASH_ID=$(printf '%s' "$EXISTING_DASH" | jq -r 'if type == "object" then (.id // empty) else empty end' 2>/dev/null || true)
  SEMI_DASH_STATUS=$(printf '%s' "$EXISTING_DASH" | jq -r 'if type == "object" then (.status // empty) else empty end' 2>/dev/null || true)
  if [[ -z "$SEMI_DASH_ID" || "$SEMI_DASH_ID" == "null" || "$SEMI_DASH_STATUS" != "PUBLISHED" || "${SHOWCASE_FORCE_REBUILD:-0}" == "1" ]]; then
    if [[ -n "$SEMI_DASH_ID" && "$SEMI_DASH_ID" != "null" ]]; then
      api_ok DELETE "/api/xblueview/dashboards/${SEMI_DASH_ID}" >/dev/null
    fi
    COCKPIT_DEF=$(cat "$PACK_DIR/dashboards/cockpit.json")
    SEMI_DASH_ID=$(api_ok POST /api/xblueview/dashboards "$(printf '%s' "$COCKPIT_DEF" | jq -c '{
      name: .name,
      slug: .slug,
      description: .description,
      theme: .theme,
      zone_layout: "FREE_GRID",
      canvas: (.canvas | .zone_layout = "FREE_GRID")
    }')" | jq -r '.id')

    ADD_PIDS=()
    while IFS= read -r W_BODY; do
      api_ok POST "/api/xblueview/dashboards/${SEMI_DASH_ID}/widgets" "$W_BODY" >/dev/null &
      ADD_PIDS+=("$!")
    done < <(printf '%s' "$COCKPIT_DEF" | jq -c --arg port_id "$PORT01_ID" '
      (.widgets | .[9] = {
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
      })[]
    ')
    if [[ ${#ADD_PIDS[@]} -gt 0 ]]; then wait_pids "${ADD_PIDS[@]}"; fi
    api_ok PUT "/api/xblueview/dashboards/${SEMI_DASH_ID}" "$(printf '%s' "$COCKPIT_DEF" | jq -c '{canvas: .canvas}')" >/dev/null
    api_ok POST "/api/xblueview/dashboards/${SEMI_DASH_ID}/publish" '{}' >/dev/null
  fi
  printf '%s' "$SEMI_DASH_ID" > "$SEED_TMP/dash_id"
) &
DASH_PID=$!

wait_pids "$SB_PID" "$DASH_PID"

SEMI_SB_ID=$(cat "$SEED_TMP/sb_id")
SEMI_DASH_ID=$(cat "$SEED_TMP/dash_id")

VERIFY_DASH_ST=$(api_ok GET /api/xblueview/dashboards/by-slug/semi-supply-cockpit | jq -r '.status // empty')
if [[ "$VERIFY_DASH_ST" != "PUBLISHED" ]]; then
  echo "semi_supply.sh: expected semi-supply-cockpit PUBLISHED, got: $VERIFY_DASH_ST" >&2
  exit 1
fi

if [[ "$SHOWCASE_TENANT_ID" == "default" ]]; then
  upsert_ids_env "SEMI_SUPPLY_SANDBOX_ID" "$SEMI_SB_ID"
  upsert_ids_env "SEMI_SUPPLY_DASHBOARD_ID" "$SEMI_DASH_ID"
  upsert_ids_env "SEMI_SUPPLY_OEM01_ID" "$OEM01_ID"
fi

# 5. 执行子图运行态 L4 认证落库
CERT_SEMI=$(api_ok POST /api/xblue/subgraphs/semi_supply/certify '{}')
CERT_LEVEL=$(printf '%s' "$CERT_SEMI" | jq -r '.cert_level')
if [[ "$CERT_LEVEL" != "L4_CERTIFIED" ]]; then
  echo "semi_supply certification did not reach L4_CERTIFIED: $CERT_SEMI" >&2
  exit 1
fi

echo "SEMI_SUPPLY_SANDBOX_ID=${SEMI_SB_ID}"
echo "SEMI_SUPPLY_DASHBOARD_ID=${SEMI_DASH_ID}"
echo "SEMI_SUPPLY_OEM01_ID=${OEM01_ID}"
