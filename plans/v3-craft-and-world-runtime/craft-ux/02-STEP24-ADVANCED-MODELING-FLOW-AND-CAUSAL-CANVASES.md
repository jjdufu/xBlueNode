# 02 · Step 24：建模、编排与因果图谱三大画布高阶交互重塑

本步彻底重塑平台内所有可拖拽节点画布与图谱可视化组件的**操作手感、视觉秩序、避障逻辑与信息密度**：在 `xBlueStudio` 与 `AxFlow` 中深度挖掘 `@xyflow/react` v12 + `elkjs` 的复合父子节点、字段级多端口正交避障、6 轴动态磁吸对齐参考线、框选对齐分布栏、平滑布局插值过渡与 `zundo` 50 步撤销重做栈；并按路由懒加载引入 `@antv/g6` v5（配合 `@antv/g6-extension-react`）重写 `AxSim` 因果传导画布与全站资产血缘画布，淘汰原有的 933 行手写原生 SVG 与随机弹跳力导图。

总览索引与工程约束见 [00-V3-MASTER-INDEX.md](../00-V3-MASTER-INDEX.md)、[01-STEP23](./01-STEP23-IA-CONVERGENCE-I18N-PURITY-AND-RESIZABLE-SHELL.md) 与 [xbluenode-web/AGENTS.md](../../../xbluenode-web/AGENTS.md)。

---

## 24.0 目标、前置与契约

- **目标**：
  1. **`xBlueStudio` 画布进阶工程化**：
     - **实测尺寸 + 多端口正交避障路由**：利用 `useNodesInitialized()` 获取真实卡片宽高后再触发 `elkjs`，开启 `'elk.edgeRouting': 'ORTHOGONAL'` 与 `'org.eclipse.elk.portConstraints': 'FIXED_ORDER'`，为每条关系边分配独立侧边端口与避障折线通道，彻底根治相邻卡片间距过窄及关系标签被遮挡截断的问题；
     - **业务域复合泳道（Compound Domain Group）与三层对象折叠卡片**：将具有相同 `domain_group` 的节点收纳进半透明可折叠业务域容器（React Flow `parentId` + `extent: 'parent'`）；支持将从属于同一主实体的 `EVENT_DYNAMIC`（单体动态事件）与 `CUBE_ROLLUP`（多维切片立方）折叠嵌入 `STATIC_FACT` 主实体卡片底部的分层抽屉槽，大幅收敛顶层画布连线复杂度；
     - **图谱拓扑态 ↔ 数据映射流水线态同源双视图切换**：画布顶部悬浮胶囊坞支持一键切换 `对象拓扑视图`（聚焦实体、关系与动作）与 `数据流水线视图`（左→右自动展开 `数据源集 → 映射转换 → 目标对象 → 聚合/派生规则`），两者共享同一份 `workingDoc` 实时双向同步；
     - **丝滑手感套件**：内置 6 轴动态磁吸对齐辅助线（左/中/右/顶/中/底阈值 `6px` 吸附并渲染高对比度对齐虚线）、框选多节点对齐与等距分布浮动栏、自动布局 240ms `requestAnimationFrame` 缓动插值动画，以及基于 `zundo` 的 50 步 `⌘Z / ⌘⇧Z` 画布级撤销重做。
  2. **`AxFlow` 编排画布直觉化操作升级**：
     - 取消“打开表单选下拉框连边”的繁琐交互，新增**连线悬停中点 `+` 快捷插入算子浮窗**（选定算子后自动切开原边并完成 `A → NewOp → B` 串联）；
     - 支持从左侧算子库直接将算子拖拽悬停至任意现有连线上高亮吸附，松手即自动切入串联；支持从节点右侧输出端口拖出连线在空白处松手直接呼出算子选择器创建下游节点；
     - 运行态连线升级为带方向脉冲动画的 SVG 流光边，节点底部内联展示步骤执行耗时（`142ms`）与输出记录数胶囊。
  3. **`@antv/g6` v5 因果传导与全站血缘画布重构**：
     - 按路由动态导入 `@antv/g6`（`^5.x`）与 `@antv/g6-extension-react`；
     - 重写 `AxSim` 的 `ImpactGraph.tsx`：支持节点自由拖拽微调、以光标为中心的丝滑无级滚轮缩放、`Combo` 按业务域折叠展开、`Hull` 高冲击风险簇半透明包络轮廓、区分节点内派生（虚线）与跨边因果传导（脉冲实线），并通过 React 自定义节点渲染**三轨并列微卡片（`基线值 | 干预值 | 推演终值` + 变化率色条 + P10–P90 迷你置信带）**；
     - 重写 `AxHub` 的 `LineageGraph.tsx`：采用确定性从左至右分层有向无环布局（`antv-dagre`），支持上游连接源、数据集、映射、业务节点、语义指标、编排流程与看板的多级折叠展开与高对比度上下游链路追踪。
