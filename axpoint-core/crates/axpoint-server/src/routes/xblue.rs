//! `xBlueStudio` modeling, versioned schema, drafts, subgraphs, dataset mappings, dynamic GraphQL,
//! domain objects, and `AxGraph` runtime topology routes (`/api/xblue/*`).

#[path = "xblue_graphql.rs"]
mod xblue_graphql;

use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;

use axpoint_db::studio_repo::NeighborBatchKey;
use axpoint_domain::AppError;
use axpoint_domain::analytics::QueryRequest;
use axpoint_domain::axhub::{Dataset, PaginatedList};
use axpoint_domain::enums::{AuditAction, Cardinality, DataType, GraphAlgo, Permission};
use axpoint_domain::graph::{
    AlgoResultsQuery, AlgoResultsResponse, AlgoRunRequest, AlgoRunResponse, BulkNodesRequest,
    BulkNodesResponse, EntityCandidatesResponse, EntityLink, GeoPoint, LinkListQuery, LinkRecord,
    MetricCatalogEntry, MetricSeriesPoint, NeighborQuery, NodeDocumentsResponse, NodeListQuery,
    NodeListResponse, NodeMetricSeriesQuery, NodeSearchQuery, NodeSearchResponse, PatchNodeRequest,
    PathQuery, PathResponse, ResolveEntitiesRequest, ResolveEntitiesResponse, SubgraphQuery,
    SubgraphResponse, UpsertLinkRequest, WindowQuery, WindowQueryResponse, WriteMetricsRequest,
    WriteMetricsResponse,
};
use axpoint_domain::platform::AuditRecord;
use axpoint_domain::ports::{
    DatasetRepository as _, GraphQueryPort as _, GraphStorePort as _, MetricStorePort as _,
    StudioRepository as _,
};
use axpoint_domain::studio::{
    AiDraftRequest, ColumnProfileStats, ColumnRoleTag, CreateDraftRequest, CreateSubgraphRequest,
    DatasetProfileSummary, DomainObjectItemQuery, DomainObjectListQuery, DomainObjectListResponse,
    DomainObjectRecord, DraftSource, DraftStatus, FkCandidate, IssueSeverity,
    MappingPreviewNodeRow, MappingPreviewResponse, MappingRecord, MappingRejectSample,
    MappingRunsResponse, MergePreview, MergePreviewRequest, MergeSubgraphRequest, NodeTypeSpec,
    ObjectCatalogResponse, ObjectFieldSig, ObjectTypeCatalogItem, ProfilingBundle,
    PublishDraftRequest, PublishDraftResponse, PublishedSchemaResponse, SaveSchemaDirectRequest,
    SchemaDiff, SchemaDoc, SchemaDraft, SchemaVersionDetail, SchemaVersionSummary, SubgraphInfo,
    SubgraphQueryParam, TriggerMappingRunRequest, TriggerMappingRunResponse, UpdateDraftRequest,
    UpdateSubgraphRequest, ValidateSchemaResponse, validate,
};
use axpoint_domain::topology::{BlueNode, UpsertBlueNodeRequest};
use axpoint_llm::{SchemaDraftInput, generate_schema_draft};
use axpoint_security::AuthPrincipal;
use axum::body::Bytes;
use axum::extract::State;
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
            "/xblue/subgraphs/{slug}",
            patch(update_subgraph_handler).delete(delete_subgraph_handler),
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
            "/xblue/schema",
            get(get_schema_handler).post(save_schema_handler),
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
            get(get_draft_handler)
                .put(update_draft_handler)
                .patch(update_draft_handler),
        )
        .route("/xblue/drafts/{id}/discard", post(discard_draft_handler))
        .route("/xblue/drafts/{id}/publish", post(publish_draft_handler))
        .route("/xblue/mappings", get(list_mappings_handler))
        .route("/xblue/mappings/{id}", get(get_mapping_handler))
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
        .route("/xblue/graphql", post(xblue_graphql::graphql_handler))
        .merge(runtime_graph_router())
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
            "/xblue/entities/links/{entity_id}/{node_id}/confirm",
            post(confirm_entity_link_handler),
        )
        .route(
            "/xblue/entities/links/{entity_id}/{node_id}/reject",
            post(reject_entity_link_handler),
        )
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
    let _ = state.audit.flush().await;
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
    principal.require(Permission::StudioEdit)?;
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
    principal.require(Permission::DataRead)?;
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

