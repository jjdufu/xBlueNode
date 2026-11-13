# 06 · xBlueView 孪生可视化、制表驾驶舱与全格式出品引擎（Step 10 ~ Step 11）

本文件定义 **xBlueView（业务孪生可视化驾驶舱）** 与 **全格式出品中心（Export & Publish Engine）** 的完整技术方案，全面落地“分析、制表、查询、孪生看板、多格式报告/应用出品”的最后一公里体验。

---

## Step 10：xBlueView 低代码孪生驾驶舱与交互式透视制表

### 10.1 核心能力组成
1. **业务节点孪生视图（Node Twin View）**：
   - 以 `BlueNodeType` 为模板，点击任意业务节点（如某座晶圆厂）即打开其专属孪生详情页：实时指标卡 + 历史趋势折线图（来自 `xb_metric_series`）+ 上下游多跳拓扑（支持点击下钻）+ 关联文档证据与血缘溯源（`xb_provenance`）。
2. **交互式制表与图表工作台（AxAnalytics Studio）**：
   - 前端集成 **TanStack Table**（支持列排序、多列固定、搜索过滤、透视交叉表、小计/总计行、条件格式高亮）与 **ECharts**（折线图、柱状图、堆叠图、饼图/环形图、散点图、桑基图、雷达图、关系拓扑图）。
   - 支持直接从 `AxQuery`（智能问数结果）或 `AxHub`（数据集透视分析结果）一键“固定到 xBlueView 看板”。
3. **可拖拽拼装驾驶舱（Low-Code Dashboard）**：
   - 基于 `react-grid-layout` 的 12 列响应式网格画布。
   - 支持全局联动筛选器（时间范围、业务节点类型、地区/部门维度），动态刷新画布内关联组件。

### 10.2 数据库 Schema（`deploy/migrations/005b_xblueview_dashboards.sql`）

```sql
-- 1. xBlueView 驾驶舱/孪生页面定义表
CREATE TABLE IF NOT EXISTS xv_dashboards (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id       TEXT NOT NULL DEFAULT 'default',
    name            TEXT NOT NULL,
    description     TEXT NOT NULL DEFAULT '',
    layout_mode     TEXT NOT NULL DEFAULT 'GRID', -- 'GRID' | 'NODE_TWIN'
    bound_type_name TEXT,                         -- NODE_TWIN 模式下绑定的 BlueNodeType
    global_filters  JSONB NOT NULL DEFAULT '[]'::jsonb,
    share_token     TEXT UNIQUE,                  -- 若生成独立分享页，存储随机安全令牌
    is_public_share BOOLEAN NOT NULL DEFAULT FALSE,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- 2. 驾驶舱内组件（Widget）定义表
CREATE TABLE IF NOT EXISTS xv_widgets (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    dashboard_id  UUID NOT NULL REFERENCES xv_dashboards(id) ON DELETE CASCADE,
    title         TEXT NOT NULL,
    widget_kind   TEXT NOT NULL, -- 'KPI_CARD' | 'TABLE' | 'PIVOT_TABLE' | 'LINE_CHART' | 'BAR_CHART' | 'PIE_CHART' | 'SANKEY' | 'GRAPH_TOPOLOGY' | 'SIM_COMPARE' | 'MARKDOWN_NOTE'
    data_source   JSONB NOT NULL, -- {source_type: 'ANALYTICS_PIVOT'|'ANALYTICS_SQL'|'XBLUE_NODES'|'METRIC_SERIES'|'SIM_COMPARE', params: {...}}
    viz_config    JSONB NOT NULL DEFAULT '{}'::jsonb, -- ECharts 映射配置/列格式/阈值告警色
    grid_pos      JSONB NOT NULL DEFAULT '{"x":0,"y":0,"w":6,"h":4}'::jsonb,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- 3. 全格式出品作业表
CREATE TABLE IF NOT EXISTS xv_export_jobs (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id     TEXT NOT NULL DEFAULT 'default',
    title         TEXT NOT NULL,
    export_format TEXT NOT NULL, -- 'XLSX' | 'CSV' | 'PDF' | 'DOCX' | 'PPTX' | 'MARKDOWN'
    source_kind   TEXT NOT NULL, -- 'TABULAR_RESULT' | 'DASHBOARD' | 'CHAT_ANSWER' | 'SIM_COMPARE' | 'NODE_TWIN'
    payload_json  JSONB NOT NULL,
    file_path     TEXT,
    file_size     BIGINT NOT NULL DEFAULT 0,
    status        TEXT NOT NULL DEFAULT 'PENDING', -- 'PENDING' | 'RUNNING' | 'SUCCEEDED' | 'FAILED'
    error_message TEXT,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    finished_at   TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_xv_widgets_dash ON xv_widgets (dashboard_id);
CREATE INDEX IF NOT EXISTS idx_xv_export_tenant ON xv_export_jobs (tenant_id, created_at DESC);
```

