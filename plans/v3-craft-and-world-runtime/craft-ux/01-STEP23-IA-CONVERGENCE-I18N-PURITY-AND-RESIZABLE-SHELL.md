# 01 · Step 23：导航层级收敛、严格双语纯净度、弹性分栏与视觉去拘谨升级

本步作为 Phase 3 前端交互重塑（Track A）的首发地基，系统性解决平台当前存在的三大表层结构痛点：① 侧边栏按后端模块平铺 13 项导致跨页心智割裂、高频可视化制图入口深埋 4 层；② 中文界面充斥内部研发代号尾缀（`productTag`）、页面标题模块英文冠名、技术黑话标语与未翻译的 `SCREAMING_SNAKE_CASE` 原始枚举；③ 全站侧栏与检查器写死固定像素宽度、三重套娃边框与过度局促的 10–12px 等宽小字造成的视觉拘谨感。

总览索引与工程约束见 [00-V3-MASTER-INDEX.md](../00-V3-MASTER-INDEX.md)、[IRON_RULES.md](../../../IRON_RULES.md)、[xbluenode-web/AGENTS.md](../../../xbluenode-web/AGENTS.md) 与 [A1-CONVENTIONS-ENUMS-ROUTES.md](../../../docs/standards/A1-CONVENTIONS-ENUMS-ROUTES.md)。

---

## 23.0 目标、前置与契约

- **目标**：
  1. **8 域任务流信息架构重构**：将侧栏主导航收敛为 **6 个高频业务工作域**（`概览`、`数据与知识`、`探索与制图`、`本体工作室`、`流程与智能体`、`推演沙盘`、`孪生看板`——其中 `概览` 置顶，主任务流共 6 项，加底部折叠 `系统治理` 区含 `权限与安全`、`运维与备份`，合计 8 个一级任务入口），对历史独立路径（`/ingest`、`/query`、`/graph`、`/alerts`）提供带查询参数保留的前端无感重定向，确保既有书签与构建深链 100% 兼容。
  2. **全局 `⌘K / Ctrl+K` 命令直达面板（`CommandPalette.tsx`）**：引入轻量无头命令原语 `cmdk`（`^1.x`），支持按关键字模糊搜索并 1 秒直达任意工作台视图、已发布领域对象、语义指标、图表资产、编排流水线、推演沙盘与驾驶舱。
  3. **品牌唯一性与单语 100% 纯净化**：品牌名 `xBlueNode` 仅在全局顶栏左上角、登录页品牌区与侧栏底部版本锚点展示；彻底删除侧边栏 `productTag` 渲染节点，将 `MODULE_META` 的中文标题、英文标题与全部内页 `PageHeader` 清理为纯粹的领域业务词汇；全面清零中文界面残留的裸大写枚举码、单字母统计缩写（如 `4p·1m·0r`）与技术栈炫技标语；在 `scripts/web-guard.py` 新增 **双语纯净度与反代号外泄静态守卫**。
  4. **弹性可调分栏（`react-resizable-panels` v4）与去拘谨视觉升级**：引入 `react-resizable-panels` 封装标准 `ResizableWorkspace` 原语，全面替换全站死板的 `xl:w-[416px]`、`xl:w-[368px]`、`max-h-56` 固定尺寸，支持拖拽调宽、双击分隔线折叠与 `localStorage` 比例持久化；重构 `globals.css` 与 `primitives.tsx` 圆角、底色层次、留白与字号阶梯，收敛 `font-mono-tech` 适用边界。
- **前置**：[Step 22](../../completed/v2-ux-and-grounded-ai-step15-22/builder-and-ux/08-STEP22-V2-E2E-BENCHMARKS-AND-ACCEPTANCE.md) 已完成合入（**本步无数据库迁移与后端存储变更**）。
- **覆盖能力项**：
  - `C76`：`step23` `"ia-8-domains"`、`"legacy-route-redirect"`、`"cmdk-jump-ok"`
  - `C77`：`step23` `"zh-zero-codename"`、`"i18n-guard-ok"`、`"resizable-pane-persist"`

### 文件变更清单（按分队写入锁严格切分）

