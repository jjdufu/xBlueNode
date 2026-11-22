//! PostgreSQL persistence for `xBlueStudio` subgraphs, drafts, schema versions, merges, mappings,
//! and published domain object queries (`06-STEP05-06` §05.4–§05.7).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::str::FromStr;

use axpoint_domain::AppError;
use axpoint_domain::axhub::ColumnProfile;
use axpoint_domain::builder::{DevSubgraphResetResponse, resolve_deep_links};
use axpoint_domain::enums::{Cardinality, DataType, DesignMaturity, JobKind, SemanticLayerTier};
use axpoint_domain::graph::GeoPoint;
use axpoint_domain::platform::NewJob;
use axpoint_domain::ports::StudioRepository;
use axpoint_domain::studio::{
    AggRuleSpec, ConfirmProposalsRequest, CreateDraftRequest, CreateSubgraphRequest,
    DatasetValidationMeta, DerivedRuleSpec, DocEntityGroup, DomainObjectListQuery,
    DownstreamConsumerBinding, DownstreamConsumerKind, DraftSnapshotReason, DraftSnapshotSummary,
    DraftSource, DraftStatus, EdgeTypeSpec, IssueSeverity, LinkDirection, MappingMode,
    MappingRecord, MappingRejectSample, MappingRunRecord, MappingRunStatus, MappingSpec,
    MergePreview, MergePreviewRequest, MergeSubgraphRequest, NodeTypeSpec, PropertySpec,
    PublishDraftRequest, PublishDraftResponse, PublishedSchemaResponse, SaveSchemaDirectRequest,
    SchemaDiff, SchemaDoc, SchemaDraft, SchemaVersionDetail, SchemaVersionSummary, SubgraphInfo,
    TriggerMappingRunRequest, TriggerMappingRunResponse, UpdateDraftRequest, UpdateSubgraphRequest,
    ValidationContext, ValidationIssue, apply_merge, diff_schema, preview_merge,
    validate_with_metrics,
};
use axpoint_domain::topology::BlueNode;
use chrono::{DateTime, Utc};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

use crate::PgStore;
use crate::jobs::enqueue_job_tx;
use crate::tenant::begin_tenant_tx;

/// Key used to batch 1-hop neighbor queries across multiple parent nodes (`DataLoader` & REST).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NeighborBatchKey {
    /// Source or destination parent `xb_nodes.id`.
    pub parent_id: Uuid,
    /// `EdgeTypeSpec.name` (`xb_links.link_type`).
    pub link_type: String,
    /// Traversal direction (`OUT` or `IN`).
    pub direction: LinkDirection,
    /// Neighbor `NodeTypeSpec.name` (`xb_nodes.type_name`).
    pub target_type: String,
    /// Per-parent neighbor limit (`1..=200`).
    pub limit: u32,
}

fn is_unique_violation(err: &sqlx::Error) -> bool {
    err.as_database_error()
        .and_then(sqlx::error::DatabaseError::code)
        .is_some_and(|code| code == "23505")
}

fn is_valid_subgraph_slug(slug: &str) -> bool {
    let bytes = slug.as_bytes();
    let Some(&first) = bytes.first() else {
        return false;
    };
    if bytes.len() > 63 || !first.is_ascii_lowercase() {
        return false;
    }
    bytes
        .iter()
        .all(|&b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

async fn ensure_main_subgraph_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
) -> Result<(), AppError> {
    sqlx::query(
        "INSERT INTO xb_subgraphs (tenant_id, slug, display_name, is_main)
         VALUES ($1, 'main', '企业主图', TRUE)
         ON CONFLICT (tenant_id, slug) DO NOTHING",
    )
    .bind(tenant_id)
    .execute(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("ensure main subgraph failed: {err}")))?;

    let head_opt = sqlx::query(
        "SELECT current_version FROM xb_schema_heads WHERE tenant_id = $1 AND subgraph_slug = 'main'",
    )
    .bind(tenant_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("query main schema head failed: {err}")))?;

    if head_opt.is_none() {
        let existing_doc = load_schema_doc_tx(tx, tenant_id, "main").await?;
        let init_ver = i32::from(!is_doc_empty(&existing_doc));
        sqlx::query(
            "INSERT INTO xb_schema_heads (tenant_id, subgraph_slug, current_version)
             VALUES ($1, 'main', $2)
             ON CONFLICT (tenant_id, subgraph_slug) DO NOTHING",
        )
        .bind(tenant_id)
        .bind(init_ver)
        .execute(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("init main schema head failed: {err}")))?;

        if init_ver == 1 {
            let doc_json = serde_json::to_value(&existing_doc)
                .map_err(|err| AppError::Database(format!("serialize baseline doc: {err}")))?;
            sqlx::query(
                "INSERT INTO xb_schema_versions (tenant_id, subgraph_slug, version, schema_doc, change_summary)
                 VALUES ($1, 'main', 1, $2, 'baseline')
                 ON CONFLICT (tenant_id, subgraph_slug, version) DO NOTHING",
            )
            .bind(tenant_id)
            .bind(doc_json)
            .execute(&mut **tx)
            .await
            .map_err(|err| AppError::Database(format!("init baseline version failed: {err}")))?;
        }
    }

    Ok(())
}

async fn require_subgraph_exists_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    slug: &str,
) -> Result<i32, AppError> {
    if slug == "main" {
        ensure_main_subgraph_tx(tx, tenant_id).await?;
    }
    let row = sqlx::query(
        "SELECT COALESCE(h.current_version, 0) AS current_version
           FROM xb_subgraphs s
           LEFT JOIN xb_schema_heads h
             ON h.tenant_id = s.tenant_id AND h.subgraph_slug = s.slug
          WHERE s.tenant_id = $1 AND s.slug = $2",
    )
    .bind(tenant_id)
    .bind(slug)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("check subgraph exists failed: {err}")))?
    .ok_or(AppError::NotFound("SUBGRAPH_NOT_FOUND"))?;

    Ok(row.get("current_version"))
}

fn decode_subgraph_info(row: &sqlx::postgres::PgRow) -> SubgraphInfo {
    SubgraphInfo {
        id: row.get("id"),
        slug: row.get("slug"),
        display_name: row.get("display_name"),
        description: row.get("description"),
        owner_dept: row.get("owner_dept"),
        is_main: row.get("is_main"),
        is_dev_sandbox: row.try_get("is_dev_sandbox").unwrap_or(false),
        source_template_id: row.try_get("source_template_id").unwrap_or(None),
        last_reset_at: row.try_get("last_reset_at").unwrap_or(None),
        current_version: row.get("current_version"),
        created_by: row.get("created_by"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    }
}

async fn load_schema_doc_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    subgraph_slug: &str,
) -> Result<SchemaDoc, AppError> {
    let node_rows = sqlx::query(
        "SELECT id, name, display_name, description, icon, color, title_template,
                properties, agg_rules, derived_rules, sync_to_knowledge, pos_x, pos_y,
                semantic_layer_tier, domain_group
           FROM xb_node_types
          WHERE tenant_id = $1 AND subgraph_slug = $2
          ORDER BY created_at ASC, name ASC",
    )
    .bind(tenant_id)
    .bind(subgraph_slug)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("load xb_node_types failed: {err}")))?;

    let mut node_types = Vec::with_capacity(node_rows.len());
    for r in node_rows {
        let props: Vec<PropertySpec> =
            serde_json::from_value(r.get("properties")).unwrap_or_default();
        let agg_rules: Vec<AggRuleSpec> =
            serde_json::from_value(r.get("agg_rules")).unwrap_or_default();
        let derived_rules: Vec<DerivedRuleSpec> =
            serde_json::from_value(r.get("derived_rules")).unwrap_or_default();
        let raw_tier: Option<String> = r.get("semantic_layer_tier");
        let semantic_layer_tier = raw_tier
            .as_deref()
            .and_then(|s| SemanticLayerTier::from_str(s).ok());
        node_types.push(NodeTypeSpec {
            id: Some(r.get("id")),
            name: r.get("name"),
            display_name: r.get("display_name"),
            description: r.get("description"),
            icon: r.get("icon"),
            color: r.get("color"),
            title_template: r.get("title_template"),
            properties: props,
            agg_rules,
            derived_rules,
            sync_to_knowledge: r.get("sync_to_knowledge"),
            pos_x: r.get("pos_x"),
            pos_y: r.get("pos_y"),
            maturity: DesignMaturity::Confirmed,
            semantic_layer_tier,
            domain_group: r.get("domain_group"),
        });
    }

    let edge_rows = sqlx::query(
        "SELECT id, name, src_type_name, dst_type_name, impact_weight,
                cardinality, description, properties, propagation
           FROM xb_edge_types
          WHERE tenant_id = $1 AND subgraph_slug = $2
          ORDER BY created_at ASC, name ASC",
    )
    .bind(tenant_id)
    .bind(subgraph_slug)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("load xb_edge_types failed: {err}")))?;

    let mut edge_types = Vec::with_capacity(edge_rows.len());
    for r in edge_rows {
        let card_raw: String = r.get("cardinality");
        let cardinality = Cardinality::from_str(&card_raw).unwrap_or(Cardinality::ManyToMany);
        let properties: Vec<PropertySpec> =
            serde_json::from_value(r.get("properties")).unwrap_or_default();
        edge_types.push(EdgeTypeSpec {
            id: Some(r.get("id")),
            name: r.get("name"),
            src_type_name: r.get("src_type_name"),
            dst_type_name: r.get("dst_type_name"),
            impact_weight: r.get("impact_weight"),
            cardinality,
            description: r.get("description"),
            properties,
            propagation: r.get("propagation"),
            maturity: DesignMaturity::Confirmed,
        });
    }

    let map_rows = sqlx::query(
        "SELECT spec FROM xb_mappings
          WHERE tenant_id = $1 AND subgraph_slug = $2
          ORDER BY created_at ASC, name ASC",
    )
    .bind(tenant_id)
    .bind(subgraph_slug)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("load xb_mappings failed: {err}")))?;

    let mut mappings = Vec::with_capacity(map_rows.len());
    for r in map_rows {
        let spec_val: serde_json::Value = r.get("spec");
        let spec: MappingSpec = serde_json::from_value(spec_val)
            .map_err(|err| AppError::Database(format!("decode MappingSpec failed: {err}")))?;
        mappings.push(spec);
    }

    Ok(SchemaDoc {
        node_types,
        edge_types,
        mappings,
    })
}

fn is_doc_empty(doc: &SchemaDoc) -> bool {
    doc.node_types.is_empty() && doc.edge_types.is_empty() && doc.mappings.is_empty()
}

async fn load_validation_context_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    subgraph_slug: &str,
) -> Result<ValidationContext, AppError> {
    let head_ver = require_subgraph_exists_tx(tx, tenant_id, subgraph_slug).await?;
    let current_doc_loaded = load_schema_doc_tx(tx, tenant_id, subgraph_slug).await?;
    let current_doc = if head_ver > 0 || !is_doc_empty(&current_doc_loaded) {
        Some(current_doc_loaded)
    } else {
        None
    };

    let mut main_node_types = BTreeMap::new();
    if subgraph_slug != "main" {
        let main_doc = load_schema_doc_tx(tx, tenant_id, "main").await?;
        for nt in main_doc.node_types {
            main_node_types.insert(nt.name.clone(), nt);
        }
    }

    let mut existing_node_counts = BTreeMap::new();
    let mut existing_link_counts = BTreeMap::new();
    let mut cardinality_violations = BTreeSet::new();

    if subgraph_slug == "main" {
        let n_rows = sqlx::query(
            "SELECT type_name, COUNT(*) AS cnt
               FROM xb_nodes
              WHERE tenant_id = $1 AND subgraph_slug = 'main'
              GROUP BY type_name",
        )
        .bind(tenant_id)
        .fetch_all(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("count xb_nodes failed: {err}")))?;
        for r in n_rows {
            existing_node_counts.insert(r.get("type_name"), r.get("cnt"));
        }

        let l_rows = sqlx::query(
            "SELECT link_type, COUNT(*) AS cnt
               FROM xb_links
              WHERE tenant_id = $1 AND subgraph_slug = 'main'
              GROUP BY link_type",
        )
        .bind(tenant_id)
        .fetch_all(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("count xb_links failed: {err}")))?;
        for r in l_rows {
            existing_link_counts.insert(r.get("link_type"), r.get("cnt"));
        }

        let cv_rows = sqlx::query(
            "SELECT DISTINCT link_type
               FROM xb_links
              WHERE tenant_id = $1 AND subgraph_slug = 'main'
              GROUP BY link_type, dst_node_id
             HAVING COUNT(*) > 1",
        )
        .bind(tenant_id)
        .fetch_all(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("check cardinality failed: {err}")))?;
        for r in cv_rows {
            cardinality_violations.insert(r.get("link_type"));
        }
    }

    let datasets = load_datasets_validation_meta_tx(tx, tenant_id).await?;

    Ok(ValidationContext {
        subgraph_slug: String::from(subgraph_slug),
        main_node_types,
        current_doc,
        existing_node_counts,
        existing_link_counts,
        datasets,
        cardinality_violations,
    })
}

