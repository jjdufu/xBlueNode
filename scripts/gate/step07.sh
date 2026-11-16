#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "$0")/lib.sh"

(cd axpoint-core && cargo test -p axpoint-flow && cargo test -p axpoint-db flow_repo)

# 环检测
expect_json "cycle" "$(api POST /api/axflow/flows/$CYCLIC_FLOW_ID/validate)" '[.issues[].code] | index("CYCLE_DETECTED") != null'

# 正式运行：运行到审批暂停
RUN=$(api POST /api/axflow/flows/$FLOW_ID/runs '{"environment":"PROD","idempotency_key":"gate-1"}' | jq -r .run_id)
wait_run_status "$RUN" WAITING_APPROVAL
expect_eq "idempotent-run" "$(api POST /api/axflow/flows/$FLOW_ID/runs '{"environment":"PROD","idempotency_key":"gate-1"}' | jq -r .run_id)" "$RUN"

# 审批通过 → 动作执行 → 运行成功
APPROVAL=$(api GET '/api/axflow/approvals?status=PENDING' | jq -r --arg r "$RUN" '.items[] | select(.run_id==$r) | .id')
api POST /api/axflow/approvals/$APPROVAL/decide '{"decision":"APPROVE","comment":"ok"}' >/dev/null
wait_run_status "$RUN" SUCCEEDED
expect_json "action-done" "$(api GET /api/xblue/nodes/$WAREHOUSE_NODE_ID)" '.metrics.replenish_flag == 1'
expect_json "steps-all"   "$(api GET /api/axflow/runs/$RUN/steps)" '[.items[] | select(.status=="SUCCEEDED")] | length == 6'

# 测试环境不产生副作用
RUN_T=$(api POST /api/axflow/flows/$FLOW_ID/runs '{"environment":"TEST"}' | jq -r .run_id)
wait_run_status "$RUN_T" WAITING_APPROVAL
APPROVAL_T=$(api GET '/api/axflow/approvals?status=PENDING' | jq -r --arg r "$RUN_T" '.items[] | select(.run_id==$r) | .id')
api POST /api/axflow/approvals/$APPROVAL_T/decide '{"decision":"APPROVE"}' >/dev/null
wait_run_status "$RUN_T" SUCCEEDED
expect_json "test-dry-run" "$(api GET "/api/axflow/actions?run_id=$RUN_T")" '.items[0].status == "DRY_RUN"'

# 崩溃恢复：执行中杀掉 Core 进程，重启后运行仍完成（只在本地门禁执行，CI 用 testcontainers 等价用例覆盖）
if [[ "${GATE_ALLOW_RESTART:-0}" == "1" ]]; then
  RUN_C=$(api POST /api/axflow/flows/$FLOW_ID/runs '{"environment":"TEST"}' | jq -r .run_id)
  restart_core
  wait_run_status "$RUN_C" WAITING_APPROVAL
fi

gate_pass step07
