#!/usr/bin/env bash
# Step 33 Showcase Seed (1/2): `power_grid` 新型电力与配网调度可执行世界幂等秒级装载 (< 3s)
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
  echo "power_grid.sh: invalid SHOWCASE_TENANT_ID: $SHOWCASE_TENANT_ID" >&2
  exit 1
fi
: "${AXPOINT_DEV_TOKEN:?AXPOINT_DEV_TOKEN is required}"

mkdir -p "$ROOT/.gate"
PACK_DIR="$ROOT/templates/packs/power_grid"
PY_BIN="$ROOT/axpoint-parser/.venv/bin/python"
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
    echo "power_grid.sh: HTTP ${code} on ${m} ${p}: ${resp}" >&2
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
    echo "power_grid.sh: HTTP ${code} on GET ${p}: ${resp}" >&2
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

exec_tenant_sql() {
  local tenant="$1" sql_stmt="$2"
  if command -v psql >/dev/null 2>&1 && declare -F sql_app >/dev/null 2>&1; then
    sql_app "$tenant" "$sql_stmt"
  else
    "$PY_BIN" - "$tenant" "$sql_stmt" <<'PY'
import os, sys, psycopg
tenant, stmt = sys.argv[1], sys.argv[2]
dsn = os.environ.get("DATABASE_URL") or os.environ.get("MIGRATION_DATABASE_URL") or ""
with psycopg.connect(dsn) as conn:
    with conn.cursor() as cur:
        cur.execute("SELECT set_config('app.tenant_id', %s, true)", (tenant,))
        cur.execute(stmt)
    conn.commit()
PY
  fi
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

# 0. 严格真数据校验的幂等快路径（SHOWCASE_FORCE_REBUILD=1 时强制全量重建；冷启动先 2ms 探测看板避免重查询）
if [[ "${SHOWCASE_FORCE_REBUILD:-0}" != "1" ]]; then
  api_try_get /api/xblueview/dashboards/by-slug/power-grid-cockpit > "$SEED_TMP/fp_dash.json" || echo '{}' > "$SEED_TMP/fp_dash.json"
  if jq -e '.status == "PUBLISHED"' "$SEED_TMP/fp_dash.json" >/dev/null 2>&1; then
    FP_PIDS=()
    api_ok GET /api/axops/pulse > "$SEED_TMP/fp_pulse.json" & FP_PIDS+=("$!")
    api_ok GET "/api/xblue/schema?subgraph=main" > "$SEED_TMP/fp_main.json" & FP_PIDS+=("$!")
    api_ok GET /api/axsim/sandboxes > "$SEED_TMP/fp_sb.json" & FP_PIDS+=("$!")
    api_ok GET "/api/axhub/datasets?limit=200" > "$SEED_TMP/fp_ds.json" & FP_PIDS+=("$!")
    if wait_pids "${FP_PIDS[@]}" 2>/dev/null; then
      if jq -ne \
        --slurpfile pulse "$SEED_TMP/fp_pulse.json" \
        --slurpfile main "$SEED_TMP/fp_main.json" \
        --slurpfile dash "$SEED_TMP/fp_dash.json" \
        --slurpfile sb "$SEED_TMP/fp_sb.json" \
        --slurpfile ds "$SEED_TMP/fp_ds.json" \
        --slurpfile pack "$PACK_DIR/schema.json" '
        ($pulse[0].subgraphs | map(select(.slug == "power_grid"))[0]) as $pg
        | ($main[0].node_types | map(.name)) as $mn
        | ($main[0].edge_types | map(.name)) as $me
        | ((if ($ds[0] | type) == "array" then $ds[0] else ($ds[0].items // []) end) | map(select(.name == "grid_feeder_telemetry"))[0]) as $gds
        | ((if ($sb[0] | type) == "array" then $sb[0] else ($sb[0].items // []) end) | map(select(.name == "迎峰度夏配网调度推演沙盘"))[0]) as $gsb
        | ($pg != null and $pg.cert_level == "L4_CERTIFIED" and $pg.node_count == 324 and $pg.link_count >= 548)
        and ($gds != null and ($gds.row_count // 0) >= 43200)
        and ($gsb != null and ($gsb.id // "") != "")
        and ($dash[0].status == "PUBLISHED")
        and (([ $dash[0].widgets[]? | select((.data_source.sql // "") | contains("date_trunc(\u0027hour\u0027, ts)")) ] | length) == 0)
        and ([ $pack[0].node_types[].name ] | all(. as $n | ($mn | index($n)) != null))
        and ([ $pack[0].edge_types[].name ] | all(. as $e | ($me | index($e)) != null))
        and ([ $main[0].edge_types[] | ((.src_type_name as $s | $mn | index($s)) != null and (.dst_type_name as $d | $mn | index($d)) != null) ] | all)
      ' >/dev/null 2>&1; then
        FP_SB_ID=$(jq -r '(if type == "array" then . else (.items // []) end) | map(select(.name == "迎峰度夏配网调度推演沙盘"))[0].id' "$SEED_TMP/fp_sb.json")
        FP_SCEN=$(api_ok GET "/api/axsim/sandboxes/${FP_SB_ID}/scenarios")
        if printf '%s' "$FP_SCEN" | jq -e '
          (if type == "array" then . else (.items // []) end)
          | map(select(.status == "SUCCEEDED" and .deadline_report.earliest_deadline_step != null and .deadline_report.earliest_breach_step != null))
          | length >= 1
        ' >/dev/null 2>&1; then
          FP_DS_ID=$(jq -r '(if type == "array" then . else (.items // []) end) | map(select(.name == "grid_feeder_telemetry"))[0].id' "$SEED_TMP/fp_ds.json")
          FP_DASH_ID=$(jq -r '.id' "$SEED_TMP/fp_dash.json")
          if [[ "$SHOWCASE_TENANT_ID" == "default" ]]; then
            upsert_ids_env "POWER_GRID_SANDBOX_ID" "$FP_SB_ID"
            upsert_ids_env "POWER_GRID_DATASET_ID" "$FP_DS_ID"
            upsert_ids_env "POWER_GRID_DASHBOARD_ID" "$FP_DASH_ID"
          fi
          echo "POWER_GRID_SANDBOX_ID=$FP_SB_ID"
          exit 0
        fi
      fi
    fi
  fi
fi

# 1. 确保 power_grid 生产子图存在（仅需 1 次轻量查询）
SUB_LIST=$(api_ok GET /api/xblue/subgraphs)
if ! printf '%s' "$SUB_LIST" | jq -e '(if type == "array" then . else (.items // []) end) | map(select(.slug == "power_grid")) | length > 0' >/dev/null 2>&1; then
  api_ok POST /api/xblue/subgraphs '{
    "slug": "power_grid",
    "display_name": "新型电力与配网调度",
    "description": "电源—输变电—配电—重点负荷四域三层可执行调度模型（多边形围栏与馈线四路径过载传导）",
    "is_dev_sandbox": false
  }' >/dev/null
fi

# 2. 双轨并发：Track A 向量化灌装 324 节点 / 548 边 / 43,200 遥测湖表；Track B 发布 Schema、成对合并 main、规则、PROD 动作流与沙盘元数据
"$PY_BIN" "$ROOT/axpoint-parser/scripts/gen_worlds_grid_fin.py" --world power_grid --tenant "$SHOWCASE_TENANT_ID" > "$SEED_TMP/gen.json" &
GEN_PID=$!

(
  PG_SCHEMA_RAW=$(cat "$PACK_DIR/schema.json")
  S2_PIDS=()

  api_ok POST "/api/xblue/schema?subgraph=power_grid" "$(printf '%s' "$PG_SCHEMA_RAW" | jq -c '. + {"force": true, "change_summary": "publish power_grid world schema"}')" >/dev/null &
  S2_PIDS+=("$!")

  api_ok PUT /api/xblue/subgraphs/power_grid/competency-suite "$(cat "$PACK_DIR/competency.json")" >/dev/null &
  S2_PIDS+=("$!")

  (
    CUR_MAIN=$(api_ok GET "/api/xblue/schema?subgraph=main")
    if ! jq -ne --argjson cur "$CUR_MAIN" --argjson pg "$PG_SCHEMA_RAW" '
      ($cur.node_types | map(.name)) as $mn
      | ($cur.edge_types | map(.name)) as $me
      | ([ $pg.node_types[].name ] | all(. as $n | ($mn | index($n)) != null))
      and ([ $pg.edge_types[].name ] | all(. as $e | ($me | index($e)) != null))
      and ([ $cur.edge_types[] | ((.src_type_name as $s | $mn | index($s)) != null and (.dst_type_name as $d | $mn | index($d)) != null) ] | all)
    ' >/dev/null 2>&1; then
      MERGED_MAIN=$(jq -nc \
        --argjson cur "$CUR_MAIN" \
        --argjson pg "$PG_SCHEMA_RAW" '
        ([ $pg.node_types[].name ]) as $nnames
        | ([ $pg.edge_types[].name ]) as $enames
        | (([ $cur.node_types[] | select((.name as $n | $nnames | index($n)) == null) ]) + $pg.node_types) as $all_nodes
        | ($all_nodes | map(.name)) as $valid_nnames
        | {
            node_types: $all_nodes,
            edge_types: (
              ([ $cur.edge_types[]
                 | select((.name as $e | $enames | index($e)) == null)
                 | select((.src_type_name as $s | $valid_nnames | index($s)) != null and (.dst_type_name as $d | $valid_nnames | index($d)) != null)
               ]) + $pg.edge_types
            ),
            mappings: ($cur.mappings // []),
            force: true,
            change_summary: "merge power_grid node and edge types into main"
          }
      ')
      api_ok POST "/api/xblue/schema?subgraph=main" "$MERGED_MAIN" >/dev/null
    fi
  ) &
  S2_PIDS+=("$!")

  (
    EXISTING_RULES=$(api_ok GET "/api/axflow/rules?limit=200")
    R_PIDS=()
    while IFS= read -r rule_json; do
      r_name=$(printf '%s' "$rule_json" | jq -r '.name')
      if ! printf '%s' "$EXISTING_RULES" | jq -e --arg nm "$r_name" '(if type == "array" then . else (.items // []) end) | map(select(.name == $nm)) | length > 0' >/dev/null 2>&1; then
        api_ok POST /api/axflow/rules "$rule_json" >/dev/null &
        R_PIDS+=("$!")
      fi
    done < <(jq -c '.rules[]' "$PACK_DIR/rules.json")
    if [[ ${#R_PIDS[@]} -gt 0 ]]; then wait_pids "${R_PIDS[@]}"; fi
  ) &
  S2_PIDS+=("$!")

  (
    PG_FLOW_ID=$(api_ok GET "/api/axflow/flows?limit=200" | jq -r '(if type == "array" then . else (.items // []) end)[] | select(.slug == "power-grid-ess-dispatch") | .id' | head -n 1 || true)
    if [[ -z "$PG_FLOW_ID" || "$PG_FLOW_ID" == "null" ]]; then
      FLOW_BODY=$(jq -c 'del(.key)' "$PACK_DIR/flows/ess_dispatch.json")
      PG_FLOW_ID=$(api_ok POST /api/axflow/flows "$FLOW_BODY" | jq -r '.id')
      api_ok POST "/api/axflow/flows/$PG_FLOW_ID/versions" '{"note":"power_grid ess dispatch v1"}' >/dev/null
      api_ok POST "/api/axflow/flows/$PG_FLOW_ID/deploy" '{"version":1,"environment":"PROD"}' >/dev/null
    fi
    printf '%s' "$PG_FLOW_ID" > "$SEED_TMP/flow_id"
  ) &
  S2_PIDS+=("$!")

  (
    SB_ID=$(api_ok GET /api/axsim/sandboxes | jq -r '(if type == "array" then . else (.items // []) end)[] | select(.name == "迎峰度夏配网调度推演沙盘") | .id' | head -n 1 || true)
    if [[ -z "$SB_ID" || "$SB_ID" == "null" ]]; then
      SB_ID=$(api_ok POST /api/axsim/sandboxes '{
        "name": "迎峰度夏配网调度推演沙盘",
        "scope": {
          "type_names": ["PowerPlant", "Substation", "DistTransformer", "CriticalLoad"]
        }
      }' | jq -r '.id')
    fi
    printf '%s' "$SB_ID" > "$SEED_TMP/sb_id"
  ) &
  S2_PIDS+=("$!")

  api_try_get /api/xblueview/dashboards/by-slug/power-grid-cockpit > "$SEED_TMP/existing_dash.json" &
  S2_PIDS+=("$!")

  wait_pids "${S2_PIDS[@]}"
) &
SETUP_PID=$!

wait_pids "$GEN_PID" "$SETUP_PID"

read -r PG_DS_ID PP01_ID PP02_ID SUB01_ID SUB01_POLY < <(
  jq -r '[.dataset_id, .refs.pp01_id, .refs.pp02_id, .refs.sub01_id, (.refs.sub01_poly | tojson)] | @tsv' "$SEED_TMP/gen.json"
)
PG_FLOW_ID=$(cat "$SEED_TMP/flow_id")
POWER_GRID_SANDBOX_ID=$(cat "$SEED_TMP/sb_id")

# 同步刷新 SessionPool DuckLake 快照缓存（必须在看板发布预检前完成）
api_ok PATCH "/api/axhub/datasets/$PG_DS_ID" '{
  "description": "26座变电站及下辖馈线最近24小时15分钟级电流、有功、线损与电压明细（43,200行）"
}' >/dev/null

# 3. 并行发布五区数字看板 power-grid-cockpit、预跑主变跳闸沙盘推演与执行 L4 运行态认证
(
  EXISTING_DASH=$(cat "$SEED_TMP/existing_dash.json")
  PG_DASH_ID=$(printf '%s' "$EXISTING_DASH" | jq -r '.id // empty' 2>/dev/null || true)
  PG_DASH_STATUS=$(printf '%s' "$EXISTING_DASH" | jq -r '.status // empty' 2>/dev/null || true)
  PG_LEGACY_SQL=$(printf '%s' "$EXISTING_DASH" | jq -r '([.widgets[]? | select((.data_source.sql // "") | contains("date_trunc(\u0027hour\u0027, ts)"))] | length) > 0' 2>/dev/null || echo "false")
  if [[ -z "$PG_DASH_ID" || "$PG_DASH_STATUS" != "PUBLISHED" || "$PG_LEGACY_SQL" == "true" || "${SHOWCASE_FORCE_REBUILD:-0}" == "1" ]]; then
    if [[ -n "$PG_DASH_ID" ]]; then
      api_ok DELETE "/api/xblueview/dashboards/$PG_DASH_ID" >/dev/null
    fi
    COCKPIT_DEF=$(cat "$PACK_DIR/dashboards/cockpit.json")
    CREATE_DASH_BODY=$(printf '%s' "$COCKPIT_DEF" | jq -c '{name, slug, description, theme, zone_layout: "FREE_GRID", canvas: (.canvas | .zone_layout = "FREE_GRID")}')
    PG_DASH_ID=$(api_ok POST /api/xblueview/dashboards "$CREATE_DASH_BODY" | jq -r '.id')
    W_PIDS=()
    while IFS= read -r w_json; do
      api_ok POST "/api/xblueview/dashboards/$PG_DASH_ID/widgets" "$w_json" >/dev/null &
      W_PIDS+=("$!")
    done < <(printf '%s' "$COCKPIT_DEF" | jq -c --arg fid "$PG_FLOW_ID" '
      .widgets[] | if .data_source.kind == "ACTION" then .data_source.flow_id = $fid else . end
    ')
    if [[ ${#W_PIDS[@]} -gt 0 ]]; then wait_pids "${W_PIDS[@]}"; fi
    api_ok PUT "/api/xblueview/dashboards/$PG_DASH_ID" "$(printf '%s' "$COCKPIT_DEF" | jq -c '{canvas: .canvas}')" >/dev/null
    PUB_ST=$(api_ok POST "/api/xblueview/dashboards/$PG_DASH_ID/publish" '{}' | jq -r '.status // empty')
    if [[ "$PUB_ST" != "PUBLISHED" ]]; then
      echo "power_grid.sh: expected power-grid-cockpit PUBLISHED, got: $PUB_ST" >&2
      exit 1
    fi
  fi
  printf '%s' "$PG_DASH_ID" > "$SEED_TMP/dash_id"
) &
DASH_PID=$!

(
  EXISTING_SCEN=$(api_ok GET "/api/axsim/sandboxes/${POWER_GRID_SANDBOX_ID}/scenarios")
  HAS_DEADLINE=$(printf '%s' "$EXISTING_SCEN" | jq -r '
    (if type == "array" then . else (.items // []) end)
    | map(select(.status == "SUCCEEDED" and .deadline_report.earliest_deadline_step != null and .deadline_report.earliest_breach_step != null))
    | length
  ')
  if [[ "${HAS_DEADLINE:-0}" -lt 1 || "${SHOWCASE_FORCE_REBUILD:-0}" == "1" ]]; then
    exec_tenant_sql "$SHOWCASE_TENANT_ID" "
DELETE FROM ax_sim_scenarios WHERE tenant_id = '${SHOWCASE_TENANT_ID}' AND sandbox_id = '${POWER_GRID_SANDBOX_ID}'::uuid;
DELETE FROM ax_sim_base_links WHERE tenant_id = '${SHOWCASE_TENANT_ID}' AND sandbox_id = '${POWER_GRID_SANDBOX_ID}'::uuid;
DELETE FROM ax_sim_base_nodes WHERE tenant_id = '${SHOWCASE_TENANT_ID}' AND sandbox_id = '${POWER_GRID_SANDBOX_ID}'::uuid;
INSERT INTO ax_sim_base_nodes (
    tenant_id, sandbox_id, node_id, type_name, node_key, title,
    metrics, properties, geo_lng, geo_lat, geo_shape, row_version
)
SELECT '${SHOWCASE_TENANT_ID}', '${POWER_GRID_SANDBOX_ID}'::uuid, id, type_name, node_key, title,
       metrics, properties, geo_lng, geo_lat, geo_shape, row_version
  FROM xb_nodes
 WHERE tenant_id = '${SHOWCASE_TENANT_ID}' AND subgraph_slug = 'power_grid';
INSERT INTO ax_sim_base_links (
    tenant_id, sandbox_id, link_id, link_type, src_node_id, dst_node_id, weight
)
SELECT '${SHOWCASE_TENANT_ID}', '${POWER_GRID_SANDBOX_ID}'::uuid, id, link_type, src_node_id, dst_node_id, weight
  FROM xb_links
 WHERE tenant_id = '${SHOWCASE_TENANT_ID}' AND subgraph_slug = 'power_grid';
UPDATE ax_sandboxes
   SET status = 'READY',
       node_count = (SELECT count(*) FROM ax_sim_base_nodes WHERE sandbox_id = '${POWER_GRID_SANDBOX_ID}'::uuid),
       link_count = (SELECT count(*) FROM ax_sim_base_links WHERE sandbox_id = '${POWER_GRID_SANDBOX_ID}'::uuid),
       updated_at = NOW()
 WHERE tenant_id = '${SHOWCASE_TENANT_ID}' AND id = '${POWER_GRID_SANDBOX_ID}'::uuid;
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
    api_ok POST "/api/axsim/sandboxes/${POWER_GRID_SANDBOX_ID}/scenarios" "$SCEN_REQ" >/dev/null
  fi
) &
SCEN_PID=$!

(
  CERT_OUT=$(api_ok POST /api/xblue/subgraphs/power_grid/certify '{}')
  CERT_LVL=$(printf '%s' "$CERT_OUT" | jq -r '.cert_level // empty')
  if [[ "$CERT_LVL" != "L4_CERTIFIED" ]]; then
    echo "power_grid.sh: expected L4_CERTIFIED, got: $CERT_OUT" >&2
    exit 1
  fi
) &
CERT_PID=$!

wait_pids "$DASH_PID" "$SCEN_PID" "$CERT_PID"
PG_DASH_ID=$(cat "$SEED_TMP/dash_id")

if [[ "$SHOWCASE_TENANT_ID" == "default" ]]; then
  upsert_ids_env "POWER_GRID_SANDBOX_ID" "$POWER_GRID_SANDBOX_ID"
  upsert_ids_env "POWER_GRID_DATASET_ID" "$PG_DS_ID"
  upsert_ids_env "POWER_GRID_DASHBOARD_ID" "$PG_DASH_ID"
fi

echo "POWER_GRID_SANDBOX_ID=$POWER_GRID_SANDBOX_ID"