async fn load_datasets_validation_meta_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
) -> Result<BTreeMap<String, DatasetValidationMeta>, AppError> {
    let ds_rows = sqlx::query(
        "SELECT id, name, status, row_count, schema_version, primary_key, schema_json
           FROM ds_datasets
          WHERE tenant_id = $1",
    )
    .bind(tenant_id)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("load ds_datasets meta failed: {err}")))?;

    let cm_rows = sqlx::query(
        "SELECT dataset_id, column_name, data_type
           FROM ds_column_meta
          WHERE tenant_id = $1",
    )
    .bind(tenant_id)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("load ds_column_meta failed: {err}")))?;

    let mut col_meta_by_ds: HashMap<Uuid, BTreeMap<String, DataType>> = HashMap::new();
    for r in cm_rows {
        let dsid: Uuid = r.get("dataset_id");
        let cname: String = r.get("column_name");
        let dt_raw: String = r.get("data_type");
        if let Ok(dt) = DataType::from_str(&dt_raw) {
            col_meta_by_ds.entry(dsid).or_default().insert(cname, dt);
        }
    }

    let mut out = BTreeMap::new();
    for r in ds_rows {
        let id: Uuid = r.get("id");
        let name: String = r.get("name");
        let status: String = r.get("status");
        let row_count: i64 = r.get("row_count");
        let schema_version: i32 = r.get("schema_version");
        let pk_cols: Vec<String> = r.get("primary_key");
        let profiles: Vec<ColumnProfile> =
            serde_json::from_value(r.get("schema_json")).unwrap_or_default();

        let mut columns = col_meta_by_ds.remove(&id).unwrap_or_default();
        for p in &profiles {
            columns.entry(p.name.clone()).or_insert(p.data_type);
        }

        let has_null_stats = row_count > 0
            && profiles
                .iter()
                .any(|p| p.distinct_estimate > 0 || p.null_count > 0);
        let has_strict_not_null = profiles.iter().any(|p| !p.nullable);

        let mut non_null_columns = BTreeSet::new();
        if has_null_stats || has_strict_not_null {
            for pk in pk_cols {
                non_null_columns.insert(pk);
            }
            for p in &profiles {
                if !p.nullable || (has_null_stats && p.null_count == 0 && p.null_ratio <= 0.0) {
                    non_null_columns.insert(p.name.clone());
                }
            }
        }

        let meta = DatasetValidationMeta {
            id,
            name: name.clone(),
            status,
            row_count,
            schema_version,
            columns,
            non_null_columns,
        };
        out.insert(id.to_string(), meta.clone());
        out.insert(name, meta);
    }
    Ok(out)
}

fn decode_draft_row(row: &sqlx::postgres::PgRow) -> Result<SchemaDraft, AppError> {
    let source = DraftSource::from_str(&row.get::<String, _>("source"))?;
    let status = DraftStatus::from_str(&row.get::<String, _>("status"))?;
    let schema_doc: SchemaDoc = serde_json::from_value(row.get("schema_doc"))
        .map_err(|err| AppError::Database(format!("decode draft schema_doc failed: {err}")))?;
    let issues: Vec<ValidationIssue> =
        serde_json::from_value(row.get("issues")).unwrap_or_default();
    let pass_count = u32::try_from(row.get::<i32, _>("pass_count").max(0)).unwrap_or(0);

    Ok(SchemaDraft {
        id: row.get("id"),
        subgraph_slug: row.get("subgraph_slug"),
        title: row.get("title"),
        source,
        base_version: row.get("base_version"),
        schema_doc,
        ai_context: row.get("ai_context"),
        issues,
        pass_count,
        status,
        created_by: row.get("created_by"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

fn decode_mapping_row(row: &sqlx::postgres::PgRow) -> Result<MappingRecord, AppError> {
    let spec: MappingSpec = serde_json::from_value(row.get("spec"))
        .map_err(|err| AppError::Database(format!("decode xb_mappings.spec failed: {err}")))?;
    Ok(MappingRecord {
        id: row.get("id"),
        subgraph_slug: row.get("subgraph_slug"),
        name: row.get("name"),
        dataset_id: row.get("dataset_id"),
        dataset_name: row.get("dataset_name"),
        node_type: row.get("node_type"),
        spec,
        enabled: row.get("enabled"),
        schema_version: row.get("schema_version"),
        last_snapshot_id: row.get("last_snapshot_id"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

fn decode_mapping_run_row(row: &sqlx::postgres::PgRow) -> Result<MappingRunRecord, AppError> {
    let mode = MappingMode::from_str(&row.get::<String, _>("mode"))?;
    let status = MappingRunStatus::from_str(&row.get::<String, _>("status"))?;
    let reject_samples: Vec<MappingRejectSample> =
        serde_json::from_value(row.get("reject_samples")).unwrap_or_default();

    Ok(MappingRunRecord {
        id: row.get("id"),
        mapping_id: row.get("mapping_id"),
        job_id: row.get("job_id"),
        mode,
        from_snapshot: row.get("from_snapshot"),
        to_snapshot: row.get("to_snapshot"),
        nodes_upserted: row.get("nodes_upserted"),
        nodes_deleted: row.get("nodes_deleted"),
        links_upserted: row.get("links_upserted"),
        points_written: row.get("points_written"),
        rows_rejected: row.get("rows_rejected"),
        reject_samples,
        status,
        error_message: row.get("error_message"),
        started_at: row.get("started_at"),
        finished_at: row.get("finished_at"),
    })
}

async fn insert_draft_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    subgraph_slug: &str,
    actor_user_id: Option<Uuid>,
    req: CreateDraftRequest,
) -> Result<SchemaDraft, AppError> {
    let title = req.title.trim();
    if title.is_empty() || title.chars().count() > 128 {
        return Err(AppError::bad(
            "NAME_INVALID",
            "draft title must be between 1 and 128 characters",
        ));
    }
    let head_ver = require_subgraph_exists_tx(tx, tenant_id, subgraph_slug).await?;
    let ctx = load_validation_context_tx(tx, tenant_id, subgraph_slug).await?;
    let val_summary = validate_with_metrics(&req.schema_doc, &ctx);
    let pass_count_i32 = i32::try_from(val_summary.pass_count).unwrap_or(i32::MAX);

    let base_version = req.base_version.unwrap_or(head_ver);
    let source = req.source.unwrap_or(DraftSource::Manual);
    let ai_context = req
        .ai_context
        .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new()));
    let doc_json = serde_json::to_value(&req.schema_doc)
        .map_err(|err| AppError::bad("INVALID_ARGUMENT", format!("invalid schema_doc: {err}")))?;
    let issues_json = serde_json::to_value(&val_summary.issues)
        .map_err(|err| AppError::Database(format!("serialize issues failed: {err}")))?;

    let row = sqlx::query(
        "INSERT INTO xb_schema_drafts (
             tenant_id, subgraph_slug, title, source, base_version,
             schema_doc, ai_context, issues, pass_count, status, created_by
         )
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, 'DRAFT', $10)
         RETURNING *",
    )
    .bind(tenant_id)
    .bind(subgraph_slug)
    .bind(title)
    .bind(source.as_str())
    .bind(base_version)
    .bind(doc_json)
    .bind(ai_context)
    .bind(issues_json)
    .bind(pass_count_i32)
    .bind(actor_user_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("insert xb_schema_drafts failed: {err}")))?;

    decode_draft_row(&row)
}

fn verify_publish_issues(issues: &[ValidationIssue], force: bool) -> Result<(), AppError> {
    if issues.iter().any(|i| i.severity == IssueSeverity::Error) {
        let summary = issues
            .iter()
            .filter(|i| i.severity == IssueSeverity::Error)
            .map(|i| format!("{}: {} ({})", i.code, i.message, i.path))
            .collect::<Vec<_>>()
            .join("; ");
        return Err(AppError::bad("SCHEMA_INVALID", summary));
    }
    if !force && issues.iter().any(|i| i.code == "BREAKING_CHANGE") {
        return Err(AppError::Conflict("SCHEMA_BREAKING_CHANGE"));
    }
    Ok(())
}

async fn publish_draft_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    draft_id: Uuid,
    actor_user_id: Option<Uuid>,
    req: &PublishDraftRequest,
) -> Result<PublishDraftResponse, AppError> {
    let draft = lock_editable_draft_tx(tx, tenant_id, draft_id).await?;
    if draft.schema_doc.has_unconfirmed_proposals() {
        return Err(AppError::bad(
            "UNCONFIRMED_PROPOSALS_REMAIN",
            "draft contains unconfirmed PROPOSED elements; confirm proposals before publishing",
        ));
    }

    let lock_key = format!("{tenant_id}:xblue_publish:{}", draft.subgraph_slug);
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1)::bigint)")
        .bind(&lock_key)
        .execute(&mut **tx)
        .await
        .map_err(|err| {
            AppError::Database(format!("acquire publish advisory lock failed: {err}"))
        })?;

    let current_version = require_subgraph_exists_tx(tx, tenant_id, &draft.subgraph_slug).await?;
    if current_version != draft.base_version {
        return Err(AppError::Conflict("VERSION_CONFLICT"));
    }

    let ctx = load_validation_context_tx(tx, tenant_id, &draft.subgraph_slug).await?;
    let val_summary = validate_with_metrics(&draft.schema_doc, &ctx);
    let pass_count_i32 = i32::try_from(val_summary.pass_count).unwrap_or(i32::MAX);
    verify_publish_issues(&val_summary.issues, req.force)?;
    let issues_json = serde_json::to_value(&val_summary.issues)
        .map_err(|err| AppError::Database(format!("serialize publish issues failed: {err}")))?;

    let prev_doc = ctx.current_doc.unwrap_or_default();
    let diff = diff_schema(&prev_doc, &draft.schema_doc);

    let mut published_doc = draft.schema_doc.clone();
    apply_node_and_edge_tables_tx(
        tx,
        tenant_id,
        &draft.subgraph_slug,
        &mut published_doc,
        &diff,
        req.force && req.cascade,
    )
    .await?;
    let upserted_mappings =
        apply_mappings_table_tx(tx, tenant_id, &draft.subgraph_slug, &published_doc).await?;

    let new_version = current_version.saturating_add(1);
    persist_published_version_tx(
        tx,
        tenant_id,
        PublishVersionRecord {
            draft: &draft,
            new_version,
            published_doc: &published_doc,
            diff: &diff,
            issues_json: &issues_json,
            pass_count: pass_count_i32,
            change_summary: &req.change_summary,
            actor_user_id,
        },
    )
    .await?;

    let (mapping_jobs, aggregate_jobs) = if draft.subgraph_slug == "main" {
        enqueue_main_publish_jobs_tx(
            tx,
            tenant_id,
            PublishJobContext {
                new_version,
                prev_doc: &prev_doc,
                published_doc: &published_doc,
                diff: &diff,
                upserted_mappings: &upserted_mappings,
            },
        )
        .await?
    } else {
        (Vec::new(), Vec::new())
    };

    Ok(PublishDraftResponse {
        subgraph_slug: draft.subgraph_slug,
        version: new_version,
        draft_id: draft.id,
        schema_doc: published_doc,
        diff,
        mapping_jobs,
        aggregate_jobs,
    })
}

