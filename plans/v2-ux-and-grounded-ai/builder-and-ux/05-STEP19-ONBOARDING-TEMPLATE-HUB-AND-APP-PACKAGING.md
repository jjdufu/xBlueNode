# 05 · Step 19：5 步构建引导、演练子图模板试用与 `xBlueStudio` 双态应用包装

本步骤打通从冷启动登录到业务价值产出的最短闭环：① 在系统概览页提供基于租户实时资产状态的 **5 步构建引导条**；② 将行业模板只读浏览与拓扑预览开放至 `DATA_READ` 权限，并支持具备 `STUDIO_EDIT` 的建模角色免 `OPS_ADMIN` 权限**克隆独立演练子图（`trial_<pack>_<shortid>`）**，在运行态隔离沙盒中试灌样例节点与关系且零污染企业主图；③ 在 `xBlueStudio` 内部提供**领域智能体（含暴露 MCP）+ 孪生驾驶舱**双态包装出口，并在画布节点卡片回显下游反向消费计数。

关联总览与工程约束：
- [00-V2-MASTER-INDEX.md](../00-V2-MASTER-INDEX.md)
- [03-STEP17-AXBUILDER-MCP-AND-DEV-SUBGRAPH.md](./03-STEP17-AXBUILDER-MCP-AND-DEV-SUBGRAPH.md)
- [04-STEP18-STUDIO-COPILOT-LINEAGE-AND-SPEC-DOC.md](./04-STEP18-STUDIO-COPILOT-LINEAGE-AND-SPEC-DOC.md)
- [IRON_RULES.md](../../../IRON_RULES.md)
- [A1-CONVENTIONS-ENUMS-ROUTES.md](../../../docs/standards/A1-CONVENTIONS-ENUMS-ROUTES.md)
- [10-STEP11-12-XBLUEVIEW-TWIN-DASHBOARD-EXPORT.md](../../completed/v1-foundation-step00-14/10-STEP11-12-XBLUEVIEW-TWIN-DASHBOARD-EXPORT.md)
- [12-STEP14-OPERATIONS-TEMPLATES-ACCEPTANCE.md](../../completed/v1-foundation-step00-14/12-STEP14-OPERATIONS-TEMPLATES-ACCEPTANCE.md)

---

## 19.0 目标、前置与契约

- **目标**：
  1. 实现 `GET /api/axops/onboarding-status`（`DATA_READ`），按租户实时统计 `1. 数据接入` → `2. 节点建模` → `3. 映射与编排` → `4. 推演验证` → `5. 驾驶舱与智能体` 五阶段完成度与下一步跳转路由，供 `DashboardPage.tsx` 顶部 `OnboardingStrip.tsx` 展示。
  2. 将只读模板目录 `GET /api/axops/templates` 与详情预览 `GET /api/axops/templates/{id}/preview` 权限设为 `DATA_READ`（生产级全量安装 `POST /api/axops/templates/{id}/install` 维持 `OPS_ADMIN` 不变）；新增 `POST /api/xblue/subgraphs/from-template`（仅需 `STUDIO_EDIT`），自动生成 `trial_<pack>_<shortid>` 开发沙盒子图（`is_dev_sandbox = true`），解析内置 CSV 样例在子图隔离命名空间内完成节点、关系与派生/聚合指标物化，并写入 `TRIAL_TEMPLATE_CLONE` 审计记录。
  3. 实现 `POST /api/xblue/subgraphs/{slug}/package-agent` 与 `GET /api/xblue/subgraphs/{slug}/consumption`：在 `xBlueStudio` 内联打包绑定该子图 `OBJECT_TYPE` 与 `METRIC` 端点的领域 `AxAgent`（支持 `expose_mcp = true`），并支持携带子图节点与指标上下文唤起 `xBlueView` 的 `AiDashboardWizard` 生成孪生驾驶舱草案，在节点卡片实时显示反向消费徽标。
