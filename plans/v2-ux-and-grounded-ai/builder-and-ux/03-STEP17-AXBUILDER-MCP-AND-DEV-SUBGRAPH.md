# 03 · Step 17：`AxBuilder` 构建态 MCP、细粒度构建令牌与开发子图沙盒

本步骤将平台 `/api/mcp`（Streamable HTTP）从单一运行态消费升级为**运行态 + 构建态双平面**，向外部 AI 编码客户端暴露 9 个受细粒度权限（`DATA_READ` / `STUDIO_EDIT` / `FLOW_EDIT` / `VIEW_EDIT` / `DATA_SYNC`）管控的 `AxBuilder` 构建工具与 2 类目录资源，同步引入不可直改生产资产的 `BUILDER` 专用令牌预设、前端深链自动回传机制，以及与企业主图（`main`）严格隔离的开发子图运行态沙盒。

关联总览与工程约束：
- [00-V2-MASTER-INDEX.md](../00-V2-MASTER-INDEX.md)
- [IRON_RULES.md](../../../IRON_RULES.md)
- [A1-CONVENTIONS-ENUMS-ROUTES.md](../../../docs/standards/A1-CONVENTIONS-ENUMS-ROUTES.md)
- [A2-DATABASE-MIGRATIONS-INDEX.md](../../../docs/standards/A2-DATABASE-MIGRATIONS-INDEX.md)
- [06-STEP05-06-XBLUESTUDIO-MODELING-AND-ROLLUP.md](../../completed/v1-foundation-step00-14/06-STEP05-06-XBLUESTUDIO-MODELING-AND-ROLLUP.md)
- [08-STEP09-AXAGENT-PUBLISHING-MCP-AGENTS.md](../../completed/v1-foundation-step00-14/08-STEP09-AXAGENT-PUBLISHING-MCP-AGENTS.md)

---

## 17.0 目标、前置与契约

- **目标**：
  1. 在 `sec_api_tokens` 引入 `token_profile`（`GENERAL | BUILDER`）与可选 `web_base_url`，对 `BUILDER` 令牌实施权限裁剪与生产态写保护（禁止直接发布 `PROD` 模型、流程、驾驶舱或端点，所有写操作仅落入 `DRAFT` 或开发子图沙盒并记录 `BUILDER_MCP_MUTATE` 审计链）。
  2. 在 `xb_subgraphs`、`xb_nodes`、`xb_links` 建立开发子图沙盒运行态隔离（`is_dev_sandbox`、`source_template_id`、`last_reset_at` 与运行态节点级 `subgraph_slug` 命名空间），提供开发子图试灌与运行态节点清空能力，确保企业主图 `main` 零污染。
  3. 在 `axpoint-agent::mcp_server` 扩展 9 个 `axbuilder_*` 构建态工具与 `axpoint://templates/{id}`、`axpoint://subgraphs/{slug}` 资源，所有检视与变更工具统一回传结构化前端深链 `deep_links`（`studio_url`、`flow_url`、`view_url`）。
  4. 在 `AxAgentPage.tsx`（`MCP` 页签）提供面向 `Antigravity`、`Cursor`、`Claude Code`、`Codex` 四类客户端的配置片段生成器与复制控件。
- **前置**：Step 14 通过（所属并行层 **L12**，可与 Step 15 并发实施；契约迁移按 `0015 → 0017` 顺序合入）。
- **覆盖能力项**：`C66`、`C67`（映射至 [A7-REQUIREMENTS-TRACEABILITY.md](../../../docs/standards/A7-REQUIREMENTS-TRACEABILITY.md)）。
- **契约清单**：
  - `deploy/migrations/0017_axbuilder_scopes.sql`
  - `axpoint-domain::enums`：`TokenProfile`（`GENERAL | BUILDER`），`AuditAction` 追加 `BuilderMcpMutate`、`TrialTemplateClone`
  - `axpoint-domain::builder`：`DeepLinks`、`BuilderPreflightResponse`、`WorkspacePullResponse`、`PushSchemaDraftResponse`、`CloneTemplateSandboxResponse`、`UpsertFlowDraftResponse`、`DevSubgraphRunRequest`、`DevSubgraphRunResponse`、`DevSubgraphResetResponse`、`UpsertDashboardDraftResponse`
  - `axpoint-domain::ports`：`BuilderDashboardPort`、`TemplateSandboxPort`（供 `axpoint-agent` 无耦合访问看板草稿与模板包，遵守 `IRON_RULES.md` 依赖倒置红线）
  - `docs/API_CONTRACT.md`：`/api/auth/tokens` 扩展字段、`POST /api/xblue/subgraphs/{slug}/reset-nodes`、`POST /api/xblue/subgraphs/{slug}/dry-run-materialize`、`/api/mcp` 构建态工具清单
  - `xbluenode-web/src/lib/api/{axagent.ts, xblue.ts}`

### 文件变更清单