async fn apply_node_and_edge_tables_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    subgraph_slug: &str,
    doc: &mut SchemaDoc,
    diff: &SchemaDiff,
    cascade_runtime: bool,
) -> Result<(), AppError> {
    let keep_node_names: Vec<String> = doc.node_types.iter().map(|nt| nt.name.clone()).collect();
    sqlx::query(
        "DELETE FROM xb_node_types
          WHERE tenant_id = $1 AND subgraph_slug = $2 AND NOT (name = ANY($3))",
    )
    .bind(tenant_id)
    .bind(subgraph_slug)
    .bind(&keep_node_names)
    .execute(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("delete removed xb_node_types failed: {err}")))?;

    if subgraph_slug == "main" && cascade_runtime && !diff.node_types.removed.is_empty() {
        cascade_delete_removed_runtime_nodes_tx(tx, tenant_id, &diff.node_types.removed).await?;
    }

    for nt in &mut doc.node_types {
        let props_json = serde_json::to_value(&nt.properties)
            .map_err(|err| AppError::Database(format!("serialize properties: {err}")))?;
        let agg_json = serde_json::to_value(&nt.agg_rules)
            .map_err(|err| AppError::Database(format!("serialize agg_rules: {err}")))?;
        let derived_json = serde_json::to_value(&nt.derived_rules)
            .map_err(|err| AppError::Database(format!("serialize derived_rules: {err}")))?;

        let row = sqlx::query(
            "INSERT INTO xb_node_types (
                 tenant_id, subgraph_slug, name, display_name, description, icon, color,
                 title_template, properties, agg_rules, derived_rules, sync_to_knowledge, pos_x, pos_y,
                 semantic_layer_tier, domain_group
             )
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)
             ON CONFLICT (tenant_id, subgraph_slug, name) DO UPDATE SET
                 display_name = EXCLUDED.display_name,
                 description = EXCLUDED.description,
                 icon = EXCLUDED.icon,
                 color = EXCLUDED.color,
                 title_template = EXCLUDED.title_template,
                 properties = EXCLUDED.properties,
                 agg_rules = EXCLUDED.agg_rules,
                 derived_rules = EXCLUDED.derived_rules,
                 sync_to_knowledge = EXCLUDED.sync_to_knowledge,
                 pos_x = EXCLUDED.pos_x,
                 pos_y = EXCLUDED.pos_y,
                 semantic_layer_tier = EXCLUDED.semantic_layer_tier,
                 domain_group = EXCLUDED.domain_group,
                 updated_at = NOW()
             RETURNING id",
        )
        .bind(tenant_id)
        .bind(subgraph_slug)
        .bind(&nt.name)
        .bind(&nt.display_name)
        .bind(&nt.description)
        .bind(&nt.icon)
        .bind(&nt.color)
        .bind(&nt.title_template)
        .bind(props_json)
        .bind(agg_json)
        .bind(derived_json)
        .bind(nt.sync_to_knowledge)
        .bind(nt.pos_x)
        .bind(nt.pos_y)
        .bind(nt.semantic_layer_tier.map(SemanticLayerTier::as_str))
        .bind(&nt.domain_group)
        .fetch_one(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("upsert xb_node_types failed: {err}")))?;
        nt.id = Some(row.get("id"));
    }

    sync_edge_types_table_tx(tx, tenant_id, subgraph_slug, doc, diff, cascade_runtime).await
}

async fn sync_edge_types_table_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    subgraph_slug: &str,
    doc: &mut SchemaDoc,
    diff: &SchemaDiff,
    cascade_runtime: bool,
) -> Result<(), AppError> {
    let existing_edges = sqlx::query(
        "SELECT id, name, src_type_name, dst_type_name
           FROM xb_edge_types WHERE tenant_id = $1 AND subgraph_slug = $2",
    )
    .bind(tenant_id)
    .bind(subgraph_slug)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("list existing xb_edge_types failed: {err}")))?;

    for r in existing_edges {
        let eid: Uuid = r.get("id");
        let name: String = r.get("name");
        let src: String = r.get("src_type_name");
        let dst: String = r.get("dst_type_name");
        let keep = doc
            .edge_types
            .iter()
            .any(|e| e.name == name && e.src_type_name == src && e.dst_type_name == dst);
        if !keep {
            sqlx::query("DELETE FROM xb_edge_types WHERE tenant_id = $1 AND id = $2")
                .bind(tenant_id)
                .bind(eid)
                .execute(&mut **tx)
                .await
                .map_err(|err| AppError::Database(format!("delete xb_edge_types failed: {err}")))?;
        }
    }

    if subgraph_slug == "main" && cascade_runtime && !diff.edge_types.removed.is_empty() {
        let removed_names: Vec<String> = diff
            .edge_types
            .removed
            .iter()
            .map(|k| k.split(':').next().unwrap_or(k.as_str()).to_owned())
            .collect();
        sqlx::query(
            "DELETE FROM xb_links
              WHERE tenant_id = $1 AND subgraph_slug = 'main' AND link_type = ANY($2)",
        )
        .bind(tenant_id)
        .bind(&removed_names)
        .execute(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("cascade delete xb_links failed: {err}")))?;
    }

    for et in &mut doc.edge_types {
        let props_json = serde_json::to_value(&et.properties)
            .map_err(|err| AppError::Database(format!("serialize edge properties: {err}")))?;
        let row = sqlx::query(
            "INSERT INTO xb_edge_types (
                 tenant_id, subgraph_slug, name, src_type_name, dst_type_name,
                 impact_weight, cardinality, description, properties, propagation
             )
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
             ON CONFLICT (tenant_id, subgraph_slug, name, src_type_name, dst_type_name) DO UPDATE SET
                 impact_weight = EXCLUDED.impact_weight,
                 cardinality = EXCLUDED.cardinality,
                 description = EXCLUDED.description,
                 properties = EXCLUDED.properties,
                 propagation = EXCLUDED.propagation,
                 updated_at = NOW()
             RETURNING id",
        )
        .bind(tenant_id)
        .bind(subgraph_slug)
        .bind(&et.name)
        .bind(&et.src_type_name)
        .bind(&et.dst_type_name)
        .bind(et.impact_weight)
        .bind(et.cardinality.as_str())
        .bind(&et.description)
        .bind(props_json)
        .bind(&et.propagation)
        .fetch_one(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("upsert xb_edge_types failed: {err}")))?;
        et.id = Some(row.get("id"));
    }

    Ok(())
}

async fn cascade_delete_removed_runtime_nodes_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    removed_types: &[String],
) -> Result<(), AppError> {
    sqlx::query(
        "DELETE FROM xb_links
          WHERE tenant_id = $1
            AND subgraph_slug = 'main'
            AND (src_node_id IN (SELECT id FROM xb_nodes WHERE tenant_id = $1 AND subgraph_slug = 'main' AND type_name = ANY($2))
              OR dst_node_id IN (SELECT id FROM xb_nodes WHERE tenant_id = $1 AND subgraph_slug = 'main' AND type_name = ANY($2)))",
    )
    .bind(tenant_id)
    .bind(removed_types)
    .execute(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("cascade delete node links failed: {err}")))?;

    sqlx::query(
        "DELETE FROM xb_metric_series
          WHERE tenant_id = $1
            AND node_id IN (SELECT id FROM xb_nodes WHERE tenant_id = $1 AND subgraph_slug = 'main' AND type_name = ANY($2))",
    )
    .bind(tenant_id)
    .bind(removed_types)
    .execute(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("cascade delete node metrics failed: {err}")))?;

    sqlx::query(
        "DELETE FROM xb_nodes
          WHERE tenant_id = $1 AND subgraph_slug = 'main' AND type_name = ANY($2)",
    )
    .bind(tenant_id)
    .bind(removed_types)
    .execute(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("cascade delete xb_nodes failed: {err}")))?;
    Ok(())
}

async fn apply_mappings_table_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    subgraph_slug: &str,
    doc: &SchemaDoc,
) -> Result<BTreeMap<String, Uuid>, AppError> {
    let keep_names: Vec<String> = doc.mappings.iter().map(|m| m.name.clone()).collect();
    sqlx::query(
        "DELETE FROM xb_mappings
          WHERE tenant_id = $1 AND subgraph_slug = $2 AND NOT (name = ANY($3))",
    )
    .bind(tenant_id)
    .bind(subgraph_slug)
    .bind(&keep_names)
    .execute(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("delete removed xb_mappings failed: {err}")))?;

    if subgraph_slug == "main" {
        sqlx::query(
            "DELETE FROM ds_lineage
              WHERE tenant_id = $1
                AND src_kind = 'DATASET'
                AND dst_kind = 'NODE_TYPE'
                AND via LIKE 'mapping:%'",
        )
        .bind(tenant_id)
        .execute(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("clean mapping lineage failed: {err}")))?;
    }

    let mut id_by_name = BTreeMap::new();
    for m in &doc.mappings {
        let ds_uuid = Uuid::parse_str(&m.dataset).ok();
        let ds_row = sqlx::query(
            "SELECT id, schema_version FROM ds_datasets
              WHERE tenant_id = $1 AND (name = $2 OR ($3::uuid IS NOT NULL AND id = $3))
              LIMIT 1",
        )
        .bind(tenant_id)
        .bind(&m.dataset)
        .bind(ds_uuid)
        .fetch_optional(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("resolve mapping dataset failed: {err}")))?
        .ok_or_else(|| {
            AppError::bad(
                "MAPPING_INVALID",
                format!("dataset '{}' not found", m.dataset),
            )
        })?;

        let dataset_id: Uuid = ds_row.get("id");
        let ds_schema_version: i32 = ds_row.get("schema_version");
        let spec_json = serde_json::to_value(m)
            .map_err(|err| AppError::Database(format!("serialize MappingSpec: {err}")))?;

        let m_row = sqlx::query(
            "INSERT INTO xb_mappings (
                 tenant_id, subgraph_slug, name, dataset_id, node_type, spec, enabled, schema_version
             )
             VALUES ($1, $2, $3, $4, $5, $6, TRUE, $7)
             ON CONFLICT (tenant_id, subgraph_slug, name) DO UPDATE SET
                 dataset_id = EXCLUDED.dataset_id,
                 node_type = EXCLUDED.node_type,
                 spec = EXCLUDED.spec,
                 schema_version = EXCLUDED.schema_version,
                 updated_at = NOW()
             RETURNING id",
        )
        .bind(tenant_id)
        .bind(subgraph_slug)
        .bind(&m.name)
        .bind(dataset_id)
        .bind(&m.node_type)
        .bind(spec_json)
        .bind(ds_schema_version)
        .fetch_one(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("upsert xb_mappings failed: {err}")))?;

        let mapping_id: Uuid = m_row.get("id");
        id_by_name.insert(m.name.clone(), mapping_id);

        if subgraph_slug == "main" {
            sqlx::query(
                "INSERT INTO ds_lineage (tenant_id, src_kind, src_id, dst_kind, dst_id, via)
                 VALUES ($1, 'DATASET', $2, 'NODE_TYPE', $3, $4)
                 ON CONFLICT (tenant_id, src_kind, src_id, dst_kind, dst_id)
                 DO UPDATE SET via = EXCLUDED.via",
            )
            .bind(tenant_id)
            .bind(dataset_id.to_string())
            .bind(&m.node_type)
            .bind(format!("mapping:{}", m.name))
            .execute(&mut **tx)
            .await
            .map_err(|err| AppError::Database(format!("insert mapping lineage failed: {err}")))?;
        }
    }

    Ok(id_by_name)
}

struct PublishVersionRecord<'a> {
    draft: &'a SchemaDraft,
    new_version: i32,
    published_doc: &'a SchemaDoc,
    diff: &'a SchemaDiff,
    issues_json: &'a serde_json::Value,
    pass_count: i32,
    change_summary: &'a str,
    actor_user_id: Option<Uuid>,
}

async fn persist_published_version_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    rec: PublishVersionRecord<'_>,
) -> Result<(), AppError> {
    sqlx::query(
        "INSERT INTO xb_schema_heads (tenant_id, subgraph_slug, current_version, updated_at)
         VALUES ($1, $2, $3, NOW())
         ON CONFLICT (tenant_id, subgraph_slug) DO UPDATE SET
             current_version = EXCLUDED.current_version,
             updated_at = NOW()",
    )
    .bind(tenant_id)
    .bind(&rec.draft.subgraph_slug)
    .bind(rec.new_version)
    .execute(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("update xb_schema_heads failed: {err}")))?;

    let doc_json = serde_json::to_value(rec.published_doc)
        .map_err(|err| AppError::Database(format!("serialize published SchemaDoc: {err}")))?;
    let diff_json = serde_json::to_value(rec.diff)
        .map_err(|err| AppError::Database(format!("serialize SchemaDiff: {err}")))?;

    sqlx::query(
        "INSERT INTO xb_schema_versions (
             tenant_id, subgraph_slug, version, schema_doc, diff, change_summary, published_by
         )
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(tenant_id)
    .bind(&rec.draft.subgraph_slug)
    .bind(rec.new_version)
    .bind(doc_json)
    .bind(diff_json)
    .bind(rec.change_summary)
    .bind(rec.actor_user_id)
    .execute(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("insert xb_schema_versions failed: {err}")))?;

    sqlx::query(
        "UPDATE xb_schema_drafts
            SET status = 'PUBLISHED',
                issues = $1,
                pass_count = $2,
                updated_at = NOW()
          WHERE tenant_id = $3 AND id = $4",
    )
    .bind(rec.issues_json)
    .bind(rec.pass_count)
    .bind(tenant_id)
    .bind(rec.draft.id)
    .execute(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("mark draft PUBLISHED failed: {err}")))?;

    Ok(())
}

