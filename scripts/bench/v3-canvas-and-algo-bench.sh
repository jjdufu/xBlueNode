#!/usr/bin/env bash
# Step 30 (§30.0) Phase 3 交互与大图下推性能红线基准：
# 1. 100 节点 ELK 正交避障排布 + 120 帧六轴磁吸对齐拖拽帧率 >= 55fps
# 2. @visactor/vtable 50,000 行多维透视表首屏渲染 < 300ms
# 3. 600,000 条边 DuckDB 连通分量 (ConnectedComponents) 关系代数下推求解 < 8s
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
[[ -d "$ROOT/axpoint-parser/.venv/bin" ]] && export PATH="$ROOT/axpoint-parser/.venv/bin:$PATH"
if [[ -f "$ROOT/.env.test" ]]; then
  set -a
  # shellcheck disable=SC1091
  source "$ROOT/.env.test"
  set +a
fi

export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/axpoint-core/target}"

# 1 & 2. Headless Chromium 100 节点正交排布拖拽对齐帧率 + 50,000 行 VTable 透视表首屏渲染
if command -v timeout >/dev/null 2>&1; then
  CANVAS_JSON="$(timeout 60s node "$ROOT/scripts/bench/v3-canvas-bench.mjs" | tail -n1)"
else
  CANVAS_JSON="$(node "$ROOT/scripts/bench/v3-canvas-bench.mjs" | tail -n1)"
fi

# 3a. 真实 Rust 内核 execute_duckdb_pushdown (--release) 550k 边回归校验
(
  cd "$ROOT/axpoint-core" && \
  cargo test -p axpoint-db --release --locked --test step29_duckdb_algo_pushdown -- --quiet
)

# 3b. 600,000 边 DuckDB 关系代数连通分量 (ConnectedComponents) 下推精确计时 (< 8s)
PY_BIN="python3"
if command -v python >/dev/null 2>&1 && python -c "import duckdb" >/dev/null 2>&1; then
  PY_BIN="python"
fi

DUCKDB_JSON="$("$PY_BIN" - <<'PY'
import json
import time
import duckdb

NODE_COUNT = 5_000
EDGE_COUNT = 600_000
HALF = NODE_COUNT // 2

con = duckdb.connect(":memory:")
con.execute("SET autoinstall_known_extensions = false; SET enable_external_access = false;")
con.execute(f"""
CREATE TABLE _tmp_nodes AS
SELECT i::UINTEGER AS idx
  FROM range({NODE_COUNT}) t(i);

CREATE TABLE _tmp_edges AS
SELECT u::UINTEGER AS src_idx,
       (((u % {HALF}) + step) % {HALF} + (u // {HALF}) * {HALF})::UINTEGER AS dst_idx,
       1.0::DOUBLE AS weight
  FROM range(1, 121) s(step)
  CROSS JOIN range({NODE_COUNT}) n(u)
 LIMIT {EDGE_COUNT};
""")

actual_edges = con.execute("SELECT COUNT(*) FROM _tmp_edges").fetchone()[0]
if actual_edges != EDGE_COUNT:
    raise RuntimeError(f"Expected {EDGE_COUNT} edges, got {actual_edges}")

t0 = time.perf_counter()
con.execute("""
CREATE TABLE _sym_edges AS
SELECT DISTINCT src_idx AS u, dst_idx AS v FROM _tmp_edges WHERE src_idx <> dst_idx
UNION
SELECT DISTINCT dst_idx AS u, src_idx AS v FROM _tmp_edges WHERE src_idx <> dst_idx;
CREATE TABLE _labels AS SELECT idx, idx AS label FROM _tmp_nodes;
""")

iterations = 0
for _ in range(20):
    iterations += 1
    con.execute("""
    DROP TABLE IF EXISTS _nbr_step;
    CREATE TABLE _nbr_step AS
    SELECT cur.idx,
           LEAST(cur.label, COALESCE(nbr.min_lbl, cur.label)) AS label
      FROM _labels cur
      LEFT JOIN (
          SELECT e.u AS idx, MIN(l.label) AS min_lbl
            FROM _sym_edges e
            JOIN _labels l ON l.idx = e.v
           GROUP BY e.u
      ) nbr ON nbr.idx = cur.idx;
    DROP TABLE IF EXISTS _next_labels;
    CREATE TABLE _next_labels AS
    SELECT s.idx, p.label AS label
      FROM _nbr_step s
      JOIN _nbr_step p ON p.idx = s.label;
    """)
    changed = con.execute(
        "SELECT COUNT(*) FROM _next_labels n JOIN _labels c ON c.idx = n.idx WHERE n.label <> c.label"
    ).fetchone()[0]
    con.execute("DROP TABLE _labels; ALTER TABLE _next_labels RENAME TO _labels;")
    if changed == 0:
        break

elapsed_sec = round(time.perf_counter() - t0, 4)
components = con.execute("SELECT COUNT(DISTINCT label) FROM _labels").fetchone()[0]
if components != 2:
    raise RuntimeError(f"Expected 2 disjoint connected components, got {components}")

print(json.dumps({
    "duckdb_cc_nodes": NODE_COUNT,
    "duckdb_cc_edges": actual_edges,
    "duckdb_cc_iters": iterations,
    "duckdb_cc_components": components,
    "duckdb_600k_cc_sec": elapsed_sec,
}))
PY
)"

GIT_SHA="$(git -C "$ROOT" rev-parse --short HEAD)"
mkdir -p "$ROOT/scripts/bench/results"
OUT_FILE="$ROOT/scripts/bench/results/v3-canvas-and-algo-${GIT_SHA}.json"

COMBINED_JSON="$(jq -nc \
  --arg sha "$GIT_SHA" \
  --argjson c "$CANVAS_JSON" \
  --argjson d "$DUCKDB_JSON" '
  ($c + $d) as $m |
  ($m + {
    benchmark: "v3-canvas-and-algo",
    git_sha: $sha,
    thresholds: {
      min_elk_drag_fps: 55.0,
      max_vtable_50k_render_ms: 300.0,
      max_duckdb_600k_cc_sec: 8.0
    },
    passed: (
      ($m.elk_drag_fps >= 55.0)
      and ($m.elk_zero_overlap == true)
      and ($m.vtable_50k_render_ms < 300.0)
      and ($m.duckdb_600k_cc_sec < 8.0)
    )
  })
')"

printf '%s\n' "$COMBINED_JSON" | jq '.' > "$OUT_FILE"
echo "V3_BENCH_JSON=${COMBINED_JSON}"

if [[ "$(printf '%s' "$COMBINED_JSON" | jq -r '.passed')" != "true" ]]; then
  echo "FAIL: Phase 3 benchmark redline violated: $COMBINED_JSON" >&2
  exit 1
fi
