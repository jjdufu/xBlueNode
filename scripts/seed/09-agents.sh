#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "$0")/../gate/lib.sh"

# 1. Issue Personal Access Token 'gate' with full caller permissions -> GATE_PAT
ALL_PERMS=$(api GET /api/auth/me | jq -c '.permissions')
PAT_Payload=$(jq -nc --argjson scopes "$ALL_PERMS" '{
  name: "gate",
  scopes: $scopes,
  expires_in_days: 365
}')
PAT_RESP=$(api POST /api/auth/tokens "$PAT_Payload")
GATE_PAT=$(printf '%s' "$PAT_RESP" | jq -r '.token // empty')
if [[ -z "$GATE_PAT" ]]; then
  echo "seed 09-agents failed creating GATE_PAT: $PAT_RESP" >&2
  exit 1
fi

# 2. Publish paid-amount-by-status (METRIC, TEST -> promote PROD, expose_mcp=true)
PAID_SLUG="paid-amount-by-status"
PAID_TEST_RESP=$(api POST /api/axagent/endpoints "{
  \"slug\": \"$PAID_SLUG\",
  \"environment\": \"TEST\",
  \"kind\": \"METRIC\",
  \"target_id\": \"paid_amount\",
  \"title\": \"已支付金额（按状态）\",
  \"description\": \"按订单状态统计已支付订单金额总和\",
  \"required_scope\": \"DATA_READ\",
  \"rate_limit_rpm\": 120,
  \"cache_ttl_s\": 0,
  \"expose_mcp\": true,
  \"show_as_app\": true,
  \"enabled\": true
}")
PAID_TEST_ID=$(printf '%s' "$PAID_TEST_RESP" | jq -r '.id // empty')
if [[ -z "$PAID_TEST_ID" ]]; then
  echo "seed 09-agents failed creating $PAID_SLUG TEST endpoint: $PAID_TEST_RESP" >&2
  exit 1
fi
api POST "/api/axagent/endpoints/$PAID_TEST_ID/promote" >/dev/null

# 3. Publish warehouse-objects (OBJECT_TYPE, target_id="Warehouse", TEST -> promote PROD, expose_mcp=true)
OBJ_SLUG="warehouse-objects"
OBJ_TEST_RESP=$(api POST /api/axagent/endpoints "{
  \"slug\": \"$OBJ_SLUG\",
  \"environment\": \"TEST\",
  \"kind\": \"OBJECT_TYPE\",
  \"target_id\": \"Warehouse\",
  \"title\": \"仓库业务对象查询\",
  \"description\": \"查询主图已发布仓库（Warehouse）对象及关联履约订单\",
  \"required_scope\": \"DATA_READ\",
  \"rate_limit_rpm\": 120,
  \"cache_ttl_s\": 0,
  \"expose_mcp\": true,
  \"show_as_app\": true,
  \"enabled\": true
}")
OBJ_TEST_ID=$(printf '%s' "$OBJ_TEST_RESP" | jq -r '.id // empty')
if [[ -z "$OBJ_TEST_ID" ]]; then
  echo "seed 09-agents failed creating $OBJ_SLUG TEST endpoint: $OBJ_TEST_RESP" >&2
  exit 1
fi
api POST "/api/axagent/endpoints/$OBJ_TEST_ID/promote" >/dev/null

# 4. Create & publish specialist agent: data_analyst
ANALYST_RESP=$(api POST /api/axagent/agents '{
  "name": "data_analyst",
  "description": "订单与仓储指标 OLAP 数据分析专家",
  "spec": {
    "system_prompt": "你是数据分析专家智能体（agent_id=data_analyst），负责调用 run_sql 与 ask_data 查询订单与库存数据。",
    "provider_purpose": "AGENT",
    "temperature": 0.1,
    "max_steps": 6,
    "memory_turns": 8,
    "tools": [
      {"kind": "BUILTIN", "name": "ask_data"},
      {"kind": "BUILTIN", "name": "run_sql"}
    ],
    "approval_required_tools": [],
    "knowledge_scope": {
      "doc_title_prefix": null,
      "dataset_names": ["demo_orders"]
    },
    "output_schema": null
  }
}')
ANALYST_AGENT_ID=$(printf '%s' "$ANALYST_RESP" | jq -r '.id // empty')
if [[ -z "$ANALYST_AGENT_ID" ]]; then
  echo "seed 09-agents failed creating data_analyst: $ANALYST_RESP" >&2
  exit 1