- **前置**：Step 17、Step 18 通过（所属并行层 **L14**，无新增数据库迁移，直接复用 `0014_ops_templates_delivery.sql` 与 `0017_axbuilder_scopes.sql`；可与 Step 20 并发实施）。
- **覆盖能力项**：`C71`、`C72`（映射至 [A7-REQUIREMENTS-TRACEABILITY.md](../../../docs/standards/A7-REQUIREMENTS-TRACEABILITY.md)）。
- **契约清单**：
  - 数据库迁移：无新文件（复用 `0014` 模板元数据结构与 `0017` 的 `xb_subgraphs.is_dev_sandbox`、`xb_subgraphs.source_template_id`、`xb_nodes.subgraph_slug`、`AuditAction::TrialTemplateClone`）
  - `axpoint-domain::platform`：`OnboardingStepKey`、`OnboardingStepItem`、`OnboardingStatusResponse`、`TemplatePreviewResponse`、`TemplateTopologySummary`
  - `axpoint-domain::studio`：`TrialCloneTemplateRequest`、`TrialCloneTemplateResponse`、`PackageSubgraphAgentRequest`、`PackageSubgraphAgentResponse`、`SubgraphConsumptionResponse`、`NodeConsumptionCounts`
  - `docs/API_CONTRACT.md`：`GET /api/axops/onboarding-status`、`GET /api/axops/templates/{id}/preview`、`POST /api/xblue/subgraphs/from-template`、`POST /api/xblue/subgraphs/{slug}/package-agent`、`GET /api/xblue/subgraphs/{slug}/consumption`
  - `xbluenode-web/src/lib/api/{axops.ts, xblue.ts, xblueview.ts}`

### 文件变更清单

| 动作 | 路径 | 内容 |
| :--- | :--- | :--- |
| 修改 | `axpoint-core/crates/axpoint-domain/src/{platform.rs, studio/doc.rs, ports.rs}` | 5 步引导 DTO、模板预览 DTO、演练子图克隆契约与双态包装契约（§19.1～§19.3） |
| 修改 | `axpoint-core/crates/axpoint-db/src/{ops_repo.rs, studio_repo.rs, agent_repo.rs}` | 实时引导进度聚合 SQL、内存 CSV 样例沙盒物化事务、子图反向消费统计查询 |
| 修改 | `axpoint-core/crates/axpoint-server/src/{templates.rs, routes/axops.rs, routes/xblue.rs}` | 开放模板只读预览至 `DATA_READ`；挂载引导进度、演练克隆、智能体打包与消费计数路由 |
| 修改 | `xbluenode-web/src/lib/api/{axops.ts, xblue.ts}`、`src/components/axops/logic.ts` | 对齐前端客户端类型；允许具备 `DATA_READ` 的主体查看 `AxOps` 模板页签 |
| 新建 | `xbluenode-web/src/components/dashboard/{OnboardingStrip.tsx, TemplateHubModal.tsx}` | 系统概览 5 步实时构建进度条与内置行业模板预览/演练克隆弹窗（§19.1～§19.2） |
| 新建 | `xbluenode-web/src/components/xbluestudio/{StudioAppDrawer.tsx, StudioAgentPackModal.tsx}` | `xBlueStudio` 双态应用包装抽屉与领域智能体打包弹窗（§19.3） |
| 修改 | `xbluenode-web/src/pages/{DashboardPage.tsx, XBlueStudioPage.tsx, XBlueViewPage.tsx}`、`src/components/xbluestudio/NodeTypeCard.tsx` | 挂载引导条、包装抽屉、节点消费角标，以及 `XBlueViewPage` 接收 Studio 预填向导参数 |
| 新建 | `scripts/gate/step19.sh` | Step 19 自动化门禁（§19.4） |

---

## 19.1 5 步实时构建引导引擎（`GET /api/axops/onboarding-status`）

### 19.1.1 阶段完成度实时判定口径

