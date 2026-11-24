//! `xBlueStudio` modeling, versioned schema, drafts, subgraphs, dataset mappings, dynamic GraphQL,
//! domain objects, and `AxGraph` runtime topology routes (`/api/xblue/*`).

#[path = "xblue_graphql.rs"]
pub(crate) mod xblue_graphql;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::str::FromStr;

use axpoint_agent::publish::{build_metric_schema, build_object_type_schema};
use axpoint_db::studio_repo::NeighborBatchKey;
use axpoint_domain::AppError;
use axpoint_domain::agent::{
    AgentSpec, CreateAgentRequest, EndpointSpec, KnowledgeScope, SaveEndpointInput, ToolRef,
    UpdateAgentRequest,
};
use axpoint_domain::analytics::MetricDef;
use axpoint_domain::axhub::{Dataset, PaginatedList};
use axpoint_domain::builder::{
    DevSubgraphResetResponse, DevSubgraphRunRequest, DevSubgraphRunResponse,
};
use axpoint_domain::enums::{
    AuditAction, CopilotActionMode, DataType, EndpointKind, Environment, GraphAlgo, JobKind,
    OutputShape, Permission,
};
use axpoint_domain::expr::{
    EvalEnv, EvaluateExprRequest, EvaluateExprResponse, ValidateExprRequest, ValidateExprResponse,
    eval as eval_expr, parse as parse_expr, validate_for_node_type,
};
use axpoint_domain::graph::{
    AlgoResultsQuery, AlgoResultsResponse, AlgoRunRequest, AlgoRunResponse, BulkNodesRequest,
    BulkNodesResponse, EntityCandidatesResponse, EntityLink, LinkListQuery, LinkRecord,
    MetricCatalogEntry, MetricSeriesPoint, NeighborQuery, NodeDocumentsResponse, NodeListQuery,
    NodeListResponse, NodeMetricSeriesQuery, NodeSearchQuery, NodeSearchResponse, PatchNodeRequest,
    PathQuery, PathResponse, ResolveEntitiesRequest, ResolveEntitiesResponse, SubgraphQuery,
    SubgraphResponse, UpsertLinkRequest, WindowQuery, WindowQueryResponse, WriteLinkMetricsRequest,
    WriteLinkMetricsResponse, WriteMetricsRequest, WriteMetricsResponse,
};
use axpoint_domain::platform::{AuditRecord, NewJob};
use axpoint_domain::ports::{
    AgentRepository as _, AnalyticsRepository as _, DatasetRepository as _, GraphQueryPort as _,
    GraphStorePort as _, JobQueuePort as _, MetricStorePort as _, OnboardingAndPackagingPort as _,
    StudioExecutionPort as _, StudioRepository as _,
};
use axpoint_domain::rollup::{
    AcceptNodeCandidateRequest, AcceptNodeCandidateResponse, GraphAggregateJobPayload,
    KnowledgeNodeCandidatesResponse, KnowledgeSyncJobPayload, LinkMetricExplainResponse,
    MetricExplainResponse, RecomputeAggregateRequest, RecomputeAggregateResponse,
};
use axpoint_domain::studio::{
    AiDraftRequest, CertifySubgraphRequest, ColumnProfileStats, ColumnRoleTag,
    ConfirmProposalsRequest, CreateDraftRequest, CreateSubgraphRequest, DatasetProfileSummary,
    DomainObjectItemQuery, DomainObjectListQuery, DomainObjectListResponse, DomainObjectRecord,
    DraftSnapshotListResponse, DraftSource, DraftStatus, InvokeObjectActionRequest,
    InvokeObjectActionResponse, IssueSeverity, MappingPreviewResponse, MappingRecord,
    MappingRunsResponse, MergePreview, MergePreviewRequest, MergeSubgraphRequest, NodeLineageGraph,
    NodeTypeSpec, ObjectCatalogResponse, ObjectFieldSig, ObjectTypeCatalogItem,
    PackageSubgraphAgentRequest, PackageSubgraphAgentResponse, PackagedEndpointItem,
    ProfilingBundle, PropertyKind, PublishDraftRequest, PublishDraftResponse,
    PublishedSchemaResponse, RuntimeCertReport, SaveSchemaDirectRequest, SchemaDiff, SchemaDoc,
    SchemaDraft, SchemaVersionDetail, SchemaVersionSummary, SpecDocCompileInput,
    SpecMappingSyncMeta, StudioCopilotRequest, StudioCopilotResponse, SubgraphConsumptionResponse,
    SubgraphInfo, SubgraphQueryParam, TrialCloneTemplateRequest, TrialCloneTemplateResponse,
    TriggerMappingRunRequest, TriggerMappingRunResponse, UpdateDraftRequest, UpdateSubgraphRequest,
    ValidateSchemaResponse, build_node_lineage, build_packaged_endpoint_slug,
    compile_subgraph_spec_doc, validate_with_metrics,
};
use axpoint_domain::topology::{BlueNode, UpsertBlueNodeRequest};
use axpoint_llm::{
    CopilotDatasetProfile, CopilotPromptInput, SchemaDraftInput, generate_schema_draft,
    run_copilot_diagnose, run_copilot_patch,
};
use axpoint_security::AuthPrincipal;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{StatusCode, header::CONTENT_TYPE};
use axum::response::{IntoResponse as _, Response};
use axum::routing::{delete, get, patch, post};
use axum::{Json, Router};
use chrono::Utc;
use serde::Deserialize;
use uuid::Uuid;

use crate::routes::analytics::{acquire_tenant_session, refresh_cached_tenant_catalog};
use crate::state::{ApiJson, ApiPath, ApiQuery, AppState, HttpError};
use crate::telemetry::current_trace_id;

/// Builds the `/xblue/*` router mounted under `/api`.
pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/xblue/subgraphs",
            get(list_subgraphs_handler).post(create_subgraph_handler),
        )
        .route(
            "/xblue/subgraphs/from-template",
            post(clone_subgraph_from_template_handler),
        )
        .route(
            "/xblue/subgraphs/{slug}",
            patch(update_subgraph_handler).delete(delete_subgraph_handler),
        )
        .route(
            "/xblue/subgraphs/{slug}/package-agent",
            post(package_subgraph_agent_handler),
        )
        .route(
            "/xblue/subgraphs/{slug}/consumption",
            get(get_subgraph_consumption_handler),
        )
        .route(
            "/xblue/subgraphs/{slug}/merge-preview",
            post(preview_subgraph_merge_handler),
        )
        .route(
            "/xblue/subgraphs/{slug}/merge",
            post(merge_subgraph_handler),
        )
        .route(
            "/xblue/subgraphs/{slug}/dry-run-materialize",
            post(dry_run_dev_subgraph_handler),
        )
        .route(
            "/xblue/subgraphs/{slug}/reset-nodes",
            post(reset_dev_subgraph_nodes_handler),
        )
        .route(
            "/xblue/subgraphs/{slug}/lineage/{node_type}",
            get(get_node_lineage_handler),
        )
        .route(
            "/xblue/subgraphs/{slug}/spec-doc",
            get(get_subgraph_spec_doc_handler),
        )
        .route(
            "/xblue/subgraphs/{slug}/certify",
            post(certify_subgraph_handler),
        )
        .route(
            "/xblue/subgraphs/{slug}/competency-suite",
            get(get_competency_suite_handler).put(put_competency_suite_handler),
        )
        .route(
            "/xblue/subgraphs/{slug}/competency-verify",
            post(verify_competency_suite_handler),
        )
        .route(
            "/xblue/subgraphs/{slug}/harness-runs",
            get(list_harness_runs_handler),
        )
        .route(
            "/xblue/schema",
            get(get_schema_handler)
                .post(save_schema_handler)
                .put(save_schema_handler),
        )
        .route("/xblue/schema/validate", post(validate_schema_handler))
        .route("/xblue/schema/versions", get(list_schema_versions_handler))
        .route(
            "/xblue/schema/versions/{v}",
            get(get_schema_version_handler),
        )
        .route(
            "/xblue/schema/versions/{v}/diff/{to}",
            get(diff_schema_versions_handler),
        )
        .route(
            "/xblue/schema/versions/{v}/restore",
            post(restore_schema_version_handler),
        )
        .route(
            "/xblue/drafts",
            get(list_drafts_handler).post(create_draft_handler),
        )
        .route("/xblue/drafts/ai", post(create_ai_draft_handler))
        .route(
            "/xblue/drafts/{id}",
            get(get_draft_handler).put(update_draft_handler),
        )
        .route("/xblue/drafts/{id}/discard", post(discard_draft_handler))
        .route("/xblue/drafts/{id}/publish", post(publish_draft_handler))
        .merge(studio_copilot_and_objects_router())
        .merge(runtime_graph_router())
}

fn studio_copilot_and_objects_router() -> Router<AppState> {
    Router::new()
        .route("/xblue/drafts/{id}/copilot", post(draft_copilot_handler))
        .route(
            "/xblue/drafts/{id}/snapshots",
            get(list_draft_snapshots_handler),
        )
        .route(
            "/xblue/drafts/{id}/snapshots/{snap_id}/restore",
            post(restore_draft_snapshot_handler),
        )
        .route(
            "/xblue/drafts/{id}/confirm-proposals",
            post(confirm_draft_proposals_handler),
        )
        .route("/xblue/expr/validate", post(validate_expr_handler))
        .route("/xblue/expr/evaluate", post(evaluate_expr_handler))
        .route(
            "/xblue/aggregate/recompute",
            post(recompute_aggregate_handler),
        )
        .route("/xblue/mappings", get(list_mappings_handler))
        .route(
            "/xblue/mappings/{id}/preview",
            post(preview_mapping_handler),
        )
        .route(
            "/xblue/mappings/{id}/run",
            post(trigger_mapping_run_handler),
        )
        .route("/xblue/mappings/{id}/runs", get(list_mapping_runs_handler))
        .route("/xblue/objects", get(list_object_catalog_handler))
        .route(
            "/xblue/objects/{type_name}",
            get(list_domain_objects_handler),
        )
        .route(
            "/xblue/objects/{type_name}/{node_key}",
            get(get_domain_object_handler),
        )
        .route(
            "/xblue/objects/{type_name}/{node_key}/actions/{action_name}",
            post(invoke_object_action_handler),
        )
        .route("/xblue/graphql", post(xblue_graphql::graphql_handler))
        .route(
            "/xblue/graphql/sdl",
            get(xblue_graphql::graphql_sdl_handler),
        )
}

fn runtime_graph_router() -> Router<AppState> {
    Router::new()
        .route(
            "/xblue/nodes",
            get(list_nodes_handler).post(upsert_node_handler),
        )
        .route("/xblue/nodes/bulk", post(bulk_upsert_nodes_handler))
        .route("/xblue/nodes/search", get(search_nodes_handler))
        .route(
            "/xblue/nodes/{id}",
            get(get_node_handler)
                .patch(patch_node_handler)
                .delete(delete_node_handler),
        )
        .route(
            "/xblue/nodes/{id}/documents",
            get(list_node_documents_handler),
        )
        .route(
            "/xblue/nodes/{id}/metrics/{metric}",
            get(query_node_metrics_handler),
        )
        .route(
            "/xblue/links",
            get(list_links_handler).post(upsert_link_handler),
        )
        .route("/xblue/links/{id}", delete(delete_link_handler))
        .route(
            "/xblue/links/{id}/explain/{metric}",
            get(explain_link_metric_handler),
        )
        .route(
            "/xblue/links/metrics/write",
            post(write_link_metrics_handler),
        )
        .route("/xblue/metrics/write", post(write_metrics_handler))
        .route("/xblue/metrics/window", post(query_window_handler))
        .route("/xblue/metrics/catalog", get(list_metric_catalog_handler))
        .route("/xblue/graph/neighbors", get(query_neighbors_handler))
        .route("/xblue/graph/path", get(query_path_handler))
        .route("/xblue/graph/subgraph", get(query_subgraph_handler))
        .route(
            "/xblue/graph/algorithms/{algo}",
            post(run_algorithm_handler),
        )
        .route(
            "/xblue/graph/algorithms/{algo}/results",
            get(list_algorithm_results_handler),
        )
        .route("/xblue/entities/resolve", post(resolve_entities_handler))
        .route(
            "/xblue/entities/candidates",
            get(list_entity_candidates_handler),
        )
        .route(
            "/xblue/nodes/{id}/explain/{metric}",
            get(explain_node_metric_handler),
        )
        .route(
            "/xblue/knowledge/node-candidates",
            get(list_knowledge_node_candidates_handler),
        )
        .route(
            "/xblue/knowledge/node-candidates/{entity_id}/accept",
            post(accept_knowledge_node_candidate_handler),
        )
        .route(
            "/xblue/entities/links/{entity_id}/{node_id}/confirm",
            post(confirm_entity_link_handler),
        )
        .route(
            "/xblue/entities/links/{entity_id}/{node_id}/reject",
            post(reject_entity_link_handler),
        )
        .route("/xblue/geo/convex-hull", post(compute_convex_hull_handler))
}

fn actor_uuid(principal: &AuthPrincipal) -> Option<Uuid> {
    if principal.user_id.is_nil() {
        None
    } else {
        Some(principal.user_id)
    }
}

fn parse_optional_json<T: Default + for<'de> Deserialize<'de>>(
    body: &Bytes,
) -> Result<T, AppError> {
    if body.is_empty() || body.iter().all(u8::is_ascii_whitespace) {
        return Ok(T::default());
    }
    serde_json::from_slice::<T>(body)
        .map_err(|err| AppError::bad("INVALID_ARGUMENT", format!("invalid JSON payload: {err}")))
}

async fn record_publish_audit(
    state: &AppState,
    principal: &AuthPrincipal,
    subgraph_slug: &str,
    version: i32,
    detail: serde_json::Value,
) {
    state.audit.record(AuditRecord {
        tenant_id: principal.tenant_id.clone(),
        user_id: actor_uuid(principal),
        username: principal.username.clone(),
        action: AuditAction::SchemaPublish,
        resource_ref: format!("xblue:schema:{subgraph_slug}:v{version}"),
        status_code: 200,
        client_ip: String::from("127.0.0.1"),
        trace_id: current_trace_id(),
        detail_json: detail,
        created_at: Utc::now(),
    });
    if let Err(err) = state.audit.flush().await {
        tracing::warn!(error = %err, "xblue schema publish audit flush failed");
    }
}

