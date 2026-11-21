#!/usr/bin/env bash
# Step 13 seed: departments, local users, attributes, dataset classification, and row policy (§13.11).
set -euo pipefail
source "$(dirname "$0")/../gate/lib.sh"

"$ROOT/scripts/ops/gen-jwt-key.sh" k1 >/dev/null

OPS_PW="Axp#Ops_2026!99"
EAST_PW="Axp#East_2026!88"
SOUTH_PW="Axp#South_2026!77"
AUDITOR_PW="Axp#Audit_2026!66"

# 1. Departments east & south
api PUT /api/axguard/departments/east '{"name":"华东事业部"}' >/dev/null
api PUT /api/axguard/departments/south '{"name":"华南事业部"}' >/dev/null

# Helper: create or reset local user idempotently
seed_user() {
  local uname=$1 pw=$2 dept=$3 clearance=$4 role=$5 attrs_json=$6
  local existing_id
  existing_id=$(api GET /api/axguard/users | jq -r --arg u "$uname" '.items // . | map(select(.username == $u))[0].id // empty')
  if [[ -z "$existing_id" ]]; then
    local payload
    payload=$(jq -nc \
      --arg u "$uname" \
      --arg p "$pw" \
      --arg d "$dept" \
      --arg c "$clearance" \
      --arg r "$role" \
      --argjson attrs "$attrs_json" \
      '{username:$u, display_name:$u, password:$p, dept_id:$d, clearance:$c, roles:[$r], attributes:$attrs}')
    api POST /api/axguard/users "$payload" >/dev/null
  else
    api PATCH "/api/axguard/users/$existing_id" \
      "$(jq -nc --arg d "$dept" --arg c "$clearance" --arg r "$role" '{dept_id:$d, clearance:$c, status:"ACTIVE", roles:[$r], unlock:true}')" >/dev/null
    api POST "/api/axguard/users/$existing_id/reset-password" \
      "$(jq -nc --arg p "$pw" '{new_password:$p}')" >/dev/null
    api PUT "/api/axguard/users/$existing_id/attributes" \
      "$(jq -nc --argjson attrs "$attrs_json" '{attributes:$attrs}')" >/dev/null
  fi
}

seed_user "ops_admin"     "$OPS_PW"     "east"    "SECRET"       "ADMIN"   '{}'
seed_user "analyst_east"  "$EAST_PW"    "east"    "CONFIDENTIAL" "ANALYST" '{"warehouses":["W-1"]}'
seed_user "analyst_south" "$SOUTH_PW"   "south"   "CONFIDENTIAL" "ANALYST" '{}'
seed_user "auditor"       "$AUDITOR_PW" "default" "INTERNAL"     "VIEWER"  '{}'

# 2. Classify demo_orders dataset as owner_dept='east', sensitivity='CONFIDENTIAL'
if [[ -z "${DEMO_ORDERS_DATASET_ID:-}" ]]; then
  DEMO_ORDERS_DATASET_ID=$(sql_app default "SELECT id FROM ds_datasets WHERE name='demo_orders' LIMIT 1")
fi
sql_owner "UPDATE ds_datasets SET owner_dept='east', sensitivity='CONFIDENTIAL' WHERE id='$DEMO_ORDERS_DATASET_ID'" >/dev/null

# Ensure phone column has pii_category='PHONE' and no overriding NONE column policy
api PUT "/api/axhub/datasets/$DEMO_ORDERS_DATASET_ID/columns/phone" \
  '{"pii_category":"PHONE","description":"客户联系电话"}' >/dev/null

# 3. Row policy east_warehouses: warehouse_code IN {{user.attr.warehouses}} for ANALYST
EXISTING_RP=$(api GET /api/axguard/row-policies | jq -r '.items // . | map(select(.name == "east_warehouses"))[0].id // empty')
RP_PAYLOAD=$(jq -nc --arg dsid "$DEMO_ORDERS_DATASET_ID" '{
  name: "east_warehouses",
  target_kind: "DATASET",
  target_ref: $dsid,
  applies_to: { roles: ["ANALYST"] },
  predicate: "warehouse_code IN {{user.attr.warehouses}}",
  enabled: true
}')
if [[ -n "$EXISTING_RP" ]]; then
  api PUT "/api/axguard/row-policies/$EXISTING_RP" "$RP_PAYLOAD" >/dev/null
else
  api POST /api/axguard/row-policies "$RP_PAYLOAD" >/dev/null
fi

# 4. Resolve saved query orders_by_status ID
if [[ -z "${ORDERS_BY_STATUS_QUERY_ID:-}" ]]; then
  ORDERS_BY_STATUS_QUERY_ID=$(sql_app default "SELECT id FROM an_saved_queries WHERE name='orders_by_status' ORDER BY created_at DESC LIMIT 1")
fi

echo "OPS_ADMIN_PASSWORD=$OPS_PW"
echo "EAST_PASSWORD=$EAST_PW"
echo "SOUTH_PASSWORD=$SOUTH_PW"
echo "AUDITOR_PASSWORD=$AUDITOR_PW"
echo "ORDERS_BY_STATUS_QUERY_ID=$ORDERS_BY_STATUS_QUERY_ID"