接口要求权限 `DATA_READ`，在单次只读租户事务（`begin_tenant_tx`）内聚合五阶段指标，不设立易与真实资产脱节的手工勾选状态表：

| 序号 | `step_key` | 阶段标签（中 / 英） | 判定完成条件（`completed == true`） | 附带实时指标（`metric_label`） | 导航路由（`target_path`） |
| :--- | :--- | :--- | :--- | :--- | :--- |
| `1` | `DATA_INGEST` | `1. 数据接入` / `1. Data Ingest` | `ds_datasets` 记录数 `> 0` 或 `hg_docs` 文档数 `> 0` | `{dataset_count} 数据集 · {doc_count} 文档` | `/hub` |
| `2` | `NODE_MODELING` | `2. 节点建模` / `2. Node Modeling` | `xb_node_types` 记录数 `> 0` 且任一子图 `xb_schema_versions` 版本数 `>= 1` | `{node_type_count} 节点类型 · {edge_type_count} 关系` | `/studio` |
| `3` | `MAPPING_AND_FLOW` | `3. 映射与编排` / `3. Mapping & Flow` | `xb_nodes` 已物化业务节点数 `> 0` | `{node_count} 业务节点 · {flow_count} 流程` | `/flow` |
| `4` | `SIM_VERIFICATION` | `4. 推演验证` / `4. Simulation` | `ax_sim_scenarios` 中 `status = 'SUCCEEDED'` 推演方案数 `> 0`（或 `ax_sandboxes` 数 `> 0`） | `{scenario_count} 推演方案` | `/sim` |
| `5` | `COCKPIT_AND_AGENT` | `5. 驾驶舱与智能体` / `5. Apps & Agents` | `xv_dashboards` 记录数 `> 0` 或 `ax_agents` 记录数 `> 0` | `{dashboard_count} 驾驶舱 · {agent_count} 智能体` | `/view` |

响应 JSON 契约：

```json
{
  "completed_steps": 3,
  "total_steps": 5,
  "progress_pct": 60,
  "recommended_next_step": "SIM_VERIFICATION",
  "has_trial_subgraph": true,
  "steps": [
    {
      "step_no": 1,
      "step_key": "DATA_INGEST",
      "label_zh": "1. 数据接入",
      "label_en": "1. Data Ingest",
      "completed": true,
      "count_primary": 4,
      "count_secondary": 2,
      "target_path": "/hub"
    }
  ]
}
```

### 19.1.2 `OnboardingStrip.tsx`（挂载于 `DashboardPage.tsx` 页头下方）

- **布局结构**：单行紧凑卡片，左侧显示完成比例徽标（如 `3 / 5 已就绪`），中部等分渲染 5 步胶囊节点（已完成态显示蓝色实心对勾与实时计数，首个未完成态高亮外框，支持直接跳转对应模块路由），右侧放置两个操作控件：
  - 主按钮：`继续构建` / `Continue`（路由至 `recommended_next_step` 对应的 `target_path`；当 `5 / 5` 全部完成时切换为收起态紧凑徽标）。
  - 次按钮：`试用模板` / `Try Template`（唤起 `TemplateHubModal.tsx`，零配置体验完整行业子图）。

---

## 19.2 行业模板中心前置预览与免运维演练克隆

### 19.2.1 权限分层模型

| 接口路径 | 所需权限 | 作用域与安全边界 |
| :--- | :--- | :--- |
| `GET /api/axops/templates` | `DATA_READ` | 只读枚举 `templates/packs/*` 元数据清单，所有登录角色可见 |
| `GET /api/axops/templates/{id}/preview` | `DATA_READ` | 只读返回模板 `manifest`、`README.md`、实体关系拓扑摘要、样例指标口径与看板组件预览清单 |
| `POST /api/xblue/subgraphs/from-template` | `STUDIO_EDIT` | **非破坏性演练克隆**：仅创建 `trial_<pack>_<shortid>` 隔离开发子图（`is_dev_sandbox = true`），不写入企业 `main` 主图、不创建全局告警或定时任务 |
| `POST /api/axops/templates/{id}/install` | `OPS_ADMIN` | **生产级全量安装**（保持 Step 14 严格权限不变）：写入正式数据集、主图模型、流程与驾驶舱 |