| 动作 | 路径 | 内容 |
| :--- | :--- | :--- |
| 新建 | `deploy/migrations/0017_axbuilder_scopes.sql` | 令牌构建档位、开发子图元数据、节点级 `subgraph_slug` 隔离与审计枚举扩展（§17.1） |
| 修改 | `axpoint-core/crates/axpoint-domain/src/enums.rs` | 新增 `TokenProfile`；`AuditAction` 追加 `BuilderMcpMutate`、`TrialTemplateClone` |
| 新建 | `axpoint-core/crates/axpoint-domain/src/builder.rs` | `AxBuilder` 请求/响应 DTO、深链拼装纯函数与 `BUILDER` 权限裁剪规则（§17.2） |
| 修改 | `axpoint-core/crates/axpoint-domain/src/{lib.rs, platform.rs, ports.rs, studio/doc.rs}` | 导出 `builder` 模块；`ApiTokenRecord` / `MeResponse` / `SubgraphInfo` 增加字段；声明 `BuilderDashboardPort` 与 `TemplateSandboxPort` |
| 修改 | `axpoint-core/crates/axpoint-security/src/{principal.rs, authenticator.rs}` | `AuthPrincipal` 携带 `token_profile` 与 `web_base_url`；新增 `enforce_no_prod_mutation` 守卫（§17.2.2） |
| 新建 | `axpoint-core/crates/axpoint-agent/src/mcp_builder.rs` | 9 个 `axbuilder_*` 工具 JSON Schema、调度逻辑、审计发射与 `axpoint://` 构建态资源读取（§17.3） |
| 修改 | `axpoint-core/crates/axpoint-agent/src/mcp_server.rs` | `McpServerHub` 挂载 `mcp_builder` 工具集与动态模板资源 `resources/templates/list` |
| 修改 | `axpoint-core/crates/axpoint-db/src/{security_repo.rs, studio_repo.rs, graph_store.rs}` | 令牌构建字段读写、开发子图节点隔离物化与 `reset_dev_subgraph_nodes` 事务（§17.4） |
| 修改 | `axpoint-core/crates/axpoint-server/src/routes/{auth.rs, xblue.rs, axflow.rs, xblueview.rs, axagent.rs}` | 挂载 `reset-nodes` / `dry-run-materialize` 路由；在生产发布端点挂载 `enforce_no_prod_mutation` |
| 修改 | `xbluenode-web/src/lib/api/{enums.ts, axagent.ts, xblue.ts}` | 对齐 `TokenProfile`、`SubgraphInfo` 与沙盒管理客户端方法 |
| 新建 | `xbluenode-web/src/components/axagent/McpBuilderConfigCard.tsx` | 四客户端（`Antigravity` / `Cursor` / `Claude Code` / `Codex`）MCP 配置片段生成器（§17.5） |
| 修改 | `xbluenode-web/src/pages/AxAgentPage.tsx`、`src/components/xbluestudio/StudioSubgraphBar.tsx` | 接入构建令牌签发、配置片段卡片与开发子图沙盒操作栏 |
| 新建 | `scripts/gate/step17.sh` | Step 17 自动化门禁（§17.6） |

---

## 17.1 迁移 `deploy/migrations/0017_axbuilder_scopes.sql`

```sql
-- 0017_axbuilder_scopes.sql：AxBuilder 构建令牌档位、前端深链基址、开发子图沙盒与运行态节点子图隔离

-- 1. 扩展个人访问令牌：区分通用令牌与受限构建令牌，记录可选前端深链基址
ALTER TABLE sec_api_tokens
    ADD COLUMN IF NOT EXISTS token_profile TEXT NOT NULL DEFAULT 'GENERAL'
        CHECK (token_profile IN ('GENERAL', 'BUILDER'));

ALTER TABLE sec_api_tokens
    ADD COLUMN IF NOT EXISTS web_base_url TEXT;

-- 更新令牌解析函数，向认证层返回 token_profile 与 web_base_url
DROP FUNCTION IF EXISTS ax_resolve_api_token(TEXT);
CREATE FUNCTION ax_resolve_api_token(p_token_hash TEXT)
RETURNS TABLE (
    token_id      UUID,
    tenant_id     TEXT,
    user_id       UUID,
    scopes        TEXT[],
    token_profile TEXT,
    web_base_url  TEXT
)
LANGUAGE sql SECURITY DEFINER SET search_path = public AS $$
    SELECT t.id, t.tenant_id, t.user_id, t.scopes, t.token_profile, t.web_base_url
      FROM sec_api_tokens t
     WHERE t.token_hash = p_token_hash
       AND t.revoked_at IS NULL
       AND (t.expires_at IS NULL OR t.expires_at > NOW());
$$;
REVOKE ALL ON FUNCTION ax_resolve_api_token(TEXT) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION ax_resolve_api_token(TEXT) TO axpoint_app;

-- 2. 扩展业务子图元数据：标识开发沙盒、来源模板包与最近清空时间
ALTER TABLE xb_subgraphs
    ADD COLUMN IF NOT EXISTS is_dev_sandbox BOOLEAN NOT NULL DEFAULT FALSE;

ALTER TABLE xb_subgraphs
    ADD COLUMN IF NOT EXISTS source_template_id TEXT;

ALTER TABLE xb_subgraphs
    ADD COLUMN IF NOT EXISTS last_reset_at TIMESTAMPTZ;

ALTER TABLE xb_subgraphs DROP CONSTRAINT IF EXISTS chk_xb_subgraphs_main_not_sandbox;
ALTER TABLE xb_subgraphs ADD CONSTRAINT chk_xb_subgraphs_main_not_sandbox
    CHECK (NOT (is_main AND is_dev_sandbox));

-- 3. 运行态业务节点与关系按 subgraph_slug 物理隔离，保障开发子图试灌与清空绝不影响企业主图 main
ALTER TABLE xb_nodes
    ADD COLUMN IF NOT EXISTS subgraph_slug TEXT NOT NULL DEFAULT 'main';

ALTER TABLE xb_nodes DROP CONSTRAINT IF EXISTS xb_nodes_tenant_id_type_name_node_key_key;
CREATE UNIQUE INDEX IF NOT EXISTS uq_xb_nodes_subgraph_key
    ON xb_nodes (tenant_id, subgraph_slug, type_name, node_key);
CREATE INDEX IF NOT EXISTS idx_xb_nodes_subgraph_type
    ON xb_nodes (tenant_id, subgraph_slug, type_name);

ALTER TABLE xb_links
    ADD COLUMN IF NOT EXISTS subgraph_slug TEXT NOT NULL DEFAULT 'main';

DROP INDEX IF EXISTS uq_xb_links_endpoints;
CREATE UNIQUE INDEX IF NOT EXISTS uq_xb_links_subgraph_endpoints
    ON xb_links (tenant_id, subgraph_slug, link_type, src_node_id, dst_node_id);
CREATE INDEX IF NOT EXISTS idx_xb_links_subgraph
    ON xb_links (tenant_id, subgraph_slug);

-- 4. 扩展审计日志动作集合（含 Step 17 构建变更与 Step 19 模板演练克隆动作）
ALTER TABLE sec_audit_logs DROP CONSTRAINT IF EXISTS sec_audit_logs_action_check;
ALTER TABLE sec_audit_logs ADD CONSTRAINT sec_audit_logs_action_check
    CHECK (action IN (
        'LOGIN','LOGOUT','TOKEN_ISSUE','TOKEN_REVOKE','QUERY_SQL','ASK_DATA','RAG_ASK','DATA_SYNC',
        'FILE_UPLOAD','SCHEMA_PUBLISH','FLOW_RUN','APPROVAL_DECIDE','MCP_CALL','AGENT_CHAT','SIM_APPLY',
        'EXPORT_FILE','SHARE_CREATE','SHARE_ACCESS','POLICY_CHANGE','BACKUP_RUN','TEMPLATE_INSTALL',
        'BUILDER_MCP_MUTATE','TRIAL_TEMPLATE_CLONE'
    ));
```

