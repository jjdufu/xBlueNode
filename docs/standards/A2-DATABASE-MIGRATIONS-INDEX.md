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
| 0015 | `0015_llm_structured_grounding.sql` | Step 15 | 提供方受限解码能力分级、确定性种子、本地小模型标识、个人归属隔离与用户默认模型偏好 | [01-STEP15](../../plans/completed/v2-ux-and-grounded-ai-step15-22/grounded-ai/01-STEP15-CONSTRAINED-DECODING-AND-LLM-CAPS.md) §15.1 |
| 0016 | `0016_grounded_ask_eval.sql` | Step 16 | 问数轮次接地路径审计列（`grounding_mode`、`verified_numbers`、`verified_citations`、`abstained`、`slot_payload`）与双轨反幻觉评测运行表 `an_grounding_eval_runs` | [02-STEP16](../../plans/completed/v2-ux-and-grounded-ai-step15-22/grounded-ai/02-STEP16-ONTOLOGY-GROUNDED-ASK-AND-CITATION-GUARD.md) §16.1 |
| 0017 | `0017_axbuilder_scopes.sql` | Step 17 | `AxBuilder` 构建令牌档位与深链基址、开发子图沙盒元数据、运行态节点 `subgraph_slug` 隔离索引与审计动作扩展 | [03-STEP17](../../plans/completed/v2-ux-and-grounded-ai-step15-22/builder-and-ux/03-STEP17-AXBUILDER-MCP-AND-DEV-SUBGRAPH.md) §17.1 |
| 0018 | `0018_studio_proposal_status.sql` | Step 18 | 模型草案增量快照表 `xb_schema_draft_snapshots`、`xb_schema_drafts.pass_count`、节点类型语义分层/业务域列与 `STUDIO_SNAPSHOT_RESTORE` 审计动作 | [04-STEP18](../../plans/completed/v2-ux-and-grounded-ai-step15-22/builder-and-ux/04-STEP18-STUDIO-COPILOT-LINEAGE-AND-SPEC-DOC.md) §18.1 |
| 0019 | `0019_im_bot_bindings.sql` | Step 21 | 企业 IM 出站长连接连接器 `ax_im_connectors`、身份映射 `ax_im_user_bindings`、一次性挑战码 `ax_im_bind_challenges`、事件去重 `ax_im_event_dedup`、`ax_conversations` IM 通道扩展及单主租约与原子绑定核销函数 | [07-STEP21](../../plans/completed/v2-ux-and-grounded-ai-step15-22/builder-and-ux/07-STEP21-IM-BIDIRECTIONAL-AGENT-CONNECTORS.md) §21.1 |
| 0020 | `0020_saved_charts.sql` | Step 25 | 可复用可视化图表资产表 `xv_charts`（强制租户 RLS）与 `CHART_PUBLISH` 审计动作扩展 | [03-STEP25](../../plans/completed/v3-craft-and-world-runtime-step23-30/craft-ux/03-STEP25-CHART-STUDIO-VTABLE-PIVOT-AND-MAGNETIC-GRID.md) §25.1 |
| 0021 | `0021_actions_and_edge_dynamics.sql` | Step 26 | `xb_node_types.actions`、`xb_edge_types.spec`、`xb_links.{metrics,row_version}`、边时序表 `xb_link_metric_series`、子图运行时认证记录 `xb_runtime_certifications` 及对象动作审计/审批约束扩展 | [04-STEP26](../../plans/completed/v3-craft-and-world-runtime-step23-30/world-runtime/04-STEP26-ACTION-CONTRACTS-EDGE-DYNAMICS-AND-L4-CERT.md) §26.1 |
| 0022 | `0022_state_visibility_policies.sql` | Step 27 | `sec_row_policies` 状态驱动动态行级视野模式 `policy_mode` 与活跃期退场列 `ttl_timestamp_field` / `ttl_max_age_hours` | [05-STEP27](../../plans/completed/v3-craft-and-world-runtime-step23-30/world-runtime/05-STEP27-AXQL-ONTOLOGY-COMPILER-AND-DYNAMIC-VISIBILITY.md) §27.1 |
| 0023 | `0023_sim_events_and_geoshape.sql` | Step 28 | `xb_nodes.geo_shape`、`ax_sim_base_nodes` / `ax_sim_overlay_nodes` 空间几何列，以及 `ax_sim_scenarios` 定时/条件事件队列与 `deadline_report` 列 | [06-STEP28](../../plans/completed/v3-craft-and-world-runtime-step23-30/world-runtime/06-STEP28-SIM-EVENT-QUEUE-DEADLINE-AND-RUST-GEOSHAPE.md) §28.1 |
| 0024 | `0024_streams_and_harness_runs.sql` | Step 29 | `ds_datasets` 常驻流租约与 `LOG_CDC` 位点列、单主流租约函数、`xb_competency_suites`、`xb_harness_runs` 及 `HARNESS_VERIFY_RUN` 审计动作 | [07-STEP29](../../plans/completed/v3-craft-and-world-runtime-step23-30/world-runtime/07-STEP29-STREAM-WORKERS-LOG-CDC-AND-AUTONOMOUS-HARNESS.md) §29.1 |

