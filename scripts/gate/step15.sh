#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "$0")/lib.sh"

# 1. 领域 Schema 构造器、受限解码协议、零温钳制与路由器单测
(cd axpoint-core && cargo test -p axpoint-domain json_schema --locked && cargo test -p axpoint-llm structured --locked && cargo test -p axpoint-llm router --locked)

# 2. 启动本地一次性 OpenAI 兼容回环捕获桩，核验 Core 真实发出的 Wire JSON 请求体
MOCK_PORT=19951
MOCK_CAPTURE="$ROOT/.gate/step15_last_wire.json"
rm -f "$MOCK_CAPTURE"
python3 - "$MOCK_PORT" "$MOCK_CAPTURE" <<'PY' &
import http.server, json, pathlib, sys

port = int(sys.argv[1])
capture_path = pathlib.Path(sys.argv[2])

class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, fmt, *args):
        pass
    def do_POST(self):
        length = int(self.headers.get("Content-Length", "0"))
        raw = self.rfile.read(length).decode("utf-8")
        payload = json.loads(raw)
        capture_path.write_text(json.dumps(payload), encoding="utf-8")
        reply_obj = {"status": "OK", "provider_echo": payload.get("model", "")}
        body = json.dumps({
            "id": "chatcmpl-step15",
            "model": payload.get("model", "mock"),
            "choices": [{"index": 0, "message": {"role": "assistant", "content": json.dumps(reply_obj)}}],
            "usage": {"prompt_tokens": 12, "completion_tokens": 6}
        }).encode("utf-8")
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

http.server.HTTPServer(("127.0.0.1", port), Handler).serve_forever()
PY
MOCK_PID=$!

# 签发仅含 AGENT_EDIT + AGENT_RUN + DATA_READ（不含 GUARD_ADMIN / OPS_ADMIN）的受限个人令牌，用于核验系统提供方只读保护
ANALYST_PAT_RESP=$(api POST /api/auth/tokens '{"name":"step15-nonadmin-gate","scopes":["DATA_READ","AGENT_EDIT","AGENT_RUN"],"expires_in_days":1}')
ANALYST_PAT=$(printf '%s' "$ANALYST_PAT_RESP" | jq -r .token)
ANALYST_PAT_ID=$(printf '%s' "$ANALYST_PAT_RESP" | jq -r .id)
SYS_PROV_ID=""
USER_PROV_ID=""

cleanup_step15() {
  kill "$MOCK_PID" >/dev/null 2>&1 || true
  api PUT /api/axagent/llm-preferences '{"preferences":[]}' >/dev/null 2>&1 || true
  [[ -n "$USER_PROV_ID" && "$USER_PROV_ID" != "null" ]] && api DELETE "/api/axagent/providers/$USER_PROV_ID" >/dev/null 2>&1 || true
  [[ -n "$SYS_PROV_ID" && "$SYS_PROV_ID" != "null" ]] && api DELETE "/api/axagent/providers/$SYS_PROV_ID" >/dev/null 2>&1 || true
  [[ -n "$ANALYST_PAT_ID" && "$ANALYST_PAT_ID" != "null" ]] && api DELETE "/api/auth/tokens/$ANALYST_PAT_ID" >/dev/null 2>&1 || true
}
trap cleanup_step15 EXIT
sleep 0.4

# 3. 注册系统级受限解码提供方（故意传入 temperature=0.75，验证确定性探测强制锁死 0.0 且携带严格 json_schema）
SYS_PROV=$(api POST /api/axagent/providers "{
  \"name\":\"step15-sys-vllm\",
  \"scope\":\"SYSTEM\",
  \"base_url\":\"http://127.0.0.1:$MOCK_PORT/v1\",
  \"model\":\"qwen2.5-14b-instruct\",
  \"purposes\":[\"NL2SQL\",\"EXTRACT\",\"SUMMARIZE\",\"CHAT\"],
  \"priority\":10,
  \"temperature\":0.75,
  \"max_tokens\":1024,
  \"max_context_tokens\":32768,
  \"timeout_s\":15,
  \"structured_mode\":\"JSON_SCHEMA\",
  \"deterministic_seed\":42,
  \"is_local_model\":true,
  \"supports_tools\":true,
  \"allow_private_network\":true,
  \"enabled\":true
}")
SYS_PROV_ID=$(printf '%s' "$SYS_PROV" | jq -r .id)

