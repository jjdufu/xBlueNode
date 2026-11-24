# 03 · Step 25：一等公民可视化制图器、VTable 多维透视表与驾驶舱磁吸栅格

本步聚焦数据洞察与可视呈现体验的全面跃升：① 新增独立图表资产表 `xv_charts`，将**可视化制图工作台（Chart Studio）**提升为「探索与制图」的一等公民视图，使用户在问数、SQL 或语义指标查询后可 1 步完成高级图表编码并一键钉入任意孪生驾驶舱（消除原有的 4 层深藏路径）；② 按需引入 `@visactor/react-vtable` 升级百万行 Canvas 级多维透视表；③ 封装统一的 ECharts 6 视觉设计令牌与内嵌 Sparkline 趋势火花线的 `KpiWidget`；④ 升级 `xBlueView` 驾驶舱网格编辑层，提供可视背景列轨、8 向显式缩放手柄、边缘磁吸辅助线、自由浮动防挤压模式与一键整齐排布（Tidy Up）。

总览索引与工程约束见 [00-V3-MASTER-INDEX.md](../00-V3-MASTER-INDEX.md)、[01-STEP23](./01-STEP23-IA-CONVERGENCE-I18N-PURITY-AND-RESIZABLE-SHELL.md) 与 [IRON_RULES.md](../../../../IRON_RULES.md)。

---

## 25.0 目标、前置与契约

- **目标**：
  1. **可复用图表资产库（`xv_charts`）与一等公民 `ChartStudioPanel`**：支持将 SQL、透视、认证指标或已发布对象查询配置为独立持久化的 16 类可视化图表（柱状、堆叠柱、水平条形、折线、渐变面积、双轴组合、环形/玫瑰饼图、散点气泡、桑基流向、漏斗转化、热力矩阵、雷达、仪表盘、树图、带火花线 KPI 卡、条件色阶明细表），并提供 `POST /api/xblueview/charts/{id}/pin` 一键投送至目标驾驶舱指定语义分区；
  2. **`@visactor/react-vtable` 高性能多维透视引擎**：替换原手写 DOM 透视表格，支持海量单元格 60fps 虚拟化 Canvas 渲染、多级行/列维度树形折叠展开、表头与首列吸顶冻结、鼠标拖拽列宽、单元格内嵌迷你数据条（Data Bar）与红绿双极热力色阶；
  3. **统一图表设计系统（`chartTheme.ts`）与高密度 `KpiWidget`**：开放柔和弹簧过渡动画（`animationDuration: 280`），配置亮/暗双模 10 色语义调色盘、柱顶圆角、面积线性渐变、阈值警戒线 `markLine`、自适应中文 `万/亿` 与英文 `K/M/B` 坐标轴缩写，移除图表底部冗余分类按钮条（统一采用图元点击丝滑下钻与顶部面包屑回退）；为 `KpiWidget` 增加内嵌迷你趋势火花线（Sparkline）与目标达成进度条；
  4. **驾驶舱可视轨道磁吸网格（`DashboardGrid.tsx` 升级）**：编辑态底层渲染 12/24 列半透明虚线轨道槽，开启 8 向显式缩放锚点（`n, s, e, w, ne, nw, se, sw`），拖拽或缩放卡片接近邻居边缘时绘制蓝色磁吸对齐线；支持在看板配置中切换 `VERTICAL_COMPACT`（自动上紧）与 `FREE_FLOAT`（锁定坐标不挤飞下方卡片），并提供一键 `自动对齐整理`（按行高与列宽对齐消除零碎缝隙）。
- **前置**：[Step 23](./01-STEP23-IA-CONVERGENCE-I18N-PURITY-AND-RESIZABLE-SHELL.md) 通过。
- **覆盖能力项**：
  - `C80`：`step25` `"chart-asset-crud"`、`"pin-chart-to-dashboard"`
  - `C81`：`step25` `"vtable-pivot-render"`、`"grid-float-tidy"`