// ============================================================================
// Subgraphs & Cross-Subgraph Merge
// ============================================================================

async fn list_subgraphs_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
) -> Result<Json<PaginatedList<SubgraphInfo>>, HttpError> {
    principal.require(Permission::DataRead)?;
    let items = state.store.list_subgraphs(&principal.tenant_id).await?;
    Ok(Json(PaginatedList {
        items,
        next_cursor: None,
    }))
}

async fn create_subgraph_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiJson(req): ApiJson<CreateSubgraphRequest>,
) -> Result<Json<SubgraphInfo>, HttpError> {
    principal.require(Permission::StudioEdit)?;
    let info = state
        .store
        .create_subgraph(&principal.tenant_id, actor_uuid(&principal), req)
        .await?;
    Ok(Json(info))
}

async fn update_subgraph_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(slug): ApiPath<String>,
    ApiJson(req): ApiJson<UpdateSubgraphRequest>,
) -> Result<Json<SubgraphInfo>, HttpError> {
    principal.require(Permission::StudioEdit)?;
    let info = state
        .store
        .update_subgraph(&principal.tenant_id, slug.trim(), req)
        .await?;
    Ok(Json(info))
}

async fn delete_subgraph_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(slug): ApiPath<String>,
) -> Result<Json<serde_json::Value>, HttpError> {
    principal.require(Permission::StudioPublish)?;
    state
        .store
        .delete_subgraph(&principal.tenant_id, slug.trim())
        .await?;
    Ok(Json(serde_json::json!({ "deleted": true })))
}

async fn preview_subgraph_merge_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(slug): ApiPath<String>,
    body: Bytes,
) -> Result<Json<MergePreview>, HttpError> {
    principal.require(Permission::StudioEdit)?;
    let req: MergePreviewRequest = parse_optional_json(&body)?;
    let preview = state
        .store
        .preview_subgraph_merge(&principal.tenant_id, slug.trim(), req)
        .await?;
    Ok(Json(preview))
}

async fn merge_subgraph_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(slug): ApiPath<String>,
    ApiJson(req): ApiJson<MergeSubgraphRequest>,
) -> Result<Json<PublishDraftResponse>, HttpError> {
    principal.enforce_no_prod_mutation()?;
    principal.require(Permission::StudioPublish)?;
    let resp = state
        .store
        .merge_subgraph_into_main(
            &principal.tenant_id,
            slug.trim(),
            actor_uuid(&principal),
            req,
        )
        .await?;
    refresh_cached_tenant_catalog(&state, &principal.tenant_id).await;
    record_publish_audit(
        &state,
        &principal,
        "main",
        resp.version,
        serde_json::json!({
            "merged_from": slug.trim(),
            "draft_id": resp.draft_id,
        }),
    )
    .await;
    Ok(Json(resp))
}

async fn record_builder_sandbox_audit(
    state: &AppState,
    principal: &AuthPrincipal,
    client_ip: &str,
    subgraph_slug: &str,
    detail: serde_json::Value,
) {
    state.audit.record(AuditRecord {
        tenant_id: principal.tenant_id.clone(),
        user_id: actor_uuid(principal),
        username: principal.username.clone(),
        action: AuditAction::BuilderMcpMutate,
        resource_ref: format!("xblue:subgraph:{subgraph_slug}"),
        status_code: 200,
        client_ip: String::from(client_ip),
        trace_id: current_trace_id(),
        detail_json: detail,
        created_at: Utc::now(),
    });
    if let Err(err) = state.audit.flush().await {
        tracing::warn!(error = %err, "xblue builder sandbox audit flush failed");
    }
}

async fn dry_run_dev_subgraph_handler(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    principal: AuthPrincipal,
    ApiPath(slug): ApiPath<String>,
    body: Bytes,
) -> Result<Json<DevSubgraphRunResponse>, HttpError> {
    principal.require(Permission::StudioEdit)?;
    principal.require(Permission::DataSync)?;
    let req: DevSubgraphRunRequest = parse_optional_json(&body)?;
    let draft_id = req.draft_id;
    let client_ip = crate::routes::auth::extract_client_ip(&headers);
    let clean_slug = slug.trim();
    let web_base = principal
        .web_base_url
        .as_deref()
        .or(state.axagent.public_host.as_deref());
    let mut resp = state
        .axagent
        .capability
        .execute_dev_subgraph_dry_run(&principal.tenant_id, clean_slug, req, web_base)
        .await?;
    state
        .axagent
        .capability
        .mask_dev_sample_nodes(
            &principal,
            &client_ip,
            clean_slug,
            draft_id,
            &mut resp.sample_nodes,
        )
        .await;
    record_builder_sandbox_audit(
        &state,
        &principal,
        &client_ip,
        clean_slug,
        serde_json::json!({
            "endpoint": "dry-run-materialize",
            "token_profile": principal.token_profile,
            "draft_id": draft_id,
            "nodes_upserted": resp.nodes_upserted,
            "links_upserted": resp.links_upserted,
            "points_computed": resp.points_computed,
        }),
    )
    .await;
    Ok(Json(resp))
}

async fn reset_dev_subgraph_nodes_handler(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    principal: AuthPrincipal,
    ApiPath(slug): ApiPath<String>,
) -> Result<Json<DevSubgraphResetResponse>, HttpError> {
    principal.require(Permission::StudioEdit)?;
    let client_ip = crate::routes::auth::extract_client_ip(&headers);
    let clean_slug = slug.trim();
    let web_base = principal
        .web_base_url
        .as_deref()
        .or(state.axagent.public_host.as_deref());
    let resp = state
        .store
        .reset_dev_subgraph_nodes(&principal.tenant_id, clean_slug, web_base)
        .await?;
    record_builder_sandbox_audit(
        &state,
        &principal,
        &client_ip,
        clean_slug,
        serde_json::json!({
            "endpoint": "reset-nodes",
            "token_profile": principal.token_profile,
            "nodes_deleted": resp.nodes_deleted,
            "links_deleted": resp.links_deleted,
            "series_points_deleted": resp.series_points_deleted,
        }),
    )
    .await;
    Ok(Json(resp))
}

async fn clone_subgraph_from_template_handler(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    principal: AuthPrincipal,
    ApiJson(req): ApiJson<TrialCloneTemplateRequest>,
) -> Result<(StatusCode, Json<TrialCloneTemplateResponse>), HttpError> {
    principal.require(Permission::StudioEdit)?;
    let client_ip = crate::routes::auth::extract_client_ip(&headers);
    let res = state
        .ops
        .clone_trial_subgraph(
            &principal.tenant_id,
            &principal.dept_id,
            actor_uuid(&principal),
            &req,
        )
        .await?;

    state.audit.record(AuditRecord {
        tenant_id: principal.tenant_id.clone(),
        user_id: actor_uuid(&principal),
        username: principal.username.clone(),
        action: AuditAction::TrialTemplateClone,
        resource_ref: res.subgraph_slug.clone(),
        status_code: 201,
        client_ip,
        trace_id: current_trace_id(),
        detail_json: serde_json::json!({
            "pack_id": res.source_template_id,
            "nodes_materialized": res.nodes_materialized,
            "links_materialized": res.links_materialized,
        }),
        created_at: Utc::now(),
    });
    if let Err(err) = state.audit.flush().await {
        tracing::warn!(error = %err, "xblue trial template clone audit flush failed");
    }

    Ok((StatusCode::CREATED, Json(res)))
}

async fn get_subgraph_consumption_handler(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    principal: AuthPrincipal,
    ApiPath(slug): ApiPath<String>,
) -> Result<Json<SubgraphConsumptionResponse>, HttpError> {
    principal.require(Permission::DataRead)?;
    let clean_slug = slug.trim();
    authorize_subgraph_read(&state, &headers, &principal, clean_slug).await?;
    let resp = state
        .store
        .get_subgraph_consumption(&principal.tenant_id, clean_slug)
        .await?;
    Ok(Json(resp))
}

async fn package_subgraph_agent_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(slug): ApiPath<String>,
    ApiJson(req): ApiJson<PackageSubgraphAgentRequest>,
) -> Result<Json<PackageSubgraphAgentResponse>, HttpError> {
    principal.require(Permission::StudioEdit)?;
    principal.require(Permission::AgentEdit)?;
    if req.environment == Environment::Prod {
        principal.enforce_no_prod_mutation()?;
    }

    let agent_name = req.agent_name.trim();
    if agent_name.is_empty() {
        return Err(AppError::bad("INVALID_ARGUMENT", "agent_name cannot be empty").into());
    }

    let clean_slug = slug.trim();
    let sg_info =
        load_packaging_subgraph_info(&state, &principal.tenant_id, clean_slug, req.environment)
            .await?;
    let published = state
        .store
        .get_published_schema(&principal.tenant_id, clean_slug)
        .await?;
    if published.version < 1 {
        return Err(AppError::bad(
            "SUBGRAPH_NOT_PUBLISHED",
            "subgraph has no published schema version",
        )
        .into());
    }

    let selected_nodes = select_packaged_node_types(clean_slug, &published.doc, &req.node_types)?;
    let actor = actor_uuid(&principal);
    let (upserted_eps, packaged_metrics) = upsert_packaged_endpoints(
        &state,
        &principal.tenant_id,
        &sg_info,
        &published,
        &selected_nodes,
        &req,
        actor,
    )
    .await?;

    let spec = build_packaged_agent_spec(
        clean_slug,
        &sg_info.display_name,
        &selected_nodes,
        &published.doc,
        &packaged_metrics,
        &upserted_eps,
    );
    let resp = upsert_and_publish_packaged_agent(
        &state,
        &principal.tenant_id,
        &req,
        spec,
        actor,
        &upserted_eps,
    )
    .await?;
    Ok(Json(resp))
}

async fn load_packaging_subgraph_info(
    state: &AppState,
    tenant_id: &str,
    slug: &str,
    env: Environment,
) -> Result<SubgraphInfo, AppError> {
    let subgraphs = state.store.list_subgraphs(tenant_id).await?;
    let sg_info = subgraphs
        .into_iter()
        .find(|s| s.slug == slug)
        .ok_or(AppError::NotFound("SUBGRAPH_NOT_FOUND"))?;
    if env == Environment::Prod && sg_info.is_dev_sandbox {
        return Err(AppError::bad(
            "SANDBOX_PROD_PACK_FORBIDDEN",
            "dev sandbox subgraphs can only package TEST endpoints; merge into main before PROD packaging",
        ));
    }
    Ok(sg_info)
}

fn select_packaged_node_types<'a>(
    subgraph_slug: &str,
    doc: &'a SchemaDoc,
    requested: &[String],
) -> Result<Vec<&'a NodeTypeSpec>, AppError> {
    if requested.is_empty() {
        return Ok(doc.node_types.iter().collect());
    }
    let mut seen = BTreeSet::new();
    let mut out = Vec::with_capacity(requested.len());
    for raw_name in requested {
        let trimmed = raw_name.trim();
        if trimmed.is_empty() || !seen.insert(String::from(trimmed)) {
            continue;
        }
        let found = doc
            .node_types
            .iter()
            .find(|nt| nt.name == trimmed)
            .ok_or_else(|| {
                AppError::bad(
                    "TYPE_NOT_FOUND",
                    format!("node type '{trimmed}' is not published in subgraph '{subgraph_slug}'"),
                )
            })?;
        out.push(found);
    }
    Ok(out)
}

async fn upsert_packaged_endpoints(
    state: &AppState,
    tenant_id: &str,
    sg_info: &SubgraphInfo,
    published: &PublishedSchemaResponse,
    selected_nodes: &[&NodeTypeSpec],
    req: &PackageSubgraphAgentRequest,
    published_by: Option<Uuid>,
) -> Result<(Vec<EndpointSpec>, Vec<MetricDef>), AppError> {
    let is_prod = req.environment == Environment::Prod;
    let mut upserted = Vec::new();

    for &nt in selected_nodes {
        let ep_slug =
            build_packaged_endpoint_slug(&sg_info.slug, sg_info.is_main, &nt.name, "objects");
        let snap = if is_prod {
            Some(
                serde_json::to_value(nt)
                    .map_err(|err| AppError::Config(format!("serialize NodeTypeSpec: {err}")))?,
            )
        } else {
            None
        };
        let label = if nt.display_name.trim().is_empty() {
            nt.name.as_str()
        } else {
            nt.display_name.trim()
        };
        let desc = if nt.description.trim().is_empty() {
            format!("查询子图 {} 的 {} 领域对象", sg_info.slug, nt.name)
        } else {
            nt.description.clone()
        };
        let ep = state
            .store
            .upsert_endpoint_by_slug(
                tenant_id,
                SaveEndpointInput {
                    slug: ep_slug,
                    environment: req.environment,
                    kind: EndpointKind::ObjectType,
                    target_id: nt.name.clone(),
                    target_version: is_prod.then_some(published.version),
                    target_snapshot: snap,
                    title: format!("{label} 领域对象查询"),
                    description: desc,
                    input_schema: build_object_type_schema(nt, &published.doc),
                    output_shape: OutputShape::Object,
                    required_scope: Permission::DataRead,
                    rate_limit_rpm: 120,
                    cache_ttl_s: 0,
                    expose_mcp: req.expose_mcp,
                    show_as_app: true,
                    enabled: true,
                    published_by,
                },
            )
            .await?;
        upserted.push(ep);
    }

    let metrics = upsert_packaged_metric_endpoints(
        state,
        tenant_id,
        sg_info,
        req,
        published_by,
        &mut upserted,
    )
    .await?;
    Ok((upserted, metrics))
}

