#!/usr/bin/env bash
# 幂等向量化装载：城市急诊与医疗床位调度（med_dispatch，264 节点 / 508 边 / 18,000 行分诊湖表 / 20,000 时序点）
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
  echo "med_dispatch.sh: invalid SHOWCASE_TENANT_ID: $SHOWCASE_TENANT_ID" >&2
  exit 1
fi
: "${AXPOINT_DEV_TOKEN:?AXPOINT_DEV_TOKEN is required}"

mkdir -p "$ROOT/.gate"
PACK_DIR="$ROOT/templates/packs/med_dispatch"
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
    echo "med_dispatch.sh: HTTP ${code} on ${m} ${p}: ${resp}" >&2
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
    echo "med_dispatch.sh: HTTP ${code} on GET ${p}: ${resp}" >&2
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
        if cur.description is not None:
            row = cur.fetchone()
            if row is not None:
                print(row[0])
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

# 0. 严格真数据校验的幂等快路径（SHOWCASE_FORCE_REBUILD=1 时强制全量重建）
if [[ "${SHOWCASE_FORCE_REBUILD:-0}" != "1" ]]; then
  api_try_get /api/xblueview/dashboards/by-slug/med-dispatch-cockpit > "$SEED_TMP/fp_dash.json" || echo '{}' > "$SEED_TMP/fp_dash.json"
  if jq -e '.status == "PUBLISHED"' "$SEED_TMP/fp_dash.json" >/dev/null 2>&1; then
  FP_PIDS=()
  api_ok GET /api/axops/pulse > "$SEED_TMP/fp_pulse.json" & FP_PIDS+=("$!")
  api_ok GET "/api/xblue/schema?subgraph=main" > "$SEED_TMP/fp_main.json" & FP_PIDS+=("$!")
  api_ok GET /api/axsim/sandboxes > "$SEED_TMP/fp_sb.json" & FP_PIDS+=("$!")
  api_ok GET "/api/axhub/datasets?limit=200" > "$SEED_TMP/fp_ds.json" & FP_PIDS+=("$!")
  api_ok GET "/api/xblue/nodes?subgraph=main&type_name=ClinicalDept&limit=20" > "$SEED_TMP/fp_dept.json" & FP_PIDS+=("$!")
  if wait_pids "${FP_PIDS[@]}" 2>/dev/null; then
    if jq -ne \
      --slurpfile pulse "$SEED_TMP/fp_pulse.json" \
      --slurpfile main "$SEED_TMP/fp_main.json" \
      --slurpfile dash "$SEED_TMP/fp_dash.json" \
      --slurpfile sb "$SEED_TMP/fp_sb.json" \
      --slurpfile ds "$SEED_TMP/fp_ds.json" \
      --slurpfile dept "$SEED_TMP/fp_dept.json" \
      --slurpfile pack "$PACK_DIR/schema.json" '
      ($pulse[0].subgraphs | map(select(.slug == "med_dispatch"))[0]) as $md
      | ($main[0].node_types | map(.name)) as $mn
      | ($main[0].edge_types | map(.name)) as $me
      | ((if ($ds[0] | type) == "array" then $ds[0] else ($ds[0].items // []) end) | map(select(.name == "med_triage_events"))[0]) as $mds
      | ((if ($sb[0] | type) == "array" then $sb[0] else ($sb[0].items // []) end) | map(select(.name == "冬季急诊重症高峰床位挤兑沙盘"))[0]) as $msb
      | (($dept[0].items // []) | map(select(.node_key == "DEPT-01"))[0]) as $dept01
      | ($md != null and $md.cert_level == "L4_CERTIFIED" and $md.node_count >= 260 and $md.link_count >= 500)
      and ($mds != null and ($mds.row_count // 0) >= 18000)
      and ($msb != null and ($msb.id // "") != "")
      and ($dept01 != null and ($dept01.id // "") != "")
      and ($dash[0].status == "PUBLISHED")
      and (([ $dash[0].widgets[]? | select(.widget_kind == "KPI_CARD" and (.encoding.y // "") != "value") ] | length) == 0)
      and ([ $pack[0].node_types[].name ] | all(. as $n | ($mn | index($n)) != null))
      and ([ $pack[0].edge_types[].name ] | all(. as $e | ($me | index($e)) != null))
      and ([ $main[0].edge_types[] | ((.src_type_name as $s | $mn | index($s)) != null and (.dst_type_name as $d | $mn | index($d)) != null) ] | all)
    ' >/dev/null 2>&1; then
      MED_ROLLUP_OK=$(exec_tenant_sql "$SHOWCASE_TENANT_ID" "
SELECT CASE
         WHEN COUNT(DISTINCT mr.grain) = 2
          AND COUNT(DISTINCT CASE WHEN n.type_name = 'HospitalCampus' THEN n.id END) >= 12
         THEN 1 ELSE 0
       END
  FROM xb_metric_rollups mr
  JOIN xb_nodes n ON n.id = mr.node_id AND n.tenant_id = mr.tenant_id
 WHERE mr.tenant_id = '${SHOWCASE_TENANT_ID}'
   AND n.subgraph_slug = 'med_dispatch'
   AND mr.grain IN ('MINUTE', 'HOUR')" | head -n 1 || echo "0")
      if [[ "${MED_ROLLUP_OK:-0}" == "1" ]]; then
        exec_tenant_sql "$SHOWCASE_TENANT_ID" "
UPDATE xb_nodes
   SET metrics = jsonb_set(jsonb_set(metrics, '{total_free_beds}', '38.0'::jsonb, true), '{surge_level}', '1.0'::jsonb, true),
       updated_at = NOW()
 WHERE tenant_id = '${SHOWCASE_TENANT_ID}'
   AND type_name = 'HospitalCampus'
   AND node_key = 'HOSP-01';
" >/dev/null
        FP_SB_ID=$(jq -r '(if type == "array" then . else (.items // []) end) | map(select(.name == "冬季急诊重症高峰床位挤兑沙盘"))[0].id' "$SEED_TMP/fp_sb.json")
        FP_DASH_ID=$(jq -r '.id' "$SEED_TMP/fp_dash.json")
        FP_DEPT01_ID=$(jq -r '(.items // []) | map(select(.node_key == "DEPT-01"))[0].id' "$SEED_TMP/fp_dept.json")
        if [[ "$SHOWCASE_TENANT_ID" == "default" ]]; then
          upsert_ids_env "MED_DISPATCH_SANDBOX_ID" "$FP_SB_ID"
          upsert_ids_env "MED_DISPATCH_DASHBOARD_ID" "$FP_DASH_ID"
          upsert_ids_env "MED_DISPATCH_DEPT01_ID" "$FP_DEPT01_ID"
        fi
        echo "MED_DISPATCH_SANDBOX_ID=${FP_SB_ID}"
        echo "MED_DISPATCH_DASHBOARD_ID=${FP_DASH_ID}"
        echo "MED_DISPATCH_DEPT01_ID=${FP_DEPT01_ID}"
        exit 0
      fi
    fi
  fi
  fi
fi

# 1. 创建或更新生产业务子图 med_dispatch 并发布三域三层 Schema
SUBGRAPHS_JSON=$(api_ok GET /api/xblue/subgraphs)
if ! printf '%s' "$SUBGRAPHS_JSON" | jq -e '(if type == "array" then . else (.items // []) end) | map(select(.slug == "med_dispatch")) | length > 0' >/dev/null 2>&1; then
  api_ok POST /api/xblue/subgraphs '{
    "slug": "med_dispatch",
    "display_name": "城市急诊床位调度",
    "description": "院区统筹、临床救治与120院前急救空间轨迹凸包调度模型",
    "owner_dept": "default",
    "is_dev_sandbox": false,
    "source_template_id": "med_dispatch"
  }' >/dev/null
fi

MED_SCHEMA_RAW=$(cat "$PACK_DIR/schema.json")
S1_PIDS=()
api_ok POST "/api/xblue/schema?subgraph=med_dispatch" "$(printf '%s' "$MED_SCHEMA_RAW" | jq -c '. + {force: true, change_summary: "seed med_dispatch v4 showcase"}')" >/dev/null &
S1_PIDS+=("$!")
api_ok PUT /api/xblue/subgraphs/med_dispatch/competency-suite "$(cat "$PACK_DIR/competency.json")" >/dev/null &
S1_PIDS+=("$!")

# 同步成对合并对象与关系类型至企业主图（同时自动剥离 main 中任何历史无源/宿孤立边）
(
  CUR_MAIN=$(api_ok GET "/api/xblue/schema?subgraph=main")
  MED_NAMES='["AmbulanceUnit","HospitalCampus","WardCapacityCube","ClinicalDept","LifeSupportUnit","TriageVisit"]'
  MED_EDGES='["TRANSPORTS_TO_CAMPUS","HAS_CLINICAL_DEPT","OPERATES_DEVICE","RECORDS_TRIAGE","SUMMARIZES_WARD"]'
  MERGED_MAIN=$(jq -nc \
    --argjson cur "$CUR_MAIN" \
    --argjson med "$MED_SCHEMA_RAW" \
    --argjson nnames "$MED_NAMES" \
    --argjson enames "$MED_EDGES" '
    (([ $cur.node_types[] | select((.name as $n | $nnames | index($n)) == null) ]) + $med.node_types) as $all_nodes
    | ($all_nodes | map(.name)) as $valid_nnames
    | {
        node_types: $all_nodes,
        edge_types: (
          ([ $cur.edge_types[]
             | select((.name as $e | $enames | index($e)) == null)
             | select((.src_type_name as $s | $valid_nnames | index($s)) != null and (.dst_type_name as $d | $valid_nnames | index($d)) != null)
           ]) + $med.edge_types
        ),
        mappings: ($cur.mappings // []),
        force: true,
        change_summary: "merge med_dispatch types"
      }
  ')
  api_ok POST "/api/xblue/schema?subgraph=main" "$MERGED_MAIN" >/dev/null
) &
S1_PIDS+=("$!")
wait_pids "${S1_PIDS[@]}"

# 2. 向量化写入 18,000 行 DuckLake 分诊表、264 节点（含 64 辆带坐标急救车）、508 边与 20,000 时序点
SEED_OUT=$(PYTHONPATH="$ROOT/axpoint-parser" "$PY_BIN" "$ROOT/axpoint-parser/scripts/gen_worlds_semi_med.py" --world med_dispatch --tenant "$SHOWCASE_TENANT_ID")
read -r MED_DS_ID HOSP01_ID DEPT01_ID AMB01_ID < <(
  printf '%s' "$SEED_OUT" | jq -r '[.dataset_id, .main_anchors.hosp01_id, .main_anchors.dept01_id, .main_anchors.amb01_id] | @tsv'
)

api_ok PATCH "/api/axhub/datasets/$MED_DS_ID" '{
  "description": "全市三甲院区急诊预检分诊、MEWS危重评分与收治去向18,000行湖表明细"
}' >/dev/null

# 3 & 4. 并行预置 AxSim 急诊重症挤兑沙盘与发布五区急诊态势看板 med-dispatch-cockpit
(
  SB_NAME="冬季急诊重症高峰床位挤兑沙盘"
  MED_SB_ID=$(api_ok GET /api/axsim/sandboxes | jq -r --arg n "$SB_NAME" '(if type == "array" then . else (.items // []) end)[] | select(.name == $n) | .id' | head -n 1 || true)
  if [[ -z "$MED_SB_ID" || "$MED_SB_ID" == "null" ]]; then
    MED_SB_ID=$(api_ok POST /api/axsim/sandboxes "$(jq -nc --arg n "$SB_NAME" '{
      name: $n,
      scope: {
        type_names: ["AmbulanceUnit", "HospitalCampus", "ClinicalDept", "LifeSupportUnit"],
        link_types: ["TRANSPORTS_TO_CAMPUS", "HAS_CLINICAL_DEPT", "OPERATES_DEVICE"]
      }
    }')" | jq -r '.id')
  else
    api_ok POST "/api/axsim/sandboxes/${MED_SB_ID}/refresh-base" '{}' >/dev/null
  fi

  SCEN_NAME="寒潮重症呼衰转运激增与机动加床推演"
  EXISTING_SCEN=$(api_ok GET "/api/axsim/sandboxes/${MED_SB_ID}/scenarios" | jq -r --arg s "$SCEN_NAME" 'if type == "array" then (.[] | select(.name == $s) | .id) else (.items[]? | select(.name == $s) | .id) end' | head -n 1 || true)
  if [[ -n "$EXISTING_SCEN" && "$EXISTING_SCEN" != "null" && "${SHOWCASE_FORCE_REBUILD:-0}" == "1" ]]; then
    api_ok DELETE "/api/axsim/scenarios/${EXISTING_SCEN}" >/dev/null
    EXISTING_SCEN=""
  fi
  if [[ -z "$EXISTING_SCEN" || "$EXISTING_SCEN" == "null" ]]; then
    api_ok POST "/api/axsim/sandboxes/${MED_SB_ID}/scenarios" "$(jq -nc \
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
  printf '%s' "$MED_SB_ID" > "$SEED_TMP/sb_id"
) &
SB_PID=$!

(
  EXISTING_DASH=$(api_try_get /api/xblueview/dashboards/by-slug/med-dispatch-cockpit)
  MED_DASH_ID=$(printf '%s' "$EXISTING_DASH" | jq -r 'if type == "object" then (.id // empty) else empty end' 2>/dev/null || true)
  MED_DASH_STATUS=$(printf '%s' "$EXISTING_DASH" | jq -r 'if type == "object" then (.status // empty) else empty end' 2>/dev/null || true)
  MED_LEGACY_KPI=$(printf '%s' "$EXISTING_DASH" | jq -r '([.widgets[]? | select(.widget_kind == "KPI_CARD" and (.encoding.y // "") != "value")] | length) > 0' 2>/dev/null || echo "false")
  if [[ -z "$MED_DASH_ID" || "$MED_DASH_ID" == "null" || "$MED_DASH_STATUS" != "PUBLISHED" || "$MED_LEGACY_KPI" == "true" || "${SHOWCASE_FORCE_REBUILD:-0}" == "1" ]]; then
    if [[ -n "$MED_DASH_ID" && "$MED_DASH_ID" != "null" ]]; then
      api_ok DELETE "/api/xblueview/dashboards/${MED_DASH_ID}" >/dev/null
    fi
    COCKPIT_DEF=$(cat "$PACK_DIR/dashboards/cockpit.json")
    MED_DASH_ID=$(api_ok POST /api/xblueview/dashboards "$(printf '%s' "$COCKPIT_DEF" | jq -c '{
      name: .name,
      slug: .slug,
      description: .description,
      theme: .theme,
      zone_layout: "FREE_GRID",
      canvas: (.canvas | .zone_layout = "FREE_GRID")
    }')" | jq -r '.id')

    ADD_PIDS=()
    while IFS= read -r W_BODY; do
      api_ok POST "/api/xblueview/dashboards/${MED_DASH_ID}/widgets" "$W_BODY" >/dev/null &
      ADD_PIDS+=("$!")
    done < <(printf '%s' "$COCKPIT_DEF" | jq -c --arg hosp_id "$HOSP01_ID" '
      (.widgets
       | .[5].options.show_convex_hull = true
       | .[9] = {
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
        })[]
    ')
    if [[ ${#ADD_PIDS[@]} -gt 0 ]]; then wait_pids "${ADD_PIDS[@]}"; fi
    api_ok PUT "/api/xblueview/dashboards/${MED_DASH_ID}" "$(printf '%s' "$COCKPIT_DEF" | jq -c '{canvas: .canvas}')" >/dev/null
    api_ok POST "/api/xblueview/dashboards/${MED_DASH_ID}/publish" '{}' >/dev/null
  fi
  printf '%s' "$MED_DASH_ID" > "$SEED_TMP/dash_id"
) &
DASH_PID=$!

wait_pids "$SB_PID" "$DASH_PID"

MED_SB_ID=$(cat "$SEED_TMP/sb_id")
MED_DASH_ID=$(cat "$SEED_TMP/dash_id")

VERIFY_DASH_ST=$(api_ok GET /api/xblueview/dashboards/by-slug/med-dispatch-cockpit | jq -r '.status // empty')
if [[ "$VERIFY_DASH_ST" != "PUBLISHED" ]]; then
  echo "med_dispatch.sh: expected med-dispatch-cockpit PUBLISHED, got: $VERIFY_DASH_ST" >&2
  exit 1
fi

if [[ "$SHOWCASE_TENANT_ID" == "default" ]]; then
  upsert_ids_env "MED_DISPATCH_SANDBOX_ID" "$MED_SB_ID"
  upsert_ids_env "MED_DISPATCH_DASHBOARD_ID" "$MED_DASH_ID"
  upsert_ids_env "MED_DISPATCH_DEPT01_ID" "$DEPT01_ID"
fi

# 5. 执行子图运行态 L4 认证落库
CERT_MED=$(api_ok POST /api/xblue/subgraphs/med_dispatch/certify '{}')
CERT_LEVEL=$(printf '%s' "$CERT_MED" | jq -r '.cert_level')
if [[ "$CERT_LEVEL" != "L4_CERTIFIED" ]]; then
  echo "med_dispatch certification did not reach L4_CERTIFIED: $CERT_MED" >&2
  exit 1
fi

echo "MED_DISPATCH_SANDBOX_ID=${MED_SB_ID}"
echo "MED_DISPATCH_DASHBOARD_ID=${MED_DASH_ID}"
echo "MED_DISPATCH_DEPT01_ID=${DEPT01_ID}"
