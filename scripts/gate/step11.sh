#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "$0")/lib.sh"

(cd axpoint-core && cargo test -p axpoint-db view_repo && cargo test -p axpoint-server view && cargo test -p axpoint-server --test dashboard_gen_eval)
(cd xbluenode-web && npm run build)

if [[ -z "${DASHBOARD_ID:-}" || -z "${KPI_WIDGET_ID:-}" || -z "${BAR_WIDGET_ID:-}" || -z "${SCENE3D_WIDGET_ID:-}" || -z "${ACTION_WIDGET_ID:-}" ]]; then
  eval "$(bash "$(dirname "$0")/../seed/11-xblueview.sh")"
fi

# 数据正确：已支付金额 = 独立 SQL 计算
EXPECTED=$(sql_owner "SELECT sum(amount) FROM demo_src.orders WHERE status='PAID'" | awk '{printf "%.2f", $1}')
ACTUAL=$(api POST "/api/xblueview/widgets/$KPI_WIDGET_ID/data" '{}' | jq -r '.rows[0][0]' | awk '{printf "%.2f", $1}')
expect_eq "kpi" "$ACTUAL" "$EXPECTED"

# 下钻与 3D 孪生场景数据绑定
expect_json "drill" "$(api POST "/api/xblueview/widgets/$BAR_WIDGET_ID/data" '{"drill_path":[{"field":"status","value":"PAID"}]}')" \
  '.drill_level == 1 and ([.columns[].name] | index("warehouse_code")) != null'
expect_json "scene-3d" "$(api POST "/api/xblueview/widgets/$SCENE3D_WIDGET_ID/data" '{}')" \
  '(.rows | length) == 1 and (.rows[0][4] == "Rack_A")'

# AI 一句话生成驾驶舱（录制响应）
AI_DB=$(api POST /api/xblueview/dashboards/ai-generate '{"prompt":"生成华东供应链库存与已支付订单监控看板"}' | jq -r .id)
expect_json "ai-dashboard" "$(api GET "/api/xblueview/dashboards/$AI_DB")" '.status == "DRAFT" and (.widgets | length) >= 2'

# 实时刷新：订阅 SSE，触发同步，30 秒内收到 invalidate
SSE_LOG=$(mktemp)
curl -sN -H "Authorization: Bearer $AXPOINT_DEV_TOKEN" -H "Accept: text/event-stream" \
  "$CORE_BASE_URL/api/xblueview/dashboards/$DASHBOARD_ID/stream" > "$SSE_LOG" &
SSE_PID=$!
sleep 1
api POST "/api/axhub/datasets/$DEMO_ORDERS_DATASET_ID/sync" '{}' >/dev/null
wait_until "sse-invalidate" 30 "grep -q '^event: invalidate' '$SSE_LOG'"
kill "$SSE_PID" || true
rm -f "$SSE_LOG"

# 页面级动作组件回写 → 审计落库 + 缓存失效自动反馈到 3D 场景组件
api POST "/api/xblueview/widgets/$ACTION_WIDGET_ID/invoke" '{"inputs":{"value":620},"idempotency_key":"gate-view-act-1"}' >/dev/null
wait_until "widget-action-loop" 30 "api POST '/api/xblueview/widgets/$SCENE3D_WIDGET_ID/data' '{}' | jq -e '(.rows[0][9] | tonumber) == 620'"
expect_eq "widget-action-log" "$(sql_app default "SELECT count(*) FROM ax_action_log WHERE source_kind='VIEW' AND idempotency_key='gate-view-act-1'")" "1"
api POST "/api/xblueview/widgets/$ACTION_WIDGET_ID/invoke" '{"inputs":{"value":500},"idempotency_key":"gate-view-act-2"}' >/dev/null

# 缓存失效后数据反映新值，并在恢复源表后同步回一致性基线
sql_owner "UPDATE demo_src.orders SET status='PAID', updated_at=now() WHERE id=2" >/dev/null
wait_job "$(api POST "/api/axhub/datasets/$DEMO_ORDERS_DATASET_ID/sync" '{}' | jq -r .job_id)"
EXPECTED2=$(sql_owner "SELECT sum(amount) FROM demo_src.orders WHERE status='PAID'" | awk '{printf "%.2f", $1}')
wait_until "kpi-refreshed" 30 "[[ \"\$(api POST '/api/xblueview/widgets/$KPI_WIDGET_ID/data' '{}' | jq -r '.rows[0][0]' | awk '{printf \"%.2f\", \$1}')\" == \"$EXPECTED2\" ]]"
sql_owner "UPDATE demo_src.orders SET status='CANCELED', updated_at=now() WHERE id=2" >/dev/null
wait_job "$(api POST "/api/axhub/datasets/$DEMO_ORDERS_DATASET_ID/sync" '{}' | jq -r .job_id)"
wait_mapping_idle "$ORDER_MAPPING_ID"
wait_aggregate_idle

# 跨租户隔离
expect_eq "rls-xv" "$(sql_app other "SELECT count(*) FROM xv_dashboards")" "0"

(cd xbluenode-web && npx playwright test e2e/xblueview.spec.ts)
gate_pass step11