async fn upsert_packaged_metric_endpoints(
    state: &AppState,
    tenant_id: &str,
    sg_info: &SubgraphInfo,
    req: &PackageSubgraphAgentRequest,
    published_by: Option<Uuid>,
    upserted: &mut Vec<EndpointSpec>,
) -> Result<Vec<MetricDef>, AppError> {
    let is_prod = req.environment == Environment::Prod;
    let mut seen = BTreeSet::new();
    let mut packaged_metrics = Vec::new();

    for raw_metric in &req.metric_names {
        let name = raw_metric.trim();
        if name.is_empty() || !seen.insert(String::from(name)) {
            continue;
        }
        let Some(metric) = state.store.get_metric_by_name(tenant_id, name).await? else {
            continue;
        };
        let ep_slug =
            build_packaged_endpoint_slug(&sg_info.slug, sg_info.is_main, &metric.name, "metric");
        let snap = if is_prod {
            Some(
                serde_json::to_value(&metric)
                    .map_err(|err| AppError::Config(format!("serialize MetricDef: {err}")))?,
            )
        } else {
            None
        };
        let title = if metric.display_name.trim().is_empty() {
            metric.name.clone()
        } else {
            metric.display_name.clone()
        };
        let ep = state
            .store
            .upsert_endpoint_by_slug(
                tenant_id,
                SaveEndpointInput {
                    slug: ep_slug,
                    environment: req.environment,
                    kind: EndpointKind::Metric,
                    target_id: metric.name.clone(),
                    target_version: is_prod.then_some(metric.version),
                    target_snapshot: snap,
                    title,
                    description: metric.description.clone(),
                    input_schema: build_metric_schema(&metric),
                    output_shape: OutputShape::Rows,
                    required_scope: Permission::DataRead,
                    rate_limit_rpm: 120,
                    cache_ttl_s: 0,
                    expose_mcp: req.expose_mcp,
                    show_as_app: true,
                    enabled: true,
                    published_by,
                },
            )
            .await?;
        upserted.push(ep);
        packaged_metrics.push(metric);
    }
    Ok(packaged_metrics)
}

fn build_packaged_agent_spec(
    subgraph_slug: &str,
    subgraph_display_name: &str,
    selected_nodes: &[&NodeTypeSpec],
    doc: &SchemaDoc,
    metrics: &[MetricDef],
    endpoints: &[EndpointSpec],
) -> AgentSpec {
    let mut prompt = String::new();
    let _w = writeln!(
        prompt,
        "你是围绕业务子图「{subgraph_display_name}」（slug: {subgraph_slug}）构建的受控领域智能体。"
    );
    let _w = writeln!(
        prompt,
        "核心铁律：严禁心算任何业务数值（求和、均值、占比、同环比、排名或因果推演）；一切对象、关系与指标查询必须调用已绑定的发布端点工具或 graph_neighbors 工具获取确定性结果，严禁编造未在工具返回中出现的数字或节点标识。"
    );
    let _w = writeln!(prompt, "\n已绑定业务节点类型：");
    let node_name_set: BTreeSet<&str> = selected_nodes.iter().map(|n| n.name.as_str()).collect();
    for &nt in selected_nodes {
        let attrs: Vec<&str> = nt
            .properties
            .iter()
            .filter(|p| p.kind == PropertyKind::Attribute)
            .map(|p| p.name.as_str())
            .collect();
        let node_metrics: Vec<&str> = nt
            .properties
            .iter()
            .filter(|p| p.kind == PropertyKind::Metric)
            .map(|p| p.name.as_str())
            .collect();
        let _w = writeln!(
            prompt,
            "- {}（{}，title_template={}）：属性=[{}] 指标=[{}]",
            nt.name,
            nt.display_name,
            nt.title_template,
            attrs.join(", "),
            node_metrics.join(", ")
        );
    }

    let rel_edges: Vec<_> = doc
        .edge_types
        .iter()
        .filter(|e| {
            node_name_set.contains(e.src_type_name.as_str())
                || node_name_set.contains(e.dst_type_name.as_str())
        })
        .collect();
    if !rel_edges.is_empty() {
        let _w = writeln!(prompt, "\n关联拓扑关系：");
        for et in rel_edges {
            let _w = writeln!(
                prompt,
                "- {} ({} -> {})",
                et.name, et.src_type_name, et.dst_type_name
            );
        }
    }

    if !metrics.is_empty() {
        let _w = writeln!(prompt, "\n认证语义指标：");
        for m in metrics {
            let _w = writeln!(prompt, "- {}（{}）", m.name, m.display_name);
        }
    }

    let mut tools: Vec<ToolRef> = endpoints
        .iter()
        .map(|ep| ToolRef::Endpoint { endpoint_id: ep.id })
        .collect();
    tools.push(ToolRef::Builtin {
        name: String::from("graph_neighbors"),
    });

    AgentSpec {
        system_prompt: prompt,
        tools,
        knowledge_scope: KnowledgeScope::default(),
        ..AgentSpec::default()
    }
}

async fn upsert_and_publish_packaged_agent(
    state: &AppState,
    tenant_id: &str,
    req: &PackageSubgraphAgentRequest,
    spec: AgentSpec,
    actor: Option<Uuid>,
    upserted_eps: &[EndpointSpec],
) -> Result<PackageSubgraphAgentResponse, AppError> {
    let agent_name = req.agent_name.trim();
    let description = req.description.trim();
    let saved = if let Some(existing) = state.store.get_agent_by_name(tenant_id, agent_name).await?
    {
        state
            .store
            .update_agent(
                tenant_id,
                existing.id,
                UpdateAgentRequest {
                    name: Some(String::from(agent_name)),
                    description: Some(String::from(description)),
                    spec,
                },
            )
            .await?
            .ok_or(AppError::NotFound("NOT_FOUND"))?
    } else {
        state
            .store
            .create_agent(
                tenant_id,
                actor,
                CreateAgentRequest {
                    name: String::from(agent_name),
                    description: String::from(description),
                    spec,
                },
            )
            .await?
    };

    let agent_version = if req.publish_agent {
        state
            .store
            .publish_agent_version(tenant_id, saved.id, actor)
            .await?
            .version
    } else {
        saved.version
    };

    let mut endpoints_upserted = Vec::with_capacity(upserted_eps.len());
    let mut mcp_tool_names = Vec::new();
    for ep in upserted_eps {
        if ep.expose_mcp {
            mcp_tool_names.push(ep.slug.replace('-', "_"));
        }
        endpoints_upserted.push(PackagedEndpointItem {
            id: ep.id,
            slug: ep.slug.clone(),
            kind: ep.kind,
            expose_mcp: ep.expose_mcp,
        });
    }
    let mcp_exposed_count = mcp_tool_names.len();

    Ok(PackageSubgraphAgentResponse {
        agent_id: saved.id,
        agent_name: saved.name,
        agent_version,
        endpoints_upserted,
        mcp_exposed_count,
        mcp_tool_names,
        workbench_url: format!("/agents?tab=workbench&agent={}", saved.id),
    })
}

// ============================================================================
// Published Schema, Validation, Versions, Diff & Restore
// ============================================================================

async fn get_schema_handler(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    principal: AuthPrincipal,
    ApiQuery(sub): ApiQuery<SubgraphQueryParam>,
) -> Result<Json<PublishedSchemaResponse>, HttpError> {
    principal.require(Permission::DataRead)?;
    let slug = sub.slug_or_main();
    if slug != "main" {
        let client_ip = crate::routes::auth::extract_client_ip(&headers);
        let _ = axpoint_server::policy::authorize_resource_for_store(
            &state.store,
            &state.cedar,
            &principal,
            Permission::DataRead,
            axpoint_domain::guard::GrantResourceKind::Subgraph,
            slug,
            &client_ip,
        )
        .await?;
    }
    let resp = state
        .store
        .get_published_schema(&principal.tenant_id, slug)
        .await?;
    Ok(Json(resp))
}

async fn save_schema_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiQuery(sub): ApiQuery<SubgraphQueryParam>,
    ApiJson(req): ApiJson<SaveSchemaDirectRequest>,
) -> Result<Json<PublishedSchemaResponse>, HttpError> {
    principal.enforce_no_prod_mutation()?;
    let slug = sub.slug_or_main();
    if slug == "main" {
        principal.require(Permission::StudioPublish)?;
    } else {
        principal.require(Permission::StudioEdit)?;
    }
    let resp = state
        .store
        .save_schema_direct(&principal.tenant_id, slug, actor_uuid(&principal), req)
        .await?;
    if slug == "main" {
        refresh_cached_tenant_catalog(&state, &principal.tenant_id).await;
    }
    record_publish_audit(
        &state,
        &principal,
        &resp.subgraph_slug,
        resp.version,
        serde_json::json!({ "mode": "direct" }),
    )
    .await;
    Ok(Json(resp))
}

#[derive(Debug, Deserialize)]
struct ValidateSchemaPayload {
    #[serde(default)]
    schema_doc: Option<SchemaDoc>,
    #[serde(flatten)]
    flat_doc: SchemaDoc,
}

async fn validate_schema_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiQuery(sub): ApiQuery<SubgraphQueryParam>,
    ApiJson(payload): ApiJson<ValidateSchemaPayload>,
) -> Result<Json<ValidateSchemaResponse>, HttpError> {
    principal.require(Permission::StudioEdit)?;
    let doc = payload.schema_doc.unwrap_or(payload.flat_doc);
    let ctx = state
        .store
        .load_validation_context(&principal.tenant_id, sub.slug_or_main())
        .await?;
    let summary = validate_with_metrics(&doc, &ctx);
    let valid = !summary
        .issues
        .iter()
        .any(|i| i.severity == IssueSeverity::Error);
    Ok(Json(ValidateSchemaResponse {
        valid,
        issues: summary.issues,
        pass_count: summary.pass_count,
    }))
}

async fn list_schema_versions_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiQuery(sub): ApiQuery<SubgraphQueryParam>,
) -> Result<Json<PaginatedList<SchemaVersionSummary>>, HttpError> {
    principal.require(Permission::DataRead)?;
    let items = state
        .store
        .list_schema_versions(&principal.tenant_id, sub.slug_or_main())
        .await?;
    Ok(Json(PaginatedList {
        items,
        next_cursor: None,
    }))
}

async fn get_schema_version_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(version): ApiPath<i32>,
    ApiQuery(sub): ApiQuery<SubgraphQueryParam>,
) -> Result<Json<SchemaVersionDetail>, HttpError> {
    principal.require(Permission::DataRead)?;
    let detail = state
        .store
        .get_schema_version(&principal.tenant_id, sub.slug_or_main(), version)
        .await?
        .ok_or(AppError::NotFound("VERSION_NOT_FOUND"))?;
    Ok(Json(detail))
}

async fn diff_schema_versions_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath((from_ver, to_ver)): ApiPath<(i32, i32)>,
    ApiQuery(sub): ApiQuery<SubgraphQueryParam>,
) -> Result<Json<SchemaDiff>, HttpError> {
    principal.require(Permission::DataRead)?;
    let diff = state
        .store
        .diff_schema_versions(&principal.tenant_id, sub.slug_or_main(), from_ver, to_ver)
        .await?;
    Ok(Json(diff))
}

async fn restore_schema_version_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(version): ApiPath<i32>,
    ApiQuery(sub): ApiQuery<SubgraphQueryParam>,
) -> Result<Json<SchemaDraft>, HttpError> {
    principal.require(Permission::StudioEdit)?;
    let draft = state
        .store
        .restore_schema_version(
            &principal.tenant_id,
            sub.slug_or_main(),
            version,
            actor_uuid(&principal),
        )
        .await?;
    Ok(Json(draft))
}

// ============================================================================
// Drafts & AI Draft Generation
// ============================================================================

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ListDraftsQuery {
    #[serde(default)]
    subgraph: Option<String>,
    #[serde(default)]
    status: Option<String>,
}

async fn list_drafts_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiQuery(query): ApiQuery<ListDraftsQuery>,
) -> Result<Json<PaginatedList<SchemaDraft>>, HttpError> {
    principal.require(Permission::StudioEdit)?;
    let slug = match query.subgraph.as_deref().map(str::trim) {
        Some(s) if !s.is_empty() => s,
        _ => "main",
    };
    let status = match query.status.as_deref().map(str::trim) {
        Some(s) if !s.is_empty() => Some(DraftStatus::from_str(s)?),
        _ => None,
    };
    let items = state
        .store
        .list_drafts(&principal.tenant_id, slug, status)
        .await?;
    Ok(Json(PaginatedList {
        items,
        next_cursor: None,
    }))
}

async fn get_draft_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<SchemaDraft>, HttpError> {
    principal.require(Permission::StudioEdit)?;
    let draft = state
        .store
        .get_draft(&principal.tenant_id, id)
        .await?
        .ok_or(AppError::NotFound("DRAFT_NOT_FOUND"))?;
    Ok(Json(draft))
}

async fn create_draft_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiQuery(sub): ApiQuery<SubgraphQueryParam>,
    ApiJson(req): ApiJson<CreateDraftRequest>,
) -> Result<Json<SchemaDraft>, HttpError> {
    principal.require(Permission::StudioEdit)?;
    let target_slug = req
        .subgraph_slug
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| sub.slug_or_main())
        .to_owned();
    let draft = state
        .store
        .create_draft(
            &principal.tenant_id,
            &target_slug,
            actor_uuid(&principal),
            req,
        )
        .await?;
    Ok(Json(draft))
}

async fn update_draft_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(req): ApiJson<UpdateDraftRequest>,
) -> Result<Json<SchemaDraft>, HttpError> {
    principal.require(Permission::StudioEdit)?;
    let existing = state
        .store
        .get_draft(&principal.tenant_id, id)
        .await?
        .ok_or(AppError::NotFound("DRAFT_NOT_FOUND"))?;
    if let Some(creator) = existing.created_by
        && actor_uuid(&principal) != Some(creator)
        && !principal.has(Permission::StudioPublish)
    {
        return Err(AppError::Forbidden("DRAFT_NOT_OWNER").into());
    }
    let draft = state
        .store
        .update_draft(&principal.tenant_id, id, req)
        .await?;
    Ok(Json(draft))
}

