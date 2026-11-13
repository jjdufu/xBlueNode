# 10 · Step 11–12：xBlueView 孪生驾驶舱、图浏览与全格式出品

Step 11 交付可配置的孪生驾驶舱（17 种组件、全局筛选与联动、逐层下钻、地图与平面图、大屏模式）、基于事件的实时刷新与 WebGL 图浏览器。Step 12 交付全格式出品引擎（XLSX / CSV / PDF / DOCX / PPTX / Markdown，含服务端图表渲染与模板）与带期限、次数、口令和锁定筛选的安全分享。

---

# Step 11：孪生驾驶舱与图浏览

## 11.0 目标、前置与契约

- **目标**：驾驶舱增删改查与发布；组件数据统一执行入口（经 Step 04 安全护栏与 Step 13 策略）；缓存与基于事件的失效；SSE 实时推送（多实例一致）；下钻与联动；地图、平面图；大屏模式；图浏览器（sigma.js，万级节点）。
- **前置**：Step 04、Step 08、Step 10 通过。
- **契约清单**：`0011_xblueview_dashboards.sql`；`WidgetKind`（A1 §4）；`WidgetDataSource`、`WidgetDataResponse`、`ViewEvent`；`/api/xblueview/*`；`lib/api/xblueview.ts`；页面键 `xblueview`（`/view`）。

### 文件变更清单

| 动作 | 路径 | 内容 |
| :--- | :--- | :--- |
| 新建 | `deploy/migrations/0011_xblueview_dashboards.sql` | §11.1 |
| 新建 | `axpoint-core/crates/axpoint-db/src/view_repo.rs` | 驾驶舱、组件、资产、摆放 |
| 新建 | `axpoint-core/crates/axpoint-server/src/view/{mod.rs, data.rs, cache.rs, deps.rs, events.rs}` | 数据执行、缓存、依赖、事件推送 |
| 新建 | `axpoint-core/crates/axpoint-server/src/routes/xblueview.rs` | §11.7 路由 |
| 修改 | `axpoint-core/crates/axpoint-server/src/routes/mod.rs` | `.merge(xblueview::router())` |
| 修改 | `axpoint-core/crates/axpoint-server/src/background.rs` | 注册 `ViewEventSubscriber` |
| 新建 | `xbluenode-web/src/pages/XBlueViewPage.tsx` | 驾驶舱列表、编辑、查看、大屏 |
| 新建 | `xbluenode-web/src/components/xblueview/{DashboardGrid.tsx, WidgetFrame.tsx, WidgetEditor.tsx, FilterBar.tsx, DrillBreadcrumb.tsx, NodeDrawer.tsx, widgets/*.tsx, MapWidget.tsx, FloorPlanWidget.tsx, BigScreen.tsx}` | 组件 |
| 修改 | `xbluenode-web/src/pages/XBlueGraphPage.tsx`；新建 `src/components/graph/{SigmaCanvas.tsx, GraphToolbar.tsx, PathPanel.tsx, AlgoLegend.tsx}` | 图浏览器 |
| 新建 | `xbluenode-web/src/lib/api/xblueview.ts`、`src/lib/sse.ts` | API 与 SSE 客户端 |
| 修改 | `xbluenode-web/src/modules/registry.ts` | 注册 `xblueview` |
| 新建 | `scripts/seed/11-xblueview.sh`、`scripts/gate/step11.sh`、`xbluenode-web/e2e/xblueview.spec.ts` | 种子、门禁、端到端 |

---

## 11.1 迁移 `deploy/migrations/0011_xblueview_dashboards.sql`

```sql
-- 0011_xblueview_dashboards.sql：驾驶舱、组件、资产、平面图摆放

CREATE TABLE IF NOT EXISTS xv_dashboards (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id       TEXT NOT NULL,
    name            TEXT NOT NULL,
    slug            TEXT NOT NULL CHECK (slug ~ '^[a-z0-9][a-z0-9-]{0,62}$'),
    description     TEXT NOT NULL DEFAULT '',
    filters         JSONB NOT NULL DEFAULT '[]'::jsonb,   -- 全局筛选定义 FilterDef[]
    theme           TEXT NOT NULL DEFAULT 'light' CHECK (theme IN ('light','dark','screen')),
    canvas          JSONB NOT NULL DEFAULT '{"cols":24,"row_height":30,"screen_width":1920,"screen_height":1080}'::jsonb,
    status          TEXT NOT NULL DEFAULT 'DRAFT' CHECK (status IN ('DRAFT','PUBLISHED')),
    version         INT NOT NULL DEFAULT 1,
    created_by      UUID,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, slug)
);

CREATE TABLE IF NOT EXISTS xv_widgets (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id     TEXT NOT NULL,
    dashboard_id  UUID NOT NULL REFERENCES xv_dashboards(id) ON DELETE CASCADE,
    widget_kind   TEXT NOT NULL CHECK (widget_kind IN (
        'KPI_CARD','TABLE','PIVOT_TABLE','LINE_CHART','BAR_CHART','PIE_CHART','SCATTER','SANKEY','HEATMAP',
        'GAUGE','RADAR','GRAPH_TOPOLOGY','MAP','FLOOR_PLAN','ALERT_LIST','SIM_COMPARE','MARKDOWN_NOTE')),
    title         TEXT NOT NULL DEFAULT '',
    position      JSONB NOT NULL,                         -- {x, y, w, h}
    data_source   JSONB NOT NULL DEFAULT '{"kind":"STATIC"}'::jsonb,
    encoding      JSONB NOT NULL DEFAULT '{}'::jsonb,     -- 字段到视觉通道的映射
    options       JSONB NOT NULL DEFAULT '{}'::jsonb,     -- 阈值、颜色、单位、格式、ECharts 覆盖项
    drill         JSONB NOT NULL DEFAULT '[]'::jsonb,     -- DrillLevel[]
    refresh_mode  TEXT NOT NULL DEFAULT 'LIVE' CHECK (refresh_mode IN ('LIVE','INTERVAL','MANUAL')),
    refresh_seconds INT NOT NULL DEFAULT 60 CHECK (refresh_seconds BETWEEN 5 AND 86400),
    deps          JSONB NOT NULL DEFAULT '{}'::jsonb,     -- 保存时计算：{dataset_ids, type_names, metrics, alerts, scenario_ids}
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS idx_xv_widgets_dashboard ON xv_widgets (dashboard_id);
CREATE INDEX IF NOT EXISTS idx_xv_widgets_deps ON xv_widgets USING GIN (deps);

CREATE TABLE IF NOT EXISTS xv_assets (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id   TEXT NOT NULL,
    asset_kind  TEXT NOT NULL CHECK (asset_kind IN ('FLOOR_PLAN_IMAGE','GEOJSON','ICON')),
    name        TEXT NOT NULL,
    file_path   TEXT NOT NULL,                            -- ${AXPOINT_DATA_ROOT}/assets/<tenant>/<id>.<ext>
    mime_type   TEXT NOT NULL,
    sha256      TEXT NOT NULL,
    width_px    INT,
    height_px   INT,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, name)
);

CREATE TABLE IF NOT EXISTS xv_floor_placements (
    tenant_id  TEXT NOT NULL,
    asset_id   UUID NOT NULL REFERENCES xv_assets(id) ON DELETE CASCADE,
    node_id    UUID NOT NULL REFERENCES xb_nodes(id) ON DELETE CASCADE,
    x_ratio    DOUBLE PRECISION NOT NULL CHECK (x_ratio BETWEEN 0 AND 1),
    y_ratio    DOUBLE PRECISION NOT NULL CHECK (y_ratio BETWEEN 0 AND 1),
    PRIMARY KEY (asset_id, node_id)
);

SELECT ax_enable_tenant_rls(t) FROM unnest(ARRAY[
    'xv_dashboards','xv_widgets','xv_assets','xv_floor_placements'
]::regclass[]) AS t;
```

