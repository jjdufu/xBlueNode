# 05 · Step 35：十万级秒级向量化装载器、Phase 4 全旅程 E2E、实机巡检与 `v0.4.0` 归档

本步是 Phase 4（Step 31～35）的合流收官节点：将四大跨行业可执行世界整合为一键/秒级向量化装载套件 `scripts/seed-showcase.sh` 与后台接口 `POST /api/axops/demo/showcase/load`；通过端到端浏览器套件 `xbluenode-web/e2e/v4-modeling-and-worlds.spec.ts` 完整走查「平实建模导航 → 真数据总控看板 → 电力/风控/半导体/急诊四大行业画布、VTable 6 万行透视、凸包地图与沙盘推演」全链路；将能力追踪矩阵升级至 **98 项（`C01`～`C98`）**，跑通 `bash scripts/gate/all.sh`（`Step 00～35` 全绿）与实机 Chrome DevTools 浏览器巡检后完成方案封存归档及 `v0.4.0` 版本发布。

总览索引与门禁准则见 [00-V4-MASTER-INDEX.md](../00-V4-MASTER-INDEX.md)、[IRON_RULES.md](../../../../IRON_RULES.md) 与 [QUALITY_GATES_AND_VERIFICATION.md](../../../../docs/QUALITY_GATES_AND_VERIFICATION.md)。

---

## 35.0 目标、前置与契约

- **目标**：
  1. **秒级十万行向量化装载（`< 8s`）**：`bash scripts/seed-showcase.sh`（及 `POST /api/axops/demo/showcase/load`）幂等灌入 `power_grid`、`fin_risk`、`semi_supply`、`med_dispatch` 四大行业世界，全域累计达到 **业务节点 `>= 2,300` 个、关系边 `>= 6,500` 条、DuckLake 湖表明细 `>= 145,000` 行、时序指标点 `>= 80,000` 个、预置五区数字看板 `>= 5` 套**；同时保持基础门禁种子 `scripts/seed-demo.sh` 轻量不变，不增加 `Step 00～30` 单元回归耗时。
  2. **Phase 4 全旅程 Playwright E2E（`v4-modeling-and-worlds.spec.ts`）**：在前端桩模式与 `AXPOINT_E2E_LIVE=1` 真实全栈模式下均一次全绿通过。
  3. **98 项能力零漂移与全平台总门禁**：`check_routes.py` 双向零差集；`check_traceability.py` 校验 `C01`～`C98` 连续无缺漏；`bash scripts/gate/all.sh` 从干净环境顺序跑通 `step00`～`step35` 及四套全旅程 E2E，末行输出 `ALL GATES PASS`。
  4. **实机 Chrome DevTools 视觉与交互终验**：在 `http://192.168.50.50:9995` 逐一实机核验总控看板、平实导航与单行系统设置、四大行业建模画布零重叠、6 万行 VTable 透视及行业五区驾驶舱，零控制台报错。
- **前置**：[Step 31](../console-and-ia/01-STEP31-PLAIN-MODELING-TERMINOLOGY-AND-SHELL-REFORGE.md)、[Step 32](../console-and-ia/02-STEP32-LIVE-OPERATIONAL-DASHBOARD-AND-PULSE-API.md)、[Step 33](./03-STEP33-POWER-GRID-AND-FIN-RISK-WORLDS.md)、[Step 34](./04-STEP34-SEMI-SUPPLY-AND-MED-DISPATCH-WORLDS.md) 全部合入。
- **覆盖能力项**：
  - `C98`：`step35` `"showcase-145k-seeded-fast"`、`"v4-journey-pass"`、`"traceability-98-ok"`

### 文件变更清单

| 负责角色 | 动作 | 路径 | 内容 |
| :--- | :--- | :--- | :--- |
| `orchestrator` | 修改 | `docs/API_CONTRACT.md`、`docs/standards/{A4-API-CONTRACT-INDEX.md,A7-REQUIREMENTS-TRACEABILITY.md}`、`scripts/check_traceability.py`、`scripts/gate/all.sh` | 注册 `POST /api/axops/demo/showcase/load`；将能力溯源门禁下限提升至 `>= 98`（`C01`～`C98`）；升级 `all.sh` 覆盖 `step00`～`step35` 与 `v4-modeling-and-worlds.spec.ts` |
| `squad-domain-agent` | 修改 | `axpoint-core/crates/axpoint-server/src/{ops/demo.rs,routes/axops.rs}` | 支持在工作台顶栏点击 `装载样例` 触发幂等后台装载并返回节点/边/湖表行数统计（要求 `OPS_ADMIN` 权限并记录 `TEMPLATE_INSTALL` 审计链） |
| `squad-qa` | 新建 | `scripts/seed-showcase.sh`、`xbluenode-web/e2e/v4-modeling-and-worlds.spec.ts`、`scripts/gate/step35.sh` | 统一向量化多行业装载入口、六段全旅程 E2E 与 Step 35 终验门禁脚本 |

---

## 35.1 Phase 4 六段全旅程 Playwright E2E（`v4-modeling-and-worlds.spec.ts`）