async fn discard_draft_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<SchemaDraft>, HttpError> {
    principal.require(Permission::StudioEdit)?;
    let draft = state.store.discard_draft(&principal.tenant_id, id).await?;
    Ok(Json(draft))
}

async fn publish_draft_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
    body: Bytes,
) -> Result<Json<PublishDraftResponse>, HttpError> {
    principal.enforce_no_prod_mutation()?;
    let existing = state
        .store
        .get_draft(&principal.tenant_id, id)
        .await?
        .ok_or(AppError::NotFound("DRAFT_NOT_FOUND"))?;
    if existing.subgraph_slug == "main" {
        principal.require(Permission::StudioPublish)?;
    } else {
        principal.require(Permission::StudioEdit)?;
    }
    let req: PublishDraftRequest = parse_optional_json(&body)?;
    let resp = state
        .store
        .publish_draft(&principal.tenant_id, id, actor_uuid(&principal), req)
        .await?;
    if resp.subgraph_slug == "main" {
        refresh_cached_tenant_catalog(&state, &principal.tenant_id).await;
    }
    record_publish_audit(
        &state,
        &principal,
        &resp.subgraph_slug,
        resp.version,
        serde_json::json!({
            "draft_id": resp.draft_id,
            "mapping_jobs": resp.mapping_jobs,
            "aggregate_jobs": resp.aggregate_jobs,
        }),
    )
    .await;
    Ok(Json(resp))
}

async fn create_ai_draft_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiQuery(sub): ApiQuery<SubgraphQueryParam>,
    ApiJson(req): ApiJson<AiDraftRequest>,
) -> Result<Json<SchemaDraft>, HttpError> {
    principal.require(Permission::StudioEdit)?;
    if req.dataset_ids.is_empty() && req.doc_ids.is_empty() && req.description.trim().is_empty() {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "at least one of dataset_ids, doc_ids, or description is required",
        )
        .into());
    }
    let slug = sub.slug_or_main();
    let profiling = build_profiling_bundle(&state, &principal.tenant_id, &req.dataset_ids).await?;
    let doc_entities = state
        .store
        .load_doc_entity_groups(&principal.tenant_id, &req.doc_ids)
        .await?;
    let published = state
        .store
        .get_published_schema(&principal.tenant_id, slug)
        .await?;
    let validation_ctx = state
        .store
        .load_validation_context(&principal.tenant_id, slug)
        .await?;

    let studio_ai = state
        .ai
        .for_tenant(&principal.tenant_id)
        .for_user(principal.user_id);
    let generated = generate_schema_draft(
        &studio_ai,
        &SchemaDraftInput {
            profiling: &profiling,
            doc_entities: &doc_entities,
            current_schema: &published.doc,
            description: &req.description,
            validation_ctx: &validation_ctx,
        },
    )
    .await?;

    let draft = state
        .store
        .create_draft(
            &principal.tenant_id,
            slug,
            actor_uuid(&principal),
            CreateDraftRequest {
                title: req.title,
                schema_doc: generated.schema_doc,
                base_version: Some(published.version),
                source: Some(DraftSource::Ai),
                ai_context: Some(serde_json::json!({
                    "description": req.description,
                    "dataset_ids": req.dataset_ids,
                    "doc_ids": req.doc_ids,
                    "attempts": generated.attempts,
                })),
                subgraph_slug: None,
            },
        )
        .await?;
    Ok(Json(draft))
}

fn check_draft_edit_owner(principal: &AuthPrincipal, draft: &SchemaDraft) -> Result<(), AppError> {
    if let Some(creator) = draft.created_by
        && actor_uuid(principal) != Some(creator)
        && !principal.has(Permission::StudioPublish)
    {
        return Err(AppError::Forbidden("DRAFT_NOT_OWNER"));
    }
    Ok(())
}

async fn load_copilot_dataset_profiles(
    state: &AppState,
    tenant_id: &str,
    req: &StudioCopilotRequest,
) -> Result<Vec<CopilotDatasetProfile>, AppError> {
    let Some(refs) = &req.refs else {
        return Ok(Vec::new());
    };
    let mut out = Vec::with_capacity(refs.dataset_ids.len());
    for &ds_id in &refs.dataset_ids {
        let ds = state
            .store
            .get_dataset(tenant_id, ds_id)
            .await?
            .ok_or(AppError::NotFound("DATASET_NOT_FOUND"))?;
        out.push(CopilotDatasetProfile::from_dataset(&ds));
    }
    Ok(out)
}

async fn draft_copilot_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(req): ApiJson<StudioCopilotRequest>,
) -> Result<Json<StudioCopilotResponse>, HttpError> {
    principal.require(Permission::StudioEdit)?;
    if req.prompt.trim().is_empty() {
        return Err(AppError::bad("INVALID_ARGUMENT", "prompt must not be empty").into());
    }
    let draft = state
        .store
        .get_draft(&principal.tenant_id, id)
        .await?
        .ok_or(AppError::NotFound("DRAFT_NOT_FOUND"))?;

    let ds_profiles = load_copilot_dataset_profiles(&state, &principal.tenant_id, &req).await?;
    let spec_markdown = req
        .refs
        .as_ref()
        .is_some_and(|r| r.include_spec_doc)
        .then(|| {
            let sync = BTreeMap::new();
            compile_subgraph_spec_doc(&SpecDocCompileInput {
                subgraph_slug: &draft.subgraph_slug,
                subgraph_display_name: &draft.title,
                version: draft.base_version,
                draft_id: Some(draft.id),
                pass_count: draft.pass_count,
                doc: &draft.schema_doc,
                mapping_sync: &sync,
            })
            .markdown
        });

    let studio_ai = state
        .ai
        .for_tenant(&principal.tenant_id)
        .for_user(principal.user_id);
    let prompt_input = CopilotPromptInput {
        subgraph_slug: &draft.subgraph_slug,
        schema_doc: &draft.schema_doc,
        user_prompt: &req.prompt,
        focus: req.focus.as_ref(),
        datasets: &ds_profiles,
        spec_overview: spec_markdown.as_deref(),
    };

    match req.mode {
        CopilotActionMode::DiagnoseOnly => {
            let diagnosis = run_copilot_diagnose(&studio_ai, &prompt_input).await?;
            Ok(Json(StudioCopilotResponse {
                mode: CopilotActionMode::DiagnoseOnly,
                mutated: false,
                snapshot_id: None,
                snapshot_no: None,
                diagnosis,
                diff: None,
                pass_count: draft.pass_count,
                issues: draft.issues,
            }))
        }
        CopilotActionMode::PatchDraft => {
            let resp =
                execute_copilot_patch_turn(&state, &principal, &draft, &studio_ai, &prompt_input)
                    .await?;
            Ok(Json(resp))
        }
    }
}

async fn execute_copilot_patch_turn(
    state: &AppState,
    principal: &AuthPrincipal,
    draft: &SchemaDraft,
    studio_ai: &axpoint_llm::AiGateway,
    prompt_input: &CopilotPromptInput<'_>,
) -> Result<StudioCopilotResponse, AppError> {
    check_draft_edit_owner(principal, draft)?;
    if draft.status != DraftStatus::Draft {
        return Err(AppError::Conflict("VERSION_CONFLICT"));
    }
    let outcome = run_copilot_patch(studio_ai, prompt_input).await?;
    if outcome.mutated {
        let (snap, updated) = state
            .store
            .apply_copilot_patch_with_snapshot(
                &principal.tenant_id,
                draft.id,
                actor_uuid(principal),
                outcome.patched_doc,
            )
            .await?;
        Ok(StudioCopilotResponse {
            mode: CopilotActionMode::PatchDraft,
            mutated: true,
            snapshot_id: Some(snap.id),
            snapshot_no: Some(snap.snapshot_no),
            diagnosis: outcome.diagnosis,
            diff: Some(outcome.diff),
            pass_count: updated.pass_count,
            issues: updated.issues,
        })
    } else {
        Ok(StudioCopilotResponse {
            mode: CopilotActionMode::PatchDraft,
            mutated: false,
            snapshot_id: None,
            snapshot_no: None,
            diagnosis: outcome.diagnosis,
            diff: Some(outcome.diff),
            pass_count: draft.pass_count,
            issues: draft.issues.clone(),
        })
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ListDraftSnapshotsQuery {
    #[serde(default)]
    limit: Option<u32>,
}

async fn list_draft_snapshots_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
    ApiQuery(query): ApiQuery<ListDraftSnapshotsQuery>,
) -> Result<Json<DraftSnapshotListResponse>, HttpError> {
    principal.require(Permission::DataRead)?;
    let items = state
        .store
        .list_draft_snapshots(&principal.tenant_id, id, query.limit.unwrap_or(20))
        .await?;
    Ok(Json(DraftSnapshotListResponse { items }))
}

async fn restore_draft_snapshot_handler(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    principal: AuthPrincipal,
    ApiPath((id, snap_id)): ApiPath<(Uuid, Uuid)>,
) -> Result<Json<SchemaDraft>, HttpError> {
    principal.require(Permission::StudioEdit)?;
    let existing = state
        .store
        .get_draft(&principal.tenant_id, id)
        .await?
        .ok_or(AppError::NotFound("DRAFT_NOT_FOUND"))?;
    check_draft_edit_owner(&principal, &existing)?;

    let restored = state
        .store
        .restore_draft_snapshot(&principal.tenant_id, id, snap_id)
        .await?;

    let client_ip = crate::routes::auth::extract_client_ip(&headers);
    state.audit.record(AuditRecord {
        tenant_id: principal.tenant_id.clone(),
        user_id: actor_uuid(&principal),
        username: principal.username.clone(),
        action: AuditAction::StudioSnapshotRestore,
        resource_ref: format!("xblue:draft:{id}:snapshot:{snap_id}"),
        status_code: 200,
        client_ip,
        trace_id: current_trace_id(),
        detail_json: serde_json::json!({
            "draft_id": id,
            "snapshot_id": snap_id,
            "subgraph_slug": restored.subgraph_slug,
            "pass_count": restored.pass_count,
        }),
        created_at: Utc::now(),
    });
    if let Err(err) = state.audit.flush().await {
        tracing::warn!(error = %err, "xblue studio snapshot restore audit flush failed");
    }

    Ok(Json(restored))
}

async fn confirm_draft_proposals_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
    body: Bytes,
) -> Result<Json<SchemaDraft>, HttpError> {
    principal.require(Permission::StudioEdit)?;
    let existing = state
        .store
        .get_draft(&principal.tenant_id, id)
        .await?
        .ok_or(AppError::NotFound("DRAFT_NOT_FOUND"))?;
    check_draft_edit_owner(&principal, &existing)?;

    let req: ConfirmProposalsRequest = parse_optional_json(&body)?;
    let updated = state
        .store
        .confirm_draft_proposals(&principal.tenant_id, id, actor_uuid(&principal), req)
        .await?;
    Ok(Json(updated))
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct LineageQuery {
    #[serde(default)]
    draft_id: Option<Uuid>,
}

async fn authorize_subgraph_read(
    state: &AppState,
    headers: &axum::http::HeaderMap,
    principal: &AuthPrincipal,
    slug: &str,
) -> Result<(), AppError> {
    if slug != "main" {
        let client_ip = crate::routes::auth::extract_client_ip(headers);
        let _ = axpoint_server::policy::authorize_resource_for_store(
            &state.store,
            &state.cedar,
            principal,
            Permission::DataRead,
            axpoint_domain::guard::GrantResourceKind::Subgraph,
            slug,
            &client_ip,
        )
        .await?;
    }
    Ok(())
}

async fn get_node_lineage_handler(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    principal: AuthPrincipal,
    ApiPath((slug, node_type)): ApiPath<(String, String)>,
    ApiQuery(query): ApiQuery<LineageQuery>,
) -> Result<Json<NodeLineageGraph>, HttpError> {
    principal.require(Permission::DataRead)?;
    let clean_slug = slug.trim();
    let clean_node = node_type.trim();
    authorize_subgraph_read(&state, &headers, &principal, clean_slug).await?;

    let doc = if let Some(did) = query.draft_id {
        let draft = state
            .store
            .get_draft(&principal.tenant_id, did)
            .await?
            .ok_or(AppError::NotFound("DRAFT_NOT_FOUND"))?;
        if draft.subgraph_slug != clean_slug {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "draft does not belong to target subgraph",
            )
            .into());
        }
        draft.schema_doc
    } else {
        state
            .store
            .get_published_schema(&principal.tenant_id, clean_slug)
            .await?
            .doc
    };

    let val_ctx = state
        .store
        .load_validation_context(&principal.tenant_id, clean_slug)
        .await?;
    let mut dataset_columns: BTreeMap<String, BTreeMap<String, DataType>> = BTreeMap::new();
    for (key, meta) in val_ctx.datasets {
        dataset_columns.insert(key, meta.columns);
    }
    let downstream = state
        .store
        .list_node_downstream_consumers(&principal.tenant_id, clean_node)
        .await?;

    let graph = build_node_lineage(clean_slug, &doc, clean_node, &dataset_columns, &downstream)?;
    Ok(Json(graph))
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct SpecDocQuery {
    #[serde(default)]
    draft_id: Option<Uuid>,
    #[serde(default)]
    format: Option<String>,
}

async fn load_spec_mapping_sync_map(
    state: &AppState,
    tenant_id: &str,
    slug: &str,
) -> Result<BTreeMap<String, SpecMappingSyncMeta>, AppError> {
    let mappings = state.store.list_mappings(tenant_id, slug).await?;
    let mut sync_map = BTreeMap::new();
    for m in mappings {
        let runs = state.store.list_mapping_runs(tenant_id, m.id, 1).await?;
        if let Some(latest) = runs.first() {
            sync_map.insert(
                m.name.clone(),
                SpecMappingSyncMeta {
                    mapping_name: m.name,
                    last_status: String::from(latest.status.as_str()),
                    nodes_upserted: latest.nodes_upserted,
                },
            );
        }
    }
    Ok(sync_map)
}