---

## 11.2 组件数据源与执行

### 11.2.1 `WidgetDataSource`

```json
{"kind": "SAVED_QUERY", "saved_query_id": "…"}
{"kind": "METRIC", "metric": "paid_amount", "dimensions": ["status"], "time_grain": "DAY", "time_range": "LAST_30_DAYS"}
{"kind": "SQL", "sql": "SELECT status, sum(amount) AS amt FROM lake.demo_orders GROUP BY 1"}
{"kind": "NODE_METRIC", "type_name": "Warehouse", "metric": "inventory", "agg": "SUM", "grain": "HOUR", "time_range": "LAST_7_DAYS", "group_by": "node"}
{"kind": "PIVOT", "spec": { "...": "Step 04 PivotSpec" }}
{"kind": "GRAPH", "type_names": ["Factory","Warehouse"], "link_types": ["SUPPLIES"], "color_metric": "inventory", "limit": 2000}
{"kind": "ALERTS", "status": ["OPEN","ACK"], "severity": ["WARN","CRITICAL"], "limit": 50}
{"kind": "SIM_SCENARIOS", "scenario_ids": ["…","…"], "metrics": ["fulfillment"]}
{"kind": "NODES_GEO", "type_names": ["Factory","Warehouse"], "color_metric": "inventory"}
{"kind": "FLOOR_PLAN", "asset_id": "…", "color_metric": "temperature"}
{"kind": "STATIC", "rows": []}
```

| `kind` | 执行方式 | 依赖（`deps`） |
| :--- | :--- | :--- |
| `SAVED_QUERY` / `SQL` / `METRIC` / `PIVOT` | Step 04 `AnalyticsService`（`sql_guard`、超时、行数上限 5 000、Step 13 行列策略）；全局筛选与下钻条件以 `WHERE` 包装注入（`SELECT * FROM (<原 SQL>) AS _w WHERE <条件>`，条件值参数化，列名白名单为原 SQL 输出列） | SQL 中引用的湖表 → `dataset_ids`；`ax.nodes` / `ax.metric_*` → `type_names`、`metrics` |
| `NODE_METRIC` | Step 03 `/metrics/window` 同一实现 | `type_names`、`metrics` |
| `GRAPH` | Step 03 `/graph/subgraph`，附带 `color_metric` 值 | `type_names` |
| `ALERTS` | `ax_alerts` 查询 | `alerts=true` |
| `SIM_SCENARIOS` | Step 10 对比接口 | `scenario_ids` |
| `NODES_GEO` | `xb_nodes` 带 `geo_lng/geo_lat` 的节点 | `type_names` |
| `FLOOR_PLAN` | `xv_floor_placements` 联接节点指标 | `type_names`（摆放节点的类型） |
| `STATIC` | 直接返回 | 无 |

`deps` 在保存组件时由 `view/deps.rs` 计算（SQL 用 `sqlparser` 解析表名；`METRIC` 由语义层展开为 SQL 后解析）。

### 11.2.2 执行接口

`POST /api/xblueview/widgets/{id}/data`：

```json
{"filters": {"status": ["PAID"], "date_range": ["2026-01-01","2026-01-31"]}, "drill_path": [{"field": "warehouse_code", "value": "W-1"}]}
```

响应 `WidgetDataResponse`：`{columns: [{name, data_type}], rows: [[...]], row_count, truncated, generated_at, data_version, drill_level}`。`POST /api/xblueview/widgets/preview`（请求体含未保存的 `widget_kind/data_source/encoding`）供编辑器预览。

### 11.2.3 缓存（`view/cache.rs`）

- `moka::future::Cache`，键 `(tenant_id, widget_id, hash(filters, drill_path, principal 策略指纹))`，容量 10 000，TTL 600 秒。
- 策略指纹 = Step 13 中主体生效的行列策略集合的哈希（Step 13 之前为角色集合哈希），保证不同权限的用户不共享缓存。
- 失效：`ViewEventSubscriber` 收到事件后按 `deps` 找出受影响组件（`deps @> '{"dataset_ids":["…"]}'` 等 GIN 查询），删除这些组件的全部缓存条目（`invalidate_entries_if`）。