- **前置**：[Step 23](./01-STEP23-IA-CONVERGENCE-I18N-PURITY-AND-RESIZABLE-SHELL.md) 通过（**本步无新增数据库迁移**；若与 [Step 26](../world-runtime/04-STEP26-ACTION-CONTRACTS-EDGE-DYNAMICS-AND-L4-CERT.md) 并行合入后，`StudioCanvas` 自动展示节点上的 `ActionSpec` 徽标，未合入前优雅兼容空列表）。
- **覆盖能力项**：
  - `C78`：`step24` `"elk-orthogonal-no-overlap"`、`"compound-tier-fold"`、`"canvas-undo-redo"`
  - `C79`：`step24` `"flow-edge-insert"`、`"g6-causal-combo-hull"`

### 文件变更清单

| 负责角色 | 动作 | 路径 | 内容 |
| :--- | :--- | :--- | :--- |
| `orchestrator` | 修改 | `xbluenode-web/src/lib/api/enums.ts`、`docs/standards/A1-CONVENTIONS-ENUMS-ROUTES.md` | 登记前端视图模式枚举 `StudioCanvasViewMode`（`ONTOLOGY_TOPOLOGY` / `MAPPING_PIPELINE`） |
| `squad-web` | 修改 | `xbluenode-web/package.json`、`package-lock.json` | 新增 `@antv/g6`（`^5.0.0`）、`@antv/g6-extension-react`（`^0.2.0`）、`zustand`（`^5.0.0`）、`zundo`（`^2.3.0`） |
| `squad-web` | 新建 | `xbluenode-web/src/components/canvas/alignmentGuides.ts`、`HelperLinesOverlay.tsx`、`SelectionAlignToolbar.tsx` | 通用 6 轴磁吸对齐坐标纯函数计算器、SVG 对齐参考线覆盖层与多选对齐/等距分布工具条（§24.1.1） |
| `squad-web` | 新建 | `xbluenode-web/src/components/canvas/elkOrthogonalLayout.ts`、`useAnimatedNodes.ts`、`useCanvasHistory.ts` | 支持复合父子节点与多端口约束的 ELK 正交布局器、240ms 平滑坐标过渡 Hook 与 `zundo` 50 步历史栈（§24.1.2） |
| `squad-web` | 新建 | `xbluenode-web/src/components/xbluestudio/DomainGroupNode.tsx`、`PipelineViewCanvas.tsx` | 业务域毛玻璃泳道容器组件、三层对象折叠抽屉渲染与数据映射流水线同源视图（§24.1.3） |
| `squad-web` | 修改 | `xbluenode-web/src/components/xbluestudio/{StudioCanvas.tsx, NodeTypeCard.tsx}`、`src/pages/XBlueStudioPage.tsx` | 接入多端口防重叠连线、磁吸线、域泳道折叠、三层收纳、双模切换胶囊与 `⌘Z / ⌘⇧Z` 快捷键 |
| `squad-web` | 新建 | `xbluenode-web/src/components/axflow/InsertableEdge.tsx`、`QuickOperatorPopover.tsx` | 连线中点 `+` 插入算子交互边、连线拖拽吸附切入与端口松手建节点浮窗（§24.2） |
| `squad-web` | 修改 | `xbluenode-web/src/components/axflow/FlowCanvas.tsx`、`src/pages/AxFlowPage.tsx` | 挂载直觉连线操作、运行态脉冲流光边、耗时微徽标与画布历史栈 |
| `squad-web` | 新建 | `xbluenode-web/src/components/axsim/SimNodeCardG6.tsx`；重写 `src/components/axsim/ImpactGraph.tsx` | 基于 `@antv/g6` v5 的三轨微卡片节点、`Combo` 域分组、`Hull` 风险包络与分步波次脉冲高亮（§24.3.1） |
| `squad-web` | 重写 | `xbluenode-web/src/components/axhub/LineageGraph.tsx` | 基于 `@antv/g6` v5 分层 DAG 布局的确定性多级可折叠资产血缘画布（§24.3.2） |
| `squad-qa` | 新建 | `xbluenode-web/e2e/step24-advanced-canvases.spec.ts`、`scripts/gate/step24.sh` | 画布几何无遮挡校验、磁吸对齐、三层折叠、边中点插算子、撤销重做与 G6 因果图自动化门禁（§24.4） |

