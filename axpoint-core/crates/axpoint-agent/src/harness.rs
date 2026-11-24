//! Step 29 `AxBuilder` fine-grained engineering tools (source profiling, FK candidate discovery,
//! deterministic expression evaluation, sandbox `ActionSpec` dry-run, `L0–L4` certification,
//! `AxQL` compilation, `AxSim` trial run, chart/dashboard preflight, and Competency QA harness,
//! `07-STEP29` §29.4).

use std::collections::BTreeMap;

use axpoint_domain::AppError;
use axpoint_domain::axhub::{Dataset, DatasetListQuery};
use axpoint_domain::builder::{
    BuilderDatasetSampleResponse, BuilderSimTrialRunInput, BuilderUpsertChartDraftInput,
    CompetencyQuestionSpec, UpsertCompetencySuiteRequest, VerifyCompetencySuiteRequest,
};
use axpoint_domain::enums::{DataType, MaskRule, Permission, PiiCategory};
use axpoint_domain::expr::{EvalEnv, collect_references, eval as eval_expr, parse as parse_expr};
use axpoint_domain::ports::{
    AgentRepository, AnalyticsRepository, DatasetRepository, FlowRepository, GraphQueryPort,
    GraphStorePort, StudioRepository,
};
use axpoint_domain::studio::{CertifySubgraphRequest, DraftStatus, diff_schema};
use axpoint_security::AuthPrincipal;
use axpoint_security::masking::mask_json_properties;
use serde::Serialize;
use uuid::Uuid;

use crate::mcp_builder::{
    BuilderMutateAudit, non_nil_uid, optional_slug_or_main, optional_str_arg, optional_uuid_arg,
    to_json_val,
};
use crate::mcp_server::{McpCallMeta, McpServerHub, optional_u32_arg, required_str_arg};
use crate::publish::to_analytics_principal;

/// Candidate foreign-key join suggestion discovered across tenant datasets (`axbuilder_inspect_dataset_profile`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DatasetFkCandidate {
    /// Column name on the inspected dataset.
    pub source_column: String,
    /// Target dataset machine name.
    pub target_dataset: String,
    /// Target column name (`primary_key` or matching key identifier).
    pub target_column: String,
    /// Deterministic heuristic reason (`"EXACT_PK_NAME"` or `"ENTITY_ID_STEM"`).
    pub match_reason: &'static str,
}

const fn types_join_compatible(a: DataType, b: DataType) -> bool {
    matches!(
        (a, b),
        (DataType::String, DataType::String)
            | (DataType::Int64, DataType::Int64)
            | (DataType::Decimal, DataType::Decimal)
    )
}

fn singular_stem(dataset_name: &str) -> &str {
    let tail = dataset_name
        .rsplit('_')
        .next()
        .unwrap_or(dataset_name)
        .trim();
    tail.strip_suffix('s')
        .filter(|s| s.len() >= 2)
        .unwrap_or(tail)
}

/// Deterministically discovers candidate FK relationships from `source` columns to other tenant datasets' primary keys.
#[must_use]
pub fn discover_fk_join_candidates(
    source: &Dataset,
    peer_datasets: &[Dataset],
) -> Vec<DatasetFkCandidate> {
    let mut out = Vec::new();
    for src_col in &source.schema_json {
        let col_low = src_col.name.to_ascii_lowercase();
        if !col_low.ends_with("_id")
            && !col_low.ends_with("_key")
            && !col_low.ends_with("_code")
            && !source.primary_key.contains(&src_col.name)
        {
            continue;
        }
        for peer in peer_datasets {
            if peer.id == source.id {
                continue;
            }
            let stem_id = format!("{}_id", singular_stem(&peer.name).to_ascii_lowercase());
            for pk_name in &peer.primary_key {
                let Some(peer_col) = peer.schema_json.iter().find(|c| c.name == *pk_name) else {
                    continue;
                };
                if !types_join_compatible(src_col.data_type, peer_col.data_type) {
                    continue;
                }
                let pk_low = pk_name.to_ascii_lowercase();
                let reason = if col_low == pk_low && col_low != "id" {
                    Some("EXACT_PK_NAME")
                } else if col_low == stem_id && (pk_low == "id" || pk_low == stem_id) {
                    Some("ENTITY_ID_STEM")
                } else {
                    None
                };
                if let Some(match_reason) = reason {
                    out.push(DatasetFkCandidate {
                        source_column: src_col.name.clone(),
                        target_dataset: peer.name.clone(),
                        target_column: pk_name.clone(),
                        match_reason,
                    });
                }
            }
        }
    }
    out
}