- **契约清单**：
  - 数据库迁移：`deploy/migrations/0020_saved_charts.sql`；
  - 公共枚举与审计动作：`GridCompactMode`（`VERTICAL_COMPACT | FREE_FLOAT`）、`AuditAction::ChartPublish`（同步更新 [A1](../../../../docs/standards/A1-CONVENTIONS-ENUMS-ROUTES.md) 与 [A2](../../../../docs/standards/A2-DATABASE-MIGRATIONS-INDEX.md)）；
  - 领域模型与端口：`axpoint-domain::view::{SavedChartRecord, SaveChartRequest, PinChartToDashboardRequest, CanvasConfig.compact_mode}`、`ViewRepository` 新增图表资产 CRUD 与钉入看板事务方法；
  - HTTP 路由：`/api/xblueview/charts`（`GET / POST`）、`/api/xblueview/charts/{id}`（`GET / PUT / DELETE`）、`/api/xblueview/charts/{id}/pin`（`POST`）。

### 文件变更清单

| 负责角色 | 动作 | 路径 | 内容 |
| :--- | :--- | :--- | :--- |
| `orchestrator` | 新建 | `deploy/migrations/0020_saved_charts.sql` | 新建 `xv_charts` 表（启用租户 RLS）并扩展 `xv_dashboards` 画布紧凑模式校验与 `CHART_PUBLISH` 审计动作（§25.1） |
| `orchestrator` | 修改 | `axpoint-core/crates/axpoint-domain/src/{enums.rs, view.rs, ports.rs}` | `GridCompactMode`、`SavedChartRecord`、`PinChartToDashboardRequest` 及 `ViewRepository` 端口签名（§25.2） |
| `orchestrator` | 修改 | `docs/API_CONTRACT.md`、`docs/standards/{A1,A2,A4,A7}`、`xbluenode-web/src/lib/api/{enums.ts, xblueview.ts}` | 同步图表资产契约与前端强类型 API 客户端 |
| `squad-domain-agent` | 修改 | `axpoint-core/crates/axpoint-db/src/view_repo.rs`、`axpoint-core/crates/axpoint-server/src/routes/xblueview.rs` | 实现 `xv_charts` 持久化、权限校验（`VIEW_EDIT` / `DATA_READ`）与原子钉入驾驶舱自动空位分配算法（§25.2.2） |
| `squad-web` | 修改 | `xbluenode-web/package.json`、`package-lock.json` | 新增 `@visactor/vtable`（`^1.16.0`）与 `@visactor/react-vtable`（`^1.16.0`） |
| `squad-web` | 新建 | `xbluenode-web/src/lib/chartTheme.ts` | 亮/暗双套 10 色语义调色盘、单位自适应缩写格式化器、圆角渐变与十字准星 Tooltip 构建器（§25.3.1） |
| `squad-web` | 新建 | `xbluenode-web/src/components/analytics/ChartStudioPanel.tsx`、`PinToDashboardModal.tsx` | 一等公民可视化制图工作台（字段拖拽/点选编码、16 类图表实时预览、阈值线配置、保存图表与一键钉入看板）（§25.3.2） |
| `squad-web` | 新建 | `xbluenode-web/src/components/analytics/VTablePivotCanvas.tsx`；修改 `PivotBuilder.tsx`、`PivotTableWidget.tsx` | 按需异步加载 `@visactor/react-vtable`，支持维度树收展、列宽拖拽、迷你数据条与热力色阶（§25.3.3） |
| `squad-web` | 修改 | `xbluenode-web/src/components/analytics/ChartPanel.tsx`、`src/components/xblueview/widgets/{EChartsWidget.tsx, KpiWidget.tsx}`、`src/components/xblueview/logic.ts` | 接入 `chartTheme.ts`、移除图表底部冗余按钮条、为 `KpiWidget` 增加内联 SVG Sparkline 火花线与目标进度条 |
| `squad-web` | 修改 | `xbluenode-web/src/components/xblueview/DashboardGrid.tsx`、`src/pages/{AnalyticsStudioPage.tsx, XBlueViewPage.tsx}` | 编辑态可视列轨背景、8 向手柄、边缘磁吸辅助线、`FREE_FLOAT` 防挤压切换与一键整齐排布算法（§25.4） |
| `squad-qa` | 新建 | `scripts/gate/step25.sh`、`xbluenode-web/e2e/step25-charts-vtable-grid.spec.ts` | 图表资产 RLS 隔离、1 步钉入看板、VTable 透视渲染与栅格防挤压门禁测试（§25.5） |

