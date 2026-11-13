#!/usr/bin/env bash
# 用法：start-services.sh [--restart core|parser]；后台启动 Sidecar 与 Core，真实进程 PID 写入 .gate/pids/
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT="$PWD"
set -a
# shellcheck disable=SC1091
source "$ROOT/.env.test"
set +a
export PATH="$ROOT/axpoint-parser/.venv/bin:$PATH"
mkdir -p "$ROOT/.gate/pids" "$ROOT/.gate/logs"

start_parser() {
  (
    cd "$ROOT/axpoint-parser"
    nohup uvicorn main:app --host 127.0.0.1 --port 8001 > "$ROOT/.gate/logs/parser.log" 2>&1 &
    echo $! > "$ROOT/.gate/pids/parser"
  )
}

start_core() {
  (
    cd "$ROOT/axpoint-core"
    cargo build -q -p axpoint-server
    nohup ./target/debug/axpoint-server > "$ROOT/.gate/logs/core.log" 2>&1 &
    echo $! > "$ROOT/.gate/pids/core"
  )
}

stop() {
  if [[ -f "$ROOT/.gate/pids/$1" ]]; then
    kill "$(cat "$ROOT/.gate/pids/$1")" 2>/dev/null || true
    rm -f "$ROOT/.gate/pids/$1"
  fi
}

wait_http() {
  for _ in $(seq 1 180); do
    curl -sf "$1" >/dev/null && return 0
    sleep 1
  done
  echo "timeout: $1" >&2
  exit 1
}

if [[ "${1:-}" == "--restart" ]]; then
  stop "$2"
  "start_$2"
else
  stop parser
  stop core
  (cd "$ROOT/xbluenode-web" && VITE_DEV_TOKEN="$AXPOINT_DEV_TOKEN" npm run build >/dev/null)
  start_parser
  start_core
fi

wait_http "$PARSER_BASE_URL/health"
wait_http "$CORE_BASE_URL/api/health"
echo "services: OK"
