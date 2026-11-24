#!/usr/bin/env bash
# Step 25 Gate：xv_charts 租户隔离、问数 1 步制图钉入驾驶舱、VTable 透视渲染与驾驶舱防挤压整齐排布验收
set -euo pipefail
source "$(dirname "$0")/lib.sh"

# 1. C80（上）：创建可复用图表资产并校验跨租户 RLS 0 行可见
CHART_RESP=$(api POST /api/xblueview/charts '{
  "name": "各状态订单金额分布图",
  "widget_kind": "BAR_CHART",
  "data_source": {"kind": "SQL", "sql": "SELECT status, SUM(amount) AS total_amt FROM demo_orders GROUP BY status"},
  "encoding": {"x": "status", "y": ["total_amt"]},
  "options": {"unit": "元", "show_mark_line": true}
}')
CHART_ID=$(printf '%s' "$CHART_RESP" | jq -r .id)
OTHER_CNT=$(sql_app other "SELECT count(*) FROM xv_charts WHERE id = '$CHART_ID'")
[[ "$OTHER_CNT" == "0" ]] || { echo "FAIL: xv_charts RLS leak" >&2; exit 1; }
echo "ASSERT chart-asset-crud PASS"

# 2. C80（下）：1 步将图表资产钉入目标五区驾驶舱 INSIGHT 分区
DASH_25=$(api POST /api/xblueview/dashboards '{
  "name": "Step25 制图联动看板",
  "slug": "step25-chart-pin-'$(date +%s)'",
  "theme": "light",
  "zone_layout": "STANDARD_FIVE_ZONE"
}' | jq -r .id)

PIN_RESP=$(api POST "/api/xblueview/charts/$CHART_ID/pin" "{
  \"dashboard_id\": \"$DASH_25\",
  \"target_zone\": \"INSIGHT\"
}")
expect_json "pin-chart-to-dashboard" "$PIN_RESP" '
  (.widget_id | length) > 0
  and .semantic_zone == "INSIGHT"
'

# 3. C81：前端构建、异步分包检查与 Playwright VTable + 磁吸网格 E2E
python3 "$ROOT/scripts/web-guard.py"
(cd "$ROOT/xbluenode-web" && npm run build && npm test)
VTABLE_MAIN=$(ls "$ROOT/xbluenode-web/dist/assets"/index-*.js | head -n 1)
if grep -q "PivotTable" "$VTABLE_MAIN" && grep -q "@visactor/vtable" "$VTABLE_MAIN"; then
  echo "FAIL: @visactor/vtable leaked into main entry bundle" >&2
  exit 1
fi
(cd "$ROOT/xbluenode-web" && npx playwright test e2e/step25-charts-vtable-grid.spec.ts e2e/xblueview.spec.ts e2e/analytics.spec.ts)
echo "ASSERT vtable-pivot-render PASS"
echo "ASSERT grid-float-tidy PASS"

gate_pass step25