### 11.2.4 组件与数据形状

| `widget_kind` | 期望列 | 渲染 |
| :--- | :--- | :--- |
| `KPI_CARD` | 1 行：`value`，可选 `compare_value` | 数值、同比 / 环比箭头、阈值着色 |
| `TABLE` | 任意 | 虚拟滚动表格，列格式来自 `options.columns` |
| `PIVOT_TABLE` | Step 04 透视响应 | 带小计 / 合计的透视表 |
| `LINE_CHART` / `BAR_CHART` | `encoding.x`、`encoding.y[]`、可选 `encoding.series` | ECharts |
| `PIE_CHART` | `encoding.category`、`encoding.value` | ECharts |
| `SCATTER` | `encoding.x`、`encoding.y`、可选 `size`、`color` | ECharts |
| `SANKEY` | `source`、`target`、`value` | ECharts |
| `HEATMAP` | `x`、`y`、`value` | ECharts |
| `GAUGE` | 1 行 `value`，`options.min/max` | ECharts |
| `RADAR` | `indicator`、`value`、可选 `series` | ECharts |
| `GRAPH_TOPOLOGY` | `GRAPH` 数据源 | ECharts graph（≤ 2 000 节点；更大规模进入图浏览器） |
| `MAP` | `NODES_GEO` 或含 `lng`、`lat` 列的结果 | §11.4 |
| `FLOOR_PLAN` | `FLOOR_PLAN` 数据源 | §11.4 |
| `ALERT_LIST` | `ALERTS` 数据源 | 列表，可确认告警（需 `ALERT_ACK`） |
| `SIM_COMPARE` | `SIM_SCENARIOS` 数据源 | 分组柱图 + 差异表 |
| `MARKDOWN_NOTE` | 无 | Markdown（`react-markdown`，禁用原始 HTML） |

---

## 11.3 筛选、联动与下钻

- **全局筛选** `FilterDef`：`{key, label, kind: "SELECT"|"MULTI_SELECT"|"DATE_RANGE"|"NUMBER_RANGE"|"TEXT", field_map: {<widget_id>: "<列名>"}, options_source?: WidgetDataSource, default?}`；未出现在 `field_map` 中的组件不受该筛选影响。
- **联动**：组件 `options.cross_filter = {field, target_filter_key}`，点击图元把值写入对应全局筛选并刷新受影响组件。
- **下钻** `DrillLevel[]`：`[{field: "region"}, {field: "warehouse_code"}, {field: "order_id", node_type: "Order", node_key_field: "order_id"}]`。点击图元 → `drill_path` 追加 `{field, value}` 并以下一层 `field` 作为分组列重新请求（数据源为 `METRIC` 时替换维度；为 `SQL` 时要求原 SQL 输出包含全部下钻字段，组件在外层 `GROUP BY` 下一层字段并对 `encoding.y` 求和）；面包屑可回退任意层。最后一层带 `node_type` 时点击打开 `NodeDrawer`：节点属性、指标曲线（Step 03 指标接口）、关联文档、相关告警、“在图浏览器中打开”、“以此节点建沙盘”。

---

## 11.4 地图与平面图

- **地图**：ECharts `geo` 组件 + `scatter` / `effectScatter` 系列。底图为 `GEOJSON` 资产（管理员通过 `POST /api/xblueview/assets` 上传，例如省市边界）；未上传底图时以经纬度直角坐标绘制并显示坐标网格。配置 `AXPOINT_MAP_TILE_URL`（XYZ 瓦片模板）时改用 `maplibre-gl` 渲染瓦片底图与节点圆点图层。节点颜色按 `color_metric` 与 `options.thresholds` 着色，关系（`SUPPLIES` 等）可选绘制为飞线（`lines` 系列）。
- **平面图**：上传 `FLOOR_PLAN_IMAGE`（PNG / JPEG / SVG，SVG 在服务端按白名单清洗：删除 `script`、事件属性与外部引用）；编辑模式拖拽节点到图上写 `xv_floor_placements`（坐标存比例，适配任意尺寸）；查看模式按指标阈值着色并显示数值徽标，告警中的节点闪烁。
- 资产上传限制：单文件 ≤ 20 MB，GeoJSON 顶层必须是 `FeatureCollection`。

---

## 11.5 实时刷新（`view/events.rs`）

1. `ViewEventSubscriber`（`OutboxSubscriber` 实现）处理 `dataset.synced`、`node.metrics_changed`、`node.upserted`、`alert.opened`、`alert.resolved`、`sim.applied`：计算受影响组件 ID 集合，失效缓存，然后 `SELECT pg_notify('ax_view_events', $1)`，载荷 `{"tenant_id":"…","widget_ids":[…],"topic":"…"}`（超过 7 000 字节时只发 `{"tenant_id","dashboard_ids"}`）。
2. 每个 Core 实例启动一个 `PgListener` 监听 `ax_view_events`，再广播到进程内 `tokio::sync::broadcast::Sender<ViewEvent>`（容量 1 024）——因此任一实例消费外发事件，所有实例的 SSE 连接都会收到。
3. `GET /api/xblueview/dashboards/{id}/stream`（`DATA_READ`）：`axum::response::sse::Sse`，只转发属于该租户且涉及本驾驶舱组件的事件：`event: invalidate`、`data: {"widget_ids":[…]}`；告警事件额外发 `event: alert`；每 15 秒 `KeepAlive`。
4. 前端用 `@microsoft/fetch-event-source`（可携带 `Authorization` 请求头）连接；收到 `invalidate` 后对相关组件节流（同一组件 2 秒内最多一次）重新请求数据；连接断开时指数退避重连并在重连后全量刷新。
5. `refresh_mode = INTERVAL` 的组件按 `refresh_seconds` 轮询；`MANUAL` 只在用户点击时刷新。