### 10.3 前端页面与组件架构（`xbluenode-web/src/`）

```text
xbluenode-web/src/
├── components/
│   ├── xbluegraph/
│   │   └── ForceTopologyCanvas.tsx      # 力导向 / 分层 DAG 图谱浏览器（替换静态圆环 SVG）
│   ├── analytics/
│   │   ├── PivotBuilder.tsx             # 维度/指标/过滤器拖拽配置面板
│   │   ├── DataTableView.tsx            # TanStack Table 交互式表格（排序/筛选/固定列/导出）
│   │   └── EChartRenderer.tsx           # 统一 ECharts 渲染器（自动响应黑白灰+科技蓝主题与 Resize）
│   └── xblueview/
│       ├── DashboardGrid.tsx            # react-grid-layout 驾驶舱装配画布
│       └── NodeTwinDrawer.tsx           # 业务节点孪生实时视图与血缘抽屉
└── pages/
    ├── AxHubPage.tsx                    # 连接器管理、文件上传双轨入库、数据集画像预览
    ├── AnalyticsStudioPage.tsx          # 自助分析、SQL 编辑器、透视制表与一键加看板
    ├── XBlueStudioPage.tsx              # React Flow 模型设计画布 + 数据映射 + AI 发现
    ├── AxQueryPage.tsx                  # 混合问答（RAG 溯源 + 智能问数表格/图表 + 导出）
    ├── AxFlowPage.tsx                   # React Flow DAG 编排画布 + 运行监控 + 审批台 + MCP 发布
    ├── AxSimPage.tsx                    # 沙盘克隆 + 多方案并行对比 + 一键应用生产
    └── XBlueViewPage.tsx                # 孪生驾驶舱管理、独立分享与全格式出品中心
```

---

## Step 11：全格式“出品（Export & Publish）”引擎

### 11.1 Python 出品渲染引擎（`axpoint-parser/exporters/`）
在 `axpoint-parser` 扩展 `/export` 服务模块（统一由 `axpoint-server` 鉴权并记录 `xv_export_jobs`，支持二进制流下载）：

```text
axpoint-parser/exporters/
├── __init__.py
├── excel_exporter.py    # openpyxl + xlsxwriter：多 Sheet（明细表 + 透视汇总表 + 指标元数据）、冻结首行、数字格式、条件色阶
├── csv_exporter.py      # UTF-8 with BOM（兼容中文 Excel 直接双击不乱码）流式导出
├── word_exporter.py     # python-docx：封面、目录、AI 分析综述、嵌表、嵌入高清图表
├── pdf_exporter.py      # weasyprint（HTML/CSS Print 模板 -> 矢量排版 PDF）：极简瑞士风格报告排版
├── ppt_exporter.py      # python-pptx：按模块自动分页生成（标题页 -> 核心 KPI 页 -> 图表分析页 -> 推演对比页 -> 行动建议页）
└── markdown_exporter.py # 完整结构化 Markdown + GFM 表格 + Mermaid 拓扑代码导出
```