fi
api POST "/api/axagent/agents/$ANALYST_AGENT_ID/publish" >/dev/null

# 5. Create & publish specialist agent: knowledge_expert
KNOWLEDGE_RESP=$(api POST /api/axagent/agents '{
  "name": "knowledge_expert",
  "description": "供应链与仓储管理制度检索专家",
  "spec": {
    "system_prompt": "你是企业制度知识专家智能体（agent_id=knowledge_expert），负责调用 search_knowledge 检索管理制度并给出引用依据。",
    "provider_purpose": "AGENT",
    "temperature": 0.1,
    "max_steps": 6,
    "memory_turns": 8,
    "tools": [
      {"kind": "BUILTIN", "name": "search_knowledge"}
    ],
    "approval_required_tools": [],
    "knowledge_scope": {
      "doc_title_prefix": null,
      "dataset_names": []
    },
    "output_schema": null
  }
}')
KNOWLEDGE_AGENT_ID=$(printf '%s' "$KNOWLEDGE_RESP" | jq -r '.id // empty')
if [[ -z "$KNOWLEDGE_AGENT_ID" ]]; then
  echo "seed 09-agents failed creating knowledge_expert: $KNOWLEDGE_RESP" >&2
  exit 1
fi
api POST "/api/axagent/agents/$KNOWLEDGE_AGENT_ID/publish" >/dev/null

# 6. Create & publish coordinator agent: ops_supervisor
SUP_PAYLOAD=$(jq -nc \
  --arg analyst_id "$ANALYST_AGENT_ID" \
  --arg knowledge_id "$KNOWLEDGE_AGENT_ID" \
  '{
    name: "ops_supervisor",
    description: "供应链运营主管智能体，协同数据分析与制度专家回答综合问题",
    spec: {
      system_prompt: "你是供应链运营主管智能体（agent_id=ops_supervisor），请将指标统计委托给子智能体，将制度检索委托给知识专家，汇总给出准确结论。",
      provider_purpose: "AGENT",
      temperature: 0.1,
      max_steps: 8,
      memory_turns: 10,
      tools: [
        {kind: "AGENT", agent_id: $analyst_id, as_tool_name: "data_analyst"},
        {kind: "AGENT", agent_id: $knowledge_id, as_tool_name: "knowledge_expert"},
        {kind: "BUILTIN", name: "ask_data"},
        {kind: "BUILTIN", name: "search_knowledge"}
      ],
      approval_required_tools: [],
      knowledge_scope: {
        doc_title_prefix: null,
        dataset_names: ["demo_orders"]
      },
      output_schema: null
    }
  }')
SUP_RESP=$(api POST /api/axagent/agents "$SUP_PAYLOAD")
SUPERVISOR_AGENT_ID=$(printf '%s' "$SUP_RESP" | jq -r '.id // empty')
if [[ -z "$SUPERVISOR_AGENT_ID" ]]; then
  echo "seed 09-agents failed creating ops_supervisor: $SUP_RESP" >&2
  exit 1
fi
api POST "/api/axagent/agents/$SUPERVISOR_AGENT_ID/publish" >/dev/null

echo "GATE_PAT=$GATE_PAT"
echo "PAID_ENDPOINT_SLUG=$PAID_SLUG"
echo "OBJECT_ENDPOINT_SLUG=$OBJ_SLUG"
echo "ANALYST_AGENT_ID=$ANALYST_AGENT_ID"
echo "KNOWLEDGE_AGENT_ID=$KNOWLEDGE_AGENT_ID"
echo "SUPERVISOR_AGENT_ID=$SUPERVISOR_AGENT_ID"
