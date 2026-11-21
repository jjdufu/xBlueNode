#!/usr/bin/env bash
# Zero-downtime Ed25519 JWT signing key rotation script (Step 13 §13.4.3).
# Usage: scripts/ops/rotate-jwt-key.sh <new_kid> [--wait-seconds 60]
set -euo pipefail

NEW_KID="${1:-}"
if [[ -z "$NEW_KID" || ! "$NEW_KID" =~ ^[a-zA-Z0-9_-]{1,64}$ ]]; then
  echo "usage: $0 <new_kid> [--wait-seconds N]" >&2
  exit 2
fi
shift

WAIT_SECS="${AXPOINT_ROTATE_WAIT_SECONDS:-60}"
if [[ "${1:-}" == "--wait-seconds" && -n "${2:-}" ]]; then
  WAIT_SECS="$2"
fi

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
ACTIVE_PRIV="${AXPOINT_JWT_PRIVATE_KEY_PATH:-$ROOT/.gate/jwt/private.pem}"

NEW_PRIV="$("$ROOT/scripts/ops/gen-jwt-key.sh" "$NEW_KID")"
echo "Stage 1/3: Published public key for kid=${NEW_KID}; waiting ${WAIT_SECS}s for verifier reload..." >&2
if (( WAIT_SECS > 0 )); then
  sleep "$WAIT_SECS"
fi

TMP_PRIV="${ACTIVE_PRIV}.next.$$"
cp -f "$NEW_PRIV" "$TMP_PRIV"
chmod 600 "$TMP_PRIV"
mv -f "$TMP_PRIV" "$ACTIVE_PRIV"
echo "Stage 2/3: Activated private key -> ${ACTIVE_PRIV} (kid=${NEW_KID})." >&2
echo "Stage 3/3: Retain previous public key in AXPOINT_JWT_PUBLIC_KEYS_DIR for >= 900s before pruning." >&2