Step 01 与 Step 06 不含迁移（Step 06 使用 0006 中的映射表与 0004 中的时序表）。

---

## 2. 表 → 迁移 → Step

| 表 | 创建 | 扩展 | 所属 Step | RLS |
| :--- | :--- | :--- | :--- | :--- |
| `hg_docs` | 0001 | 0003（`upload_id`） | 仓库已有 / 02 | 是（0002 开启） |
| `hg_paras` | 0001 | 0013（`pii_spans`） | 仓库已有 / 13 | 是 |
| `hg_entities` | 0001 | 0004 | 仓库已有 / 03 | 是 |
| `hg_edges`、`hg_embeddings` | 0001 | — | 仓库已有 | 是 |
| `xb_node_types` | 0001 | 0006、0013、0018（语义分层与业务域）、0021（`actions` 契约） | 05 / 13 / 18 / 26 | 是 |
| `xb_edge_types` | 0001 | 0006、0021（`spec` 边派生与聚合规则） | 05 / 26 | 是 |
| `xb_nodes` | 0001 | 0004、0006、0017（`subgraph_slug` 隔离）、0023（`geo_shape`） | 03 / 05 / 17 / 28 | 是 |
| `xb_links` | 0001 | 0004、0006、0017（`subgraph_slug` 隔离）、0021（`metrics`、`row_version`） | 03 / 05 / 17 / 26 | 是 |
| `ax_flows` | 0001 | 0007 | 07 | 是 |
| `ax_sandboxes` | 0001 | 0010 | 10 | 是 |
| `sec_tenants` | 0002 | — | 00 | 否（平台级，应用只读） |
| `sec_users` | 0002 | 0013 | 00 / 13 | 是 |
| `sec_roles`、`sec_user_roles` | 0002 | — | 00 | 是 |
| `sec_api_tokens` | 0002 | 0017（`token_profile`、`web_base_url`） | 00 / 09 / 17 | 是 |
| `sec_audit_logs` | 0002 | 0013（哈希链）、0017（构建与克隆动作）、0018（快照回退动作）、0019（`IM_ACCOUNT_BIND`）、0020（`CHART_PUBLISH`）、0021（`OBJECT_ACTION_INVOKE`、`TRIAL_TICK_CERTIFY`）、0024（`HARNESS_VERIFY_RUN`） | 00 / 13 / 17 / 18 / 21 / 25 / 26 / 29 | 是（应用角色无 UPDATE / DELETE） |
| `ax_jobs`、`ax_outbox` | 0002 | — | 00 | 是 |
| `ax_retention_policies` | 0002 | 0019（`IM_EVENT_DEDUP` 目标） | 00 / 21 | 是 |
| `ds_credentials`、`ds_connections`、`ds_column_meta`、`ds_sync_jobs`、`ds_uploads`、`ds_webhooks`、`ds_webhook_buffer`、`ds_stream_offsets`、`ds_quality_rules`、`ds_glossary_terms`、`ds_lineage` | 0003 | — | 02 | 是 |
| `ds_datasets` | 0003 | 0013（归属、密级）、0024（`LOG_CDC`、常驻流与租约列） | 02 / 13 / 29 | 是 |
| `xb_metric_series`（按月分区，含 `xb_metric_series_default`） | 0004 | — | 03 | 是 |
| `xb_metric_rollups`、`xb_rollup_watermarks`、`xb_rollup_dirty`、`xb_entity_links`、`xb_graph_algo_results` | 0004 | — | 03 | 是 |
| `xb_link_metric_series`、`xb_runtime_certifications` | 0021 | — | 26 | 是 |
| `xb_competency_suites`、`xb_harness_runs` | 0024 | — | 29 | 是 |
| `an_metrics`、`an_saved_queries`、`an_query_history`、`an_transform_drafts` | 0005 | — | 04 | 是 |
| `an_ask_turns` | 0005 | 0016（接地路径与核验计数列） | 04 / 16 | 是 |
| `an_grounding_eval_runs` | 0016 | — | 16 | 是 |
| `xb_subgraphs` | 0006 | 0013（密级）、0017（开发沙盒元数据） | 05 / 13 / 17 | 是 |
| `xb_schema_heads`、`xb_schema_versions`、`xb_mappings`、`xb_mapping_runs` | 0006 | — | 05 / 06 | 是 |
| `xb_schema_drafts` | 0006 | 0018（`pass_count`） | 05 / 18 | 是 |
| `xb_schema_draft_snapshots` | 0018 | — | 18 | 是 |
| `ax_flow_versions`、`ax_flow_deployments`、`ax_flow_runs`、`ax_flow_step_logs`、`ax_action_log`、`ax_flow_hooks` | 0007 | — | 07 | 是 |
| `ax_approvals` | 0007 | 0013（`GRANT` 来源） | 07 / 13 | 是 |
| `ax_notify_channels`、`ax_rules`、`ax_alerts`、`ax_alert_events`、`ax_notify_deliveries`、`ax_alert_silences` | 0008 | — | 08 | 是 |
| `ax_llm_providers` | 0009 | 0013（`allow_pii`）、0015（受限解码与个人归属） | 09 / 13 / 15 | 是 |
| `ax_user_llm_preferences` | 0015 | — | 15 | 是 |
| `ax_llm_usage`、`ax_published_endpoints`、`ax_mcp_clients`、`ax_agent_versions`、`ax_messages`、`ax_agent_embeds` | 0009 | — | 09 | 是 |
| `ax_conversations` | 0009 | 0019（`IM` 通道、`im_connector_id`、`external_chat_id`） | 09 / 21 | 是 |
| `ax_im_connectors`、`ax_im_user_bindings`、`ax_im_bind_challenges`、`ax_im_event_dedup` | 0019 | — | 21 | 是 |
| `ax_agents` | 0009 | 0013（归属、密级） | 09 / 13 | 是 |
| `ax_sim_base_nodes`、`ax_sim_base_links`、`ax_sim_overlay_nodes`、`ax_sim_overlay_links`、`ax_sim_scenarios`、`ax_sim_impacts`、`ax_sim_checkpoints`、`ax_sim_decisions` | 0010 | 0023（空间几何列、事件队列与 `deadline_report`） | 10 / 28 | 是 |
| `xv_dashboards` | 0011 | 0013（归属、密级） | 11 / 13 | 是 |
| `xv_widgets`、`xv_assets`、`xv_floor_placements`、`xv_scene3d_bindings` | 0011 | — | 11 | 是 |
| `xv_charts` | 0020 | — | 25 | 是 |
| `xv_export_templates`、`xv_export_jobs`、`xv_share_links` | 0012 | — | 12 | 是 |
| `sec_departments`、`sec_user_attributes`、`sec_sessions`、`sec_policies`、`sec_row_policy_defaults`、`sec_column_policies`、`sec_grants` | 0013 | — | 13 | 是 |
| `sec_row_policies` | 0013 | 0022（`policy_mode`、`ttl_timestamp_field`、`ttl_max_age_hours`） | 13 / 27 | 是 |
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
| `ax_resolve_api_token(hash)` | 0002 / 0017 | 个人访问令牌定位（含 `token_profile` 与 `web_base_url`） |
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
| `ax_claim_im_connectors(holder, limit, lease_s)` | 0019 | IM 连接器单主租约认领与续期 |
| `ax_release_im_connectors(holder)` | 0019 | 优雅停机释放本实例持有的 IM 连接器租约 |
| `ax_consume_im_bind_challenge(hash, tenant, user_id)` | 0019 | 原子校验核销一次性 IM 绑定挑战码并写入绑定记录 |
| `ax_claim_stream_datasets(holder, limit, lease_s)` | 0024 | 常驻流消费数据集单主租约认领与续期 |
| `ax_release_stream_datasets(holder)` | 0024 | 优雅停机释放本实例持有的常驻流消费租约 |

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
