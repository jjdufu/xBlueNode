#!/usr/bin/env bash
# Step 31 Gate：平实建模语言静态守卫、建模置前 8 域导航与底栏单行系统设置验收
set -euo pipefail
source "$(dirname "$0")/lib.sh"

python3 "$ROOT/scripts/web-guard.py"
python3 "$ROOT/scripts/docs-and-plans-guard.py"
echo "ASSERT ui-zero-ontology-jargon PASS"

(cd "$ROOT/xbluenode-web" && npm run build && npm test)
PW_PORT="${PLAYWRIGHT_PORT:-$(( 4173 + ${WT_SLOT:-0} * 100 ))}"
(cd "$ROOT/xbluenode-web" && PLAYWRIGHT_PORT="$PW_PORT" npx playwright test \
  e2e/smoke.spec.ts \
  e2e/step23-ia-i18n-shell.spec.ts \
  e2e/step31-plain-shell.spec.ts)

echo "ASSERT nav-modeling-promoted PASS"
echo "ASSERT settings-flat-entry PASS"
gate_pass step31
