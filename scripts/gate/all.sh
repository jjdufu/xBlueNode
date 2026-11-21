#!/usr/bin/env bash
# 全平台终验门禁（Step 00～14 + 路由/溯源静态核验 + 端到端全旅程，§14.9.1）
set -euo pipefail
cd "$(dirname "$0")/../.."
ROOT="$PWD"

bash scripts/dev-down.sh || true
bash scripts/dev-up.sh
if command -v docker >/dev/null 2>&1 && docker info >/dev/null 2>&1 && [[ -f deploy/docker-compose.test.yml ]]; then
  docker compose -f deploy/docker-compose.test.yml --env-file .env.test up -d --wait
fi

set -a
# shellcheck disable=SC1091
source .env.test
set +a
export AXPOINT_LLM_CASSETTE="${AXPOINT_LLM_CASSETTE:-$ROOT/tests/cassettes}"

bash scripts/start-services.sh
bash scripts/seed-demo.sh

if [[ -n "${NAMING_DENYLIST:-}" || -f .env ]]; then
  bash scripts/naming-guard.sh
fi
python3 scripts/lint-guard.py
python3 scripts/check_routes.py --routes axpoint-core/crates/axpoint-server/src/routes --index docs/blueprint/A4-API-CONTRACT-INDEX.md
python3 scripts/check_traceability.py docs/blueprint/A7-REQUIREMENTS-TRACEABILITY.md scripts/gate

mkdir -p .gate
for n in $(seq -w 0 14); do
  bash "scripts/gate/step${n}.sh" | tee ".gate/step${n}.log"
  tail -n1 ".gate/step${n}.log" | grep -qx "GATE step${n} PASS"
done

(cd xbluenode-web && AXPOINT_E2E_LIVE=1 npx playwright test e2e/full-journey.spec.ts)
echo "ALL GATES PASS"
