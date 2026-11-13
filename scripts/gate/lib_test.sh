#!/usr/bin/env bash
# Self-test for scripts/gate/lib.sh per A5 §2.
set -euo pipefail

GATE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TMP_DIR="$(mktemp -d)"
MOCK_PID=""
cleanup() {
  [[ -n "$MOCK_PID" ]] && kill "$MOCK_PID" 2>/dev/null || true
  rm -rf "$TMP_DIR"
}
trap cleanup EXIT

PORT_FILE="$TMP_DIR/port"
python3 - "$PORT_FILE" <<'PY' &
import http.server
import json
import socketserver
import sys

port_file = sys.argv[1]

class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, fmt, *args):
        pass

    def do_GET(self):
        auth = self.headers.get("Authorization", "")
        if self.path == "/public":
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.end_headers()
            self.wfile.write(b'{"ok":true}')
            return
        if auth == "Bearer dev_tok":
            self.send_response(200)
        elif auth == "Bearer custom_tok":
            self.send_response(202)
        else:
            self.send_response(401)
        self.send_header("Content-Type", "application/json")
        self.end_headers()
        self.wfile.write(json.dumps({"auth": auth}).encode())

with socketserver.TCPServer(("127.0.0.1", 0), Handler) as httpd:
    port = httpd.server_address[1]
    with open(port_file, "w", encoding="utf-8") as fh:
        fh.write(str(port))
    httpd.serve_forever()
PY
MOCK_PID=$!

for _ in $(seq 1 50); do
  [[ -s "$PORT_FILE" ]] && break
  sleep 0.1
done
MOCK_PORT="$(cat "$PORT_FILE")"

# Setup isolated fake ROOT with .env.test so lib_test works even without a full .env.test if needed,
# while still testing the exact lib.sh functions!
mkdir -p "$TMP_DIR/scripts/gate"
cp "$GATE_DIR/lib.sh" "$TMP_DIR/scripts/gate/lib.sh"
cat > "$TMP_DIR/.env.test" <<EOF
CORE_BASE_URL=http://127.0.0.1:$MOCK_PORT
PARSER_BASE_URL=http://127.0.0.1:$MOCK_PORT
AXPOINT_DEV_TOKEN=dev_tok
DATABASE_URL=postgres://localhost/unused
MIGRATION_DATABASE_URL=postgres://localhost/unused
EOF

run_in_subshell() {
  bash -c "source '$TMP_DIR/scripts/gate/lib.sh' >/dev/null; $1"
}

# 1. expect_json: true passes, false fails, invalid JSON fails
run_in_subshell 'expect_json "json-true" "{\"a\":1}" ".a == 1"' >/dev/null
if run_in_subshell 'expect_json "json-false" "{\"a\":1}" ".a == 2"' >/dev/null 2>&1; then
  echo "lib_test FAIL: expect_json false did not fail" >&2
  exit 1
fi
if run_in_subshell 'expect_json "json-invalid" "not-json" ".a == 1"' >/dev/null 2>&1; then
  echo "lib_test FAIL: expect_json invalid JSON did not fail" >&2
  exit 1
fi

# 2. expect_code: default token, _NOAUTH, override token
run_in_subshell 'expect_code "code-default" 200 GET /protected' >/dev/null
run_in_subshell 'expect_code "code-noauth-401" 401 GET_NOAUTH /protected' >/dev/null
run_in_subshell 'expect_code "code-noauth-200" 200 GET_NOAUTH /public' >/dev/null
run_in_subshell 'expect_code "code-override" 202 GET /protected "" custom_tok' >/dev/null

# 3. wait_until: succeeds on true, fails on timeout
run_in_subshell 'wait_until "wait-ok" 3 "true"' >/dev/null
if run_in_subshell 'wait_until "wait-timeout" 1 "false"' >/dev/null 2>&1; then
  echo "lib_test FAIL: wait_until timeout did not fail" >&2
  exit 1
fi

echo "ok  lib-self-test"
