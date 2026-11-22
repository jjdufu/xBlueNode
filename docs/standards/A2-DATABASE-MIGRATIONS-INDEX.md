# A2 · 数据库迁移索引

迁移文件位于 `deploy/migrations/`，命名 `NNNN_<module>.sql`，由 `sqlx migrate` 按编号顺序执行；规则见 `deploy/migrations/README.md`（全文在 [01](../../plans/completed/v1-foundation-step00-14/01-STEP00-FOUNDATION-AND-GUARD-SKELETON.md) §00.5.4）。每个迁移只属于一个 Step，契约 PR 按编号顺序合入；已合入 `main` 的迁移文件不再修改，修正以下一个编号的迁移完成。

---

## 1. 迁移清单

| 编号 | 文件 | 所属 Step | 内容概要 | 定义位置 |
| :--- | :--- | :--- | :--- | :--- |
| 0001 | `0001_init_core.sql` | 仓库已有 | 扩展 `vector`、`pgcrypto`；超图 `hg_*`；业务节点 `xb_*`；`ax_flows`、`ax_sandboxes` | 仓库文件 |
| 0002 | `0002_platform_foundation.sql` | Step 00 | 角色 `axpoint_app` / `axpoint_lake`、`lake_catalog` 模式、租户注册、`ax_enable_tenant_rls`、身份表、审计、任务队列、外发表、保留策略、内置角色 | [01](../../plans/completed/v1-foundation-step00-14/01-STEP00-FOUNDATION-AND-GUARD-SKELETON.md) §00.5.2 |
| 0003 | `0003_data_fabric.sql` | Step 02 | 凭证、连接、数据集、列画像、同步记录、上传、Webhook、流位点、质量规则、术语、血缘 | [03](../../plans/completed/v1-foundation-step00-14/03-STEP02-AXHUB-CONNECTORS-AND-SYNC.md) |
| 0004 | `0004_axgraph_timeseries.sql` | Step 03 | 节点属性 / 地理 / 全文 / 行版本；关系唯一索引；时序分区表与汇总；实体链接；图算法结果 | [04](../../plans/completed/v1-foundation-step00-14/04-STEP03-AXGRAPH-STORAGE-AND-GRAPH-QUERY.md) |
| 0005 | `0005_analytics_semantic.sql` | Step 04 | 指标、保存查询、查询历史、问数轮次、AI 处理草稿 | [05](../../plans/completed/v1-foundation-step00-14/05-STEP04-AXANALYTICS-OLAP-AND-NL2SQL.md) |
| 0006 | `0006_xbluestudio_modeling.sql` | Step 05 | 部门子图注册、节点 / 关系类型扩展、模型头 / 版本 / 草稿、映射与映射运行 | [06](../../plans/completed/v1-foundation-step00-14/06-STEP05-06-XBLUESTUDIO-MODELING-AND-ROLLUP.md) |
| 0007 | `0007_axflow_orchestration.sql` | Step 07 | 流程 DAG 列与链式转换、版本、部署、运行、步骤日志、审批、动作日志、钩子 | [07](../../plans/completed/v1-foundation-step00-14/07-STEP07-08-AXFLOW-ORCHESTRATION-RULES-ALERTS.md) |
| 0008 | `0008_rules_alerts.sql` | Step 08 | 通知渠道、规则、告警、告警事件、投递记录、静默 | [07](../../plans/completed/v1-foundation-step00-14/07-STEP07-08-AXFLOW-ORCHESTRATION-RULES-ALERTS.md) |
| 0009 | `0009_publishing_agents.sql` | Step 09 | 模型提供方与用量、发布端点、MCP 客户端、智能体与版本、会话与消息、嵌入 | [08](../../plans/completed/v1-foundation-step00-14/08-STEP09-AXAGENT-PUBLISHING-MCP-AGENTS.md) |
| 0010 | `0010_axsim_scenarios.sql` | Step 10 | 沙盘范围、基线快照、写时复制覆盖层、场景、影响、检查点、决策 | [09](../../plans/completed/v1-foundation-step00-14/09-STEP10-AXSIM-SIMULATION-AND-DECISION.md) §10.1 |
| 0011 | `0011_xblueview_dashboards.sql` | Step 11 | 驾驶舱、组件、资产、平面图摆放、3D 孪生场景绑定 | [10](../../plans/completed/v1-foundation-step00-14/10-STEP11-12-XBLUEVIEW-TWIN-DASHBOARD-EXPORT.md) §11.1 |
| 0012 | `0012_export_and_share.sql` | Step 12 | 出品模板、出品任务、分享链接 | [10](../../plans/completed/v1-foundation-step00-14/10-STEP11-12-XBLUEVIEW-TWIN-DASHBOARD-EXPORT.md) §12.1 |
| 0013 | `0013_axguard_policies.sql` | Step 13 | 登录安全、会话、部门与属性、资源归属与密级、Cedar 策略、行列策略、跨部门授权、PII 位置、审计链 | [11](../../plans/completed/v1-foundation-step00-14/11-STEP13-AXGUARD-SECURITY-GOVERNANCE.md) §13.1 |
| 0014 | `0014_ops_templates_delivery.sql` | Step 14 | 工作项、模板安装、备份记录、审计链锚点 | [12](../../plans/completed/v1-foundation-step00-14/12-STEP14-OPERATIONS-TEMPLATES-ACCEPTANCE.md) §14.1 |