---

## 25.1 迁移 `deploy/migrations/0020_saved_charts.sql`

```sql
-- 0020_saved_charts.sql：独立可复用可视化图表资产表与审计动作扩展

CREATE TABLE IF NOT EXISTS xv_charts (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id       TEXT NOT NULL,
    name            TEXT NOT NULL CHECK (char_length(trim(name)) BETWEEN 1 AND 128),
    description     TEXT NOT NULL DEFAULT '',
    widget_kind     TEXT NOT NULL,
    data_source     JSONB NOT NULL DEFAULT '{}'::jsonb,
    encoding        JSONB NOT NULL DEFAULT '{}'::jsonb,
    options         JSONB NOT NULL DEFAULT '{}'::jsonb,
    owner_dept      TEXT NOT NULL DEFAULT 'default',
    sensitivity     TEXT NOT NULL DEFAULT 'INTERNAL'
                    CHECK (sensitivity IN ('PUBLIC', 'INTERNAL', 'CONFIDENTIAL', 'SECRET')),
    shared          BOOLEAN NOT NULL DEFAULT TRUE,
    created_by      UUID REFERENCES sec_users(id) ON DELETE SET NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, name)
);

CREATE INDEX IF NOT EXISTS idx_xv_charts_tenant_updated
    ON xv_charts (tenant_id, updated_at DESC);

SELECT ax_enable_tenant_rls('xv_charts');
```

---

## 25.2 图表资产与一键钉入驾驶舱契约

### 25.2.1 领域模型（`axpoint-domain::view`）

```rust
/// Grid compaction strategy in `CanvasConfig` (`03-STEP25` §25.4).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum GridCompactMode {
    /// Automatically compacts items upward when dragging or resizing.
    #[default]
    VerticalCompact,
    /// Preserves exact `(x, y)` coordinates without pushing unrelated below-items upward.
    FreeFloat,
}

/// Request payload for `POST /api/xblueview/charts/{id}/pin`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PinChartToDashboardRequest {
    /// Target `xv_dashboards.id` to mount a widget bound to this chart definition.
    pub dashboard_id: Uuid,
    /// Optional target semantic zone when the dashboard uses `STANDARD_FIVE_ZONE`.
    #[serde(default)]
    pub target_zone: Option<SemanticZoneKind>,
    /// Optional explicit grid width (`2..=24`); defaults by `widget_kind` (KPI=3/6, Chart=6/12).
    #[serde(default)]
    pub width: Option<i32>,
    /// Optional explicit grid height (`2..=16`); defaults by `widget_kind`.
    #[serde(default)]
    pub height: Option<i32>,
}
```

### 25.2.2 自动空位寻址与钉入事务（`routes/xblueview.rs`）

调用 `POST /api/xblueview/charts/{id}/pin` 时：
1. 校验调用者具备目标看板的 `VIEW_EDIT` 权限及源图表数据集/指标的读取权限；
2. 扫描目标看板现有 `xv_widgets` 占用矩阵：若目标看板为 `STANDARD_FIVE_ZONE` 且传入了 `target_zone`（默认 KPI 类落 `KPI` 区、图表类落 `INSIGHT` 区），优先在该语义分区边界内自上而下、自左向右寻找首个不重叠的 `(x, y)` 矩形槽位；否则追加至当前最大 `y + h` 行首；
3. 创建 `xv_widgets` 记录（在 `options.source_chart_id` 保留溯源引用）并返回新组件 ID 与看板深链 `/view/{slug}`。