---

## 24.1 `xBlueStudio` 画布高阶工程设计

### 24.1.1 6 轴动态磁吸对齐辅助线与多选分布工具（`alignmentGuides.ts`）

在拖拽任意节点（`onNodeDrag`）时，基于 React Flow `node.measured` 真实包围盒实时计算与视口内其余可见节点的 6 条对齐轴（左边缘 `left`、水平中心 `centerX`、右边缘 `right`、顶边缘 `top`、垂直中心 `centerY`、底边缘 `bottom`）：

```ts
export interface AlignmentSnapResult {
  readonly snappedX: number | null;
  readonly snappedY: number | null;
  readonly verticalLines: readonly { x: number; y1: number; y2: number }[];
  readonly horizontalLines: readonly { y: number; x1: number; x2: number }[];
}

export function computeAlignmentGuides(
  dragged: MeasuredBox,
  others: readonly MeasuredBox[],
  thresholdPx = 6
): AlignmentSnapResult;
```

- 当距离小于 `6px` 时自动吸附坐标，并在 `HelperLinesOverlay` 中以 `--color-xblue` `1px` 虚线精准绘制起止于两节点外包络的参考线段（拖拽结束 `onNodeDragStop` 立即清除）。
- 开启 `selectionOnDrag`（按住 `Shift` 或切换框选模式框选 `>= 2` 个节点时），画布顶部自动浮现 `SelectionAlignToolbar`：`左对齐`、`水平居中`、`右对齐`、`顶对齐`、`垂直居中`、`水平等距分布`、`垂直等距分布`。

### 24.1.2 实测宽高 + 多端口 ELK 正交避障路由（`elkOrthogonalLayout.ts`）

为彻底消除平行多边重合与卡片遮挡关系标签的问题：
1. **双侧动态多端口分配**：对每张节点卡片，按关联入边数 $k_{\text{in}}$ 与出边数 $k_{\text{out}}$ 在卡片左/右侧均匀生成独立 `<Handle id={portId} />`，并在映射模式下支持直接锚定至具体属性行高度。
2. **ELK 布局参数固定契约**：
   - `'elk.algorithm': 'layered'`
   - `'elk.direction': 'RIGHT'`
   - `'elk.edgeRouting': 'ORTHOGONAL'`
   - `'elk.layered.spacing.nodeNodeBetweenLayers': '220'`（留足关系胶囊完整展示宽度，杜绝标签被相邻卡片遮盖）
   - `'elk.spacing.nodeNode': '96'`
   - `'elk.spacing.edgeEdge': '28'`
   - `'elk.hierarchyHandling': 'INCLUDE_CHILDREN'`（原生支持业务域复合泳道内子图协同排布）
   - 节点级 `'org.eclipse.elk.portConstraints': 'FIXED_ORDER'`
3. **240ms 平滑过渡插值（`useAnimatedNodes`）**：触发“自动排布”或“折叠/展开三层对象”时，以三次贝塞尔缓动曲线 `easeOutCubic` 在 `240ms` 内插值过渡所有节点坐标，告别生硬闪跳。

### 24.1.3 业务域复合泳道、三层对象折叠与双视图同源切换

1. **业务域泳道（`DomainGroupNode`）**：当画布开启 `按域分组` 开关时，所有具有非空 `domain_group` 的节点自动挂载至对应 `type: "domainGroup"` 父容器下；容器头部展示域名、包含对象数与折叠按钮，折叠后容器收缩为紧凑摘要卡片，跨组关系边自动汇聚至域容器端口。
2. **三层对象折叠收纳**：对 `semantic_layer_tier === "EVENT_DYNAMIC"` 或 `"CUBE_ROLLUP"` 且通过 `1:N` 归属边连接至唯一 `STATIC_FACT` 主档实体的从属节点，主档卡片右上角提供 `[收纳从属层]` 开关——开启后从属事件表与切片立方直接折叠为主档卡片底部的紧凑层叠标签槽（点击任意槽位直接在右侧检查器切换对应 `NodeTypeSpec` 编辑），使包含数十张日/月汇总表的复杂业务模型在顶层画布始终保持清爽的主干拓扑。
3. **同源双模画布（`ONTOLOGY_TOPOLOGY` ↔ `MAPPING_PIPELINE`）**：顶部悬浮工具坞左侧设分段切换器 `[对象拓扑 | 映射流水线]`。切至 `映射流水线` 时，自动从 `workingDoc.mappings` 派生四列只读/可配置流水线画布（`数据集源表 → 字段映射规则 → 业务对象类型 → 聚合与派生规则`），在任一视图中新增映射或修改规则均实时写回同一 `workingDoc` 历史栈。