async fn get_subgraph_spec_doc_handler(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    principal: AuthPrincipal,
    ApiPath(slug): ApiPath<String>,
    ApiQuery(query): ApiQuery<SpecDocQuery>,
) -> Result<Response, HttpError> {
    principal.require(Permission::DataRead)?;
    let clean_slug = slug.trim();
    authorize_subgraph_read(&state, &headers, &principal, clean_slug).await?;

    let want_markdown = match query.format.as_deref().map(str::trim) {
        None | Some("" | "json" | "JSON") => false,
        Some("markdown" | "MARKDOWN") => true,
        Some(other) => {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                format!("unsupported spec-doc format '{other}'; expected json or markdown"),
            )
            .into());
        }
    };

    let subgraphs = state.store.list_subgraphs(&principal.tenant_id).await?;
    let sub_info = subgraphs
        .into_iter()
        .find(|s| s.slug == clean_slug)
        .ok_or(AppError::NotFound("SUBGRAPH_NOT_FOUND"))?;

    let (doc, version, pass_count, draft_id) = if let Some(did) = query.draft_id {
        let draft = state
            .store
            .get_draft(&principal.tenant_id, did)
            .await?
            .ok_or(AppError::NotFound("DRAFT_NOT_FOUND"))?;
        if draft.subgraph_slug != clean_slug {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "draft does not belong to target subgraph",
            )
            .into());
        }
        (
            draft.schema_doc,
            draft.base_version,
            draft.pass_count,
            Some(did),
        )
    } else {
        let published = state
            .store
            .get_published_schema(&principal.tenant_id, clean_slug)
            .await?;
        let val_ctx = state
            .store
            .load_validation_context(&principal.tenant_id, clean_slug)
            .await?;
        let summary = validate_with_metrics(&published.doc, &val_ctx);
        (published.doc, published.version, summary.pass_count, None)
    };

    let mapping_sync = load_spec_mapping_sync_map(&state, &principal.tenant_id, clean_slug).await?;
    let spec_doc = compile_subgraph_spec_doc(&SpecDocCompileInput {
        subgraph_slug: clean_slug,
        subgraph_display_name: &sub_info.display_name,
        version,
        draft_id,
        pass_count,
        doc: &doc,
        mapping_sync: &mapping_sync,
    });

    if want_markdown {
        Ok((
            StatusCode::OK,
            [(CONTENT_TYPE, "text/markdown; charset=utf-8")],
            spec_doc.markdown,
        )
            .into_response())
    } else {
        Ok(Json(spec_doc).into_response())
    }
}

async fn build_profiling_bundle(
    state: &AppState,
    tenant_id: &str,
    dataset_ids: &[Uuid],
) -> Result<ProfilingBundle, AppError> {
    if dataset_ids.is_empty() {
        return Ok(ProfilingBundle::default());
    }
    let mut datasets = Vec::with_capacity(dataset_ids.len());
    for &ds_id in dataset_ids {
        let ds = state
            .store
            .get_dataset(tenant_id, ds_id)
            .await?
            .ok_or(AppError::NotFound("DATASET_NOT_FOUND"))?;
        datasets.push(ds);
    }

    if let Err(err) = acquire_tenant_session(state, tenant_id, false).await {
        tracing::debug!(tenant_id = %tenant_id, error = %err, "profiling tenant session warmup skipped");
    }
    if let Ok(bundle) =
        axpoint_analytics::profile_datasets(&state.analytics_pool, tenant_id, &datasets).await
    {
        return Ok(bundle);
    }

    let summaries = datasets
        .iter()
        .map(fallback_dataset_profile_summary)
        .collect();
    Ok(ProfilingBundle {
        datasets: summaries,
        fk_candidates: Vec::new(),
    })
}

fn fallback_dataset_profile_summary(ds: &Dataset) -> DatasetProfileSummary {
    let row_cnt = u64::try_from(ds.row_count.max(0)).unwrap_or(0);
    let mut pk_candidates = ds.primary_key.clone();
    let mut columns = Vec::with_capacity(ds.schema_json.len());
    for col in &ds.schema_json {
        let mut roles = BTreeSet::new();
        if ds.primary_key.contains(&col.name) {
            roles.insert(ColumnRoleTag::PkCandidate);
        }
        if matches!(
            col.data_type,
            DataType::Int64 | DataType::Float64 | DataType::Decimal
        ) && !roles.contains(&ColumnRoleTag::PkCandidate)
        {
            roles.insert(ColumnRoleTag::NumericMetric);
        }
        if matches!(col.data_type, DataType::Date | DataType::Timestamp) {
            roles.insert(ColumnRoleTag::Temporal);
        }
        let distinct_est = u64::try_from(col.distinct_estimate.max(0)).unwrap_or(0);
        if distinct_est > 0 && distinct_est <= 50 {
            roles.insert(ColumnRoleTag::Categorical);
        }
        columns.push(ColumnProfileStats {
            name: col.name.clone(),
            data_type: col.data_type,
            null_rate: col.null_ratio.clamp(0.0, 1.0),
            distinct_count: distinct_est,
            roles,
            sample_values: Vec::new(),
        });
    }
    rank_pk_names(&mut pk_candidates);
    DatasetProfileSummary {
        dataset_id: ds.id,
        dataset_name: ds.name.clone(),
        display_name: ds.display_name.clone(),
        row_count: row_cnt,
        columns,
        pk_candidates,
    }
}

fn rank_pk_names(names: &mut [String]) {
    let priority = |s: &str| -> u8 {
        let low = s.to_ascii_lowercase();
        u8::from(
            !(low == "id"
                || low.ends_with("_id")
                || low.ends_with("_code")
                || low.ends_with("_no")
                || low.ends_with("_key")),
        )
    };
    names.sort_by(|a, b| priority(a).cmp(&priority(b)).then_with(|| a.cmp(b)));
}

// ============================================================================
// Mappings & Dry-Run Preview
// ============================================================================

async fn list_mappings_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiQuery(sub): ApiQuery<SubgraphQueryParam>,
) -> Result<Json<PaginatedList<MappingRecord>>, HttpError> {
    principal.require(Permission::DataRead)?;
    let items = state
        .store
        .list_mappings(&principal.tenant_id, sub.slug_or_main())
        .await?;
    Ok(Json(PaginatedList {
        items,
        next_cursor: None,
    }))
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ListMappingRunsQuery {
    #[serde(default)]
    limit: Option<u32>,
}

async fn list_mapping_runs_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
    ApiQuery(query): ApiQuery<ListMappingRunsQuery>,
) -> Result<Json<MappingRunsResponse>, HttpError> {
    principal.require(Permission::DataRead)?;
    let items = state
        .store
        .list_mapping_runs(&principal.tenant_id, id, query.limit.unwrap_or(20))
        .await?;
    Ok(Json(MappingRunsResponse { items }))
}

async fn trigger_mapping_run_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
    body: Bytes,
) -> Result<Json<TriggerMappingRunResponse>, HttpError> {
    principal.enforce_no_prod_mutation()?;
    principal.require(Permission::DataSync)?;
    let req: TriggerMappingRunRequest = parse_optional_json(&body)?;
    let resp = state
        .store
        .enqueue_mapping_run(&principal.tenant_id, id, req)
        .await?;
    Ok(Json(resp))
}

async fn preview_mapping_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<MappingPreviewResponse>, HttpError> {
    principal.require(Permission::StudioEdit)?;
    let mapping = state
        .store
        .get_mapping(&principal.tenant_id, id)
        .await?
        .ok_or(AppError::NotFound("MAPPING_NOT_FOUND"))?;
    let ds = state
        .store
        .get_dataset(&principal.tenant_id, mapping.dataset_id)
        .await?
        .ok_or(AppError::NotFound("DATASET_NOT_FOUND"))?;

    let published = state
        .store
        .get_published_schema(&principal.tenant_id, &mapping.subgraph_slug)
        .await?;
    let main_published = if mapping.subgraph_slug == "main" {
        None
    } else {
        state
            .store
            .get_published_schema(&principal.tenant_id, "main")
            .await
            .ok()
    };
    let node_type = published
        .doc
        .node_types
        .iter()
        .find(|n| n.name == mapping.spec.node_type)
        .or_else(|| {
            main_published.as_ref().and_then(|m| {
                m.doc
                    .node_types
                    .iter()
                    .find(|n| n.name == mapping.spec.node_type)
            })
        })
        .cloned()
        .unwrap_or_else(|| NodeTypeSpec {
            id: None,
            name: mapping.spec.node_type.clone(),
            display_name: mapping.spec.node_type.clone(),
            description: String::new(),
            icon: String::from("circle"),
            color: String::from("#2563eb"),
            title_template: mapping.spec.title.clone(),
            properties: Vec::new(),
            agg_rules: Vec::new(),
            derived_rules: Vec::new(),
            sync_to_knowledge: false,
            pos_x: 0.0,
            pos_y: 0.0,
            ..NodeTypeSpec::default()
        });

    let _ = acquire_tenant_session(&state, &principal.tenant_id, false).await?;
    let preview = axpoint_analytics::preview_mapping_rows(
        &state.analytics_pool,
        &principal.tenant_id,
        mapping.id,
        &ds,
        &node_type,
        &mapping.spec,
        50,
    )
    .await?;

    Ok(Json(preview))
}

// ============================================================================
// Strongly-Typed Published Domain Objects REST API (`/api/xblue/objects*`)
// ============================================================================

async fn list_object_catalog_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
) -> Result<Json<ObjectCatalogResponse>, HttpError> {
    principal.require(Permission::DataRead)?;
    let published = state
        .store
        .get_published_schema(&principal.tenant_id, "main")
        .await?;

    let mut items = Vec::with_capacity(published.doc.node_types.len());
    for nt in &published.doc.node_types {
        let gql_single = xblue_graphql::lower_camel_case(&nt.name);
        let gql_list = format!("{gql_single}List");
        let fields: Vec<ObjectFieldSig> = nt
            .properties
            .iter()
            .map(|p| ObjectFieldSig {
                name: p.name.clone(),
                display_name: p.display_name.clone(),
                kind: p.kind,
                data_type: p.data_type,
                required: p.required,
                unit: p.unit.clone(),
                pii_category: p.pii_category,
            })
            .collect();
        let neighbors = xblue_graphql::compute_neighbor_sigs(nt, &published.doc);
        let sample_graphql =
            format!("{{ {gql_list}(limit: 10) {{ items {{ id nodeKey title }} nextCursor }} }}");

        items.push(ObjectTypeCatalogItem {
            name: nt.name.clone(),
            display_name: nt.display_name.clone(),
            description: nt.description.clone(),
            collection_path: format!("/api/xblue/objects/{}", nt.name),
            item_path: format!("/api/xblue/objects/{}/{{node_key}}", nt.name),
            graphql_field: gql_single,
            graphql_list_field: gql_list,
            fields,
            neighbors,
            sample_graphql,
        });
    }

    Ok(Json(ObjectCatalogResponse {
        version: published.version,
        items,
    }))
}

struct DomainMaskCtx<'a> {
    policy: &'a axpoint_domain::guard::PolicyContext,
    has_pii_raw: bool,
    tenant_key: &'a [u8; 32],
}

async fn list_domain_objects_handler(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    principal: AuthPrincipal,
    ApiPath(type_name): ApiPath<String>,
    ApiQuery(query): ApiQuery<DomainObjectListQuery>,
) -> Result<Json<DomainObjectListResponse>, HttpError> {
    principal.require(Permission::DataRead)?;
    let published = state
        .store
        .get_published_schema(&principal.tenant_id, "main")
        .await?;
    let nt = published
        .doc
        .node_types
        .iter()
        .find(|n| n.name == type_name.trim())
        .ok_or(AppError::NotFound("TYPE_NOT_FOUND"))?;

    let client_ip = crate::routes::auth::extract_client_ip(&headers);
    let _ = axpoint_server::policy::authorize_resource_for_store(
        &state.store,
        &state.cedar,
        &principal,
        Permission::DataRead,
        axpoint_domain::guard::GrantResourceKind::NodeType,
        &nt.name,
        &client_ip,
    )
    .await?;
    let policy = axpoint_server::policy::build_policy_context_for_store(
        &state.store,
        &state.cedar,
        &principal,
        false,
        &client_ip,
    )
    .await?;
    let tenant_key = state.mask_keys.tenant_key(&principal.tenant_id);
    let has_pii_raw = principal.has(Permission::PiiRawRead);

    let (mut nodes, next_cursor) = state
        .store
        .query_domain_objects_list(&principal.tenant_id, &nt.name, &query)
        .await?;
    if !policy.bypass_row_policies && !nodes.is_empty() {
        let ids: Vec<Uuid> = nodes.iter().map(|n| n.id).collect();
        let visible = state
            .store
            .filter_visible_node_ids(&principal.tenant_id, &ids, &policy.node_types)
            .await?;
        nodes.retain(|n| visible.contains(&n.id));
    }

    let mask_ctx = DomainMaskCtx {
        policy: &policy,
        has_pii_raw,
        tenant_key: &tenant_key,
    };
    let items = nodes
        .iter()
        .map(|n| blue_node_to_domain_record(n, Some(nt), &mask_ctx, None))
        .collect();
    Ok(Json(DomainObjectListResponse { items, next_cursor }))
}