### 19.2.2 `GET /api/axops/templates/{id}/preview` 响应结构

```json
{
  "manifest": {
    "id": "supply_chain",
    "name": "供应链协同",
    "version": "1.0.0",
    "industry": "制造 / 流通",
    "description": "供应商—工厂—仓库—订单四层网络，含库存监控、断供推演与履约驾驶舱"
  },
  "readme_markdown": "# 供应链协同模板包...",
  "topology_summary": {
    "node_types": [
      { "name": "Supplier", "display_name": "供应商", "property_count": 5, "metric_count": 2 },
      { "name": "Factory", "display_name": "工厂", "property_count": 4, "metric_count": 2 },
      { "name": "Warehouse", "display_name": "仓库", "property_count": 5, "metric_count": 3 },
      { "name": "Order", "display_name": "订单", "property_count": 4, "metric_count": 2 }
    ],
    "edge_types": [
      { "name": "SUPPLIES", "src_type_name": "Supplier", "dst_type_name": "Factory", "cardinality": "MANY_TO_MANY" }
    ],
    "agg_rule_count": 3,
    "derived_rule_count": 2
  },
  "sample_metrics": [
    { "name": "paid_amount", "display_name": "已支付订单金额", "unit": "元" }
  ],
  "dashboard_previews": [
    { "slug": "supply-overview", "name": "供应链总览驾驶舱", "widget_count": 6 }
  ]
}
```

### 19.2.3 演练子图克隆事务（`POST /api/xblue/subgraphs/from-template`）

- **入参**：`{ pack_id: string, slug_override?: string, display_name?: string }`。
- **确定性执行步骤**：
  1. 校验 `pack_id` 存在于 `templates/packs/{pack_id}` 且各文件 SHA-256 与 `manifest.json` 一致（复用 Step 14 完整性核验器）。
  2. 生成子图 `slug`：若未传 `slug_override`，取 `format!("trial_{}_{}", sanitize_slug(&pack_id), short_hex_4(Uuid::new_v4()))`（截断至 `<= 63` 字符）；若传入 `slug_override`，强制校验 `slug != "main"` 且匹配 `^trial_[a-z0-9_-]{1,56}$`，防止覆盖主图或正式部门子图。
  3. 在单个租户事务内：
     - 插入 `xb_subgraphs`（`slug = trial_slug`, `is_main = false`, `is_dev_sandbox = true`, `source_template_id = Some(pack_id)`, `owner_dept = principal.dept_id`）。
     - 解析模板包 `schema.json`，写入该演练子图的 `xb_node_types`、`xb_edge_types`、`xb_schema_heads (current_version = 1)`、`xb_schema_versions (version = 1)`，同时创建一条 `status = 'DRAFT'`、`source = 'TEMPLATE'` 的初始工作草案供用户在 `xBlueStudio` 自由实验。
     - 读取模板包 `sample_data/*.csv` 与 `mappings.json`，在内存中按列类型与外键规则解析样例行，直接批量写入 `xb_nodes (subgraph_slug = trial_slug)`、`xb_links (subgraph_slug = trial_slug)` 与 `xb_metric_series`，并立即对该子图节点执行一次确定性派生公式求值与跨边动态聚合计算，使演练子图开箱即具备完整图拓扑与指标数值。
  4. 通过 `AuditWriter` 写入 `AuditAction::TrialTemplateClone`（`resource_ref = trial_slug`，`detail_json = { pack_id, nodes_materialized, links_materialized }`）。
  5. 返回 `201 Created`：
     ```json
     {
       "subgraph_slug": "trial_supply_chain_8a2f",
       "display_name": "供应链协同（演练沙盒）",
       "is_dev_sandbox": true,
       "source_template_id": "supply_chain",
       "draft_id": "9f1d...",
       "nodes_materialized": 24,
       "links_materialized": 31,
       "studio_url": "/studio?subgraph=trial_supply_chain_8a2f"
     }
     ```

