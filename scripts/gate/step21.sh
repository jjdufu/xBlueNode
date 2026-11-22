#!/usr/bin/env bash
# Step 21 门禁：企业 IM 出站长连接、一次性挑战码绑定、AxGuard 行级权限继承与 <45s 租约故障接管
set -euo pipefail
source "$(dirname "$0")/lib.sh"

# 1. Rust 单元与数据库集成测试
(cd "$ROOT/axpoint-core" && \
  cargo test -p axpoint-domain im --locked && \
  cargo test -p axpoint-db im_repo --locked && \
  cargo test -p axpoint-agent im_gateway --locked)

# 2. 迁移完整性与跨租户 RLS 零可见断言
expect_json "step21-migrations-clean" "$(api GET /api/axops/health/deep)" '.migrations.pending == 0'

mkdir -p "$ROOT/.gate"
MOCK_IM_PORT="${MOCK_IM_PORT:-19821}"
MOCK_IM_PID=""
CONN_ID=""
IM_AGENT_ID=""

cleanup_step21() {
  if [[ -n "$CONN_ID" ]]; then
    api DELETE "/api/axagent/im-connectors/$CONN_ID" >/dev/null 2>&1 || true
  fi
  if [[ -n "$IM_AGENT_ID" ]]; then
    api DELETE "/api/axagent/agents/$IM_AGENT_ID" >/dev/null 2>&1 || true
  fi
  sql_owner "DELETE FROM ax_im_user_bindings WHERE tenant_id = 'default' AND external_user_id = 'ou_west_user_01'" >/dev/null 2>&1 || true
  if [[ -n "$MOCK_IM_PID" ]]; then
    kill "$MOCK_IM_PID" >/dev/null 2>&1 || true
  fi
}
trap cleanup_step21 EXIT

# 启动本地确定性 IM WebSocket 回环桩
python3 "$ROOT/scripts/gate/tools/mock_im_hub.py" --port "$MOCK_IM_PORT" >"$ROOT/.gate/mock_im_hub.log" 2>&1 &
MOCK_IM_PID=$!
wait_until "mock-im-ready" 15 "curl -sf 'http://127.0.0.1:${MOCK_IM_PORT}/test/health'"

# 若未显式传入受限用户令牌，则确保加载 Step 13 受限分析员种子并签发真实非 nil 用户会话
if [[ -z "${WEST_USER_TOKEN:-}" ]]; then
  if [[ -z "${EAST_PASSWORD:-}" ]]; then
    if [[ -f "$ROOT/.gate/ids.env" ]]; then
      # shellcheck disable=SC1091
      source "$ROOT/.gate/ids.env"
    fi
  fi
  if [[ -z "${EAST_PASSWORD:-}" ]]; then
    bash "$ROOT/scripts/seed-demo.sh" 13 >/dev/null
    # shellcheck disable=SC1091
    source "$ROOT/.gate/ids.env"
  fi
  WEST_USER_TOKEN=$(curl -sS -H "Content-Type: application/json" \
    -X POST "$CORE_BASE_URL/api/auth/login" \
    -d "{\"tenant\":\"default\",\"username\":\"analyst_east\",\"password\":\"$EAST_PASSWORD\"}" | jq -r .access_token)
fi

# 清理可能残留的同名测试实体以保证可重入
OLD_CONN_ID=$(api GET /api/axagent/im-connectors | jq -r '(.items // .)[] | select(.name=="feishu_gate_21") | .id' | head -n1 || true)
if [[ -n "$OLD_CONN_ID" ]]; then
  api DELETE "/api/axagent/im-connectors/$OLD_CONN_ID" >/dev/null 2>&1 || true
fi
OLD_AGENT_ID=$(api GET /api/axagent/agents | jq -r '(.items // .)[] | select(.name=="im_gate_assistant_21") | .id' | head -n1 || true)
if [[ -n "$OLD_AGENT_ID" ]]; then
  api DELETE "/api/axagent/agents/$OLD_AGENT_ID" >/dev/null 2>&1 || true
fi
sql_owner "DELETE FROM ax_im_user_bindings WHERE tenant_id = 'default' AND external_user_id = 'ou_west_user_01'" >/dev/null 2>&1 || true

# 准备具备数据问数工具的已发布智能体
IM_AGENT_ID=$(api POST /api/axagent/agents '{
  "name": "im_gate_assistant_21",
  "description": "IM 双向联调智能体",
  "spec": {
    "system_prompt": "你是企业经营数据助手，必须调用工具查询真实数据并严格按结果作答。",
    "tools": [{"kind": "BUILTIN", "name": "ask_analytics"}, {"kind": "BUILTIN", "name": "query_sql"}],
    "max_steps": 4
  }
}' | jq -r .id)
api POST "/api/axagent/agents/$IM_AGENT_ID/publish" '{}' >/dev/null

CONN=$(api POST /api/axagent/im-connectors "{
  \"name\": \"feishu_gate_21\",
  \"platform\": \"FEISHU\",
  \"agent_id\": \"$IM_AGENT_ID\",
  \"identity_policy\": \"REQUIRE_USER_BIND\",
  \"secret\": {\"app_id\": \"cli_gate21\", \"app_secret\": \"sec_gate21\"},
  \"config\": {
    \"endpoint_override\": \"http://127.0.0.1:${MOCK_IM_PORT}\",
    \"allow_private_network\": true
  }
}")
CONN_ID=$(printf '%s' "$CONN" | jq -r .id)

# 验证跨租户零可见
expect_eq "im-rls-tenant-isolation" "$(sql_app other "SELECT count(*) FROM ax_im_connectors")" "0"