| 负责角色 | 动作 | 路径 | 内容 |
| :--- | :--- | :--- | :--- |
| `orchestrator` | 修改 | `docs/standards/A1-CONVENTIONS-ENUMS-ROUTES.md` | 修订 §3 页面注册表：更新 8 域主侧栏结构、兼容重定向路由映射与无品牌前缀标题规范（§23.1） |
| `orchestrator` | 修改 | `xbluenode-web/src/modules/meta.ts` | 移除 `productTag` 展示字段，重构 `NAV_GROUPS` 与纯净中英文 `MODULE_META` 标题真源（§23.1.2） |
| `orchestrator` | 修改 | `xbluenode-web/src/modules/registry.ts` | 注册 8 个主侧栏模块与历史路径 `<Navigate>` 兼容路由，挂载全局快捷键元数据（§23.1.3） |
| `orchestrator` | 修改 | `xbluenode-web/src/App.tsx` | 删除侧栏 `item.productTag` 渲染；底部收束折叠治理分组；顶栏中部/右侧接入 `⌘K` 搜索触发器（§23.1.4） |
| `orchestrator` | 修改 | `scripts/web-guard.py` | 新增 `I18N_PURITY_PATTERNS` 规则：拦截中文文案中的模块内部代号前缀与裸枚举泄露（§23.3.3） |
| `squad-web` | 修改 | `xbluenode-web/package.json`、`package-lock.json` | 新增 `react-resizable-panels`（`^4.0.0`）与 `cmdk`（`^1.1.1`）运行时依赖 |
| `squad-web` | 新建 | `xbluenode-web/src/components/ui/ResizableWorkspace.tsx` | 封装带拖拽高亮指示条、双击折叠与 `storageKey` 记忆的水平/垂直可调分栏原语（§23.4.1） |
| `squad-web` | 新建 | `xbluenode-web/src/components/ui/CommandPalette.tsx` | 全局 `⌘K` 跨模块资产与视图搜索直达面板（§23.2） |
| `squad-web` | 修改 | `xbluenode-web/src/styles/globals.css`、`src/components/ui/primitives.tsx` | 升级表面层级色值（画布微灰底 vs 纯白悬浮层）、圆角阶梯、13–14px 阅读字号与等宽字体收敛（§23.4.2） |
| `squad-web` | 修改 | `xbluenode-web/src/i18n/enums.ts` 及 `src/pages/*.tsx`、`src/components/**/*.tsx` | 合并双轨数据/探索/本体/流程视图切换器；全量替换 `4p·1m·0r`、`SUCCEEDED`、`SOURCE_INGEST`、`Cedar ABAC`、`OVERLAY · NEW` 等英文残留；换装 `ResizableWorkspace`（§23.3～§23.4） |
| `squad-qa` | 修改 | `xbluenode-web/e2e/*.spec.ts`；新建 `xbluenode-web/e2e/step23-ia-i18n-shell.spec.ts`、`scripts/gate/step23.sh` | 同步更新既有 E2E 中写死的旧品牌前缀标题断言；新增 8 域导航、兼容跳转、中英双态零泄漏 DOM 审计与分栏拖拽持久化测试（§23.5） |

---

## 23.1 8 域信息架构收敛与历史路由零断链兼容

### 23.1.1 侧栏一级导航目标矩阵（`src/modules/meta.ts` & `A1` §3）

主工作区聚焦业务创造闭环，治理与运维下沉至侧栏底部可折叠区，消除模块碎片感：

