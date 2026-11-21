#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "$0")/lib.sh"

(cd axpoint-core && cargo test -p axpoint-security)
(cd axpoint-parser && (uv run pytest tests/test_pii.py -q || pytest tests/test_pii.py -q))

if [[ -z "${EAST_PASSWORD:-}" || -z "${SOUTH_PASSWORD:-}" || -z "${AUDITOR_PASSWORD:-}" || -z "${ORDERS_BY_STATUS_QUERY_ID:-}" ]]; then
  bash "$ROOT/scripts/seed-demo.sh" 13
  # shellcheck disable=SC1091
  source "$ROOT/.gate/ids.env"
fi

login() { curl -s -c "/tmp/cj_$1" -H "Content-Type: application/json" -X POST "$CORE_BASE_URL/api/auth/login" \
            -d "{\"tenant\":\"default\",\"username\":\"$1\",\"password\":\"$2\"}" | jq -r .access_token; }
as() { local tok=$1; shift; curl -s -H "Authorization: Bearer $tok" -H "Content-Type: application/json" "$@"; }
export -f as

EAST=$(login analyst_east "$EAST_PASSWORD")
SOUTH=$(login analyst_south "$SOUTH_PASSWORD")
expect_json "jwt-shape" "{\"t\":\"$EAST\"}" '.t | split(".") | length == 3'

# 行策略：east 只看到 W-1 的订单
Q='{"sql":"SELECT DISTINCT warehouse_code FROM demo_orders ORDER BY 1"}'
expect_json "row-policy" "$(as "$EAST" -X POST "$CORE_BASE_URL/api/analytics/sql/run" -d "$Q")" '[.rows[].warehouse_code] == ["W-1"]'
expect_json "row-policy-filter" "$(as "$EAST" -X POST "$CORE_BASE_URL/api/analytics/sql/run" -d '{"sql":"SELECT count(*) AS n FROM demo_orders WHERE warehouse_code = '"'"'W-2'"'"'"}')" '.rows[0].n == 0'

# 列脱敏
expect_json "mask-phone" "$(as "$EAST" -X POST "$CORE_BASE_URL/api/analytics/sql/run" -d '{"sql":"SELECT phone FROM demo_orders LIMIT 1"}')" '.rows[0].phone | test("^1[3-9][0-9]\\*{4}[0-9]{4}$")'
expect_json "east-no-reveal" "$(as "$EAST" -X POST "$CORE_BASE_URL/api/analytics/sql/run" -d '{"sql":"SELECT phone FROM demo_orders LIMIT 1","reveal_pii":true}')" '.error_code == "PERMISSION_DENIED"'
expect_json "admin-reveal" "$(api POST /api/analytics/sql/run '{"sql":"SELECT phone FROM demo_orders LIMIT 1","reveal_pii":true}')" '.rows[0].phone | test("^1[3-9][0-9]{9}$")'

# Cedar：south 跨部门访问机密数据集被拒绝；授权后可访问
expect_code "cedar-deny" 403 POST "/api/analytics/sql/run" "$Q" "$SOUTH"
GID=$(as "$SOUTH" -X POST "$CORE_BASE_URL/api/axguard/grants" -d "{\"resource_kind\":\"DATASET\",\"resource_ref\":\"$DEMO_ORDERS_DATASET_ID\",\"grantee_kind\":\"DEPT\",\"grantee_ref\":\"south\",\"actions\":[\"DATA_READ\"],\"row_predicate\":\"warehouse_code = 'W-2'\",\"reason\":\"门禁\",\"expires_in_days\":1}" | jq -r .id)
APR=$(api GET /api/axguard/grants | jq -r "(.items // .)[] | select(.id==\"$GID\") | .approval_id")
api POST "/api/axflow/approvals/$APR/decide" '{"decision":"APPROVE"}' >/dev/null
wait_until "grant-active" 30 "as '$SOUTH' -X POST '$CORE_BASE_URL/api/analytics/sql/run' -d '$Q' | jq -e '[.rows[].warehouse_code] == [\"W-2\"]'"

# 刷新轮换与重放检测
R1=$(curl -s -b /tmp/cj_analyst_east -c /tmp/cj_east2 -X POST "$CORE_BASE_URL/api/auth/refresh" -o /dev/null -w '%{http_code}')
expect_eq "refresh-ok" "$R1" "200"
R2=$(curl -s -b /tmp/cj_analyst_east -X POST "$CORE_BASE_URL/api/auth/refresh" -o /dev/null -w '%{http_code}')
expect_eq "refresh-reuse" "$R2" "401"
R3=$(curl -s -b /tmp/cj_east2 -X POST "$CORE_BASE_URL/api/auth/refresh" -o /dev/null -w '%{http_code}')
expect_eq "family-revoked" "$R3" "401"
rm -f /tmp/cj_analyst_east /tmp/cj_analyst_south /tmp/cj_east2

# 锁定
for _ in 1 2 3 4 5; do
  curl -s -o /dev/null -H "Content-Type: application/json" -X POST "$CORE_BASE_URL/api/auth/login" \
    -d '{"tenant":"default","username":"auditor","password":"wrong-password-x"}'
done
expect_eq "lockout" "$(curl -s -o /dev/null -w '%{http_code}' -H 'Content-Type: application/json' -X POST "$CORE_BASE_URL/api/auth/login" -d "{\"tenant\":\"default\",\"username\":\"auditor\",\"password\":\"$AUDITOR_PASSWORD\"}")" "401"

# 租户 forbid 策略
api PUT /api/axguard/policies/no_export '{"policy_text":"forbid (principal, action == AxPoint::Action::\"EXPORT_DATA\", resource);"}' >/dev/null
expect_code "forbid-export" 403 POST "/api/xblueview/exports" "{\"export_format\":\"CSV\",\"source\":{\"kind\":\"SAVED_QUERY\",\"saved_query_id\":\"$ORDERS_BY_STATUS_QUERY_ID\"}}" "$EAST"
api DELETE /api/axguard/policies/no_export >/dev/null

# PII 出境过滤（录制回放下检查出站请求体）
expect_json "pii-egress" "$(api POST /api/axguard/debug/llm-egress-preview '{"text":"客户电话 13812345678"}')" '.outbound | contains("13812345678") | not'

# 审计链
expect_json "audit-chain" "$(api GET /api/axguard/audit/verify)" '.ok == true'
LAST_ID=$(sql_owner "SELECT max(id) FROM sec_audit_logs WHERE tenant_id='default'")
ORIG=$(sql_owner "SELECT username FROM sec_audit_logs WHERE id=$LAST_ID")
sql_owner "UPDATE sec_audit_logs SET username='tampered' WHERE id=$LAST_ID" >/dev/null
expect_json "audit-tamper" "$(api GET /api/axguard/audit/verify)" '.ok == false and .first_broken_id == '"$LAST_ID"
sql_owner "UPDATE sec_audit_logs SET username='$ORIG' WHERE id=$LAST_ID" >/dev/null
expect_json "audit-restored" "$(api GET /api/axguard/audit/verify)" '.ok == true'

gate_pass step13