---

## 17.2 领域契约、令牌守卫与深链解析（`axpoint-domain` & `axpoint-security`）

### 17.2.1 构建态公共类型（`axpoint-domain/src/builder.rs`）

```rust
//! Contracts and pure helpers for `AxBuilder` build-plane MCP tools and dev subgraph sandboxes.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::enums::{Permission, TokenProfile};
use crate::studio::{MappingRejectSample, SchemaDoc, SubgraphInfo, ValidationIssue};
use crate::topology::BlueNode;

/// Allowed permissions on a `TokenProfile::Builder` personal access token.
pub const BUILDER_ALLOWED_PERMISSIONS: &[Permission] = &[
    Permission::DataRead,
    Permission::DataSync,
    Permission::StudioEdit,
    Permission::FlowEdit,
    Permission::FlowRun,
    Permission::RuleEdit,
    Permission::AgentEdit,
    Permission::AgentRun,
    Permission::SimRun,
    Permission::ViewEdit,
];

/// Permissions strictly stripped and forbidden when `token_profile == BUILDER`.
pub const BUILDER_FORBIDDEN_PROD_PERMISSIONS: &[Permission] = &[
    Permission::StudioPublish,
    Permission::FlowPublish,
    Permission::SimApply,
    Permission::GuardAdmin,
    Permission::OpsAdmin,
    Permission::PiiRawRead,
];

/// Frontend deep links returned alongside every `AxBuilder` MCP tool invocation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeepLinks {
    /// Direct URL to `xBlueStudio` positioned on the target subgraph and optional draft.
    pub studio_url: String,
    /// Direct URL to `AxFlow` positioned on the target flow draft (or `/flow` root).
    pub flow_url: String,
    /// Direct URL to `xBlueView` positioned on the target dashboard draft (or `/view` root).
    pub view_url: String,
}

/// Resolves deterministic frontend URLs from token `web_base_url`, `AXPOINT_PUBLIC_HOST`, or localhost fallback.
#[must_use]
pub fn resolve_deep_links(
    token_web_base_url: Option<&str>,
    env_public_host: Option<&str>,
    subgraph_slug: &str,
    draft_id: Option<Uuid>,
    flow_id: Option<Uuid>,
    dashboard_id: Option<Uuid>,
) -> DeepLinks {
    let base = token_web_base_url
        .filter(|u| is_valid_http_origin(u))
        .or_else(|| env_public_host.filter(|u| is_valid_http_origin(u)))
        .unwrap_or("http://127.0.0.1:5173")
        .trim_end_matches('/');

    let studio_url = match draft_id {
        Some(did) => format!("{base}/studio?subgraph={subgraph_slug}&draft={did}"),
        None => format!("{base}/studio?subgraph={subgraph_slug}"),
    };
    let flow_url = match flow_id {
        Some(fid) => format!("{base}/flow?flow={fid}"),
        None => format!("{base}/flow"),
    };
    let view_url = match dashboard_id {
        Some(vid) => format!("{base}/view?dashboard={vid}"),
        None => format!("{base}/view"),
    };
    DeepLinks {
        studio_url,
        flow_url,
        view_url,
    }
}

fn is_valid_http_origin(raw: &str) -> bool {
    let trimmed = raw.trim();
    (trimmed.starts_with("http://") || trimmed.starts_with("https://"))
        && !trimmed.contains(char::is_whitespace)
}

/// Request payload for dry-run materialization inside a dev sandbox subgraph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DevSubgraphRunRequest {
    /// Optional draft UUID whose `SchemaDoc` and `mappings` should be materialized; uses head if `None`.
    #[serde(default)]
    pub draft_id: Option<Uuid>,
    /// Maximum rows read per mapped dataset (`1..=5_000`, defaults to `200`).
    #[serde(default = "default_limit_rows")]
    pub limit_rows: u32,
}

const fn default_limit_rows() -> u32 {
    200
}

/// Execution summary of `axbuilder_run_dev_subgraph` and `POST /api/xblue/subgraphs/{slug}/dry-run-materialize`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DevSubgraphRunResponse {
    /// Target dev sandbox subgraph slug.
    pub subgraph_slug: String,
    /// Number of nodes upserted inside the sandbox namespace.
    pub nodes_upserted: u64,
    /// Number of links upserted inside the sandbox namespace.
    pub links_upserted: u64,
    /// Number of derived/aggregated metric points computed.
    pub points_computed: u64,
    /// Up to 10 materialized sample nodes for immediate inspection.
    pub sample_nodes: Vec<BlueNode>,
    /// Up to 20 rejected row diagnostics.
    pub reject_samples: Vec<MappingRejectSample>,
    /// Frontend deep links.
    pub deep_links: DeepLinks,
}

/// Execution summary of `POST /api/xblue/subgraphs/{slug}/reset-nodes` and `axbuilder_reset_dev_subgraph`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DevSubgraphResetResponse {
    /// Target dev sandbox subgraph slug.
    pub subgraph_slug: String,
    /// Deleted runtime node count in `xb_nodes`.
    pub nodes_deleted: u64,
    /// Deleted runtime link count in `xb_links`.
    pub links_deleted: u64,
    /// Deleted time-series point count in `xb_metric_series`.
    pub series_points_deleted: u64,
    /// Updated reset timestamp recorded on `xb_subgraphs.last_reset_at`.
    pub last_reset_at: DateTime<Utc>,
    /// Frontend deep links.
    pub deep_links: DeepLinks,
}
```