---

## 11.6 驾驶舱页面与大屏

- `XBlueViewPage`：驾驶舱列表（搜索、复制、删除）→ 编辑模式（`react-grid-layout` 拖拽缩放，24 列栅格；右侧 `WidgetEditor` 选择类型、数据源、编码、样式、阈值、下钻；实时预览）→ 查看模式（筛选栏、联动、下钻、导出与分享按钮）。
- 发布：`POST /dashboards/{id}/publish`（`VIEW_EDIT`），`version + 1`，`status = PUBLISHED`；查看者（只有 `DATA_READ`）只能看到已发布驾驶舱。
- 运行页问数：查看模式右上角“问一问”调用 `POST /api/analytics/ask`，请求附带 `context: {dashboard_id, dataset_ids, filters, drill_path}`（`dataset_ids` 取本驾驶舱各组件 `deps.dataset_ids` 的并集），问数管线据此限定候选数据集并把筛选与下钻条件作为附加约束写入提示（Step 04 §04.6.7）；回答以表格与推荐图表展示，可一键“添加为组件”。
- 大屏：`/view/{slug}?screen=1`，按 `canvas.screen_width × screen_height` 设计尺寸等比缩放到窗口（CSS `transform: scale`），深色主题，隐藏侧栏，全屏 API。
- 图浏览器（`XBlueGraphPage`）：
  - `sigma` + `graphology`，布局 `graphology-layout-forceatlas2/worker`（Web Worker 中运行，不阻塞界面）；
  - 初始画布来自 `/graph/subgraph`，双击节点调用 `/graph/neighbors` 展开，右键“查找路径”调用 `/graph/path`；
  - 节点颜色可选：按类型、按指标、按图算法结果（`/graph/algorithms/{algo}/results` 的社区编号或中心性分数）；
  - 搜索框（`/nodes/search`）定位并高亮；
  - 1 万节点 / 3 万关系渲染帧率 ≥ 30 fps（基准环境浏览器测量）。

---

## 11.7 路由（`/api/xblueview` 下）

| 方法与路径 | 权限 |
| :--- | :--- |
| `GET /dashboards`、`POST /dashboards`、`GET /dashboards/{id}`、`PUT /dashboards/{id}`、`DELETE /dashboards/{id}` | `DATA_READ` / `VIEW_EDIT` |
| `POST /dashboards/{id}/publish`、`POST /dashboards/{id}/duplicate` | `VIEW_EDIT` |
| `GET /dashboards/by-slug/{slug}` | `DATA_READ` |
| `POST /dashboards/{id}/widgets`、`PUT /widgets/{id}`、`DELETE /widgets/{id}`、`PUT /dashboards/{id}/layout` | `VIEW_EDIT` |
| `POST /widgets/{id}/data`、`POST /widgets/preview` | `DATA_READ` |
| `GET /dashboards/{id}/stream` | `DATA_READ` |
| `GET /assets`、`POST /assets`（multipart）、`GET /assets/{id}/content`、`DELETE /assets/{id}` | `DATA_READ` / `VIEW_EDIT` |
| `GET /assets/{id}/placements`、`PUT /assets/{id}/placements` | `DATA_READ` / `VIEW_EDIT` |

---

## 11.8 验收

`scripts/seed/11-xblueview.sh`：创建驾驶舱 `supply-overview`（`KPI_CARD` 已支付金额（`METRIC paid_amount`）、`BAR_CHART` 按状态金额（`SQL`，下钻 `status → warehouse_code`）、`LINE_CHART` 仓库库存（`NODE_METRIC`）、`MAP`（`NODES_GEO`）、`ALERT_LIST`、`SIM_COMPARE`（Step 10 场景 s1、s2））并发布；输出 `DASHBOARD_ID`、`KPI_WIDGET_ID`、`BAR_WIDGET_ID`。

`scripts/gate/step11.sh`：

```bash
#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "$0")/lib.sh"

(cd axpoint-core && cargo test -p axpoint-server view::)
(cd xbluenode-web && npm run test -- xblueview)

# 数据正确：已支付金额 = 独立 SQL 计算
EXPECTED=$(sql_owner "SELECT sum(amount) FROM demo_src.orders WHERE status='PAID'")
expect_json "kpi" "$(api POST /api/xblueview/widgets/$KPI_WIDGET_ID/data '{}')" ".rows[0][0] | tonumber == $EXPECTED"

# 下钻
expect_json "drill" "$(api POST /api/xblueview/widgets/$BAR_WIDGET_ID/data '{"drill_path":[{"field":"status","value":"PAID"}]}')" \
  '.drill_level == 1 and ([.columns[].name] | index("warehouse_code")) != null'

# 实时刷新：订阅 SSE，触发同步，10 秒内收到 invalidate
SSE_LOG=$(mktemp)
curl -sN -H "Authorization: Bearer $AXPOINT_DEV_TOKEN" -H "Accept: text/event-stream" \
  "$CORE_BASE_URL/api/xblueview/dashboards/$DASHBOARD_ID/stream" > "$SSE_LOG" &
SSE_PID=$!
sleep 1
api POST /api/axhub/datasets/$DEMO_ORDERS_DATASET_ID/sync '{}' >/dev/null
wait_until "sse-invalidate" 30 "grep -q '^event: invalidate' $SSE_LOG"
kill $SSE_PID

# 缓存失效后数据反映新值
sql_owner "UPDATE demo_src.orders SET status='PAID' WHERE id=2" >/dev/null
api POST /api/axhub/datasets/$DEMO_ORDERS_DATASET_ID/sync '{}' >/dev/null
wait_dataset_idle "$DEMO_ORDERS_DATASET_ID"
EXPECTED2=$(sql_owner "SELECT sum(amount) FROM demo_src.orders WHERE status='PAID'")
wait_until "kpi-refreshed" 30 "api POST /api/xblueview/widgets/$KPI_WIDGET_ID/data '{}' | jq -e '.rows[0][0] | tonumber == $EXPECTED2'"
sql_owner "UPDATE demo_src.orders SET status='CANCELED' WHERE id=2" >/dev/null

# 跨租户隔离
expect_eq "rls-xv" "$(sql_app other "SELECT count(*) FROM xv_dashboards")" "0"

(cd xbluenode-web && npx playwright test e2e/xblueview.spec.ts)
gate_pass step11
```

