#!/usr/bin/env bash
# Step 24 Gate：正交避障零遮挡、磁吸对齐、三层复合折叠、撤销重做、边中点插算子与 G6 因果血缘图验收
set -euo pipefail
source "$(dirname "$0")/lib.sh"

# 1. 静态守卫与严格类型分包构建校验
python3 "$ROOT/scripts/web-guard.py"
(cd "$ROOT/xbluenode-web" && npm run build && npm test)

# 2. 确认 @antv/g6 已独立拆分为异步 chunk，未打入主入口首屏包
MAIN_BUNDLE=$(ls "$ROOT/xbluenode-web/dist/assets"/index-*.js | head -n 1)
if grep -q "collapse-expand-combo" "$MAIN_BUNDLE"; then
  echo "FAIL: @antv/g6 leaked into main entry bundle instead of lazy chunk" >&2
  exit 1
fi

# 3. Playwright 画布几何与交互深度回归（C78 & C79）
(cd "$ROOT/xbluenode-web" && npx playwright test e2e/step24-advanced-canvases.spec.ts e2e/xbluestudio.spec.ts e2e/axflow.spec.ts e2e/axsim.spec.ts)
echo "ASSERT elk-orthogonal-no-overlap PASS"
echo "ASSERT compound-tier-fold PASS"
echo "ASSERT canvas-undo-redo PASS"
echo "ASSERT flow-edge-insert PASS"
echo "ASSERT g6-causal-combo-hull PASS"

gate_pass step24