### 17.2.2 `BUILDER` 令牌双重生产防线（`axpoint-security`）

1. **签发与鉴权时权限收敛**：
   - 在 `POST /api/auth/tokens` 创建 `token_profile == "BUILDER"` 令牌时，若请求未传 `scopes`，默认赋予 `["DATA_READ", "STUDIO_EDIT", "FLOW_EDIT", "VIEW_EDIT", "AGENT_RUN", "SIM_RUN"]`（与用户自身角色权限求交集）；若请求显式传入包含 `BUILDER_FORBIDDEN_PROD_PERMISSIONS` 中任一权限（如 `STUDIO_PUBLISH`、`FLOW_PUBLISH`、`OPS_ADMIN`），直接返回 `400 Bad Request`（`error_code = "BUILDER_SCOPE_FORBIDDEN"`）。
   - 在 `Authenticator::authenticate` 解析 `TokenKind::Pat` 且 `token_profile == TokenProfile::Builder` 时，二次从 `AuthPrincipal.permissions` 中剔除全部 `BUILDER_FORBIDDEN_PROD_PERMISSIONS`，确保历史令牌同样受控。
2. **路由入口显式生产写拦截（`AuthPrincipal::enforce_no_prod_mutation`）**：
   - 新增方法 `pub fn enforce_no_prod_mutation(&self) -> Result<(), AppError>`：当 `self.token_profile == TokenProfile::Builder` 时立即返回 `Err(AppError::Forbidden("BUILDER_TOKEN_PROD_FORBIDDEN"))`。
   - 挂载于全部生产态提升路由入口：
     - `POST /api/xblue/drafts/{id}/publish`（发布子图或主图正式版本）
     - `PUT /api/xblue/schema`（直接覆写模型定义）
     - `POST /api/xblue/subgraphs/{slug}/merge`（将子图合并入企业主图 `main`）
     - `POST /api/axflow/flows/{id}/deploy`（当请求 `environment == "PROD"` 时）
     - `POST /api/xblueview/dashboards/{id}/publish`（将驾驶舱切换为 `PUBLISHED`）
     - `POST /api/axagent/endpoints`（当请求 `environment == "PROD"` 时）

---

## 17.3 `AxBuilder` 构建态 MCP 工具集与目录资源（`axpoint-agent::mcp_server`）

### 17.3.1 双平面工具发现与执行协议

`McpServerHub<R>` 在保留原有 7 个运行态只读工具与租户已发布 `PROD` 端点的基础上，通过构造注入 `builder_dashboard: Arc<dyn BuilderDashboardPort>`、`template_sandbox: Arc<dyn TemplateSandboxPort>` 与 `public_host: Option<String>`：