`e2e/xblueview.spec.ts`：打开 `/view/supply-overview`，断言 6 个组件渲染完成；点击柱图 `PAID` 柱后面包屑出现“PAID”；切换 `?screen=1` 后画布按比例缩放。

**通过标准**：最后一行 `GATE step11 PASS`。

---

# Step 12：全格式出品与安全分享

## 12.0 目标、前置与契约

- **目标**：以统一文档模型 `ExportDocument` 出品 XLSX、CSV、PDF、DOCX、PPTX、Markdown；服务端 ECharts 渲染（SVG / PNG，中文字体）；用户模板（DOCX / PPTX 占位符）；后台任务、下载、审计、水印、保留清理；安全分享（`axs_` 令牌、期限、次数、口令、锁定筛选、撤销、访问审计）。
- **前置**：Step 11 通过。
- **契约清单**：`0012_export_and_share.sql`；`ExportFormat`（A1 §4）；`ExportDocument`（Rust 与 Python 两侧同构）；Sidecar `/export/{format}`、`/render/chart`；`/api/xblueview/exports*`、`/api/xblueview/shares*`、`/api/xblueview/share/{token}/*`；页面 `SharePage`（`/share/:token`）。

### 文件变更清单

| 动作 | 路径 | 内容 |
| :--- | :--- | :--- |
| 新建 | `deploy/migrations/0012_export_and_share.sql` | §12.1 |
| 新建 | `axpoint-parser/exporters/{__init__.py, document.py, xlsx.py, csv_.py, pdf.py, docx_.py, pptx_.py, markdown.py, templates/report.html.j2, templates/report.css}` | 出品器 |
| 新建 | `axpoint-parser/charts/{__init__.py, renderer.py, render.mjs, package.json, package-lock.json}` | 服务端图表 |
| 修改 | `axpoint-parser/main.py` | `/export/{format}`、`/render/chart` |
| 修改 | `deploy/docker/axpoint-parser.Dockerfile` | Node.js 运行时、`npm ci --omit=dev`（`charts/`）、`fonts-noto-cjk`、WeasyPrint 系统依赖 |
| 新建 | `axpoint-core/crates/axpoint-server/src/view/{export.rs, share.rs}` | 出品任务与分享 |
| 修改 | `axpoint-core/crates/axpoint-server/src/routes/xblueview.rs` | §12.6 路由 |
| 修改 | `axpoint-core/crates/axpoint-security/src/authenticator.rs` | `axs_` 前缀识别为 `SHARE`（只在分享路由中被接受） |
| 新建 | `xbluenode-web/src/pages/SharePage.tsx`、`src/components/xblueview/{ExportDialog.tsx, ShareDialog.tsx, ExportList.tsx}` | 前端 |
| 新建 | `axpoint-parser/tests/test_exporters.py`、`tests/test_charts.py` | 测试 |
| 新建 | `scripts/gate/step12.sh` | 门禁 |

---

## 12.1 迁移 `deploy/migrations/0012_export_and_share.sql`

```sql
-- 0012_export_and_share.sql：出品任务、出品模板、分享链接

CREATE TABLE IF NOT EXISTS xv_export_templates (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id     TEXT NOT NULL,
    name          TEXT NOT NULL,
    export_format TEXT NOT NULL CHECK (export_format IN ('DOCX','PPTX')),
    file_path     TEXT NOT NULL,
    sha256        TEXT NOT NULL,
    placeholders  TEXT[] NOT NULL DEFAULT '{}',          -- 上传时解析出的占位符
    created_by    UUID,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, name)
);

CREATE TABLE IF NOT EXISTS xv_export_jobs (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id     TEXT NOT NULL,
    export_format TEXT NOT NULL CHECK (export_format IN ('XLSX','CSV','PDF','DOCX','PPTX','MARKDOWN')),
    source        JSONB NOT NULL,                        -- ExportSource
    options       JSONB NOT NULL DEFAULT '{}'::jsonb,    -- {title, watermark, csv_bom, page_size, orientation}
    template_id   UUID REFERENCES xv_export_templates(id) ON DELETE SET NULL,
    job_id        UUID,                                  -- ax_jobs.id
    status        TEXT NOT NULL DEFAULT 'PENDING'
                  CHECK (status IN ('PENDING','RUNNING','SUCCEEDED','FAILED','CANCELED','EXPIRED')),
    file_name     TEXT,
    file_path     TEXT,
    file_size     BIGINT,
    sha256        TEXT,
    error_message TEXT,
    created_by    UUID NOT NULL,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    finished_at   TIMESTAMPTZ,
    expires_at    TIMESTAMPTZ
);
CREATE INDEX IF NOT EXISTS idx_xv_export_jobs_user ON xv_export_jobs (tenant_id, created_by, created_at DESC);

CREATE TABLE IF NOT EXISTS xv_share_links (
    id             UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id      TEXT NOT NULL,
    token_prefix   TEXT NOT NULL,
    token_hash     TEXT NOT NULL UNIQUE,                 -- hex(SHA-256(axs_<prefix8>_<secret32>))
    target_kind    TEXT NOT NULL CHECK (target_kind IN ('DASHBOARD','EXPORT_FILE','SCENARIO')),
    target_id      UUID NOT NULL,
    locked_filters JSONB NOT NULL DEFAULT '{}'::jsonb,   -- 访问者不可修改的筛选
    allow_filters  BOOLEAN NOT NULL DEFAULT FALSE,       -- 是否允许访问者调整其余筛选
    allow_export   BOOLEAN NOT NULL DEFAULT FALSE,
    password_hash  TEXT,                                 -- argon2id
    expires_at     TIMESTAMPTZ NOT NULL,
    max_views      INT CHECK (max_views IS NULL OR max_views > 0),
    view_count     INT NOT NULL DEFAULT 0,
    created_by     UUID NOT NULL,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    revoked_at     TIMESTAMPTZ
);

SELECT ax_enable_tenant_rls(t) FROM unnest(ARRAY[
    'xv_export_templates','xv_export_jobs','xv_share_links'
]::regclass[]) AS t;

-- 分享页无登录态：以令牌哈希解析租户与目标（只返回有效链接）
CREATE OR REPLACE FUNCTION ax_resolve_share(p_token_hash TEXT)
RETURNS TABLE (share_id UUID, tenant_id TEXT, target_kind TEXT, target_id UUID, created_by UUID,
               locked_filters JSONB, allow_filters BOOLEAN, allow_export BOOLEAN, password_hash TEXT,
               max_views INT, view_count INT)
LANGUAGE sql SECURITY DEFINER SET search_path = public AS $$
    SELECT s.id, s.tenant_id, s.target_kind, s.target_id, s.created_by, s.locked_filters,
           s.allow_filters, s.allow_export, s.password_hash, s.max_views, s.view_count
      FROM xv_share_links s
     WHERE s.token_hash = p_token_hash
       AND s.revoked_at IS NULL
       AND s.expires_at > NOW()
       AND (s.max_views IS NULL OR s.view_count < s.max_views);
$$;
REVOKE ALL ON FUNCTION ax_resolve_share(TEXT) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION ax_resolve_share(TEXT) TO axpoint_app;
```

