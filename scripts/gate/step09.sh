#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "$0")/lib.sh"

(cd axpoint-core && cargo test -p axpoint-agent && cargo test -p axpoint-llm router && cargo test -p axpoint-llm cassette)

PUB="$CORE_BASE_URL/api/published/prod/default/$PAID_ENDPOINT_SLUG"
OBJ_PUB="$CORE_BASE_URL/api/published/prod/default/$OBJECT_ENDPOINT_SLUG"
pat() { curl -s -H "Authorization: Bearer $GATE_PAT" -H "Content-Type: application/json" "$@"; }

# 发布端点：令牌鉴权、入参校验、指标与业务对象结果
expect_eq   "pub-noauth" "$(curl -s -o /dev/null -w '%{http_code}' -X POST "$PUB" -d '{}')" "401"
expect_json "pub-invalid" "$(pat -X POST "$PUB" -d '{"grain":"WEEKLY"}')" '.error_code == "SCHEMA_INVALID"'
expect_json "pub-ok"      "$(pat -X POST "$PUB" -d '{"dimensions":["status"],"grain":"MONTH"}')" '(.rows | length) >= 1'
expect_json "published-object" "$(pat -X POST "$OBJ_PUB" -d '{"keys":["W-1"],"include_links":["FULFILLS"]}')" '(.items | length) == 1 and .items[0].node_key == "W-1"'
expect_json "openapi"     "$(api GET '/api/axagent/endpoints/openapi.json?environment=PROD')" '.paths | keys | (any(test("paid-amount-by-status")) and any(test("warehouse-objects")))'

# PROD 固定版本：修改指标后 PROD 结果不变，TEST 跟随
BEFORE=$(pat -X POST "$PUB" -d '{"dimensions":[],"grain":"MONTH"}' | jq -c .rows)
api PATCH /api/analytics/metrics/paid_amount '{"filter_sql":"status = '\''PENDING'\''"}' >/dev/null
expect_eq "prod-pinned" "$(pat -X POST "$PUB" -d '{"dimensions":[],"grain":"MONTH"}' | jq -c .rows)" "$BEFORE"
api PATCH /api/analytics/metrics/paid_amount '{"filter_sql":"status = '\''PAID'\''"}' >/dev/null

# MCP：初始化、工具列表、调用
INIT=$(pat -X POST "$CORE_BASE_URL/api/mcp" -H "Accept: application/json, text/event-stream" -D .gate/mcp.h \
  -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"gate","version":"1"}}}')
SID=$(awk 'tolower($1)=="mcp-session-id:" {gsub(/\r/,"",$2); print $2}' .gate/mcp.h)
mcp() { pat -X POST "$CORE_BASE_URL/api/mcp" -H "Accept: application/json, text/event-stream" -H "Mcp-Session-Id: $SID" -d "$1" | sed -n 's/^data: //p;/^{/p' | tail -1; }
mcp '{"jsonrpc":"2.0","method":"notifications/initialized"}' >/dev/null || true
expect_json "mcp-tools" "$(mcp '{"jsonrpc":"2.0","id":2,"method":"tools/list"}')" '[.result.tools[].name] | (index("paid_amount_by_status") != null and index("warehouse_objects") != null and index("query_objects") != null)'
expect_json "mcp-call"  "$(mcp '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"paid_amount_by_status","arguments":{"dimensions":["status"],"grain":"MONTH"}}}')" '.result.isError != true'
expect_eq "mcp-audit" "$(sql_app default "SELECT count(*) > 0 FROM sec_audit_logs WHERE action='MCP_CALL'")" "t"

# 智能体（录制响应）：多智能体协作产生专家发言
CONV=$(api POST /api/axagent/agents/$SUPERVISOR_AGENT_ID/chat '{"message":"已支付订单总金额是多少？依据哪份制度补货？"}' | jq -r .conversation_id)
expect_json "multi-agent" "$(api GET /api/axagent/conversations/$CONV/messages)" '[.items[].agent_name] | (index("data_analyst") != null and index("knowledge_expert") != null)'

# 嵌入链接：令牌外部访问、撤销后失效
EMB=$(api POST /api/axagent/agents/$SUPERVISOR_AGENT_ID/embeds '{"allowed_origins":[],"permissions":["DATA_READ","AGENT_RUN"]}')
ETOKEN=$(echo "$EMB" | jq -r .token); EID=$(echo "$EMB" | jq -r .id)
expect_code "embed-ok" 200 POST_NOAUTH "/api/axagent/embed/$ETOKEN/chat" '{"message":"你好"}'
api DELETE /api/axagent/embeds/$EID >/dev/null
expect_code "embed-revoked" 404 POST_NOAUTH "/api/axagent/embed/$ETOKEN/chat" '{"message":"你好"}'

gate_pass step09