# 在非回放模式下探测真实出站报文；若当前 Core 启用了全局 Cassette 回放，则联合回放校验与单测报文核验
PROBE_RESP=$(api POST "/api/axagent/providers/$SYS_PROV_ID/test" '{}')
if [[ -f "$MOCK_CAPTURE" ]]; then
  CAPTURED_WIRE=$(cat "$MOCK_CAPTURE")
  expect_json "schema-guided-ok" "$CAPTURED_WIRE" '.response_format.type == "json_schema" and .response_format.json_schema.strict == true and .seed == 42'
  expect_json "temp-zero-enforced" "$CAPTURED_WIRE" '.temperature == 0 or .temperature == 0.0'
else
  expect_json "schema-guided-ok" "$PROBE_RESP" '.ok == true and .structured_mode == "JSON_SCHEMA" and .structured_verified == true'
  expect_json "temp-zero-enforced" "$PROBE_RESP" '.effective_temperature == 0 or .effective_temperature == 0.0'
fi

# 4. 非系统管理员令牌不得修改或删除系统级提供方（403 PERMISSION_DENIED）
expect_code "system-provider-readonly" 403 DELETE "/api/axagent/providers/$SYS_PROV_ID" "" "$ANALYST_PAT"

# 5. 注册低优先级个人提供方（priority=500），绑定为当前用户 NL2SQL 默认模型，验证优先命中个人模型
USER_PROV=$(api POST /api/axagent/providers "{
  \"name\":\"step15-personal-coder\",
  \"scope\":\"PERSONAL\",
  \"base_url\":\"http://127.0.0.1:$MOCK_PORT/v1\",
  \"model\":\"personal-sql-7b\",
  \"purposes\":[\"NL2SQL\",\"EXTRACT\"],
  \"priority\":500,
  \"temperature\":0.5,
  \"max_tokens\":1024,
  \"max_context_tokens\":16384,
  \"timeout_s\":15,
  \"structured_mode\":\"JSON_SCHEMA\",
  \"deterministic_seed\":7,
  \"is_local_model\":true,
  \"supports_tools\":false,
  \"allow_private_network\":true,
  \"enabled\":true
}")
USER_PROV_ID=$(printf '%s' "$USER_PROV" | jq -r .id)

PREF_SAVED=$(api PUT /api/axagent/llm-preferences "{\"preferences\":[{\"purpose\":\"NL2SQL\",\"provider_id\":\"$USER_PROV_ID\"}]}")
BEFORE_REQ=$(sql_app default "SELECT COALESCE((SELECT requests FROM ax_llm_usage WHERE provider_id = '$USER_PROV_ID' AND purpose = 'NL2SQL' AND usage_date = CURRENT_DATE), 0)")
api POST /api/analytics/ask '{"question":"已支付订单总金额是多少？"}' >/dev/null
AFTER_REQ=$(sql_app default "SELECT COALESCE((SELECT requests FROM ax_llm_usage WHERE provider_id = '$USER_PROV_ID' AND purpose = 'NL2SQL' AND usage_date = CURRENT_DATE), 0)")

expect_json "user-default-provider" "$PREF_SAVED" \
  "(.items | length) == 1 and .items[0].purpose == \"NL2SQL\" and .items[0].provider_id == \"$USER_PROV_ID\" and ($AFTER_REQ > $BEFORE_REQ)"

# 6. 验证新增租户表 ax_user_llm_preferences 的跨租户 RLS 零可见性
expect_eq "user-llm-pref-rls" "$(sql_app other "SELECT count(*) FROM ax_user_llm_preferences")" "0"

gate_pass step15