---

## 24.2 `AxFlow` 直觉化连线与运行态视觉反馈

1. **连线悬停中点 `+` 一键切入算子（`InsertableEdge.tsx`）**：
   - 自定义边在路径几何中点渲染常驻微圆点（默认直径 `6px`，鼠标移近连线 `24px` 范围内平滑放大为 `20px` 蓝色 `+` 按钮）；
   - 触发 `+` 后原地弹出带搜索框的 `QuickOperatorPopover`，选定算子 `Op` 后在同一历史事务内原子执行：删除原边 `(u, v)`、在 `(u, v)` 中点坐标插入新节点 `Op`、新建边 `(u, Op)`（继承原 `when` 守卫条件）与 `(Op, v)`，并自动触发局部避障微调。
2. **拖拽算子吸附切线 & 端口拖出建节点**：
   - 拖拽左侧算子经过已有连线缓冲区（距离贝塞尔/正交线段 `< 18px`）时，该连线高亮为 `3px` 蓝色虚线并显示 `松手插入此处` 徽标，释放即自动串联；
   - 从任意节点输出端口拖出连线至空白画布松开鼠标（`onConnectEnd` 未命中目标 Handle）时，在落点直接弹出 `QuickOperatorPopover`，选定即一步完成建点与连线。
3. **运行态动态流光**：处于 `RUNNING` 状态的节点上游入边渲染 `stroke-dasharray: 6 4` 循环流动动画，已完成节点右上角显示绿色耗时胶囊（如 `84ms`）。

---

## 24.3 `@antv/g6` v5 因果传导与血缘图工程规范

为兼顾首屏加载速度，`@antv/g6` 与 `@antv/g6-extension-react` 封装于异步分包模块 `src/components/graph/g6Engine.ts`（通过 `React.lazy` 按需加载）：

### 24.3.1 `AxSim` 因果传导画布（`ImpactGraph.tsx` & `SimNodeCardG6.tsx`）

1. **三轨高密度微卡片节点（`SimNodeCardG6`）**：
   - 卡片宽 `228px`，顶部色条区分 `外生干预源（蓝色）`、`高冲击受损节点（琥珀/红色，按 |delta_pct| 分级）` 与 `传导桥接节点（中性灰）`；
   - 主体采用三列对齐数字网格同屏展示受影响首要指标：**`基线` | `干预` | `推演值（含涨跌幅徽标）`**；若启用了蒙特卡洛抽样，底部绘制一条 `4px` 高的 `[P10 — 均值 — P90]` 置信区间横条。
2. **`Combo` 业务域折叠与 `Hull` 风险簇包络**：
   - 按节点的 `domain_group` 或 `type_name` 生成可双击收展的 `Combo` 分组框；
   - 当某连通子路径上存在 `>= 2` 个变化幅度超过 `15%` 的受冲击节点时，自动调用 G6 `hull` 插件绘制柔和的半透明警示轮廓包络，直观呈现风险扩散圈。
3. **分步回放波次脉冲**：拖动底部 `ReplayTimeline` 时间步滑块至第 $t$ 步时，第 $t$ 步新激活的传导边自动播放高对比度流光动画，未波及的远端节点自动降暗至 `35%` 不透明度聚焦视线。

### 24.3.2 全站确定性分层血缘图（`LineageGraph.tsx`）

采用 G6 `antv-dagre`（`rankdir: 'LR'`, `nodesep: 28`, `ranksep: 96`）替换原 ECharts 随机力导图：每种资产类别（连接源、上传文件、数据集、知识文档、节点类型、语义指标、流程、智能体、看板）配备统一直角圆角语义卡片与图标，点击任一节点高亮完整上下游依赖闭包，双击数据集或看板卡片可直接新标签或路由跳转至对应资产详情页。

---

## 24.4 可执行门禁验收（`scripts/gate/step24.sh`）

```bash
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
```

**几何零遮挡硬断言（`step24-advanced-canvases.spec.ts` 内置）**：在浏览器中读取所有 `[data-testid^="canvas-edge-"]` 标签元素的 `getBoundingClientRect()` 与所有 `[data-testid^="canvas-node-"]` 卡片元素的 `getBoundingClientRect()`，数学断言**任意关系标签矩形与任意节点卡片矩形的交集面积严格等于 `0`**，彻底锁死标签不被卡片遮挡截断。