async fn get_domain_object_handler(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    principal: AuthPrincipal,
    ApiPath((type_name, node_key)): ApiPath<(String, String)>,
    ApiQuery(query): ApiQuery<DomainObjectItemQuery>,
) -> Result<Json<DomainObjectRecord>, HttpError> {
    principal.require(Permission::DataRead)?;
    let published = state
        .store
        .get_published_schema(&principal.tenant_id, "main")
        .await?;
    let nt = published
        .doc
        .node_types
        .iter()
        .find(|n| n.name == type_name.trim())
        .ok_or(AppError::NotFound("TYPE_NOT_FOUND"))?;

    let client_ip = crate::routes::auth::extract_client_ip(&headers);
    let _ = axpoint_server::policy::authorize_resource_for_store(
        &state.store,
        &state.cedar,
        &principal,
        Permission::DataRead,
        axpoint_domain::guard::GrantResourceKind::NodeType,
        &nt.name,
        &client_ip,
    )
    .await?;
    let policy = axpoint_server::policy::build_policy_context_for_store(
        &state.store,
        &state.cedar,
        &principal,
        false,
        &client_ip,
    )
    .await?;

    let node = state
        .store
        .get_domain_object_by_key(&principal.tenant_id, &nt.name, node_key.trim())
        .await?
        .ok_or(AppError::NotFound("NODE_NOT_FOUND"))?;
    if !policy.bypass_row_policies {
        let visible = state
            .store
            .filter_visible_node_ids(&principal.tenant_id, &[node.id], &policy.node_types)
            .await?;
        if !visible.contains(&node.id) {
            return Err(AppError::NotFound("NODE_NOT_FOUND").into());
        }
    }

    let tenant_key = state.mask_keys.tenant_key(&principal.tenant_id);
    let mask_ctx = DomainMaskCtx {
        policy: &policy,
        has_pii_raw: principal.has(Permission::PiiRawRead),
        tenant_key: &tenant_key,
    };
    let neighbors = if query.include_neighbors {
        Some(
            load_object_neighbors(
                &state,
                &principal.tenant_id,
                &node,
                nt,
                &published.doc,
                &mask_ctx,
            )
            .await?,
        )
    } else {
        None
    };

    Ok(Json(blue_node_to_domain_record(
        &node,
        Some(nt),
        &mask_ctx,
        neighbors,
    )))
}

async fn load_object_neighbors(
    state: &AppState,
    tenant_id: &str,
    node: &BlueNode,
    nt: &NodeTypeSpec,
    doc: &SchemaDoc,
    mask_ctx: &DomainMaskCtx<'_>,
) -> Result<BTreeMap<String, Vec<DomainObjectRecord>>, AppError> {
    let sigs = xblue_graphql::compute_neighbor_sigs(nt, doc);
    if sigs.is_empty() {
        return Ok(BTreeMap::new());
    }
    let batch_keys: Vec<NeighborBatchKey> = sigs
        .iter()
        .map(|s| NeighborBatchKey {
            parent_id: node.id,
            link_type: s.link_type.clone(),
            direction: s.direction,
            target_type: s.target_type.clone(),
            limit: 50,
        })
        .collect();

    let mut loaded = state
        .store
        .batch_load_neighbors(tenant_id, &batch_keys)
        .await?;
    if !mask_ctx.policy.bypass_row_policies {
        let mut all_ids = Vec::new();
        for list in loaded.values() {
            for nb in list {
                all_ids.push(nb.id);
            }
        }
        if !all_ids.is_empty() {
            let visible = state
                .store
                .filter_visible_node_ids(tenant_id, &all_ids, &mask_ctx.policy.node_types)
                .await?;
            for list in loaded.values_mut() {
                list.retain(|nb| visible.contains(&nb.id));
            }
        }
    }

    let nt_by_name: BTreeMap<&str, &NodeTypeSpec> = doc
        .node_types
        .iter()
        .map(|n| (n.name.as_str(), n))
        .collect();

    let mut out = BTreeMap::new();
    for (sig, key) in sigs.iter().zip(batch_keys.iter()) {
        let target_spec = nt_by_name.get(sig.target_type.as_str()).copied();
        let recs: Vec<DomainObjectRecord> = loaded.get(key).map_or_default(|list| {
            list.iter()
                .map(|nb| blue_node_to_domain_record(nb, target_spec, mask_ctx, None))
                .collect()
        });
        out.insert(sig.field_name.clone(), recs);
    }
    Ok(out)
}

fn blue_node_to_domain_record(
    node: &BlueNode,
    nt: Option<&NodeTypeSpec>,
    mask_ctx: &DomainMaskCtx<'_>,
    neighbors: Option<BTreeMap<String, Vec<DomainObjectRecord>>>,
) -> DomainObjectRecord {
    DomainObjectRecord {
        id: node.id,
        type_name: node.type_name.clone(),
        node_key: node.node_key.clone(),
        title: node.title.clone(),
        attributes: xblue_graphql::mask_node_attributes_with_policy(
            node,
            nt,
            Some(mask_ctx.policy),
            mask_ctx.has_pii_raw,
            mask_ctx.tenant_key,
        ),
        metrics: node.metrics.clone(),
        geo: node.geo,
        updated_at: node.updated_at,
        neighbors,
    }
}

// ============================================================================
// Existing AxGraph Runtime Nodes, Links, Metrics, Topology & Entity Resolution
// ============================================================================

async fn list_nodes_handler(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    principal: AuthPrincipal,
    ApiQuery(query): ApiQuery<NodeListQuery>,
) -> Result<Json<NodeListResponse>, HttpError> {
    principal.require(Permission::DataRead)?;
    let client_ip = crate::routes::auth::extract_client_ip(&headers);
    let target_slug = query
        .subgraph
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("main");
    if target_slug != "main" {
        let _ = axpoint_server::policy::authorize_resource_for_store(
            &state.store,
            &state.cedar,
            &principal,
            Permission::DataRead,
            axpoint_domain::guard::GrantResourceKind::Subgraph,
            target_slug,
            &client_ip,
        )
        .await?;
    }
    if let Some(tn) = query
        .type_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let _ = axpoint_server::policy::authorize_resource_for_store(
            &state.store,
            &state.cedar,
            &principal,
            Permission::DataRead,
            axpoint_domain::guard::GrantResourceKind::NodeType,
            tn,
            &client_ip,
        )
        .await?;
    }

    let policy = axpoint_server::policy::build_policy_context_for_subgraph(
        &state.store,
        &state.cedar,
        &principal,
        target_slug,
        query.reveal_pii,
        &client_ip,
    )
    .await?;
    let mut resp = state
        .store
        .list_nodes_with_row_policies(
            &principal.tenant_id,
            query.clone(),
            (!policy.bypass_row_policies).then_some(&policy.node_types),
        )
        .await?;

    let tenant_key = state.mask_keys.tenant_key(&principal.tenant_id);
    for node in &mut resp.items {
        axpoint_server::policy::apply_node_property_masks(node, &policy, &tenant_key);
    }
    for node in &mut resp.nodes {
        axpoint_server::policy::apply_node_property_masks(node, &policy, &tenant_key);
    }

    if policy.reveal_pii_authorized {
        state.audit.record(AuditRecord {
            tenant_id: principal.tenant_id.clone(),
            user_id: actor_uuid(&principal),
            username: principal.username.clone(),
            action: AuditAction::QuerySql,
            resource_ref: format!("xblue:nodes:{}", query.type_name.as_deref().unwrap_or("*")),
            status_code: 200,
            client_ip,
            trace_id: current_trace_id(),
            detail_json: serde_json::json!({
                "raw_pii": true,
                "returned_rows": resp.items.len(),
            }),
            created_at: Utc::now(),
        });
        if let Err(err) = state.audit.flush().await {
            tracing::warn!(error = %err, "xblue nodes reveal_pii audit flush failed");
        }
    }

    Ok(Json(resp))
}

async fn upsert_node_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiJson(req): ApiJson<UpsertBlueNodeRequest>,
) -> Result<Json<BlueNode>, HttpError> {
    principal.enforce_no_prod_mutation()?;
    principal.require(Permission::StudioEdit)?;
    let node = state.store.upsert_node(&principal.tenant_id, req).await?;
    if let Err(err) = state
        .store
        .resolve_entity_links(
            &principal.tenant_id,
            ResolveEntitiesRequest {
                node_id: Some(node.id),
            },
        )
        .await
    {
        tracing::warn!(node_id = %node.id, error = %err, "resolve_entity_links failed");
    }
    Ok(Json(node))
}

async fn bulk_upsert_nodes_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiJson(req): ApiJson<BulkNodesRequest>,
) -> Result<Json<BulkNodesResponse>, HttpError> {
    principal.enforce_no_prod_mutation()?;
    principal.require(Permission::StudioEdit)?;
    let resp = state.store.bulk_upsert(&principal.tenant_id, req).await?;
    if resp.upserted_nodes > 0
        && let Err(err) = state
            .store
            .resolve_entity_links(&principal.tenant_id, ResolveEntitiesRequest::default())
            .await
    {
        tracing::warn!(error = %err, "resolve_entity_links after bulk_upsert failed");
    }
    Ok(Json(resp))
}

async fn search_nodes_handler(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    principal: AuthPrincipal,
    ApiQuery(query): ApiQuery<NodeSearchQuery>,
) -> Result<Json<NodeSearchResponse>, HttpError> {
    principal.require(Permission::DataRead)?;
    if query.q.trim().is_empty() {
        return Ok(Json(NodeSearchResponse { items: Vec::new() }));
    }
    let client_ip = crate::routes::auth::extract_client_ip(&headers);
    let policy = axpoint_server::policy::build_policy_context_for_store(
        &state.store,
        &state.cedar,
        &principal,
        false,
        &client_ip,
    )
    .await?;
    let query_vec = state
        .ai
        .embed_one(query.q.trim())
        .await
        .ok()
        .map(|(v, _)| v);
    let mut resp = state
        .store
        .search_nodes(&principal.tenant_id, query, query_vec.as_deref())
        .await?;
    if !policy.bypass_row_policies && !resp.items.is_empty() {
        let ids: Vec<Uuid> = resp.items.iter().map(|h| h.id).collect();
        let visible = state
            .store
            .filter_visible_node_ids(&principal.tenant_id, &ids, &policy.node_types)
            .await?;
        resp.items.retain(|h| visible.contains(&h.id));
    }
    let tenant_key = state.mask_keys.tenant_key(&principal.tenant_id);
    for hit in &mut resp.items {
        axpoint_server::policy::apply_search_hit_property_masks(hit, &policy, &tenant_key);
    }
    Ok(Json(resp))
}

async fn get_node_handler(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<BlueNode>, HttpError> {
    principal.require(Permission::DataRead)?;
    let mut node = state
        .store
        .get_node(&principal.tenant_id, id)
        .await?
        .ok_or(AppError::NotFound("NODE_NOT_FOUND"))?;
    let client_ip = crate::routes::auth::extract_client_ip(&headers);
    let _ = axpoint_server::policy::authorize_resource_for_store(
        &state.store,
        &state.cedar,
        &principal,
        Permission::DataRead,
        axpoint_domain::guard::GrantResourceKind::NodeType,
        &node.type_name,
        &client_ip,
    )
    .await?;
    let policy = axpoint_server::policy::build_policy_context_for_store(
        &state.store,
        &state.cedar,
        &principal,
        false,
        &client_ip,
    )
    .await?;
    if !policy.bypass_row_policies {
        let visible = state
            .store
            .filter_visible_node_ids(&principal.tenant_id, &[node.id], &policy.node_types)
            .await?;
        if !visible.contains(&node.id) {
            return Err(AppError::NotFound("NODE_NOT_FOUND").into());
        }
    }
    let tenant_key = state.mask_keys.tenant_key(&principal.tenant_id);
    axpoint_server::policy::apply_node_property_masks(&mut node, &policy, &tenant_key);
    Ok(Json(node))
}

async fn patch_node_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(req): ApiJson<PatchNodeRequest>,
) -> Result<Json<BlueNode>, HttpError> {
    principal.enforce_no_prod_mutation()?;
    principal.require(Permission::StudioEdit)?;
    let node = state
        .store
        .patch_node(&principal.tenant_id, id, req)
        .await?;
    Ok(Json(node))
}

async fn delete_node_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<serde_json::Value>, HttpError> {
    principal.enforce_no_prod_mutation()?;
    principal.require(Permission::StudioEdit)?;
    state.store.delete_node(&principal.tenant_id, id).await?;
    Ok(Json(serde_json::json!({ "deleted": true })))
}

async fn list_node_documents_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<NodeDocumentsResponse>, HttpError> {
    principal.require(Permission::DataRead)?;
    let resp = state
        .store
        .list_node_documents(&principal.tenant_id, id)
        .await?;
    Ok(Json(resp))
}

async fn query_node_metrics_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath((id, metric)): ApiPath<(Uuid, String)>,
    ApiQuery(query): ApiQuery<NodeMetricSeriesQuery>,
) -> Result<Json<Vec<MetricSeriesPoint>>, HttpError> {
    principal.require(Permission::DataRead)?;
    let points = state
        .store
        .query_node_metrics(&principal.tenant_id, id, &metric, query)
        .await?;
    Ok(Json(points))
}

async fn list_links_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiQuery(query): ApiQuery<LinkListQuery>,
) -> Result<Json<Vec<LinkRecord>>, HttpError> {
    principal.require(Permission::DataRead)?;
    let links = state.store.list_links(&principal.tenant_id, query).await?;
    Ok(Json(links))
}

async fn upsert_link_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiJson(req): ApiJson<UpsertLinkRequest>,
) -> Result<Json<LinkRecord>, HttpError> {
    principal.enforce_no_prod_mutation()?;
    principal.require(Permission::StudioEdit)?;
    let link = state.store.upsert_link(&principal.tenant_id, req).await?;
    Ok(Json(link))
}

async fn delete_link_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<serde_json::Value>, HttpError> {
    principal.enforce_no_prod_mutation()?;
    principal.require(Permission::StudioEdit)?;
    state.store.delete_link(&principal.tenant_id, id).await?;
    Ok(Json(serde_json::json!({ "deleted": true })))
}

async fn write_metrics_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiJson(req): ApiJson<WriteMetricsRequest>,
) -> Result<Json<WriteMetricsResponse>, HttpError> {
    principal.enforce_no_prod_mutation()?;
    principal.require(Permission::DataSync)?;
    let resp = state.store.write_metrics(&principal.tenant_id, req).await?;
    Ok(Json(resp))
}

async fn query_window_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiJson(query): ApiJson<WindowQuery>,
) -> Result<Json<WindowQueryResponse>, HttpError> {
    principal.require(Permission::DataRead)?;
    let resp = state
        .store
        .query_window(&principal.tenant_id, query)
        .await?;
    Ok(Json(resp))
}