---

## 12.2 统一文档模型 `ExportDocument`

Python（`exporters/document.py`，Pydantic）与 Rust（`axpoint-domain::export`，serde）字段一致：

```python
class ExportTable(BaseModel):
    kind: Literal["TABLE"] = "TABLE"
    title: str = ""
    columns: list[ColumnSpec]          # {name, data_type: DataType, format?: "#,##0.00" | "0.0%" | "yyyy-mm-dd"}
    rows: list[list[Any]]              # DECIMAL 以字符串传输，出品器转为 Decimal
    totals: list[Any] | None = None

class ExportChart(BaseModel):
    kind: Literal["CHART"] = "CHART"
    title: str = ""
    widget_kind: str                   # WidgetKind
    echarts_option: dict[str, Any]     # Core 以与前端相同的 encoding → option 规则生成
    data_table: ExportTable | None     # PPTX 原生图表与 XLSX 图表使用
    width_px: int = 960
    height_px: int = 540

class ExportKpi(BaseModel):
    kind: Literal["KPI"] = "KPI"
    label: str
    value: str
    compare: str | None = None

class ExportText(BaseModel):
    kind: Literal["HEADING", "PARAGRAPH", "MARKDOWN"]
    text: str
    level: int = 1

class ExportDocument(BaseModel):
    title: str
    subtitle: str = ""
    generated_at: datetime
    generated_by: str
    timezone: str
    watermark: str | None              # 例："张三 2026-01-31 16:00"
    sections: list[ExportTable | ExportChart | ExportKpi | ExportText]
    template_path: str | None = None
    placeholders: dict[str, str] = {}
```

`ExportSource`（Core 侧把来源转换为 `ExportDocument`）：

| `kind` | 内容 |
| :--- | :--- |
| `DASHBOARD` | `{dashboard_id, filters}`：按组件顺序（`position.y, position.x`）生成：KPI → `ExportKpi`；图表 → `ExportChart`（含数据表）；表格 / 透视 → `ExportTable`；Markdown → `ExportText`；地图 / 平面图 / 图拓扑 → 前端截图不在服务端执行，改为 `ExportChart`（ECharts 散点或关系图选项） |
| `WIDGET` | `{widget_id, filters, drill_path}` |
| `SAVED_QUERY` | `{saved_query_id}`：单表，CSV / XLSX 行数上限 1 000 000（XLSX 单表超过 1 048 575 行时自动分表） |
| `PIVOT` | `{spec}`：XLSX 保留小计 / 合计行样式与合并单元格 |
| `SCENARIO` | `{scenario_ids}`：KPI 对比、影响表、对比图 |
| `CONVERSATION` | `{conversation_id}`：问答记录（问题、回答、SQL、结果表） |

数据一律以**创建者**身份在执行时取数（`load_principal(created_by)`，行列策略生效），确保出品内容不超出创建者权限。

---

## 12.3 服务端图表（`axpoint-parser/charts/`）

- `render.mjs`：Node.js 常驻进程，逐行读取标准输入的 JSON 请求 `{id, option, width, height, format: "svg"|"png", theme}`，使用 `echarts`（SSR：`echarts.init(null, null, {renderer: "svg", ssr: true, width, height})`，`chart.renderToSVGString()`）生成 SVG；`png` 时用 `@resvg/resvg-js` 加载 `/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc` 转换（2 倍像素密度）；逐行输出 `{id, data_base64}` 或 `{id, error}`。
- `renderer.py`：维护 `AXPOINT_CHART_WORKERS`（默认 2）个子进程的池，单次渲染超时 15 秒（超时则重启该子进程并返回 `UPSTREAM_TIMEOUT`）；`option` 中禁止函数（JSON 无法携带），`formatter` 只允许字符串模板。
- `POST /render/chart`：`{option, width, height, format}` → `image/svg+xml` 或 `image/png`。
- `package.json` 固定 `echarts` 与 `@resvg/resvg-js` 的版本（见 A6），镜像构建时 `npm ci --omit=dev`。

---

## 12.4 出品器（`axpoint-parser/exporters/`）

