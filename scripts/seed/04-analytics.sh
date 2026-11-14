#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "$0")/../gate/lib.sh"

# 1. Enrich demo_orders column metadata (confirm PHONE PII masking & business terms)
api PATCH "/api/axhub/datasets/$DEMO_ORDERS_DATASET_ID/columns/phone" \
  '{"pii_category":"PHONE","description":"客户联系电话"}' >/dev/null
api PATCH "/api/axhub/datasets/$DEMO_ORDERS_DATASET_ID/columns/amount" \
  '{"description":"订单金额","business_term":"交易金额","unit":"CNY"}' >/dev/null
api PATCH "/api/axhub/datasets/$DEMO_ORDERS_DATASET_ID/columns/status" \
  '{"description":"订单状态(PAID=已支付,SHIPPED=已发货,OPEN=待处理)","business_term":"订单状态"}' >/dev/null

# 2. Create certified semantic metric paid_amount (SUM(amount) WHERE status = 'PAID')
METRIC_PAYLOAD=$(jq -nc --arg dsid "$DEMO_ORDERS_DATASET_ID" '{
  name: "paid_amount",
  display_name: "已支付订单总金额",
  description: "统计状态为 PAID 的已支付订单金额总和",
  dataset_id: $dsid,
  expression: "SUM(amount)",
  filter_sql: "status = '\''PAID'\''",
  time_column: "updated_at",
  default_grain: "MONTH",
  dimensions: [
    { name: "status", column: "status", display_name: "订单状态" },
    { name: "region", column: "region", display_name: "区域" }
  ],
  unit: "CNY",
  format: "0,0.00",
  synonyms: ["已支付订单总金额", "已付款金额", "实付总额"],
  certified: true
}')
api DELETE "/api/analytics/metrics/paid_amount" >/dev/null 2>&1 || true
api POST "/api/analytics/metrics" "$METRIC_PAYLOAD" >/dev/null

# 3. Create shared saved query orders_by_status
SAVED_JSON=$(api POST "/api/analytics/saved-queries" '{
  "name": "orders_by_status",
  "kind": "SQL",
  "spec": {
    "sql": "SELECT status, COUNT(*) AS order_count, SUM(amount) AS total_amount FROM demo_orders GROUP BY status ORDER BY status"
  },
  "parameters": [],
  "shared": true
}')
SAVED_ID=$(printf '%s' "$SAVED_JSON" | jq -r '.id // empty')

# 4. Wait for EMBED_NODES background jobs to drain (redirecting stdout so only env vars are emitted)
for _ in $(seq 1 60); do
  PENDING=$(sql_app default "SELECT count(*) FROM ax_jobs WHERE kind='EMBED_NODES' AND status IN ('PENDING','RUNNING')")
  [[ "$PENDING" == "0" ]] && break
  sleep 1
done

echo "PAID_METRIC_NAME=paid_amount"
if [[ -n "$SAVED_ID" ]]; then
  echo "ORDERS_BY_STATUS_QUERY_ID=$SAVED_ID"
fi