| 分组 | `TabKey` | 主路由 | 合并兼容的旧路由（自动带参跳转） | 中文侧栏与 H1 标题 | 英文侧栏与 H1 标题 | 页内顶层切换视图 |
| :--- | :--- | :--- | :--- | :--- | :--- | :--- |
| **顶层置顶** | `dashboard` | `/` | — | `概览` | `Overview` | 构建向导 · 模板中心 · 运行态势 |
| **核心工作域** | `axhub` | `/hub` | `/ingest` → `/hub?tab=knowledge` | `数据与知识` | `Data & Knowledge` | `连接源` · `数据集` · `知识文档` · `全局血缘` |
| **核心工作域** | `axanalytics` | `/analytics` | `/query` → `/analytics?tab=ask` | `探索与制图` | `Explore & Charts` | `智能问答` · `SQL 查询` · `透视分析` · `可视化制图` · `语义指标` |
| **核心工作域** | `xbluestudio` | `/studio` | `/graph` → `/studio?tab=explorer` | `本体工作室` | `Ontology Studio` | `模型设计` · `图谱浏览` · `版本与规格` |
| **核心工作域** | `axflow` | `/flow` | `/alerts` → `/flow?tab=alerts`<br/>`/agents` → `/flow?tab=agents`（或保留独立 `/agents` 作为第 5 核心域，见下条精确定义） | `编排与告警` | `Automation & Alerts` | `流程编排` · `规则告警` · `审批待办` |
| **核心工作域** | `axagent` | `/agents` | — | `智能体中心` | `Agents & APIs` | `对话工作台` · `智能体编排` · `接口与 MCP` · `IM 机器人` · `模型配置` |
| **核心工作域** | `axsim` | `/sim` | — | `推演沙盘` | `Simulation` | `拓扑与覆盖层` · `干预推演` · `对比与追问` · `决策下发` |
| **核心工作域** | `xblueview` | `/view` | `/view/:slug` | `孪生看板` | `Twin Cockpits` | `看板列表` · `报告出品` · `空间资产` |
| **底部系统治理** | `axguard` / `axops` | `/governance` | `/guard` → `/governance?section=security`<br/>`/ops` → `/governance?section=ops`（同时保留 `/guard` 与 `/ops` 子路由直接可渲染） | `系统治理` | `Governance & Ops` | `权限与脱敏` · `审计日志` · `健康与备份` · `任务与保留期` |

> **精确一级项数核定**：侧栏自上而下共 **8 个一级导航项**（`概览`、`数据与知识`、`探索与制图`、`本体工作室`、`编排与告警`、`智能体中心`、`推演沙盘`、`孪生看板`）+ 侧栏底部独立分隔的 **1 个治理入口（`系统治理`）**（对无 `GUARD_ADMIN / AUDIT_READ / OPS_ADMIN` 权限的普通业务角色仅展示其可见的跨部门授权或自动隐藏）。旧路径 `/ingest`、`/query`、`/graph`、`/alerts`、`/guard`、`/ops` 全部保留无感路由映射，保证外部链接、`AxBuilder` 深链与既有集成脚本 100% 正常打开目标面板。

### 23.1.2 `MODULE_META` 纯净化契约（`src/modules/meta.ts`）

```ts
export interface ModuleTitleMeta {
  readonly key: TabKey;
  readonly shortZh: string;
  readonly shortEn: string;
  readonly label: string;
  readonly enLabel: string;
  readonly group: NavGroupKey;
  readonly keywords?: readonly string[];
}
```

- 物理移除 `productTag?: string` 属性定义。
- 所有条目的 `label` 与 `shortZh` 统一为纯中文业务名词（如 `"数据与知识"`、`"本体工作室"`、`"推演沙盘"`），严禁包含 `"Ax*"`、`"xBlue*"` 或 `" · "` 前缀；`LoginPage.tsx` 一级标题由 `"AxGuard · 统一身份认证"` / `"AxGuard · Sign In"` 收敛为 `"登录"` / `"Sign In"`。

---

## 23.2 全局 `⌘K` 命令直达面板（`CommandPalette.tsx`）

在 `App.tsx` 顶栏右侧（语言切换左侧）放置紧凑搜索触发胶囊 `[🔍 搜索视图、模型与图表…  ⌘K]`，全局监听 `keydown`（`(e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k"`）：

