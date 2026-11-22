//! `AxBuilder` build-plane `MCP` tools (`axbuilder_*`) and `axpoint://` workspace/template
//! catalog resources (`03-STEP17` §17.3).

use std::collections::BTreeMap;

use axpoint_domain::AppError;
use axpoint_domain::axhub::DatasetListQuery;
use axpoint_domain::builder::{
    BuilderDatasetSummary, BuilderFlowBrief, BuilderLlmCapabilitySummary, BuilderPreflightResponse,
    DeepLinks, DevSubgraphRunRequest, PushSchemaDraftResponse, UpsertDashboardDraftInput,
    UpsertFlowDraftResponse, ValidateSchemaDraftResponse, WorkspacePullResponse,
    resolve_deep_links,
};
use axpoint_domain::enums::{AuditAction, MaskRule, Permission, PiiCategory, StructuredOutputMode};
use axpoint_domain::flow::{
    FlowEdge, FlowGraph, FlowIssueSeverity, FlowListQuery, FlowNode, TriggerCfg,
    UpdateAxFlowRequest,
};
use axpoint_domain::graph::NodeListQuery;
use axpoint_domain::platform::AuditRecord;
use axpoint_domain::ports::{
    AgentRepository, AnalyticsRepository, DatasetRepository, FlowRepository, GraphQueryPort,
    GraphStorePort, StudioRepository,
};
use axpoint_domain::studio::{
    CreateDraftRequest, DraftSource, DraftStatus, IssueSeverity, SchemaDoc, UpdateDraftRequest,
    validate as validate_schema_doc,
};
use axpoint_domain::topology::{BlueNode, CreateAxFlowRequest};
use axpoint_security::AuthPrincipal;
use axpoint_security::masking::mask_json_properties;
use chrono::Utc;
use serde::Serialize;
use uuid::Uuid;

use crate::mcp_server::{
    McpCallMeta, McpResourceDescriptor, McpResourceTemplateDescriptor, McpServerHub,
    McpToolDescriptor, jsonrpc_ok, optional_u32_arg, required_str_arg,
};
use crate::publish::validate_instance_against_schema;

/// Ordered list of all 9 Step 17 `AxBuilder` build-plane `MCP` tool names (`03-STEP17` §17.3.2).
pub const ALL_AXBUILDER_TOOL_NAMES: &[&str] = &[
    "axbuilder_preflight",
    "axbuilder_pull_workspace",
    "axbuilder_validate_schema",
    "axbuilder_push_schema_draft",
    "axbuilder_clone_template",
    "axbuilder_upsert_flow_draft",
    "axbuilder_run_dev_subgraph",
    "axbuilder_reset_dev_subgraph",
    "axbuilder_upsert_dashboard_draft",
];

/// Returns `true` if `name` is one of the 9 built-in `AxBuilder` build-plane `MCP` tools.
#[must_use]
pub fn is_axbuilder_mcp_tool(name: &str) -> bool {
    ALL_AXBUILDER_TOOL_NAMES.contains(&name)
}

/// Returns `true` when `principal` holds all permissions required to discover and invoke `tool_name`.
#[must_use]
pub fn has_axbuilder_tool_permissions(principal: &AuthPrincipal, tool_name: &str) -> bool {
    match tool_name {
        "axbuilder_preflight" | "axbuilder_pull_workspace" => principal.has(Permission::DataRead),
        "axbuilder_validate_schema"
        | "axbuilder_push_schema_draft"
        | "axbuilder_clone_template"
        | "axbuilder_reset_dev_subgraph" => principal.has(Permission::StudioEdit),
        "axbuilder_run_dev_subgraph" => {
            principal.has(Permission::StudioEdit) && principal.has(Permission::DataSync)
        }
        "axbuilder_upsert_flow_draft" => principal.has(Permission::FlowEdit),
        "axbuilder_upsert_dashboard_draft" => principal.has(Permission::ViewEdit),
        _ => false,
    }
}

fn require_axbuilder_permissions(
    principal: &AuthPrincipal,
    tool_name: &str,
) -> Result<(), AppError> {
    match tool_name {
        "axbuilder_preflight" | "axbuilder_pull_workspace" => {
            principal.require(Permission::DataRead)
        }
        "axbuilder_validate_schema"
        | "axbuilder_push_schema_draft"
        | "axbuilder_clone_template"
        | "axbuilder_reset_dev_subgraph" => principal.require(Permission::StudioEdit),
        "axbuilder_run_dev_subgraph" => {
            principal.require(Permission::StudioEdit)?;
            principal.require(Permission::DataSync)
        }
        "axbuilder_upsert_flow_draft" => principal.require(Permission::FlowEdit),
        "axbuilder_upsert_dashboard_draft" => principal.require(Permission::ViewEdit),
        _ => Err(AppError::bad(
            "INVALID_ARGUMENT",
            format!("unknown AxBuilder MCP tool: {tool_name}"),
        )),
    }
}