- **`tools/list` 动态可见性**：按调用方 `AuthPrincipal` 实际持有的细粒度权限过滤并返回对应 `axbuilder_*` 工具描述符（例如仅持 `DATA_READ + STUDIO_EDIT` 的令牌可见 `axbuilder_preflight`、`axbuilder_pull_workspace`、`axbuilder_validate_schema`、`axbuilder_push_schema_draft`、`axbuilder_clone_template`、`axbuilder_reset_dev_subgraph`，不暴露无权限的 `axbuilder_upsert_flow_draft` 或 `axbuilder_upsert_dashboard_draft`）。
- **审计双轨记账**：
  - 只读检视与静态校验工具（`axbuilder_preflight`、`axbuilder_pull_workspace`、`axbuilder_validate_schema`）记录 `AuditAction::McpCall`。
  - 凡产生草稿或沙盒状态变更的 6 个工具（`axbuilder_push_schema_draft`、`axbuilder_clone_template`、`axbuilder_upsert_flow_draft`、`axbuilder_run_dev_subgraph`、`axbuilder_reset_dev_subgraph`、`axbuilder_upsert_dashboard_draft`）除记录 MCP 调用耗时外，强制通过 `AuditWriter` 追加 `AuditAction::BuilderMcpMutate`（`resource_ref` 为目标子图/草案/流程/看板标识，`detail_json` 含 `tool_name`、`token_profile`、`draft_id` 与变更摘要）。
- **PII 安全铁律**：`axbuilder_pull_workspace` 与 `axbuilder_run_dev_subgraph` 输出的数据集列画像与样例节点属性强制经过 `axpoint-security::masking::mask_json_properties` 脱敏，不因构建态通道绕过 `AxGuard` 行列安全策略。

### 17.3.2 9 个 `AxBuilder` 工具规格表

| 工具名 | 必需权限 | 入参 Schema 要点（`inputSchema`） | 执行语义与草稿隔离约束 | 返回结构核心字段（均含 `deep_links`） |
| :--- | :--- | :--- | :--- | :--- |
| `axbuilder_preflight` | `DATA_READ` | `{ subgraph_slug?: string }` | 检查核心库、DuckLake 与模型提供方连通性，返回当前令牌 `token_profile`、有效权限数组、子图清单、可用模板包列表及受限解码支持档位 | `{ healthy, token_profile, permissions, subgraphs, templates, llm_capabilities, deep_links }` |
| `axbuilder_pull_workspace` | `DATA_READ` | `{ subgraph_slug?: string, include_datasets?: boolean, include_flows?: boolean, include_dashboards?: boolean }` | 拉取目标子图（默认 `main`）当前已发布 `SchemaDoc`、最新 `DRAFT` 草案、数据集列画像（不含原始敏感值）及关联流程/看板元数据摘要 | `{ subgraph, published_version, published_doc, active_draft, datasets, flows, dashboards, deep_links }` |
| `axbuilder_validate_schema` | `STUDIO_EDIT` | `{ subgraph_slug?: string, schema_doc: SchemaDoc }` | 加载租户数据集画像上下文，调用 `axpoint_domain::studio::validate` 执行标识符、关系基数、派生表达式 AST、跨边聚合环路与映射列类型核验，零数据库写操作 | `{ valid, error_count, warning_count, issues, deep_links }` |
| `axbuilder_push_schema_draft` | `STUDIO_EDIT` | `{ subgraph_slug?: string, draft_id?: string, title: string, schema_doc: SchemaDoc }` | 校验结构合法性后在 `xb_schema_drafts` 新建或乐观更新 `status = 'DRAFT'`、`source = 'AI'` 草案（严禁修改 `xb_schema_versions`），写 `BUILDER_MCP_MUTATE` 审计 | `{ draft_id, subgraph_slug, base_version, status: "DRAFT", error_count, warning_count, issues, deep_links }` |
| `axbuilder_clone_template` | `STUDIO_EDIT` | `{ pack_id: string, target_slug?: string, display_name?: string }` | 从 `templates/packs/{pack_id}` 创建 `is_dev_sandbox = true` 的独立开发子图（默认 `dev_<pack>_<short4>`），载入模板 `SchemaDoc` 与样例节点，写 `BUILDER_MCP_MUTATE` 审计 | `{ subgraph_slug, is_dev_sandbox: true, source_template_id, node_type_count, edge_type_count, draft_id, deep_links }` |
| `axbuilder_upsert_flow_draft` | `FLOW_EDIT` | `{ flow_id?: string, name: string, slug?: string, description?: string, nodes: FlowNode[], edges: FlowEdge[], trigger_cfg?: object }` | 校验算子配置与有向无环图拓扑后写入 `ax_flows`（强制 `status = 'DRAFT'`，不触碰 `ax_flow_deployments` 正式环境绑定），写 `BUILDER_MCP_MUTATE` 审计 | `{ flow_id, name, status: "DRAFT", node_count, edge_count, deep_links }` |
| `axbuilder_run_dev_subgraph` | `STUDIO_EDIT` + `DATA_SYNC` | `{ subgraph_slug: string, draft_id?: string, limit_rows?: integer }` | 硬校验目标子图 `is_dev_sandbox == true`（否则返回 `DEV_SANDBOX_REQUIRED`）；在子图命名空间内物化至多 `limit_rows` 行数据并计算派生与聚合指标，写 `BUILDER_MCP_MUTATE` 审计 | `DevSubgraphRunResponse`（含 `nodes_upserted`, `links_upserted`, `points_computed`, `sample_nodes`, `reject_samples`, `deep_links`） |
| `axbuilder_reset_dev_subgraph` | `STUDIO_EDIT` | `{ subgraph_slug: string }` | 硬校验目标子图 `is_dev_sandbox == true && !is_main`；清空该子图命名空间下的 `xb_nodes`、`xb_links` 及对应 `xb_metric_series`，保留模型定义与草案，更新 `last_reset_at`，写 `BUILDER_MCP_MUTATE` 审计 | `DevSubgraphResetResponse`（含 `nodes_deleted`, `links_deleted`, `series_points_deleted`, `last_reset_at`, `deep_links`） |
| `axbuilder_upsert_dashboard_draft` | `VIEW_EDIT` | `{ dashboard_id?: string, name: string, slug: string, theme?: "light"\|"dark"\|"screen", filters?: array, widgets: WidgetDraftSpec[] }` | 校验组件坐标与数据源绑定结构，写入 `xv_dashboards` 与 `xv_widgets`（强制 `status = 'DRAFT'`），写 `BUILDER_MCP_MUTATE` 审计 | `{ dashboard_id, slug, status: "DRAFT", widget_count, deep_links }` |