/// Returns the JSON Schema (`inputSchema`) for one of the 13 Step 29 `axbuilder_*` tools (`07-STEP29` §29.4).
#[must_use]
pub fn step29_axbuilder_tool_schema(name: &str) -> Option<serde_json::Value> {
    step29_data_and_model_schema(name).or_else(|| step29_sandbox_and_qa_schema(name))
}

fn step29_data_and_model_schema(name: &str) -> Option<serde_json::Value> {
    match name {
        "axbuilder_list_datasets" => Some(serde_json::json!({
            "type": "object",
            "properties": {
                "q": { "type": "string" },
                "tag": { "type": "string" },
                "limit": { "type": "integer", "minimum": 1, "maximum": 200 }
            },
            "additionalProperties": false
        })),
        "axbuilder_inspect_dataset_profile" => Some(serde_json::json!({
            "type": "object",
            "required": ["dataset"],
            "properties": {
                "dataset": { "type": "string", "minLength": 1 }
            },
            "additionalProperties": false
        })),
        "axbuilder_sample_dataset_rows" => Some(serde_json::json!({
            "type": "object",
            "required": ["dataset"],
            "properties": {
                "dataset": { "type": "string", "minLength": 1 },
                "limit": { "type": "integer", "minimum": 1, "maximum": 20 }
            },
            "additionalProperties": false
        })),
        "axbuilder_diff_schema_draft" => Some(serde_json::json!({
            "type": "object",
            "properties": {
                "subgraph_slug": { "type": "string", "minLength": 1 },
                "draft_id": { "type": "string", "format": "uuid" }
            },
            "additionalProperties": false
        })),
        "axbuilder_eval_expression" => Some(serde_json::json!({
            "type": "object",
            "required": ["expression"],
            "properties": {
                "expression": { "type": "string", "minLength": 1 },
                "metrics": { "type": "object" },
                "properties": { "type": "object" },
                "subgraph_slug": { "type": "string", "minLength": 1 },
                "node_id": { "type": "string", "format": "uuid" }
            },
            "additionalProperties": false
        })),
        "axbuilder_dryrun_action_spec" => Some(serde_json::json!({
            "type": "object",
            "required": ["subgraph_slug", "type_name", "node_key", "action_name"],
            "properties": {
                "subgraph_slug": { "type": "string", "minLength": 1 },
                "type_name": { "type": "string", "minLength": 1 },
                "node_key": { "type": "string", "minLength": 1 },
                "action_name": { "type": "string", "minLength": 1 },
                "inputs": { "type": "object" }
            },
            "additionalProperties": false
        })),
        _ => None,
    }
}