---

## 25.3 高级图表设计令牌、`ChartStudioPanel` 与 `@visactor/react-vtable`

### 25.3.1 统一视觉设计令牌（`src/lib/chartTheme.ts`）

1. **双主题无障碍 10 色语义调色盘**：
   - 亮色：`["#2563eb", "#0d9488", "#7c3aed", "#d97706", "#e11d48", "#0284c7", "#059669", "#4f46e5", "#ea580c", "#475569"]`
   - 暗色：`["#3b82f6", "#2dd4bf", "#a78bfa", "#fbbf24", "#fb7185", "#38bdf8", "#34d399", "#818cf8", "#fb923c", "#94a3b8"]`
2. **精细几何与微排版规范**：
   - 柱状图默认顶角圆角 `borderRadius: [4, 4, 0, 0]`（水平条形图 `[0, 4, 4, 0]`），最大柱宽 `barMaxWidth: 28`；
   - 折线/面积图默认启用 `smooth: 0.25`、`symbolSize: 5`（悬停放大至 `8`），面积填充采用主色 `22% → 0%` 垂直线性渐变；
   - 数值坐标轴标签统一经 `formatAxisCompactNumber(val, locale)` 缩写（中文 `12,000 → 1.2万`、`150,000,000 → 1.5亿`；英文 `12K`、`150M`），悬浮 Tooltip 卡片则精确展示完整千分位数值与彩色圆点图例对齐列表；
   - 移除 `EChartsWidget.tsx` 底部多余的分类按钮横条，保留点击柱体/扇区/折线点直接触发下钻并在卡片头部渲染轻量面包屑。
3. **高信息密度 `KpiWidget`**：
   - 当返回数据行数 `>= 2` 且存在时间序列列或配置了 `sparkline_field` 时，在数值右侧或底部自动绘制平滑内联 SVG 火花线（带首尾极值圆点与柔和渐变底）；
   - 当配置了 `target_value` 时，在底部渲染 `4px` 圆角目标达成进度条及达成率百分比。

### 25.3.2 一等公民 `ChartStudioPanel` 交互流

在 `/analytics`（探索与制图）中：
- 无论用户在 `智能问答`、`SQL 查询` 还是 `语义指标` 页签获得结果集，结果栏右上角均常驻主操作按钮 **`制图`** 与 **`钉入看板`**；
- 切入 `可视化制图` 页签后，左侧为可视化编码槽（`图表类型 16 宫格`、`横轴 / 维度`、`纵轴 / 度量（支持多序列与右轴折线双轴组合）`、`颜色分组`、`阈值警戒线`、`单位与小数位`），右侧实时全尺寸渲染图表预览与数据契约检查，顶部一键 **`保存图表`** 或 **`钉入看板`**。

### 25.3.3 `@visactor/react-vtable` 多维透视引擎（`VTablePivotCanvas.tsx`）

通过 `React.lazy` 按需加载 `@visactor/react-vtable` 的 `<PivotTable />` 组件：
- 完整对接后端 `/api/analytics/pivot` 返回的行列树与聚合度量，同步适配亮/暗 CSS 变量主题（表头微灰底、极细网格线、`tabular-nums` 右对齐数值列）；
- 工具栏提供一键开关：`[数据条]`（在主度量列内嵌柔和蓝色比例横条）、`[热力色阶]`（按列分位数自动着色高低极值）、`[冻结行列头]`，并支持直接将当前透视配置一键钉入驾驶舱。

---

## 25.4 `xBlueView` 驾驶舱可视轨道磁吸栅格升级

