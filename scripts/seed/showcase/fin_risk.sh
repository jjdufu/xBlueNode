#!/usr/bin/env bash
# Step 33 Showcase Seed (2/2): `fin_risk` 集团授信与互保风控可执行世界幂等秒级装载 (< 3s)
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
  echo "fin_risk.sh: invalid SHOWCASE_TENANT_ID: $SHOWCASE_TENANT_ID" >&2
  exit 1
fi
: "${AXPOINT_DEV_TOKEN:?AXPOINT_DEV_TOKEN is required}"

mkdir -p "$ROOT/.gate"
PACK_DIR="$ROOT/templates/packs/fin_risk"
FIN_ANALYST_SH_USERNAME="fin_analyst_sh"
FIN_ANALYST_SH_PASSWORD="Axp#FinSh_2026!55"

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
    echo "fin_risk.sh: HTTP ${code} on ${m} ${p}: ${resp}" >&2
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
    echo "fin_risk.sh: HTTP ${code} on GET ${p}: ${resp}" >&2
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

if [[ ! -s "${AXPOINT_JWT_PRIVATE_KEY_PATH:-/nonexistent}" ]]; then
  "$ROOT/scripts/ops/gen-jwt-key.sh" k1 >/dev/null 2>&1 || true
fi

PY_BIN="$ROOT/axpoint-parser/.venv/bin/python"
[[ -x "$PY_BIN" ]] || PY_BIN="python3"

SEED_TMP=$(mktemp -d)
trap 'rm -rf "$SEED_TMP"' EXIT