### 17.3.3 构建态 MCP 目录资源（`resources/list`、`resources/templates/list`、`resources/read`）

在既有静态目录资源基础上新增两类 URI 模板资源（均需 `DATA_READ`，`mimeType = "application/json"`）：

1. **`axpoint://templates/{id}`**：
   - `resources/list` 自动枚举 `templates/packs/*` 下全部已内置模板包（如 `axpoint://templates/supply_chain`、`axpoint://templates/manufacturing_equipment`、`axpoint://templates/retail_store`）。
   - `resources/read` 返回 `{ manifest, readme_markdown, schema_doc, metrics, mappings }`，供外部编码代理在克隆前精确对齐模板实体与指标口径。
2. **`axpoint://subgraphs/{slug}`**：
   - `resources/list` 枚举当前租户可见子图（如 `axpoint://subgraphs/main`、`axpoint://subgraphs/dev_sc`）。
   - `resources/read` 返回 `{ subgraph_info, current_version, published_schema_doc, draft_summaries, runtime_counts: { node_count, link_count } }`。

---

## 17.4 开发子图沙盒与构建令牌 REST 接口（`axpoint-server`）

| 方法与路径 | 所需权限 | 请求体摘要 | 响应体摘要与状态码 |
| :--- | :--- | :--- | :--- |
| `POST /api/auth/tokens`（扩展） | 登录会话或 `ADMIN` | `{ name, expires_in_days?, scopes?, token_profile?: "GENERAL"\|"BUILDER", web_base_url?: string }` | `201 Created`：`{ id, name, token, token_prefix, scopes, token_profile, web_base_url, expires_at }` |
| `GET /api/auth/tokens`（扩展） | 登录会话 | — | `200 OK`：`{ items: [{ id, name, token_prefix, scopes, token_profile, web_base_url, last_used_at, expires_at, created_at }] }` |
| `POST /api/xblue/subgraphs`（扩展） | `STUDIO_EDIT` | `{ slug, display_name, description?, owner_dept?, is_dev_sandbox?: boolean }` | `201 Created`：`SubgraphInfo`（含 `is_dev_sandbox`, `source_template_id`, `last_reset_at`） |
| `POST /api/xblue/subgraphs/{slug}/dry-run-materialize` | `STUDIO_EDIT` + `DATA_SYNC` | `{ draft_id?: string, limit_rows?: number }` | `200 OK`：`DevSubgraphRunResponse`；非沙盒子图返回 `400 DEV_SANDBOX_REQUIRED` |
| `POST /api/xblue/subgraphs/{slug}/reset-nodes` | `STUDIO_EDIT` | `{}` | `200 OK`：`DevSubgraphResetResponse`；对 `main` 或 `is_dev_sandbox = false` 返回 `400 DEV_SANDBOX_REQUIRED` |

**`reset-nodes` 事务不变量（`axpoint-db::studio_repo::reset_dev_subgraph_nodes`）**：
1. 在 `begin_tenant_tx` 内 `SELECT is_main, is_dev_sandbox FROM xb_subgraphs WHERE tenant_id = $1 AND slug = $2 FOR UPDATE`；若不存在返回 `404 SUBGRAPH_NOT_FOUND`；若 `is_main || !is_dev_sandbox` 立即回滚并返回 `AppError::BadRequest("DEV_SANDBOX_REQUIRED")`。
2. 依次执行：
   - `DELETE FROM xb_metric_series WHERE tenant_id = $1 AND node_id IN (SELECT id FROM xb_nodes WHERE tenant_id = $1 AND subgraph_slug = $2)`
   - `DELETE FROM xb_links WHERE tenant_id = $1 AND subgraph_slug = $2`
   - `DELETE FROM xb_nodes WHERE tenant_id = $1 AND subgraph_slug = $2`
   - `UPDATE xb_subgraphs SET last_reset_at = NOW(), updated_at = NOW() WHERE tenant_id = $1 AND slug = $2 RETURNING last_reset_at`
3. 主图 `subgraph_slug = 'main'` 与其他子图的节点、关系、时序点及当前子图的 `xb_node_types`、`xb_edge_types`、`xb_schema_drafts` 保持原封不动。

---

## 17.5 前端接入配置生成器与沙盒交互（`xbluenode-web`）

