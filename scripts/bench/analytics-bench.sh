#!/usr/bin/env bash
# Executes 20 iterations of the 4 Step 04 §04.11 OLAP & AST guard benchmarks and writes P50/P95 JSON.
set -euo pipefail
source "$(dirname "$0")/../gate/lib.sh"

ITERS="${BENCH_ITERS:-20}"
GIT_SHA="$(git -C "$ROOT" rev-parse --short HEAD)"
mkdir -p "$ROOT/scripts/bench/results"
OUT_FILE="$ROOT/scripts/bench/results/analytics-${GIT_SHA}.json"

# Warm up tenant DuckDB session before timed iterations
api POST /api/analytics/sql/run '{"sql":"SELECT count(*) FROM bench_orders"}' >/dev/null

"$ROOT/axpoint-parser/.venv/bin/python" - "$ITERS" "$OUT_FILE" "$GIT_SHA" <<'PY'
from __future__ import annotations

import json
import os
import subprocess
import sys
import time
import urllib.request

iters = int(sys.argv[1])
out_file = sys.argv[2]
git_sha = sys.argv[3]
base_url = os.environ["CORE_BASE_URL"]
token = os.environ["AXPOINT_DEV_TOKEN"]
root_dir = os.path.abspath(os.path.join(os.path.dirname(out_file), "../../.."))


def post_json(path: str, payload: dict[str, object]) -> float:
    body = json.dumps(payload).encode("utf-8")
    req = urllib.request.Request(
        f"{base_url}{path}",
        data=body,
        headers={
            "Authorization": f"Bearer {token}",
            "Content-Type": "application/json",
            "Accept": "application/json",
        },
        method="POST",
    )
    t0 = time.perf_counter()
    with urllib.request.urlopen(req, timeout=60) as resp:
        raw = resp.read()
        if resp.status >= 400:
            raise RuntimeError(f"HTTP {resp.status}: {raw.decode('utf-8', 'replace')}")
    return (time.perf_counter() - t0) * 1000.0


def pct(samples: list[float], p: float) -> float:
    s = sorted(samples)
    idx = min(len(s) - 1, max(0, int(round((p / 100.0) * (len(s) - 1)))))
    return round(s[idx], 3)


sql_group_ms = [
    post_json(
        "/api/analytics/sql/run",
        {"sql": "SELECT region, SUM(amount) AS total FROM bench_orders GROUP BY region"},
    )
    for _ in range(iters)
]

pivot_ms = [
    post_json(
        "/api/analytics/pivot",
        {
            "dataset": "bench_orders",
            "rows": [{"field": "region"}, {"field": "status"}],
            "columns": [{"field": "channel"}],
            "values": [{"field": "amount", "agg": "SUM", "alias": "amt"}],
            "filters": [],
            "sort": [],
            "subtotals": True,
            "grand_total": True,
            "row_limit": 500,
            "column_limit": 50,
        },
    )
    for _ in range(iters)
]

metric_yoy_ms = [
    post_json(
        "/api/analytics/metrics/bench_revenue/query",
        {
            "dimensions": ["region"],
            "filters": [],
            "time_range": {
                "from": "2026-01-01T00:00:00Z",
                "to": "2027-01-01T00:00:00Z",
            },
            "grain": "MONTH",
            "compare": "SAME_PERIOD_LAST_YEAR",
        },
    )
    for _ in range(iters)
]

# Measure pure Rust AST sql_guard + rewrite latency (20 iterations via cargo test micro-probe or explain overhead)
guard_proc = subprocess.run(
    [
        "cargo",
        "test",
        "-p",
        "axpoint-analytics",
        "--release",
        "guard",
        "--",
        "--nocapture",
    ],
    cwd=os.path.join(root_dir, "axpoint-core"),
    capture_output=True,
    text=True,
    check=True,
)
# Measure per-call AST guard via /api/analytics/sql/explain minus DuckDB plan or direct sub-ms guard
guard_samples: list[float] = []
for _ in range(iters):
    t0 = time.perf_counter()
    # Invalid table triggers pure guard + rewrite path without DuckDB scan
    req = urllib.request.Request(
        f"{base_url}/api/analytics/sql/run",
        data=json.dumps({"sql": "SELECT region, SUM(amount) FROM unknown_bench_tbl GROUP BY region"}).encode(),
        headers={
            "Authorization": f"Bearer {token}",
            "Content-Type": "application/json",
        },
        method="POST",
    )
    try:
        urllib.request.urlopen(req, timeout=10)
    except urllib.error.HTTPError:
        pass
    guard_samples.append((time.perf_counter() - t0) * 1000.0)

report = {
    "git_sha": git_sha,
    "iterations": iters,
    "scenarios": {
        "single_dim_sum": {
            "p50_ms": pct(sql_group_ms, 50),
            "p95_ms": pct(sql_group_ms, 95),
            "limit_p95_ms": 1500.0,
        },
        "pivot_2x1_subtotals": {
            "p50_ms": pct(pivot_ms, 50),
            "p95_ms": pct(pivot_ms, 95),
            "limit_p95_ms": 3000.0,
        },
        "metric_yoy_12m_10r": {
            "p50_ms": pct(metric_yoy_ms, 50),
            "p95_ms": pct(metric_yoy_ms, 95),
            "limit_p95_ms": 2000.0,
        },
        "guard_and_rewrite": {
            "p50_ms": pct(guard_samples, 50),
            "p95_ms": pct(guard_samples, 95),
            "limit_p95_ms": 5.0,
        },
    },
}

with open(out_file, "w", encoding="utf-8") as fh:
    json.dump(report, fh, indent=2, ensure_ascii=False)

print(json.dumps(report, indent=2, ensure_ascii=False))
PY
