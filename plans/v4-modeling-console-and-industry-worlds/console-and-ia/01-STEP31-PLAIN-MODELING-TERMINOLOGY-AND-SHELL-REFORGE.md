# 01 · Step 31：平实建模语言规范、建模优先导航重排与单行系统设置重构

本步作为 Phase 4 壳层与信息架构重塑（Track A）的首发步骤，聚焦三项核心工程收口：① 将全站用户可见界面与产品文档统一收敛为平实直白的工程建模词汇（消除生硬学术术语外露）；② 顺着「看全局 → 接数据 → 建模型 → 查数制图 → 跑推演 → 配自动化 → 看板与智能体」的真实工程动线重排一级导航，将 **`建模工作室`** 提至核心前排；③ 移除侧栏底部多余的单元素折叠套娃，收敛为干净利落的底部单行 **`系统设置`** 入口。

总览索引与工程约束见 [00-V4-MASTER-INDEX.md](../00-V4-MASTER-INDEX.md)、[IRON_RULES.md](../../../IRON_RULES.md)、[xbluenode-web/AGENTS.md](../../../xbluenode-web/AGENTS.md) 与 [A1-CONVENTIONS-ENUMS-ROUTES.md](../../../docs/standards/A1-CONVENTIONS-ENUMS-ROUTES.md)。

---

## 31.0 目标、前置与契约

- **目标**：
  1. **全站 UI 与文档平实建模语言统一**：用户可见界面（侧栏、一级标题、徽标、权限显示名、模板摘要、AI 草案默认标题、算子说明）零学术黑话残留，统一采用 `业务模型 / Business Model`、`建模工作室 / Modeling Studio`、`业务对象 / Domain Object`、`模型草案 / Model Draft`、`拓扑摘要 / Topology Summary`；`⌘K` 命令面板在隐藏 `keywords` 中保留搜索别名兼容，不影响老用户检索习惯。
  2. **建模优先 8 域导航重排**：主导航区按真实业务构建闭环顺序排列为 8 项，将 `建模工作室`（`/studio`）紧随 `数据与知识`（`/hub`）排在第 3 位；全部历史路径（`/ingest`、`/query`、`/graph`、`/alerts`、`/governance`、`/guard`、`/ops`）保持带查询参数的前端无感重定向兼容。
  3. **底栏单行 `系统设置` 收敛**：删除 `App.tsx` 底栏外层可折叠分组按钮与重复子项套娃，固定渲染单行齿轮图标入口 `系统设置`（`Settings`，路由 `/settings`，内聚 `成员与权限`、`安全与脱敏`、`模型通道`、`任务与备份`、`审计日志`）。
  4. **静态守卫与历史门禁同步**：在 `scripts/web-guard.py` 增加 `check_plain_modeling_terminology` 规则防止学术黑话重回可见文案；同步更新既有 E2E 用例与 `step23.sh` 中的导航顺序及标题断言，保证 `Step 00～31` 全绿。
- **前置**：[Step 30](../../completed/v3-craft-and-world-runtime-step23-30/world-runtime/08-STEP30-V3-E2E-CRAFT-BENCHMARKS-AND-ACCEPTANCE.md) 已完成合入（**本步无数据库迁移与后端协议破坏性变更**）。
- **覆盖能力项**：
  - `C93`：`step31` `"ui-zero-ontology-jargon"`、`"nav-modeling-promoted"`、`"settings-flat-entry"`

### 文件变更清单（按分队写入锁切分）