struct PublishJobContext<'a> {
    new_version: i32,
    prev_doc: &'a SchemaDoc,
    published_doc: &'a SchemaDoc,
    diff: &'a SchemaDiff,
    upserted_mappings: &'a BTreeMap<String, Uuid>,
}

async fn enqueue_main_publish_jobs_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    ctx: PublishJobContext<'_>,
) -> Result<(Vec<Uuid>, Vec<Uuid>), AppError> {
    let mut mapping_jobs = Vec::new();
    let mut changed_mapping_names: BTreeSet<&str> = BTreeSet::new();
    for m in &ctx.diff.mappings.added {
        changed_mapping_names.insert(m.as_str());
    }
    for m in &ctx.diff.mappings.changed {
        changed_mapping_names.insert(m.name.as_str());
    }

    for m_name in changed_mapping_names {
        if let Some(&mapping_id) = ctx.upserted_mappings.get(m_name) {
            let job = NewJob {
                kind: JobKind::MapMaterialize,
                payload: serde_json::json!({
                    "mapping_id": mapping_id,
                    "mode": "FULL",
                    "schema_version": ctx.new_version,
                }),
                priority: 0,
                max_attempts: 3,
                next_run_at: None,
                idempotency_key: Some(format!("map_pub:{mapping_id}:v{}", ctx.new_version)),
            };
            let job_id = enqueue_job_tx(tx, tenant_id, &job).await?;
            mapping_jobs.push(job_id);
        }
    }

    let mut aggregate_jobs = Vec::new();
    if rules_modified_in_publish(ctx.published_doc, ctx.diff) {
        let job = NewJob {
            kind: JobKind::GraphAggregate,
            payload: serde_json::json!({
                "full": true,
                "schema_version": ctx.new_version,
            }),
            priority: 0,
            max_attempts: 3,
            next_run_at: None,
            idempotency_key: Some(format!("graph_agg_pub:{tenant_id}:v{}", ctx.new_version)),
        };
        let job_id = enqueue_job_tx(tx, tenant_id, &job).await?;
        aggregate_jobs.push(job_id);
    }

    let newly_synced = newly_enabled_knowledge_types(ctx.prev_doc, ctx.published_doc);
    if !newly_synced.is_empty() {
        let job = NewJob {
            kind: JobKind::KnowledgeSync,
            payload: serde_json::json!({
                "type_names": newly_synced,
                "schema_version": ctx.new_version,
            }),
            priority: 0,
            max_attempts: 3,
            next_run_at: None,
            idempotency_key: Some(format!(
                "knowledge_sync_pub:{tenant_id}:v{}",
                ctx.new_version
            )),
        };
        let _ = enqueue_job_tx(tx, tenant_id, &job).await?;
    }

    Ok((mapping_jobs, aggregate_jobs))
}

fn rules_modified_in_publish(published_doc: &SchemaDoc, diff: &SchemaDiff) -> bool {
    for added_name in &diff.node_types.added {
        if let Some(nt) = published_doc
            .node_types
            .iter()
            .find(|n| n.name == *added_name)
            && (!nt.agg_rules.is_empty() || nt.derived_rules.iter().any(|r| r.enabled))
        {
            return true;
        }
    }
    for changed_nt in &diff.node_types.changed {
        if changed_nt
            .fields
            .iter()
            .any(|f| f.path.starts_with("agg_rules[") || f.path.starts_with("derived_rules["))
        {
            return true;
        }
    }
    false
}

fn newly_enabled_knowledge_types(prev_doc: &SchemaDoc, next_doc: &SchemaDoc) -> Vec<String> {
    let prev_synced: BTreeSet<&str> = prev_doc
        .node_types
        .iter()
        .filter(|n| n.sync_to_knowledge)
        .map(|n| n.name.as_str())
        .collect();
    next_doc
        .node_types
        .iter()
        .filter(|n| n.sync_to_knowledge && !prev_synced.contains(n.name.as_str()))
        .map(|n| n.name.clone())
        .collect()
}

fn decode_blue_node_row(row: &sqlx::postgres::PgRow) -> BlueNode {
    let p_val: serde_json::Value = row.get("properties");
    let properties = match p_val {
        serde_json::Value::Object(map) => map,
        _ => serde_json::Map::new(),
    };
    let m_val: serde_json::Value = row.get("metrics");
    let metrics: BTreeMap<String, f64> = serde_json::from_value(m_val).unwrap_or_default();
    let geo_lng: Option<f64> = row.get("geo_lng");
    let geo_lat: Option<f64> = row.get("geo_lat");
    let geo = match (geo_lng, geo_lat) {
        (Some(lng), Some(lat)) => Some(GeoPoint { lng, lat }),
        _ => None,
    };
    BlueNode {
        id: row.get("id"),
        type_name: row.get("type_name"),
        node_key: row.get("node_key"),
        title: row.get("title"),
        properties,
        metrics,
        geo,
        source_dataset_id: row.get("source_dataset_id"),
        source_row_key: row.get("source_row_key"),
        row_version: row.get("row_version"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    }
}

fn parse_object_cursor(raw: &str) -> Result<(DateTime<Utc>, Uuid), AppError> {
    let (ts_part, id_part) = raw
        .split_once('|')
        .ok_or_else(|| AppError::bad("INVALID_ARGUMENT", "invalid pagination cursor"))?;
    let ts = DateTime::parse_from_rfc3339(ts_part)
        .map_err(|err| AppError::bad("INVALID_ARGUMENT", format!("invalid cursor ts: {err}")))?
        .with_timezone(&Utc);
    let id = Uuid::parse_str(id_part)
        .map_err(|err| AppError::bad("INVALID_ARGUMENT", format!("invalid cursor id: {err}")))?;
    Ok((ts, id))
}

impl PgStore {
    /// Queries paginated `xb_nodes` for a published domain object type (`/api/xblue/objects/{type_name}` & GraphQL).
    ///
    /// # Errors
    /// Returns [`AppError::BadRequest`] on invalid `filter` JSON or `cursor`, or [`AppError::Database`] on SQL failure.
    pub async fn query_domain_objects_list(
        &self,
        tenant_id: &str,
        type_name: &str,
        query: &DomainObjectListQuery,
    ) -> Result<(Vec<BlueNode>, Option<String>), AppError> {
        let limit = query.limit.unwrap_or(50).clamp(1, 200);
        let fetch_limit = i64::from(limit).saturating_add(1);

        let filter_json: Option<serde_json::Value> = match query.filter.as_deref().map(str::trim) {
            Some(s) if !s.is_empty() => {
                let parsed: serde_json::Value = serde_json::from_str(s).map_err(|err| {
                    AppError::bad("INVALID_ARGUMENT", format!("invalid filter JSON: {err}"))
                })?;
                if !parsed.is_object() {
                    return Err(AppError::bad(
                        "INVALID_ARGUMENT",
                        "filter must be a JSON object",
                    ));
                }
                Some(parsed)
            }
            _ => None,
        };

        let cursor_pair = match query.cursor.as_deref().map(str::trim) {
            Some(c) if !c.is_empty() => Some(parse_object_cursor(c)?),
            _ => None,
        };

        let q_like = query
            .q
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| format!("%{s}%"));

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let rows = sqlx::query(
            "SELECT id, type_name, node_key, title, properties, metrics,
                    geo_lng, geo_lat, source_dataset_id, source_row_key,
                    row_version, created_at, updated_at
               FROM xb_nodes
              WHERE tenant_id = $1
                AND subgraph_slug = 'main'
                AND type_name = $2
                AND ($3::text IS NULL OR node_key ILIKE $3 OR title ILIKE $3)
                AND ($4::jsonb IS NULL OR properties @> $4)
                AND ($5::timestamptz IS NULL OR (updated_at, id) < ($5, $6::uuid))
              ORDER BY updated_at DESC, id DESC
              LIMIT $7",
        )
        .bind(tenant_id)
        .bind(type_name)
        .bind(q_like)
        .bind(filter_json)
        .bind(cursor_pair.map(|(ts, _)| ts))
        .bind(cursor_pair.map(|(_, id)| id))
        .bind(fetch_limit)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("query domain objects failed: {err}")))?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit query_domain_objects_list tx failed: {err}"))
        })?;

        let limit_usize = usize::try_from(limit).unwrap_or(50);
        let mut nodes: Vec<BlueNode> = rows.iter().map(decode_blue_node_row).collect();
        let next_cursor = if nodes.len() > limit_usize {
            nodes.truncate(limit_usize);
            nodes
                .last()
                .map(|last| format!("{}|{}", last.updated_at.to_rfc3339(), last.id))
        } else {
            None
        };

        Ok((nodes, next_cursor))
    }

    /// Fetches a single runtime `BlueNode` by `(tenant_id, type_name, node_key)`.
    ///
    /// # Errors
    /// Returns [`AppError::Database`] if querying `xb_nodes` fails.
    pub async fn get_domain_object_by_key(
        &self,
        tenant_id: &str,
        type_name: &str,
        node_key: &str,
    ) -> Result<Option<BlueNode>, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let row_opt = sqlx::query(
            "SELECT id, type_name, node_key, title, properties, metrics,
                    geo_lng, geo_lat, source_dataset_id, source_row_key,
                    row_version, created_at, updated_at
               FROM xb_nodes
              WHERE tenant_id = $1 AND subgraph_slug = 'main' AND type_name = $2 AND node_key = $3",
        )
        .bind(tenant_id)
        .bind(type_name)
        .bind(node_key)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("get domain object by key failed: {err}")))?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit get_domain_object_by_key tx failed: {err}"))
        })?;

        Ok(row_opt.as_ref().map(decode_blue_node_row))
    }

    /// Batches 1-hop neighbor lookups across multiple parent node IDs into one `ANY($3)` SQL query per
    /// `(link_type, direction, target_type)` group (`06-STEP05-06` §05.6 N+1 elimination).
    ///
    /// # Errors
    /// Returns [`AppError::Database`] if executing batched neighbor queries fails.
    pub async fn batch_load_neighbors(
        &self,
        tenant_id: &str,
        keys: &[NeighborBatchKey],
    ) -> Result<HashMap<NeighborBatchKey, Vec<BlueNode>>, AppError> {
        if keys.is_empty() {
            return Ok(HashMap::new());
        }

        let mut groups: HashMap<(String, LinkDirection, String), (Vec<Uuid>, u32)> = HashMap::new();
        for k in keys {
            let entry = groups
                .entry((k.link_type.clone(), k.direction, k.target_type.clone()))
                .or_insert_with(|| (Vec::new(), 1));
            if !entry.0.contains(&k.parent_id) {
                entry.0.push(k.parent_id);
            }
            entry.1 = entry.1.max(k.limit.clamp(1, 200));
        }

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let mut by_parent_group: HashMap<(Uuid, String, LinkDirection, String), Vec<BlueNode>> =
            HashMap::new();

        for ((link_type, direction, target_type), (parent_ids, max_limit)) in groups {
            let rows = sqlx::query(batch_neighbor_sql(direction))
                .bind(tenant_id)
                .bind(&link_type)
                .bind(&parent_ids)
                .bind(&target_type)
                .bind(i64::from(max_limit))
                .fetch_all(&mut *tx)
                .await
                .map_err(|err| {
                    AppError::Database(format!("batch_load_neighbors query failed: {err}"))
                })?;

            for r in rows {
                let pid: Uuid = r.get("parent_id");
                let node = decode_blue_node_row(&r);
                by_parent_group
                    .entry((pid, link_type.clone(), direction, target_type.clone()))
                    .or_default()
                    .push(node);
            }
        }

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit batch_load_neighbors tx failed: {err}"))
        })?;

        let mut result = HashMap::with_capacity(keys.len());
        for k in keys {
            let cap = usize::try_from(k.limit.clamp(1, 200)).unwrap_or(50);
            let items = by_parent_group
                .get(&(
                    k.parent_id,
                    k.link_type.clone(),
                    k.direction,
                    k.target_type.clone(),
                ))
                .map_or_default(|list| list.iter().take(cap).cloned().collect());
            result.insert(k.clone(), items);
        }
        Ok(result)
    }
}

