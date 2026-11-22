#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "$0")/lib.sh"

ANALYST_TOK_ID=""
TRIAL_SLUG=""
PACK_AGENT_ID=""
PACK_EP_IDS=""
DASH_ID=""

cleanup_step19() {
  if [[ -n "$DASH_ID" ]]; then
    api DELETE "/api/xblueview/dashboards/$DASH_ID" >/dev/null 2>&1 || true
  fi
  if [[ -n "$PACK_AGENT_ID" ]]; then
    api DELETE "/api/axagent/agents/$PACK_AGENT_ID" >/dev/null 2>&1 || true
  fi
  if [[ -n "$PACK_EP_IDS" ]]; then
    for ep_id in $PACK_EP_IDS; do
      api DELETE "/api/axagent/endpoints/$ep_id" >/dev/null 2>&1 || true
    done
  fi
  if [[ -n "$TRIAL_SLUG" && "$TRIAL_SLUG" != "null" && "$TRIAL_SLUG" != "main" ]]; then
    api DELETE "/api/xblue/subgraphs/$TRIAL_SLUG" >/dev/null 2>&1 || true
  fi
  if [[ -n "$ANALYST_TOK_ID" && "$ANALYST_TOK_ID" != "null" ]]; then
    api DELETE "/api/auth/tokens/$ANALYST_TOK_ID" >/dev/null 2>&1 || true
  fi
}
trap cleanup_step19 EXIT

# 清理可能残留的同名门禁看板，确保重复执行幂等
EXISTING_DASH_ID=$(api GET /api/xblueview/dashboards/by-slug/gate-step19-warehouse-cockpit | jq -r '.id // empty' 2>/dev/null || true)
if [[ -n "$EXISTING_DASH_ID" ]]; then
  api DELETE "/api/xblueview/dashboards/$EXISTING_DASH_ID" >/dev/null 2>&1 || true
fi

# 1. 单元测试：覆盖 5 步完成度状态推导、模板演练子图命名校验与智能体工具集幂等打包
(cd "$ROOT/axpoint-core" && cargo test -p axpoint-domain --locked && cargo test -p axpoint-server templates --locked)

# 2. 验证 5 步构建引导状态接口（返回固定 5 步、进度百分比与下一步路由）
ONBOARD_RES=$(api GET /api/axops/onboarding-status)
expect_json "onboarding-progress" "$ONBOARD_RES" \
  '.total_steps == 5 and (.steps | length) == 5 and .completed_steps >= 1 and (.progress_pct >= 20)'

# 3. 验证非 OPS_ADMIN 建模用户可只读预览模板并克隆隔离演练子图（零污染企业主图 main）
ANALYST_TOK_RES=$(api POST /api/auth/tokens '{
  "name": "gate-step19-studio-analyst",
  "expires_in_days": 3,
  "token_profile": "GENERAL",
  "scopes": ["DATA_READ", "STUDIO_EDIT"]
}')
ANALYST_TOK_ID=$(echo "$ANALYST_TOK_RES" | jq -r '.id // empty')
ANALYST_PAT=$(echo "$ANALYST_TOK_RES" | jq -r .token)

# 无 OPS_ADMIN 仍可访问模板列表与拓扑预览，但直接调用生产安装接口必须返回 403
expect_code "tpl-list-data-read" 200 GET "/api/axops/templates" "" "$ANALYST_PAT"
PREVIEW_RES=$(curl -sS -H "Authorization: Bearer $ANALYST_PAT" -H "Accept: application/json" \
  "$CORE_BASE_URL/api/axops/templates/supply_chain/preview")
expect_json "tpl-preview-ok" "$PREVIEW_RES" \
  '.manifest.id == "supply_chain" and (.topology_summary.node_types | length) >= 3'
expect_code "tpl-prod-install-guarded" 403 POST "/api/axops/templates/supply_chain/install" '{"dry_run":true}' "$ANALYST_PAT"

MAIN_NODES_BEFORE=$(sql_app default "SELECT count(*) FROM xb_nodes WHERE subgraph_slug = 'main'")
CLONE_RES=$(curl -sS -X POST -H "Authorization: Bearer $ANALYST_PAT" \
  -H "Accept: application/json" -H "Content-Type: application/json" \
  "$CORE_BASE_URL/api/xblue/subgraphs/from-template" \
  -d '{"pack_id":"supply_chain"}')
TRIAL_SLUG=$(echo "$CLONE_RES" | jq -r .subgraph_slug)
MAIN_NODES_AFTER=$(sql_app default "SELECT count(*) FROM xb_nodes WHERE subgraph_slug = 'main'")
TRIAL_AUDIT=$(sql_app default "SELECT count(*) > 0 FROM sec_audit_logs WHERE action = 'TRIAL_TEMPLATE_CLONE' AND resource_ref = '$TRIAL_SLUG'")

expect_json "trial-template-clone" "$CLONE_RES" \
  "(.subgraph_slug | startswith(\"trial_supply_chain_\")) and .is_dev_sandbox == true and .nodes_materialized > 0 and .links_materialized > 0 and ($MAIN_NODES_BEFORE == $MAIN_NODES_AFTER) and (\"$TRIAL_AUDIT\" == \"t\")"

# 4. 验证 Studio 一键打包领域智能体（自动暴露 OBJECT_TYPE / METRIC 发布端点并注册至 MCP）
PACK_AGENT_RES=$(api POST /api/xblue/subgraphs/main/package-agent '{
  "agent_name": "gate_step19_supply_agent",
  "description": "门禁验证主图打包智能体",
  "node_types": ["Warehouse"],
  "metric_names": ["paid_amount"],
  "environment": "PROD",
  "expose_mcp": true,
  "publish_agent": true
}')
PACK_AGENT_ID=$(echo "$PACK_AGENT_RES" | jq -r '.agent_id // empty')
PACK_EP_IDS=$(echo "$PACK_AGENT_RES" | jq -r '.endpoints_upserted[]?.id // empty' | tr '\n' ' ')
expect_json "studio-pack-agent" "$PACK_AGENT_RES" \
  '(.agent_id | length) > 10 and .agent_version >= 1 and (.endpoints_upserted | length) >= 1 and .mcp_exposed_count >= 1'

# 5. 验证 Studio 上下文驱动生成孪生驾驶舱草稿，并在子图反向消费统计中双向回显
DASH_RES=$(api POST /api/xblueview/dashboards/ai-draft '{
  "name": "门禁仓库履约孪生看板",
  "slug": "gate-step19-warehouse-cockpit",
  "prompt": "围绕 Warehouse 在库量与履约告警生成监控驾驶舱",
  "subgraph_slug": "main",
  "seed_node_types": ["Warehouse"]
}')
DASH_ID=$(echo "$DASH_RES" | jq -r '.dashboard.id // .id')

CONSUMPTION_RES=$(api GET /api/xblue/subgraphs/main/consumption)
expect_json "studio-pack-dashboard" "$CONSUMPTION_RES" \
  "(\"$DASH_ID\" | length) > 10 and .by_node_type.Warehouse.endpoint_count >= 1 and .by_node_type.Warehouse.agent_count >= 1 and .by_node_type.Warehouse.dashboard_count >= 1"

gate_pass step19
