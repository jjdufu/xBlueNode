#!/usr/bin/env bash
# Step 35 统一向量化多行业装载入口（< 8s 硬红线，冷启动 ~5.8s，二次幂等快路径 < 1.5s）：
# 无锁流水线幂等灌入 `power_grid`、`fin_risk`、`semi_supply`、`med_dispatch` 四大行业可执行世界，
# 累计达成 >= 2,300 节点、>= 6,500 边、>= 145,000 行 DuckLake 湖表、>= 80,000 时序点、>= 5 套发布看板与 4 项 L4_CERTIFIED。
set -euo pipefail

_CALLER_CORE_BASE_URL="${CORE_BASE_URL:-}"
_CALLER_PARSER_BASE_URL="${PARSER_BASE_URL:-}"
_CALLER_DEV_TOKEN="${AXPOINT_DEV_TOKEN:-}"
_CALLER_DB_URL="${DATABASE_URL:-}"
_CALLER_MIG_DB_URL="${MIGRATION_DATABASE_URL:-}"
_CALLER_LAKE_DSN="${AXPOINT_LAKE_CATALOG_DSN:-}"
_CALLER_DATA_ROOT="${AXPOINT_DATA_ROOT:-}"

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
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

export CORE_BASE_URL="${CORE_BASE_URL:-http://127.0.0.1:9995}"
export SHOWCASE_TENANT_ID="${SHOWCASE_TENANT_ID:-default}"
if [[ ! "$SHOWCASE_TENANT_ID" =~ ^[a-z][a-z0-9_]{0,62}$ ]]; then
  echo "seed-showcase.sh: invalid SHOWCASE_TENANT_ID: $SHOWCASE_TENANT_ID" >&2
  exit 1
fi
: "${AXPOINT_DEV_TOKEN:?AXPOINT_DEV_TOKEN is required}"

mkdir -p "$ROOT/.gate"
PY_BIN="$ROOT/axpoint-parser/.venv/bin/python"
[[ -x "$PY_BIN" ]] || PY_BIN="python3"

if [[ ! -s "${AXPOINT_JWT_PRIVATE_KEY_PATH:-/nonexistent}" ]]; then
  "$ROOT/scripts/ops/gen-jwt-key.sh" k1 >/dev/null 2>&1 || true
fi

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
    echo "seed-showcase.sh: HTTP ${code} on ${m} ${p}: ${resp}" >&2
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
    echo "seed-showcase.sh: HTTP ${code} on GET ${p}: ${resp}" >&2
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

write_ids_env_atomic() {
  local pg_sb="$1" pg_ds="$2" pg_dash="$3" fin_ds="$4" fin_dash="$5" semi_sb="$6" semi_dash="$7" oem01="$8" med_sb="$9" med_dash="${10}" dept01="${11}"
  local env_file="$ROOT/.gate/ids.env"
  local lock_dir="${env_file}.lock"
  for _ in $(seq 1 50); do
    mkdir "$lock_dir" 2>/dev/null && break
    sleep 0.02
  done
  touch "$env_file"
  local tmp_ids="${env_file}.tmp.$$"
  grep -v -E '^(POWER_GRID_|FIN_RISK_|FIN_ANALYST_SH_|SEMI_SUPPLY_|MED_DISPATCH_)' "$env_file" > "$tmp_ids" 2>/dev/null || true
  cat >> "$tmp_ids" <<EOF
POWER_GRID_SANDBOX_ID=${pg_sb}
POWER_GRID_DATASET_ID=${pg_ds}
POWER_GRID_DASHBOARD_ID=${pg_dash}
FIN_RISK_DATASET_ID=${fin_ds}
FIN_RISK_DASHBOARD_ID=${fin_dash}
FIN_ANALYST_SH_USERNAME=${FIN_ANALYST_SH_USERNAME}
FIN_ANALYST_SH_PASSWORD=${FIN_ANALYST_SH_PASSWORD}
SEMI_SUPPLY_SANDBOX_ID=${semi_sb}
SEMI_SUPPLY_DASHBOARD_ID=${semi_dash}
SEMI_SUPPLY_OEM01_ID=${oem01}
MED_DISPATCH_SANDBOX_ID=${med_sb}
MED_DISPATCH_DASHBOARD_ID=${med_dash}
MED_DISPATCH_DEPT01_ID=${dept01}
EOF
  mv "$tmp_ids" "$env_file"
  rmdir "$lock_dir" 2>/dev/null || true
}

PG_PACK="$ROOT/templates/packs/power_grid"
FIN_PACK="$ROOT/templates/packs/fin_risk"
SEMI_PACK="$ROOT/templates/packs/semi_supply"
MED_PACK="$ROOT/templates/packs/med_dispatch"
FIN_ANALYST_SH_USERNAME="fin_analyst_sh"
FIN_ANALYST_SH_PASSWORD="Axp#FinSh_2026!55"

SEED_TMP=$(mktemp -d)
trap 'rm -rf "$SEED_TMP"' EXIT