Step 01 与 Step 06 不含迁移（Step 06 使用 0006 中的映射表与 0004 中的时序表）。

---

## 2. 表 → 迁移 → Step

| 表 | 创建 | 扩展 | 所属 Step | RLS |
| :--- | :--- | :--- | :--- | :--- |
| `hg_docs` | 0001 | 0003（`upload_id`） | 仓库已有 / 02 | 是（0002 开启） |
| `hg_paras` | 0001 | 0013（`pii_spans`） | 仓库已有 / 13 | 是 |
| `hg_entities` | 0001 | 0004 | 仓库已有 / 03 | 是 |
| `hg_edges`、`hg_embeddings` | 0001 | — | 仓库已有 | 是 |
| `xb_node_types` | 0001 | 0006、0013 | 05 / 13 | 是 |
| `xb_edge_types` | 0001 | 0006 | 05 | 是 |
| `xb_nodes` | 0001 | 0004 | 03 | 是 |
| `xb_links` | 0001 | 0004 | 03 | 是 |
| `ax_flows` | 0001 | 0007 | 07 | 是 |
| `ax_sandboxes` | 0001 | 0010 | 10 | 是 |
| `sec_tenants` | 0002 | — | 00 | 否（平台级，应用只读） |
| `sec_users` | 0002 | 0013 | 00 / 13 | 是 |
| `sec_roles`、`sec_user_roles`、`sec_api_tokens` | 0002 | — | 00 | 是 |
| `sec_audit_logs` | 0002 | 0013（哈希链） | 00 / 13 | 是（应用角色无 UPDATE / DELETE） |
| `ax_jobs`、`ax_outbox`、`ax_retention_policies` | 0002 | — | 00 | 是 |
| `ds_credentials`、`ds_connections`、`ds_column_meta`、`ds_sync_jobs`、`ds_uploads`、`ds_webhooks`、`ds_webhook_buffer`、`ds_stream_offsets`、`ds_quality_rules`、`ds_glossary_terms`、`ds_lineage` | 0003 | — | 02 | 是 |
| `ds_datasets` | 0003 | 0013（归属、密级） | 02 / 13 | 是 |
| `xb_metric_series`（按月分区，含 `xb_metric_series_default`） | 0004 | — | 03 | 是 |
| `xb_metric_rollups`、`xb_rollup_watermarks`、`xb_rollup_dirty`、`xb_entity_links`、`xb_graph_algo_results` | 0004 | — | 03 | 是 |
| `an_metrics`、`an_saved_queries`、`an_query_history`、`an_ask_turns`、`an_transform_drafts` | 0005 | — | 04 | 是 |
| `xb_subgraphs` | 0006 | 0013（密级） | 05 / 13 | 是 |
| `xb_schema_heads`、`xb_schema_versions`、`xb_schema_drafts`、`xb_mappings`、`xb_mapping_runs` | 0006 | — | 05 / 06 | 是 |
| `ax_flow_versions`、`ax_flow_deployments`、`ax_flow_runs`、`ax_flow_step_logs`、`ax_action_log`、`ax_flow_hooks` | 0007 | — | 07 | 是 |
| `ax_approvals` | 0007 | 0013（`GRANT` 来源） | 07 / 13 | 是 |
| `ax_notify_channels`、`ax_rules`、`ax_alerts`、`ax_alert_events`、`ax_notify_deliveries`、`ax_alert_silences` | 0008 | — | 08 | 是 |
| `ax_llm_providers` | 0009 | 0013（`allow_pii`） | 09 / 13 | 是 |
| `ax_llm_usage`、`ax_published_endpoints`、`ax_mcp_clients`、`ax_agent_versions`、`ax_conversations`、`ax_messages`、`ax_agent_embeds` | 0009 | — | 09 | 是 |
| `ax_agents` | 0009 | 0013（归属、密级） | 09 / 13 | 是 |
| `ax_sim_base_nodes`、`ax_sim_base_links`、`ax_sim_overlay_nodes`、`ax_sim_overlay_links`、`ax_sim_scenarios`、`ax_sim_impacts`、`ax_sim_checkpoints`、`ax_sim_decisions` | 0010 | — | 10 | 是 |
| `xv_dashboards` | 0011 | 0013（归属、密级） | 11 / 13 | 是 |
| `xv_widgets`、`xv_assets`、`xv_floor_placements`、`xv_scene3d_bindings` | 0011 | — | 11 | 是 |
| `xv_export_templates`、`xv_export_jobs`、`xv_share_links` | 0012 | — | 12 | 是 |
| `sec_departments`、`sec_user_attributes`、`sec_sessions`、`sec_policies`、`sec_row_policies`、`sec_row_policy_defaults`、`sec_column_policies`、`sec_grants` | 0013 | — | 13 | 是 |
| `sec_audit_chain_heads` | 0013 | 0014（锚点） | 13 / 14 | 是 |
| `sec_login_states` | 0013 | — | 13 | 否（登录前无租户上下文；一次性、10 分钟过期） |
| `ax_work_items`、`ax_work_item_events`、`ax_template_installs` | 0014 | — | 14 | 是 |
| `ax_backups` | 0014 | — | 14 | 否（平台级，仅 `default` 租户 `OPS_ADMIN` 经接口访问） |

