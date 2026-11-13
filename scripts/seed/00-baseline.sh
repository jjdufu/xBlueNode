#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "$0")/../gate/lib.sh"
DOC=$(api POST /api/documents/ingest '{"title":"供应链制度","source_name":"policy.md","content":"华东仓（W-1）负责履约订单 O-1 与 O-2。晶圆一厂又称 Fab-1。"}' | jq -r .doc_id)
api POST /api/xblue/schema '{"node_types":[{"name":"Warehouse","display_name":"仓库"},{"name":"Order","display_name":"订单"}],"edge_types":[{"name":"FULFILLS","src_type_name":"Warehouse","dst_type_name":"Order","impact_weight":0.8}]}' >/dev/null
W1=$(api POST /api/xblue/nodes '{"type_name":"Warehouse","node_key":"W-1","title":"华东仓","metrics":{"inventory":500}}' | jq -r .id)
O1=$(api POST /api/xblue/nodes "{\"type_name\":\"Order\",\"node_key\":\"O-1\",\"title\":\"订单 O-1\",\"metrics\":{\"demand\":120},\"link_to_node_id\":\"$W1\",\"link_type\":\"FULFILLS\"}" | jq -r .id)
O2=$(api POST /api/xblue/nodes "{\"type_name\":\"Order\",\"node_key\":\"O-2\",\"title\":\"订单 O-2\",\"metrics\":{\"demand\":80},\"link_to_node_id\":\"$W1\",\"link_type\":\"FULFILLS\"}" | jq -r .id)
echo "DOC_ID=$DOC"; echo "WAREHOUSE_NODE_ID=$W1"; echo "ORDER1_NODE_ID=$O1"; echo "ORDER2_NODE_ID=$O2"