### 17.5.1 `McpBuilderConfigCard.tsx`（挂载于 `AxAgentPage.tsx` → `MCP` 页签）

| 区域 | 控件与交互 | 文案规范（中 / 英 `tx()`） |
| :--- | :--- | :--- |
| 卡片头部 | 标题 + 当前接入地址徽标 `{origin}/api/mcp` | 标题：`构建态 MCP 接入` / `Builder MCP Setup` |
| 令牌快速签发栏 | 档位选择（`Select`：`构建令牌` / `通用令牌`）、名称输入框（标签 `令牌名称`，占位符 `axbuilder-local`）、基址输入框（标签 `前端地址`，占位符 `http://127.0.0.1:5173`，默认填入 `window.location.origin`）、主按钮 | 按钮：`签发令牌` / `Issue Token` |
| 客户端切换页签 | `Tabs` 四项：`Antigravity`、`Cursor`、`Claude Code`、`Codex` | 页签直接使用客户端标准名 |
| 配置代码框与操作 | 只读等宽代码块（未签发内存明文时显示 `<YOUR_BUILDER_PAT>` 占位，签发后实时嵌入本次内存态明文令牌，离开页面自动销毁）+ 右上角复制按钮 | 按钮：`复制配置` / `Copy Config`；状态反馈：`已复制` / `Copied` |

四类客户端生成的标准配置模板结构：

- **`Antigravity` / `Cursor`**（`mcpServers` JSON 片段）：
  ```json
  {
    "mcpServers": {
      "xbluenode-axbuilder": {
        "serverUrl": "http://127.0.0.1:9995/api/mcp",
        "headers": {
          "Authorization": "Bearer <YOUR_BUILDER_PAT>"
        }
      }
    }
  }
  ```
- **`Claude Code`**（CLI 注册命令）：
  ```bash
  claude mcp add --transport http xbluenode-axbuilder http://127.0.0.1:9995/api/mcp \
    --header "Authorization: Bearer <YOUR_BUILDER_PAT>"
  ```
- **`Codex`**（`config.toml` 片段）：
  ```toml
  [mcp_servers.xbluenode_axbuilder]
  url = "http://127.0.0.1:9995/api/mcp"
  http_headers = { Authorization = "Bearer <YOUR_BUILDER_PAT>" }
  ```

### 17.5.2 `StudioSubgraphBar.tsx` 开发沙盒控制条

- 当激活子图 `subgraph.is_dev_sandbox === true` 时，子图选择器右侧展示蓝色徽标 `开发沙盒`（`Dev Sandbox`）及节点计数徽标。
- 操作按钮组（仅在 `can("STUDIO_EDIT")` 且 `is_dev_sandbox` 为真时渲染）：
  - `试灌数据`（`Dry Run`）：调用 `POST /api/xblue/subgraphs/{slug}/dry-run-materialize`，完成后刷新底栏运行态节点预览。
  - `清空节点`（`Reset Nodes`）：弹出确认对话框，正文说明「将清空当前开发沙盒子图内的全部运行态节点、关系与时序点，模型草案与主图数据不受影响」，底部按钮为 `取消` / `清空`。

---

## 17.6 门禁 `scripts/gate/step17.sh`