在 `DashboardGrid.tsx` 编辑模式（`isEditing === true`）下：
1. **可视列轨背景层**：通过纯 CSS `linear-gradient` + `calc()` 精确匹配当前 `cols`（12 或 24 列）、`rowHeight` 与 `margin: [12, 12]`，在画布底层绘制柔和的虚线单元格参考框，消除“白底盲拖”；
2. **8 向缩放手柄与磁吸参考线**：开启 `handles: ["n", "s", "e", "w", "ne", "nw", "se", "sw"]`，拖拽或缩放过程中当活动卡片四边与其余卡片边缘对齐时渲染 `1.5px` 蓝色贯穿对齐线；
3. **自由浮动防挤压（`FREE_FLOAT`）与一键整齐排布（`tidyDashboardLayout`）**：
   - 编辑工具栏提供分段开关 `[自动上紧 | 锁定位置]`：切至 `锁定位置` 时向 `GridLayout` 传入无垂直自动上推的碰撞整理器，移动上方卡片绝不打乱下方已排好的布局；
   - 提供 **`对齐整理`** 按钮：纯函数扫描同行近似高度卡片（`y` 差值 `<= 1` 格）自动拉齐顶边与高度，若同行总列宽处于 `[cols - 2, cols)` 自动等比微调填满整行，瞬间消除手工拖拽留下的参差锯齿。

---

## 25.5 可执行门禁验收（`scripts/gate/step25.sh`）

```bash
#!/usr/bin/env bash
# Step 25 Gate：xv_charts 租户隔离、问数 1 步制图钉入驾驶舱、VTable 透视渲染与驾驶舱防挤压整齐排布验收
set -euo pipefail
source "$(dirname "$0")/lib.sh"

# 1. C80（上）：创建可复用图表资产并校验跨租户 RLS 0 行可见
CHART_RESP=$(api POST /api/xblueview/charts '{
  "name": "各状态订单金额分布图",
  "widget_kind": "BAR_CHART",
  "data_source": {"kind": "SQL", "sql": "SELECT status, SUM(amount) AS total_amt FROM demo_orders GROUP BY status"},
  "encoding": {"x": "status", "y": ["total_amt"]},
  "options": {"unit": "元", "show_mark_line": true}
}')
CHART_ID=$(printf '%s' "$CHART_RESP" | jq -r .id)
OTHER_CNT=$(sql_app other "SELECT count(*) FROM xv_charts WHERE id = '$CHART_ID'")
[[ "$OTHER_CNT" == "0" ]] || { echo "FAIL: xv_charts RLS leak" >&2; exit 1; }
echo "ASSERT chart-asset-crud PASS"

# 2. C80（下）：1 步将图表资产钉入目标五区驾驶舱 INSIGHT 分区
DASH_25=$(api POST /api/xblueview/dashboards '{
  "name": "Step25 制图联动看板",
  "slug": "step25-chart-pin-'$(date +%s)'",
  "theme": "light",
  "zone_layout": "STANDARD_FIVE_ZONE"
}' | jq -r .id)

PIN_RESP=$(api POST "/api/xblueview/charts/$CHART_ID/pin" "{
  \"dashboard_id\": \"$DASH_25\",
  \"target_zone\": \"INSIGHT\"
}")
expect_json "pin-chart-to-dashboard" "$PIN_RESP" '
  (.widget_id | length) > 0
  and .semantic_zone == "INSIGHT"
'

# 3. C81：前端构建、异步分包检查与 Playwright VTable + 磁吸网格 E2E
python3 "$ROOT/scripts/web-guard.py"
(cd "$ROOT/xbluenode-web" && npm run build && npm test)
(cd "$ROOT/xbluenode-web" && npx playwright test e2e/step25-charts-vtable-grid.spec.ts e2e/xblueview.spec.ts e2e/analytics.spec.ts)
echo "ASSERT vtable-pivot-render PASS"
echo "ASSERT grid-float-tidy PASS"

gate_pass step25
```

**通过标准**：`scripts/gate/step25.sh` 末行输出 `GATE step25 PASS`；`xv_charts` 强制 RLS 生效，`@visactor/react-vtable` 独立异步分包加载无首屏膨胀，制图与钉入看板全流程畅通。