1. **四组实时聚合索引**（打开面板时并发读取轻量缓存目录，零阻塞首屏）：
   - **快捷动作**：`新建连接源`、`上传解析文档`、`新建可视化图表`、`新建节点模型草案`、`新建推演沙盘`、`新建孪生看板`、`切换亮暗主题`、`切换中英语言`。
   - **工作台视图**：直达全部一级与二级页签（例如输入 `"透视"` 直达 `/analytics?tab=pivot`，输入 `"出品"` 直达 `/view?tab=exports`，输入 `"脱敏"` 直达 `/governance?section=security&tab=column_policies`）。
   - **领域模型与数据资产**：搜索当前租户 `xb_node_types`、`ds_datasets`、`an_metrics`，回车直接跳转对应节点检查器或指标详情。
   - **看板、沙盘与智能体**：搜索 `xv_dashboards`、`ax_sim_scenarios`、`ax_agents`，回车直达目标运行页。
2. 全程支持上下方向键、`Enter` 跳转、`Esc` 关闭，赋予 `data-testid="global-cmdk-dialog"` 与 `data-testid="global-cmdk-input"`。

---

## 23.3 双语单态纯净度清理与 `web-guard.py` 静态拦截

### 23.3.1 重点组件乱码式英文缩写与裸枚举根治清单

1. **建模节点卡片（`NodeTypeCard.tsx`）**：
   - 徽标计数由 `{counts.property_count}p·{counts.metric_count}m·{counts.rule_count}r` 改为双语函数格式化：中文 `"4 属性 · 1 指标"`（规则数 `> 0` 时追加 `" · 2 规则"`），英文 `"4 Attr · 1 Metric"`。
   - 字段行右侧类型由裸 `M·FLOAT64`、`STRING` 改为本地化精炼类型标签：`指标·浮点` / `文本` / `整数` / `金额` / `时间`（英文 `Metric·Float` / `Text` / `Int` / `Decimal` / `Timestamp`）。
   - 检查器当前焦点下拉框旁删除生硬的英文页签键名徽标（如 `basic`）。
2. **流程编排画布与算子面板（`FlowCanvas.tsx`、`OperatorPalette.tsx`）**：
   - `OperatorPalette.tsx` 删除算子标题下方的灰色 `spec.kind`（如 `SOURCE_INGEST`）副行，改为悬停气泡展示详细用途说明。
   - `FlowCanvas.tsx` 节点卡片将裸 `{node.kind}`、`{node.join}`、`CONTINUE` 及 `renderStepStatusBadge` 中硬编码的 `SUCCEEDED` / `WAITING_APPROVAL` / `RUNNING` / `FAILED` / `SKIPPED` / `PENDING` 全部替换为 `resolveOperatorTitle` 与 `tEnum(status)`（例如中文显示 `已成功`、`待审批`、`运行中 #2`、`失败 #1`）。
3. **推演沙盘、驾驶舱与安全页（`AxSimPage.tsx`、`DashboardGrid.tsx`、`WidgetFrame.tsx`、`AxGuardPage.tsx`、`DashboardPage.tsx`）**：
   - `AxGuardPage` 移除标题栏 `Cedar ABAC · RLS · SHA-256 Chain` 装饰性英文串；
   - `DashboardPage` 将 `5 + RRF` 改为 `tx("五路融合重排", "5-Way RRF")`，将 `64d` 改为 `tx("64 维", "64-dim")`；
   - `AxSimPage` / `OverlayEditor` 将 `1 种子 · 深度 4 · OUT`、`t = 0 · steady`、`OVERLAY · NEW`、`Schema v3` 全部接入 `tx()` 双语映射；
   - `DashboardGrid.tsx` 五区蓝图条移除冗余枚举前缀 `{zone} · `，仅渲染 `formatSemanticZoneLabel(zone, isZh)` 与栅格尺寸 `12×2`；`WidgetFrame.tsx` 编辑态类型徽标调用 `getWidgetKindLabel(widget.widget_kind, isZh)` 替代裸 `widget.widget_kind`。

### 23.3.2 `scripts/web-guard.py` 新增双语纯净度静态守卫规则

在 `scripts/web-guard.py` 中新增 AST/正则静态扫描规则 `check_i18n_purity`：
1. 扫描所有 `tx("...", "...")` 调用的第 1 个参数（中文分支字面量）：
   - 严禁出现内部模块工程代号前缀（正则 `\b(AxHub|AxIngest|AxParser|AxGraph|AxAnalytics|AxQuery|xBlueStudio|xBlueGraph|AxFlow|AxAgent|AxBuilder|AxSim|xBlueView|AxGuard|AxOps)\s*·`）；
   - 严禁出现连续大写下划线原始枚举字面量（正则 `\b[A-Z]{2,}_[A-Z0-9_]+\b`）。