| 负责角色 | 动作 | 路径 | 内容 |
| :--- | :--- | :--- | :--- |
| `orchestrator` | 修改 | `docs/standards/A1-CONVENTIONS-ENUMS-ROUTES.md`、`README.md` | 更新 §1 词表说明与 §3 页面注册表（新 8 域顺序、`/settings` 路由映射与平实标题规范） |
| `orchestrator` | 修改 | `xbluenode-web/src/modules/meta.ts` | 更新 `MODULE_META` 中英文标题（`总控看板 / Dashboard`、`建模工作室 / Modeling Studio`、`查询与制图 / Analytics & Charts`、`数字看板 / Cockpits`、`智能体与接口 / Agents & APIs`、`系统设置 / Settings`） |
| `orchestrator` | 修改 | `xbluenode-web/src/modules/registry.ts` | 调整 `MODULE_REGISTRY` 顺序（`xbluestudio` 排第 3），挂载 `/settings` 主路由并保留 `/governance`、`/guard`、`/ops` 兼容路由 |
| `orchestrator` | 修改 | `xbluenode-web/src/App.tsx` | 移除 `governance-section-toggle` 单元素折叠套娃，底部固定渲染单行 `sidebar-nav-settings`（保留 `data-testid` 兼容锚点或同步升级 E2E） |
| `orchestrator` | 修改 | `scripts/web-guard.py` | 新增 `check_plain_modeling_terminology`：拦截 `MODULE_META` 展示字段与 `tx()` 可见文案中出现学术黑话字面量 |
| `squad-web` | 修改 | `xbluenode-web/src/i18n/enums.ts`、`src/components/analytics/GroundingBadge.tsx`、`src/components/dashboard/TemplateHubModal.tsx`、`src/components/xbluestudio/AiDraftWizard.tsx`、`src/components/axagent/McpBuilderConfigCard.tsx`、`src/components/axguard/constants.ts`、`src/components/ui/CommandPalette.tsx`、`src/lib/api/axflow.ts`、`src/pages/GovernancePage.tsx` | 全面收敛可见微文案至平实建模词汇；优化 `/settings` 页内顶层分组页签体验 |
| `squad-qa` | 修改 / 新建 | `xbluenode-web/e2e/{smoke.spec.ts,step23-ia-i18n-shell.spec.ts,v3-craft-and-runtime.spec.ts}`；`scripts/gate/step23.sh`；新建 `xbluenode-web/e2e/step31-plain-shell.spec.ts`、`scripts/gate/step31.sh` | 同步历史 E2E 导航标签断言；新增平实语言全路由 DOM 扫描、建模置前顺序与 `/settings` 兼容跳转验收 |

---

## 31.1 建模闭环一级导航矩阵（`meta.ts` / `registry.ts` / `A1` §3）

| 区域与序号 | `TabKey` | 主路由 | 兼容旧路由映射 | 中文侧栏与 H1 | 英文侧栏与 H1 | 核心职责 |
| :--- | :--- | :--- | :--- | :--- | :--- | :--- |
| **主导航 #1** | `dashboard` | `/` | — | `总控看板` | `Dashboard` | 全域态势指标、多行业模型矩阵、实时吞吐图表与待办决策 |
| **主导航 #2** | `axhub` | `/hub` | `/ingest` → `/hub?tab=knowledge` | `数据与知识` | `Data Hub` | 多源连接器、流式/CDC 同步、湖表资产与知识文档 |
| **主导航 #3** | `xbluestudio` | `/studio` | `/graph` → `/studio?tab=explorer` | `建模工作室` | `Modeling Studio` | 对象、关系、派生/聚合规则、`ActionSpec` 动作契约、图谱浏览与试算认证 |
| **主导航 #4** | `axanalytics` | `/analytics` | `/query` → `/analytics?tab=ask` | `查询与制图` | `Analytics & Charts` | 受模型保护的智能问数、免连接查询、SQL、VTable 多维透视与制图工作台 |
| **主导航 #5** | `axsim` | `/sim` | — | `推演沙盘` | `Simulation` | 隔离克隆、定时/条件事件队列、因果传导、`deadline_step` 预警与方案对比 |
| **主导航 #6** | `axflow` | `/flow` | `/alerts` → `/flow?tab=alerts` | `编排与告警` | `Automation & Alerts` | DAG 自动化编排、实时指标规则监测、告警处置与动作审批 |
| **主导航 #7** | `xblueview` | `/view` | `/view/:slug` | `数字看板` | `Cockpits` | 多行业五区驾驶舱、磁吸栅格、3D 孪生场景、安全分享与全格式出品 |
| **主导航 #8** | `axagent` | `/agents` | — | `智能体与接口` | `Agents & APIs` | 领域智能体、22 工具构建与运行态 MCP 服务、REST/GraphQL 发布与 IM 机器人 |
| **底栏固定单行** | `governance` | `/settings` | `/governance`、`/guard`、`/ops` 自动映射对应分区 | `系统设置` | `Settings` | 成员与角色权限、行列与脱敏安全策略、模型通道、任务备份与审计哈希链 |

