#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "$0")/lib.sh"

# 1. 单元与文档测试：覆盖深链解析、BUILDER 令牌裁剪、MCP 构建态工具调度与子图节点隔离
(cd axpoint-core && cargo test -p axpoint-domain builder --locked && cargo test -p axpoint-security --locked && cargo test -p axpoint-agent mcp --locked)

# 2. 签发受限 BUILDER 令牌（带自定义 web_base_url 验证深链回传）
BUILDER_WEB_URL="https://studio.xblue.example"
TOK_RES=$(api POST /api/auth/tokens "{
  \"name\": \"gate-step17-builder\",
  \"expires_in_days\": 7,
  \"token_profile\": \"BUILDER\",
  \"web_base_url\": \"$BUILDER_WEB_URL\",
  \"scopes\": [\"DATA_READ\", \"DATA_SYNC\", \"STUDIO_EDIT\", \"FLOW_EDIT\", \"VIEW_EDIT\"]
}")
BUILDER_PAT=$(echo "$TOK_RES" | jq -r .token)
BUILDER_PAT_ID=$(echo "$TOK_RES" | jq -r .id)
expect_eq "builder-profile-saved" "$(echo "$TOK_RES" | jq -r .token_profile)" "BUILDER"

DEV_SLUG="dev_step17_sbx"
TPL_SLUG="dev_step17_tpl"

cleanup_step17() {
  api DELETE "/api/xblue/subgraphs/$DEV_SLUG" >/dev/null 2>&1 || true
  api DELETE "/api/xblue/subgraphs/$TPL_SLUG" >/dev/null 2>&1 || true
  [[ -n "$BUILDER_PAT_ID" && "$BUILDER_PAT_ID" != "null" ]] && api DELETE "/api/auth/tokens/$BUILDER_PAT_ID" >/dev/null 2>&1 || true
}
trap cleanup_step17 EXIT

# 3. 初始化 MCP 会话并验证构建态工具可见性与预检
mkdir -p .gate
curl -sS -X POST "$CORE_BASE_URL/api/mcp" \
  -H "Authorization: Bearer $BUILDER_PAT" \
  -H "Accept: application/json, text/event-stream" \
  -H "Content-Type: application/json" \
  -D .gate/mcp17.h \
  -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"gate-axbuilder","version":"1.0"}}}' >/dev/null
SID17=$(awk 'tolower($1)=="mcp-session-id:" {gsub(/\r/,"",$2); print $2}' .gate/mcp17.h)

bmcp() {
  curl -sS -X POST "$CORE_BASE_URL/api/mcp" \
    -H "Authorization: Bearer $BUILDER_PAT" \
    -H "Accept: application/json, text/event-stream" \
    -H "Content-Type: application/json" \
    -H "Mcp-Session-Id: $SID17" \
    -d "$1" | sed -n 's/^data: //p;/^{/p' | tail -1
}

bmcp '{"jsonrpc":"2.0","method":"notifications/initialized"}' >/dev/null || true

PREFLIGHT_RPC=$(bmcp '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"axbuilder_preflight","arguments":{"subgraph_slug":"main"}}}')
PREFLIGHT_BODY=$(echo "$PREFLIGHT_RPC" | jq -r '.result.content[0].text')
expect_json "mcp-axbuilder-preflight" "$PREFLIGHT_BODY" \
  '.healthy == true and .token_profile == "BUILDER" and (.subgraphs | length) >= 1'

# 4. 创建开发沙盒子图并通过 MCP 推送模型草案 + 核验前端深链回传
api DELETE "/api/xblue/subgraphs/$DEV_SLUG" >/dev/null 2>&1 || true
api POST /api/xblue/subgraphs "{\"slug\":\"$DEV_SLUG\",\"display_name\":\"Step17 开发沙盒\",\"is_dev_sandbox\":true}" >/dev/null