---

## 19.3 `xBlueStudio` 内联双态应用包装与反向消费计数

### 19.3.1 领域智能体一键打包（`POST /api/xblue/subgraphs/{slug}/package-agent`）

- **所需权限**：`STUDIO_EDIT` + `AGENT_EDIT`；当请求 `environment == "PROD"` 时额外执行 `principal.enforce_no_prod_mutation()`（拦截受限 `BUILDER` 令牌直推生产）。
- **请求体 `PackageSubgraphAgentRequest`**：
  ```json
  {
    "agent_name": "supply_domain_assistant",
    "description": "围绕供应链子图实体与认证指标的受控领域助手",
    "node_types": ["Supplier", "Warehouse"],
    "metric_names": ["paid_amount"],
    "environment": "PROD",
    "expose_mcp": true,
    "publish_agent": true
  }
  ```
- **打包事务逻辑**：
  1. 读取目标子图最新已发布 `SchemaDoc`（若子图尚未发布任何版本返回 `400 SUBGRAPH_NOT_PUBLISHED`；若 `environment == "PROD"` 且子图为 `is_dev_sandbox == true` 返回 `400 SANDBOX_PROD_PACK_FORBIDDEN`，演练子图仅允许打包 `environment == "TEST"` 预览端点，合并入 `main` 后可发布 `PROD`）。
  2. **自动挂载/更新领域对象端点**：对 `node_types` 中每个已发布 `NodeTypeSpec`，采用固定规则生成 `slug`（主图取 `<kebab(node_type)>-objects`，非主图测试态取 `<subgraph>-<kebab(node_type)>-objects`）幂等 `UPSERT` 至 `ax_published_endpoints`（`kind = 'OBJECT_TYPE'`, `output_shape = 'OBJECT'`, `expose_mcp = req.expose_mcp`, `target_snapshot = serde_json::to_value(node_spec)`）。
  3. **自动挂载/更新认证指标端点**：对 `metric_names` 中每个存在于 `an_metrics` 的指标，幂等 `UPSERT` 至 `ax_published_endpoints`（`kind = 'METRIC'`, `output_shape = 'ROWS'`, `expose_mcp = req.expose_mcp`）。
  4. **合成确定性领域智能体 `AgentSpec`**：生成包含子图实体列表、主键模板、合法连接边与「严禁心算数值，一切指标与对象查询必须调用绑定端点工具」约束的 `system_prompt`，将上述端点工具引用与内置 `traverse_neighbors` 写入 `ax_agents.spec`；当 `publish_agent == true` 时递增 `ax_agents.version` 并写入 `ax_agent_versions`。
  5. 返回 `{ agent_id, agent_name, agent_version, endpoints_upserted: [{ id, slug, kind, expose_mcp }], mcp_exposed_count, workbench_url: "/agents?tab=workbench&agent=<id>" }`。

### 19.3.2 下游反向消费统计（`GET /api/xblue/subgraphs/{slug}/consumption`）

要求权限 `DATA_READ`，汇总引用当前子图各 `NodeTypeSpec` 的四类下游资产计数与快捷跳转元数据：
- **已发布端点（`endpoint_count`）**：`ax_published_endpoints` 中 `kind = 'OBJECT_TYPE'` 且 `target_id = node_type.name` 的启用端点数。
- **领域智能体（`agent_count`）**：`ax_agents.spec.tools` 中引用上述领域端点的智能体数。
- **孪生驾驶舱（`dashboard_count`）**：`xv_widgets` 中 `deps.node_types` 或 `data_source` 绑定 `node_type.name` 的去重驾驶舱数。
- **运营告警规则（`rule_count`）**：`ax_rules` 中 `scope_kind = 'METRIC'` 且 `target->>'type_name' = node_type.name` 的启用规则数。