async fn list_metric_catalog_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
) -> Result<Json<Vec<MetricCatalogEntry>>, HttpError> {
    principal.require(Permission::DataRead)?;
    let catalog = state
        .store
        .list_metric_catalog(&principal.tenant_id)
        .await?;
    Ok(Json(catalog))
}

async fn filter_and_mask_subgraph(
    state: &AppState,
    tenant_id: &str,
    resp: &mut SubgraphResponse,
    policy: &axpoint_domain::guard::PolicyContext,
    tenant_key: &[u8; 32],
) -> Result<(), AppError> {
    if !policy.bypass_row_policies && !resp.nodes.is_empty() {
        let ids: Vec<Uuid> = resp.nodes.iter().map(|n| n.id).collect();
        let visible = state
            .store
            .filter_visible_node_ids(tenant_id, &ids, &policy.node_types)
            .await?;
        resp.nodes.retain(|n| visible.contains(&n.id));
        resp.links
            .retain(|l| visible.contains(&l.src_node_id) && visible.contains(&l.dst_node_id));
    }
    for node in &mut resp.nodes {
        axpoint_server::policy::apply_node_property_masks(node, policy, tenant_key);
    }
    Ok(())
}

async fn query_neighbors_handler(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    principal: AuthPrincipal,
    ApiQuery(query): ApiQuery<NeighborQuery>,
) -> Result<Json<SubgraphResponse>, HttpError> {
    principal.require(Permission::DataRead)?;
    let client_ip = crate::routes::auth::extract_client_ip(&headers);
    let policy = axpoint_server::policy::build_policy_context_for_store(
        &state.store,
        &state.cedar,
        &principal,
        false,
        &client_ip,
    )
    .await?;
    let mut resp = state
        .store
        .query_neighbors(&principal.tenant_id, query)
        .await?;
    let tenant_key = state.mask_keys.tenant_key(&principal.tenant_id);
    filter_and_mask_subgraph(
        &state,
        &principal.tenant_id,
        &mut resp,
        &policy,
        &tenant_key,
    )
    .await?;
    Ok(Json(resp))
}

async fn query_path_handler(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    principal: AuthPrincipal,
    ApiQuery(query): ApiQuery<PathQuery>,
) -> Result<Json<PathResponse>, HttpError> {
    principal.require(Permission::DataRead)?;
    let client_ip = crate::routes::auth::extract_client_ip(&headers);
    let policy = axpoint_server::policy::build_policy_context_for_store(
        &state.store,
        &state.cedar,
        &principal,
        false,
        &client_ip,
    )
    .await?;
    let mut resp = state
        .store
        .query_shortest_path(&principal.tenant_id, query)
        .await?;
    if !policy.bypass_row_policies && !resp.path.is_empty() {
        let visible = state
            .store
            .filter_visible_node_ids(&principal.tenant_id, &resp.path, &policy.node_types)
            .await?;
        if resp.path.iter().any(|id| !visible.contains(id)) {
            resp.path.clear();
            resp.nodes.clear();
            resp.links.clear();
            resp.total_cost = None;
        }
    }
    let tenant_key = state.mask_keys.tenant_key(&principal.tenant_id);
    for node in &mut resp.nodes {
        axpoint_server::policy::apply_node_property_masks(node, &policy, &tenant_key);
    }
    Ok(Json(resp))
}

async fn query_subgraph_handler(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    principal: AuthPrincipal,
    ApiQuery(query): ApiQuery<SubgraphQuery>,
) -> Result<Json<SubgraphResponse>, HttpError> {
    principal.require(Permission::DataRead)?;
    let client_ip = crate::routes::auth::extract_client_ip(&headers);
    let target_slug = query
        .subgraph
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("main");
    if target_slug != "main" {
        let _ = axpoint_server::policy::authorize_resource_for_store(
            &state.store,
            &state.cedar,
            &principal,
            Permission::DataRead,
            axpoint_domain::guard::GrantResourceKind::Subgraph,
            target_slug,
            &client_ip,
        )
        .await?;
    }
    let policy = axpoint_server::policy::build_policy_context_for_subgraph(
        &state.store,
        &state.cedar,
        &principal,
        target_slug,
        false,
        &client_ip,
    )
    .await?;
    let mut resp = state
        .store
        .query_subgraph(&principal.tenant_id, query)
        .await?;
    let tenant_key = state.mask_keys.tenant_key(&principal.tenant_id);
    filter_and_mask_subgraph(
        &state,
        &principal.tenant_id,
        &mut resp,
        &policy,
        &tenant_key,
    )
    .await?;
    Ok(Json(resp))
}

async fn run_algorithm_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(algo_raw): ApiPath<String>,
    ApiJson(req): ApiJson<AlgoRunRequest>,
) -> Result<Json<AlgoRunResponse>, HttpError> {
    principal.require(Permission::DataRead)?;
    let algo = GraphAlgo::from_str(algo_raw.trim())?;
    let resp = state
        .store
        .run_algorithm(&principal.tenant_id, algo, req)
        .await?;
    Ok(Json(resp))
}

async fn list_algorithm_results_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(algo_raw): ApiPath<String>,
    ApiQuery(query): ApiQuery<AlgoResultsQuery>,
) -> Result<Json<AlgoResultsResponse>, HttpError> {
    principal.require(Permission::DataRead)?;
    let algo = GraphAlgo::from_str(algo_raw.trim())?;
    let resp = state
        .store
        .list_algorithm_results(&principal.tenant_id, algo, query)
        .await?;
    Ok(Json(resp))
}

async fn resolve_entities_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiJson(req): ApiJson<ResolveEntitiesRequest>,
) -> Result<Json<ResolveEntitiesResponse>, HttpError> {
    principal.enforce_no_prod_mutation()?;
    principal.require(Permission::StudioEdit)?;
    let resp = state
        .store
        .resolve_entity_links(&principal.tenant_id, req)
        .await?;
    Ok(Json(resp))
}

async fn list_entity_candidates_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
) -> Result<Json<EntityCandidatesResponse>, HttpError> {
    principal.require(Permission::DataRead)?;
    let resp = state
        .store
        .list_entity_candidates(&principal.tenant_id)
        .await?;
    Ok(Json(resp))
}

async fn confirm_entity_link_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath((entity_id, node_id)): ApiPath<(Uuid, Uuid)>,
) -> Result<Json<EntityLink>, HttpError> {
    principal.enforce_no_prod_mutation()?;
    principal.require(Permission::StudioEdit)?;
    let link = state
        .store
        .confirm_entity_link(&principal.tenant_id, entity_id, node_id)
        .await?;
    Ok(Json(link))
}

async fn reject_entity_link_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath((entity_id, node_id)): ApiPath<(Uuid, Uuid)>,
) -> Result<Json<EntityLink>, HttpError> {
    principal.enforce_no_prod_mutation()?;
    principal.require(Permission::StudioEdit)?;
    let link = state
        .store
        .reject_entity_link(&principal.tenant_id, entity_id, node_id)
        .await?;
    Ok(Json(link))
}

async fn validate_expr_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiQuery(q): ApiQuery<SubgraphQueryParam>,
    ApiJson(req): ApiJson<ValidateExprRequest>,
) -> Result<Json<ValidateExprResponse>, HttpError> {
    principal.require(Permission::StudioEdit)?;
    let slug = req
        .subgraph
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| q.slug_or_main());

    let target_type = req.type_name.trim();
    let published = state
        .store
        .get_published_schema(&principal.tenant_id, slug)
        .await?;

    let mut found_nt = published
        .doc
        .node_types
        .into_iter()
        .find(|nt| nt.name == target_type);

    if found_nt.is_none()
        && slug != "main"
        && let Ok(main_pub) = state
            .store
            .get_published_schema(&principal.tenant_id, "main")
            .await
    {
        found_nt = main_pub
            .doc
            .node_types
            .into_iter()
            .find(|nt| nt.name == target_type);
    }

    let nt = found_nt.ok_or(AppError::NotFound("TYPE_NOT_FOUND"))?;
    Ok(Json(validate_for_node_type(&nt, &req.expression)))
}

async fn evaluate_expr_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiJson(req): ApiJson<EvaluateExprRequest>,
) -> Result<Json<EvaluateExprResponse>, HttpError> {
    principal.require(Permission::DataRead)?;
    let node = state
        .store
        .get_node(&principal.tenant_id, req.node_id)
        .await?
        .ok_or(AppError::NotFound("NODE_NOT_FOUND"))?;

    let ast = parse_expr(&req.expression)
        .map_err(|issue| AppError::bad("EXPR_INVALID", issue.message))?;
    let env = EvalEnv::from_node(&node.metrics, &node.properties);
    let rep = eval_expr(&ast, &env);

    Ok(Json(EvaluateExprResponse {
        value: rep.value.to_json_value(),
        value_type: rep.value.expr_type(),
        warnings: rep.warnings,
    }))
}

async fn recompute_aggregate_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    body: Bytes,
) -> Result<Json<RecomputeAggregateResponse>, HttpError> {
    principal.enforce_no_prod_mutation()?;
    principal.require(Permission::StudioEdit)?;
    let req: RecomputeAggregateRequest = parse_optional_json(&body)?;
    let clean_type = req
        .type_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from);

    let published = state
        .store
        .get_published_schema(&principal.tenant_id, "main")
        .await?;

    if let Some(ref tn) = clean_type
        && !published.doc.node_types.iter().any(|nt| nt.name == *tn)
    {
        return Err(AppError::NotFound("TYPE_NOT_FOUND").into());
    }

    let job_id = state
        .store
        .enqueue_graph_aggregate(
            &principal.tenant_id,
            GraphAggregateJobPayload {
                full: true,
                full_type: clean_type.clone(),
                node_ids: Vec::new(),
                metrics: Vec::new(),
                schema_version: Some(published.version),
            },
            None,
        )
        .await?;

    Ok(Json(RecomputeAggregateResponse {
        job_id,
        type_name: clean_type,
    }))
}

async fn explain_node_metric_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath((id, metric)): ApiPath<(Uuid, String)>,
) -> Result<Json<MetricExplainResponse>, HttpError> {
    principal.require(Permission::DataRead)?;
    let resp = state
        .store
        .explain_node_metric(&principal.tenant_id, id, &metric)
        .await?;
    Ok(Json(resp))
}

async fn list_knowledge_node_candidates_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
) -> Result<Json<KnowledgeNodeCandidatesResponse>, HttpError> {
    principal.require(Permission::DataRead)?;
    let resp = state
        .store
        .list_knowledge_node_candidates(&principal.tenant_id)
        .await?;
    Ok(Json(resp))
}

async fn accept_knowledge_node_candidate_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(entity_id): ApiPath<Uuid>,
    ApiJson(req): ApiJson<AcceptNodeCandidateRequest>,
) -> Result<Json<AcceptNodeCandidateResponse>, HttpError> {
    principal.enforce_no_prod_mutation()?;
    principal.require(Permission::StudioEdit)?;
    let resp = state
        .store
        .accept_knowledge_node_candidate(&principal.tenant_id, entity_id, req)
        .await?;

    if let Ok(main_pub) = state
        .store
        .get_published_schema(&principal.tenant_id, "main")
        .await
        && main_pub
            .doc
            .node_types
            .iter()
            .any(|nt| nt.name == resp.node.type_name && nt.sync_to_knowledge)
        && let Ok(payload_json) = serde_json::to_value(KnowledgeSyncJobPayload {
            node_id: Some(resp.node.id),
            ..KnowledgeSyncJobPayload::default()
        })
    {
        let job = NewJob::new(JobKind::KnowledgeSync, payload_json)
            .with_idempotency_key(format!("ksync_accept:{}", resp.node.id));
        if let Err(err) = state.store.enqueue(&principal.tenant_id, job).await {
            tracing::warn!(node_id = %resp.node.id, error = %err, "enqueue KnowledgeSync job failed");
        }
    }

    Ok(Json(resp))
}

async fn invoke_object_action_handler(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    principal: AuthPrincipal,
    ApiPath((type_name, node_key, action_name)): ApiPath<(String, String, String)>,
    ApiJson(req): ApiJson<InvokeObjectActionRequest>,
) -> Result<(StatusCode, Json<InvokeObjectActionResponse>), HttpError> {
    principal.enforce_no_prod_mutation()?;
    principal.require(Permission::FlowRun)?;

    let client_ip = crate::routes::auth::extract_client_ip(&headers);
    let policy = authorize_object_action_target(
        &state,
        &principal,
        &client_ip,
        type_name.trim(),
        node_key.trim(),
        action_name.trim(),
    )
    .await?;

    let payload_sha256 = {
        use sha2::{Digest as _, Sha256};
        hex::encode(Sha256::digest(req.inputs.to_string().as_bytes()))
    };
    let idem_key = req.idempotency_key.clone();
    let mut resp = state
        .store
        .invoke_object_action(
            &principal.tenant_id,
            actor_uuid(&principal),
            &type_name,
            &node_key,
            &action_name,
            req,
        )
        .await?;

    mask_action_response_node(&state, &principal, &policy, &mut resp).await;

    let status = if resp.status == "WAITING_APPROVAL" {
        StatusCode::ACCEPTED
    } else {
        StatusCode::OK
    };
    state.audit.record(AuditRecord {
        tenant_id: principal.tenant_id.clone(),
        user_id: actor_uuid(&principal),
        username: principal.username.clone(),
        action: AuditAction::ObjectActionInvoke,
        resource_ref: format!(
            "{}:{}:{}",
            type_name.trim(),
            node_key.trim(),
            action_name.trim()
        ),
        status_code: i32::from(status.as_u16()),
        client_ip,
        trace_id: current_trace_id(),
        detail_json: serde_json::json!({
            "channel": "REST",
            "status": &resp.status,
            "idempotency_key": idem_key,
            "idempotency_replay": resp.idempotency_replay,
            "payload_sha256": payload_sha256,
        }),
        created_at: Utc::now(),
    });
    if let Err(err) = state.audit.flush().await {
        tracing::warn!(error = %err, "rest object action audit flush failed");
    }
    Ok((status, Json(resp)))
}