pub(crate) fn visible_axbuilder_tools(principal: &AuthPrincipal) -> Vec<McpToolDescriptor> {
    let specs = [
        (
            "axbuilder_preflight",
            "检查核心库、数据湖与大模型提供方就绪状态，返回当前令牌档位、有效权限、子图与行业模板清单及前端深链",
        ),
        (
            "axbuilder_pull_workspace",
            "拉取目标子图已发布 SchemaDoc、当前 DRAFT 草案、脱敏数据集列画像及流程/看板摘要",
        ),
        (
            "axbuilder_validate_schema",
            "零落盘静态校验 SchemaDoc（标识符、基数、派生表达式 AST、跨边聚合环路与映射列结构）",
        ),
        (
            "axbuilder_push_schema_draft",
            "在 xb_schema_drafts 新建或更新 DRAFT 模型草案（绝不修改已发布版本）并返回深链",
        ),
        (
            "axbuilder_clone_template",
            "将内置行业模板包克隆为独立开发沙盒子图（is_dev_sandbox = true）与 DRAFT 草案",
        ),
        (
            "axbuilder_upsert_flow_draft",
            "静态校验 DAG 后新建或更新 DRAFT 编排流程（不触碰 PROD 部署绑定）",
        ),
        (
            "axbuilder_run_dev_subgraph",
            "在开发沙盒子图命名空间内试灌映射数据并计算派生与聚合指标（严禁污染企业主图 main）",
        ),
        (
            "axbuilder_reset_dev_subgraph",
            "清空开发沙盒子图的运行态节点、关系与时序点（保留模型定义与草案，企业主图 main 拒绝清空）",
        ),
        (
            "axbuilder_upsert_dashboard_draft",
            "校验 24 列栅格坐标与数据源绑定后新建或更新 DRAFT 数字孪生看板草稿",
        ),
    ];

    specs
        .into_iter()
        .filter(|(name, _)| has_axbuilder_tool_permissions(principal, name))
        .map(|(name, desc)| McpToolDescriptor {
            name: String::from(name),
            description: String::from(desc),
            input_schema: axbuilder_tool_schema(name),
        })
        .collect()
}

/// Returns the JSON Schema (`inputSchema`) for a Step 17 `axbuilder_*` tool (`03-STEP17` §17.3.2).
#[must_use]
pub fn axbuilder_tool_schema(name: &str) -> serde_json::Value {
    match name {
        "axbuilder_preflight" => serde_json::json!({
            "type": "object",
            "properties": {
                "subgraph_slug": { "type": "string", "minLength": 1 }
            },
            "additionalProperties": false
        }),
        "axbuilder_pull_workspace" => serde_json::json!({
            "type": "object",
            "properties": {
                "subgraph_slug": { "type": "string", "minLength": 1 },
                "include_datasets": { "type": "boolean" },
                "include_flows": { "type": "boolean" },
                "include_dashboards": { "type": "boolean" }
            },
            "additionalProperties": false
        }),
        "axbuilder_validate_schema" => serde_json::json!({
            "type": "object",
            "required": ["schema_doc"],
            "properties": {
                "subgraph_slug": { "type": "string", "minLength": 1 },
                "schema_doc": { "type": "object" }
            },
            "additionalProperties": false
        }),
        "axbuilder_push_schema_draft" => serde_json::json!({
            "type": "object",
            "required": ["title", "schema_doc"],
            "properties": {
                "subgraph_slug": { "type": "string", "minLength": 1 },
                "draft_id": { "type": "string", "format": "uuid" },
                "title": { "type": "string", "minLength": 1 },
                "schema_doc": { "type": "object" }
            },
            "additionalProperties": false
        }),
        "axbuilder_clone_template" => serde_json::json!({
            "type": "object",
            "required": ["pack_id"],
            "properties": {
                "pack_id": { "type": "string", "minLength": 1 },
                "target_slug": { "type": "string", "minLength": 1 },
                "display_name": { "type": "string", "minLength": 1 }
            },
            "additionalProperties": false
        }),
        "axbuilder_upsert_flow_draft" => serde_json::json!({
            "type": "object",
            "required": ["name", "nodes", "edges"],
            "properties": {
                "flow_id": { "type": "string", "format": "uuid" },
                "name": { "type": "string", "minLength": 1 },
                "slug": { "type": "string" },
                "description": { "type": "string" },
                "nodes": { "type": "array", "items": { "type": "object" } },
                "edges": { "type": "array", "items": { "type": "object" } },
                "trigger_cfg": { "type": "object" }
            },
            "additionalProperties": false
        }),
        "axbuilder_run_dev_subgraph" => serde_json::json!({
            "type": "object",
            "required": ["subgraph_slug"],
            "properties": {
                "subgraph_slug": { "type": "string", "minLength": 1 },
                "draft_id": { "type": "string", "format": "uuid" },
                "limit_rows": { "type": "integer", "minimum": 1, "maximum": 5000 }
            },
            "additionalProperties": false
        }),
        "axbuilder_reset_dev_subgraph" => serde_json::json!({
            "type": "object",
            "required": ["subgraph_slug"],
            "properties": {
                "subgraph_slug": { "type": "string", "minLength": 1 }
            },
            "additionalProperties": false
        }),
        "axbuilder_upsert_dashboard_draft" => serde_json::json!({
            "type": "object",
            "required": ["name", "slug", "widgets"],
            "properties": {
                "dashboard_id": { "type": "string", "format": "uuid" },
                "name": { "type": "string", "minLength": 1 },
                "slug": { "type": "string", "minLength": 1 },
                "theme": { "type": "string", "enum": ["light", "dark", "screen"] },
                "filters": { "type": "array", "items": { "type": "object" } },
                "widgets": { "type": "array", "items": { "type": "object" } }
            },
            "additionalProperties": false
        }),
        _ => serde_json::json!({ "type": "object" }),
    }
}

