#!/usr/bin/env bash
# Step 32 Gate：/api/axops/pulse 租户隔离、真数据聚合准确性与总控看板前端渲染验收
set -euo pipefail
source "$(dirname "$0")/lib.sh"

# 1. 静态守卫与路由契约核对
python3 "$ROOT/scripts/lint-guard.py"
python3 "$ROOT/scripts/web-guard.py"
python3 "$ROOT/scripts/check_routes.py" \
  --routes "$ROOT/axpoint-core/crates/axpoint-server/src/routes" \
  --index "$ROOT/docs/standards/A4-API-CONTRACT-INDEX.md"

# 2. 后端脉冲聚合、RLS 隔离与 RBAC 集成测试
(cd "$ROOT/axpoint-core" && cargo test -p axpoint-server --test step32_pulse_api --locked)

# 3. 确保 default 租户具备非空业务节点后验证跨租户 RLS 零泄露
PULSE_DEFAULT=$(api GET /api/axops/pulse)
if [[ "$(printf '%s' "$PULSE_DEFAULT" | jq -r '.totals.node_count // 0')" -eq 0 ]]; then
  if [[ ! -f "$ROOT/axpoint-parser/tests/fixtures/supply_chain.xlsx" ]]; then
    (cd "$ROOT/axpoint-parser" && python3 -c "from tests.fixtures.make_fixtures import PARSER_ROOT, generate_all; generate_all(root_dir=PARSER_ROOT, convert_legacy=False)")
  fi
  bash "$ROOT/scripts/seed-demo.sh" 10
  # shellcheck disable=SC1091
  [[ -f "$ROOT/.gate/ids.env" ]] && source "$ROOT/.gate/ids.env"
  PULSE_DEFAULT=$(api GET /api/axops/pulse)
fi

OTHER_PW="Axp#Step32_Other!26"
PROBE_USER="step32_hash_probe"
EXISTING_PROBE_ID=$(api GET /api/axguard/users | jq -r --arg u "$PROBE_USER" '.items // . | map(select(.username == $u))[0].id // empty')
if [[ -z "$EXISTING_PROBE_ID" ]]; then
  api POST /api/axguard/users \
    "$(jq -nc --arg u "$PROBE_USER" --arg p "$OTHER_PW" '{username:$u, display_name:$u, password:$p, dept_id:"default", clearance:"INTERNAL", roles:["VIEWER"], attributes:{}}')" >/dev/null
else
  api POST "/api/axguard/users/$EXISTING_PROBE_ID/reset-password" \
    "$(jq -nc --arg p "$OTHER_PW" '{new_password:$p}')" >/dev/null
fi
OTHER_PW_HASH=$(sql_owner "SELECT password_hash FROM sec_users WHERE tenant_id = 'default' AND username = '$PROBE_USER' LIMIT 1")
sql_owner "DELETE FROM sec_users WHERE tenant_id = 'default' AND username = '$PROBE_USER'" >/dev/null

sql_owner "
INSERT INTO sec_tenants (tenant_id, display_name)
VALUES ('other', '隔离对照租户')
ON CONFLICT (tenant_id) DO NOTHING;

INSERT INTO sec_departments (tenant_id, dept_id, name)
VALUES ('other', 'default', '默认部门')
ON CONFLICT (tenant_id, dept_id) DO NOTHING;

INSERT INTO sec_roles (tenant_id, name, permissions, is_builtin)
VALUES ('other', 'VIEWER', ARRAY['DATA_READ'], TRUE)
ON CONFLICT (tenant_id, name) DO UPDATE SET permissions = ARRAY['DATA_READ'];

INSERT INTO sec_users (tenant_id, username, display_name, dept_id, password_hash, clearance, status, failed_logins, locked_until)
VALUES ('other', 'step32_other_reader', '隔离租户只读员', 'default', '${OTHER_PW_HASH}', 'INTERNAL', 'ACTIVE', 0, NULL)
ON CONFLICT (tenant_id, username) DO UPDATE
SET password_hash = EXCLUDED.password_hash,
    status = 'ACTIVE',
    failed_logins = 0,
    locked_until = NULL;

INSERT INTO sec_user_roles (tenant_id, user_id, role_id)
SELECT 'other', u.id, r.id
  FROM sec_users u
  JOIN sec_roles r ON r.tenant_id = 'other' AND r.name = 'VIEWER'
 WHERE u.tenant_id = 'other' AND u.username = 'step32_other_reader'
ON CONFLICT DO NOTHING;
" >/dev/null

OTHER_TOKEN=$(curl -sS -H "Content-Type: application/json" -X POST "$CORE_BASE_URL/api/auth/login" \
  -d "{\"tenant\":\"other\",\"username\":\"step32_other_reader\",\"password\":\"$OTHER_PW\"}" | jq -r .access_token)
expect_json "other-jwt-shape" "{\"t\":\"$OTHER_TOKEN\"}" '.t | split(".") | length == 3'

PULSE_OTHER=$(curl -sS -H "Authorization: Bearer $OTHER_TOKEN" -H "Accept: application/json" "$CORE_BASE_URL/api/axops/pulse")
expect_json "pulse-rls-isolated" "$(jq -n --argjson d "$PULSE_DEFAULT" --argjson o "$PULSE_OTHER" '{d:$d, o:$o}')" '
  (.d.totals.node_count > 0)
  and (.o.totals.node_count == 0)
  and ((.o.subgraphs | length) <= 1)
'

# 4. 前端构建与隔离端口 Playwright 验收
PW_PORT="${PLAYWRIGHT_PORT:-$(( 4173 + ${WT_SLOT:-0} * 100 ))}"
(
  cd "$ROOT/xbluenode-web"
  VITE_DEV_TOKEN="$AXPOINT_DEV_TOKEN" npm run build
  AXPOINT_E2E_LIVE=1 CORE_BASE_URL="$CORE_BASE_URL" VITE_DEV_TOKEN="$AXPOINT_DEV_TOKEN" PLAYWRIGHT_PORT="$PW_PORT" \
    npx playwright test e2e/step32-live-dashboard.spec.ts
)
echo "ASSERT dashboard-live-pulse PASS"
echo "ASSERT dashboard-subgraph-matrix PASS"

gate_pass step32