async fn authorize_object_action_target(
    state: &AppState,
    principal: &AuthPrincipal,
    client_ip: &str,
    type_name: &str,
    node_key: &str,
    action_name: &str,
) -> Result<axpoint_domain::guard::PolicyContext, AppError> {
    let published = state
        .store
        .get_published_schema(&principal.tenant_id, "main")
        .await?;
    let nt = published
        .doc
        .node_types
        .iter()
        .find(|n| n.name == type_name)
        .ok_or(AppError::NotFound("TYPE_NOT_FOUND"))?;
    let action = nt
        .actions
        .iter()
        .find(|a| {
            a.name == action_name && a.maturity == axpoint_domain::enums::DesignMaturity::Confirmed
        })
        .ok_or(AppError::NotFound("ACTION_NOT_FOUND"))?;
    let req_perm = axpoint_domain::studio::resolve_action_permission(&action.required_permission)
        .map_err(|_| AppError::Forbidden("PERMISSION_DENIED"))?;
    principal.require(req_perm)?;

    let _ = axpoint_server::policy::authorize_resource_for_store(
        &state.store,
        &state.cedar,
        principal,
        Permission::FlowRun,
        axpoint_domain::guard::GrantResourceKind::NodeType,
        type_name,
        client_ip,
    )
    .await?;
    let policy = axpoint_server::policy::build_policy_context_for_store(
        &state.store,
        &state.cedar,
        principal,
        false,
        client_ip,
    )
    .await?;
    if !policy.bypass_row_policies
        && let Some(target_node) = state
            .store
            .get_domain_object_by_key(&principal.tenant_id, type_name, node_key)
            .await?
    {
        let visible = state
            .store
            .filter_visible_node_ids(&principal.tenant_id, &[target_node.id], &policy.node_types)
            .await?;
        if !visible.contains(&target_node.id) {
            return Err(AppError::Forbidden("PERMISSION_DENIED"));
        }
    }
    Ok(policy)
}

async fn mask_action_response_node(
    state: &AppState,
    principal: &AuthPrincipal,
    policy: &axpoint_domain::guard::PolicyContext,
    resp: &mut InvokeObjectActionResponse,
) {
    let Some(ref mut n) = resp.node else {
        return;
    };
    let published = state
        .store
        .get_published_schema(&principal.tenant_id, "main")
        .await
        .ok();
    let nt_opt = published
        .as_ref()
        .and_then(|p| p.doc.node_types.iter().find(|t| t.name == n.type_name));
    let hmac_key = state.mask_keys.tenant_key(&principal.tenant_id);
    n.properties = xblue_graphql::mask_node_attributes_with_policy(
        n,
        nt_opt,
        Some(policy),
        principal.has(Permission::PiiRawRead),
        &hmac_key,
    );
}

async fn certify_subgraph_handler(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    principal: AuthPrincipal,
    ApiPath(slug): ApiPath<String>,
    body: Bytes,
) -> Result<Json<RuntimeCertReport>, HttpError> {
    principal.require(Permission::StudioEdit)?;
    let client_ip = crate::routes::auth::extract_client_ip(&headers);
    let clean_slug = slug.trim();
    if clean_slug != "main" {
        let _ = axpoint_server::policy::authorize_resource_for_store(
            &state.store,
            &state.cedar,
            &principal,
            Permission::StudioEdit,
            axpoint_domain::guard::GrantResourceKind::Subgraph,
            clean_slug,
            &client_ip,
        )
        .await?;
    }
    let req: CertifySubgraphRequest = if body.is_empty() || body.iter().all(u8::is_ascii_whitespace)
    {
        CertifySubgraphRequest::default()
    } else {
        serde_json::from_slice(&body).map_err(|err| {
            AppError::bad("INVALID_ARGUMENT", format!("invalid JSON payload: {err}"))
        })?
    };
    let report = state
        .store
        .certify_subgraph_runtime(
            &principal.tenant_id,
            clean_slug,
            actor_uuid(&principal),
            req,
        )
        .await?;
    state.audit.record(AuditRecord {
        tenant_id: principal.tenant_id.clone(),
        user_id: actor_uuid(&principal),
        username: principal.username.clone(),
        action: AuditAction::TrialTickCertify,
        resource_ref: report.subgraph_slug.clone(),
        status_code: 200,
        client_ip,
        trace_id: current_trace_id(),
        detail_json: serde_json::json!({
            "cert_id": report.id,
            "cert_level": report.cert_level,
            "readiness_score": report.readiness_score,
            "trial_tick_passed": report.trial_tick_passed,
            "dead_knowledge_cnt": report.dead_knowledge_cnt,
        }),
        created_at: Utc::now(),
    });
    if let Err(err) = state.audit.flush().await {
        tracing::warn!(error = %err, "certify_subgraph audit flush failed");
    }
    Ok(Json(report))
}

async fn explain_link_metric_handler(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    principal: AuthPrincipal,
    ApiPath((link_id, metric)): ApiPath<(Uuid, String)>,
) -> Result<Json<LinkMetricExplainResponse>, HttpError> {
    principal.require(Permission::DataRead)?;
    let (subgraph_slug, src_id, src_type_opt, dst_id, dst_type_opt) = state
        .store
        .get_link_authz_meta(&principal.tenant_id, link_id)
        .await?
        .ok_or(AppError::NotFound("LINK_NOT_FOUND"))?;
    authorize_subgraph_read(&state, &headers, &principal, &subgraph_slug).await?;

    let client_ip = crate::routes::auth::extract_client_ip(&headers);
    for nt_name in [src_type_opt.as_deref(), dst_type_opt.as_deref()]
        .into_iter()
        .flatten()
    {
        let _ = axpoint_server::policy::authorize_resource_for_store(
            &state.store,
            &state.cedar,
            &principal,
            Permission::DataRead,
            axpoint_domain::guard::GrantResourceKind::NodeType,
            nt_name,
            &client_ip,
        )
        .await?;
    }

    let policy = axpoint_server::policy::build_policy_context_for_store(
        &state.store,
        &state.cedar,
        &principal,
        false,
        &client_ip,
    )
    .await?;
    if !policy.bypass_row_policies {
        let visible = state
            .store
            .filter_visible_node_ids(&principal.tenant_id, &[src_id, dst_id], &policy.node_types)
            .await?;
        if !visible.contains(&src_id) || !visible.contains(&dst_id) {
            return Err(AppError::NotFound("LINK_NOT_FOUND").into());
        }
    }

    let resp = state
        .store
        .explain_link_metric(&principal.tenant_id, link_id, &metric)
        .await?;
    Ok(Json(resp))
}

async fn write_link_metrics_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiJson(req): ApiJson<WriteLinkMetricsRequest>,
) -> Result<Json<WriteLinkMetricsResponse>, HttpError> {
    principal.enforce_no_prod_mutation()?;
    principal.require(Permission::DataSync)?;
    let resp = state
        .store
        .write_link_metrics(&principal.tenant_id, req)
        .await?;
    Ok(Json(resp))
}

async fn compute_convex_hull_handler(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    principal: AuthPrincipal,
    ApiJson(req): ApiJson<axpoint_domain::geo_ops::ConvexHullRequest>,
) -> Result<Json<axpoint_domain::geo_ops::ConvexHullResponse>, HttpError> {
    principal.require(Permission::DataRead)?;
    for pt in &req.points {
        axpoint_domain::geo_ops::validate_geo_point(*pt)?;
    }

    let mut all_points = req.points.clone();
    let should_query_nodes = req.points.is_empty()
        || req.type_name.is_some()
        || !req.node_ids.is_empty()
        || req.subgraph.is_some();

    if should_query_nodes {
        let target_slug = req
            .subgraph
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("main");
        authorize_subgraph_read(&state, &headers, &principal, target_slug).await?;
        let client_ip = crate::routes::auth::extract_client_ip(&headers);
        if let Some(tn) = req
            .type_name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            let _ = axpoint_server::policy::authorize_resource_for_store(
                &state.store,
                &state.cedar,
                &principal,
                Permission::DataRead,
                axpoint_domain::guard::GrantResourceKind::NodeType,
                tn,
                &client_ip,
            )
            .await?;
        }
        let policy = axpoint_server::policy::build_policy_context_for_subgraph(
            &state.store,
            &state.cedar,
            &principal,
            target_slug,
            false,
            &client_ip,
        )
        .await?;
        let db_points = state
            .store
            .collect_governed_geo_points(
                &principal.tenant_id,
                target_slug,
                req.type_name.as_deref(),
                &req.node_ids,
                (!policy.bypass_row_policies).then_some(&policy.node_types),
            )
            .await?;
        all_points.extend(db_points);
    }

    let resp = axpoint_domain::geo_ops::compute_convex_hull(&all_points)?;
    Ok(Json(resp))
}

async fn get_competency_suite_handler(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    principal: AuthPrincipal,
    ApiPath(slug): ApiPath<String>,
) -> Result<Json<axpoint_domain::builder::CompetencySuiteRecord>, HttpError> {
    principal.require(Permission::DataRead)?;
    let clean_slug = slug.trim();
    authorize_subgraph_read(&state, &headers, &principal, clean_slug).await?;
    let suite = state
        .store
        .get_competency_suite(&principal.tenant_id, clean_slug)
        .await?;
    Ok(Json(suite))
}

async fn put_competency_suite_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(slug): ApiPath<String>,
    ApiJson(req): ApiJson<axpoint_domain::builder::UpsertCompetencySuiteRequest>,
) -> Result<Json<axpoint_domain::builder::CompetencySuiteRecord>, HttpError> {
    principal.require(Permission::StudioEdit)?;
    let suite = state
        .store
        .upsert_competency_suite(
            &principal.tenant_id,
            slug.trim(),
            actor_uuid(&principal),
            req,
        )
        .await?;
    Ok(Json(suite))
}

async fn verify_competency_suite_handler(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    principal: AuthPrincipal,
    ApiPath(slug): ApiPath<String>,
    body: Bytes,
) -> Result<Json<axpoint_domain::builder::HarnessRunRecord>, HttpError> {
    principal.require(Permission::StudioEdit)?;
    principal.require(Permission::DataRead)?;
    let clean_slug = slug.trim();
    authorize_subgraph_read(&state, &headers, &principal, clean_slug).await?;
    let req: axpoint_domain::builder::VerifyCompetencySuiteRequest =
        if body.is_empty() || body.iter().all(u8::is_ascii_whitespace) {
            axpoint_domain::builder::VerifyCompetencySuiteRequest::default()
        } else {
            serde_json::from_slice(&body).map_err(|err| {
                AppError::bad("INVALID_ARGUMENT", format!("invalid JSON payload: {err}"))
            })?
        };
    let mut perms = principal.permissions.clone();
    perms.remove(&Permission::PiiRawRead);
    let analytics_principal = axpoint_domain::analytics::AnalyticsPrincipal {
        user_id: principal.user_id,
        tenant_id: principal.tenant_id.clone(),
        username: principal.username.clone(),
        permissions: perms,
    };
    let run = axpoint_domain::ports::BuilderHarnessPort::verify_competency_suite(
        state.axagent.capability.as_ref(),
        &analytics_principal,
        actor_uuid(&principal),
        &principal.username,
        clean_slug,
        req,
    )
    .await?;
    Ok(Json(run))
}

async fn list_harness_runs_handler(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    principal: AuthPrincipal,
    ApiPath(slug): ApiPath<String>,
) -> Result<Json<serde_json::Value>, HttpError> {
    principal.require(Permission::DataRead)?;
    let clean_slug = slug.trim();
    authorize_subgraph_read(&state, &headers, &principal, clean_slug).await?;
    let items = state
        .store
        .list_harness_runs(&principal.tenant_id, clean_slug, 20)
        .await?;
    Ok(Json(serde_json::json!({ "items": items })))
}

#[cfg(test)]
mod tests {
    use axpoint_domain::studio::EdgeTypeSpec;

    use super::*;

    #[test]
    fn packaged_agent_spec_binds_endpoints_and_graph_neighbors_tool() {
        let _router = router();
        let wh = NodeTypeSpec {
            name: String::from("Warehouse"),
            display_name: String::from("仓库"),
            ..NodeTypeSpec::default()
        };
        let doc = SchemaDoc {
            node_types: vec![wh.clone()],
            edge_types: vec![EdgeTypeSpec {
                name: String::from("FULFILLS"),
                src_type_name: String::from("Warehouse"),
                dst_type_name: String::from("Order"),
                ..EdgeTypeSpec::default()
            }],
            mappings: Vec::new(),
        };
        let ep_id = Uuid::from_u128(0x99);
        let ep = EndpointSpec {
            id: ep_id,
            tenant_id: String::from("default"),
            slug: String::from("warehouse-objects"),
            environment: Environment::Prod,
            kind: EndpointKind::ObjectType,
            target_id: String::from("Warehouse"),
            target_version: Some(1),
            target_snapshot: None,
            title: String::from("仓库对象"),
            description: String::new(),
            input_schema: serde_json::json!({"type": "object"}),
            output_shape: OutputShape::Object,
            required_scope: Permission::DataRead,
            rate_limit_rpm: 120,
            cache_ttl_s: 0,
            expose_mcp: true,
            show_as_app: true,
            enabled: true,
            published_by: None,
            published_at: Utc::now(),
        };
        let selected = vec![&wh];
        let spec = build_packaged_agent_spec("main", "企业主图", &selected, &doc, &[], &[ep]);
        assert!(spec.system_prompt.contains("严禁心算"));
        assert!(spec.system_prompt.contains("Warehouse"));
        assert!(spec.system_prompt.contains("FULFILLS"));
        assert_eq!(
            spec.tools,
            vec![
                ToolRef::Endpoint { endpoint_id: ep_id },
                ToolRef::Builtin {
                    name: String::from("graph_neighbors")
                }
            ]
        );
    }
}
