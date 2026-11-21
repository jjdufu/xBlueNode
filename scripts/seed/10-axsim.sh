#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "$0")/../gate/lib.sh"

# Seed Step 10 AxSim causal topology + demo sandbox (`09-STEP10` §10.12).
# Idempotent: reuses existing F-1 / W-1 / O-1 / O-2 nodes and demo sandbox if present.

F1_ID=$(sql_app default "SELECT id FROM xb_nodes WHERE node_key = 'F-1' LIMIT 1")
W1_ID=$(sql_app default "SELECT id FROM xb_nodes WHERE node_key = 'W-1' LIMIT 1")
O1_ID=$(sql_app default "SELECT id FROM xb_nodes WHERE node_key = 'O-1' LIMIT 1")
O2_ID=$(sql_app default "SELECT id FROM xb_nodes WHERE node_key = 'O-2' LIMIT 1")

if [[ -z "$F1_ID" ]]; then
  F1_ID=$(api POST /api/xblue/nodes '{"type_name":"Factory","node_key":"F-1","title":"晶圆一厂 F-1","attributes":{"region":"华东"},"metrics":{"output":1000.0}}' | jq -r '.id')
else
  api PATCH "/api/xblue/nodes/$F1_ID" '{"metrics":{"output":1000.0}}' >/dev/null || true
fi

if [[ -z "$W1_ID" ]]; then
  W1_ID=$(api POST /api/xblue/nodes '{"type_name":"Warehouse","node_key":"W-1","title":"华东中心仓 W-1","attributes":{"region":"华东"},"metrics":{"inventory":500.0}}' | jq -r '.id')
else
  api PATCH "/api/xblue/nodes/$W1_ID" '{"metrics":{"inventory":500.0}}' >/dev/null || true
fi

if [[ -z "$O1_ID" ]]; then
  O1_ID=$(api POST /api/xblue/nodes '{"type_name":"Order","node_key":"O-1","title":"订单 O-1","metrics":{"fulfillment":100.0,"demand":100.0}}' | jq -r '.id')
else
  api PATCH "/api/xblue/nodes/$O1_ID" '{"metrics":{"fulfillment":100.0}}' >/dev/null || true
fi

if [[ -z "$O2_ID" ]]; then
  O2_ID=$(api POST /api/xblue/nodes '{"type_name":"Order","node_key":"O-2","title":"订单 O-2","metrics":{"fulfillment":120.0,"demand":120.0}}' | jq -r '.id')
else
  api PATCH "/api/xblue/nodes/$O2_ID" '{"metrics":{"fulfillment":120.0}}' >/dev/null || true
fi

for var_name in F1_ID W1_ID O1_ID O2_ID; do
  val="${!var_name:-}"
  if [[ -z "$val" || "$val" == "null" ]]; then
    echo "10-axsim.sh: failed to seed required node variable $var_name (got '$val')" >&2
    exit 1
  fi
done

api POST /api/xblue/links "{\"link_type\":\"SUPPLIES\",\"src_node_id\":\"$F1_ID\",\"dst_node_id\":\"$W1_ID\",\"weight\":0.8}" >/dev/null || true
api POST /api/xblue/links "{\"link_type\":\"FULFILLS\",\"src_node_id\":\"$W1_ID\",\"dst_node_id\":\"$O1_ID\",\"weight\":0.9}" >/dev/null || true
api POST /api/xblue/links "{\"link_type\":\"FULFILLS\",\"src_node_id\":\"$W1_ID\",\"dst_node_id\":\"$O2_ID\",\"weight\":0.9}" >/dev/null || true

# Ensure edge propagation specs exist on published edge_types for multi-hop causal propagation.
sql_owner "
UPDATE xb_edge_types
   SET propagation = '{\"rules\":[{\"src_metric\":\"output\",\"dst_metric\":\"inventory\",\"mode\":\"RATIO\",\"coefficient\":0.5,\"lag_steps\":1}]}'::jsonb
 WHERE tenant_id = 'default' AND name = 'SUPPLIES';