// ============================================================================
// Published Schema, Validation, Versions, Diff & Restore
// ============================================================================

async fn get_schema_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiQuery(sub): ApiQuery<SubgraphQueryParam>,
) -> Result<Json<PublishedSchemaResponse>, HttpError> {
    principal.require(Permission::DataRead)?;
    let resp = state
        .store
        .get_published_schema(&principal.tenant_id, sub.slug_or_main())
        .await?;
    Ok(Json(resp))
}

async fn save_schema_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiQuery(sub): ApiQuery<SubgraphQueryParam>,
    ApiJson(req): ApiJson<SaveSchemaDirectRequest>,
) -> Result<Json<PublishedSchemaResponse>, HttpError> {
    principal.require(Permission::StudioEdit)?;
    let slug = sub.slug_or_main();
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
    principal.require(Permission::DataRead)?;
    let doc = payload.schema_doc.unwrap_or(payload.flat_doc);
    let ctx = state
        .store
        .load_validation_context(&principal.tenant_id, sub.slug_or_main())
        .await?;
    let issues = validate(&doc, &ctx);
    let valid = !issues.iter().any(|i| i.severity == IssueSeverity::Error);
    Ok(Json(ValidateSchemaResponse { valid, issues }))
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
    principal.require(Permission::DataRead)?;
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
    principal.require(Permission::DataRead)?;
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
    let draft = state
        .store
        .create_draft(
            &principal.tenant_id,
            sub.slug_or_main(),
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
    principal.require(Permission::StudioPublish)?;
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

    let generated = generate_schema_draft(
        &state.ai,
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
            },
        )
        .await?;
    Ok(Json(draft))
}

async fn build_profiling_bundle(
    state: &AppState,
    tenant_id: &str,
    dataset_ids: &[Uuid],
) -> Result<ProfilingBundle, AppError> {
    if dataset_ids.is_empty() {
        return Ok(ProfilingBundle::default());
    }
    let session_opt = acquire_tenant_session(state, tenant_id, false).await.ok();
    let mut summaries = Vec::with_capacity(dataset_ids.len());
    let mut distinct_sets: BTreeMap<(String, String), BTreeSet<String>> = BTreeMap::new();

    for &ds_id in dataset_ids {
        let ds = state
            .store
            .get_dataset(tenant_id, ds_id)
            .await?
            .ok_or(AppError::NotFound("DATASET_NOT_FOUND"))?;
        let summary = profile_single_dataset(
            &ds,
            session_opt.as_ref().map(|(s, _)| s.as_ref()),
            &mut distinct_sets,
        )
        .await;
        summaries.push(summary);
    }

    let fk_candidates = infer_fk_candidates(&summaries, &distinct_sets);
    Ok(ProfilingBundle {
        datasets: summaries,
        fk_candidates,
    })
}