PUSH_RPC=$(bmcp "$(jq -nc --arg slug "$DEV_SLUG" '{
  jsonrpc: "2.0",
  id: 3,
  method: "tools/call",
  params: {
    name: "axbuilder_push_schema_draft",
    arguments: {
      subgraph_slug: $slug,
      title: "MCP 构建草案验证",
      schema_doc: {
        node_types: [{
          name: "SandboxHub",
          display_name: "沙盒枢纽",
          properties: [
            { name: "region", display_name: "区域", data_type: "STRING", kind: "ATTRIBUTE", required: true },
            { name: "stock_qty", display_name: "库存量", unit: "件", data_type: "FLOAT64", kind: "METRIC", required: false }
          ]
        }],
        edge_types: [],
        mappings: []
      }
    }
  }
}')")
PUSH_BODY=$(echo "$PUSH_RPC" | jq -r '.result.content[0].text')
DRAFT_ID=$(echo "$PUSH_BODY" | jq -r .draft_id)
expect_json "mcp-push-draft" "$PUSH_BODY" \
  '.status == "DRAFT" and .error_count == 0 and (.draft_id | length) > 10'
expect_json "mcp-deeplink" "$PUSH_BODY" \
  "(.deep_links.studio_url | startswith(\"$BUILDER_WEB_URL/studio?subgraph=$DEV_SLUG&draft=$DRAFT_ID\"))"

# 核验 BUILDER_MCP_MUTATE 审计动作落盘
expect_eq "builder-mcp-audit" \
  "$(sql_app default "SELECT count(*) > 0 FROM sec_audit_logs WHERE action = 'BUILDER_MCP_MUTATE'")" "t"

# 5. 验证 BUILDER 令牌严禁直接发布生产模型版本或部署 PROD 资产
PUB_CODE=$(curl -sS -o /dev/null -w '%{http_code}' -X POST \
  -H "Authorization: Bearer $BUILDER_PAT" -H "Content-Type: application/json" \
  "$CORE_BASE_URL/api/xblue/drafts/$DRAFT_ID/publish" -d '{"change_summary":"attempt prod publish"}')
expect_eq "builder-token-no-prod" "$PUB_CODE" "403"

# 6. 验证开发沙盒子图试灌与清空（且企业主图 main 拒绝清空、主图节点数零变化）
MAIN_BEFORE=$(sql_app default "SELECT count(*) FROM xb_nodes WHERE subgraph_slug = 'main'")
expect_code "main-reset-rejected" 400 POST "/api/xblue/subgraphs/main/reset-nodes" '{}'

api DELETE "/api/xblue/subgraphs/$TPL_SLUG" >/dev/null 2>&1 || true
CLONE_RPC=$(bmcp '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"axbuilder_clone_template","arguments":{"pack_id":"supply_chain","target_slug":"dev_step17_tpl"}}}')
CLONE_BODY=$(echo "$CLONE_RPC" | jq -r '.result.content[0].text')
TPL_SLUG=$(echo "$CLONE_BODY" | jq -r .subgraph_slug)

RUN_RPC=$(bmcp "$(jq -nc --arg slug "$TPL_SLUG" '{
  jsonrpc: "2.0",
  id: 5,
  method: "tools/call",
  params: { name: "axbuilder_run_dev_subgraph", arguments: { subgraph_slug: $slug, limit_rows: 100 } }
}')")
RUN_BODY=$(echo "$RUN_RPC" | jq -r '.result.content[0].text')
expect_json "dev-subgraph-populated" "$RUN_BODY" '.nodes_upserted > 0'

RESET_RES=$(api POST "/api/xblue/subgraphs/$TPL_SLUG/reset-nodes" '{}')
MAIN_AFTER=$(sql_app default "SELECT count(*) FROM xb_nodes WHERE subgraph_slug = 'main'")
SBX_REMAIN=$(sql_app default "SELECT count(*) FROM xb_nodes WHERE subgraph_slug = '$TPL_SLUG'")
expect_json "dev-subgraph-reset" "$RESET_RES" \
  ".nodes_deleted > 0 and (.last_reset_at != null) and ($SBX_REMAIN == 0) and ($MAIN_BEFORE == $MAIN_AFTER)"

# 7. 跨租户 RLS 隔离断言
expect_eq "xb-subgraphs-rls" "$(sql_app other "SELECT count(*) FROM xb_subgraphs WHERE slug = '$DEV_SLUG'")" "0"

gate_pass step17