### 11.2 五类“出品”场景直达链路
1. **制表出品**：在 `AnalyticsStudioPage` 或 `AxQueryPage` 表格工具栏点击“导出 Excel / CSV”，直接生成带样式的多 Sheet `.xlsx`（Sheet 1 为明细/透视结果，Sheet 2 为执行的 SQL、数据源血缘与时间戳）。
2. **分析/问答报告出品**：在 `AxQueryPage` 或 `XBlueViewPage` 点击“生成汇报文档（PDF / Word / PPTX）”，服务端组合：文字结论 + 图表高清渲染图 + 数据表格 + 溯源引用，一键交付可直接汇报的 `.pptx` / `.docx` / `.pdf`。
3. **推演决策简报出品**：在 `AxSimPage` 多策略对比后导出《What-If 策略推演对比报告》（含各方案 KPI Diff 表与推荐动作）。
4. **独立应用页面出品**：通过 `POST /api/xblueview/dashboards/{id}/share` 生成带 `share_token` 的独立只读/交互 URL（`/share/{share_token}`）与 `<iframe src="...">` 嵌入代码，外部业务系统可直接内嵌展示。
5. **数据服务出品**：通过 Step 08 的 `AxFlow` 一键发布为带 OpenAPI 文档的 REST API 与 MCP Tool。

### 11.3 核心 API 契约（`/api/xblueview/*`）

| 方法与路径 | 说明 |
| :--- | :--- |
| `GET / POST /api/xblueview/dashboards` | 列出或创建 xBlueView 驾驶舱 |
| `GET / PUT /api/xblueview/dashboards/{id}` | 获取或更新驾驶舱布局与组件列表 |
| `POST /api/xblueview/dashboards/{id}/share` | 开启/重置独立分享链接与 Iframe 嵌入令牌 |
| `GET /api/xblueview/share/{share_token}` | 免登录（受脱敏策略与 Token 校验约束）加载已分享看板数据 |
| `POST /api/xblueview/exports` | 提交出品生成任务（`XLSX`/`CSV`/`PDF`/`DOCX`/`PPTX`/`MARKDOWN`） |
| `GET /api/xblueview/exports/{job_id}/download` | 下载已生成的出品文件（设置标准 `Content-Disposition`） |

### 11.4 Step 10 ~ Step 11 可验证门禁（Verification Gate）

```bash
# 1. 数据库迁移与前端完整编译
psql "$DATABASE_URL" -f deploy/migrations/005b_xblueview_dashboards.sql
cd xbluenode-web && npm run build

# 2. Python 出品渲染引擎单元测试（校验生成的文件头魔数与结构可读性）
cd ../axpoint-parser
pytest -v tests/test_exporters.py

# 3. 端到端出品下载验证
curl -s -X POST http://127.0.0.1:9995/api/xblueview/exports \
  -H "Content-Type: application/json" \
  -d '{"tenant_id":"default","title":"供应链分析月报","export_format":"PPTX","source_kind":"SIM_COMPARE","payload_json":{"kpis":[{"name":"产能","before":1000,"after":850}]}}' | jq '{job_id: .id, status}'
```
**通过标准**：
1. `npm run build` 零 TypeScript 错误、零 Vite 构建报错。
2. `test_exporters.py` 生成的 `.xlsx` 可被 `openpyxl` 正常重读且含完整数字与表头；`.docx` 可被 `python-docx` 读出段落与表格；`.pptx` 幻灯片页数 >= 2 且含图表/表格；`.pdf` 文件以 `%PDF-` 开头且体积非空；`.csv` 含 UTF-8 BOM。
3. 通过 `/share/{share_token}` 访问公开分享驾驶舱可正常渲染所有脱敏后的指标与图表，且未授权访问管理接口返回 `401/403`。