pub(crate) fn axbuilder_resource_templates() -> Vec<McpResourceTemplateDescriptor> {
    vec![
        McpResourceTemplateDescriptor {
            uri_template: "axpoint://templates/{id}",
            name: "Industry Template Pack",
            description: "Built-in industry template manifest, README, SchemaDoc, semantic metrics, and dataset mappings",
            mime_type: "application/json",
        },
        McpResourceTemplateDescriptor {
            uri_template: "axpoint://subgraphs/{slug}",
            name: "Business Subgraph Workspace",
            description: "Subgraph metadata, published SchemaDoc, open drafts, and isolated runtime node/link counts",
            mime_type: "application/json",
        },
    ]
}

fn optional_slug_or_main(args: &serde_json::Value) -> &str {
    args.get("subgraph_slug")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("main")
}

fn optional_str_arg<'a>(args: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    args.get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn optional_uuid_arg(args: &serde_json::Value, key: &str) -> Result<Option<Uuid>, AppError> {
    let Some(raw) = optional_str_arg(args, key) else {
        return Ok(None);
    };
    let parsed = Uuid::parse_str(raw)
        .map_err(|err| AppError::bad("SCHEMA_INVALID", format!("invalid UUID '{key}': {err}")))?;
    Ok(Some(parsed))
}

fn non_nil_uid(uid: Uuid) -> Option<Uuid> {
    (!uid.is_nil()).then_some(uid)
}

fn mode_rank(mode: StructuredOutputMode) -> u8 {
    match mode {
        StructuredOutputMode::None => 0,
        StructuredOutputMode::JsonObject => 1,
        StructuredOutputMode::JsonSchema => 2,
    }
}

fn to_json_val<T: Serialize>(value: &T, label: &str) -> Result<serde_json::Value, AppError> {
    serde_json::to_value(value)
        .map_err(|err| AppError::Config(format!("serialize {label} failed: {err}")))
}

struct BuilderMutateAudit<'a> {
    tool_name: &'a str,
    resource_ref: String,
    draft_id: Option<Uuid>,
    summary: serde_json::Value,
    meta: McpCallMeta<'a>,
}

