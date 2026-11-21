#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "$0")/lib.sh"

(cd axpoint-parser && (uv run pytest tests/test_exporters.py tests/test_charts.py -q || pytest tests/test_exporters.py tests/test_charts.py -q))

if [[ -z "${DASHBOARD_ID:-}" || -z "${KPI_WIDGET_ID:-}" ]]; then
  bash "$ROOT/scripts/seed-demo.sh" 11
  # shellcheck disable=SC1091
  source "$ROOT/.gate/ids.env"
fi

# 服务端图表
curl -sf -H "X-AxPoint-Internal-Token: $AXPOINT_INTERNAL_TOKEN" -H "Content-Type: application/json" \
  -X POST "$PARSER_BASE_URL/render/chart" \
  -d '{"option":{"xAxis":{"type":"category","data":["甲","乙"]},"yAxis":{},"series":[{"type":"bar","data":[1,2]}]},"width":600,"height":400,"format":"png"}' \
  -o /tmp/chart.png
expect_eq "chart-png" "$(head -c 8 /tmp/chart.png | od -An -tx1 | tr -d ' \n')" "89504e470d0a1a0a"
rm -f /tmp/chart.png

# 六种格式出品
for F in XLSX CSV PDF DOCX PPTX MARKDOWN; do
  EID=$(api POST /api/xblueview/exports "{\"export_format\":\"$F\",\"source\":{\"kind\":\"DASHBOARD\",\"dashboard_id\":\"$DASHBOARD_ID\",\"filters\":{}},\"options\":{\"title\":\"供应概览\",\"watermark\":true}}" | jq -r .id)
  wait_export "$EID"
  curl -sf -H "Authorization: Bearer $AXPOINT_DEV_TOKEN" "$CORE_BASE_URL/api/xblueview/exports/$EID/download" -o "/tmp/export.$F"
  python scripts/gate/verify_export.py "$F" "/tmp/export.$F" "$(sql_owner "SELECT sum(amount) FROM demo_src.orders WHERE status='PAID'")"
  rm -f "/tmp/export.$F"
done
expect_eq "audit-export" "$(sql_app default "SELECT count(*) >= 6 FROM sec_audit_logs WHERE action='EXPORT_FILE'")" "t"

# 分享：锁定筛选、次数、撤销
SH=$(api POST /api/xblueview/shares "{\"target_kind\":\"DASHBOARD\",\"target_id\":\"$DASHBOARD_ID\",\"expires_in_hours\":1,\"max_views\":2,\"locked_filters\":{},\"allow_filters\":false}")
TOKEN=$(echo "$SH" | jq -r .token)
expect_code "share-view-1" 200 GET_NOAUTH "/api/xblueview/share/$TOKEN"
expect_code "share-data" 200 POST_NOAUTH "/api/xblueview/share/$TOKEN/widgets/$KPI_WIDGET_ID/data" '{}'
expect_code "share-view-2" 200 GET_NOAUTH "/api/xblueview/share/$TOKEN"
expect_code "share-max-views" 404 GET_NOAUTH "/api/xblueview/share/$TOKEN"

SH2=$(api POST /api/xblueview/shares "{\"target_kind\":\"DASHBOARD\",\"target_id\":\"$DASHBOARD_ID\",\"expires_in_hours\":1,\"password\":\"p@ss-123456\",\"locked_filters\":{},\"allow_filters\":false}")
T2=$(echo "$SH2" | jq -r .token)
S2=$(echo "$SH2" | jq -r .id)
expect_json "share-password" "$(curl -s "$CORE_BASE_URL/api/xblueview/share/$T2")" '.password_required == true'
api DELETE "/api/xblueview/shares/$S2" >/dev/null
expect_code "share-revoked" 404 GET_NOAUTH "/api/xblueview/share/$T2"
expect_code "share-token-not-api" 401 GET "/api/xblueview/dashboards" "" "$TOKEN"
expect_eq "audit-share" "$(sql_app default "SELECT count(*) >= 2 FROM sec_audit_logs WHERE action='SHARE_ACCESS'")" "t"

gate_pass step12
