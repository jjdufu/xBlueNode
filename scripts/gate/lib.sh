#!/usr/bin/env bash
# 门禁公共函数。所有 scripts/gate/stepNN.sh 与 scripts/seed/*.sh 以 source 方式引入。
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
[[ -d "$ROOT/axpoint-parser/.venv/bin" ]] && export PATH="$ROOT/axpoint-parser/.venv/bin:$PATH"
set -a
# shellcheck disable=SC1091
source "$ROOT/.env.test"
# shellcheck disable=SC1091
[[ -f "$ROOT/.gate/ids.env" ]] && source "$ROOT/.gate/ids.env"
set +a
: "${CORE_BASE_URL:?}" "${PARSER_BASE_URL:?}" "${AXPOINT_DEV_TOKEN:?}"
GATE_NAME="$(basename "${0%.sh}")"

_fail() { echo "GATE ${GATE_NAME} FAIL: $1" >&2; [[ -n "${2:-}" ]] && echo "$2" | head -c 2000 >&2; exit 1; }

# api METHOD PATH [JSON]：带开发令牌调用 Core，输出响应体（非 2xx 时同样输出，便于断言错误体）
api() {
  local m=$1 p=$2 body=${3:-}
  if [[ -n "$body" ]]; then
    curl -sS -X "$m" -H "Authorization: Bearer $AXPOINT_DEV_TOKEN" -H "Accept: application/json" \
         -H "Content-Type: application/json" "$CORE_BASE_URL$p" -d "$body"
  else
    curl -sS -X "$m" -H "Authorization: Bearer $AXPOINT_DEV_TOKEN" -H "Accept: application/json" "$CORE_BASE_URL$p"
  fi
}

# api_noauth METHOD PATH [JSON]：不带令牌
api_noauth() {
  local m=$1 p=$2 body=${3:-}
  if [[ -n "$body" ]]; then
    curl -sS -X "$m" -H "Accept: application/json" -H "Content-Type: application/json" "$CORE_BASE_URL$p" -d "$body"
  else
    curl -sS -X "$m" -H "Accept: application/json" "$CORE_BASE_URL$p"
  fi
}

expect_eq() { [[ "$2" == "$3" ]] || _fail "$1" "actual=[$2] expected=[$3]"; echo "ok  $1"; }

# expect_code NAME CODE METHOD PATH [JSON] [TOKEN]
#   METHOD 以 _NOAUTH 结尾（GET_NOAUTH、POST_NOAUTH 等）时不带令牌；TOKEN 非空时用其替代开发令牌
expect_code() {
  local name=$1 code=$2 m=$3 p=$4 body=${5:-} tok=${6:-$AXPOINT_DEV_TOKEN} auth=()
  if [[ "$m" == *_NOAUTH ]]; then m=${m%_NOAUTH}; else auth=(-H "Authorization: Bearer $tok"); fi
  local args=(-sS -o /dev/null -w '%{http_code}' -X "$m" "${auth[@]}" -H "Accept: application/json")
  [[ -n "$body" ]] && args+=(-H "Content-Type: application/json" -d "$body")
  local got; got=$(curl "${args[@]}" "$CORE_BASE_URL$p")
  expect_eq "$name" "$got" "$code"
}

# expect_json NAME JSON JQ_EXPR：jq 表达式结果必须为 true
expect_json() {
  local out; out=$(printf '%s' "$2" | jq -e "$3" 2>&1) || _fail "$1" "$2"
  [[ "$out" == "true" ]] || _fail "$1" "$2"
  echo "ok  $1"
}

sql_owner() { psql "$MIGRATION_DATABASE_URL" -v ON_ERROR_STOP=1 -Atqc "$1"; }

# sql_app TENANT SQL：以应用角色、指定租户上下文执行（验证 RLS）
sql_app() {
  local tenant="${1//\'/\'\'}"
  psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -Atq <<SQL | sed '/^$/d'
\\o /dev/null
BEGIN;
SELECT set_config('app.tenant_id', '${tenant}', true);
\\o
$2;
\\o /dev/null
COMMIT;
SQL
}

# wait_until NAME TIMEOUT_S CMD：每秒执行 CMD，直到成功或超时
wait_until() {
  local name=$1 timeout=$2 cmd=$3 i=0
  until bash -c "source '$ROOT/scripts/gate/lib.sh' >/dev/null 2>&1; $cmd" >/dev/null 2>&1; do
    i=$((i + 1)); (( i >= timeout )) && _fail "$name" "timeout ${timeout}s: $cmd"; sleep 1
  done
  echo "ok  $name"
}

# 任务与业务对象等待（默认 120 秒）
wait_job() {
  local id=$1 st
  for _ in $(seq 1 120); do
    st=$(api GET "/api/axops/jobs/$id" | jq -r .status)
    case "$st" in SUCCEEDED) return 0 ;; FAILED|CANCELED|EXPIRED) _fail "job $id" "$(api GET "/api/axops/jobs/$id")" ;; esac
    sleep 1
  done; _fail "job $id" "timeout"
}
wait_dataset_idle() {
  wait_until "dataset-idle:$1" 120 \
    "api GET '/api/axhub/datasets/$1/sync-jobs?limit=1' | jq -e '.items[0].status == \"SUCCEEDED\"'"
}
wait_aggregate_idle() {
  wait_until "aggregate-idle" 120 \
    "[[ \$(sql_app default \"SELECT count(*) FROM ax_jobs WHERE kind='GRAPH_AGGREGATE' AND status IN ('PENDING','RUNNING')\") == 0 ]]"
}
wait_mapping_idle() {
  wait_until "mapping-idle:$1" 120 \
    "api GET '/api/xblue/mappings/$1/runs?limit=1' | jq -e '.items[0].status == \"SUCCEEDED\"'"
}
wait_run_status() {
  wait_until "run:$1:$2" 120 "api GET '/api/axflow/runs/$1' | jq -e '.status == \"$2\"'"
}
wait_scenario() {
  wait_until "scenario:$1" 120 "api GET '/api/axsim/scenarios/$1' | jq -e '.status == \"SUCCEEDED\"'"
}
wait_export() {
  wait_until "export:$1" 180 "api GET '/api/xblueview/exports/$1' | jq -e '.status == \"SUCCEEDED\"'"
}

# restart_core：重启本地 Core 进程（scripts/start-services.sh 记录 PID），等待健康检查
restart_core() {
  bash "$ROOT/scripts/start-services.sh" --restart core
  wait_until "core-healthy" 60 "curl -sf '$CORE_BASE_URL/api/health'"
}

gate_pass() { echo "GATE $1 PASS"; }
