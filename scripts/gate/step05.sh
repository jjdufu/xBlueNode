#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "$0")/lib.sh"

(cd axpoint-core && cargo test -p axpoint-domain studio && cargo test -p axpoint-db studio_repo && cargo test -p axpoint-llm schema_draft_eval)

# Cycle detection
CYCLIC=$(api GET /api/xblue/schema | jq '.node_types |= map(if .name=="Warehouse" then .derived_rules += [{"target_metric":"inventory","expression":"fill_rate * 2","enabled":true}] else . end)')
expect_json "cycle" "$(api POST /api/xblue/schema/validate "$CYCLIC")" '[.issues[].code] | index("CYCLE_DETECTED") != null'

# Optimistic concurrency publish conflict
D1=$(api POST /api/xblue/drafts "{\"title\":\"a\",\"schema_doc\":$(api GET /api/xblue/schema | jq .)}" | jq -r .id)
D2=$(api POST /api/xblue/drafts "{\"title\":\"b\",\"schema_doc\":$(api GET /api/xblue/schema | jq .)}" | jq -r .id)
expect_code "publish-1" 200 POST "/api/xblue/drafts/$D1/publish" '{"change_summary":"a"}'
expect_json "publish-2-conflict" "$(api POST "/api/xblue/drafts/$D2/publish" '{"change_summary":"b"}')" '.error_code == "VERSION_CONFLICT"'

# Version & diff
V=$(api GET /api/xblue/schema | jq -r .version)
expect_json "diff" "$(api GET "/api/xblue/schema/versions/1/diff/$V")" '(.node_types.changed | length) >= 1'

# Multi-subgraph isolated modeling & merge into main
PRE_MERGE_VER=$(api GET /api/xblue/schema | jq -r .version)
if [[ "$(api GET /api/xblue/schema | jq '([.node_types[].name] | index("Shipment")) != null')" == "true" ]]; then
  CLEAN_DOC=$(api GET /api/xblue/schema | jq '{node_types: (.node_types | map(select(.name != "Shipment"))), edge_types: (.edge_types | map(select(.name != "SHIPS_ORDER"))), mappings: .mappings, force: true}')
  PRE_MERGE_VER=$(api POST /api/xblue/schema "$CLEAN_DOC" | jq -r .version)
fi
api DELETE /api/xblue/subgraphs/logistics >/dev/null 2>&1 || true
api POST /api/xblue/subgraphs '{"slug":"logistics","display_name":"物流子图","owner_dept":"logistics"}' >/dev/null
SUB_DOC='{"node_types":[{"name":"Shipment","display_name":"运单","properties":[{"name":"carrier","display_name":"承运商","data_type":"STRING","kind":"ATTRIBUTE","required":false,"pii_category":"NONE"}],"agg_rules":[],"derived_rules":[],"sync_to_knowledge":false,"pos_x":420,"pos_y":160}],"edge_types":[{"name":"SHIPS_ORDER","src_type_name":"Shipment","dst_type_name":"Order","impact_weight":0.7,"cardinality":"MANY_TO_ONE"}],"mappings":[]}'
SD=$(api POST '/api/xblue/drafts?subgraph=logistics' "{\"title\":\"ship\",\"schema_doc\":$SUB_DOC}" | jq -r .id)
expect_code "subgraph-publish" 200 POST "/api/xblue/drafts/$SD/publish" '{"change_summary":"init shipment"}'
expect_json "subgraph-isolated" "$(api GET /api/xblue/schema)" '([.node_types[].name] | index("Shipment")) == null'
expect_json "subgraph-preview" "$(api POST /api/xblue/subgraphs/logistics/merge-preview '{}')" '(.added_node_types | index("Shipment")) != null'
expect_code "subgraph-merge" 200 POST /api/xblue/subgraphs/logistics/merge '{"default_strategy":"EXTEND_PROPS","change_summary":"merge logistics"}'
expect_json "subgraph-merged-main" "$(api GET /api/xblue/schema)" '([.node_types[].name] | index("Shipment")) != null'
REST_ID=$(api POST "/api/xblue/schema/versions/$PRE_MERGE_VER/restore" | jq -r .id)
expect_code "subgraph-restore-clean" 200 POST "/api/xblue/drafts/$REST_ID/publish" '{"change_summary":"restore pre-merge","force":true}'
expect_code "subgraph-delete" 200 DELETE /api/xblue/subgraphs/logistics

# Breaking change requires force=true
BREAK=$(api GET /api/xblue/schema | jq '.node_types |= map(select(.name != "Order"))')
D3=$(api POST /api/xblue/drafts "{\"title\":\"break\",\"schema_doc\":$BREAK}" | jq -r .id)
expect_json "breaking" "$(api POST "/api/xblue/drafts/$D3/publish" '{"change_summary":"x"}')" '.error_code == "SCHEMA_BREAKING_CHANGE" or .error_code == "SCHEMA_INVALID"'
api POST "/api/xblue/drafts/$D3/discard" >/dev/null

# AI draft (recorded cassette response)
expect_json "ai-draft" "$(api POST /api/xblue/drafts/ai "{\"title\":\"ai\",\"dataset_ids\":[\"$DEMO_ORDERS_DATASET_ID\"]}")" '(.schema_doc.node_types | length) >= 1'

# Auto-published dynamic GraphQL & strongly-typed domain object REST API
expect_json "domain-graphql" "$(api POST /api/xblue/graphql '{"query":"{ warehouse(nodeKey: \"W-1\") { title inventory fulfillsOrders(limit: 5) { nodeKey } } }"}')" \
  '.data.warehouse.title == "华东仓" and .data.warehouse.inventory == 500 and (.data.warehouse.fulfillsOrders | length) >= 2'
expect_json "domain-objects" "$(api GET /api/xblue/objects/Warehouse/W-1?include_neighbors=true)" \
  '.type_name == "Warehouse" and .node_key == "W-1" and .metrics.inventory == 500'

gate_pass step05