UPDATE xb_edge_types
   SET propagation = '{\"rules\":[{\"src_metric\":\"inventory\",\"dst_metric\":\"fulfillment\",\"mode\":\"RATIO\",\"coefficient\":0.2,\"lag_steps\":1}]}'::jsonb
 WHERE tenant_id = 'default' AND name = 'FULFILLS';
" >/dev/null

SANDBOX_ID=$(api GET /api/axsim/sandboxes | jq -r '.[] | select(.name=="供应链抗压演练沙盘") | .id' | head -n 1)
if [[ -z "$SANDBOX_ID" || "$SANDBOX_ID" == "null" ]]; then
  SANDBOX_ID=$(api POST /api/axsim/sandboxes "{\"name\":\"供应链抗压演练沙盘\",\"scope\":{\"root_node_ids\":[\"$F1_ID\"],\"depth\":4,\"direction\":\"OUT\"}}" | jq -r '.id')
else
  api POST "/api/axsim/sandboxes/$SANDBOX_ID/reset" '{}' >/dev/null || true
  api POST "/api/axsim/sandboxes/$SANDBOX_ID/refresh-base" '{}' >/dev/null || true
fi

if [[ -z "$SANDBOX_ID" || "$SANDBOX_ID" == "null" ]]; then
  echo "10-axsim.sh: failed to seed required variable SANDBOX_ID (got '$SANDBOX_ID')" >&2
  exit 1
fi

SCENARIOS_JSON=$(api GET "/api/axsim/sandboxes/$SANDBOX_ID/scenarios" 2>/dev/null || echo "[]")
SIM_S1_ID=$(printf '%s' "$SCENARIOS_JSON" | jq -r 'if type == "array" then (.[] | select(.name=="产能下调15%保守方案") | .id) else empty end' | head -n 1)
if [[ -z "$SIM_S1_ID" || "$SIM_S1_ID" == "null" ]]; then
  SIM_S1_ID=$(api POST "/api/axsim/sandboxes/$SANDBOX_ID/scenarios" "{\"name\":\"产能下调15%保守方案\",\"actions\":[{\"target_node_id\":\"$F1_ID\",\"metric_key\":\"output\",\"mode\":\"RATIO\",\"value\":-0.15}]}" | jq -r '.id')
fi

SIM_S2_ID=$(printf '%s' "$SCENARIOS_JSON" | jq -r 'if type == "array" then (.[] | select(.name=="产能扩充10%保供方案") | .id) else empty end' | head -n 1)
if [[ -z "$SIM_S2_ID" || "$SIM_S2_ID" == "null" ]]; then
  SIM_S2_ID=$(api POST "/api/axsim/sandboxes/$SANDBOX_ID/scenarios" "{\"name\":\"产能扩充10%保供方案\",\"actions\":[{\"target_node_id\":\"$F1_ID\",\"metric_key\":\"output\",\"mode\":\"RATIO\",\"value\":0.10}]}" | jq -r '.id')
fi

for var_name in F1_ID W1_ID O1_ID O2_ID SANDBOX_ID SIM_S1_ID SIM_S2_ID; do
  val="${!var_name:-}"
  if [[ -z "$val" || "$val" == "null" ]]; then
    echo "10-axsim.sh: failed to seed required variable $var_name (got '$val')" >&2
    exit 1
  fi
done

wait_scenario "$SIM_S1_ID"
wait_scenario "$SIM_S2_ID"

echo "FACTORY1_NODE_ID=$F1_ID"
echo "WAREHOUSE_NODE_ID=$W1_ID"
echo "ORDER1_NODE_ID=$O1_ID"
echo "ORDER2_NODE_ID=$O2_ID"
echo "SANDBOX_ID=$SANDBOX_ID"
echo "SIM_S1_ID=$SIM_S1_ID"
echo "SIM_S2_ID=$SIM_S2_ID"