所有出品器签名 `export(doc: ExportDocument, out_path: Path) -> ExportResult{file_size, sha256}`，写入 `${AXPOINT_DATA_ROOT}/exports/<tenant>/<job_id>.<ext>`（先写 `.part` 再原子重命名）。

| 格式 | 实现要点 |
| :--- | :--- |
| `XLSX` | `openpyxl` `write_only=True`；首个工作表“概览”放 KPI 与图表图片（PNG，`openpyxl.drawing.image.Image`），每个表格一个工作表（工作表名去除 `[]:*?/\` 并截断 31 字符、去重）；数值写原生数字并设置 `number_format`，日期写 `datetime`；表头加粗冻结；透视的小计 / 合计行加粗；`DECIMAL` 以 `Decimal` 写入 |
| `CSV` | 每个表格一个文件，多表时打包 ZIP；`csv_bom=true` 时写 UTF-8 BOM（便于电子表格软件识别中文）；流式写出 |
| `PDF` | Jinja2 渲染 `report.html.j2`（标题页、KPI 卡片、SVG 图表内联、表格分页重复表头），WeasyPrint 输出；字体 Noto Sans CJK；页眉标题、页脚“页码 / 总页数 + 水印文本”；`page_size` A4 / A3，`orientation` portrait / landscape；单表超过 5 000 行时截断并注明“完整数据请导出 XLSX” |
| `DOCX` | `python-docx`；有模板时打开模板，替换段落与表格中的 `{{key}}` 占位符（跨 run 合并后替换），在 `{{sections}}` 所在段落位置插入内容；无模板时按默认样式生成；图表为 PNG 图片；表格使用模板中名为 `AxTable` 的样式（不存在时用 `Table Grid`）；页脚水印 |
| `PPTX` | `python-pptx`；有模板时使用模板母版的“标题”与“标题和内容”版式；每个 KPI 组一页、每个图表一页、每个表格一页（超过 15 行分页）；`BAR_CHART` / `LINE_CHART` / `PIE_CHART` 且带 `data_table` 时生成**原生可编辑图表**（`CategoryChartData`），其余类型插入 PNG |
| `MARKDOWN` | 标题、KPI 列表、GFM 表格（超过 500 行截断并注明），图表以 SVG 文件随 ZIP 打包 |

`POST /export/{format}`（内部令牌）：请求体 `{tenant_id, job_id, document}`，返回 `{file_path, file_size, sha256, file_name}`。

**测试**（`tests/test_exporters.py`）：对每种格式生成含中文、DECIMAL、日期、2 张图表、10 000 行表格的文档，再用对应库读回：XLSX 单元格值与 `number_format`、DOCX 段落与表格文本、PPTX 原生图表的分类与数值、PDF 用 `pypdf` 抽取文本含标题与水印、CSV 行数与 BOM。模板测试：`tests/data/templates/report_template.docx` 与 `deck_template.pptx`（A5 §1）的占位符全部被替换。

---

## 12.5 出品任务、下载与分享

### 12.5.1 出品任务

1. `POST /api/xblueview/exports {export_format, source, options, template_id?}`（`EXPORT_DATA`）→ 校验来源存在且创建者可读 → 写 `xv_export_jobs` 与 `ax_jobs(kind=EXPORT, idempotency_key=export_job_id)` → 返回 `{id, status}`。
2. `EXPORT` 处理器：生成 `ExportDocument`（图表的 `echarts_option` 由 `view/export.rs` 以前端同一套 encoding 规则生成，规则表以 JSON 写在 `xbluenode-web/src/lib/chartSpec.json`，Rust 与 TypeScript 共用同一文件）→ 调 Sidecar → 回写文件信息、`expires_at = now + make_interval(days => EXPORT_FILE 的 keep_days)`、状态 → 外发 `export.finished` → 审计 `EXPORT_FILE`（`detail_json` 含格式、来源、行数、文件哈希）。
3. `GET /api/xblueview/exports/{id}/download`：只允许创建者或具备 `DATA_ADMIN` 者；`Content-Disposition: attachment; filename*=UTF-8''<百分号编码文件名>`；流式读取文件（`tokio_util::io::ReaderStream`）。
4. 保留：`RETENTION` 处理器对 `EXPORT_FILE` 目标删除过期文件并把行状态置为 `EXPIRED`。

### 12.5.2 分享

1. 创建 `POST /api/xblueview/shares {target_kind, target_id, expires_in_hours (1..=720), max_views?, password?, locked_filters, allow_filters, allow_export}`（`VIEW_SHARE`）：`DASHBOARD` 要求已发布；令牌 `axs_<prefix8>_<secret32>`（`rand::rngs::OsRng`，base62）只返回一次；口令以 argon2id 保存；审计 `SHARE_CREATE`；返回 `{id, token, url: "/share/<token>"}`。
2. 公开接口（不经过 `AuthPrincipal`，由 `share.rs` 中的 `ShareContext` 提取器处理）：
   - `GET /api/xblueview/share/{token}`：`ax_resolve_share(sha256(token))` → 不存在 / 过期 / 撤销 / 超次数一律 404 `SHARE_NOT_FOUND`；有口令且请求未携带有效 `X-Share-Session` 时返回 `{password_required: true}`；否则返回目标元数据（驾驶舱布局、组件定义、锁定筛选）。
   - `POST /api/xblueview/share/{token}/unlock {password}`：校验口令（失败计入 `tower_governor` 每 IP 每分钟 10 次限制），成功返回 `share_session`（HMAC 签名，含 `share_id` 与 2 小时有效期）。
   - `POST /api/xblueview/share/{token}/widgets/{widget_id}/data {filters, drill_path}`：组件必须属于分享的驾驶舱；`locked_filters` 强制覆盖同名筛选；`allow_filters=false` 时忽略请求中的其他筛选；以**分享创建者**身份取数（创建者被禁用或失去 `DATA_READ` 时分享立即失效，返回 404）。
   - `GET /api/xblueview/share/{token}/download`：`EXPORT_FILE` 目标或 `allow_export=true` 时可用。
   - 访问计数：每次加载元数据（非组件数据请求）在租户事务中 `UPDATE xv_share_links SET view_count = view_count + 1 WHERE id = $1 AND (max_views IS NULL OR view_count < max_views)`，影响行数为 0 时返回 404；审计 `SHARE_ACCESS`（`username = "share:<prefix>"`，`client_ip`）。
3. 撤销：`DELETE /api/xblueview/shares/{id}`，立即生效。
4. `SharePage`：只读查看器（复用 `DashboardGrid` 的查看模式），显示到期时间，口令页，无侧栏，无内部导航链接；响应头 `Referrer-Policy: no-referrer`、`X-Robots-Tag: noindex`。

---

## 12.6 路由（`/api/xblueview` 下）

| 方法与路径 | 权限 |
| :--- | :--- |
| `POST /exports`、`GET /exports?mine=true`、`GET /exports/{id}`、`GET /exports/{id}/download`、`DELETE /exports/{id}` | `EXPORT_DATA` |
| `GET /export-templates`、`POST /export-templates`（multipart）、`DELETE /export-templates/{id}` | `EXPORT_DATA` / `VIEW_EDIT` |
| `POST /shares`、`GET /shares?target_id=`、`DELETE /shares/{id}` | `VIEW_SHARE` |
| `GET /share/{token}`、`POST /share/{token}/unlock`、`POST /share/{token}/widgets/{widget_id}/data`、`GET /share/{token}/download` | 分享令牌 |

---

## 12.7 验收

`scripts/gate/step12.sh`：

```bash
#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "$0")/lib.sh"