# 等待 Core 后台 ImGatewaySupervisor 认领租约并建立出站长连接
wait_until "im-ws-connected" 25 \
  "api GET /api/axagent/im-connectors | jq -e '(.items // .)[] | select(.id==\"$CONN_ID\") | .status == \"CONNECTED\" and (.leader_holder != null)'"

# 3. C75（一）：未绑定账号提问仅返回一次性绑定挑战码，绑定确认后写入审计且禁止重放
curl -sf -X POST "http://127.0.0.1:${MOCK_IM_PORT}/test/inject" \
  -H "Content-Type: application/json" \
  -d '{"event_id":"evt_bind_01","external_user_id":"ou_west_user_01","external_display_name":"王西区","chat_id":"oc_dm_01","text":"查询各区域销售额"}' >/dev/null

wait_until "im-challenge-emitted" 15 \
  "curl -sf 'http://127.0.0.1:${MOCK_IM_PORT}/test/replies' | jq -e '(.items | length) >= 1'"

CHALLENGE_REPLY=$(curl -sf "http://127.0.0.1:${MOCK_IM_PORT}/test/replies" | jq -r '.items[-1].text')
BIND_TOKEN=$(printf '%s' "$CHALLENGE_REPLY" | grep -Eo 'axim_[A-Za-z0-9_-]+' | head -n1)
[[ -n "$BIND_TOKEN" ]] || _fail "im-bind-token" "missing axim_ token in reply: $CHALLENGE_REPLY"

BIND_BEARER="${WEST_USER_TOKEN:-$AXPOINT_DEV_TOKEN}"
CONFIRM_RESP=$(curl -sS -X POST "$CORE_BASE_URL/api/axagent/im-bind/confirm" \
  -H "Authorization: Bearer $BIND_BEARER" \
  -H "Content-Type: application/json" \
  -d "{\"token\": \"$BIND_TOKEN\"}")

# 重放已消费挑战码必须返回 400
expect_code "im-bind-replay-rejected" 400 POST /api/axagent/im-bind/confirm "{\"token\": \"$BIND_TOKEN\"}" "$BIND_BEARER"
expect_eq "im-bind-audit" "$(sql_app default "SELECT count(*) > 0 FROM sec_audit_logs WHERE action='IM_ACCOUNT_BIND'")" "t"
expect_json "im-bind-token" "$CONFIRM_RESP" '.platform == "FEISHU" and .external_user_id == "ou_west_user_01"'

# 4. C75（二）：绑定后提问全量继承本人 AxGuard 行级权限与 PII 脱敏（且事件重投幂等）
BEFORE_REPLIES=$(curl -sf "http://127.0.0.1:${MOCK_IM_PORT}/test/replies" | jq '.items | length')
curl -sf -X POST "http://127.0.0.1:${MOCK_IM_PORT}/test/inject" \
  -H "Content-Type: application/json" \
  -d '{"event_id":"evt_ask_rls_01","external_user_id":"ou_west_user_01","external_display_name":"王西区","chat_id":"oc_dm_01","text":"查询我可见的部门订单明细"}' >/dev/null
# 重复注入相同 event_id，验证去重不触发二次回复
curl -sf -X POST "http://127.0.0.1:${MOCK_IM_PORT}/test/inject" \
  -H "Content-Type: application/json" \
  -d '{"event_id":"evt_ask_rls_01","external_user_id":"ou_west_user_01","external_display_name":"王西区","chat_id":"oc_dm_01","text":"查询我可见的部门订单明细"}' >/dev/null

wait_until "im-agent-replied" 30 \
  "[[ \$(curl -sf 'http://127.0.0.1:${MOCK_IM_PORT}/test/replies' | jq '.items | length') -gt $BEFORE_REPLIES ]]"

# 核验生成的 IM 会话归属于绑定用户本人，且未泄露跨部门受保护记录或未脱敏手机号
BOUND_UID=$(sql_app default "SELECT user_id FROM ax_im_user_bindings WHERE platform='FEISHU' AND external_user_id='ou_west_user_01'")
IM_CONV_CHECK=$(sql_app default "SELECT count(*) FROM ax_conversations WHERE channel='IM' AND im_connector_id='$CONN_ID' AND user_id='$BOUND_UID'")
LAST_IM_TEXT=$(curl -sf "http://127.0.0.1:${MOCK_IM_PORT}/test/replies" | jq -c '.items[-1]')
expect_eq "im-conv-bound-principal" "$IM_CONV_CHECK" "1"
expect_json "im-rls-inherited" "$LAST_IM_TEXT" '
  (.text | length) > 0
  and ((.text | test("13[0-9]{9}")) | not)
  and ((.text | contains("W-2")) | not)
'

# 5. C75（三）：模拟原持有副本失联（lease_until 过期 + WebSocket 断开），验证 < 45s 自动抢占接管
sql_owner "UPDATE ax_im_connectors SET leader_holder = 'crashed-replica-0', lease_until = NOW() - INTERVAL '2 seconds', status = 'ERROR' WHERE id = '$CONN_ID'" >/dev/null
curl -sf -X POST "http://127.0.0.1:${MOCK_IM_PORT}/test/drop-ws" >/dev/null

T_START=$(date +%s)
wait_until "im-failover-reclaimed" 40 \
  "api GET /api/axagent/im-connectors | jq -e '(.items // .)[] | select(.id==\"$CONN_ID\") | .status == \"CONNECTED\" and .leader_holder != \"crashed-replica-0\" and (.leader_holder != null)'"
T_ELAPSED=$(( $(date +%s) - T_START ))
expect_json "im-leader-failover" "{\"elapsed_s\": $T_ELAPSED}" '.elapsed_s < 45'

gate_pass step21