const fn batch_neighbor_sql(direction: LinkDirection) -> &'static str {
    match direction {
        LinkDirection::Out => {
            "SELECT sub.parent_id, sub.id, sub.type_name, sub.node_key, sub.title,
                    sub.properties, sub.metrics, sub.geo_lng, sub.geo_lat,
                    sub.source_dataset_id, sub.source_row_key, sub.row_version,
                    sub.created_at, sub.updated_at
               FROM (
                   SELECT l.src_node_id AS parent_id,
                          n.id, n.type_name, n.node_key, n.title,
                          n.properties, n.metrics, n.geo_lng, n.geo_lat,
                          n.source_dataset_id, n.source_row_key, n.row_version,
                          n.created_at, n.updated_at,
                          ROW_NUMBER() OVER (PARTITION BY l.src_node_id ORDER BY n.node_key ASC, n.id ASC) AS rn
                     FROM xb_links l
                     JOIN xb_nodes n ON n.tenant_id = l.tenant_id AND n.id = l.dst_node_id
                    WHERE l.tenant_id = $1
                      AND l.subgraph_slug = 'main'
                      AND n.subgraph_slug = 'main'
                      AND l.link_type = $2
                      AND l.src_node_id = ANY($3)
                      AND n.type_name = $4
               ) sub
              WHERE sub.rn <= $5
              ORDER BY sub.parent_id, sub.rn"
        }
        LinkDirection::In => {
            "SELECT sub.parent_id, sub.id, sub.type_name, sub.node_key, sub.title,
                    sub.properties, sub.metrics, sub.geo_lng, sub.geo_lat,
                    sub.source_dataset_id, sub.source_row_key, sub.row_version,
                    sub.created_at, sub.updated_at
               FROM (
                   SELECT l.dst_node_id AS parent_id,
                          n.id, n.type_name, n.node_key, n.title,
                          n.properties, n.metrics, n.geo_lng, n.geo_lat,
                          n.source_dataset_id, n.source_row_key, n.row_version,
                          n.created_at, n.updated_at,
                          ROW_NUMBER() OVER (PARTITION BY l.dst_node_id ORDER BY n.node_key ASC, n.id ASC) AS rn
                     FROM xb_links l
                     JOIN xb_nodes n ON n.tenant_id = l.tenant_id AND n.id = l.src_node_id
                    WHERE l.tenant_id = $1
                      AND l.subgraph_slug = 'main'
                      AND n.subgraph_slug = 'main'
                      AND l.link_type = $2
                      AND l.dst_node_id = ANY($3)
                      AND n.type_name = $4
               ) sub
              WHERE sub.rn <= $5
              ORDER BY sub.parent_id, sub.rn"
        }
    }
}

impl StudioRepository for PgStore {
    async fn list_subgraphs(&self, tenant_id: &str) -> Result<Vec<SubgraphInfo>, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        ensure_main_subgraph_tx(&mut tx, tenant_id).await?;