impl<R> McpServerHub<R>
where
    R: AgentRepository
        + AnalyticsRepository
        + StudioRepository
        + FlowRepository
        + DatasetRepository
        + GraphStorePort
        + GraphQueryPort
        + 'static,
{
    fn effective_web_base<'a>(&'a self, principal: &'a AuthPrincipal) -> Option<&'a str> {
        principal
            .web_base_url
            .as_deref()
            .or(self.public_host.as_deref())
    }

    fn build_deep_links(
        &self,
        principal: &AuthPrincipal,
        subgraph_slug: &str,
        draft_id: Option<Uuid>,
        flow_id: Option<Uuid>,
        dashboard_id: Option<Uuid>,
    ) -> DeepLinks {
        resolve_deep_links(
            principal.web_base_url.as_deref(),
            self.public_host.as_deref(),
            subgraph_slug,
            draft_id,
            flow_id,
            dashboard_id,
        )
    }

    async fn emit_builder_mutate_audit(
        &self,
        principal: &AuthPrincipal,
        event: BuilderMutateAudit<'_>,
    ) {
        let Some(ref writer) = self.audit else {
            return;
        };
        writer.record(AuditRecord {
            tenant_id: principal.tenant_id.clone(),
            user_id: non_nil_uid(principal.user_id),
            username: principal.username.clone(),
            action: AuditAction::BuilderMcpMutate,
            resource_ref: event.resource_ref,
            status_code: 200,
            client_ip: if event.meta.client_ip.is_empty() {
                String::from("127.0.0.1")
            } else {
                String::from(event.meta.client_ip)
            },
            trace_id: String::from(event.meta.trace_id),
            detail_json: serde_json::json!({
                "tool_name": event.tool_name,
                "token_profile": principal.token_profile,
                "draft_id": event.draft_id,
                "summary": event.summary,
            }),
            created_at: Utc::now(),
        });
        if let Err(err) = writer.flush().await {
            tracing::warn!(error = %err, tool = event.tool_name, "builder MCP mutate audit flush failed");
        }
    }

    pub(crate) async fn execute_axbuilder_tool(
        &self,
        principal: &AuthPrincipal,
        tool_name: &str,
        args: &serde_json::Value,
        meta: McpCallMeta<'_>,
    ) -> Result<serde_json::Value, AppError> {
        require_axbuilder_permissions(principal, tool_name)?;
        let schema = axbuilder_tool_schema(tool_name);
        validate_instance_against_schema(args, &schema)?;

        match tool_name {
            "axbuilder_preflight" => self.tool_preflight(principal, args).await,
            "axbuilder_pull_workspace" => self.tool_pull_workspace(principal, args).await,
            "axbuilder_validate_schema" => self.tool_validate_schema(principal, args).await,
            "axbuilder_push_schema_draft" => {
                self.tool_push_schema_draft(principal, args, meta).await
            }
            "axbuilder_clone_template" => self.tool_clone_template(principal, args, meta).await,
            "axbuilder_upsert_flow_draft" => {
                self.tool_upsert_flow_draft(principal, args, meta).await
            }
            "axbuilder_run_dev_subgraph" => self.tool_run_dev_subgraph(principal, args, meta).await,
            "axbuilder_reset_dev_subgraph" => {
                self.tool_reset_dev_subgraph(principal, args, meta).await
            }
            "axbuilder_upsert_dashboard_draft" => {
                self.tool_upsert_dashboard_draft(principal, args, meta)
                    .await
            }
            _ => Err(AppError::bad(
                "INVALID_ARGUMENT",
                format!("unknown AxBuilder MCP tool: {tool_name}"),
            )),
        }
    }

    async fn tool_preflight(
        &self,
        principal: &AuthPrincipal,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, AppError> {
        let tenant_id = &principal.tenant_id;
        let slug = optional_slug_or_main(args);
        let subgraphs = self.store.list_subgraphs(tenant_id).await?;
        let templates = match &self.template_sandbox {
            Some(sb) => sb.list_template_packs().await?,
            None => Vec::new(),
        };
        let providers = self.store.list_llm_providers(tenant_id).await?;
        let active: Vec<_> = providers.iter().filter(|p| p.enabled).collect();
        let highest_structured_mode = active
            .iter()
            .map(|p| p.structured_caps.structured_mode)
            .max_by_key(|m| mode_rank(*m))
            .unwrap_or(StructuredOutputMode::None);
        let supports_tools = active.iter().any(|p| p.flags.supports_tools);

        let resp = BuilderPreflightResponse {
            healthy: true,
            token_profile: principal.token_profile,
            permissions: principal.permissions.iter().copied().collect(),
            subgraphs,
            templates,
            llm_capabilities: BuilderLlmCapabilitySummary {
                active_provider_count: active.len(),
                highest_structured_mode,
                supports_tools,
            },
            deep_links: self.build_deep_links(principal, slug, None, None, None),
        };
        to_json_val(&resp, "BuilderPreflightResponse")
    }

    async fn tool_pull_workspace(
        &self,
        principal: &AuthPrincipal,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, AppError> {
        let tenant_id = &principal.tenant_id;
        let slug = optional_slug_or_main(args);
        let inc_ds = args
            .get("include_datasets")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true);
        let inc_flows = args
            .get("include_flows")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true);
        let inc_dash = args
            .get("include_dashboards")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true);

        let subgraphs = self.store.list_subgraphs(tenant_id).await?;
        let subgraph = subgraphs
            .into_iter()
            .find(|s| s.slug == slug)
            .ok_or(AppError::NotFound("SUBGRAPH_NOT_FOUND"))?;
        let published = self.store.get_published_schema(tenant_id, slug).await?;
        let drafts = self
            .store
            .list_drafts(tenant_id, slug, Some(DraftStatus::Draft))
            .await?;
        let active_draft = drafts.into_iter().next();

        let datasets = if inc_ds {
            self.collect_masked_dataset_summaries(tenant_id).await?
        } else {
            Vec::new()
        };
        let flows = if inc_flows {
            self.collect_flow_briefs(tenant_id).await?
        } else {
            Vec::new()
        };
        let dashboards = if inc_dash {
            match &self.builder_dashboard {
                Some(port) => port.list_dashboard_briefs(tenant_id).await?,
                None => Vec::new(),
            }
        } else {
            Vec::new()
        };

        let deep_links = self.build_deep_links(
            principal,
            slug,
            active_draft.as_ref().map(|d| d.id),
            flows.first().map(|f| f.id),
            dashboards.first().map(|d| d.id),
        );
        let resp = WorkspacePullResponse {
            subgraph,
            published_version: published.version,
            published_doc: published.doc,
            active_draft,
            datasets,
            flows,
            dashboards,
            deep_links,
        };
        to_json_val(&resp, "WorkspacePullResponse")
    }

    async fn collect_masked_dataset_summaries(
        &self,
        tenant_id: &str,
    ) -> Result<Vec<BuilderDatasetSummary>, AppError> {
        let page = self
            .store
            .list_datasets(
                tenant_id,
                DatasetListQuery {
                    limit: Some(200),
                    ..DatasetListQuery::default()
                },
            )
            .await?;
        let mut summaries = Vec::with_capacity(page.items.len());
        for ds in page.items {
            let cols = self.store.list_columns(tenant_id, ds.id).await?;
            let columns: Vec<serde_json::Value> = if cols.is_empty() {
                ds.schema_json
                    .iter()
                    .map(|c| {
                        serde_json::json!({
                            "name": c.name,
                            "data_type": c.data_type,
                            "pii_category": PiiCategory::None,
                        })
                    })
                    .collect()
            } else {
                cols.into_iter()
                    .map(|c| {
                        serde_json::json!({
                            "name": c.column_name,
                            "data_type": c.data_type,
                            "pii_category": c.pii_category,
                        })
                    })
                    .collect()
            };
            summaries.push(BuilderDatasetSummary {
                id: ds.id,
                name: ds.name,
                row_count: ds.row_count,
                columns,
            });
        }
        Ok(summaries)
    }

    async fn collect_flow_briefs(
        &self,
        tenant_id: &str,
    ) -> Result<Vec<BuilderFlowBrief>, AppError> {
        let page = self
            .store
            .list_flows(
                tenant_id,
                FlowListQuery {
                    limit: Some(200),
                    ..FlowListQuery::default()
                },
            )
            .await?;
        let mut out = Vec::with_capacity(page.items.len());
        for fl in page.items {
            let versions = self.store.list_flow_versions(tenant_id, fl.id).await?;
            let latest_version = versions.items.iter().map(|v| v.version).max().unwrap_or(0);
            out.push(BuilderFlowBrief {
                id: fl.id,
                name: fl.name,
                slug: fl.slug,
                status: fl.status,
                latest_version,
            });
        }
        Ok(out)
    }

    async fn tool_validate_schema(
        &self,
        principal: &AuthPrincipal,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, AppError> {
        let slug = optional_slug_or_main(args);
        let raw_doc = args
            .get("schema_doc")
            .cloned()
            .ok_or_else(|| AppError::bad("SCHEMA_INVALID", "missing schema_doc"))?;
        let doc: SchemaDoc = serde_json::from_value(raw_doc).map_err(|err| {
            AppError::bad(
                "SCHEMA_INVALID",
                format!("invalid SchemaDoc payload: {err}"),
            )
        })?;

        let ctx = self
            .store
            .load_validation_context(&principal.tenant_id, slug)
            .await?;
        let issues = validate_schema_doc(&doc, &ctx);
        let error_count = issues
            .iter()
            .filter(|i| i.severity == IssueSeverity::Error)
            .count();
        let warning_count = issues
            .iter()
            .filter(|i| i.severity == IssueSeverity::Warn)
            .count();

        let resp = ValidateSchemaDraftResponse {
            valid: error_count == 0,
            error_count,
            warning_count,
            issues,
            deep_links: self.build_deep_links(principal, slug, None, None, None),
        };
        to_json_val(&resp, "ValidateSchemaDraftResponse")
    }

    async fn tool_push_schema_draft(
        &self,
        principal: &AuthPrincipal,
        args: &serde_json::Value,
        meta: McpCallMeta<'_>,
    ) -> Result<serde_json::Value, AppError> {
        let tenant_id = &principal.tenant_id;
        let slug = optional_slug_or_main(args);
        let title = required_str_arg(args, "title")?;
        let maybe_draft_id = optional_uuid_arg(args, "draft_id")?;
        let raw_doc = args
            .get("schema_doc")
            .cloned()
            .ok_or_else(|| AppError::bad("SCHEMA_INVALID", "missing schema_doc"))?;
        let doc: SchemaDoc = serde_json::from_value(raw_doc).map_err(|err| {
            AppError::bad(
                "SCHEMA_INVALID",
                format!("invalid SchemaDoc payload: {err}"),
            )
        })?;

        let ctx = self.store.load_validation_context(tenant_id, slug).await?;
        let computed_issues = validate_schema_doc(&doc, &ctx);

        let draft = if let Some(did) = maybe_draft_id {
            let _existing = self
                .store
                .get_draft(tenant_id, did)
                .await?
                .ok_or(AppError::NotFound("DRAFT_NOT_FOUND"))?;
            self.store
                .update_draft(
                    tenant_id,
                    did,
                    UpdateDraftRequest {
                        title: Some(String::from(title)),
                        schema_doc: doc,
                    },
                )
                .await?
        } else {
            let published = self.store.get_published_schema(tenant_id, slug).await?;
            self.store
                .create_draft(
                    tenant_id,
                    slug,
                    non_nil_uid(principal.user_id),
                    CreateDraftRequest {
                        title: String::from(title),
                        schema_doc: doc,
                        base_version: Some(published.version),
                        source: Some(DraftSource::Ai),
                        ai_context: Some(serde_json::json!({ "channel": "axbuilder_mcp" })),
                    },
                )
                .await?
        };

        let issues = if draft.issues.is_empty() {
            computed_issues
        } else {
            draft.issues
        };
        let error_count = issues
            .iter()
            .filter(|i| i.severity == IssueSeverity::Error)
            .count();
        let warning_count = issues
            .iter()
            .filter(|i| i.severity == IssueSeverity::Warn)
            .count();

        self.emit_builder_mutate_audit(
            principal,
            BuilderMutateAudit {
                tool_name: "axbuilder_push_schema_draft",
                resource_ref: format!("xblue:draft:{}", draft.id),
                draft_id: Some(draft.id),
                summary: serde_json::json!({
                    "subgraph_slug": draft.subgraph_slug,
                    "title": draft.title,
                    "error_count": error_count,
                    "warning_count": warning_count,
                }),
                meta,
            },
        )
        .await;

        let deep_links =
            self.build_deep_links(principal, &draft.subgraph_slug, Some(draft.id), None, None);
        let resp = PushSchemaDraftResponse {
            draft_id: draft.id,
            subgraph_slug: draft.subgraph_slug,
            base_version: draft.base_version,
            status: String::from("DRAFT"),
            error_count,
            warning_count,
            issues,
            deep_links,
        };
        to_json_val(&resp, "PushSchemaDraftResponse")
    }

    async fn tool_clone_template(
        &self,
        principal: &AuthPrincipal,
        args: &serde_json::Value,
        meta: McpCallMeta<'_>,
    ) -> Result<serde_json::Value, AppError> {
        let pack_id = required_str_arg(args, "pack_id")?;
        let target_slug = optional_str_arg(args, "target_slug");
        let display_name = optional_str_arg(args, "display_name");
        let sb = self.template_sandbox.as_ref().ok_or_else(|| {
            AppError::Config(String::from("TemplateSandboxPort is not configured"))
        })?;

        let resp = sb
            .clone_template_sandbox(
                &principal.tenant_id,
                non_nil_uid(principal.user_id),
                pack_id,
                target_slug,
                display_name,
                self.effective_web_base(principal),
            )
            .await?;

        self.emit_builder_mutate_audit(
            principal,
            BuilderMutateAudit {
                tool_name: "axbuilder_clone_template",
                resource_ref: format!("xblue:subgraph:{}", resp.subgraph_slug),
                draft_id: Some(resp.draft_id),
                summary: serde_json::json!({
                    "pack_id": resp.source_template_id,
                    "subgraph_slug": resp.subgraph_slug,
                    "node_type_count": resp.node_type_count,
                    "edge_type_count": resp.edge_type_count,
                }),
                meta,
            },
        )
        .await;
        to_json_val(&resp, "CloneTemplateSandboxResponse")
    }

    async fn tool_upsert_flow_draft(
        &self,
        principal: &AuthPrincipal,
        args: &serde_json::Value,
        meta: McpCallMeta<'_>,
    ) -> Result<serde_json::Value, AppError> {
        let tenant_id = &principal.tenant_id;
        let parsed = parse_upsert_flow_args(args)?;

        if let Some(ref db_port) = self.builder_dashboard {
            let graph = FlowGraph {
                nodes: parsed.nodes.clone(),
                edges: parsed.edges.clone(),
            };
            let issues = db_port
                .validate_flow_draft(&graph, &parsed.trigger_cfg, parsed.slug.as_deref())
                .await?;
            let errs: Vec<&str> = issues
                .iter()
                .filter(|i| i.severity == FlowIssueSeverity::Error)
                .map(|i| i.message.as_str())
                .collect();
            if !errs.is_empty() {
                return Err(AppError::unprocessable("FLOW_INVALID", errs.join("; ")));
            }
        }

        let flow = if let Some(fid) = parsed.flow_id {
            self.store
                .update_flow(
                    tenant_id,
                    fid,
                    UpdateAxFlowRequest {
                        name: Some(parsed.name),
                        description: Some(parsed.description),
                        slug: parsed.slug,
                        nodes: Some(parsed.nodes),
                        edges: Some(parsed.edges),
                        trigger_cfg: Some(parsed.trigger_cfg),
                    },
                )
                .await?
                .ok_or(AppError::NotFound("FLOW_NOT_FOUND"))?
        } else {
            self.store
                .create_flow(
                    tenant_id,
                    non_nil_uid(principal.user_id),
                    CreateAxFlowRequest {
                        name: parsed.name,
                        description: parsed.description,
                        slug: parsed.slug,
                        nodes: parsed.nodes,
                        edges: parsed.edges,
                        trigger_cfg: parsed.trigger_cfg,
                        steps: None,
                    },
                )
                .await?
        };

        self.emit_builder_mutate_audit(
            principal,
            BuilderMutateAudit {
                tool_name: "axbuilder_upsert_flow_draft",
                resource_ref: format!("axflow:flow:{}", flow.id),
                draft_id: Some(flow.id),
                summary: serde_json::json!({
                    "flow_id": flow.id,
                    "name": flow.name,
                    "node_count": flow.nodes.len(),
                    "edge_count": flow.edges.len(),
                }),
                meta,
            },
        )
        .await;

        let resp = UpsertFlowDraftResponse {
            flow_id: flow.id,
            name: flow.name,
            status: String::from("DRAFT"),
            node_count: flow.nodes.len(),
            edge_count: flow.edges.len(),
            deep_links: self.build_deep_links(principal, "main", None, Some(flow.id), None),
        };
        to_json_val(&resp, "UpsertFlowDraftResponse")
    }

    async fn tool_run_dev_subgraph(
        &self,
        principal: &AuthPrincipal,
        args: &serde_json::Value,
        meta: McpCallMeta<'_>,
    ) -> Result<serde_json::Value, AppError> {
        let subgraph_slug = required_str_arg(args, "subgraph_slug")?;
        let draft_id = optional_uuid_arg(args, "draft_id")?;
        let limit_rows = optional_u32_arg(args, "limit_rows", 200, 5_000);
        let sb = self.template_sandbox.as_ref().ok_or_else(|| {
            AppError::Config(String::from("TemplateSandboxPort is not configured"))
        })?;

        let mut resp = sb
            .dry_run_dev_subgraph(
                &principal.tenant_id,
                subgraph_slug,
                DevSubgraphRunRequest {
                    draft_id,
                    limit_rows,
                },
                self.effective_web_base(principal),
            )
            .await?;

        if !principal.has(Permission::PiiRawRead) && !resp.sample_nodes.is_empty() {
            self.mask_sandbox_sample_nodes(
                &principal.tenant_id,
                subgraph_slug,
                draft_id,
                &mut resp.sample_nodes,
            )
            .await;
        }

        self.emit_builder_mutate_audit(
            principal,
            BuilderMutateAudit {
                tool_name: "axbuilder_run_dev_subgraph",
                resource_ref: format!("xblue:subgraph:{subgraph_slug}"),
                draft_id,
                summary: serde_json::json!({
                    "subgraph_slug": resp.subgraph_slug,
                    "nodes_upserted": resp.nodes_upserted,
                    "links_upserted": resp.links_upserted,
                    "points_computed": resp.points_computed,
                }),
                meta,
            },
        )
        .await;
        to_json_val(&resp, "DevSubgraphRunResponse")
    }

    async fn mask_sandbox_sample_nodes(
        &self,
        tenant_id: &str,
        subgraph_slug: &str,
        draft_id: Option<Uuid>,
        nodes: &mut [BlueNode],
    ) {
        let doc_opt = if let Some(did) = draft_id {
            self.store
                .get_draft(tenant_id, did)
                .await
                .ok()
                .flatten()
                .map(|d| d.schema_doc)
        } else if let Ok(drafts) = self
            .store
            .list_drafts(tenant_id, subgraph_slug, Some(DraftStatus::Draft))
            .await
            && let Some(latest) = drafts.into_iter().next()
        {
            Some(latest.schema_doc)
        } else {
            self.store
                .get_published_schema(tenant_id, subgraph_slug)
                .await
                .ok()
                .map(|p| p.doc)
        };

        let main_doc_opt = self
            .store
            .get_published_schema(tenant_id, "main")
            .await
            .ok()
            .map(|p| p.doc);
        let tenant_key = self.mask_keys.tenant_key(tenant_id);

        for node in nodes.iter_mut() {
            let mut masks = BTreeMap::new();
            if let Some(ref main_doc) = main_doc_opt
                && let Some(nt) = main_doc
                    .node_types
                    .iter()
                    .find(|n| n.name == node.type_name)
            {
                for prop in &nt.properties {
                    if prop.pii_category != PiiCategory::None {
                        masks.insert(
                            prop.name.clone(),
                            MaskRule::from_pii_category(prop.pii_category),
                        );
                    }
                }
            }
            if let Some(ref doc) = doc_opt
                && let Some(nt) = doc.node_types.iter().find(|n| n.name == node.type_name)
            {
                for prop in &nt.properties {
                    if prop.pii_category != PiiCategory::None {
                        masks.insert(
                            prop.name.clone(),
                            MaskRule::from_pii_category(prop.pii_category),
                        );
                    }
                }
            }
            if !masks.is_empty() {
                mask_json_properties(&mut node.properties, &masks, &tenant_key);
            }
        }
    }

    async fn tool_reset_dev_subgraph(
        &self,
        principal: &AuthPrincipal,
        args: &serde_json::Value,
        meta: McpCallMeta<'_>,
    ) -> Result<serde_json::Value, AppError> {
        let subgraph_slug = required_str_arg(args, "subgraph_slug")?;
        let resp = self
            .store
            .reset_dev_subgraph_nodes(
                &principal.tenant_id,
                subgraph_slug,
                self.effective_web_base(principal),
            )
            .await?;

        self.emit_builder_mutate_audit(
            principal,
            BuilderMutateAudit {
                tool_name: "axbuilder_reset_dev_subgraph",
                resource_ref: format!("xblue:subgraph:{subgraph_slug}"),
                draft_id: None,
                summary: serde_json::json!({
                    "subgraph_slug": resp.subgraph_slug,
                    "nodes_deleted": resp.nodes_deleted,
                    "links_deleted": resp.links_deleted,
                    "series_points_deleted": resp.series_points_deleted,
                }),
                meta,
            },
        )
        .await;
        to_json_val(&resp, "DevSubgraphResetResponse")
    }

    async fn tool_upsert_dashboard_draft(
        &self,
        principal: &AuthPrincipal,
        args: &serde_json::Value,
        meta: McpCallMeta<'_>,
    ) -> Result<serde_json::Value, AppError> {
        let input: UpsertDashboardDraftInput =
            serde_json::from_value(args.clone()).map_err(|err| {
                AppError::bad(
                    "SCHEMA_INVALID",
                    format!("invalid UpsertDashboardDraftInput: {err}"),
                )
            })?;
        let db_port = self.builder_dashboard.as_ref().ok_or_else(|| {
            AppError::Config(String::from("BuilderDashboardPort is not configured"))
        })?;

        let resp = db_port
            .upsert_dashboard_draft(
                &principal.tenant_id,
                non_nil_uid(principal.user_id),
                &principal.dept_id,
                input,
                self.effective_web_base(principal),
            )
            .await?;

        self.emit_builder_mutate_audit(
            principal,
            BuilderMutateAudit {
                tool_name: "axbuilder_upsert_dashboard_draft",
                resource_ref: format!("xblueview:dashboard:{}", resp.dashboard_id),
                draft_id: Some(resp.dashboard_id),
                summary: serde_json::json!({
                    "dashboard_id": resp.dashboard_id,
                    "slug": resp.slug,
                    "widget_count": resp.widget_count,
                }),
                meta,
            },
        )
        .await;
        to_json_val(&resp, "UpsertDashboardDraftResponse")
    }

    pub(crate) async fn append_axbuilder_resources(
        &self,
        tenant_id: &str,
        out: &mut Vec<McpResourceDescriptor>,
    ) -> Result<(), AppError> {
        if let Some(ref sb) = self.template_sandbox {
            for tpl in sb.list_template_packs().await? {
                out.push(McpResourceDescriptor {
                    uri: format!("axpoint://templates/{}", tpl.id),
                    name: tpl.name,
                    description: tpl.description,
                    mime_type: "application/json",
                });
            }
        }

        for sg in self.store.list_subgraphs(tenant_id).await? {
            out.push(McpResourceDescriptor {
                uri: format!("axpoint://subgraphs/{}", sg.slug),
                name: sg.display_name,
                description: sg.description,
                mime_type: "application/json",
            });
        }
        Ok(())
    }

    pub(crate) async fn try_read_axbuilder_resource(
        &self,
        tenant_id: &str,
        req_id: &serde_json::Value,
        uri: &str,
    ) -> Result<Option<serde_json::Value>, AppError> {
        if let Some(pack_id) = uri.strip_prefix("axpoint://templates/") {
            let sb = self
                .template_sandbox
                .as_ref()
                .ok_or(AppError::NotFound("TEMPLATE_NOT_FOUND"))?;
            let payload = sb.read_template_resource(pack_id).await?;
            let text = serde_json::to_string(&payload).unwrap_or_else(|_| String::from("{}"));
            return Ok(Some(jsonrpc_ok(
                req_id,
                &serde_json::json!({
                    "contents": [{ "uri": uri, "mimeType": "application/json", "text": text }]
                }),
            )));
        }

        if let Some(slug) = uri.strip_prefix("axpoint://subgraphs/") {
            let subgraphs = self.store.list_subgraphs(tenant_id).await?;
            let subgraph_info = subgraphs
                .into_iter()
                .find(|s| s.slug == slug)
                .ok_or(AppError::NotFound("SUBGRAPH_NOT_FOUND"))?;
            let published = self.store.get_published_schema(tenant_id, slug).await?;
            let drafts = self.store.list_drafts(tenant_id, slug, None).await?;
            let draft_summaries: Vec<serde_json::Value> = drafts
                .into_iter()
                .map(|d| {
                    serde_json::json!({
                        "id": d.id,
                        "title": d.title,
                        "status": d.status,
                        "base_version": d.base_version,
                        "source": d.source,
                        "updated_at": d.updated_at,
                    })
                })
                .collect();
            let nodes_page = self
                .store
                .list_nodes(
                    tenant_id,
                    NodeListQuery {
                        subgraph: Some(String::from(slug)),
                        limit: Some(10_000),
                        ..NodeListQuery::default()
                    },
                )
                .await?;
            let text = serde_json::to_string(&serde_json::json!({
                "subgraph_info": subgraph_info,
                "current_version": published.version,
                "published_schema_doc": published.doc,
                "draft_summaries": draft_summaries,
                "runtime_counts": {
                    "node_count": nodes_page.nodes.len(),
                    "link_count": nodes_page.links.len(),
                }
            }))
            .unwrap_or_else(|_| String::from("{}"));
            return Ok(Some(jsonrpc_ok(
                req_id,
                &serde_json::json!({
                    "contents": [{ "uri": uri, "mimeType": "application/json", "text": text }]
                }),
            )));
        }

        Ok(None)
    }
}