# 0. 严格真数据校验的幂等快路径（SHOWCASE_FORCE_REBUILD=1 时强制全量重建）
if [[ "${SHOWCASE_FORCE_REBUILD:-0}" != "1" ]]; then
  FP_PIDS=()
  api_try_get /api/xblueview/dashboards/by-slug/fin-risk-cockpit > "$SEED_TMP/fp_dash.json" || echo '{}' > "$SEED_TMP/fp_dash.json"
  if jq -e '.status == "PUBLISHED"' "$SEED_TMP/fp_dash.json" >/dev/null 2>&1; then
    FP_PIDS=()
    api_ok GET /api/axops/pulse > "$SEED_TMP/fp_pulse.json" & FP_PIDS+=("$!")
    api_ok GET "/api/xblue/schema?subgraph=main" > "$SEED_TMP/fp_main.json" & FP_PIDS+=("$!")
    api_ok GET "/api/axhub/datasets?limit=200" > "$SEED_TMP/fp_ds.json" & FP_PIDS+=("$!")
    api_ok GET /api/axguard/users > "$SEED_TMP/fp_users.json" & FP_PIDS+=("$!")
    api_ok GET /api/axguard/row-policies > "$SEED_TMP/fp_rp.json" & FP_PIDS+=("$!")
    if wait_pids "${FP_PIDS[@]}" 2>/dev/null; then
      if jq -ne \
        --arg u "$FIN_ANALYST_SH_USERNAME" \
        --slurpfile pulse "$SEED_TMP/fp_pulse.json" \
        --slurpfile main "$SEED_TMP/fp_main.json" \
        --slurpfile dash "$SEED_TMP/fp_dash.json" \
        --slurpfile ds "$SEED_TMP/fp_ds.json" \
        --slurpfile users "$SEED_TMP/fp_users.json" \
        --slurpfile rp "$SEED_TMP/fp_rp.json" \
        --slurpfile pack "$PACK_DIR/schema.json" '
        ($pulse[0].subgraphs | map(select(.slug == "fin_risk"))[0]) as $fn
        | ($main[0].node_types | map(.name)) as $mn
        | ($main[0].edge_types | map(.name)) as $me
        | ((if ($ds[0] | type) == "array" then $ds[0] else ($ds[0].items // []) end) | map(select(.name == "fin_corp_wire_transfers"))[0]) as $fds
        | ((if ($users[0] | type) == "array" then $users[0] else ($users[0].items // []) end) | map(select(.username == $u))[0]) as $usr
        | ((if ($rp[0] | type) == "array" then $rp[0] else ($rp[0].items // []) end) | map(select(.name == "fin_risk_cross_branch_overdue_escalate" and .enabled == true))[0]) as $pol
        | ($fn != null and $fn.cert_level == "L4_CERTIFIED" and $fn.node_count == 1210 and $fn.link_count >= 4260)
        and ($fds != null and ($fds.row_count // 0) >= 60000)
        and ($usr != null)
        and ($pol != null)
        and ($dash[0].status == "PUBLISHED")
        and ([ $pack[0].node_types[].name ] | all(. as $n | ($mn | index($n)) != null))
        and ([ $pack[0].edge_types[].name ] | all(. as $e | ($me | index($e)) != null))
        and ([ $main[0].edge_types[] | ((.src_type_name as $s | $mn | index($s)) != null and (.dst_type_name as $d | $mn | index($d)) != null) ] | all)
      ' >/dev/null 2>&1; then
        FP_DS_ID=$(jq -r '(if type == "array" then . else (.items // []) end) | map(select(.name == "fin_corp_wire_transfers"))[0].id' "$SEED_TMP/fp_ds.json")
        FP_DASH_ID=$(jq -r '.id' "$SEED_TMP/fp_dash.json")
        if [[ "$SHOWCASE_TENANT_ID" == "default" ]]; then
          upsert_ids_env "FIN_RISK_DATASET_ID" "$FP_DS_ID"
          upsert_ids_env "FIN_RISK_DASHBOARD_ID" "$FP_DASH_ID"
          upsert_ids_env "FIN_ANALYST_SH_USERNAME" "$FIN_ANALYST_SH_USERNAME"
          upsert_ids_env "FIN_ANALYST_SH_PASSWORD" "$FIN_ANALYST_SH_PASSWORD"
        fi
        echo "FIN_RISK_DATASET_ID=$FP_DS_ID"
        exit 0
      fi
    fi
  fi
fi

# 1. 确保 fin_risk 生产子图存在
SUB_LIST=$(api_ok GET /api/xblue/subgraphs)
if ! printf '%s' "$SUB_LIST" | jq -e '(if type == "array" then . else (.items // []) end) | map(select(.slug == "fin_risk")) | length > 0' >/dev/null 2>&1; then
  api_ok POST /api/xblue/subgraphs '{
    "slug": "fin_risk",
    "display_name": "集团授信与互保风控",
    "description": "集团母体—关联法人—实控人—授信借据—行业敞口可执行风控模型（三道敏感脱敏与逾期动态联防视野）",
    "is_dev_sandbox": false
  }' >/dev/null
fi

# 2. 双轨并发：Track A 向量化节点/互保边/60,000行流水灌装；Track B 发布 Schema、成对合并 main、分行部门/风控账号/动态视野/规则/动作流
"$PY_BIN" "$ROOT/axpoint-parser/scripts/gen_worlds_grid_fin.py" --world fin_risk --tenant "$SHOWCASE_TENANT_ID" > "$SEED_TMP/gen.json" &
GEN_PID=$!

(
  FIN_SCHEMA_RAW=$(cat "$PACK_DIR/schema.json")
  S2_PIDS=()

  api_ok POST "/api/xblue/schema?subgraph=fin_risk" "$(printf '%s' "$FIN_SCHEMA_RAW" | jq -c '. + {"force": true, "change_summary": "publish fin_risk world schema"}')" >/dev/null &
  S2_PIDS+=("$!")

  api_ok PUT /api/xblue/subgraphs/fin_risk/competency-suite "$(cat "$PACK_DIR/competency.json")" >/dev/null &
  S2_PIDS+=("$!")

  (
    CUR_MAIN=$(api_ok GET "/api/xblue/schema?subgraph=main")
    if ! jq -ne --argjson cur "$CUR_MAIN" --argjson fin "$FIN_SCHEMA_RAW" '
      ($cur.node_types | map(.name)) as $mn
      | ($cur.edge_types | map(.name)) as $me
      | ([ $fin.node_types[].name ] | all(. as $n | ($mn | index($n)) != null))
      and ([ $fin.edge_types[].name ] | all(. as $e | ($me | index($e)) != null))
      and ([ $cur.edge_types[] | ((.src_type_name as $s | $mn | index($s)) != null and (.dst_type_name as $d | $mn | index($d)) != null) ] | all)
    ' >/dev/null 2>&1; then
      MERGED_MAIN=$(jq -nc \
        --argjson cur "$CUR_MAIN" \
        --argjson fin "$FIN_SCHEMA_RAW" '
        ([ $fin.node_types[].name ]) as $nnames
        | ([ $fin.edge_types[].name ]) as $enames
        | (([ $cur.node_types[] | select((.name as $n | $nnames | index($n)) == null) ]) + $fin.node_types) as $all_nodes
        | ($all_nodes | map(.name)) as $valid_nnames
        | {
            node_types: $all_nodes,
            edge_types: (
              ([ $cur.edge_types[]
                 | select((.name as $e | $enames | index($e)) == null)
                 | select((.src_type_name as $s | $valid_nnames | index($s)) != null and (.dst_type_name as $d | $valid_nnames | index($d)) != null)
               ]) + $fin.edge_types
            ),
            mappings: ($cur.mappings // []),
            force: true,
            change_summary: "merge fin_risk node and edge types into main"
          }
      ')
      api_ok POST "/api/xblue/schema?subgraph=main" "$MERGED_MAIN" >/dev/null
    fi
  ) &
  S2_PIDS+=("$!")

  DEPT_PIDS=()
  api_ok PUT /api/axguard/departments/sh_branch '{"name":"上海分行风控部"}' >/dev/null & DEPT_PIDS+=("$!")
  api_ok PUT /api/axguard/departments/hz_branch '{"name":"杭州分行风控部"}' >/dev/null & DEPT_PIDS+=("$!")
  api_ok PUT /api/axguard/departments/sz_branch '{"name":"深圳分行风控部"}' >/dev/null & DEPT_PIDS+=("$!")
  api_ok PUT /api/axguard/row-policy-defaults '{
    "target_kind": "NODE_TYPE",
    "target_ref": "LegalEntity",
    "default_deny": false
  }' >/dev/null &
  S2_PIDS+=("$!")

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
    FIN_FLOW_ID=$(api_ok GET "/api/axflow/flows?limit=200" | jq -r '(if type == "array" then . else (.items // []) end)[] | select(.slug == "fin-risk-freeze-credit") | .id' | head -n 1 || true)
    if [[ -z "$FIN_FLOW_ID" || "$FIN_FLOW_ID" == "null" ]]; then
      FLOW_BODY=$(jq -c 'del(.key)' "$PACK_DIR/flows/freeze_credit.json")
      FIN_FLOW_ID=$(api_ok POST /api/axflow/flows "$FLOW_BODY" | jq -r '.id')
      api_ok POST "/api/axflow/flows/$FIN_FLOW_ID/versions" '{"note":"fin_risk credit freeze v1"}' >/dev/null
      api_ok POST "/api/axflow/flows/$FIN_FLOW_ID/deploy" '{"version":1,"environment":"PROD"}' >/dev/null
    fi
    printf '%s' "$FIN_FLOW_ID" > "$SEED_TMP/flow_id"
  ) &
  S2_PIDS+=("$!")

  api_try_get /api/xblueview/dashboards/by-slug/fin-risk-cockpit > "$SEED_TMP/existing_dash.json" &
  S2_PIDS+=("$!")

  wait_pids "${DEPT_PIDS[@]}"
  EXISTING_UID=$(api_ok GET /api/axguard/users | jq -r --arg u "$FIN_ANALYST_SH_USERNAME" '(if type == "array" then . else (.items // []) end) | map(select(.username == $u))[0].id // empty')
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

  wait_pids "${S2_PIDS[@]}"
) &
SETUP_PID=$!

wait_pids "$GEN_PID" "$SETUP_PID"

FIN_DS_ID=$(jq -r '.dataset_id' "$SEED_TMP/gen.json")
FIN_FLOW_ID=$(cat "$SEED_TMP/flow_id")

# 3. 先同步完成湖表 PII 脱敏列配置（同时刷新 SessionPool DuckLake 快照缓存），再并行预跑图算法与发布五区看板
PII_PIDS=()
api_ok PUT "/api/axhub/datasets/$FIN_DS_ID/columns/operator_phone" '{"pii_category":"PHONE","description":"经办财务人员手机号"}' >/dev/null & PII_PIDS+=("$!")
api_ok PUT "/api/axhub/datasets/$FIN_DS_ID/columns/payee_card_no" '{"pii_category":"BANK_CARD","description":"对手方结算银行卡号"}' >/dev/null & PII_PIDS+=("$!")
wait_pids "${PII_PIDS[@]}"

S3_PIDS=()
api_ok POST /api/xblue/graph/algorithms/PAGERANK '{"subgraph":"fin_risk"}' >/dev/null & S3_PIDS+=("$!")
api_ok POST /api/xblue/graph/algorithms/LABEL_PROPAGATION '{"subgraph":"fin_risk"}' >/dev/null & S3_PIDS+=("$!")

(
  EXISTING_DASH=$(cat "$SEED_TMP/existing_dash.json")
  FIN_DASH_ID=$(printf '%s' "$EXISTING_DASH" | jq -r '.id // empty' 2>/dev/null || true)
  FIN_DASH_STATUS=$(printf '%s' "$EXISTING_DASH" | jq -r '.status // empty' 2>/dev/null || true)
  if [[ -z "$FIN_DASH_ID" || "$FIN_DASH_STATUS" != "PUBLISHED" || "${SHOWCASE_FORCE_REBUILD:-0}" == "1" ]]; then
    if [[ -n "$FIN_DASH_ID" ]]; then
      api_ok DELETE "/api/xblueview/dashboards/$FIN_DASH_ID" >/dev/null
    fi
    COCKPIT_DEF=$(cat "$PACK_DIR/dashboards/cockpit.json")
    CREATE_DASH_BODY=$(printf '%s' "$COCKPIT_DEF" | jq -c '{name, slug, description, theme, zone_layout: "FREE_GRID", canvas: (.canvas | .zone_layout = "FREE_GRID")}')
    FIN_DASH_ID=$(api_ok POST /api/xblueview/dashboards "$CREATE_DASH_BODY" | jq -r '.id')
    W_PIDS=()
    while IFS= read -r w_json; do
      api_ok POST "/api/xblueview/dashboards/$FIN_DASH_ID/widgets" "$w_json" >/dev/null &
      W_PIDS+=("$!")
    done < <(printf '%s' "$COCKPIT_DEF" | jq -c --arg fid "$FIN_FLOW_ID" '
      .widgets[] | if .data_source.kind == "ACTION" then .data_source.flow_id = $fid else . end
    ')
    if [[ ${#W_PIDS[@]} -gt 0 ]]; then wait_pids "${W_PIDS[@]}"; fi
    api_ok PUT "/api/xblueview/dashboards/$FIN_DASH_ID" "$(printf '%s' "$COCKPIT_DEF" | jq -c '{canvas: .canvas}')" >/dev/null
    PUB_ST=$(api_ok POST "/api/xblueview/dashboards/$FIN_DASH_ID/publish" '{}' | jq -r '.status // empty')
    if [[ "$PUB_ST" != "PUBLISHED" ]]; then
      echo "fin_risk.sh: expected fin-risk-cockpit PUBLISHED, got: $PUB_ST" >&2
      exit 1
    fi
  fi
  printf '%s' "$FIN_DASH_ID" > "$SEED_TMP/dash_id"
) &
S3_PIDS+=("$!")

(
  CERT_OUT=$(api_ok POST /api/xblue/subgraphs/fin_risk/certify '{}')
  CERT_LVL=$(printf '%s' "$CERT_OUT" | jq -r '.cert_level // empty')
  if [[ "$CERT_LVL" != "L4_CERTIFIED" ]]; then
    echo "fin_risk.sh: expected L4_CERTIFIED, got: $CERT_OUT" >&2
    exit 1
  fi
) &
S3_PIDS+=("$!")

wait_pids "${S3_PIDS[@]}"
FIN_DASH_ID=$(cat "$SEED_TMP/dash_id")

if [[ "$SHOWCASE_TENANT_ID" == "default" ]]; then
  upsert_ids_env "FIN_RISK_DATASET_ID" "$FIN_DS_ID"
  upsert_ids_env "FIN_RISK_DASHBOARD_ID" "$FIN_DASH_ID"
  upsert_ids_env "FIN_ANALYST_SH_USERNAME" "$FIN_ANALYST_SH_USERNAME"
  upsert_ids_env "FIN_ANALYST_SH_PASSWORD" "$FIN_ANALYST_SH_PASSWORD"
fi

echo "FIN_RISK_DATASET_ID=$FIN_DS_ID"