        let rows = sqlx::query(
            "SELECT s.id, s.slug, s.display_name, s.description, s.owner_dept,
                    s.is_main, s.is_dev_sandbox, s.source_template_id, s.last_reset_at,
                    s.created_by, s.created_at, s.updated_at,
                    COALESCE(h.current_version, 0) AS current_version
               FROM xb_subgraphs s
               LEFT JOIN xb_schema_heads h
                 ON h.tenant_id = s.tenant_id AND h.subgraph_slug = s.slug
              WHERE s.tenant_id = $1
              ORDER BY s.is_main DESC, s.created_at ASC, s.slug ASC",
        )
        .bind(tenant_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("list_subgraphs failed: {err}")))?;

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit list_subgraphs tx failed: {err}")))?;

        Ok(rows.iter().map(decode_subgraph_info).collect())
    }

    async fn create_subgraph(
        &self,
        tenant_id: &str,
        actor_user_id: Option<Uuid>,
        req: CreateSubgraphRequest,
    ) -> Result<SubgraphInfo, AppError> {
        let slug = req.slug.trim();
        if slug == "main" {
            return Err(AppError::Conflict("NAME_TAKEN"));
        }
        if !is_valid_subgraph_slug(slug) {
            return Err(AppError::bad(
                "NAME_INVALID",
                "subgraph slug must match ^[a-z][a-z0-9_-]{0,62}$",
            ));
        }
        let display_name = req.display_name.trim();
        if display_name.is_empty() || display_name.chars().count() > 64 {
            return Err(AppError::bad(
                "NAME_INVALID",
                "display_name must be between 1 and 64 characters",
            ));
        }

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        ensure_main_subgraph_tx(&mut tx, tenant_id).await?;

        let insert_res = sqlx::query(
            "INSERT INTO xb_subgraphs (
                 tenant_id, slug, display_name, description, owner_dept, is_main,
                 is_dev_sandbox, source_template_id, created_by
             )
             VALUES ($1, $2, $3, $4, $5, FALSE, $6, $7, $8)
             RETURNING id, slug, display_name, description, owner_dept, is_main,
                       is_dev_sandbox, source_template_id, last_reset_at,
                       created_by, created_at, updated_at, 0 AS current_version",
        )
        .bind(tenant_id)
        .bind(slug)
        .bind(display_name)
        .bind(req.description.trim())
        .bind(req.owner_dept.trim())
        .bind(req.is_dev_sandbox)
        .bind(&req.source_template_id)
        .bind(actor_user_id)
        .fetch_one(&mut *tx)
        .await;

        let mut info = match insert_res {
            Ok(row) => decode_subgraph_info(&row),
            Err(err) if is_unique_violation(&err) => {
                return Err(AppError::Conflict("NAME_TAKEN"));
            }
            Err(err) => {
                return Err(AppError::Database(format!(
                    "insert xb_subgraphs failed: {err}"
                )));
            }
        };

        sqlx::query(
            "INSERT INTO xb_schema_heads (tenant_id, subgraph_slug, current_version)
             VALUES ($1, $2, 0)
             ON CONFLICT (tenant_id, subgraph_slug) DO NOTHING",
        )
        .bind(tenant_id)
        .bind(slug)
        .execute(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("init subgraph head failed: {err}")))?;

        let should_seed = req.seed_from_main
            || req
                .seed_node_types
                .as_ref()
                .is_some_and(|list| !list.is_empty());
        if should_seed {
            let seeded_ver =
                seed_subgraph_from_main_tx(&mut tx, tenant_id, slug, actor_user_id, &req).await?;
            info.current_version = seeded_ver;
        }

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit create_subgraph tx failed: {err}"))
        })?;
        Ok(info)
    }

    async fn update_subgraph(
        &self,
        tenant_id: &str,
        slug: &str,
        req: UpdateSubgraphRequest,
    ) -> Result<SubgraphInfo, AppError> {
        if let Some(dn) = &req.display_name {
            let trimmed = dn.trim();
            if trimmed.is_empty() || trimmed.chars().count() > 64 {
                return Err(AppError::bad(
                    "NAME_INVALID",
                    "display_name must be between 1 and 64 characters",
                ));
            }
        }

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        ensure_main_subgraph_tx(&mut tx, tenant_id).await?;

        let row = sqlx::query(
            "UPDATE xb_subgraphs s
                SET display_name = COALESCE($3, s.display_name),
                    description  = COALESCE($4, s.description),
                    owner_dept   = COALESCE($5, s.owner_dept),
                    updated_at   = NOW()
              WHERE s.tenant_id = $1 AND s.slug = $2
             RETURNING s.id, s.slug, s.display_name, s.description, s.owner_dept,
                       s.is_main, s.is_dev_sandbox, s.source_template_id, s.last_reset_at,
                       s.created_by, s.created_at, s.updated_at,
                       COALESCE((SELECT current_version FROM xb_schema_heads h
                                  WHERE h.tenant_id = s.tenant_id AND h.subgraph_slug = s.slug), 0) AS current_version",
        )
        .bind(tenant_id)
        .bind(slug)
        .bind(req.display_name.as_deref().map(str::trim))
        .bind(req.description.as_deref().map(str::trim))
        .bind(req.owner_dept.as_deref().map(str::trim))
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("update xb_subgraphs failed: {err}")))?
        .ok_or(AppError::NotFound("SUBGRAPH_NOT_FOUND"))?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit update_subgraph tx failed: {err}"))
        })?;
        Ok(decode_subgraph_info(&row))
    }

    async fn delete_subgraph(&self, tenant_id: &str, slug: &str) -> Result<(), AppError> {
        if slug == "main" {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "enterprise main subgraph cannot be deleted",
            ));
        }

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let _ = require_subgraph_exists_tx(&mut tx, tenant_id, slug).await?;

        for stmt in [
            "DELETE FROM xb_metric_series WHERE tenant_id = $1 AND node_id IN (SELECT id FROM xb_nodes WHERE tenant_id = $1 AND subgraph_slug = $2)",
            "DELETE FROM xb_links WHERE tenant_id = $1 AND subgraph_slug = $2",
            "DELETE FROM xb_nodes WHERE tenant_id = $1 AND subgraph_slug = $2",
            "DELETE FROM xb_mappings WHERE tenant_id = $1 AND subgraph_slug = $2",
            "DELETE FROM xb_edge_types WHERE tenant_id = $1 AND subgraph_slug = $2",
            "DELETE FROM xb_node_types WHERE tenant_id = $1 AND subgraph_slug = $2",
            "DELETE FROM xb_schema_drafts WHERE tenant_id = $1 AND subgraph_slug = $2",
            "DELETE FROM xb_schema_versions WHERE tenant_id = $1 AND subgraph_slug = $2",
            "DELETE FROM xb_schema_heads WHERE tenant_id = $1 AND subgraph_slug = $2",
            "DELETE FROM xb_subgraphs WHERE tenant_id = $1 AND slug = $2",
        ] {
            sqlx::query(stmt)
                .bind(tenant_id)
                .bind(slug)
                .execute(&mut *tx)
                .await
                .map_err(|err| {
                    AppError::Database(format!("delete subgraph cascade failed: {err}"))
                })?;
        }

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit delete_subgraph tx failed: {err}"))
        })?;
        Ok(())
    }

    async fn reset_dev_subgraph_nodes(
        &self,
        tenant_id: &str,
        slug: &str,
        web_base_url: Option<&str>,
    ) -> Result<DevSubgraphResetResponse, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        ensure_main_subgraph_tx(&mut tx, tenant_id).await?;

        let meta_row = sqlx::query(
            "SELECT is_main, is_dev_sandbox
               FROM xb_subgraphs
              WHERE tenant_id = $1 AND slug = $2
              FOR UPDATE",
        )
        .bind(tenant_id)
        .bind(slug)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("lock xb_subgraphs for reset failed: {err}")))?
        .ok_or(AppError::NotFound("SUBGRAPH_NOT_FOUND"))?;

        let is_main: bool = meta_row.get("is_main");
        let is_dev_sandbox: bool = meta_row.get("is_dev_sandbox");
        if is_main || !is_dev_sandbox {
            return Err(AppError::bad(
                "DEV_SANDBOX_REQUIRED",
                "target subgraph must be a non-main dev sandbox",
            ));
        }

        let series_res = sqlx::query(
            "DELETE FROM xb_metric_series
              WHERE tenant_id = $1
                AND node_id IN (SELECT id FROM xb_nodes WHERE tenant_id = $1 AND subgraph_slug = $2)",
        )
        .bind(tenant_id)
        .bind(slug)
        .execute(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("reset sandbox xb_metric_series failed: {err}")))?;

        let links_res =
            sqlx::query("DELETE FROM xb_links WHERE tenant_id = $1 AND subgraph_slug = $2")
                .bind(tenant_id)
                .bind(slug)
                .execute(&mut *tx)
                .await
                .map_err(|err| {
                    AppError::Database(format!("reset sandbox xb_links failed: {err}"))
                })?;

        let nodes_res =
            sqlx::query("DELETE FROM xb_nodes WHERE tenant_id = $1 AND subgraph_slug = $2")
                .bind(tenant_id)
                .bind(slug)
                .execute(&mut *tx)
                .await
                .map_err(|err| {
                    AppError::Database(format!("reset sandbox xb_nodes failed: {err}"))
                })?;

        let reset_row = sqlx::query(
            "UPDATE xb_subgraphs
                SET last_reset_at = NOW(), updated_at = NOW()
              WHERE tenant_id = $1 AND slug = $2
             RETURNING last_reset_at",
        )
        .bind(tenant_id)
        .bind(slug)
        .fetch_one(&mut *tx)
        .await
        .map_err(|err| {
            AppError::Database(format!("update xb_subgraphs.last_reset_at failed: {err}"))
        })?;
        let last_reset_at: DateTime<Utc> = reset_row.get("last_reset_at");

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit reset_dev_subgraph_nodes tx failed: {err}"))
        })?;

        Ok(DevSubgraphResetResponse {
            subgraph_slug: String::from(slug),
            nodes_deleted: nodes_res.rows_affected(),
            links_deleted: links_res.rows_affected(),
            series_points_deleted: series_res.rows_affected(),
            last_reset_at,
            deep_links: resolve_deep_links(web_base_url, None, slug, None, None, None),
        })
    }

    async fn get_published_schema(
        &self,
        tenant_id: &str,
        subgraph_slug: &str,
    ) -> Result<PublishedSchemaResponse, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let version = require_subgraph_exists_tx(&mut tx, tenant_id, subgraph_slug).await?;
        let doc = load_schema_doc_tx(&mut tx, tenant_id, subgraph_slug).await?;
        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit get_published_schema tx failed: {err}"))
        })?;

        Ok(PublishedSchemaResponse {
            subgraph_slug: String::from(subgraph_slug),
            version,
            doc,
        })
    }

    async fn save_schema_direct(
        &self,
        tenant_id: &str,
        subgraph_slug: &str,
        actor_user_id: Option<Uuid>,
        req: SaveSchemaDirectRequest,
    ) -> Result<PublishedSchemaResponse, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let head_ver = require_subgraph_exists_tx(&mut tx, tenant_id, subgraph_slug).await?;

        let draft = insert_draft_tx(
            &mut tx,
            tenant_id,
            subgraph_slug,
            actor_user_id,
            CreateDraftRequest {
                title: String::from("direct schema save"),
                base_version: Some(req.base_version.unwrap_or(head_ver)),
                schema_doc: req.doc,
                source: Some(DraftSource::Manual),
                ai_context: None,
                subgraph_slug: None,
            },
        )
        .await?;

        let pub_resp = publish_draft_tx(
            &mut tx,
            tenant_id,
            draft.id,
            actor_user_id,
            &PublishDraftRequest {
                change_summary: req
                    .change_summary
                    .unwrap_or_else(|| String::from("direct schema save")),
                force: req.force,
                cascade: req.cascade,
            },
        )
        .await?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit save_schema_direct tx failed: {err}"))
        })?;

        Ok(PublishedSchemaResponse {
            subgraph_slug: pub_resp.subgraph_slug,
            version: pub_resp.version,
            doc: pub_resp.schema_doc,
        })
    }

    async fn load_validation_context(
        &self,
        tenant_id: &str,
        subgraph_slug: &str,
    ) -> Result<ValidationContext, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let ctx = load_validation_context_tx(&mut tx, tenant_id, subgraph_slug).await?;
        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit load_validation_context tx failed: {err}"))
        })?;
        Ok(ctx)
    }

    async fn list_schema_versions(
        &self,
        tenant_id: &str,
        subgraph_slug: &str,
    ) -> Result<Vec<SchemaVersionSummary>, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let _ = require_subgraph_exists_tx(&mut tx, tenant_id, subgraph_slug).await?;

        let rows = sqlx::query(
            "SELECT id, subgraph_slug, version, change_summary, published_by, published_at
               FROM xb_schema_versions
              WHERE tenant_id = $1 AND subgraph_slug = $2
              ORDER BY version DESC",
        )
        .bind(tenant_id)
        .bind(subgraph_slug)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("list_schema_versions failed: {err}")))?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit list_schema_versions tx failed: {err}"))
        })?;

        Ok(rows
            .into_iter()
            .map(|r| SchemaVersionSummary {
                id: r.get("id"),
                subgraph_slug: r.get("subgraph_slug"),
                version: r.get("version"),
                change_summary: r.get("change_summary"),
                published_by: r.get("published_by"),
                published_at: r.get("published_at"),
            })
            .collect())
    }

    async fn get_schema_version(
        &self,
        tenant_id: &str,
        subgraph_slug: &str,
        version: i32,
    ) -> Result<Option<SchemaVersionDetail>, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let _ = require_subgraph_exists_tx(&mut tx, tenant_id, subgraph_slug).await?;

        let row_opt = sqlx::query(
            "SELECT id, subgraph_slug, version, schema_doc, diff,
                    change_summary, published_by, published_at
               FROM xb_schema_versions
              WHERE tenant_id = $1 AND subgraph_slug = $2 AND version = $3",
        )
        .bind(tenant_id)
        .bind(subgraph_slug)
        .bind(version)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("get_schema_version failed: {err}")))?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit get_schema_version tx failed: {err}"))
        })?;

        let Some(r) = row_opt else {
            return Ok(None);
        };
        let schema_doc: SchemaDoc = serde_json::from_value(r.get("schema_doc"))
            .map_err(|err| AppError::Database(format!("decode version schema_doc: {err}")))?;
        let diff: SchemaDiff = serde_json::from_value(r.get("diff")).unwrap_or_default();

        Ok(Some(SchemaVersionDetail {
            id: r.get("id"),
            subgraph_slug: r.get("subgraph_slug"),
            version: r.get("version"),
            schema_doc,
            diff,
            change_summary: r.get("change_summary"),
            published_by: r.get("published_by"),
            published_at: r.get("published_at"),
        }))
    }

    async fn diff_schema_versions(
        &self,
        tenant_id: &str,
        subgraph_slug: &str,
        from_version: i32,
        to_version: i32,
    ) -> Result<SchemaDiff, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let _ = require_subgraph_exists_tx(&mut tx, tenant_id, subgraph_slug).await?;

        let from_doc =
            fetch_version_doc_tx(&mut tx, tenant_id, subgraph_slug, from_version).await?;
        let to_doc = fetch_version_doc_tx(&mut tx, tenant_id, subgraph_slug, to_version).await?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit diff_schema_versions tx failed: {err}"))
        })?;

        Ok(diff_schema(&from_doc, &to_doc))
    }

    async fn restore_schema_version(
        &self,
        tenant_id: &str,
        subgraph_slug: &str,
        version: i32,
        actor_user_id: Option<Uuid>,
    ) -> Result<SchemaDraft, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let head_ver = require_subgraph_exists_tx(&mut tx, tenant_id, subgraph_slug).await?;
        let target_doc = fetch_version_doc_tx(&mut tx, tenant_id, subgraph_slug, version).await?;

        let draft = insert_draft_tx(
            &mut tx,
            tenant_id,
            subgraph_slug,
            actor_user_id,
            CreateDraftRequest {
                title: format!("Restore v{version}"),
                base_version: Some(head_ver),
                schema_doc: target_doc,
                source: Some(DraftSource::Restore),
                ai_context: None,
                subgraph_slug: None,
            },
        )
        .await?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit restore_schema_version tx failed: {err}"))
        })?;
        Ok(draft)
    }

    async fn list_drafts(
        &self,
        tenant_id: &str,
        subgraph_slug: &str,
        status: Option<DraftStatus>,
    ) -> Result<Vec<SchemaDraft>, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let _ = require_subgraph_exists_tx(&mut tx, tenant_id, subgraph_slug).await?;

        let rows = sqlx::query(
            "SELECT * FROM xb_schema_drafts
              WHERE tenant_id = $1
                AND subgraph_slug = $2
                AND ($3::text IS NULL OR status = $3)
              ORDER BY updated_at DESC",
        )
        .bind(tenant_id)
        .bind(subgraph_slug)
        .bind(status.map(DraftStatus::as_str))
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("list_drafts failed: {err}")))?;

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit list_drafts tx failed: {err}")))?;

        rows.iter().map(decode_draft_row).collect()
    }

    async fn get_draft(
        &self,
        tenant_id: &str,
        draft_id: Uuid,
    ) -> Result<Option<SchemaDraft>, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let row_opt =
            sqlx::query("SELECT * FROM xb_schema_drafts WHERE tenant_id = $1 AND id = $2")
                .bind(tenant_id)
                .bind(draft_id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(|err| AppError::Database(format!("get_draft failed: {err}")))?;

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit get_draft tx failed: {err}")))?;

        row_opt.as_ref().map(decode_draft_row).transpose()
    }

    async fn create_draft(
        &self,
        tenant_id: &str,
        subgraph_slug: &str,
        actor_user_id: Option<Uuid>,
        req: CreateDraftRequest,
    ) -> Result<SchemaDraft, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let draft = insert_draft_tx(&mut tx, tenant_id, subgraph_slug, actor_user_id, req).await?;
        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit create_draft tx failed: {err}")))?;
        Ok(draft)
    }

    async fn update_draft(
        &self,
        tenant_id: &str,
        draft_id: Uuid,
        req: UpdateDraftRequest,
    ) -> Result<SchemaDraft, AppError> {
        if let Some(t) = &req.title {
            let trimmed = t.trim();
            if trimmed.is_empty() || trimmed.chars().count() > 128 {
                return Err(AppError::bad(
                    "NAME_INVALID",
                    "draft title must be between 1 and 128 characters",
                ));
            }
        }

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let existing_row = sqlx::query(
            "SELECT * FROM xb_schema_drafts WHERE tenant_id = $1 AND id = $2 FOR UPDATE",
        )
        .bind(tenant_id)
        .bind(draft_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("select draft for update failed: {err}")))?
        .ok_or(AppError::NotFound("DRAFT_NOT_FOUND"))?;

        let existing = decode_draft_row(&existing_row)?;
        if existing.status != DraftStatus::Draft {
            return Err(AppError::Conflict("VERSION_CONFLICT"));
        }

        let ctx = load_validation_context_tx(&mut tx, tenant_id, &existing.subgraph_slug).await?;
        let val_summary = validate_with_metrics(&req.schema_doc, &ctx);
        let pass_count_i32 = i32::try_from(val_summary.pass_count).unwrap_or(i32::MAX);
        let doc_json = serde_json::to_value(&req.schema_doc).map_err(|err| {
            AppError::bad("INVALID_ARGUMENT", format!("invalid schema_doc: {err}"))
        })?;
        let issues_json = serde_json::to_value(&val_summary.issues)
            .map_err(|err| AppError::Database(format!("serialize draft issues: {err}")))?;

        let updated_row = sqlx::query(
            "UPDATE xb_schema_drafts
                SET title = COALESCE($3, title),
                    schema_doc = $4,
                    issues = $5,
                    pass_count = $6,
                    updated_at = NOW()
              WHERE tenant_id = $1 AND id = $2
             RETURNING *",
        )
        .bind(tenant_id)
        .bind(draft_id)
        .bind(req.title.as_deref().map(str::trim))
        .bind(doc_json)
        .bind(issues_json)
        .bind(pass_count_i32)
        .fetch_one(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("update xb_schema_drafts failed: {err}")))?;

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit update_draft tx failed: {err}")))?;
        decode_draft_row(&updated_row)
    }

    async fn discard_draft(
        &self,
        tenant_id: &str,
        draft_id: Uuid,
    ) -> Result<SchemaDraft, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let existing_row = sqlx::query(
            "SELECT * FROM xb_schema_drafts WHERE tenant_id = $1 AND id = $2 FOR UPDATE",
        )
        .bind(tenant_id)
        .bind(draft_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("select draft for discard failed: {err}")))?
        .ok_or(AppError::NotFound("DRAFT_NOT_FOUND"))?;

        let existing = decode_draft_row(&existing_row)?;
        if existing.status != DraftStatus::Draft {
            return Err(AppError::Conflict("VERSION_CONFLICT"));
        }

        let row = sqlx::query(
            "UPDATE xb_schema_drafts
                SET status = 'DISCARDED', updated_at = NOW()
              WHERE tenant_id = $1 AND id = $2
             RETURNING *",
        )
        .bind(tenant_id)
        .bind(draft_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("discard draft failed: {err}")))?;

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit discard_draft tx failed: {err}")))?;
        decode_draft_row(&row)
    }

    async fn publish_draft(
        &self,
        tenant_id: &str,
        draft_id: Uuid,
        actor_user_id: Option<Uuid>,
        req: PublishDraftRequest,
    ) -> Result<PublishDraftResponse, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let resp = publish_draft_tx(&mut tx, tenant_id, draft_id, actor_user_id, &req).await?;
        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit publish_draft tx failed: {err}")))?;
        Ok(resp)
    }

    async fn preview_subgraph_merge(
        &self,
        tenant_id: &str,
        subgraph_slug: &str,
        req: MergePreviewRequest,
    ) -> Result<MergePreview, AppError> {
        if subgraph_slug == "main" {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "cannot preview merging main into itself",
            ));
        }

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let main_version = require_subgraph_exists_tx(&mut tx, tenant_id, "main").await?;
        let sub_version = require_subgraph_exists_tx(&mut tx, tenant_id, subgraph_slug).await?;
        let main_doc = load_schema_doc_tx(&mut tx, tenant_id, "main").await?;
        let sub_doc = load_schema_doc_tx(&mut tx, tenant_id, subgraph_slug).await?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit preview_subgraph_merge tx failed: {err}"))
        })?;

        Ok(preview_merge(
            subgraph_slug,
            sub_version,
            &sub_doc,
            main_version,
            &main_doc,
            &req,
            None,
        ))
    }

    async fn merge_subgraph_into_main(
        &self,
        tenant_id: &str,
        subgraph_slug: &str,
        actor_user_id: Option<Uuid>,
        req: MergeSubgraphRequest,
    ) -> Result<PublishDraftResponse, AppError> {
        if subgraph_slug == "main" {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "cannot merge main into itself",
            ));
        }

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let main_version = require_subgraph_exists_tx(&mut tx, tenant_id, "main").await?;
        let sub_version = require_subgraph_exists_tx(&mut tx, tenant_id, subgraph_slug).await?;

        if let Some(expected_main) = req.base_main_version
            && expected_main != main_version
        {
            return Err(AppError::Conflict("VERSION_CONFLICT"));
        }

        let sub_doc = load_schema_doc_tx(&mut tx, tenant_id, subgraph_slug).await?;
        let main_doc = load_schema_doc_tx(&mut tx, tenant_id, "main").await?;

        let merged_doc = apply_merge(subgraph_slug, &sub_doc, &main_doc, &req, None)?;
        let summary = if req.change_summary.trim().is_empty() {
            format!("merge subgraph {subgraph_slug}@v{sub_version}")
        } else {
            req.change_summary.trim().to_owned()
        };

        let merge_draft = insert_draft_tx(
            &mut tx,
            tenant_id,
            "main",
            actor_user_id,
            CreateDraftRequest {
                title: format!("Merge {subgraph_slug}@v{sub_version}"),
                base_version: Some(main_version),
                schema_doc: merged_doc,
                source: Some(DraftSource::Merge),
                ai_context: Some(serde_json::json!({
                    "source_subgraph": subgraph_slug,
                    "source_version": sub_version,
                    "default_strategy": req.default_strategy,
                })),
                subgraph_slug: None,
            },
        )
        .await?;

        let resp = publish_draft_tx(
            &mut tx,
            tenant_id,
            merge_draft.id,
            actor_user_id,
            &PublishDraftRequest {
                change_summary: summary,
                force: req.force,
                cascade: false,
            },
        )
        .await?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit merge_subgraph_into_main tx failed: {err}"))
        })?;
        Ok(resp)
    }

    async fn list_mappings(
        &self,
        tenant_id: &str,
        subgraph_slug: &str,
    ) -> Result<Vec<MappingRecord>, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let _ = require_subgraph_exists_tx(&mut tx, tenant_id, subgraph_slug).await?;

        let rows = sqlx::query(
            "SELECT m.*, d.name AS dataset_name
               FROM xb_mappings m
               JOIN ds_datasets d ON d.tenant_id = m.tenant_id AND d.id = m.dataset_id
              WHERE m.tenant_id = $1 AND m.subgraph_slug = $2
              ORDER BY m.created_at ASC, m.name ASC",
        )
        .bind(tenant_id)
        .bind(subgraph_slug)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("list_mappings failed: {err}")))?;

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit list_mappings tx failed: {err}")))?;

        rows.iter().map(decode_mapping_row).collect()
    }

    async fn get_mapping(
        &self,
        tenant_id: &str,
        mapping_id: Uuid,
    ) -> Result<Option<MappingRecord>, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let row_opt = sqlx::query(
            "SELECT m.*, d.name AS dataset_name
               FROM xb_mappings m
               JOIN ds_datasets d ON d.tenant_id = m.tenant_id AND d.id = m.dataset_id
              WHERE m.tenant_id = $1 AND m.id = $2",
        )
        .bind(tenant_id)
        .bind(mapping_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("get_mapping failed: {err}")))?;

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit get_mapping tx failed: {err}")))?;

        row_opt.as_ref().map(decode_mapping_row).transpose()
    }

    async fn list_mapping_runs(
        &self,
        tenant_id: &str,
        mapping_id: Uuid,
        limit: u32,
    ) -> Result<Vec<MappingRunRecord>, AppError> {
        let capped = i64::from(limit.clamp(1, 200));
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;

        let exists = sqlx::query("SELECT 1 FROM xb_mappings WHERE tenant_id = $1 AND id = $2")
            .bind(tenant_id)
            .bind(mapping_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|err| AppError::Database(format!("check mapping exists failed: {err}")))?;
        if exists.is_none() {
            return Err(AppError::NotFound("MAPPING_NOT_FOUND"));
        }

        let rows = sqlx::query(
            "SELECT * FROM xb_mapping_runs
              WHERE tenant_id = $1 AND mapping_id = $2
              ORDER BY started_at DESC
              LIMIT $3",
        )
        .bind(tenant_id)
        .bind(mapping_id)
        .bind(capped)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("list_mapping_runs failed: {err}")))?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit list_mapping_runs tx failed: {err}"))
        })?;

        rows.iter().map(decode_mapping_run_row).collect()
    }

    async fn enqueue_mapping_run(
        &self,
        tenant_id: &str,
        mapping_id: Uuid,
        req: TriggerMappingRunRequest,
    ) -> Result<TriggerMappingRunResponse, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let row =
            sqlx::query("SELECT subgraph_slug FROM xb_mappings WHERE tenant_id = $1 AND id = $2")
                .bind(tenant_id)
                .bind(mapping_id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(|err| AppError::Database(format!("select mapping for run failed: {err}")))?
                .ok_or(AppError::NotFound("MAPPING_NOT_FOUND"))?;

        let subgraph_slug: String = row.get("subgraph_slug");
        if subgraph_slug != "main" {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "department subgraph mappings cannot materialize runtime nodes until merged into main",
            ));
        }

        let job = NewJob {
            kind: JobKind::MapMaterialize,
            payload: serde_json::json!({
                "mapping_id": mapping_id,
                "mode": req.mode.as_str(),
            }),
            priority: 0,
            max_attempts: 3,
            next_run_at: None,
            idempotency_key: None,
        };
        let job_id = enqueue_job_tx(&mut tx, tenant_id, &job).await?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit enqueue_mapping_run tx failed: {err}"))
        })?;

        Ok(TriggerMappingRunResponse {
            job_id,
            mapping_id,
            mode: req.mode,
        })
    }

    async fn load_doc_entity_groups(
        &self,
        tenant_id: &str,
        doc_ids: &[Uuid],
    ) -> Result<Vec<DocEntityGroup>, AppError> {
        if doc_ids.is_empty() {
            return Ok(Vec::new());
        }

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let ent_rows = sqlx::query(
            "SELECT DISTINCT ent.id, ent.name, ent.entity_type, ent.occur_count
               FROM hg_paras p
               JOIN hg_edges m
                 ON m.tenant_id = p.tenant_id AND m.src_id = p.id AND m.rel_type = 'MENTIONS'
               JOIN hg_entities ent
                 ON ent.tenant_id = p.tenant_id AND ent.id = m.dst_id
              WHERE p.tenant_id = $1
                AND p.doc_id = ANY($2)
              ORDER BY ent.entity_type ASC, ent.occur_count DESC, ent.name ASC",
        )
        .bind(tenant_id)
        .bind(doc_ids)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("load_doc_entity_groups entities: {err}")))?;

        let mut entity_ids = Vec::with_capacity(ent_rows.len());
        let mut groups: BTreeMap<String, DocEntityGroup> = BTreeMap::new();
        for r in ent_rows {
            let eid: Uuid = r.get("id");
            let name: String = r.get("name");
            let etype: String = r.get("entity_type");
            entity_ids.push(eid);
            let g = groups
                .entry(etype.clone())
                .or_insert_with(|| DocEntityGroup {
                    entity_type: etype,
                    entity_names: Vec::new(),
                    relations: Vec::new(),
                });
            if g.entity_names.len() < 20 && !g.entity_names.contains(&name) {
                g.entity_names.push(name);
            }
        }

        if !entity_ids.is_empty() {
            let rel_rows = sqlx::query(
                "SELECT e1.entity_type AS src_type,
                        e2.entity_type AS dst_type,
                        COALESCE(rel.fact, '') AS fact
                   FROM hg_edges rel
                   JOIN hg_entities e1 ON e1.tenant_id = rel.tenant_id AND e1.id = rel.src_id
                   JOIN hg_entities e2 ON e2.tenant_id = rel.tenant_id AND e2.id = rel.dst_id
                  WHERE rel.tenant_id = $1
                    AND rel.rel_type = 'RELATES'
                    AND rel.src_id = ANY($2)
                    AND rel.dst_id = ANY($2)
                  ORDER BY rel.weight DESC
                  LIMIT 100",
            )
            .bind(tenant_id)
            .bind(&entity_ids)
            .fetch_all(&mut *tx)
            .await
            .map_err(|err| {
                AppError::Database(format!("load_doc_entity_groups relations: {err}"))
            })?;

            for r in rel_rows {
                let src_t: String = r.get("src_type");
                let dst_t: String = r.get("dst_type");
                let fact: String = r.get("fact");
                if let Some(g) = groups.get_mut(&src_t)
                    && g.relations.len() < 15
                {
                    g.relations.push((src_t, dst_t, fact));
                }
            }
        }

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit load_doc_entity_groups tx failed: {err}"))
        })?;
        Ok(groups.into_values().collect())
    }

    async fn list_draft_snapshots(
        &self,
        tenant_id: &str,
        draft_id: Uuid,
        limit: u32,
    ) -> Result<Vec<DraftSnapshotSummary>, AppError> {
        let capped = i64::from(limit.clamp(1, 100));
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;

        let exists = sqlx::query("SELECT 1 FROM xb_schema_drafts WHERE tenant_id = $1 AND id = $2")
            .bind(tenant_id)
            .bind(draft_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|err| AppError::Database(format!("check draft exists failed: {err}")))?;
        if exists.is_none() {
            return Err(AppError::NotFound("DRAFT_NOT_FOUND"));
        }

        let rows = sqlx::query(
            "SELECT id, draft_id, snapshot_no, reason, created_by, created_at
               FROM xb_schema_draft_snapshots
              WHERE tenant_id = $1 AND draft_id = $2
              ORDER BY snapshot_no DESC
              LIMIT $3",
        )
        .bind(tenant_id)
        .bind(draft_id)
        .bind(capped)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("list_draft_snapshots failed: {err}")))?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit list_draft_snapshots tx failed: {err}"))
        })?;

        rows.iter().map(decode_snapshot_summary_row).collect()
    }

    async fn apply_copilot_patch_with_snapshot(
        &self,
        tenant_id: &str,
        draft_id: Uuid,
        actor_user_id: Option<Uuid>,
        patched_doc: SchemaDoc,
    ) -> Result<(DraftSnapshotSummary, SchemaDraft), AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let existing = lock_editable_draft_tx(&mut tx, tenant_id, draft_id).await?;

        let snap = insert_draft_snapshot_tx(
            &mut tx,
            tenant_id,
            draft_id,
            DraftSnapshotReason::CopilotPatch,
            &existing.schema_doc,
            actor_user_id,
        )
        .await?;

        let updated = persist_draft_doc_tx(
            &mut tx,
            tenant_id,
            draft_id,
            &existing.subgraph_slug,
            &patched_doc,
        )
        .await?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!(
                "commit apply_copilot_patch_with_snapshot tx failed: {err}"
            ))
        })?;

        Ok((snap, updated))
    }

    async fn restore_draft_snapshot(
        &self,
        tenant_id: &str,
        draft_id: Uuid,
        snapshot_id: Uuid,
    ) -> Result<SchemaDraft, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let existing = lock_editable_draft_tx(&mut tx, tenant_id, draft_id).await?;

        let snap_row = sqlx::query(
            "SELECT schema_doc FROM xb_schema_draft_snapshots
              WHERE tenant_id = $1 AND draft_id = $2 AND id = $3",
        )
        .bind(tenant_id)
        .bind(draft_id)
        .bind(snapshot_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("load draft snapshot failed: {err}")))?
        .ok_or(AppError::NotFound("STUDIO_SNAPSHOT_NOT_FOUND"))?;

        let restored_doc: SchemaDoc = serde_json::from_value(snap_row.get("schema_doc"))
            .map_err(|err| AppError::Database(format!("decode snapshot schema_doc: {err}")))?;

        let updated = persist_draft_doc_tx(
            &mut tx,
            tenant_id,
            draft_id,
            &existing.subgraph_slug,
            &restored_doc,
        )
        .await?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit restore_draft_snapshot tx failed: {err}"))
        })?;

        Ok(updated)
    }

    async fn confirm_draft_proposals(
        &self,
        tenant_id: &str,
        draft_id: Uuid,
        actor_user_id: Option<Uuid>,
        req: ConfirmProposalsRequest,
    ) -> Result<SchemaDraft, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let existing = lock_editable_draft_tx(&mut tx, tenant_id, draft_id).await?;

        let _ = insert_draft_snapshot_tx(
            &mut tx,
            tenant_id,
            draft_id,
            DraftSnapshotReason::BatchConfirm,
            &existing.schema_doc,
            actor_user_id,
        )
        .await?;

        let mut confirmed_doc = existing.schema_doc.clone();
        confirmed_doc.confirm_proposals(&req);

        let updated = persist_draft_doc_tx(
            &mut tx,
            tenant_id,
            draft_id,
            &existing.subgraph_slug,
            &confirmed_doc,
        )
        .await?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit confirm_draft_proposals tx failed: {err}"))
        })?;

        Ok(updated)
    }

    async fn list_node_downstream_consumers(
        &self,
        tenant_id: &str,
        node_type: &str,
    ) -> Result<Vec<DownstreamConsumerBinding>, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let mut out = Vec::new();
        query_rule_consumers_tx(&mut tx, tenant_id, node_type, &mut out).await?;
        query_endpoint_consumers_tx(&mut tx, tenant_id, node_type, &mut out).await?;
        query_dashboard_consumers_tx(&mut tx, tenant_id, node_type, &mut out).await?;
        tx.commit().await.map_err(|err| {
            AppError::Database(format!(
                "commit list_node_downstream_consumers tx failed: {err}"
            ))
        })?;
        Ok(out)
    }
}