(cd axpoint-parser && uv run pytest tests/test_exporters.py tests/test_charts.py -q)

# 服务端图表
curl -sf -H "X-AxPoint-Internal-Token: $AXPOINT_INTERNAL_TOKEN" -H "Content-Type: application/json" \
  -X POST "$PARSER_BASE_URL/render/chart" \
  -d '{"option":{"xAxis":{"type":"category","data":["甲","乙"]},"yAxis":{},"series":[{"type":"bar","data":[1,2]}]},"width":600,"height":400,"format":"png"}' \
  -o /tmp/chart.png
expect_eq "chart-png" "$(head -c 8 /tmp/chart.png | od -An -tx1 | tr -d ' \n')" "89504e470d0a1a0a"

# 六种格式出品
for F in XLSX CSV PDF DOCX PPTX MARKDOWN; do
  EID=$(api POST /api/xblueview/exports "{\"export_format\":\"$F\",\"source\":{\"kind\":\"DASHBOARD\",\"dashboard_id\":\"$DASHBOARD_ID\",\"filters\":{}},\"options\":{\"title\":\"供应概览\",\"watermark\":true}}" | jq -r .id)
  wait_export "$EID"
  curl -sf -H "Authorization: Bearer $AXPOINT_DEV_TOKEN" "$CORE_BASE_URL/api/xblueview/exports/$EID/download" -o "/tmp/export.$F"
  python3 scripts/gate/verify_export.py "$F" "/tmp/export.$F" "$(sql_owner "SELECT sum(amount) FROM demo_src.orders WHERE status='PAID'")"
done
expect_eq "audit-export" "$(sql_app default "SELECT count(*) >= 6 FROM sec_audit_logs WHERE action='EXPORT_FILE'")" "t"

# 分享：锁定筛选、次数、撤销
SH=$(api POST /api/xblueview/shares "{\"target_kind\":\"DASHBOARD\",\"target_id\":\"$DASHBOARD_ID\",\"expires_in_hours\":1,\"max_views\":2,\"locked_filters\":{},\"allow_filters\":false}")
TOKEN=$(echo "$SH" | jq -r .token); SID=$(echo "$SH" | jq -r .id)
expect_code "share-view-1" 200 GET_NOAUTH "/api/xblueview/share/$TOKEN"
expect_code "share-data" 200 POST_NOAUTH "/api/xblueview/share/$TOKEN/widgets/$KPI_WIDGET_ID/data" '{}'
expect_code "share-view-2" 200 GET_NOAUTH "/api/xblueview/share/$TOKEN"
expect_code "share-max-views" 404 GET_NOAUTH "/api/xblueview/share/$TOKEN"
SH2=$(api POST /api/xblueview/shares "{\"target_kind\":\"DASHBOARD\",\"target_id\":\"$DASHBOARD_ID\",\"expires_in_hours\":1,\"password\":\"p@ss-123456\",\"locked_filters\":{},\"allow_filters\":false}")
T2=$(echo "$SH2" | jq -r .token); S2=$(echo "$SH2" | jq -r .id)
expect_json "share-password" "$(curl -s "$CORE_BASE_URL/api/xblueview/share/$T2")" '.password_required == true'
api DELETE /api/xblueview/shares/$S2 >/dev/null
expect_code "share-revoked" 404 GET_NOAUTH "/api/xblueview/share/$T2"
expect_code "share-token-not-api" 401 GET "/api/xblueview/dashboards" "" "$TOKEN"
expect_eq "audit-share" "$(sql_app default "SELECT count(*) >= 2 FROM sec_audit_logs WHERE action='SHARE_ACCESS'")" "t"

gate_pass step12
```

- `scripts/gate/verify_export.py FORMAT PATH EXPECTED_PAID`：按格式读回文件（`openpyxl`、`csv`、`pypdf`、`python-docx`、`python-pptx`、纯文本），断言文件含标题“供应概览”、KPI 数值等于 `EXPECTED_PAID`（按两位小数与千分位两种书写匹配）、PDF / DOCX / PPTX 含水印文本 `dev`。
- `wait_export`、`GET_NOAUTH` 方法与 `expect_code` 的第 6 个参数（覆盖令牌）由 `lib.sh` 提供（A5 §2）；`share-token-not-api` 断言分享令牌不能访问普通业务接口。

**通过标准**：最后一行 `GATE step12 PASS`。
