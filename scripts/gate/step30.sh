#!/usr/bin/env bash
# Step 30 Gate：Phase 3 全栈守卫、92 项能力溯源、性能基准与三套全旅程 E2E 总验收
set -euo pipefail
source "$(dirname "$0")/lib.sh"

# 1. 全套 G1 静态守卫与双向零漂移核验（C92）
if [[ -n "${NAMING_DENYLIST:-}" || -f "$ROOT/.env" ]]; then
  bash "$ROOT/scripts/naming-guard.sh"
fi
python3 "$ROOT/scripts/lint-guard.py"
python3 "$ROOT/scripts/web-guard.py"
python3 "$ROOT/scripts/python-guard.py"
python3 "$ROOT/scripts/hallucination-guard.py"
python3 "$ROOT/scripts/docs-and-plans-guard.py"
python3 "$ROOT/scripts/check_routes.py" \
  --routes "$ROOT/axpoint-core/crates/axpoint-server/src/routes" \
  --index "$ROOT/docs/standards/A4-API-CONTRACT-INDEX.md"
python3 "$ROOT/scripts/check_traceability.py" \
  "$ROOT/docs/standards/A7-REQUIREMENTS-TRACEABILITY.md" \
  "$ROOT/scripts/gate"
bash "$ROOT/scripts/check-write-scope.sh" --self-test
echo "ASSERT traceability-92-ok PASS"

# 2. 双轨 50 例反幻觉回归 + 画布/大图性能基准
run_dual_track_eval() {
  local track="$1"
  (
    cd "$ROOT/axpoint-core" && \
    AXPOINT_LLM_CASSETTE="$ROOT/tests/cassettes/v2_dual_track/$track" \
    AXPOINT_EVAL_TRACK="$track" \
    cargo test -p axpoint-analytics --test grounded_ask_eval --locked -- --nocapture \
      | grep -E '^EVAL_SUMMARY_JSON=' | tail -n1 | sed 's/^EVAL_SUMMARY_JSON=//'
  )
}

CLOUD_EVAL=$(run_dual_track_eval "cloud_flagship")
expect_json "step30-dual-track-cloud-100" "$CLOUD_EVAL" '
  .total_cases == 50 and .intercepted_or_exact == 50 and .hallucination_leaked == 0 and .pii_leaked == 0
'

LOCAL_EVAL=$(run_dual_track_eval "local_7b_qwen")
expect_json "step30-dual-track-local-100" "$LOCAL_EVAL" '
  .total_cases == 50 and .intercepted_or_exact == 50 and .hallucination_leaked == 0 and .pii_leaked == 0
'

GROUNDED_BENCH_JSON=$(bash "$ROOT/scripts/bench/grounded-ai-bench.sh" | tail -n1)
expect_json "step30-grounding-p95-lt-50ms" "$GROUNDED_BENCH_JSON" '.p95_ms < 50.0'

V3_BENCH_RAW=$(bash "$ROOT/scripts/bench/v3-canvas-and-algo-bench.sh" | grep -E '^V3_BENCH_JSON=' | tail -n1 | sed 's/^V3_BENCH_JSON=//')
expect_json "step30-v3-bench-redlines" "$V3_BENCH_RAW" '
  .passed == true
  and .elk_drag_fps >= 55.0
  and .vtable_50k_render_ms < 300.0
  and .duckdb_600k_cc_sec < 8.0
'

# 3. Phase 3 端到端全旅程 E2E 验收
PW_PORT="${PLAYWRIGHT_PORT:-$(( 4173 + ${WT_SLOT:-0} * 100 ))}"
(cd "$ROOT/xbluenode-web" && PLAYWRIGHT_PORT="$PW_PORT" npx playwright test e2e/v3-craft-and-runtime.spec.ts)
echo "ASSERT v3-journey-pass PASS"

gate_pass step30
