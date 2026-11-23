#!/usr/bin/env bash
# Step 22 门禁：Phase 2 静态与溯源守卫、双轨 50 例反幻觉基准、接地守卫延迟基准与全旅程 Playwright E2E
set -euo pipefail
source "$(dirname "$0")/lib.sh"

MOCK_IM_PORT="${MOCK_IM_PORT:-19822}"
MOCK_IM_PID=""

cleanup_step22() {
  sql_owner "DELETE FROM ax_im_user_bindings WHERE tenant_id = 'default' AND external_user_id = 'ou_step22_e2e_user'" >/dev/null 2>&1 || true
  if [[ -n "$MOCK_IM_PID" ]]; then
    kill "$MOCK_IM_PID" >/dev/null 2>&1 || true
  fi
}
trap cleanup_step22 EXIT

# 1. 全套 G1 静态守卫与双向契约审计
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
echo "ok  v2-static-and-traceability-guards"

# 2. 双轨录制回放无幻觉对抗基准（cloud_flagship 与 local_7b_qwen 均要求 50/50 = 100% 拦截）
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

CLOUD_TRACK_REPORT=$(run_dual_track_eval "cloud_flagship")
expect_json "dual-track-cloud_flagship-100" "$CLOUD_TRACK_REPORT" '
  .total_cases == 50
  and .intercepted_or_exact == 50
  and .hallucination_leaked == 0
  and .pii_leaked == 0
'

LOCAL_TRACK_REPORT=$(run_dual_track_eval "local_7b_qwen")
expect_json "dual-track-local_7b_qwen-100" "$LOCAL_TRACK_REPORT" '
  .total_cases == 50
  and .intercepted_or_exact == 50
  and .hallucination_leaked == 0
  and .pii_leaked == 0
'

# 3. 接地守卫纯计算开销基准（P95 < 50ms）
BENCH_JSON=$(bash "$ROOT/scripts/bench/grounded-ai-bench.sh" | tail -n1)
expect_json "grounding-guard-p95-lt-50ms" "$BENCH_JSON" '.p95_ms < 50.0'

# 4. 自愈演示种子与本地 IM 回环桩，执行前端生产构建与 Phase 2 端到端全旅程测试
FACTORY1_NODE_ID=$(sql_app default "SELECT id FROM xb_nodes WHERE node_key = 'F-1' LIMIT 1" || true)
SUPPLIES_PROP=$(sql_app default "SELECT COALESCE(propagation::text, '{}') FROM xb_edge_types WHERE name = 'SUPPLIES' LIMIT 1" || echo "{}")
if [[ -z "$FACTORY1_NODE_ID" || -z "$SUPPLIES_PROP" || "$SUPPLIES_PROP" == "{}" ]]; then
  bash "$ROOT/scripts/seed-demo.sh" 10 >/dev/null
  bash "$ROOT/scripts/seed/10-axsim.sh" >/dev/null
fi
OPS_EXISTS=$(sql_owner "SELECT count(*) FROM sec_users WHERE tenant_id = 'default' AND username = 'ops_admin'" || echo "0")
if [[ "$OPS_EXISTS" == "0" ]]; then
  bash "$ROOT/scripts/seed-demo.sh" 13 >/dev/null
fi
if [[ -f "$ROOT/.gate/ids.env" ]]; then
  # shellcheck disable=SC1091
  source "$ROOT/.gate/ids.env"
fi

mkdir -p "$ROOT/.gate"
python3 "$ROOT/scripts/gate/tools/mock_im_hub.py" --port "$MOCK_IM_PORT" >"$ROOT/.gate/mock_im_hub_step22.log" 2>&1 &
MOCK_IM_PID=$!
wait_until "mock-im-step22-ready" 15 "curl -sf 'http://127.0.0.1:${MOCK_IM_PORT}/test/health'"

(cd "$ROOT/xbluenode-web" && \
  npm run build && \
  MOCK_IM_HUB_URL="http://127.0.0.1:${MOCK_IM_PORT}" \
  OPS_ADMIN_PASSWORD="${OPS_ADMIN_PASSWORD:-}" \
  AXPOINT_E2E_LIVE=1 \
  npx playwright test e2e/v2-grounded-journey.spec.ts)
echo "ok  v2-grounded-journey-e2e"

gate_pass step22