### 19.3.3 前端双态包装交互（`StudioAppDrawer.tsx`、`StudioAgentPackModal.tsx` 与 `XBlueViewPage.tsx` 联动）

| 组件 / 页面 | 位置与触发入口 | 交互与状态规格 | 控件微文案规范（中 / 英 `tx()`） |
| :--- | :--- | :--- | :--- |
| `XBlueStudioPage.tsx` 工具栏 | 顶部右侧操作区新增入口按钮 | 唤起 `StudioAppDrawer.tsx` 并拉取 `GET /api/xblue/subgraphs/{slug}/consumption` | 按钮：`应用包装` / `Package App` |
| `StudioAppDrawer.tsx` | 右侧滑出抽屉 | 上半部分并列两张包装出口卡片：① **领域智能体卡片**（展示当前子图已关联智能体数，主按钮打开 `StudioAgentPackModal.tsx`）；② **孪生驾驶舱卡片**（勾选要纳入监控的节点类型后，携带查询参数 `/view?wizard=ai&subgraph={slug}&node_types=Supplier,Warehouse` 导航至 `XBlueViewPage.tsx` 自动展开预填好实体与指标上下文的 `AiDashboardWizard`）；下半部分列出当前子图已关联的端点、智能体与驾驶舱清单 | 卡片按钮：`打包智能体` / `Pack Agent`、`生成驾驶舱` / `Build Cockpit`；列表操作：`打开对话` / `Open Chat`、`查看看板` / `Open View` |
| `StudioAgentPackModal.tsx` | 从 `StudioAppDrawer` 打开的模态框 | 表单项：`智能体名称`（占位符 `supply_domain_assistant`）、`节点类型`（多选复选框，默认全选当前子图已发布节点）、`认证指标`（多选复选框）、`环境`（主图可选 `PROD` / `TEST`，演练沙盒子图固定 `TEST`）、`暴露 MCP`（`Switch` 开关） | 弹窗标题：`打包领域智能体` / `Package Domain Agent`；按钮：`取消` / `Cancel`、`生成智能体` / `Create Agent` |
| `NodeTypeCard.tsx` 消费角标 | 节点卡片底栏 | 当 `endpoint_count + agent_count + dashboard_count > 0` 时渲染浅灰底胶囊徽标（如 `1 智能体 · 2 看板`），选择该徽标直接打开 `StudioAppDrawer.tsx` 并按该节点类型过滤下游资产 | 徽标：`{n} 智能体` / `{n} Agent`、`{m} 看板` / `{m} View` |

---

## 19.4 门禁 `scripts/gate/step19.sh`