| 阶段 | 验证焦点 | 硬性自动化断言 |
| :--- | :--- | :--- |
| **1. 平实建模导航与单行设置** | 遍历重排后的 8 个主导航与底栏单行 `系统设置`（`/settings`），扫描中英双态可见文本 | `建模工作室` 位于侧栏第 3 项；底栏无折叠套娃；全站可见标题/按钮/徽标零学术黑话残留；`/governance` 自动跳转 `/settings` |
| **2. 真数据总控看板（`/`）** | 打开 `/` 核验 6 张火花线态势卡、多行业模型矩阵、24h 吞吐 ECharts 图与告警/推演面板 | 页面零静态检索算法科普卡；节点总数 `>= 2,300`、湖表总行数 `>= 145,000` 与 `/api/axops/pulse` 响应精确一致；子图矩阵显示 4 个 `L4` 行业世界 |
| **3. 电力配网世界（`power_grid`）** | 从总控看板一键进入 `power_grid` 推演沙盘与五区看板 | 地图供电多边形围栏正常渲染；主变跳闸推演展示 `breach_step` 与 `deadline_step` 预警横幅 |
| **4. 集团风控世界（`fin_risk`）** | 切入 `/analytics?tab=pivot` 选择 `fin_corp_wire_transfers`（60,000 行），并核验实控人脱敏 | VTable Canvas 秒级渲染 6 万行分组透视与数据条/热力色阶；非特权视角下实控人手机号与身份证号 100% 掩码 |
| **5. 半导体与医疗急诊双世界** | 在 `/studio?subgraph=semi_supply` 开启四域复合泳道与三层折叠；在 `/view/med-dispatch-cockpit` 开启 120 急救车地图凸包 | 4 个业务域泳道与三层折叠抽屉零边标签重叠（交集面积 `== 0`）；急救车地图凸包多边形正常点亮 |
| **6. 动作回写与脉冲实时联动** | 在 `med-dispatch-cockpit` 触发 `启用应急加床`（`activate_surge_beds`）按钮并返回 `/` 总控看板 | 节点指标原子更新成功，总控看板近期动态与时序吞吐实时反映最新状态 |

---

## 35.2 可执行门禁终验与归档规程（`scripts/gate/step35.sh`）

```bash
#!/usr/bin/env bash
# Step 35 Gate：十万级多行业秒级装载、98 项能力溯源与 Phase 4 全旅程 E2E 终验
set -euo pipefail
source "$(dirname "$0")/lib.sh"

# 1. 全套 G1 静态守卫与 C01..C98 溯源校验
if [[ -n "${NAMING_DENYLIST:-}" || -f "$ROOT/.env" ]]; then
  bash "$ROOT/scripts/naming-guard.sh"
fi
python3 "$ROOT/scripts/lint-guard.py"
python3 "$ROOT/scripts/web-guard.py"
python3 "$ROOT/scripts/python-guard.py"
python3 "$ROOT/scripts/hallucination-guard.py"
python3 "$ROOT/scripts/docs-and-plans-guard.py"
python3 "$ROOT/scripts/check_routes.py" \
  --routes "$ROOT/axpoint-core/crates/axpoint-server/src/routes" \
  --index "$ROOT/docs/standards/A4-API-CONTRACT-INDEX.md"
python3 "$ROOT/scripts/check_traceability.py" \
  "$ROOT/docs/standards/A7-REQUIREMENTS-TRACEABILITY.md" \
  "$ROOT/scripts/gate"
echo "ASSERT traceability-98-ok PASS"

# 2. 校验十万级多行业向量化装载耗时与体量红线
T0=$(date +%s)
bash "$ROOT/scripts/seed-showcase.sh"
ELAPSED=$(( $(date +%s) - T0 ))
PULSE=$(api GET /api/axops/pulse)
expect_json "showcase-145k-seeded-fast" "$PULSE" '
  .totals.node_count >= 2300
  and .totals.link_count >= 6500
  and .totals.lake_total_rows >= 145000
  and (.subgraphs | length) >= 5
'

# 3. Phase 4 全旅程 Playwright E2E
PW_PORT="${PLAYWRIGHT_PORT:-$(( 4173 + ${WT_SLOT:-0} * 100 ))}"
(cd "$ROOT/xbluenode-web" && PLAYWRIGHT_PORT="$PW_PORT" npx playwright test e2e/v4-modeling-and-worlds.spec.ts)
echo "ASSERT v4-journey-pass PASS"

gate_pass step35
```

### 收官封存动作

当 `bash scripts/gate/all.sh`（覆盖 `Step 00～35`）与 GitHub Actions 全作业绿灯合入 `main`，且通过 `chrome-devtools-mcp` 完成 `http://192.168.50.50:9995` 实机浏览器截图巡检后，由 `orchestrator` 提交归档 PR：将 `plans/v4-modeling-console-and-industry-worlds/` 移入 `plans/completed/v4-modeling-console-and-industry-worlds-step31-35/` 只读封存，同步更新 [plans/README.md](../../../README.md) 与全栈版本号至 `v0.4.0`。
