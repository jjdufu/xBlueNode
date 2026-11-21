#!/usr/bin/env bash
# Generates an Ed25519 JWT signing keypair (<kid>) for AxGuard (Step 13 §13.4.3).
set -euo pipefail

KID="${1:-}"
if [[ -z "$KID" || ! "$KID" =~ ^[a-zA-Z0-9_-]{1,64}$ ]]; then
  echo "usage: $0 <kid>" >&2
  exit 2
fi

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
PUB_DIR="${AXPOINT_JWT_PUBLIC_KEYS_DIR:-$ROOT/.gate/jwt/public}"
PRIV_DIR="$(dirname "${AXPOINT_JWT_PRIVATE_KEY_PATH:-$ROOT/.gate/jwt/private.pem}")"
mkdir -p "$PUB_DIR" "$PRIV_DIR"

KEY_PRIV="$PRIV_DIR/${KID}.key.pem"
KEY_PUB="$PUB_DIR/${KID}.pem"

umask 077
openssl genpkey -algorithm ed25519 -out "$KEY_PRIV" >/dev/null 2>&1
chmod 600 "$KEY_PRIV"
umask 022
openssl pkey -in "$KEY_PRIV" -pubout -out "$KEY_PUB" >/dev/null 2>&1

# If no active private.pem exists yet, initialize it from this keypair.
ACTIVE_PRIV="${AXPOINT_JWT_PRIVATE_KEY_PATH:-$ROOT/.gate/jwt/private.pem}"
if [[ ! -f "$ACTIVE_PRIV" ]]; then
  cp -f "$KEY_PRIV" "$ACTIVE_PRIV"
  chmod 600 "$ACTIVE_PRIV"
fi

echo "$KEY_PRIV"