```bash
#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "$0")/lib.sh"

# 1. 单元测试：覆盖 5 步完成度状态推导、模板演练子图命名校验与智能体工具集幂等打包
(cd axpoint-core && cargo test -p axpoint-domain && cargo test -p axpoint-server templates)

# 2. 验证 5 步构建引导状态接口（返回固定 5 步、进度百分比与下一步路由）
ONBOARD_RES=$(api GET /api/axops/onboarding-status)
expect_json "onboarding-progress" "$ONBOARD_RES" \
  '.total_steps == 5 and (.steps | length) == 5 and .completed_steps >= 1 and (.progress_pct >= 20)'

# 3. 验证非 OPS_ADMIN 建模用户可只读预览模板并克隆隔离演练子图（零污染企业主图 main）
ANALYST_TOK_RES=$(api POST /api/auth/tokens '{
  "name": "gate-step19-studio-analyst",
  "expires_in_days": 3,
  "token_profile": "GENERAL",
  "scopes": ["DATA_READ", "STUDIO_EDIT"]
}')
ANALYST_PAT=$(echo "$ANALYST_TOK_RES" | jq -r .token)

# 无 OPS_ADMIN 仍可访问模板列表与拓扑预览，但直接调用生产安装接口必须返回 403
expect_code "tpl-list-data-read" 200 GET "/api/axops/templates" "" "$ANALYST_PAT"
PREVIEW_RES=$(curl -sS -H "Authorization: Bearer $ANALYST_PAT" -H "Accept: application/json" \
  "$CORE_BASE_URL/api/axops/templates/supply_chain/preview")
expect_json "tpl-preview-ok" "$PREVIEW_RES" \
  '.manifest.id == "supply_chain" and (.topology_summary.node_types | length) >= 3'
expect_code "tpl-prod-install-guarded" 403 POST "/api/axops/templates/supply_chain/install" '{"dry_run":true}' "$ANALYST_PAT"

MAIN_NODES_BEFORE=$(sql_app default "SELECT count(*) FROM xb_nodes WHERE subgraph_slug = 'main'")
CLONE_RES=$(curl -sS -X POST -H "Authorization: Bearer $ANALYST_PAT" \
  -H "Accept: application/json" -H "Content-Type: application/json" \
  "$CORE_BASE_URL/api/xblue/subgraphs/from-template" \
  -d '{"pack_id":"supply_chain"}')
TRIAL_SLUG=$(echo "$CLONE_RES" | jq -r .subgraph_slug)
MAIN_NODES_AFTER=$(sql_app default "SELECT count(*) FROM xb_nodes WHERE subgraph_slug = 'main'")
TRIAL_AUDIT=$(sql_app default "SELECT count(*) > 0 FROM sec_audit_logs WHERE action = 'TRIAL_TEMPLATE_CLONE' AND resource_ref = '$TRIAL_SLUG'")

expect_json "trial-template-clone" "$CLONE_RES" \
  "(.subgraph_slug | startswith(\"trial_supply_chain_\")) and .is_dev_sandbox == true and .nodes_materialized > 0 and .links_materialized > 0 and ($MAIN_NODES_BEFORE == $MAIN_NODES_AFTER) and (\"$TRIAL_AUDIT\" == \"t\")"

# 4. 验证 Studio 一键打包领域智能体（自动暴露 OBJECT_TYPE / METRIC 发布端点并注册至 MCP）
PACK_AGENT_RES=$(api POST /api/xblue/subgraphs/main/package-agent '{
  "agent_name": "gate_step19_supply_agent",
  "description": "门禁验证主图打包智能体",
  "node_types": ["Warehouse"],
  "metric_names": ["paid_amount"],
  "environment": "PROD",
  "expose_mcp": true,
  "publish_agent": true
}')
expect_json "studio-pack-agent" "$PACK_AGENT_RES" \
  '(.agent_id | length) > 10 and .agent_version >= 1 and (.endpoints_upserted | length) >= 1 and .mcp_exposed_count >= 1'

# 5. 验证 Studio 上下文驱动生成孪生驾驶舱草稿，并在子图反向消费统计中双向回显
DASH_RES=$(api POST /api/xblueview/dashboards/ai-draft '{
  "name": "门禁仓库履约孪生看板",
  "slug": "gate-step19-warehouse-cockpit",
  "prompt": "围绕 Warehouse 在库量与履约告警生成监控驾驶舱",
  "subgraph_slug": "main",
  "seed_node_types": ["Warehouse"]
}')
DASH_ID=$(echo "$DASH_RES" | jq -r '.dashboard.id // .id')

CONSUMPTION_RES=$(api GET /api/xblue/subgraphs/main/consumption)
expect_json "studio-pack-dashboard" "$CONSUMPTION_RES" \
  "(\"$DASH_ID\" | length) > 10 and .by_node_type.Warehouse.endpoint_count >= 1 and .by_node_type.Warehouse.agent_count >= 1 and .by_node_type.Warehouse.dashboard_count >= 1"

gate_pass step19
```

**通过标准**：最后一行输出 `GATE step19 PASS`；非运维分析师角色可独立完成行业模板预览与隔离演练子图试灌，主图一键双态包装后在 `consumption` 接口与 MCP 工具列表中实时可见。