const MAX_DRAFT_SNAPSHOTS_RETAINED: i32 = 20;

fn decode_snapshot_summary_row(
    row: &sqlx::postgres::PgRow,
) -> Result<DraftSnapshotSummary, AppError> {
    let reason = DraftSnapshotReason::from_str(&row.get::<String, _>("reason"))?;
    Ok(DraftSnapshotSummary {
        id: row.get("id"),
        draft_id: row.get("draft_id"),
        snapshot_no: row.get("snapshot_no"),
        reason,
        created_by: row.get("created_by"),
        created_at: row.get("created_at"),
    })
}

async fn lock_editable_draft_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    draft_id: Uuid,
) -> Result<SchemaDraft, AppError> {
    let row =
        sqlx::query("SELECT * FROM xb_schema_drafts WHERE tenant_id = $1 AND id = $2 FOR UPDATE")
            .bind(tenant_id)
            .bind(draft_id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(|err| AppError::Database(format!("lock xb_schema_drafts failed: {err}")))?
            .ok_or(AppError::NotFound("DRAFT_NOT_FOUND"))?;

    let draft = decode_draft_row(&row)?;
    if draft.status != DraftStatus::Draft {
        return Err(AppError::Conflict("VERSION_CONFLICT"));
    }
    Ok(draft)
}

async fn insert_draft_snapshot_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    draft_id: Uuid,
    reason: DraftSnapshotReason,
    pre_doc: &SchemaDoc,
    actor_user_id: Option<Uuid>,
) -> Result<DraftSnapshotSummary, AppError> {
    let next_row = sqlx::query(
        "SELECT COALESCE(MAX(snapshot_no), 0) + 1 AS next_no
           FROM xb_schema_draft_snapshots
          WHERE tenant_id = $1 AND draft_id = $2",
    )
    .bind(tenant_id)
    .bind(draft_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("compute next snapshot_no failed: {err}")))?;
    let next_no: i32 = next_row.get("next_no");

    let doc_json = serde_json::to_value(pre_doc)
        .map_err(|err| AppError::Database(format!("serialize snapshot schema_doc: {err}")))?;

    let snap_row = sqlx::query(
        "INSERT INTO xb_schema_draft_snapshots (
             tenant_id, draft_id, snapshot_no, reason, schema_doc, created_by
         )
         VALUES ($1, $2, $3, $4, $5, $6)
         RETURNING id, draft_id, snapshot_no, reason, created_by, created_at",
    )
    .bind(tenant_id)
    .bind(draft_id)
    .bind(next_no)
    .bind(reason.as_str())
    .bind(doc_json)
    .bind(actor_user_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("insert xb_schema_draft_snapshots failed: {err}")))?;

    let prune_cutoff = next_no.saturating_sub(MAX_DRAFT_SNAPSHOTS_RETAINED);
    if prune_cutoff > 0 {
        sqlx::query(
            "DELETE FROM xb_schema_draft_snapshots
              WHERE tenant_id = $1 AND draft_id = $2 AND snapshot_no <= $3",
        )
        .bind(tenant_id)
        .bind(draft_id)
        .bind(prune_cutoff)
        .execute(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("prune draft snapshots failed: {err}")))?;
    }

    decode_snapshot_summary_row(&snap_row)
}