struct ParsedFlowDraftArgs {
    flow_id: Option<Uuid>,
    name: String,
    slug: Option<String>,
    description: String,
    nodes: Vec<FlowNode>,
    edges: Vec<FlowEdge>,
    trigger_cfg: TriggerCfg,
}

fn parse_upsert_flow_args(args: &serde_json::Value) -> Result<ParsedFlowDraftArgs, AppError> {
    let flow_id = optional_uuid_arg(args, "flow_id")?;
    let name = String::from(required_str_arg(args, "name")?);
    let slug = optional_str_arg(args, "slug").map(String::from);
    let description = optional_str_arg(args, "description")
        .unwrap_or("")
        .to_owned();
    let nodes: Vec<FlowNode> = serde_json::from_value(
        args.get("nodes")
            .cloned()
            .unwrap_or_else(|| serde_json::json!([])),
    )
    .map_err(|err| AppError::bad("SCHEMA_INVALID", format!("invalid flow nodes: {err}")))?;
    let edges: Vec<FlowEdge> = serde_json::from_value(
        args.get("edges")
            .cloned()
            .unwrap_or_else(|| serde_json::json!([])),
    )
    .map_err(|err| AppError::bad("SCHEMA_INVALID", format!("invalid flow edges: {err}")))?;
    let trigger_cfg: TriggerCfg = match args.get("trigger_cfg") {
        Some(v) => serde_json::from_value(v.clone()).map_err(|err| {
            AppError::bad("SCHEMA_INVALID", format!("invalid trigger_cfg: {err}"))
        })?,
        None => TriggerCfg::default(),
    };
    Ok(ParsedFlowDraftArgs {
        flow_id,
        name,
        slug,
        description,
        nodes,
        edges,
        trigger_cfg,
    })
}