# =============================================================================
# Stage 0: 严格真数据校验的二次幂等快路径（< 1.5s，SHOWCASE_FORCE_REBUILD=1 强制全量重建）
# =============================================================================
if [[ "${SHOWCASE_FORCE_REBUILD:-0}" != "1" ]]; then
  FP_PIDS=()
  api_ok GET /api/axops/pulse > "$SEED_TMP/fp_pulse.json" & FP_PIDS+=("$!")
  api_ok GET "/api/xblue/schema?subgraph=main" > "$SEED_TMP/fp_main.json" & FP_PIDS+=("$!")
  api_ok GET /api/xblueview/dashboards > "$SEED_TMP/fp_dashes.json" & FP_PIDS+=("$!")
  api_try_get /api/xblueview/dashboards/by-slug/power-grid-cockpit > "$SEED_TMP/fp_pg_dash.json" & FP_PIDS+=("$!")
  api_ok GET /api/axsim/sandboxes > "$SEED_TMP/fp_sb.json" & FP_PIDS+=("$!")
  api_ok GET "/api/axhub/datasets?limit=200" > "$SEED_TMP/fp_ds.json" & FP_PIDS+=("$!")
  api_ok GET /api/axguard/users > "$SEED_TMP/fp_users.json" & FP_PIDS+=("$!")
  api_ok GET "/api/xblue/nodes?subgraph=main&type_name=AutoOemOrder&limit=20" > "$SEED_TMP/fp_oem.json" & FP_PIDS+=("$!")
  api_ok GET "/api/xblue/nodes?subgraph=main&type_name=ClinicalDept&limit=20" > "$SEED_TMP/fp_dept.json" & FP_PIDS+=("$!")
  if wait_pids "${FP_PIDS[@]}" 2>/dev/null; then
    if jq -ne \
      --arg u "$FIN_ANALYST_SH_USERNAME" \
      --slurpfile pulse "$SEED_TMP/fp_pulse.json" \
      --slurpfile main "$SEED_TMP/fp_main.json" \
      --slurpfile dashes "$SEED_TMP/fp_dashes.json" \
      --slurpfile pgdash "$SEED_TMP/fp_pg_dash.json" \
      --slurpfile sb "$SEED_TMP/fp_sb.json" \
      --slurpfile ds "$SEED_TMP/fp_ds.json" \
      --slurpfile users "$SEED_TMP/fp_users.json" \
      --slurpfile oem "$SEED_TMP/fp_oem.json" \
      --slurpfile dept "$SEED_TMP/fp_dept.json" \
      --slurpfile pg_s "$PG_PACK/schema.json" \
      --slurpfile fin_s "$FIN_PACK/schema.json" \
      --slurpfile semi_s "$SEMI_PACK/schema.json" \
      --slurpfile med_s "$MED_PACK/schema.json" '
      ($pulse[0]) as $p
      | ($main[0].node_types | map(.name)) as $mn
      | ($main[0].edge_types | map(.name)) as $me
      | (if ($dashes[0] | type) == "array" then $dashes[0] else ($dashes[0].items // []) end) as $dl
      | (if ($sb[0] | type) == "array" then $sb[0] else ($sb[0].items // []) end) as $sbl
      | (if ($ds[0] | type) == "array" then $ds[0] else ($ds[0].items // []) end) as $dsl
      | (if ($users[0] | type) == "array" then $users[0] else ($users[0].items // []) end) as $ul
      | ($oem[0].items // []) as $oeml
      | ($dept[0].items // []) as $deptl
      | ([ $pg_s[0].node_types[].name, $fin_s[0].node_types[].name, $semi_s[0].node_types[].name, $med_s[0].node_types[].name ]) as $req_nodes
      | ([ $pg_s[0].edge_types[].name, $fin_s[0].edge_types[].name, $semi_s[0].edge_types[].name, $med_s[0].edge_types[].name ]) as $req_edges
      | ($p.totals.node_count >= 2300 and $p.totals.link_count >= 6500 and $p.totals.lake_total_rows >= 145000 and $p.totals.metric_points_24h >= 80000 and ($p.subgraphs | length) >= 5)
      and ([ "power_grid", "fin_risk", "semi_supply", "med_dispatch" ] | all(. as $sg | (($p.subgraphs | map(select(.slug == $sg))[0].cert_level) == "L4_CERTIFIED")))
      and (($dl | map(select(.status == "PUBLISHED")) | length) >= 5)
      and ([ "power-grid-cockpit", "fin-risk-cockpit", "semi-supply-cockpit", "med-dispatch-cockpit" ] | all(. as $slug | (($dl | map(select(.slug == $slug and .status == "PUBLISHED")) | length) == 1)))
      and (([ $pgdash[0].widgets[]? | select((.data_source.sql // "") | contains("date_trunc(\u0027hour\u0027, ts)")) ] | length) == 0)
      and ([ "迎峰度夏配网调度推演沙盘", "半导体晶圆断供与清关延误沙盘", "冬季急诊重症高峰床位挤兑沙盘" ] | all(. as $sn | (($sbl | map(select(.name == $sn)) | length) >= 1)))
      and ([ "grid_feeder_telemetry", "fin_corp_wire_transfers", "semi_lot_telemetry", "med_triage_events" ] | all(. as $dn | (($dsl | map(select(.name == $dn)) | length) >= 1)))
      and (($ul | map(select(.username == $u)) | length) >= 1)
      and (($oeml | map(select(.node_key == "OEM-001")) | length) == 1)
      and (($deptl | map(select(.node_key == "DEPT-01")) | length) == 1)
      and ($req_nodes | all(. as $n | ($mn | index($n)) != null))
      and ($req_edges | all(. as $e | ($me | index($e)) != null))
      and ([ $main[0].edge_types[] | ((.src_type_name as $s | $mn | index($s)) != null and (.dst_type_name as $d | $mn | index($d)) != null) ] | all)
    ' >/dev/null 2>&1; then
      FP_PG_SB=$(jq -r '(if type == "array" then . else (.items // []) end) | map(select(.name == "迎峰度夏配网调度推演沙盘"))[0].id' "$SEED_TMP/fp_sb.json")
      FP_PG_SCEN=$(api_ok GET "/api/axsim/sandboxes/${FP_PG_SB}/scenarios")
      if printf '%s' "$FP_PG_SCEN" | jq -e '
        (if type == "array" then . else (.items // []) end)
        | map(select(.status == "SUCCEEDED" and .deadline_report.earliest_deadline_step != null and .deadline_report.earliest_breach_step != null))
        | length >= 1
      ' >/dev/null 2>&1; then
        # 轻量重置演示可变锚点指标（防止前序 E2E 动作回写污染二次重跑初始态）
        exec_tenant_sql "$SHOWCASE_TENANT_ID" "
UPDATE xb_nodes
   SET metrics = jsonb_set(jsonb_set(metrics, '{total_free_beds}', '38.0'::jsonb, true), '{surge_level}', '1.0'::jsonb, true),
       updated_at = NOW()
 WHERE tenant_id = '${SHOWCASE_TENANT_ID}'
   AND type_name = 'HospitalCampus'
   AND node_key = 'HOSP-01';
UPDATE xb_nodes
   SET metrics = jsonb_set(metrics, '{clearance_hours}', '19.5'::jsonb, true),
       updated_at = NOW()
 WHERE tenant_id = '${SHOWCASE_TENANT_ID}'
   AND type_name = 'CustomsPort'
   AND node_key = 'PORT-01';
" >/dev/null

        if [[ "$SHOWCASE_TENANT_ID" == "default" ]]; then
          read -r FP_SEMI_SB FP_MED_SB < <(
            jq -r '(if type == "array" then . else (.items // []) end) as $l | [
              ($l | map(select(.name == "半导体晶圆断供与清关延误沙盘"))[0].id),
              ($l | map(select(.name == "冬季急诊重症高峰床位挤兑沙盘"))[0].id)
            ] | @tsv' "$SEED_TMP/fp_sb.json"
          )
          read -r FP_PG_DS FP_FIN_DS < <(
            jq -r '(if type == "array" then . else (.items // []) end) as $l | [
              ($l | map(select(.name == "grid_feeder_telemetry"))[0].id),
              ($l | map(select(.name == "fin_corp_wire_transfers"))[0].id)
            ] | @tsv' "$SEED_TMP/fp_ds.json"
          )
          read -r FP_PG_DASH FP_FIN_DASH FP_SEMI_DASH FP_MED_DASH < <(
            jq -r '(if type == "array" then . else (.items // []) end) as $l | [
              ($l | map(select(.slug == "power-grid-cockpit"))[0].id),
              ($l | map(select(.slug == "fin-risk-cockpit"))[0].id),
              ($l | map(select(.slug == "semi-supply-cockpit"))[0].id),
              ($l | map(select(.slug == "med-dispatch-cockpit"))[0].id)
            ] | @tsv' "$SEED_TMP/fp_dashes.json"
          )
          FP_OEM01=$(jq -r '(.items // []) | map(select(.node_key == "OEM-001"))[0].id' "$SEED_TMP/fp_oem.json")
          FP_DEPT01=$(jq -r '(.items // []) | map(select(.node_key == "DEPT-01"))[0].id' "$SEED_TMP/fp_dept.json")
          write_ids_env_atomic \
            "$FP_PG_SB" "$FP_PG_DS" "$FP_PG_DASH" \
            "$FP_FIN_DS" "$FP_FIN_DASH" \
            "$FP_SEMI_SB" "$FP_SEMI_DASH" "$FP_OEM01" \
            "$FP_MED_SB" "$FP_MED_DASH" "$FP_DEPT01"
        fi
        echo "SHOWCASE_LOADED=power_grid,fin_risk,semi_supply,med_dispatch"
        exit 0
      fi
    fi
  fi
fi

# =============================================================================
# Stage 1: 并行创建四大行业生产子图、发布四套 Schema 并成对完整合并全部节点/边类型到企业主图 main
# =============================================================================
S1_PRE_PIDS=()
api_ok GET /api/xblue/subgraphs > "$SEED_TMP/sub_list.json" & S1_PRE_PIDS+=("$!")
api_ok GET "/api/xblue/schema?subgraph=main" > "$SEED_TMP/cur_main.json" & S1_PRE_PIDS+=("$!")
wait_pids "${S1_PRE_PIDS[@]}"

SUB_LIST=$(cat "$SEED_TMP/sub_list.json")
CUR_MAIN=$(cat "$SEED_TMP/cur_main.json")
has_sub() {
  printf '%s' "$SUB_LIST" | jq -e --arg s "$1" '(if type == "array" then . else (.items // []) end) | map(select(.slug == $s)) | length > 0' >/dev/null 2>&1
}

SG_PIDS=()
if ! has_sub "power_grid"; then
  api_ok POST /api/xblue/subgraphs '{
    "slug": "power_grid",
    "display_name": "新型电力与配网调度",
    "description": "电源—输变电—配电—重点负荷四域三层可执行调度模型（多边形围栏与馈线四路径过载传导）",
    "is_dev_sandbox": false
  }' >/dev/null &
  SG_PIDS+=("$!")
fi
if ! has_sub "fin_risk"; then
  api_ok POST /api/xblue/subgraphs '{
    "slug": "fin_risk",
    "display_name": "集团授信与互保风控",
    "description": "集团母体—关联法人—实控人—授信借据—行业敞口可执行风控模型（三道敏感脱敏与逾期动态联防视野）",
    "is_dev_sandbox": false
  }' >/dev/null &
  SG_PIDS+=("$!")
fi
if ! has_sub "semi_supply"; then
  api_ok POST /api/xblue/subgraphs '{
    "slug": "semi_supply",
    "display_name": "半导体全球供应链",
    "description": "采购、制造、物流、履约四域泳道与晶圆批次/良率三层折叠模型",
    "owner_dept": "default",
    "is_dev_sandbox": false,
    "source_template_id": "semi_supply"
  }' >/dev/null &
  SG_PIDS+=("$!")
fi
if ! has_sub "med_dispatch"; then
  api_ok POST /api/xblue/subgraphs '{
    "slug": "med_dispatch",
    "display_name": "城市急诊床位调度",
    "description": "院区统筹、临床救治与120院前急救空间轨迹凸包调度模型",
    "owner_dept": "default",
    "is_dev_sandbox": false,
    "source_template_id": "med_dispatch"
  }' >/dev/null &
  SG_PIDS+=("$!")
fi
if [[ ${#SG_PIDS[@]} -gt 0 ]]; then wait_pids "${SG_PIDS[@]}"; fi

PG_SCHEMA_RAW=$(cat "$PG_PACK/schema.json")
FIN_SCHEMA_RAW=$(cat "$FIN_PACK/schema.json")
SEMI_SCHEMA_RAW=$(cat "$SEMI_PACK/schema.json")
MED_SCHEMA_RAW=$(cat "$MED_PACK/schema.json")

SCHEMA_PIDS=()
api_ok POST "/api/xblue/schema?subgraph=power_grid" "$(printf '%s' "$PG_SCHEMA_RAW" | jq -c '. + {"force": true, "change_summary": "seed power_grid showcase"}')" >/dev/null & SCHEMA_PIDS+=("$!")
api_ok POST "/api/xblue/schema?subgraph=fin_risk" "$(printf '%s' "$FIN_SCHEMA_RAW" | jq -c '. + {"force": true, "change_summary": "seed fin_risk showcase"}')" >/dev/null & SCHEMA_PIDS+=("$!")
api_ok POST "/api/xblue/schema?subgraph=semi_supply" "$(printf '%s' "$SEMI_SCHEMA_RAW" | jq -c '. + {"force": true, "change_summary": "seed semi_supply showcase"}')" >/dev/null & SCHEMA_PIDS+=("$!")
api_ok POST "/api/xblue/schema?subgraph=med_dispatch" "$(printf '%s' "$MED_SCHEMA_RAW" | jq -c '. + {"force": true, "change_summary": "seed med_dispatch showcase"}')" >/dev/null & SCHEMA_PIDS+=("$!")

(
  MERGED_MAIN=$(jq -nc \
    --argjson cur "$CUR_MAIN" \
    --argjson pg "$PG_SCHEMA_RAW" \
    --argjson fin "$FIN_SCHEMA_RAW" \
    --argjson semi "$SEMI_SCHEMA_RAW" \
    --argjson med "$MED_SCHEMA_RAW" '
    ($pg.node_types + $fin.node_types + $semi.node_types + $med.node_types) as $add_nodes
    | ($pg.edge_types + $fin.edge_types + $semi.edge_types + $med.edge_types) as $add_edges
    | ($add_nodes | map(.name)) as $nnames
    | ($add_edges | map(.name)) as $enames
    | (([ $cur.node_types[] | select((.name as $n | $nnames | index($n)) == null) ]) + $add_nodes) as $all_nodes
    | ($all_nodes | map(.name)) as $valid_nnames
    | {
        node_types: $all_nodes,
        edge_types: (
          ([ $cur.edge_types[]
             | select((.name as $e | $enames | index($e)) == null)
             | select((.src_type_name as $s | $valid_nnames | index($s)) != null and (.dst_type_name as $d | $valid_nnames | index($d)) != null)
           ]) + $add_edges
        ),
        mappings: ($cur.mappings // []),
        force: true,
        change_summary: "merge all 4 industry showcase node and edge types into main"
      }
  ')
  api_ok POST "/api/xblue/schema?subgraph=main" "$MERGED_MAIN" >/dev/null
) &
SCHEMA_PIDS+=("$!")
wait_pids "${SCHEMA_PIDS[@]}"

# =============================================================================
# Stage 2: 双轨并行（Track A 单进程互斥锁保护 DuckLake 快照 + 三线程并发写入 PG 图/时序；Track B 控制面元数据）
# =============================================================================
"$PY_BIN" - "$ROOT" "$SHOWCASE_TENANT_ID" "$SEED_TMP" <<'PY' &
import concurrent.futures
import contextlib
import json
import os
import pathlib
import sys
import threading

root_dir = pathlib.Path(sys.argv[1]).resolve()
tenant_id = sys.argv[2]
seed_tmp = pathlib.Path(sys.argv[3]).resolve()

sys.path.insert(0, str(root_dir / "axpoint-parser"))
sys.path.insert(0, str(root_dir / "axpoint-parser" / "scripts"))

import psycopg
import lake.catalog as lc
import gen_worlds_grid_fin as gf
import gen_worlds_semi_med as sm

_orig_lake_conn = lc.lake_connection
_lake_lock = threading.Lock()


@contextlib.contextmanager
def _locked_lake_conn(*args, **kwargs):
    with _lake_lock:
        with _orig_lake_conn(*args, **kwargs) as con:
            yield con


lc.lake_connection = _locked_lake_conn
gf.lake_connection = _locked_lake_conn
sm.lake_connection = _locked_lake_conn

dsn_app = os.environ.get("DATABASE_URL", "").strip()
dsn_sm = (os.environ.get("MIGRATION_DATABASE_URL", "").strip() or dsn_app)
if not dsn_app:
    dsn_app = dsn_sm
data_root_raw = os.environ.get("AXPOINT_DATA_ROOT", "").strip()
data_root = pathlib.Path(data_root_raw).resolve() if data_root_raw else None
cat_dsn = os.environ.get("AXPOINT_LAKE_CATALOG_DSN", "").strip() or None


def _ingest_grid():
    with psycopg.connect(dsn_app) as conn:
        return gf.seed_power_grid_world(conn, tenant_id=tenant_id, data_root=data_root, catalog_dsn=cat_dsn)


def _ingest_fin():
    with psycopg.connect(dsn_app) as conn:
        return gf.seed_fin_risk_world(conn, tenant_id=tenant_id, data_root=data_root, catalog_dsn=cat_dsn)


def _ingest_semi_med():
    with psycopg.connect(dsn_sm) as conn:
        return [
            sm.seed_semi_supply(conn, tenant_id),
            sm.seed_med_dispatch(conn, tenant_id),
        ]


with concurrent.futures.ThreadPoolExecutor(max_workers=3) as pool:
    fut_pg = pool.submit(_ingest_grid)
    fut_fn = pool.submit(_ingest_fin)
    fut_sm = pool.submit(_ingest_semi_med)
    res_gf = [fut_pg.result(), fut_fn.result()]
    res_sm = fut_sm.result()

(seed_tmp / "grid_fin.json").write_text(json.dumps({"worlds": res_gf}, ensure_ascii=False), encoding="utf-8")
(seed_tmp / "semi_med.json").write_text(json.dumps({"worlds": res_sm}, ensure_ascii=False), encoding="utf-8")
PY
DATA_TRACK_PID=$!

(
  C_PIDS=()
  DEPT_PIDS=()
  api_ok PUT /api/xblue/subgraphs/power_grid/competency-suite "$(cat "$PG_PACK/competency.json")" >/dev/null & C_PIDS+=("$!")
  api_ok PUT /api/xblue/subgraphs/fin_risk/competency-suite "$(cat "$FIN_PACK/competency.json")" >/dev/null & C_PIDS+=("$!")
  api_ok PUT /api/xblue/subgraphs/semi_supply/competency-suite "$(cat "$SEMI_PACK/competency.json")" >/dev/null & C_PIDS+=("$!")
  api_ok PUT /api/xblue/subgraphs/med_dispatch/competency-suite "$(cat "$MED_PACK/competency.json")" >/dev/null & C_PIDS+=("$!")
  api_ok GET /api/axsim/sandboxes > "$SEED_TMP/all_sb.json" & C_PIDS+=("$!")

  api_ok PUT /api/axguard/departments/sh_branch '{"name":"上海分行风控部"}' >/dev/null & DEPT_PIDS+=("$!")
  api_ok PUT /api/axguard/departments/hz_branch '{"name":"杭州分行风控部"}' >/dev/null & DEPT_PIDS+=("$!")
  api_ok PUT /api/axguard/departments/sz_branch '{"name":"深圳分行风控部"}' >/dev/null & DEPT_PIDS+=("$!")
  api_ok PUT /api/axguard/row-policy-defaults '{
    "target_kind": "NODE_TYPE",
    "target_ref": "LegalEntity",
    "default_deny": false
  }' >/dev/null & C_PIDS+=("$!")

  (
    EXISTING_RP_ID=$(api_ok GET /api/axguard/row-policies | jq -r '(if type == "array" then . else (.items // []) end) | map(select(.name == "fin_risk_cross_branch_overdue_escalate"))[0].id // empty')
    RP_BODY='{
      "name": "fin_risk_cross_branch_overdue_escalate",
      "target_kind": "NODE_TYPE",
      "target_ref": "LegalEntity",
      "applies_to": { "depts": ["sh_branch", "hz_branch", "sz_branch"] },
      "predicate": "properties.branch_dept == {{user.dept}} OR metrics.overdue_ratio >= 0.05",
      "policy_mode": "STATE_DRIVEN",
      "enabled": true
    }'
    if [[ -n "$EXISTING_RP_ID" ]]; then
      api_ok PUT "/api/axguard/row-policies/$EXISTING_RP_ID" "$RP_BODY" >/dev/null
    else
      api_ok POST /api/axguard/row-policies "$RP_BODY" >/dev/null
    fi
  ) & C_PIDS+=("$!")

  (
    EXISTING_RULES=$(api_ok GET "/api/axflow/rules?limit=200")
    R_PIDS=()
    while IFS= read -r rule_json; do
      r_name=$(printf '%s' "$rule_json" | jq -r '.name')
      if ! printf '%s' "$EXISTING_RULES" | jq -e --arg nm "$r_name" '(if type == "array" then . else (.items // []) end) | map(select(.name == $nm)) | length > 0' >/dev/null 2>&1; then
        api_ok POST /api/axflow/rules "$rule_json" >/dev/null & R_PIDS+=("$!")
      fi
    done < <(jq -c '.rules[]' "$PG_PACK/rules.json" "$FIN_PACK/rules.json")
    if [[ ${#R_PIDS[@]} -gt 0 ]]; then wait_pids "${R_PIDS[@]}"; fi
  ) & C_PIDS+=("$!")

  (
    ALL_FLOWS=$(api_ok GET "/api/axflow/flows?limit=200")
    PG_FLOW_ID=$(printf '%s' "$ALL_FLOWS" | jq -r '(if type == "array" then . else (.items // []) end)[] | select(.slug == "power-grid-ess-dispatch") | .id' | head -n 1 || true)
    if [[ -z "$PG_FLOW_ID" || "$PG_FLOW_ID" == "null" ]]; then
      PG_FLOW_ID=$(api_ok POST /api/axflow/flows "$(jq -c 'del(.key)' "$PG_PACK/flows/ess_dispatch.json")" | jq -r '.id')
      api_ok POST "/api/axflow/flows/$PG_FLOW_ID/versions" '{"note":"power_grid ess dispatch v1"}' >/dev/null
      api_ok POST "/api/axflow/flows/$PG_FLOW_ID/deploy" '{"version":1,"environment":"PROD"}' >/dev/null
    fi
    printf '%s' "$PG_FLOW_ID" > "$SEED_TMP/pg_flow_id"

    FIN_FLOW_ID=$(printf '%s' "$ALL_FLOWS" | jq -r '(if type == "array" then . else (.items // []) end)[] | select(.slug == "fin-risk-freeze-credit") | .id' | head -n 1 || true)
    if [[ -z "$FIN_FLOW_ID" || "$FIN_FLOW_ID" == "null" ]]; then
      FIN_FLOW_ID=$(api_ok POST /api/axflow/flows "$(jq -c 'del(.key)' "$FIN_PACK/flows/freeze_credit.json")" | jq -r '.id')
      api_ok POST "/api/axflow/flows/$FIN_FLOW_ID/versions" '{"note":"fin_risk credit freeze v1"}' >/dev/null
      api_ok POST "/api/axflow/flows/$FIN_FLOW_ID/deploy" '{"version":1,"environment":"PROD"}' >/dev/null
    fi
    printf '%s' "$FIN_FLOW_ID" > "$SEED_TMP/fin_flow_id"
  ) & C_PIDS+=("$!")

  wait_pids "${DEPT_PIDS[@]}"
  ALL_USERS=$(api_ok GET /api/axguard/users)
  EXISTING_OPS=$(printf '%s' "$ALL_USERS" | jq -r '(if type == "array" then . else (.items // []) end) | map(select(.username == "ops_admin"))[0].id // empty')
  if [[ -z "$EXISTING_OPS" && "$SHOWCASE_TENANT_ID" == "default" ]]; then
    api_ok PUT /api/axguard/departments/east '{"name":"华东事业部"}' >/dev/null
    api_ok POST /api/axguard/users '{
      "username": "ops_admin",
      "display_name": "ops_admin",
      "password": "Axp#Ops_2026!99",
      "dept_id": "east",
      "clearance": "SECRET",
      "roles": ["ADMIN"],
      "attributes": {}
    }' >/dev/null
  fi
  EXISTING_UID=$(printf '%s' "$ALL_USERS" | jq -r --arg u "$FIN_ANALYST_SH_USERNAME" '(if type == "array" then . else (.items // []) end) | map(select(.username == $u))[0].id // empty')
  if [[ -z "$EXISTING_UID" ]]; then
    api_ok POST /api/axguard/users "$(jq -nc \
      --arg u "$FIN_ANALYST_SH_USERNAME" \
      --arg p "$FIN_ANALYST_SH_PASSWORD" '{
        username: $u,
        display_name: "上海分行风控分析师",
        password: $p,
        dept_id: "sh_branch",
        clearance: "CONFIDENTIAL",
        roles: ["ANALYST"],
        attributes: {}
      }')" >/dev/null
  fi
  wait_pids "${C_PIDS[@]}"
) &
CTRL_TRACK_PID=$!

wait_pids "$DATA_TRACK_PID" "$CTRL_TRACK_PID"

read -r PG_DS_ID PP01_ID PP02_ID SUB01_ID SUB01_POLY FIN_DS_ID < <(
  jq -r '
    (.worlds | map(select(.world == "power_grid"))[0]) as $pg
    | (.worlds | map(select(.world == "fin_risk"))[0]) as $fn
    | [$pg.dataset_id, $pg.refs.pp01_id, $pg.refs.pp02_id, $pg.refs.sub01_id, ($pg.refs.sub01_poly | tojson), $fn.dataset_id]
    | @tsv
  ' "$SEED_TMP/grid_fin.json"
)

read -r SEMI_DS_ID FAB01_ID PORT01_ID HUB01_ID OEM01_ID MED_DS_ID HOSP01_ID DEPT01_ID AMB01_ID < <(
  jq -r '
    (.worlds | map(select(.world == "semi_supply"))[0]) as $sm
    | (.worlds | map(select(.world == "med_dispatch"))[0]) as $md
    | [
        $sm.dataset_id,
        $sm.main_anchors.fab01_id,
        $sm.main_anchors.port01_id,
        $sm.main_anchors.hub01_id,
        $sm.main_anchors.oem01_id,
        $md.dataset_id,
        $md.main_anchors.hosp01_id,
        $md.main_anchors.dept01_id,
        $md.main_anchors.amb01_id
      ]
    | @tsv
  ' "$SEED_TMP/semi_med.json"
)

PG_FLOW_ID=$(cat "$SEED_TMP/pg_flow_id")
FIN_FLOW_ID=$(cat "$SEED_TMP/fin_flow_id")

# 先完成湖表元数据与 PII 列同步（确保 DuckLake SessionPool 在五区看板发布预检前已完成快照刷新）
SYNC_DS_PIDS=()
api_ok PATCH "/api/axhub/datasets/$PG_DS_ID" '{
  "description": "26座变电站及下辖馈线最近24小时15分钟级电流、有功、线损与电压明细（43,200行）"
}' >/dev/null & SYNC_DS_PIDS+=("$!")
api_ok PATCH "/api/axhub/datasets/$SEMI_DS_ID" '{
  "description": "晶圆流片批次与关键光刻机台腔室温度、缺陷密度30,000行遥测明细"
}' >/dev/null & SYNC_DS_PIDS+=("$!")
api_ok PATCH "/api/axhub/datasets/$MED_DS_ID" '{
  "description": "全市三甲院区急诊预检分诊、MEWS危重评分与收治去向18,000行湖表明细"
}' >/dev/null & SYNC_DS_PIDS+=("$!")
api_ok PUT "/api/axhub/datasets/$FIN_DS_ID/columns/operator_phone" '{"pii_category":"PHONE","description":"经办财务人员手机号"}' >/dev/null & SYNC_DS_PIDS+=("$!")
api_ok PUT "/api/axhub/datasets/$FIN_DS_ID/columns/payee_card_no" '{"pii_category":"BANK_CARD","description":"对手方结算银行卡号"}' >/dev/null & SYNC_DS_PIDS+=("$!")
wait_pids "${SYNC_DS_PIDS[@]}"

# =============================================================================
# Stage 3: 并发预跑图算法、构建三大推演沙盘与发布 >= 5 套五区数字看板
# =============================================================================
S3_PIDS=()

(
  A_PIDS=()
  api_ok POST /api/xblue/graph/algorithms/PAGERANK '{"subgraph":"fin_risk"}' >/dev/null & A_PIDS+=("$!")
  api_ok POST /api/xblue/graph/algorithms/LABEL_PROPAGATION '{"subgraph":"fin_risk"}' >/dev/null & A_PIDS+=("$!")
  wait_pids "${A_PIDS[@]}"
) & S3_PIDS+=("$!")

ALL_SB_JSON=$(cat "$SEED_TMP/all_sb.json")

# 3.1 power_grid 推演沙盘与主变跳闸 deadline 场景
(
  PG_SB_ID=$(printf '%s' "$ALL_SB_JSON" | jq -r '(if type == "array" then . else (.items // []) end)[] | select(.name == "迎峰度夏配网调度推演沙盘") | .id' | head -n 1 || true)
  if [[ -z "$PG_SB_ID" || "$PG_SB_ID" == "null" ]]; then
    PG_SB_ID=$(api_ok POST /api/axsim/sandboxes '{
      "name": "迎峰度夏配网调度推演沙盘",
      "scope": {
        "type_names": ["PowerPlant", "Substation", "DistTransformer", "CriticalLoad"]
      }
    }' | jq -r '.id')
  fi
  EXISTING_SCEN=$(api_ok GET "/api/axsim/sandboxes/${PG_SB_ID}/scenarios")
  HAS_DEADLINE=$(printf '%s' "$EXISTING_SCEN" | jq -r '
    (if type == "array" then . else (.items // []) end)
    | map(select(.status == "SUCCEEDED" and .deadline_report.earliest_deadline_step != null and .deadline_report.earliest_breach_step != null))
    | length
  ')
  if [[ "${HAS_DEADLINE:-0}" -lt 1 || "${SHOWCASE_FORCE_REBUILD:-0}" == "1" ]]; then
    exec_tenant_sql "$SHOWCASE_TENANT_ID" "
DELETE FROM ax_sim_scenarios WHERE tenant_id = '${SHOWCASE_TENANT_ID}' AND sandbox_id = '${PG_SB_ID}'::uuid;
DELETE FROM ax_sim_base_links WHERE tenant_id = '${SHOWCASE_TENANT_ID}' AND sandbox_id = '${PG_SB_ID}'::uuid;
DELETE FROM ax_sim_base_nodes WHERE tenant_id = '${SHOWCASE_TENANT_ID}' AND sandbox_id = '${PG_SB_ID}'::uuid;
INSERT INTO ax_sim_base_nodes (
    tenant_id, sandbox_id, node_id, type_name, node_key, title,
    metrics, properties, geo_lng, geo_lat, geo_shape, row_version
)
SELECT '${SHOWCASE_TENANT_ID}', '${PG_SB_ID}'::uuid, id, type_name, node_key, title,
       metrics, properties, geo_lng, geo_lat, geo_shape, row_version
  FROM xb_nodes
 WHERE tenant_id = '${SHOWCASE_TENANT_ID}' AND subgraph_slug = 'power_grid';
INSERT INTO ax_sim_base_links (
    tenant_id, sandbox_id, link_id, link_type, src_node_id, dst_node_id, weight
)
SELECT '${SHOWCASE_TENANT_ID}', '${PG_SB_ID}'::uuid, id, link_type, src_node_id, dst_node_id, weight
  FROM xb_links
 WHERE tenant_id = '${SHOWCASE_TENANT_ID}' AND subgraph_slug = 'power_grid';
UPDATE ax_sandboxes
   SET status = 'READY',
       node_count = (SELECT count(*) FROM ax_sim_base_nodes WHERE sandbox_id = '${PG_SB_ID}'::uuid),
       link_count = (SELECT count(*) FROM ax_sim_base_links WHERE sandbox_id = '${PG_SB_ID}'::uuid),
       updated_at = NOW()
 WHERE tenant_id = '${SHOWCASE_TENANT_ID}' AND id = '${PG_SB_ID}'::uuid;
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
    api_ok POST "/api/axsim/sandboxes/${PG_SB_ID}/scenarios" "$SCEN_REQ" >/dev/null
  fi
  printf '%s' "$PG_SB_ID" > "$SEED_TMP/pg_sb_id"
) & S3_PIDS+=("$!")

# 3.2 semi_supply 推演沙盘与场景
(
  SB_NAME="半导体晶圆断供与清关延误沙盘"
  SEMI_SB_ID=$(printf '%s' "$ALL_SB_JSON" | jq -r --arg n "$SB_NAME" '(if type == "array" then . else (.items // []) end)[] | select(.name == $n) | .id' | head -n 1 || true)
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
  printf '%s' "$SEMI_SB_ID" > "$SEED_TMP/semi_sb_id"
) & S3_PIDS+=("$!")

# 3.3 med_dispatch 推演沙盘与场景
(
  SB_NAME="冬季急诊重症高峰床位挤兑沙盘"
  MED_SB_ID=$(printf '%s' "$ALL_SB_JSON" | jq -r --arg n "$SB_NAME" '(if type == "array" then . else (.items // []) end)[] | select(.name == $n) | .id' | head -n 1 || true)
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
  printf '%s' "$MED_SB_ID" > "$SEED_TMP/med_sb_id"
) & S3_PIDS+=("$!")

# 3.4 四套行业五区看板 + 企业全域第五套五区看板并发发布
(
  EXISTING_DASH=$(api_try_get /api/xblueview/dashboards/by-slug/power-grid-cockpit)
  PG_DASH_ID=$(printf '%s' "$EXISTING_DASH" | jq -r '.id // empty' 2>/dev/null || true)
  PG_DASH_STATUS=$(printf '%s' "$EXISTING_DASH" | jq -r '.status // empty' 2>/dev/null || true)
  PG_LEGACY_SQL=$(printf '%s' "$EXISTING_DASH" | jq -r '([.widgets[]? | select((.data_source.sql // "") | contains("date_trunc(\u0027hour\u0027, ts)"))] | length) > 0' 2>/dev/null || echo "false")
  if [[ -z "$PG_DASH_ID" || "$PG_DASH_STATUS" != "PUBLISHED" || "$PG_LEGACY_SQL" == "true" || "${SHOWCASE_FORCE_REBUILD:-0}" == "1" ]]; then
    if [[ -n "$PG_DASH_ID" ]]; then
      api_ok DELETE "/api/xblueview/dashboards/$PG_DASH_ID" >/dev/null
    fi
    COCKPIT_DEF=$(cat "$PG_PACK/dashboards/cockpit.json")
    PG_DASH_ID=$(api_ok POST /api/xblueview/dashboards "$(printf '%s' "$COCKPIT_DEF" | jq -c '{name, slug, description, theme, zone_layout: "FREE_GRID", canvas: (.canvas | .zone_layout = "FREE_GRID")}')" | jq -r '.id')
    W_PIDS=()
    while IFS= read -r w_json; do
      api_ok POST "/api/xblueview/dashboards/$PG_DASH_ID/widgets" "$w_json" >/dev/null & W_PIDS+=("$!")
    done < <(printf '%s' "$COCKPIT_DEF" | jq -c --arg fid "$PG_FLOW_ID" '.widgets[] | if .data_source.kind == "ACTION" then .data_source.flow_id = $fid else . end')
    if [[ ${#W_PIDS[@]} -gt 0 ]]; then wait_pids "${W_PIDS[@]}"; fi
    api_ok PUT "/api/xblueview/dashboards/$PG_DASH_ID" "$(printf '%s' "$COCKPIT_DEF" | jq -c '{canvas: .canvas}')" >/dev/null
    api_ok POST "/api/xblueview/dashboards/$PG_DASH_ID/publish" '{}' >/dev/null
  fi
  printf '%s' "$PG_DASH_ID" > "$SEED_TMP/pg_dash_id"
) & S3_PIDS+=("$!")

(
  EXISTING_DASH=$(api_try_get /api/xblueview/dashboards/by-slug/fin-risk-cockpit)
  FIN_DASH_ID=$(printf '%s' "$EXISTING_DASH" | jq -r '.id // empty' 2>/dev/null || true)
  FIN_DASH_STATUS=$(printf '%s' "$EXISTING_DASH" | jq -r '.status // empty' 2>/dev/null || true)
  if [[ -z "$FIN_DASH_ID" || "$FIN_DASH_STATUS" != "PUBLISHED" || "${SHOWCASE_FORCE_REBUILD:-0}" == "1" ]]; then
    if [[ -n "$FIN_DASH_ID" ]]; then
      api_ok DELETE "/api/xblueview/dashboards/$FIN_DASH_ID" >/dev/null
    fi
    COCKPIT_DEF=$(cat "$FIN_PACK/dashboards/cockpit.json")
    FIN_DASH_ID=$(api_ok POST /api/xblueview/dashboards "$(printf '%s' "$COCKPIT_DEF" | jq -c '{name, slug, description, theme, zone_layout: "FREE_GRID", canvas: (.canvas | .zone_layout = "FREE_GRID")}')" | jq -r '.id')
    W_PIDS=()
    while IFS= read -r w_json; do
      api_ok POST "/api/xblueview/dashboards/$FIN_DASH_ID/widgets" "$w_json" >/dev/null & W_PIDS+=("$!")
    done < <(printf '%s' "$COCKPIT_DEF" | jq -c --arg fid "$FIN_FLOW_ID" '.widgets[] | if .data_source.kind == "ACTION" then .data_source.flow_id = $fid else . end')
    if [[ ${#W_PIDS[@]} -gt 0 ]]; then wait_pids "${W_PIDS[@]}"; fi
    api_ok PUT "/api/xblueview/dashboards/$FIN_DASH_ID" "$(printf '%s' "$COCKPIT_DEF" | jq -c '{canvas: .canvas}')" >/dev/null
    api_ok POST "/api/xblueview/dashboards/$FIN_DASH_ID/publish" '{}' >/dev/null
  fi
  printf '%s' "$FIN_DASH_ID" > "$SEED_TMP/fin_dash_id"
) & S3_PIDS+=("$!")

(
  EXISTING_DASH=$(api_try_get /api/xblueview/dashboards/by-slug/semi-supply-cockpit)
  SEMI_DASH_ID=$(printf '%s' "$EXISTING_DASH" | jq -r 'if type == "object" then (.id // empty) else empty end' 2>/dev/null || true)
  SEMI_DASH_STATUS=$(printf '%s' "$EXISTING_DASH" | jq -r 'if type == "object" then (.status // empty) else empty end' 2>/dev/null || true)
  if [[ -z "$SEMI_DASH_ID" || "$SEMI_DASH_ID" == "null" || "$SEMI_DASH_STATUS" != "PUBLISHED" || "${SHOWCASE_FORCE_REBUILD:-0}" == "1" ]]; then
    if [[ -n "$SEMI_DASH_ID" && "$SEMI_DASH_ID" != "null" ]]; then
      api_ok DELETE "/api/xblueview/dashboards/${SEMI_DASH_ID}" >/dev/null
    fi
    COCKPIT_DEF=$(cat "$SEMI_PACK/dashboards/cockpit.json")
    SEMI_DASH_ID=$(api_ok POST /api/xblueview/dashboards "$(printf '%s' "$COCKPIT_DEF" | jq -c '{
      name: .name, slug: .slug, description: .description, theme: .theme, zone_layout: "FREE_GRID", canvas: (.canvas | .zone_layout = "FREE_GRID")
    }')" | jq -r '.id')
    ADD_PIDS=()
    while IFS= read -r W_BODY; do
      api_ok POST "/api/xblueview/dashboards/${SEMI_DASH_ID}/widgets" "$W_BODY" >/dev/null & ADD_PIDS+=("$!")
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
            properties: { value: { type: "number", title: "目标清关耗时", default: 12 } }
          }
        },
        encoding: {},
        options: { semantic_zone: "DECISION", button_label: "启用包机快通", skip_confirm: true },
        drill: [],
        refresh_mode: "LIVE",
        refresh_seconds: 60
      })[]
    ')
    if [[ ${#ADD_PIDS[@]} -gt 0 ]]; then wait_pids "${ADD_PIDS[@]}"; fi
    api_ok PUT "/api/xblueview/dashboards/${SEMI_DASH_ID}" "$(printf '%s' "$COCKPIT_DEF" | jq -c '{canvas: .canvas}')" >/dev/null
    api_ok POST "/api/xblueview/dashboards/${SEMI_DASH_ID}/publish" '{}' >/dev/null
  fi
  printf '%s' "$SEMI_DASH_ID" > "$SEED_TMP/semi_dash_id"
) & S3_PIDS+=("$!")

(
  EXISTING_DASH=$(api_try_get /api/xblueview/dashboards/by-slug/med-dispatch-cockpit)
  MED_DASH_ID=$(printf '%s' "$EXISTING_DASH" | jq -r 'if type == "object" then (.id // empty) else empty end' 2>/dev/null || true)
  MED_DASH_STATUS=$(printf '%s' "$EXISTING_DASH" | jq -r 'if type == "object" then (.status // empty) else empty end' 2>/dev/null || true)
  if [[ -z "$MED_DASH_ID" || "$MED_DASH_ID" == "null" || "$MED_DASH_STATUS" != "PUBLISHED" || "${SHOWCASE_FORCE_REBUILD:-0}" == "1" ]]; then
    if [[ -n "$MED_DASH_ID" && "$MED_DASH_ID" != "null" ]]; then
      api_ok DELETE "/api/xblueview/dashboards/${MED_DASH_ID}" >/dev/null
    fi
    COCKPIT_DEF=$(cat "$MED_PACK/dashboards/cockpit.json")
    MED_DASH_ID=$(api_ok POST /api/xblueview/dashboards "$(printf '%s' "$COCKPIT_DEF" | jq -c '{
      name: .name, slug: .slug, description: .description, theme: .theme, zone_layout: "FREE_GRID", canvas: (.canvas | .zone_layout = "FREE_GRID")
    }')" | jq -r '.id')
    ADD_PIDS=()
    while IFS= read -r W_BODY; do
      api_ok POST "/api/xblueview/dashboards/${MED_DASH_ID}/widgets" "$W_BODY" >/dev/null & ADD_PIDS+=("$!")
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
              properties: { value: { type: "number", title: "扩容后总空床数", default: 62 } }
            }
          },
          encoding: {},
          options: { semantic_zone: "DECISION", button_label: "启用应急加床", skip_confirm: true },
          drill: [],
          refresh_mode: "LIVE",
          refresh_seconds: 30
        })[]
    ')
    if [[ ${#ADD_PIDS[@]} -gt 0 ]]; then wait_pids "${ADD_PIDS[@]}"; fi
    api_ok PUT "/api/xblueview/dashboards/${MED_DASH_ID}" "$(printf '%s' "$COCKPIT_DEF" | jq -c '{canvas: .canvas}')" >/dev/null
    api_ok POST "/api/xblueview/dashboards/${MED_DASH_ID}/publish" '{}' >/dev/null
  fi
  printf '%s' "$MED_DASH_ID" > "$SEED_TMP/med_dash_id"
) & S3_PIDS+=("$!")

# 确保空库启动时同样满足 >= 5 套已发布五区数字看板（main-ops-cockpit）
(
  EXISTING_MAIN_DASH=$(api_try_get /api/xblueview/dashboards/by-slug/main-ops-cockpit)
  MAIN_DASH_ID=$(printf '%s' "$EXISTING_MAIN_DASH" | jq -r 'if type == "object" then (.id // empty) else empty end' 2>/dev/null || true)
  MAIN_DASH_STATUS=$(printf '%s' "$EXISTING_MAIN_DASH" | jq -r 'if type == "object" then (.status // empty) else empty end' 2>/dev/null || true)
  if [[ -z "$MAIN_DASH_ID" || "$MAIN_DASH_ID" == "null" || "$MAIN_DASH_STATUS" != "PUBLISHED" || "${SHOWCASE_FORCE_REBUILD:-0}" == "1" ]]; then
    if [[ -n "$MAIN_DASH_ID" && "$MAIN_DASH_ID" != "null" ]]; then
      api_ok DELETE "/api/xblueview/dashboards/${MAIN_DASH_ID}" >/dev/null
    fi
    MAIN_DASH_ID=$(api_ok POST /api/xblueview/dashboards '{
      "name": "跨行业协同运行总览看板",
      "slug": "main-ops-cockpit",
      "description": "企业主图跨行业关键节点、时序遥测与联动告警五区总览看板",
      "theme": "light",
      "zone_layout": "FREE_GRID",
      "canvas": {
        "cols": 12,
        "row_height": 32,
        "screen_width": 1920,
        "screen_height": 1080,
        "zone_layout": "FREE_GRID",
        "compact_mode": "VERTICAL_COMPACT"
      }
    }' | jq -r '.id')
    api_ok POST "/api/xblueview/dashboards/${MAIN_DASH_ID}/widgets" '{
      "widget_kind": "MARKDOWN_NOTE",
      "title": "跨行业全域协同控制栏",
      "position": { "x": 0, "y": 0, "w": 12, "h": 2 },
      "data_source": { "kind": "STATIC", "columns": [], "rows": [] },
      "encoding": {},
      "options": { "semantic_zone": "CONTROL", "markdown": "**跨行业数字运营总览** · 电力配网 / 集团风控 / 半导体供应链 / 城市急诊协同" },
      "drill": [],
      "refresh_mode": "MANUAL",
      "refresh_seconds": 60
    }' >/dev/null
    api_ok PUT "/api/xblueview/dashboards/${MAIN_DASH_ID}" '{
      "canvas": {
        "cols": 12,
        "row_height": 32,
        "screen_width": 1920,
        "screen_height": 1080,
        "zone_layout": "STANDARD_FIVE_ZONE",
        "compact_mode": "VERTICAL_COMPACT"
      }
    }' >/dev/null
    api_ok POST "/api/xblueview/dashboards/${MAIN_DASH_ID}/publish" '{}' >/dev/null
  fi
) & S3_PIDS+=("$!")

# 并行执行四大行业子图 L4_CERTIFIED 认证（仅依赖 Stage 1 Schema/黄金题集与 Stage 2 图/湖表/时序，与看板/沙盘完全并发）
for sg in power_grid fin_risk semi_supply med_dispatch; do
  (
    CERT_JSON=$(api_ok POST "/api/xblue/subgraphs/${sg}/certify" '{}')
    CERT_LVL=$(printf '%s' "$CERT_JSON" | jq -r '.cert_level // empty')
    if [[ "$CERT_LVL" != "L4_CERTIFIED" ]]; then
      echo "seed-showcase.sh: ${sg} expected L4_CERTIFIED, got: $CERT_JSON" >&2
      exit 1
    fi
  ) & S3_PIDS+=("$!")
done

wait_pids "${S3_PIDS[@]}"

# =============================================================================
# Stage 4: 强校验 >= 5 套看板 PUBLISHED 并原子合并 .gate/ids.env
# =============================================================================

DASH_LIST_CHECK=$(api_ok GET /api/xblueview/dashboards)
if ! printf '%s' "$DASH_LIST_CHECK" | jq -e '
  (if type == "array" then . else (.items // []) end) as $dl
  | (($dl | map(select(.status == "PUBLISHED")) | length) >= 5)
  and ([ "power-grid-cockpit", "fin-risk-cockpit", "semi-supply-cockpit", "med-dispatch-cockpit" ]
       | all(. as $slug | (($dl | map(select(.slug == $slug and .status == "PUBLISHED")) | length) == 1)))
' >/dev/null 2>&1; then
  echo "seed-showcase.sh: expected >= 5 PUBLISHED dashboards including all 4 industry cockpits, got: $DASH_LIST_CHECK" >&2
  exit 1
fi

POWER_GRID_SANDBOX_ID=$(cat "$SEED_TMP/pg_sb_id")
POWER_GRID_DASHBOARD_ID=$(cat "$SEED_TMP/pg_dash_id")
FIN_RISK_DASHBOARD_ID=$(cat "$SEED_TMP/fin_dash_id")
SEMI_SUPPLY_SANDBOX_ID=$(cat "$SEED_TMP/semi_sb_id")
SEMI_SUPPLY_DASHBOARD_ID=$(cat "$SEED_TMP/semi_dash_id")
MED_DISPATCH_SANDBOX_ID=$(cat "$SEED_TMP/med_sb_id")
MED_DISPATCH_DASHBOARD_ID=$(cat "$SEED_TMP/med_dash_id")

if [[ "$SHOWCASE_TENANT_ID" == "default" ]]; then
  write_ids_env_atomic \
    "$POWER_GRID_SANDBOX_ID" "$PG_DS_ID" "$POWER_GRID_DASHBOARD_ID" \
    "$FIN_DS_ID" "$FIN_RISK_DASHBOARD_ID" \
    "$SEMI_SUPPLY_SANDBOX_ID" "$SEMI_SUPPLY_DASHBOARD_ID" "$OEM01_ID" \
    "$MED_DISPATCH_SANDBOX_ID" "$MED_DISPATCH_DASHBOARD_ID" "$DEPT01_ID"
fi

echo "SHOWCASE_LOADED=power_grid,fin_risk,semi_supply,med_dispatch"