async fn profile_single_dataset(
    ds: &Dataset,
    session_opt: Option<&axpoint_analytics::TenantDb>,
    distinct_sets: &mut BTreeMap<(String, String), BTreeSet<String>>,
) -> DatasetProfileSummary {
    let sampled_res = match session_opt {
        Some(sess) => {
            let escaped = ds.name.replace('"', "\"\"");
            sess.execute_sql(
                &QueryRequest {
                    sql: format!("SELECT * FROM \"{escaped}\" LIMIT 5000"),
                    params: BTreeMap::new(),
                    max_rows: Some(5000),
                    timeout_ms: Some(5000),
                    reveal_pii: false,
                    allow_live: false,
                },
                false,
                false,
            )
            .await
            .ok()
        }
        None => None,
    };

    if let Some(out) = sampled_res
        && !out.result.rows.is_empty()
    {
        return summarize_sampled_rows(ds, &out.result, distinct_sets);
    }

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

fn summarize_sampled_rows(
    ds: &Dataset,
    qr: &axpoint_domain::analytics::QueryResult,
    distinct_sets: &mut BTreeMap<(String, String), BTreeSet<String>>,
) -> DatasetProfileSummary {
    let total = qr.rows.len();
    let sampled_count = u64::try_from(total).unwrap_or(0);
    let sampled_denom = f64::from(u32::try_from(total).unwrap_or(1));

    let mut columns = Vec::with_capacity(qr.columns.len());
    let mut pk_candidates: Vec<String> = Vec::new();

    for col_meta in &qr.columns {
        let mut nulls: u32 = 0;
        let mut seen_vals: BTreeSet<String> = BTreeSet::new();
        let mut samples: Vec<String> = Vec::new();

        for row in &qr.rows {
            match row.get(&col_meta.name) {
                None | Some(serde_json::Value::Null) => {
                    nulls = nulls.saturating_add(1);
                }
                Some(val) => {
                    let s = json_scalar_display(val);
                    if seen_vals.insert(s.clone()) && samples.len() < 8 {
                        samples.push(s);
                    }
                }
            }
        }

        let distinct_count = u64::try_from(seen_vals.len()).unwrap_or(0);
        let null_rate = (f64::from(nulls) / sampled_denom).clamp(0.0, 1.0);
        let mut roles = BTreeSet::new();
        if (nulls == 0 && distinct_count == sampled_count && sampled_count > 0)
            || ds.primary_key.contains(&col_meta.name)
        {
            roles.insert(ColumnRoleTag::PkCandidate);
            pk_candidates.push(col_meta.name.clone());
        }
        if matches!(
            col_meta.data_type,
            DataType::Int64 | DataType::Float64 | DataType::Decimal
        ) && !roles.contains(&ColumnRoleTag::PkCandidate)
        {
            roles.insert(ColumnRoleTag::NumericMetric);
        }
        if matches!(col_meta.data_type, DataType::Date | DataType::Timestamp) {
            roles.insert(ColumnRoleTag::Temporal);
        }
        if distinct_count > 0
            && distinct_count <= 50
            && !roles.contains(&ColumnRoleTag::PkCandidate)
        {
            roles.insert(ColumnRoleTag::Categorical);
        }

        distinct_sets.insert((ds.name.clone(), col_meta.name.clone()), seen_vals);
        columns.push(ColumnProfileStats {
            name: col_meta.name.clone(),
            data_type: col_meta.data_type,
            null_rate,
            distinct_count,
            roles,
            sample_values: samples,
        });
    }

    rank_pk_names(&mut pk_candidates);
    DatasetProfileSummary {
        dataset_id: ds.id,
        dataset_name: ds.name.clone(),
        display_name: ds.display_name.clone(),
        row_count: sampled_count,
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

fn infer_fk_candidates(
    summaries: &[DatasetProfileSummary],
    distinct_sets: &BTreeMap<(String, String), BTreeSet<String>>,
) -> Vec<FkCandidate> {
    let mut out = Vec::new();
    for src_ds in summaries {
        let src_rows = f64::from(u32::try_from(src_ds.row_count).unwrap_or(0));
        if src_rows <= 0.0 {
            continue;
        }
        for src_col in &src_ds.columns {
            let Some(src_vals) =
                distinct_sets.get(&(src_ds.dataset_name.clone(), src_col.name.clone()))
            else {
                continue;
            };
            if src_vals.is_empty() {
                continue;
            }
            for dst_ds in summaries {
                if dst_ds.dataset_name == src_ds.dataset_name {
                    continue;
                }
                for dst_pk in &dst_ds.pk_candidates {
                    let Some(dst_vals) =
                        distinct_sets.get(&(dst_ds.dataset_name.clone(), dst_pk.clone()))
                    else {
                        continue;
                    };
                    let overlap_cnt = src_vals.intersection(dst_vals).count();
                    let denom = f64::from(u32::try_from(src_vals.len()).unwrap_or(1));
                    let ratio = f64::from(u32::try_from(overlap_cnt).unwrap_or(0)) / denom;
                    if ratio >= 0.9 {
                        let uniq_ratio = denom / src_rows;
                        let cardinality = if uniq_ratio >= 0.99 {
                            Cardinality::OneToOne
                        } else {
                            Cardinality::ManyToOne
                        };
                        out.push(FkCandidate {
                            src_dataset: src_ds.dataset_name.clone(),
                            src_column: src_col.name.clone(),
                            dst_dataset: dst_ds.dataset_name.clone(),
                            dst_pk_column: dst_pk.clone(),
                            overlap_ratio: ratio,
                            cardinality,
                        });
                    }
                }
            }
        }
    }
    out
}

fn json_scalar_display(val: &serde_json::Value) -> String {
    match val {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
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

async fn get_mapping_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<MappingRecord>, HttpError> {
    principal.require(Permission::DataRead)?;
    let rec = state
        .store
        .get_mapping(&principal.tenant_id, id)
        .await?
        .ok_or(AppError::NotFound("MAPPING_NOT_FOUND"))?;
    Ok(Json(rec))
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
    principal.require(Permission::StudioEdit)?;
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
    principal.require(Permission::DataRead)?;
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

    let has_pii_raw = principal.has(Permission::PiiRawRead);
    let (session, _) = acquire_tenant_session(&state, &principal.tenant_id, false).await?;
    let escaped = ds.name.replace('"', "\"\"");
    let sql = match mapping.spec.filter.as_deref().map(str::trim) {
        Some(f) if !f.is_empty() => format!("SELECT * FROM \"{escaped}\" WHERE ({f}) LIMIT 50"),
        _ => format!("SELECT * FROM \"{escaped}\" LIMIT 50"),
    };
    let out = session
        .execute_sql(
            &QueryRequest {
                sql,
                params: BTreeMap::new(),
                max_rows: Some(50),
                timeout_ms: Some(5000),
                reveal_pii: has_pii_raw,
                allow_live: false,
            },
            false,
            has_pii_raw,
        )
        .await?;

    Ok(Json(build_mapping_preview_rows(
        mapping.id,
        &mapping.spec,
        &out.result,
    )))
}

fn build_mapping_preview_rows(
    mapping_id: Uuid,
    spec: &axpoint_domain::studio::MappingSpec,
    qr: &axpoint_domain::analytics::QueryResult,
) -> MappingPreviewResponse {
    let mut items = Vec::with_capacity(qr.rows.len());
    let mut rows_rejected: usize = 0;
    let mut reject_samples: Vec<MappingRejectSample> = Vec::new();

    for row in &qr.rows {
        let key_opt = row
            .get(&spec.key_column)
            .filter(|v| !v.is_null())
            .map(json_scalar_display)
            .filter(|s| !s.trim().is_empty());

        let Some(node_key) = key_opt else {
            rows_rejected = rows_rejected.saturating_add(1);
            if reject_samples.len() < 20 {
                reject_samples.push(MappingRejectSample {
                    row: serde_json::Value::Object(row.clone()),
                    reason: String::from("NULL_OR_EMPTY_KEY"),
                });
            }
            continue;
        };

        let title = render_preview_title(&spec.title, &node_key, row);
        let mut attributes = serde_json::Map::new();
        for (prop_name, col_name) in &spec.attributes {
            if let Some(v) = row.get(col_name) {
                attributes.insert(prop_name.clone(), v.clone());
            }
        }

        let mut metrics = BTreeMap::new();
        let mut metric_err = false;
        for (m_name, col_name) in &spec.metrics {
            if let Some(v) = row.get(col_name)
                && !v.is_null()
            {
                if let Some(num) = v
                    .as_f64()
                    .or_else(|| v.as_str().and_then(|s| s.parse::<f64>().ok()))
                {
                    metrics.insert(m_name.clone(), num);
                } else {
                    metric_err = true;
                }
            }
        }
        if metric_err {
            rows_rejected = rows_rejected.saturating_add(1);
            if reject_samples.len() < 20 {
                reject_samples.push(MappingRejectSample {
                    row: serde_json::Value::Object(row.clone()),
                    reason: String::from("INVALID_NUMERIC_METRIC"),
                });
            }
            continue;
        }

        let geo = extract_preview_geo(&spec.geo, row);
        let links = extract_preview_links(&spec.links, row);

        items.push(MappingPreviewNodeRow {
            node_key,
            title,
            attributes,
            metrics,
            geo,
            links,
        });
    }

    MappingPreviewResponse {
        mapping_id,
        sampled_rows: qr.rows.len(),
        items,
        rows_rejected,
        reject_samples,
    }
}

fn render_preview_title(
    template: &str,
    node_key: &str,
    row: &serde_json::Map<String, serde_json::Value>,
) -> String {
    let mut rendered = template.replace("{node_key}", node_key);
    for (col_name, val) in row {
        let token = format!("{{{col_name}}}");
        if rendered.contains(&token) {
            let replacement = if val.is_null() {
                String::new()
            } else {
                json_scalar_display(val)
            };
            rendered = rendered.replace(&token, &replacement);
        }
    }
    if rendered.trim().is_empty() {
        String::from(node_key)
    } else {
        rendered
    }
}

fn extract_preview_geo(
    geo_spec: &axpoint_domain::studio::MappingGeoSpec,
    row: &serde_json::Map<String, serde_json::Value>,
) -> Option<GeoPoint> {
    let lng_col = geo_spec.lng_column.as_deref()?;
    let lat_col = geo_spec.lat_column.as_deref()?;
    let lng = row.get(lng_col).and_then(|v| {
        v.as_f64()
            .or_else(|| v.as_str().and_then(|s| s.parse::<f64>().ok()))
    })?;
    let lat = row.get(lat_col).and_then(|v| {
        v.as_f64()
            .or_else(|| v.as_str().and_then(|s| s.parse::<f64>().ok()))
    })?;
    if (-180.0..=180.0).contains(&lng) && (-90.0..=90.0).contains(&lat) {
        Some(GeoPoint { lng, lat })
    } else {
        None
    }
}

fn extract_preview_links(
    link_specs: &[axpoint_domain::studio::MappingLinkSpec],
    row: &serde_json::Map<String, serde_json::Value>,
) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    for ls in link_specs {
        if let Some(val) = row.get(&ls.target_key_column)
            && !val.is_null()
        {
            let target_key = json_scalar_display(val);
            if !target_key.trim().is_empty() {
                out.push(serde_json::json!({
                    "link_type": ls.link_type,
                    "direction": ls.direction,
                    "target_type": ls.target_type,
                    "target_node_key": target_key,
                }));
            }
        }
    }
    out
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

async fn list_domain_objects_handler(
    State(state): State<AppState>,
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

    let has_pii_raw = principal.has(Permission::PiiRawRead);
    let (nodes, next_cursor) = state
        .store
        .query_domain_objects_list(&principal.tenant_id, &nt.name, &query)
        .await?;

    let items = nodes
        .iter()
        .map(|n| blue_node_to_domain_record(n, Some(nt), has_pii_raw, None))
        .collect();
    Ok(Json(DomainObjectListResponse { items, next_cursor }))
}

async fn get_domain_object_handler(
    State(state): State<AppState>,
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

    let node = state
        .store
        .get_domain_object_by_key(&principal.tenant_id, &nt.name, node_key.trim())
        .await?
        .ok_or(AppError::NotFound("NODE_NOT_FOUND"))?;

    let has_pii_raw = principal.has(Permission::PiiRawRead);
    let neighbors = if query.include_neighbors {
        Some(
            load_object_neighbors(
                &state,
                &principal.tenant_id,
                &node,
                nt,
                &published.doc,
                has_pii_raw,
            )
            .await?,
        )
    } else {
        None
    };

    Ok(Json(blue_node_to_domain_record(
        &node,
        Some(nt),
        has_pii_raw,
        neighbors,
    )))
}

async fn load_object_neighbors(
    state: &AppState,
    tenant_id: &str,
    node: &BlueNode,
    nt: &NodeTypeSpec,
    doc: &SchemaDoc,
    has_pii_raw: bool,
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

    let loaded = state
        .store
        .batch_load_neighbors(tenant_id, &batch_keys)
        .await?;
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
                .map(|nb| blue_node_to_domain_record(nb, target_spec, has_pii_raw, None))
                .collect()
        });
        out.insert(sig.field_name.clone(), recs);
    }
    Ok(out)
}

fn blue_node_to_domain_record(
    node: &BlueNode,
    nt: Option<&NodeTypeSpec>,
    has_pii_raw: bool,
    neighbors: Option<BTreeMap<String, Vec<DomainObjectRecord>>>,
) -> DomainObjectRecord {
    DomainObjectRecord {
        id: node.id,
        type_name: node.type_name.clone(),
        node_key: node.node_key.clone(),
        title: node.title.clone(),
        attributes: xblue_graphql::mask_node_attributes(node, nt, has_pii_raw),
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
    principal: AuthPrincipal,
    ApiQuery(query): ApiQuery<NodeListQuery>,
) -> Result<Json<NodeListResponse>, HttpError> {
    principal.require(Permission::DataRead)?;
    let resp = state.store.list_nodes(&principal.tenant_id, query).await?;
    Ok(Json(resp))
}

async fn upsert_node_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiJson(req): ApiJson<UpsertBlueNodeRequest>,
) -> Result<Json<BlueNode>, HttpError> {
    principal.require(Permission::StudioEdit)?;
    let node = state.store.upsert_node(&principal.tenant_id, req).await?;
    let _ = state
        .store
        .resolve_entity_links(
            &principal.tenant_id,
            ResolveEntitiesRequest {
                node_id: Some(node.id),
            },
        )
        .await;
    Ok(Json(node))
}

async fn bulk_upsert_nodes_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiJson(req): ApiJson<BulkNodesRequest>,
) -> Result<Json<BulkNodesResponse>, HttpError> {
    principal.require(Permission::StudioEdit)?;
    let resp = state.store.bulk_upsert(&principal.tenant_id, req).await?;
    if resp.upserted_nodes > 0 {
        let _ = state
            .store
            .resolve_entity_links(&principal.tenant_id, ResolveEntitiesRequest::default())
            .await;
    }
    Ok(Json(resp))
}

async fn search_nodes_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiQuery(query): ApiQuery<NodeSearchQuery>,
) -> Result<Json<NodeSearchResponse>, HttpError> {
    principal.require(Permission::DataRead)?;
    if query.q.trim().is_empty() {
        return Ok(Json(NodeSearchResponse { items: Vec::new() }));
    }
    let query_vec = state
        .ai
        .embed_one(query.q.trim())
        .await
        .ok()
        .map(|(v, _)| v);
    let resp = state
        .store
        .search_nodes(&principal.tenant_id, query, query_vec.as_deref())
        .await?;
    Ok(Json(resp))
}

