#!/usr/bin/env bash
# Creates or updates a local ADMIN user via owner connection, reading password from stdin (Step 13 §13.3).
# Usage: printf '%s\n' "$PASSWORD" | scripts/ops/create-admin.sh <tenant> <username>
set -euo pipefail

TENANT="${1:-}"
USERNAME="${2:-}"
if [[ -z "$TENANT" || -z "$USERNAME" ]]; then
  echo "usage: $0 <tenant> <username>  (reads password from stdin)" >&2
  exit 2
fi
if [[ ! "$TENANT" =~ ^[a-z0-9_-]{1,64}$ || ! "$USERNAME" =~ ^[a-zA-Z0-9_.-]{1,64}$ ]]; then
  echo "error: invalid tenant or username characters" >&2
  exit 2
fi

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
if [[ -z "${MIGRATION_DATABASE_URL:-}" && -f "$ROOT/.env.test" ]]; then
  set -a
  # shellcheck disable=SC1091
  source "$ROOT/.env.test"
  set +a
fi
: "${MIGRATION_DATABASE_URL:?MIGRATION_DATABASE_URL is required}"

if [[ -t 0 ]]; then
  read -r -s -p "Password for ${USERNAME}@${TENANT}: " PASSWORD
  echo >&2
else
  IFS= read -r PASSWORD
fi

if (( ${#PASSWORD} < 12 )); then
  echo "error: password must be at least 12 characters" >&2
  exit 2
fi
if [[ "${PASSWORD,,}" == *"${USERNAME,,}"* ]]; then
  echo "error: password must not contain username" >&2
  exit 2
fi

PY_BIN="python3"
[[ -x "$ROOT/axpoint-parser/.venv/bin/python" ]] && PY_BIN="$ROOT/axpoint-parser/.venv/bin/python"

# Compute RFC 9106 Argon2id (v=19, m=19456, t=2, p=1) PHC hash via Python or openssl/argon2 fallback.
PHC_HASH="$(
  AXP_PW="$PASSWORD" "$PY_BIN" - <<'PY'
import base64
import hashlib
import os

pw = os.environ["AXP_PW"].encode("utf-8")
salt = os.urandom(16)
try:
    from argon2.low_level import Type, hash_secret
    raw = hash_secret(pw, salt, time_cost=2, memory_cost=19456, parallelism=1, hash_len=32, type=Type.ID)
    print(raw.decode("ascii"))
except Exception:
    # Standard library hashlib.scrypt is not argon2id; check cryptography / passlib or raise clear error
    import ctypes, ctypes.util
    raise SystemExit("argon2-cffi or Core /api/axguard/users required; fallback via Core API")
PY
2>/dev/null || true
)"

if [[ -z "$PHC_HASH" && -n "${CORE_BASE_URL:-}" && -n "${AXPOINT_DEV_TOKEN:-}" ]]; then
  PAYLOAD="$(jq -nc --arg u "$USERNAME" --arg p "$PASSWORD" '{
    username: $u,
    display_name: $u,
    password: $p,
    dept_id: "default",
    clearance: "SECRET",
    roles: ["ADMIN"]
  }')"
  curl -sf -X POST -H "Authorization: Bearer $AXPOINT_DEV_TOKEN" -H "Content-Type: application/json" \
    "$CORE_BASE_URL/api/axguard/users" -d "$PAYLOAD" >/dev/null
  exit 0
fi

if [[ -z "$PHC_HASH" ]]; then
  echo "error: unable to compute argon2id hash without argon2-cffi or running Core API" >&2
  exit 1
fi

TENANT_SQL="${TENANT//\'/\'\'}"
USER_SQL="${USERNAME//\'/\'\'}"
HASH_SQL="${PHC_HASH//\'/\'\'}"

psql "$MIGRATION_DATABASE_URL" -v ON_ERROR_STOP=1 -Atq <<SQL >/dev/null
BEGIN;
INSERT INTO sec_tenants (tenant_id, name) VALUES ('${TENANT_SQL}', '${TENANT_SQL}') ON CONFLICT DO NOTHING;
INSERT INTO sec_departments (tenant_id, dept_id, name) VALUES ('${TENANT_SQL}', 'default', '默认部门') ON CONFLICT DO NOTHING;
WITH upserted AS (
    INSERT INTO sec_users (tenant_id, username, display_name, dept_id, password_hash, auth_source, status, clearance, password_changed_at)
    VALUES ('${TENANT_SQL}', '${USER_SQL}', '${USER_SQL}', 'default', '${HASH_SQL}', 'LOCAL', 'ACTIVE', 'SECRET', NOW())
    ON CONFLICT (tenant_id, username) DO UPDATE
       SET password_hash = EXCLUDED.password_hash,
           status = 'ACTIVE',
           clearance = 'SECRET',
           locked_until = NULL,
           failed_logins = 0,
           tokens_valid_after = NOW(),
           password_changed_at = NOW(),
           updated_at = NOW()
    RETURNING id
)
INSERT INTO sec_user_roles (tenant_id, user_id, role_id)
SELECT '${TENANT_SQL}', u.id, r.id
  FROM upserted u
  JOIN sec_roles r ON r.tenant_id = '${TENANT_SQL}' AND r.name = 'ADMIN'
ON CONFLICT DO NOTHING;

INSERT INTO sec_audit_logs (tenant_id, username, action, resource_ref, status_code, client_ip, detail_json)
VALUES ('${TENANT_SQL}', 'system:ops', 'POLICY_CHANGE', 'user:${USER_SQL}', 200, '127.0.0.1', '{"op":"create-admin","role":"ADMIN"}'::jsonb);
COMMIT;
SQL