2. 扫描 `src/modules/meta.ts`：禁止定义 `productTag` 属性，禁止 `label` 或 `shortZh` 出现 ASCII 模块代号前缀。

---

## 23.4 弹性可调分栏（`ResizableWorkspace`）与去拘谨视觉设计系统

### 23.4.1 `ResizableWorkspace` 组件契约（`src/components/ui/ResizableWorkspace.tsx`）

基于 `react-resizable-panels` 封装统一工作台分栏原语：

```tsx
export interface WorkspacePaneSpec {
  readonly id: string;
  readonly defaultSize: number; // 百分比 10..90
  readonly minSize?: number;
  readonly maxSize?: number;
  readonly collapsible?: boolean;
  readonly children: React.ReactNode;
}

export interface ResizableWorkspaceProps {
  readonly storageKey: string;
  readonly direction?: "horizontal" | "vertical";
  readonly panes: readonly WorkspacePaneSpec[];
  readonly className?: string;
}
```

- 分隔手柄（`PanelResizeHandle`）常态为 `1px` 极细边框色，悬停或拖拽时渐变为 `3px` `--color-xblue` 高亮指示条，支持键盘左右/上下微调与**双击分隔条快速折叠/还原侧栏**。
- 在以下 5 个全屏重交互工作台全面落地：
  1. `本体工作室 (/studio)`：中画布（默认 `68%`，可拖至 `85%`）| 右属性与 AI 助手分栏（默认 `32%`，可双击收起）+ 底部可拖拽高度校验与节点快照抽屉；
  2. `探索与制图 (/analytics)`：左数据目录树（`18%`，可折叠）| 右侧上下分栏（上 SQL/配置区 `42%` | 下结果网格与可视化制图区 `58%`）；
  3. `编排与告警 (/flow)`：左算子库（`16%`）| 中编排画布（`56%`）| 右节点配置与运行日志（`28%`）；
  4. `本体工作室 · 图谱浏览 (/studio?tab=explorer)`：左画布（`70%`）| 右节点详情、邻居与算法面板（`30%`）；
  5. `推演沙盘 (/sim)`：左干预与场景配置（`36%`）| 右因果拓扑画布与影响明细（`64%`）。

### 23.4.2 去拘谨视觉令牌升级（`globals.css` & `primitives.tsx`）

1. **空间底色分层（消除三重套娃框线）**：
   - 引入语义表面变量 `--canvas-bg`（亮色 `#f8fafc`、暗色 `#09090b`）与 `--panel-bg`（亮色 `#ffffff`、暗色 `#111114`）；
   - 画布类工作台取消外围多余 `p-3` 内边距与外框线，采用直通视口边缘的无边框画布（Edge-to-Edge Canvas），工具条改为顶部悬浮毛玻璃胶囊坞（`rounded-lg border border-border/80 bg-card/85 backdrop-blur-md shadow-sm`）。
2. **圆角、字号与字体呼吸感**：
   - 圆角阶梯：基础按钮与输入框 `rounded-md (6px)`，内容卡片与侧栏面板 `rounded-lg (8px)`，模态框与命令面板 `rounded-xl (12px)`；
   - 主体交互字号由全站清一色 `12px` 提升为：正文与导航 `13px`，表格与表单项 `13px`，次要元数据 `12px`，微徽标 `11px`；
   - `Badge` 与 `StatusBadge` 移除强制 `font-mono-tech` 类（中文状态词使用系统无衬线字体更舒展清晰，仅数字列保留 `tabular-nums`）。

---

## 23.5 可执行门禁验收（`scripts/gate/step23.sh`）

```bash
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
```

**通过标准**：`scripts/gate/step23.sh` 输出 `GATE step23 PASS`；切换中文/英文与亮色/暗色四象限下，全站无内部模块英文尾缀、无裸枚举码外泄、全部旧 URL 平滑落地对应视图。