DuckLake 目录表位于 `lake_catalog` 模式，属主为 `axpoint_lake`，由 DuckLake 扩展自行创建与迁移，不在 `deploy/migrations/` 中。

---

## 3. 行级安全与跨租户函数

- 全部带 `tenant_id` 的业务表通过 `ax_enable_tenant_rls(regclass)` 开启并强制 RLS，策略名 `p_tenant_isolation`，条件 `tenant_id = current_setting('app.tenant_id', true)`，并向 `axpoint_app` 授予 DML。
- 不启用 RLS 的表只有：`sec_tenants`、`sec_login_states`、`ax_backups`（各自理由见上表）。
- 后台任务与无登录态入口跨租户读取只通过以下 `SECURITY DEFINER` 函数（均 `SET search_path = public`，`REVOKE ALL FROM PUBLIC` 后只授予 `axpoint_app`）：

| 函数 | 迁移 | 用途 |
| :--- | :--- | :--- |
| `ax_lease_jobs(worker, kinds, limit, lease_s)` | 0002 | 任务租约 |
| `ax_claim_outbox(worker, limit, lease_s)` | 0002 | 外发事件认领 |
| `ax_resolve_api_token(hash)` | 0002 | 个人访问令牌定位 |
| `ax_resolve_webhook(tenant, slug)` | 0003 | Webhook 接收 |
| `ax_scheduled_datasets()` | 0003 | 同步调度扫描 |
| `ax_ensure_metric_partitions(months_back, months_ahead)` | 0004 | 时序分区预建与分区级 RLS 开启 |
| `ax_drop_expired_metric_partitions()` | 0004 | 卸载早于全部租户保留期的整月时序分区 |
| `ax_rollup_tenants()` | 0004 | 汇总任务扫描 |
| `ax_scheduled_flows()` | 0007 | Cron 触发扫描 |
| `ax_event_flows(topic)` | 0007 | 事件触发匹配 |
| `ax_resolve_flow_hook(tenant, slug)` | 0007 | 流程钩子 |
| `ax_absence_rules()` | 0008 | 缺失数据规则扫描 |
| `ax_resolve_agent_embed(hash)` | 0009 | 智能体嵌入 |
| `ax_resolve_share(hash)` | 0012 | 分享链接 |
| `ax_lookup_login(tenant, username)` | 0013 | 本地登录 |
| `ax_resolve_session(hash)` | 0013 | 刷新令牌 |

函数返回结果后，调用方立即以返回的 `tenant_id` 开启租户事务（`begin_tenant_tx`），后续读写全部受 RLS 约束。

---

## 4. 验证命令

```bash
# 迁移全部执行、无待执行
! sqlx migrate info --source deploy/migrations --database-url "$MIGRATION_DATABASE_URL" | grep -q pending

# 列出未启用 RLS 的业务表（期望只有 sec_tenants、sec_login_states、ax_backups、_sqlx_migrations）
psql "$MIGRATION_DATABASE_URL" -Atc "SELECT relname FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace
  WHERE n.nspname='public' AND c.relkind IN ('r','p') AND NOT c.relrowsecurity AND c.relispartition = false ORDER BY 1"

# 列出 SECURITY DEFINER 函数（期望与第 3 节表格一致）
psql "$MIGRATION_DATABASE_URL" -Atc "SELECT proname FROM pg_proc WHERE prosecdef AND pronamespace='public'::regnamespace ORDER BY 1"
```

每个含迁移的 Step 门禁都包含其所建表的跨租户隔离断言（`sql_app other "SELECT count(*) FROM <表>"` 为 0）。