fn step29_sandbox_and_qa_schema(name: &str) -> Option<serde_json::Value> {
    match name {
        "axbuilder_certify_subgraph" => Some(serde_json::json!({
            "type": "object",
            "required": ["subgraph_slug"],
            "properties": {
                "subgraph_slug": { "type": "string", "minLength": 1 },
                "draft_id": { "type": "string", "format": "uuid" },
                "sample_limit": { "type": "integer", "minimum": 1, "maximum": 500 }
            },
            "additionalProperties": false
        })),
        "axbuilder_compile_axql" => Some(serde_json::json!({
            "type": "object",
            "required": ["axql"],
            "properties": {
                "axql": { "type": "string", "minLength": 1 },
                "subgraph_slug": { "type": "string", "minLength": 1 }
            },
            "additionalProperties": false
        })),
        "axbuilder_sim_trial_run" => Some(serde_json::json!({
            "type": "object",
            "required": ["subgraph_slug", "actions"],
            "properties": {
                "subgraph_slug": { "type": "string", "minLength": 1 },
                "actions": { "type": "array", "items": { "type": "object" } },
                "params": { "type": "object" }
            },
            "additionalProperties": false
        })),
        "axbuilder_upsert_chart_draft" => Some(serde_json::json!({
            "type": "object",
            "required": ["name", "widget_kind", "data_source"],
            "properties": {
                "chart_id": { "type": "string", "format": "uuid" },
                "name": { "type": "string", "minLength": 1 },
                "description": { "type": "string" },
                "widget_kind": { "type": "string", "minLength": 1 },
                "data_source": { "type": "object" },
                "encoding": { "type": "object" },
                "options": { "type": "object" },
                "sensitivity": { "type": "string", "enum": ["PUBLIC", "INTERNAL", "CONFIDENTIAL", "RESTRICTED"] }
            },
            "additionalProperties": false
        })),
        "axbuilder_preflight_dashboard" => Some(serde_json::json!({
            "type": "object",
            "required": ["dashboard_id"],
            "properties": {
                "dashboard_id": { "type": "string", "format": "uuid" }
            },
            "additionalProperties": false
        })),
        "axbuilder_upsert_competency_suite" => Some(serde_json::json!({
            "type": "object",
            "required": ["questions"],
            "properties": {
                "subgraph_slug": { "type": "string", "minLength": 1 },
                "questions": { "type": "array", "items": { "type": "object" } }
            },
            "additionalProperties": false
        })),
        "axbuilder_verify_competency_suite" => Some(serde_json::json!({
            "type": "object",
            "properties": {
                "subgraph_slug": { "type": "string", "minLength": 1 },
                "draft_id": { "type": "string", "format": "uuid" },
                "run_dev_materialize": { "type": "boolean" },
                "limit_rows": { "type": "integer", "minimum": 1, "maximum": 5000 },
                "questions_override": { "type": "array", "items": { "type": "object" } }
            },
            "additionalProperties": false
        })),
        _ => None,
    }
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
    pub(crate) async fn execute_step29_axbuilder_tool(
        &self,
        principal: &AuthPrincipal,
        tool_name: &str,
        args: &serde_json::Value,
        meta: McpCallMeta<'_>,
    ) -> Result<serde_json::Value, AppError> {
        match tool_name {
            "axbuilder_list_datasets" => self.tool_list_datasets_builder(principal, args).await,
            "axbuilder_inspect_dataset_profile" => {
                self.tool_inspect_dataset_profile(principal, args).await
            }
            "axbuilder_sample_dataset_rows" => self.tool_sample_dataset_rows(principal, args).await,
            "axbuilder_diff_schema_draft" => self.tool_diff_schema_draft(principal, args).await,
            "axbuilder_eval_expression" => self.tool_eval_expression(principal, args).await,
            "axbuilder_dryrun_action_spec" => self.tool_dryrun_action_spec(principal, args).await,
            "axbuilder_certify_subgraph" => self.tool_certify_subgraph(principal, args, meta).await,
            "axbuilder_compile_axql" => self.tool_compile_axql(principal, args).await,
            "axbuilder_sim_trial_run" => self.tool_sim_trial_run(principal, args).await,
            "axbuilder_upsert_chart_draft" => {
                self.tool_upsert_chart_draft(principal, args, meta).await
            }
            "axbuilder_preflight_dashboard" => self.tool_preflight_dashboard(principal, args).await,
            "axbuilder_upsert_competency_suite" => {
                self.tool_upsert_competency_suite(principal, args, meta)
                    .await
            }
            "axbuilder_verify_competency_suite" => {
                self.tool_verify_competency_suite(principal, args).await
            }
            _ => Err(AppError::bad(
                "INVALID_ARGUMENT",
                format!("unknown AxBuilder MCP tool: {tool_name}"),
            )),
        }
    }

    async fn resolve_dataset_by_ref(
        &self,
        tenant_id: &str,
        ds_ref: &str,
    ) -> Result<Dataset, AppError> {
        let found = if let Ok(ds_id) = Uuid::parse_str(ds_ref) {
            self.store.get_dataset(tenant_id, ds_id).await?
        } else {
            self.store.get_dataset_by_name(tenant_id, ds_ref).await?
        };
        found.ok_or(AppError::NotFound("DATASET_NOT_FOUND"))
    }

    async fn tool_list_datasets_builder(
        &self,
        principal: &AuthPrincipal,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, AppError> {
        let limit = optional_u32_arg(args, "limit", 50, 200);
        let page = self
            .store
            .list_datasets(
                &principal.tenant_id,
                DatasetListQuery {
                    q: optional_str_arg(args, "q").map(String::from),
                    tag: optional_str_arg(args, "tag").map(String::from),
                    source_kind: None,
                    limit: Some(limit),
                    cursor: None,
                },
            )
            .await?;

        let items: Vec<serde_json::Value> = page
            .items
            .into_iter()
            .map(|ds| {
                let pii_cols: Vec<String> = ds
                    .schema_json
                    .iter()
                    .filter(|c| c.pii_category != PiiCategory::None)
                    .map(|c| c.name.clone())
                    .collect();
                serde_json::json!({
                    "id": ds.id,
                    "name": ds.name,
                    "display_name": ds.display_name,
                    "source_kind": ds.source_kind,
                    "sync_mode": ds.sync_mode,
                    "continuous_stream": ds.continuous_stream,
                    "primary_key": ds.primary_key,
                    "row_count": ds.row_count,
                    "column_count": ds.schema_json.len(),
                    "pii_columns": pii_cols,
                })
            })
            .collect();

        Ok(serde_json::json!({
            "items": items,
            "next_cursor": page.next_cursor,
            "deep_links": self.build_deep_links(principal, "main", None, None, None),
        }))
    }

    async fn tool_inspect_dataset_profile(
        &self,
        principal: &AuthPrincipal,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, AppError> {
        let tenant_id = &principal.tenant_id;
        let ds_ref = required_str_arg(args, "dataset")?;
        let ds = self.resolve_dataset_by_ref(tenant_id, ds_ref).await?;
        let col_metas = self.store.list_columns(tenant_id, ds.id).await?;
        let meta_by_name: BTreeMap<String, _> = col_metas
            .into_iter()
            .map(|m| (m.column_name.clone(), m))
            .collect();

        let columns: Vec<serde_json::Value> = ds
            .schema_json
            .iter()
            .map(|c| {
                let meta = meta_by_name.get(&c.name);
                let pii = meta.map_or(c.pii_category, |m| m.pii_category);
                serde_json::json!({
                    "name": c.name,
                    "data_type": c.data_type,
                    "nullable": c.nullable,
                    "null_ratio": c.null_ratio,
                    "distinct_estimate": c.distinct_estimate,
                    "pii_category": pii,
                    "business_term": meta.map_or("", |m| m.business_term.as_str()),
                    "unit": meta.map_or("", |m| m.unit.as_str()),
                })
            })
            .collect();

        let peers = self
            .store
            .list_datasets(
                tenant_id,
                DatasetListQuery {
                    limit: Some(200),
                    ..DatasetListQuery::default()
                },
            )
            .await?;
        let fk_candidates = discover_fk_join_candidates(&ds, &peers.items);

        Ok(serde_json::json!({
            "dataset_id": ds.id,
            "name": ds.name,
            "display_name": ds.display_name,
            "source_kind": ds.source_kind,
            "sync_mode": ds.sync_mode,
            "continuous_stream": ds.continuous_stream,
            "primary_key": ds.primary_key,
            "row_count": ds.row_count,
            "columns": columns,
            "fk_candidates": fk_candidates,
            "deep_links": self.build_deep_links(principal, "main", None, None, None),
        }))
    }

    async fn tool_sample_dataset_rows(
        &self,
        principal: &AuthPrincipal,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, AppError> {
        let tenant_id = &principal.tenant_id;
        let ds_ref = required_str_arg(args, "dataset")?;
        let ds = self.resolve_dataset_by_ref(tenant_id, ds_ref).await?;
        let limit = optional_u32_arg(args, "limit", 10, 20);

        if let Some(ref harness) = self.builder_harness {
            let resp = harness
                .sample_masked_dataset_rows(
                    tenant_id,
                    ds.id,
                    limit,
                    self.effective_web_base(principal),
                )
                .await?;
            return to_json_val(&resp, "BuilderDatasetSampleResponse");
        }

        let columns: Vec<serde_json::Value> = ds
            .schema_json
            .iter()
            .map(|c| {
                serde_json::json!({
                    "name": c.name,
                    "data_type": c.data_type,
                    "pii_category": c.pii_category,
                })
            })
            .collect();
        let fallback = BuilderDatasetSampleResponse {
            dataset_id: ds.id,
            dataset_name: ds.name,
            columns,
            rows: Vec::new(),
            deep_links: self.build_deep_links(principal, "main", None, None, None),
        };
        to_json_val(&fallback, "BuilderDatasetSampleResponse")
    }

    async fn tool_diff_schema_draft(
        &self,
        principal: &AuthPrincipal,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, AppError> {
        let tenant_id = &principal.tenant_id;
        let slug = optional_slug_or_main(args);
        let maybe_draft_id = optional_uuid_arg(args, "draft_id")?;
        let published = self.store.get_published_schema(tenant_id, slug).await?;
        let draft = if let Some(did) = maybe_draft_id {
            self.store
                .get_draft(tenant_id, did)
                .await?
                .ok_or(AppError::NotFound("DRAFT_NOT_FOUND"))?
        } else {
            self.store
                .list_drafts(tenant_id, slug, Some(DraftStatus::Draft))
                .await?
                .into_iter()
                .next()
                .ok_or(AppError::NotFound("DRAFT_NOT_FOUND"))?
        };

        let diff = diff_schema(&published.doc, &draft.schema_doc);
        Ok(serde_json::json!({
            "subgraph_slug": slug,
            "published_version": published.version,
            "draft_id": draft.id,
            "diff": diff,
            "deep_links": self.build_deep_links(principal, slug, Some(draft.id), None, None),
        }))
    }

    async fn tool_eval_expression(
        &self,
        principal: &AuthPrincipal,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, AppError> {
        let expr_str = required_str_arg(args, "expression")?;
        let ast = match parse_expr(expr_str) {
            Ok(parsed) => parsed,
            Err(issue) => {
                return Ok(serde_json::json!({
                    "expression": expr_str,
                    "valid": false,
                    "issue": issue,
                    "value": serde_json::Value::Null,
                    "value_type": "NULL",
                    "warnings": [],
                }));
            }
        };

        let (metric_refs, prop_refs) = collect_references(&ast);
        let env = self.build_expr_eval_env(principal, args).await?;
        let report = eval_expr(&ast, &env);

        Ok(serde_json::json!({
            "expression": expr_str,
            "valid": true,
            "referenced_metrics": metric_refs,
            "referenced_properties": prop_refs,
            "value": report.value.to_json_value(),
            "value_type": report.value.expr_type(),
            "warnings": report.warnings,
        }))
    }

    async fn build_expr_eval_env(
        &self,
        principal: &AuthPrincipal,
        args: &serde_json::Value,
    ) -> Result<EvalEnv, AppError> {
        if let Some(nid) = optional_uuid_arg(args, "node_id")? {
            let mut node = self
                .store
                .get_node(&principal.tenant_id, nid)
                .await?
                .ok_or(AppError::NotFound("NODE_NOT_FOUND"))?;
            if !principal.has(Permission::PiiRawRead)
                && let Ok(pub_schema) = self
                    .store
                    .get_published_schema(&principal.tenant_id, optional_slug_or_main(args))
                    .await
                && let Some(nt) = pub_schema
                    .doc
                    .node_types
                    .iter()
                    .find(|n| n.name == node.type_name)
            {
                let mut masks = BTreeMap::new();
                for p in &nt.properties {
                    if p.pii_category != PiiCategory::None {
                        masks.insert(p.name.clone(), MaskRule::from_pii_category(p.pii_category));
                    }
                }
                if !masks.is_empty() {
                    let key = self.mask_keys.tenant_key(&principal.tenant_id);
                    mask_json_properties(&mut node.properties, &masks, &key);
                }
            }
            return Ok(EvalEnv::from_node(&node.metrics, &node.properties));
        }

        let mut metrics = BTreeMap::new();
        if let Some(obj) = args.get("metrics").and_then(serde_json::Value::as_object) {
            for (k, v) in obj {
                if let Some(num) = v.as_f64().filter(|f| f.is_finite()) {
                    metrics.insert(k.clone(), num);
                }
            }
        }
        let empty_props = serde_json::Map::new();
        let props = args
            .get("properties")
            .and_then(serde_json::Value::as_object)
            .unwrap_or(&empty_props);
        Ok(EvalEnv::from_node(&metrics, props))
    }

    async fn tool_dryrun_action_spec(
        &self,
        principal: &AuthPrincipal,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, AppError> {
        let slug = required_str_arg(args, "subgraph_slug")?;
        if slug == "main" {
            return Err(AppError::bad(
                "DEV_SANDBOX_REQUIRED",
                "axbuilder_dryrun_action_spec requires a non-main dev/trial subgraph",
            ));
        }
        let type_name = required_str_arg(args, "type_name")?;
        let node_key = required_str_arg(args, "node_key")?;
        let action_name = required_str_arg(args, "action_name")?;
        let inputs = args
            .get("inputs")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));
        let harness = self.builder_harness.as_ref().ok_or_else(|| {
            AppError::Config(String::from("BuilderHarnessPort is not configured"))
        })?;

        let resp = harness
            .dryrun_dev_action_spec(
                &principal.tenant_id,
                non_nil_uid(principal.user_id),
                slug,
                (type_name, node_key, action_name),
                inputs,
            )
            .await?;
        to_json_val(&resp, "InvokeObjectActionResponse")
    }

    async fn tool_certify_subgraph(
        &self,
        principal: &AuthPrincipal,
        args: &serde_json::Value,
        meta: McpCallMeta<'_>,
    ) -> Result<serde_json::Value, AppError> {
        let slug = required_str_arg(args, "subgraph_slug")?;
        let draft_id = optional_uuid_arg(args, "draft_id")?;
        let sample_limit = args
            .get("sample_limit")
            .and_then(serde_json::Value::as_u64)
            .and_then(|v| u32::try_from(v).ok())
            .map(|v| v.clamp(1, 500));
        let req = CertifySubgraphRequest {
            draft_id,
            sample_limit,
        };

        let report = if let Some(ref harness) = self.builder_harness {
            harness
                .certify_builder_subgraph(
                    &principal.tenant_id,
                    non_nil_uid(principal.user_id),
                    slug,
                    req,
                )
                .await?
        } else {
            self.store
                .certify_subgraph_runtime(
                    &principal.tenant_id,
                    slug,
                    non_nil_uid(principal.user_id),
                    req,
                )
                .await?
        };

        self.emit_builder_mutate_audit(
            principal,
            BuilderMutateAudit {
                tool_name: "axbuilder_certify_subgraph",
                resource_ref: format!("xblue:subgraph:{slug}"),
                draft_id,
                summary: serde_json::json!({
                    "cert_level": report.cert_level,
                    "readiness_score": report.readiness_score,
                    "trial_tick_passed": report.trial_tick_passed,
                }),
                meta,
            },
        )
        .await;

        Ok(serde_json::json!({
            "report": report,
            "deep_links": self.build_deep_links(principal, slug, draft_id, None, None),
        }))
    }

    async fn tool_compile_axql(
        &self,
        principal: &AuthPrincipal,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, AppError> {
        let axql = required_str_arg(args, "axql")?;
        let slug = optional_slug_or_main(args);
        let harness = self.builder_harness.as_ref().ok_or_else(|| {
            AppError::Config(String::from("BuilderHarnessPort is not configured"))
        })?;
        let plan = harness
            .compile_builder_axql(&principal.tenant_id, slug, axql)
            .await?;
        to_json_val(&plan, "BuilderAxqlPlan")
    }

    async fn tool_sim_trial_run(
        &self,
        principal: &AuthPrincipal,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, AppError> {
        let input: BuilderSimTrialRunInput = serde_json::from_value(args.clone()).map_err(|e| {
            AppError::bad(
                "SCHEMA_INVALID",
                format!("invalid BuilderSimTrialRunInput: {e}"),
            )
        })?;
        if input.subgraph_slug.trim() == "main" {
            return Err(AppError::bad(
                "DEV_SANDBOX_REQUIRED",
                "axbuilder_sim_trial_run requires a non-main dev/trial subgraph",
            ));
        }
        let harness = self.builder_harness.as_ref().ok_or_else(|| {
            AppError::Config(String::from("BuilderHarnessPort is not configured"))
        })?;
        let resp = harness
            .sim_trial_dev_subgraph(
                &principal.tenant_id,
                input,
                self.effective_web_base(principal),
            )
            .await?;
        to_json_val(&resp, "BuilderSimTrialRunResponse")
    }

    async fn tool_upsert_chart_draft(
        &self,
        principal: &AuthPrincipal,
        args: &serde_json::Value,
        meta: McpCallMeta<'_>,
    ) -> Result<serde_json::Value, AppError> {
        let input: BuilderUpsertChartDraftInput =
            serde_json::from_value(args.clone()).map_err(|err| {
                AppError::bad(
                    "SCHEMA_INVALID",
                    format!("invalid BuilderUpsertChartDraftInput: {err}"),
                )
            })?;
        let harness = self.builder_harness.as_ref().ok_or_else(|| {
            AppError::Config(String::from("BuilderHarnessPort is not configured"))
        })?;
        let resp = harness
            .upsert_chart_draft(
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
                tool_name: "axbuilder_upsert_chart_draft",
                resource_ref: format!("xblueview:chart:{}", resp.chart_id),
                draft_id: Some(resp.chart_id),
                summary: serde_json::json!({
                    "chart_id": resp.chart_id,
                    "name": resp.name,
                    "widget_kind": resp.widget_kind,
                }),
                meta,
            },
        )
        .await;
        to_json_val(&resp, "BuilderUpsertChartDraftResponse")
    }

    async fn tool_preflight_dashboard(
        &self,
        principal: &AuthPrincipal,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, AppError> {
        let raw_id = required_str_arg(args, "dashboard_id")?;
        let dashboard_id = Uuid::parse_str(raw_id)
            .map_err(|e| AppError::bad("SCHEMA_INVALID", format!("invalid dashboard_id: {e}")))?;
        let harness = self.builder_harness.as_ref().ok_or_else(|| {
            AppError::Config(String::from("BuilderHarnessPort is not configured"))
        })?;
        let report = harness
            .preflight_dashboard_draft(&principal.tenant_id, dashboard_id)
            .await?;
        to_json_val(&report, "BuilderDashboardPreflight")
    }

    async fn tool_upsert_competency_suite(
        &self,
        principal: &AuthPrincipal,
        args: &serde_json::Value,
        meta: McpCallMeta<'_>,
    ) -> Result<serde_json::Value, AppError> {
        let slug = optional_slug_or_main(args);
        let raw_qs = args
            .get("questions")
            .cloned()
            .unwrap_or_else(|| serde_json::json!([]));
        let questions: Vec<CompetencyQuestionSpec> =
            serde_json::from_value(raw_qs).map_err(|err| {
                AppError::bad(
                    "SCHEMA_INVALID",
                    format!("invalid competency questions: {err}"),
                )
            })?;
        let q_count = questions.len();

        let suite = self
            .store
            .upsert_competency_suite(
                &principal.tenant_id,
                slug,
                non_nil_uid(principal.user_id),
                UpsertCompetencySuiteRequest { questions },
            )
            .await?;

        self.emit_builder_mutate_audit(
            principal,
            BuilderMutateAudit {
                tool_name: "axbuilder_upsert_competency_suite",
                resource_ref: format!("xblue:competency_suite:{slug}"),
                draft_id: None,
                summary: serde_json::json!({
                    "subgraph_slug": slug,
                    "question_count": q_count,
                }),
                meta,
            },
        )
        .await;

        Ok(serde_json::json!({
            "suite": suite,
            "deep_links": self.build_deep_links(principal, slug, None, None, None),
        }))
    }

    async fn tool_verify_competency_suite(
        &self,
        principal: &AuthPrincipal,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, AppError> {
        let slug = optional_slug_or_main(args);
        let draft_id = optional_uuid_arg(args, "draft_id")?;
        let run_dev_materialize = args
            .get("run_dev_materialize")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true);
        let limit_rows = args
            .get("limit_rows")
            .and_then(serde_json::Value::as_u64)
            .and_then(|v| u32::try_from(v).ok())
            .map(|v| v.clamp(1, 5_000));
        let questions_override = match args.get("questions_override") {
            Some(v) if !v.is_null() => {
                let parsed: Vec<CompetencyQuestionSpec> = serde_json::from_value(v.clone())
                    .map_err(|err| {
                        AppError::bad(
                            "SCHEMA_INVALID",
                            format!("invalid questions_override: {err}"),
                        )
                    })?;
                Some(parsed)
            }
            _ => None,
        };

        let mut an_principal = to_analytics_principal(principal);
        an_principal.permissions.remove(&Permission::PiiRawRead);
        let harness = self.builder_harness.as_ref().ok_or_else(|| {
            AppError::Config(String::from("BuilderHarnessPort is not configured"))
        })?;
        let record = harness
            .verify_competency_suite(
                &an_principal,
                non_nil_uid(principal.user_id),
                &principal.username,
                slug,
                VerifyCompetencySuiteRequest {
                    draft_id,
                    run_dev_materialize,
                    limit_rows,
                    questions_override,
                },
            )
            .await?;
        to_json_val(&record, "HarnessRunRecord")
    }
}