async fn persist_draft_doc_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    draft_id: Uuid,
    subgraph_slug: &str,
    next_doc: &SchemaDoc,
) -> Result<SchemaDraft, AppError> {
    let ctx = load_validation_context_tx(tx, tenant_id, subgraph_slug).await?;
    let val_summary = validate_with_metrics(next_doc, &ctx);
    let pass_count_i32 = i32::try_from(val_summary.pass_count).unwrap_or(i32::MAX);
    let doc_json = serde_json::to_value(next_doc)
        .map_err(|err| AppError::bad("INVALID_ARGUMENT", format!("invalid schema_doc: {err}")))?;
    let issues_json = serde_json::to_value(&val_summary.issues)
        .map_err(|err| AppError::Database(format!("serialize draft issues: {err}")))?;

    let row = sqlx::query(
        "UPDATE xb_schema_drafts
            SET schema_doc = $3,
                issues = $4,
                pass_count = $5,
                updated_at = NOW()
          WHERE tenant_id = $1 AND id = $2
         RETURNING *",
    )
    .bind(tenant_id)
    .bind(draft_id)
    .bind(doc_json)
    .bind(issues_json)
    .bind(pass_count_i32)
    .fetch_one(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("update xb_schema_drafts failed: {err}")))?;

    decode_draft_row(&row)
}

async fn query_rule_consumers_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    node_type: &str,
    out: &mut Vec<DownstreamConsumerBinding>,
) -> Result<(), AppError> {
    let rows = sqlx::query(
        "SELECT name, description, title_template, target->>'metric' AS metric
           FROM ax_rules
          WHERE tenant_id = $1
            AND scope_kind = 'METRIC'
            AND (target->>'type_name' = $2 OR target->>'type_name' IS NULL)
          ORDER BY name ASC",
    )
    .bind(tenant_id)
    .bind(node_type)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("query ax_rules consumers failed: {err}")))?;

    for r in rows {
        let name: String = r.get("name");
        let desc: String = r.get("description");
        let title_tpl: String = r.get("title_template");
        let metric_opt: Option<String> = r.get("metric");
        let label = if !desc.trim().is_empty() {
            desc
        } else if !title_tpl.trim().is_empty() {
            title_tpl
        } else {
            name.clone()
        };
        out.push(DownstreamConsumerBinding {
            id: format!("rule:{name}"),
            label,
            consumer_kind: DownstreamConsumerKind::AlertRule,
            target_metric: metric_opt.filter(|m| !m.trim().is_empty()),
        });
    }
    Ok(())
}

async fn query_endpoint_consumers_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    node_type: &str,
    out: &mut Vec<DownstreamConsumerBinding>,
) -> Result<(), AppError> {
    let rows = sqlx::query(
        "SELECT slug, environment
           FROM ax_published_endpoints
          WHERE tenant_id = $1
            AND kind = 'OBJECT_TYPE'
            AND target_id = $2
          ORDER BY slug ASC, environment ASC",
    )
    .bind(tenant_id)
    .bind(node_type)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| {
        AppError::Database(format!(
            "query ax_published_endpoints consumers failed: {err}"
        ))
    })?;

    let mut seen_ids = BTreeSet::new();
    for r in rows {
        let slug: String = r.get("slug");
        let env: String = r.get("environment");
        let base_id = format!("ep:{slug}");
        let id = if seen_ids.insert(base_id.clone()) {
            base_id
        } else {
            format!("ep:{slug}:{}", env.to_ascii_lowercase())
        };
        out.push(DownstreamConsumerBinding {
            id,
            label: format!("{slug} ({env})"),
            consumer_kind: DownstreamConsumerKind::Endpoint,
            target_metric: None,
        });
    }
    Ok(())
}

async fn query_dashboard_consumers_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    node_type: &str,
    out: &mut Vec<DownstreamConsumerBinding>,
) -> Result<(), AppError> {
    let rows = sqlx::query(
        "SELECT DISTINCT d.slug, d.name
           FROM xv_dashboards d
           JOIN xv_widgets w
             ON w.tenant_id = d.tenant_id AND w.dashboard_id = d.id
          WHERE d.tenant_id = $1
            AND jsonb_typeof(w.deps->'type_names') = 'array'
            AND (w.deps->'type_names') ? $2
          ORDER BY d.slug ASC",
    )
    .bind(tenant_id)
    .bind(node_type)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("query xv_dashboards consumers failed: {err}")))?;

    for r in rows {
        let slug: String = r.get("slug");
        let name: String = r.get("name");
        out.push(DownstreamConsumerBinding {
            id: format!("dash:{slug}"),
            label: name,
            consumer_kind: DownstreamConsumerKind::Dashboard,
            target_metric: None,
        });
    }
    Ok(())
}

async fn fetch_version_doc_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    subgraph_slug: &str,
    version: i32,
) -> Result<SchemaDoc, AppError> {
    let row = sqlx::query(
        "SELECT schema_doc FROM xb_schema_versions
          WHERE tenant_id = $1 AND subgraph_slug = $2 AND version = $3",
    )
    .bind(tenant_id)
    .bind(subgraph_slug)
    .bind(version)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("fetch schema version {version} failed: {err}")))?
    .ok_or(AppError::NotFound("VERSION_NOT_FOUND"))?;

    serde_json::from_value(row.get("schema_doc"))
        .map_err(|err| AppError::Database(format!("decode version {version} schema_doc: {err}")))
}

async fn seed_subgraph_from_main_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    subgraph_slug: &str,
    actor_user_id: Option<Uuid>,
    req: &CreateSubgraphRequest,
) -> Result<i32, AppError> {
    let main_doc = load_schema_doc_tx(tx, tenant_id, "main").await?;
    let selected_nodes: Vec<NodeTypeSpec> = if let Some(subset) = &req.seed_node_types {
        let mut out = Vec::with_capacity(subset.len());
        for wanted in subset {
            let mut found = main_doc
                .node_types
                .iter()
                .find(|nt| nt.name == *wanted)
                .cloned()
                .ok_or_else(|| {
                    AppError::bad(
                        "TYPE_NOT_FOUND",
                        format!("seed node type '{wanted}' does not exist in main"),
                    )
                })?;
            found.id = None;
            out.push(found);
        }
        out
    } else {
        main_doc
            .node_types
            .into_iter()
            .map(|mut nt| {
                nt.id = None;
                nt
            })
            .collect()
    };

    if selected_nodes.is_empty() {
        return Ok(0);
    }

    let kept_names: BTreeSet<&str> = selected_nodes.iter().map(|n| n.name.as_str()).collect();
    let selected_edges: Vec<EdgeTypeSpec> = main_doc
        .edge_types
        .into_iter()
        .filter(|e| {
            kept_names.contains(e.src_type_name.as_str())
                && kept_names.contains(e.dst_type_name.as_str())
        })
        .map(|mut e| {
            e.id = None;
            e
        })
        .collect();

    let draft = insert_draft_tx(
        tx,
        tenant_id,
        subgraph_slug,
        actor_user_id,
        CreateDraftRequest {
            title: String::from("Initial seed from main"),
            base_version: Some(0),
            schema_doc: SchemaDoc {
                node_types: selected_nodes,
                edge_types: selected_edges,
                mappings: Vec::new(),
            },
            source: Some(DraftSource::Manual),
            ai_context: None,
            subgraph_slug: None,
        },
    )
    .await?;

    let pub_resp = publish_draft_tx(
        tx,
        tenant_id,
        draft.id,
        actor_user_id,
        &PublishDraftRequest {
            change_summary: String::from("seed from main"),
            force: true,
            cascade: false,
        },
    )
    .await?;
    Ok(pub_resp.version)
}