---

## 31.2 全站可见文案平实化对照表

后端内部 Rust 类型名与 JSON 协议枚举码（如 `DOMAIN_OBJECT`、`ONTOLOGY_TOPOLOGY`、`strict_ontology_join`）保持 100% 向后兼容不破坏接口；所有面向用户的界面文案按以下标准统一落地：

| 组件或字典文件 | 键名 / 场景 | 目标中文文案 | 目标英文文案 |
| :--- | :--- | :--- | :--- |
| `src/modules/meta.ts` | `xbluestudio` | `建模工作室` | `Modeling Studio` |
| `src/modules/meta.ts` | `dashboard` | `总控看板` | `Dashboard` |
| `src/modules/meta.ts` | `governance` | `系统设置` | `Settings` |
| `src/i18n/enums.ts` & `GroundingBadge.tsx` | `DOMAIN_OBJECT` | `业务对象` | `Domain Object` |
| `src/i18n/enums.ts` | `STUDIO_EDIT` | `业务建模编辑` | `Edit Model` |
| `src/i18n/enums.ts` | `STUDIO_PUBLISH` | `业务模型发布` | `Publish Model` |
| `src/components/dashboard/TemplateHubModal.tsx` | 模板结构标题 | `模型拓扑摘要` | `Model Topology` |
| `src/components/xbluestudio/AiDraftWizard.tsx` | 默认草案名称与占位符 | `AI 供应链模型草案` | `AI Supply Chain Draft` |
| `src/components/axagent/McpBuilderConfigCard.tsx` | 工具分组标题 | `业务模型与动作` | `Models & Actions` |
| `src/components/axguard/constants.ts` | 角色职责描述 | `多子图业务建模、沙盘推演与数字看板` | `Subgraph business modeling, simulation, and cockpits` |
| `src/lib/api/axflow.ts` | `XBLUE_MAP` 算子说明 | `将湖表数据按业务模型映射规则物化为节点与关系` | `Materialize lake tables into nodes and links via model mappings` |
| `src/components/ui/CommandPalette.tsx` | 跳转项展示名 | `建模工作室 · 图谱浏览`（`keywords` 保留隐藏搜索别名） | `Modeling Studio · Graph Explorer` |

---

## 31.3 `scripts/web-guard.py` 平实建模语言静态守卫

在 `scripts/web-guard.py` 新增检查函数 `check_plain_modeling_terminology(src_root)`：
1. 扫描 `xbluenode-web/src/modules/meta.ts` 的 `shortZh`、`shortEn`、`label`、`enLabel` 字段（排除 `keywords` 数组）；
2. 扫描 `xbluenode-web/src/` 下所有 `.ts` / `.tsx` 文件中的 `tx("...", "...")` 双语调用参数与 `ENUM_LABELS` 的 `zh` / `en` 显示值；
3. 一旦命中汉字 `"本体"` 或不区分大小写的英文单词 `\bontology\b`，立即报错退出非 `0` 状态码，从 CI 源头杜绝玄学词汇回流用户界面。

---

## 31.4 可执行门禁（`scripts/gate/step31.sh`）

```bash
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
```
