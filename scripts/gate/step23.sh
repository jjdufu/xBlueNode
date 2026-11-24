#!/usr/bin/env bash
# Step 23 Gate：8 域信息架构收敛、双语单态纯净度守卫、⌘K 面板与弹性分栏持久化验收
set -euo pipefail
source "$(dirname "$0")/lib.sh"

# 1. 静态双语纯净度与零绕过守卫校验（C77）
python3 "$ROOT/scripts/web-guard.py"
echo "ASSERT i18n-guard-ok PASS"

# 2. 扫描 meta.ts 与 App.tsx 确认零 productTag 残留与零模块英文前缀
if grep -E 'productTag|(AxHub|AxIngest|AxAnalytics|AxQuery|xBlueStudio|xBlueGraph|AxFlow|AxAgent|AxSim|xBlueView|AxGuard|AxOps)\s*·' \
  "$ROOT/xbluenode-web/src/modules/meta.ts" "$ROOT/xbluenode-web/src/App.tsx"; then
  echo "FAIL: codename prefix leaked in navigation meta" >&2
  exit 1
fi
echo "ASSERT zh-zero-codename PASS"

# 3. 前端严格类型构建与单测
(cd "$ROOT/xbluenode-web" && npm run build && npm test)

# 4. Playwright 专项验收：8 域侧栏、旧路由兼容重定向、中文态 DOM 零裸枚举泄漏、⌘K 搜索跳转、分栏拖拽刷新持久化（C76 & C77）
(cd "$ROOT/xbluenode-web" && npx playwright test e2e/step23-ia-i18n-shell.spec.ts e2e/smoke.spec.ts)
echo "ASSERT ia-8-domains PASS"
echo "ASSERT legacy-route-redirect PASS"
echo "ASSERT cmdk-jump-ok PASS"
echo "ASSERT resizable-pane-persist PASS"

gate_pass step23
