#!/usr/bin/env bash
# 调用 GET /api/axops/health/deep 并以表格输出健康状态；任一项 ok=false 则退出码 1（§14.4.3）
set -euo pipefail

CORE_URL="${CORE_BASE_URL:-http://127.0.0.1:9995}"
TOKEN="${AXPOINT_API_TOKEN:-${AXPOINT_DEV_TOKEN:-}}"
if [[ -z "$TOKEN" ]]; then
  echo "health-report: AXPOINT_API_TOKEN (or AXPOINT_DEV_TOKEN) is required" >&2
  exit 2
fi

RESP=$(curl -fsS -H "Authorization: Bearer $TOKEN" -H "Accept: application/json" "$CORE_URL/api/axops/health/deep")

python3 - "$RESP" <<'PY'
from __future__ import annotations

import json
import sys

data = json.loads(sys.argv[1])
rows: list[tuple[str, bool, str]] = [
    (
        "database",
        bool(data["database"]["ok"]),
        f"latency={data['database'].get('latency_ms')}ms lag={data['database'].get('replication_lag_s')}s",
    ),
    (
        "parser",
        bool(data["parser"]["ok"]),
        f"version={data['parser'].get('version', '-')}",
    ),
    (
        "lake",
        bool(data["lake"]["ok"]),
        f"snapshot_id={data['lake'].get('snapshot_id', 0)}",
    ),
    (
        "llm",
        bool(data["llm"]["ok"]),
        f"provider={data['llm'].get('provider', '-')}",
    ),
    (
        "embed",
        bool(data["embed"]["ok"]),
        data["embed"].get("error", "ready") or "ready",
    ),
    (
        "disk",
        bool(data["disk"]["ok"]),
        f"free_bytes={data['disk'].get('data_root_free_bytes', 0)}",
    ),
    (
        "migrations",
        bool(data["migrations"].get("ok", data["migrations"].get("pending", 1) == 0)),
        f"current={data['migrations'].get('current')} pending={data['migrations'].get('pending')}",
    ),
]

print(f"xBlueNode Deep Health Report (core version: {data.get('version', '-')})")
print("-" * 74)
print(f"{'COMPONENT':<14} {'STATUS':<8} {'DETAIL'}")
print("-" * 74)
all_ok = True
for name, ok, detail in rows:
    status = "OK" if ok else "FAIL"
    if not ok:
        all_ok = False
    print(f"{name:<14} {status:<8} {detail}")
print("-" * 74)
jobs = data.get("jobs", {})
outbox = data.get("outbox", {})
print(
    f"jobs: pending={jobs.get('pending', 0)} running={jobs.get('running', 0)} "
    f"failed_24h={jobs.get('failed_24h', 0)} oldest_pending_s={jobs.get('oldest_pending_s', 0)}"
)
print(f"outbox: pending={outbox.get('pending', 0)} lag_s={outbox.get('lag_s', 0)}")
sys.exit(0 if all_ok else 1)
PY