async fn get_node_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<BlueNode>, HttpError> {
    principal.require(Permission::DataRead)?;
    let node = state
        .store
        .get_node(&principal.tenant_id, id)
        .await?
        .ok_or(AppError::NotFound("NODE_NOT_FOUND"))?;
    Ok(Json(node))
}

async fn patch_node_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(req): ApiJson<PatchNodeRequest>,
) -> Result<Json<BlueNode>, HttpError> {
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
    principal.require(Permission::StudioEdit)?;
    let link = state.store.upsert_link(&principal.tenant_id, req).await?;
    Ok(Json(link))
}

async fn delete_link_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<serde_json::Value>, HttpError> {
    principal.require(Permission::StudioEdit)?;
    state.store.delete_link(&principal.tenant_id, id).await?;
    Ok(Json(serde_json::json!({ "deleted": true })))
}

async fn write_metrics_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiJson(req): ApiJson<WriteMetricsRequest>,
) -> Result<Json<WriteMetricsResponse>, HttpError> {
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

async fn query_neighbors_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiQuery(query): ApiQuery<NeighborQuery>,
) -> Result<Json<SubgraphResponse>, HttpError> {
    principal.require(Permission::DataRead)?;
    let resp = state
        .store
        .query_neighbors(&principal.tenant_id, query)
        .await?;
    Ok(Json(resp))
}

async fn query_path_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiQuery(query): ApiQuery<PathQuery>,
) -> Result<Json<PathResponse>, HttpError> {
    principal.require(Permission::DataRead)?;
    let resp = state
        .store
        .query_shortest_path(&principal.tenant_id, query)
        .await?;
    Ok(Json(resp))
}

async fn query_subgraph_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiQuery(query): ApiQuery<SubgraphQuery>,
) -> Result<Json<SubgraphResponse>, HttpError> {
    principal.require(Permission::DataRead)?;
    let resp = state
        .store
        .query_subgraph(&principal.tenant_id, query)
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
    principal.require(Permission::StudioEdit)?;
    let link = state
        .store
        .reject_entity_link(&principal.tenant_id, entity_id, node_id)
        .await?;
    Ok(Json(link))
}
