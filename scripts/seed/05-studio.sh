#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "$0")/../gate/lib.sh"

# 1. Publish full main schema with Factory, Warehouse (agg + derived rules), Order, SUPPLIES/FULFILLS, and orders_to_order_nodes mapping
SCHEMA_PAYLOAD='{
  "node_types": [
    {
      "name": "Factory",
      "display_name": "工厂",
      "description": "上游制造与封测基地",
      "icon": "factory",
      "color": "#4f46e5",
      "title_template": "{node_key}",
      "properties": [
        {"name": "region", "display_name": "区域", "data_type": "STRING", "kind": "ATTRIBUTE", "required": false, "pii_category": "NONE"},
        {"name": "city", "display_name": "城市", "data_type": "STRING", "kind": "ATTRIBUTE", "required": false, "pii_category": "NONE"},
        {"name": "alias", "display_name": "别名", "data_type": "STRING", "kind": "ATTRIBUTE", "required": false, "pii_category": "NONE"},
        {"name": "output", "display_name": "日产能", "unit": "件", "data_type": "FLOAT64", "kind": "METRIC", "required": false, "pii_category": "NONE"}
      ],
      "agg_rules": [],
      "derived_rules": [],
      "sync_to_knowledge": false,
      "pos_x": 80,
      "pos_y": 140
    },
    {
      "name": "Warehouse",
      "display_name": "仓库",
      "description": "区域履约仓储中心",
      "icon": "warehouse",
      "color": "#2563eb",
      "title_template": "{node_key}",
      "properties": [
        {"name": "region", "display_name": "区域", "data_type": "STRING", "kind": "ATTRIBUTE", "required": false, "pii_category": "NONE"},
        {"name": "city", "display_name": "城市", "data_type": "STRING", "kind": "ATTRIBUTE", "required": false, "pii_category": "NONE"},
        {"name": "inventory", "display_name": "当前库存", "unit": "件", "data_type": "FLOAT64", "kind": "METRIC", "required": false, "pii_category": "NONE"},
        {"name": "capacity", "display_name": "额定仓容", "unit": "件", "data_type": "FLOAT64", "kind": "METRIC", "required": false, "pii_category": "NONE"},
        {"name": "total_demand", "display_name": "订单需求汇总", "unit": "CNY", "data_type": "FLOAT64", "kind": "METRIC", "required": false, "pii_category": "NONE"},
        {"name": "fill_rate", "display_name": "库存满足率", "data_type": "FLOAT64", "kind": "METRIC", "required": false, "pii_category": "NONE"}
      ],
      "agg_rules": [
        {
          "target_metric": "total_demand",
          "op": "SUM",
          "via_link_type": "FULFILLS",
          "direction": "OUT",
          "source_type_name": "Order",
          "source_metric": "demand",
          "weight_by_link": false
        }
      ],
      "derived_rules": [
        {
          "target_metric": "fill_rate",
          "expression": "if(total_demand > 0, inventory / total_demand, null)",
          "enabled": true
        }
      ],
      "sync_to_knowledge": true,
      "pos_x": 360,
      "pos_y": 140
    },
    {
      "name": "Order",
      "display_name": "订单",
      "description": "客户销售履约订单",
      "icon": "shopping-cart",
      "color": "#16a34a",
      "title_template": "订单 {node_key}",
      "properties": [
        {"name": "status", "display_name": "订单状态", "data_type": "STRING", "kind": "ATTRIBUTE", "required": false, "pii_category": "NONE"},
        {"name": "customer", "display_name": "客户名称", "data_type": "STRING", "kind": "ATTRIBUTE", "required": false, "pii_category": "NONE"},
        {"name": "priority", "display_name": "优先级", "data_type": "STRING", "kind": "ATTRIBUTE", "required": false, "pii_category": "NONE"},
        {"name": "demand", "display_name": "订单需求金额", "unit": "CNY", "data_type": "FLOAT64", "kind": "METRIC", "required": false, "pii_category": "NONE"}
      ],
      "agg_rules": [],
      "derived_rules": [],
      "sync_to_knowledge": false,
      "pos_x": 640,
      "pos_y": 140
    }
  ],
  "edge_types": [
    {
      "name": "SUPPLIES",
      "src_type_name": "Factory",
      "dst_type_name": "Warehouse",
      "impact_weight": 0.85,
      "cardinality": "MANY_TO_MANY"
    },
    {
      "name": "FULFILLS",
      "src_type_name": "Warehouse",
      "dst_type_name": "Order",
      "impact_weight": 0.8,
      "cardinality": "ONE_TO_MANY"
    }
  ],
  "mappings": [
    {
      "name": "orders_to_order_nodes",
      "dataset": "demo_orders",
      "node_type": "Order",
      "key_column": "order_id",
      "title": "订单 {order_id}",
      "attributes": {
        "status": "status",
        "customer": "customer"
      },
      "metrics": {
        "demand": "amount"
      },
      "metric_time_column": "updated_at",
      "links": [
        {
          "link_type": "FULFILLS",
          "direction": "IN",
          "target_type": "Warehouse",
          "target_key_column": "warehouse_code",
          "create_missing_target": false
        }
      ],
      "on_source_delete": "KEEP",
      "mode": "INCREMENTAL"
    }
  ]
}'

PUB_RESP=$(api POST /api/xblue/schema "$SCHEMA_PAYLOAD")
SCHEMA_VER=$(printf '%s' "$PUB_RESP" | jq -r .version)
MAPPING_ID=$(api GET /api/xblue/mappings | jq -r '.items[] | select(.name=="orders_to_order_nodes") | .id')

# 2. Restore W-1 snapshot inventory=500 & capacity=900 (preserving xb_metric_series hourly points from 03-axgraph.sh)
api PATCH "/api/xblue/nodes/$WAREHOUSE_NODE_ID" '{"metrics":{"inventory":500,"capacity":900}}' >/dev/null
wait_mapping_idle "$MAPPING_ID"
wait_aggregate_idle

echo "SCHEMA_VERSION=$SCHEMA_VER"
echo "ORDER_MAPPING_ID=$MAPPING_ID"
