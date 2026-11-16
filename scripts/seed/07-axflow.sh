#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "$0")/../gate/lib.sh"

# 1. Create daily_supply_refresh flow draft (6 nodes: sync_orders -> map_orders -> agg -> check -(n > 0)-> approve -> emit)
FLOW_PAYLOAD=$(jq -n \
  --arg ds_id "$DEMO_ORDERS_DATASET_ID" \
  --arg map_id "$ORDER_MAPPING_ID" \
  --arg wh_id "$WAREHOUSE_NODE_ID" \
  '{
    name: "daily_supply_refresh",
    description: "每日供应链订单入湖、业务节点映射、聚合核验、人工审批与补货指标回写编排流",
    slug: "daily-supply-refresh",
    nodes: [
      {
        id: "sync_orders",
        kind: "SOURCE_INGEST",
        label: "同步订单数据入湖",
        config: {
          dataset_id: $ds_id,
          wait: true,
          timeout_s: 300
        },
        retry: { max_attempts: 3, backoff_s: 2, backoff: "FIXED" },
        timeout_s: 300,
        on_failure: "FAIL_RUN",
        join: "ALL_SUCCESS",
        pos: { x: 40, y: 140 }
      },
      {
        id: "map_orders",
        kind: "XBLUE_MAP",
        label: "映射订单业务节点",
        config: {
          mapping_id: $map_id,
          mode: "INCREMENTAL"
        },
        retry: { max_attempts: 3, backoff_s: 2, backoff: "FIXED" },
        timeout_s: 300,
        on_failure: "FAIL_RUN",
        join: "ALL_SUCCESS",
        pos: { x: 280, y: 140 }
      },
      {
        id: "agg",
        kind: "AGG_TRIGGER",
        label: "重算仓储动态聚合指标",
        config: {
          type_name: "Warehouse"
        },
        retry: { max_attempts: 3, backoff_s: 2, backoff: "FIXED" },
        timeout_s: 300,
        on_failure: "FAIL_RUN",
        join: "ALL_SUCCESS",
        pos: { x: 520, y: 140 }
      },
      {
        id: "check",
        kind: "SQL_TRANSFORM",
        label: "核验已支付订单数",
        config: {
          sql: "SELECT count(*) AS n FROM demo_orders WHERE status = '\''PAID'\''",
          max_rows: 10
        },
        retry: { max_attempts: 3, backoff_s: 2, backoff: "FIXED" },
        timeout_s: 120,
        on_failure: "FAIL_RUN",
        join: "ALL_SUCCESS",
        pos: { x: 40, y: 320 }
      },
      {
        id: "approve",
        kind: "HUMAN_APPROVAL",
        label: "补货人工审批门禁",
        config: {
          title: "补货审批",
          detail_template: {
            warehouse: "W-1",
            paid_orders: "{{ nodes.check.output.n }}"
          },
          assignee_roles: ["ADMIN"],
          expires_in_minutes: 1440
        },
        retry: { max_attempts: 1, backoff_s: 5, backoff: "FIXED" },
        timeout_s: 3600,
        on_failure: "FAIL_RUN",
        join: "ALL_SUCCESS",
        pos: { x: 300, y: 320 }
      },
      {
        id: "emit",
        kind: "ACTION_EMIT",
        label: "下发补货指标回写",
        config: {
          actions: [
            {
              kind: "WRITE_NODE_METRIC",
              node_ref: $wh_id,
              metric: "replenish_flag",
              value: 1
            }
          ]
        },
        retry: { max_attempts: 3, backoff_s: 2, backoff: "FIXED" },
        timeout_s: 120,
        on_failure: "FAIL_RUN",
        join: "ALL_SUCCESS",
        pos: { x: 560, y: 320 }
      }
    ],
    edges: [
      { from: "sync_orders", to: "map_orders" },
      { from: "map_orders", to: "agg" },
      { from: "agg", to: "check" },
      { from: "check", to: "approve", when: "n > 0" },
      { from: "approve", to: "emit" }
    ],
    trigger_cfg: {
      type: "MANUAL"
    }
  }')

FLOW_RESP=$(api POST /api/axflow/flows "$FLOW_PAYLOAD")
FLOW_ID=$(printf '%s' "$FLOW_RESP" | jq -r '.id // empty')
if [[ -z "$FLOW_ID" ]]; then
  echo "seed 07-axflow failed creating daily_supply_refresh: $FLOW_RESP" >&2
  exit 1
fi

api POST "/api/axflow/flows/$FLOW_ID/versions" '{"note":"initial supply replenishment flow v1"}' >/dev/null
api POST "/api/axflow/flows/$FLOW_ID/deploy" '{"version":1,"environment":"TEST"}' >/dev/null
api POST "/api/axflow/flows/$FLOW_ID/deploy" '{"version":1,"environment":"PROD"}' >/dev/null

# 2. Create cyclic_demo draft with a -> b -> a cycle (intentionally unpublished for cycle validation gate)
CYCLIC_PAYLOAD='{
  "name": "cyclic_demo",
  "description": "环检测门禁验证草稿（包含 a -> b -> a 有向环）",
  "nodes": [
    {
      "id": "a",
      "kind": "SQL_TRANSFORM",
      "label": "节点 A",
      "config": { "sql": "SELECT 1 AS n" },
      "pos": { "x": 80, "y": 140 }
    },
    {
      "id": "b",
      "kind": "SQL_TRANSFORM",
      "label": "节点 B",
      "config": { "sql": "SELECT 2 AS n" },
      "pos": { "x": 320, "y": 140 }
    }
  ],
  "edges": [
    { "from": "a", "to": "b" },
    { "from": "b", "to": "a" }
  ],
  "trigger_cfg": {
    "type": "MANUAL"
  }
}'

CYCLIC_RESP=$(api POST /api/axflow/flows "$CYCLIC_PAYLOAD")
CYCLIC_FLOW_ID=$(printf '%s' "$CYCLIC_RESP" | jq -r '.id // empty')
if [[ -z "$CYCLIC_FLOW_ID" ]]; then
  echo "seed 07-axflow failed creating cyclic_demo: $CYCLIC_RESP" >&2
  exit 1
fi

echo "FLOW_ID=$FLOW_ID"
echo "CYCLIC_FLOW_ID=$CYCLIC_FLOW_ID"