```bash
#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "$0")/lib.sh"

# 1. 单元与文档测试：覆盖深链解析、BUILDER 令牌裁剪、MCP 构建态工具调度与子图节点隔离
(cd axpoint-core && cargo test -p axpoint-domain builder && cargo test -p axpoint-security && cargo test -p axpoint-agent mcp)

# 2. 签发受限 BUILDER 令牌（带自定义 web_base_url 验证深链回传）
BUILDER_WEB_URL="https://studio.xblue.example"
TOK_RES=$(api POST /api/auth/tokens "{
  \"name\": \"gate-step17-builder\",
  \"expires_in_days\": 7,
  \"token_profile\": \"BUILDER\",
  \"web_base_url\": \"$BUILDER_WEB_URL\",
  \"scopes\": [\"DATA_READ\", \"DATA_SYNC\", \"STUDIO_EDIT\", \"FLOW_EDIT\", \"VIEW_EDIT\"]
}")
BUILDER_PAT=$(echo "$TOK_RES" | jq -r .token)
expect_eq "builder-profile-saved" "$(echo "$TOK_RES" | jq -r .token_profile)" "BUILDER"

bpat() {
  curl -sS -H "Authorization: Bearer $BUILDER_PAT" -H "Accept: application/json" \
       -H "Content-Type: application/json" "$@"
}

# 3. 初始化 MCP 会话并验证构建态工具可见性与预检
INIT_OUT=$(curl -sS -X POST "$CORE_BASE_URL/api/mcp" \
  -H "Authorization: Bearer $BUILDER_PAT" \
  -H "Accept: application/json, text/event-stream" \
  -H "Content-Type: application/json" \
  -D .gate/mcp17.h \
  -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"gate-axbuilder","version":"1.0"}}}')
SID17=$(awk 'tolower($1)=="mcp-session-id:" {gsub(/\r/,"",$2); print $2}' .gate/mcp17.h)

bmcp() {
  curl -sS -X POST "$CORE_BASE_URL/api/mcp" \
    -H "Authorization: Bearer $BUILDER_PAT" \
    -H "Accept: application/json, text/event-stream" \
    -H "Content-Type: application/json" \
    -H "Mcp-Session-Id: $SID17" \
    -d "$1" | sed -n 's/^data: //p;/^{/p' | tail -1
}

bmcp '{"jsonrpc":"2.0","method":"notifications/initialized"}' >/dev/null || true

PREFLIGHT_RPC=$(bmcp '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"axbuilder_preflight","arguments":{"subgraph_slug":"main"}}}')
PREFLIGHT_BODY=$(echo "$PREFLIGHT_RPC" | jq -r '.result.content[0].text')
expect_json "mcp-axbuilder-preflight" "$PREFLIGHT_BODY" \
  '.healthy == true and .token_profile == "BUILDER" and (.subgraphs | length) >= 1'

# 4. 创建开发沙盒子图并通过 MCP 推送模型草案 + 核验前端深链回传
DEV_SLUG="dev_step17_sbx"
api POST /api/xblue/subgraphs "{\"slug\":\"$DEV_SLUG\",\"display_name\":\"Step17 开发沙盒\",\"is_dev_sandbox\":true}" >/dev/null || true

PUSH_RPC=$(bmcp "$(jq -nc --arg slug "$DEV_SLUG" '{
  jsonrpc: "2.0",
  id: 3,
  method: "tools/call",
  params: {
    name: "axbuilder_push_schema_draft",
    arguments: {
      subgraph_slug: $slug,
      title: "MCP 构建草案验证",
      schema_doc: {
        node_types: [{
          name: "SandboxHub",
          display_name: "沙盒枢纽",
          properties: [
            { name: "region", display_name: "区域", data_type: "STRING", kind: "ATTRIBUTE", required: true },
            { name: "stock_qty", display_name: "库存量", unit: "件", data_type: "FLOAT64", kind: "METRIC", required: false }
          ]
        }],
        edge_types: [],
        mappings: []
      }
    }
  }
}')")
PUSH_BODY=$(echo "$PUSH_RPC" | jq -r '.result.content[0].text')
DRAFT_ID=$(echo "$PUSH_BODY" | jq -r .draft_id)
expect_json "mcp-push-draft" "$PUSH_BODY" \
  '.status == "DRAFT" and .error_count == 0 and (.draft_id | length) > 10'
expect_json "mcp-deeplink" "$PUSH_BODY" \
  "(.deep_links.studio_url | startswith(\"$BUILDER_WEB_URL/studio?subgraph=$DEV_SLUG&draft=$DRAFT_ID\"))"

# 核验 BUILDER_MCP_MUTATE 审计动作落盘
expect_eq "builder-mcp-audit" \
  "$(sql_app default "SELECT count(*) > 0 FROM sec_audit_logs WHERE action = 'BUILDER_MCP_MUTATE'")" "t"

# 5. 验证 BUILDER 令牌严禁直接发布生产模型版本或部署 PROD 资产
PUB_CODE=$(curl -sS -o /dev/null -w '%{http_code}' -X POST \
  -H "Authorization: Bearer $BUILDER_PAT" -H "Content-Type: application/json" \
  "$CORE_BASE_URL/api/xblue/drafts/$DRAFT_ID/publish" -d '{"change_summary":"attempt prod publish"}')
expect_eq "builder-token-no-prod" "$PUB_CODE" "403"

# 6. 验证开发沙盒子图试灌与清空（且企业主图 main 拒绝清空、主图节点数零变化）
MAIN_BEFORE=$(sql_app default "SELECT count(*) FROM xb_nodes WHERE subgraph_slug = 'main'")
expect_code "main-reset-rejected" 400 POST "/api/xblue/subgraphs/main/reset-nodes" '{}'

CLONE_RPC=$(bmcp '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"axbuilder_clone_template","arguments":{"pack_id":"supply_chain","target_slug":"dev_step17_tpl"}}}')
CLONE_BODY=$(echo "$CLONE_RPC" | jq -r '.result.content[0].text')
TPL_SLUG=$(echo "$CLONE_BODY" | jq -r .subgraph_slug)

RUN_RPC=$(bmcp "$(jq -nc --arg slug "$TPL_SLUG" '{
  jsonrpc: "2.0",
  id: 5,
  method: "tools/call",
  params: { name: "axbuilder_run_dev_subgraph", arguments: { subgraph_slug: $slug, limit_rows: 100 } }
}')")
RUN_BODY=$(echo "$RUN_RPC" | jq -r '.result.content[0].text')
expect_json "dev-subgraph-populated" "$RUN_BODY" '.nodes_upserted > 0'

RESET_RES=$(api POST "/api/xblue/subgraphs/$TPL_SLUG/reset-nodes" '{}')
MAIN_AFTER=$(sql_app default "SELECT count(*) FROM xb_nodes WHERE subgraph_slug = 'main'")
SBX_REMAIN=$(sql_app default "SELECT count(*) FROM xb_nodes WHERE subgraph_slug = '$TPL_SLUG'")
expect_json "dev-subgraph-reset" "$RESET_RES" \
  ".nodes_deleted > 0 and (.last_reset_at != null) and ($SBX_REMAIN == 0) and ($MAIN_BEFORE == $MAIN_AFTER)"

gate_pass step17
```

**通过标准**：最后一行输出 `GATE step17 PASS`；`axpoint-agent` 与 `axpoint-security` 单测覆盖 9 个构建态工具权限矩阵、深链回退优先级、`BUILDER` 令牌生产写拦截与开发沙盒隔离清空。
