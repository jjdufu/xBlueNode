//! Dataset-to-graph full and incremental mapping materialization executor (`06-STEP05-06` §06.2).

use std::collections::{BTreeMap, BTreeSet};
use std::num::FpCategory;
use std::str::FromStr;

use axpoint_domain::AppError;
use axpoint_domain::axhub::ColumnProfile;
use axpoint_domain::enums::{DataType, JobKind, OutboxTopic};
use axpoint_domain::expr::{
    BinaryOp, EvalEnv, Expr, ExprValue, UnaryOp, collect_references, eval as eval_expr,
    parse as parse_expr,
};
use axpoint_domain::graph::GeoPoint;
use axpoint_domain::platform::{NewJob, NewOutboxEvent};
use axpoint_domain::ports::LakeInternalQueryPort;
use axpoint_domain::rollup::{KnowledgeSyncJobPayload, MapMaterializeJobPayload};
use axpoint_domain::studio::{
    EdgeTypeSpec, LinkDirection, MappingMode, MappingRecord, MappingRejectSample, MappingRunRecord,
    MappingRunStatus, MappingSpec, NodeTypeSpec, OnSourceDelete, PropertyKind, PropertySpec,
};
use chrono::{DateTime, NaiveDate, NaiveDateTime, Timelike, Utc};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

use crate::PgStore;
use crate::jobs::enqueue_job_tx;
use crate::outbox::insert_outbox_tx;
use crate::tenant::begin_tenant_tx;

const CHUNK_ROWS: usize = 5_000;
const MAX_REJECT_SAMPLES: usize = 20;

type JsonRow = serde_json::Map<String, serde_json::Value>;
type DuckLakeArgs = (String, String, String);
type LakeRefCandidate = (String, Option<DuckLakeArgs>);

struct DatasetExecMeta {
    id: Uuid,
    name: String,
    lake_schema: String,
    lake_table: String,
    last_snapshot_id: Option<i64>,
    schema_columns: BTreeSet<String>,
}

struct MainModelLookup {
    node_types: BTreeMap<String, NodeTypeSpec>,
    edge_weights: BTreeMap<(String, String, String), f64>,
}

struct ResolvedLakeTable {
    full_ref: String,
    ducklake_args: Option<(String, String, String)>,
    physical_columns: BTreeSet<String>,
}

#[derive(Default)]
struct RunCounters {
    nodes_upserted: i64,
    nodes_deleted: i64,
    links_upserted: i64,
    points_written: i64,
    rows_rejected: i64,
    reject_samples: Vec<MappingRejectSample>,
}

impl RunCounters {
    fn record_reject(&mut self, row: &JsonRow, reason: String) {
        self.rows_rejected = self.rows_rejected.saturating_add(1);
        if self.reject_samples.len() < MAX_REJECT_SAMPLES {
            let mut clean = serde_json::Map::new();
            for (k, v) in row {
                if !matches!(k.as_str(), "snapshot_id" | "rowid" | "change_type") {
                    clean.insert(k.clone(), v.clone());
                }
            }
            self.reject_samples.push(MappingRejectSample {
                row: serde_json::Value::Object(clean),
                reason,
            });
        }
    }
}

#[derive(Clone)]
struct CoercedLink {
    link_type: String,
    direction: LinkDirection,
    target_type: String,
    target_key: String,
    weight: f64,
    create_missing_target: bool,
}

#[derive(Clone)]
struct CoercedRow {
    node_key: String,
    title: String,
    attributes: serde_json::Map<String, serde_json::Value>,
    metrics: BTreeMap<String, f64>,
    metric_ts: DateTime<Utc>,
    geo: Option<GeoPoint>,
    links: Vec<CoercedLink>,
}

struct ChunkExecCtx<'a> {
    tenant_id: &'a str,
    run_id: Uuid,
    mapping: &'a MappingRecord,
    ds_meta: &'a DatasetExecMeta,
    model: &'a MainModelLookup,
    eff_mode: MappingMode,
    filter_ast: Option<&'a Expr>,
}

/// Lists enabled `main` subgraph mappings bound to `dataset_id` (`06-STEP05-06` §06.2.3).
///
/// # Errors
/// Returns [`AppError::Database`] if querying `xb_mappings` fails.
pub async fn list_enabled_main_mappings_for_dataset_impl(
    store: &PgStore,
    tenant_id: &str,
    dataset_id: Uuid,
) -> Result<Vec<MappingRecord>, AppError> {
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let rows = sqlx::query(
        "SELECT m.id, m.subgraph_slug, m.name, m.dataset_id, d.name AS dataset_name,
                m.node_type, m.spec, m.enabled, m.schema_version, m.last_snapshot_id,
                m.created_at, m.updated_at
           FROM xb_mappings m
           JOIN ds_datasets d ON d.id = m.dataset_id AND d.tenant_id = m.tenant_id
          WHERE m.tenant_id = $1
            AND m.subgraph_slug = 'main'
            AND m.dataset_id = $2
            AND m.enabled = TRUE
          ORDER BY m.name ASC",
    )
    .bind(tenant_id)
    .bind(dataset_id)
    .fetch_all(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("list enabled main mappings failed: {err}")))?;

    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(decode_mapping_row(row)?);
    }
    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit list enabled mappings failed: {err}")))?;
    Ok(out)
}

/// Executes `MAP_MATERIALIZE` (`FULL` or `INCREMENTAL`) for `payload.mapping_id` (`06-STEP05-06` §06.2).
///
/// # Errors
/// Returns [`AppError`] if the mapping or dataset cannot be loaded, referenced columns are missing,
/// or `DuckDB` / `PostgreSQL` execution fails.
pub async fn execute_mapping_materialize_impl<L: LakeInternalQueryPort>(
    store: &PgStore,
    tenant_id: &str,
    job_id: Uuid,
    payload: &MapMaterializeJobPayload,
    lake: &L,
) -> Result<MappingRunRecord, AppError> {
    let (mapping, ds_meta, model) =
        load_mapping_and_model(store, tenant_id, payload.mapping_id).await?;
    if mapping.subgraph_slug != "main" {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "only main subgraph mappings can materialize runtime graph nodes",
        ));
    }

    if let Some(missing_msg) =
        check_missing_columns(&mapping.spec, &ds_meta.schema_columns, &ds_meta.name)
    {
        persist_schema_broken_failure(
            store,
            tenant_id,
            mapping.id,
            job_id,
            payload.mode,
            &missing_msg,
        )
        .await?;
        return Err(AppError::Unprocessable {
            code: "MAPPING_SCHEMA_BROKEN",
            message: missing_msg,
        });
    }

    if !model.node_types.contains_key(&mapping.node_type) {
        let msg = format!(
            "target node_type '{}' is not published in main",
            mapping.node_type
        );
        persist_schema_broken_failure(store, tenant_id, mapping.id, job_id, payload.mode, &msg)
            .await?;
        return Err(AppError::bad("MAPPING_INVALID", msg));
    }

    let _ = lake.invalidate_tenant_session(tenant_id).await;
    let resolved = resolve_lake_table(lake, tenant_id, &ds_meta).await?;
    if let Some(missing_msg) =
        check_missing_columns(&mapping.spec, &resolved.physical_columns, &ds_meta.name)
    {
        persist_schema_broken_failure(
            store,
            tenant_id,
            mapping.id,
            job_id,
            payload.mode,
            &missing_msg,
        )
        .await?;
        return Err(AppError::Unprocessable {
            code: "MAPPING_SCHEMA_BROKEN",
            message: missing_msg,
        });
    }

    let (eff_mode, from_snap, to_snap) =
        select_execution_mode(lake, tenant_id, payload.mode, &mapping, &ds_meta, &resolved).await;

    let run_id = create_running_record(
        store,
        tenant_id,
        mapping.id,
        job_id,
        eff_mode,
        from_snap,
        to_snap,
    )
    .await?;
    let mut counters = RunCounters::default();

    let exec_res = run_streaming_pipeline(
        store,
        ChunkExecCtx {
            tenant_id,
            run_id,
            mapping: &mapping,
            ds_meta: &ds_meta,
            model: &model,
            eff_mode,
            filter_ast: None,
        },
        &resolved,
        (from_snap, to_snap),
        lake,
        &mut counters,
    )
    .await;

    match exec_res {
        Ok(()) => {
            finalize_succeeded_run(store, tenant_id, mapping.id, run_id, to_snap, &counters).await
        }
        Err(err) => {
            let _ = finalize_failed_run(store, tenant_id, run_id, &err.client_message(), &counters)
                .await;
            Err(err)
        }
    }
}

async fn run_streaming_pipeline<L: LakeInternalQueryPort>(
    store: &PgStore,
    base_ctx: ChunkExecCtx<'_>,
    resolved: &ResolvedLakeTable,
    snap_window: (Option<i64>, Option<i64>),
    lake: &L,
    counters: &mut RunCounters,
) -> Result<(), AppError> {
    let (from_snap, to_snap) = snap_window;
    if base_ctx.eff_mode == MappingMode::Incremental
        && let (Some(f), Some(t)) = (from_snap, to_snap)
        && f >= t
    {
        return Ok(());
    }

    let parsed_filter = match base_ctx.mapping.spec.filter.as_deref().map(str::trim) {
        Some(f) if !f.is_empty() => {
            let raw_ast =
                parse_expr(f).map_err(|issue| AppError::bad("EXPR_INVALID", issue.message))?;
            Some(normalize_mapping_filter_ast(raw_ast))
        }
        _ => None,
    };

    let sql_where = if base_ctx.eff_mode == MappingMode::Full {
        parsed_filter.as_ref().and_then(try_translate_duckdb_filter)
    } else {
        None
    };

    let ctx = ChunkExecCtx {
        filter_ast: parsed_filter.as_ref(),
        ..base_ctx
    };

    let mut seen_keys = BTreeSet::new();
    let mut offset = 0_usize;

    loop {
        let query_sql = build_chunk_sql(
            ctx.eff_mode,
            resolved,
            from_snap,
            to_snap,
            sql_where.as_deref(),
            offset,
        )?;
        let chunk_res = lake
            .query_internal(ctx.tenant_id, &query_sql, CHUNK_ROWS)
            .await?;
        let fetched = chunk_res.rows.len();
        if fetched == 0 {
            break;
        }

        process_single_chunk(store, &ctx, &chunk_res.rows, &mut seen_keys, counters).await?;

        if fetched < CHUNK_ROWS {
            break;
        }
        offset = offset.saturating_add(CHUNK_ROWS);
    }

    if ctx.eff_mode == MappingMode::Full
        && ctx.mapping.spec.on_source_delete == OnSourceDelete::Delete
    {
        let sync_k = ctx
            .model
            .node_types
            .get(&ctx.mapping.node_type)
            .is_some_and(|nt| nt.sync_to_knowledge);
        let deleted = prune_unseen_full_nodes(
            store,
            ctx.tenant_id,
            &ctx.mapping.node_type,
            ctx.ds_meta.id,
            &seen_keys,
            sync_k,
        )
        .await?;
        counters.nodes_deleted = counters.nodes_deleted.saturating_add(deleted);
    }

    Ok(())
}

async fn process_single_chunk(
    store: &PgStore,
    ctx: &ChunkExecCtx<'_>,
    rows: &[JsonRow],
    seen_keys: &mut BTreeSet<String>,
    counters: &mut RunCounters,
) -> Result<(), AppError> {
    let Some(target_nt) = ctx.model.node_types.get(&ctx.mapping.node_type) else {
        return Err(AppError::bad(
            "MAPPING_INVALID",
            format!("node_type '{}' missing in main", ctx.mapping.node_type),
        ));
    };

    let chunk_now = Utc::now();
    let mut upserts_by_key: BTreeMap<String, CoercedRow> = BTreeMap::new();
    let mut delete_keys: BTreeSet<String> = BTreeSet::new();

    for row in rows {
        let change_kind = if ctx.eff_mode == MappingMode::Incremental {
            row.get("change_type")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("insert")
        } else {
            "insert"
        };

        if change_kind == "delete" {
            if let Some(k) = extract_non_empty_key(row, &ctx.mapping.spec.key_column) {
                let _ = upserts_by_key.remove(&k);
                if ctx.mapping.spec.on_source_delete == OnSourceDelete::Delete {
                    delete_keys.insert(k);
                }
            }
            continue;
        }

        if let Some(f_ast) = ctx.filter_ast
            && !eval_row_filter(f_ast, row)
        {
            if change_kind == "update_postimage"
                && ctx.mapping.spec.on_source_delete == OnSourceDelete::Delete
                && let Some(k) = extract_non_empty_key(row, &ctx.mapping.spec.key_column)
            {
                let _ = upserts_by_key.remove(&k);
                delete_keys.insert(k);
            }
            continue;
        }

        match coerce_mapping_row(
            row,
            &ctx.mapping.spec,
            target_nt,
            &ctx.model.edge_weights,
            chunk_now,
        ) {
            Ok(coerced) => {
                let _ = delete_keys.remove(&coerced.node_key);
                seen_keys.insert(coerced.node_key.clone());
                upserts_by_key.insert(coerced.node_key.clone(), coerced);
            }
            Err(reason) => counters.record_reject(row, reason),
        }
    }

    if upserts_by_key.is_empty() && delete_keys.is_empty() {
        return Ok(());
    }

    commit_chunk_tx(store, ctx, target_nt, upserts_by_key, delete_keys, counters).await
}

async fn commit_chunk_tx(
    store: &PgStore,
    ctx: &ChunkExecCtx<'_>,
    target_nt: &NodeTypeSpec,
    upserts_by_key: BTreeMap<String, CoercedRow>,
    delete_keys: BTreeSet<String>,
    counters: &mut RunCounters,
) -> Result<(), AppError> {
    let mut tx = begin_tenant_tx(store.pool(), ctx.tenant_id).await?;
    let mut dirty_agg_nodes: BTreeSet<Uuid> = BTreeSet::new();
    let mut changed_metric_names: BTreeSet<String> = BTreeSet::new();

    if !delete_keys.is_empty() {
        let del_vec: Vec<String> = delete_keys.into_iter().collect();
        let (del_cnt, affected_neighbors) = delete_nodes_by_keys_tx(
            &mut tx,
            ctx.tenant_id,
            &target_nt.name,
            &del_vec,
            target_nt.sync_to_knowledge,
        )
        .await?;
        counters.nodes_deleted = counters.nodes_deleted.saturating_add(del_cnt);
        dirty_agg_nodes.extend(affected_neighbors);
    }

    let mut upserted_ids = Vec::new();
    if !upserts_by_key.is_empty() {
        let batch_rows: Vec<CoercedRow> = upserts_by_key.into_values().collect();
        let (key_to_id, metric_dirty_ids, pts_cnt, m_names) = upsert_mapped_nodes_tx(
            &mut tx,
            ctx.tenant_id,
            ctx.run_id,
            ctx.ds_meta.id,
            &target_nt.name,
            &batch_rows,
        )
        .await?;

        upserted_ids.extend(key_to_id.values().copied());
        dirty_agg_nodes.extend(metric_dirty_ids);
        changed_metric_names.extend(m_names);
        counters.nodes_upserted = counters
            .nodes_upserted
            .saturating_add(i64::try_from(key_to_id.len()).unwrap_or(i64::MAX));
        counters.points_written = counters.points_written.saturating_add(pts_cnt);

        let (link_cnt, link_dirty_ids) = upsert_mapped_links_tx(
            &mut tx,
            ctx.tenant_id,
            ctx.ds_meta.id,
            &batch_rows,
            &key_to_id,
        )
        .await?;
        counters.links_upserted = counters.links_upserted.saturating_add(link_cnt);
        dirty_agg_nodes.extend(link_dirty_ids);
    }

    emit_chunk_events_and_jobs_tx(
        &mut tx,
        ctx.tenant_id,
        &target_nt.name,
        &upserted_ids,
        &dirty_agg_nodes,
        &changed_metric_names,
    )
    .await?;

    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit mapping chunk tx failed: {err}")))?;
    Ok(())
}

async fn upsert_mapped_nodes_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    run_id: Uuid,
    dataset_id: Uuid,
    type_name: &str,
    batch_rows: &[CoercedRow],
) -> Result<(BTreeMap<String, Uuid>, BTreeSet<Uuid>, i64, BTreeSet<String>), AppError> {
    let keys: Vec<String> = batch_rows.iter().map(|r| r.node_key.clone()).collect();
    let existing_rows = sqlx::query(
        "SELECT id, node_key, properties, metrics
           FROM xb_nodes
          WHERE tenant_id = $1 AND type_name = $2 AND node_key = ANY($3)
          FOR UPDATE",
    )
    .bind(tenant_id)
    .bind(type_name)
    .bind(&keys)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("preload existing mapped nodes failed: {err}")))?;

    let mut existing_map: BTreeMap<String, (serde_json::Value, serde_json::Value)> =
        BTreeMap::new();
    for er in existing_rows {
        let k: String = er.get("node_key");
        existing_map.insert(k, (er.get("properties"), er.get("metrics")));
    }

    let cap = batch_rows.len();
    let mut u_keys = Vec::with_capacity(cap);
    let mut u_titles = Vec::with_capacity(cap);
    let mut u_props = Vec::with_capacity(cap);
    let mut u_metrics = Vec::with_capacity(cap);
    let mut u_lngs: Vec<Option<f64>> = Vec::with_capacity(cap);
    let mut u_lats: Vec<Option<f64>> = Vec::with_capacity(cap);
    let mut dirty_keys = BTreeSet::new();
    let mut dirty_metric_names = BTreeSet::new();

    for row in batch_rows {
        let (merged_props, merged_metrics, changed) =
            merge_node_state(row, existing_map.get(&row.node_key))?;
        if changed {
            dirty_keys.insert(row.node_key.clone());
            dirty_metric_names.extend(row.metrics.keys().cloned());
        }
        u_keys.push(row.node_key.clone());
        u_titles.push(row.title.clone());
        u_props.push(merged_props);
        u_metrics.push(merged_metrics);
        u_lngs.push(row.geo.map(|g| g.lng));
        u_lats.push(row.geo.map(|g| g.lat));
    }

    let returned = sqlx::query(
        "INSERT INTO xb_nodes (
             tenant_id, type_name, node_key, title, properties, metrics,
             geo_lng, geo_lat, source_dataset_id, source_row_key
         )
         SELECT $1, $2, u.node_key, u.title, u.properties, u.metrics,
                u.geo_lng, u.geo_lat, $3, u.node_key
           FROM UNNEST(
               $4::text[], $5::text[], $6::jsonb[], $7::jsonb[], $8::float8[], $9::float8[]
           ) AS u(node_key, title, properties, metrics, geo_lng, geo_lat)
         ON CONFLICT (tenant_id, type_name, node_key) DO UPDATE SET
             title = EXCLUDED.title,
             properties = EXCLUDED.properties,
             metrics = EXCLUDED.metrics,
             geo_lng = EXCLUDED.geo_lng,
             geo_lat = EXCLUDED.geo_lat,
             source_dataset_id = EXCLUDED.source_dataset_id,
             source_row_key = EXCLUDED.source_row_key,
             row_version = xb_nodes.row_version + 1,
             updated_at = NOW()
         RETURNING id, node_key",
    )
    .bind(tenant_id)
    .bind(type_name)
    .bind(dataset_id)
    .bind(&u_keys)
    .bind(&u_titles)
    .bind(&u_props)
    .bind(&u_metrics)
    .bind(&u_lngs)
    .bind(&u_lats)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("batch upsert mapped xb_nodes failed: {err}")))?;

    let mut key_to_id = BTreeMap::new();
    let mut dirty_ids = BTreeSet::new();
    for r in returned {
        let id: Uuid = r.get("id");
        let k: String = r.get("node_key");
        if dirty_keys.contains(&k) {
            dirty_ids.insert(id);
        }
        key_to_id.insert(k, id);
    }

    let pts = write_mapped_metric_series_tx(tx, tenant_id, run_id, batch_rows, &key_to_id).await?;
    Ok((key_to_id, dirty_ids, pts, dirty_metric_names))
}

fn merge_node_state(
    row: &CoercedRow,
    existing: Option<&(serde_json::Value, serde_json::Value)>,
) -> Result<(serde_json::Value, serde_json::Value, bool), AppError> {
    let mut props_map = existing
        .and_then(|(p, _)| p.as_object().cloned())
        .unwrap_or_default();
    let mut metrics_map: BTreeMap<String, f64> = existing
        .and_then(|(_, m)| serde_json::from_value(m.clone()).ok())
        .unwrap_or_default();

    let mut ts_obj = match props_map.remove("__metric_ts") {
        Some(serde_json::Value::Object(m)) => m,
        _ => serde_json::Map::new(),
    };

    for (ak, av) in &row.attributes {
        props_map.insert(ak.clone(), av.clone());
    }

    let mut metric_changed = false;
    let ts_rfc = row.metric_ts.to_rfc3339();
    for (mk, &mv) in &row.metrics {
        let prev_ts = ts_obj
            .get(mk)
            .and_then(serde_json::Value::as_str)
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&Utc));

        if prev_ts.is_none_or(|prev| row.metric_ts >= prev) {
            let changed = metrics_map
                .get(mk)
                .is_none_or(|old_v| (*old_v - mv).abs() > 1e-12);
            metrics_map.insert(mk.clone(), mv);
            ts_obj.insert(mk.clone(), serde_json::Value::String(ts_rfc.clone()));
            if changed {
                metric_changed = true;
            }
        }
    }

    if !ts_obj.is_empty() {
        props_map.insert(
            String::from("__metric_ts"),
            serde_json::Value::Object(ts_obj),
        );
    }

    let metrics_json = serde_json::to_value(&metrics_map)
        .map_err(|err| AppError::Database(format!("serialize mapped metrics: {err}")))?;
    Ok((
        serde_json::Value::Object(props_map),
        metrics_json,
        metric_changed,
    ))
}

async fn write_mapped_metric_series_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    run_id: Uuid,
    batch_rows: &[CoercedRow],
    key_to_id: &BTreeMap<String, Uuid>,
) -> Result<i64, AppError> {
    let mut nids = Vec::new();
    let mut mnames = Vec::new();
    let mut tss = Vec::new();
    let mut vals = Vec::new();
    let mut dirty_buckets: BTreeSet<(Uuid, String, DateTime<Utc>)> = BTreeSet::new();

    for row in batch_rows {
        let Some(&nid) = key_to_id.get(&row.node_key) else {
            continue;
        };
        let bucket_min = row
            .metric_ts
            .with_second(0)
            .and_then(|t| t.with_nanosecond(0))
            .unwrap_or(row.metric_ts);
        for (mk, &mv) in &row.metrics {
            nids.push(nid);
            mnames.push(mk.clone());
            tss.push(row.metric_ts);
            vals.push(mv);
            dirty_buckets.insert((nid, mk.clone(), bucket_min));
        }
    }

    if nids.is_empty() {
        return Ok(0);
    }

    sqlx::query(
        "INSERT INTO xb_metric_series (
             tenant_id, node_id, metric, ts, value, source_tag, batch_id
         )
         SELECT $1, u.node_id, u.metric, u.ts, u.value, 'SYNC', $6
           FROM UNNEST($2::uuid[], $3::text[], $4::timestamptz[], $5::float8[])
                AS u(node_id, metric, ts, value)
         ON CONFLICT (tenant_id, node_id, metric, ts, source_tag) DO UPDATE SET
             value = EXCLUDED.value,
             batch_id = EXCLUDED.batch_id",
    )
    .bind(tenant_id)
    .bind(&nids)
    .bind(&mnames)
    .bind(&tss)
    .bind(&vals)
    .bind(run_id)
    .execute(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("insert mapped xb_metric_series failed: {err}")))?;

    let mut d_nids = Vec::with_capacity(dirty_buckets.len());
    let mut d_metrics = Vec::with_capacity(dirty_buckets.len());
    let mut d_mins = Vec::with_capacity(dirty_buckets.len());
    for (nid, m, bmin) in dirty_buckets {
        d_nids.push(nid);
        d_metrics.push(m);
        d_mins.push(bmin);
    }

    sqlx::query(
        "INSERT INTO xb_rollup_dirty (tenant_id, node_id, metric, minute_start)
         SELECT $1, u.node_id, u.metric, u.minute_start
           FROM UNNEST($2::uuid[], $3::text[], $4::timestamptz[])
                AS u(node_id, metric, minute_start)
         ON CONFLICT DO NOTHING",
    )
    .bind(tenant_id)
    .bind(&d_nids)
    .bind(&d_metrics)
    .bind(&d_mins)
    .execute(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("insert mapped xb_rollup_dirty failed: {err}")))?;

    let minute_key = format!("{tenant_id}:{}", Utc::now().format("%Y-%m-%dT%H:%MZ"));
    let rollup_job =
        NewJob::new(JobKind::Rollup, serde_json::json!({})).with_idempotency_key(minute_key);
    let _ = enqueue_job_tx(tx, tenant_id, &rollup_job).await?;

    Ok(i64::try_from(nids.len()).unwrap_or(i64::MAX))
}

async fn upsert_mapped_links_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    dataset_id: Uuid,
    batch_rows: &[CoercedRow],
    key_to_id: &BTreeMap<String, Uuid>,
) -> Result<(i64, BTreeSet<Uuid>), AppError> {
    let mut needed_targets: BTreeMap<(String, String), bool> = BTreeMap::new();
    for row in batch_rows {
        for lk in &row.links {
            let entry = needed_targets
                .entry((lk.target_type.clone(), lk.target_key.clone()))
                .or_insert(false);
            if lk.create_missing_target {
                *entry = true;
            }
        }
    }
    if needed_targets.is_empty() {
        return Ok((0, BTreeSet::new()));
    }

    let target_ids =
        resolve_or_create_targets_tx(tx, tenant_id, dataset_id, &needed_targets).await?;

    let mut dedup_links: BTreeMap<(String, Uuid, Uuid), f64> = BTreeMap::new();
    for row in batch_rows {
        let Some(&self_id) = key_to_id.get(&row.node_key) else {
            continue;
        };
        for lk in &row.links {
            let Some(&other_id) = target_ids.get(&(lk.target_type.clone(), lk.target_key.clone()))
            else {
                continue;
            };
            let (src_id, dst_id) = match lk.direction {
                LinkDirection::Out => (self_id, other_id),
                LinkDirection::In => (other_id, self_id),
            };
            dedup_links.insert((lk.link_type.clone(), src_id, dst_id), lk.weight);
        }
    }

    if dedup_links.is_empty() {
        return Ok((0, BTreeSet::new()));
    }

    let mut l_types = Vec::with_capacity(dedup_links.len());
    let mut l_srcs = Vec::with_capacity(dedup_links.len());
    let mut l_dsts = Vec::with_capacity(dedup_links.len());
    let mut l_weights = Vec::with_capacity(dedup_links.len());
    let mut touched_endpoints = BTreeSet::new();

    for ((lt, src, dst), w) in dedup_links {
        touched_endpoints.insert(src);
        touched_endpoints.insert(dst);
        l_types.push(lt);
        l_srcs.push(src);
        l_dsts.push(dst);
        l_weights.push(w);
    }

    sqlx::query(
        "INSERT INTO xb_links (tenant_id, link_type, src_node_id, dst_node_id, weight, source_dataset_id)
         SELECT $1, u.link_type, u.src_id, u.dst_id, u.weight, $2
           FROM UNNEST($3::text[], $4::uuid[], $5::uuid[], $6::float8[])
                AS u(link_type, src_id, dst_id, weight)
         ON CONFLICT (tenant_id, link_type, src_node_id, dst_node_id) DO UPDATE SET
             weight = EXCLUDED.weight,
             source_dataset_id = EXCLUDED.source_dataset_id,
             updated_at = NOW()",
    )
    .bind(tenant_id)
    .bind(dataset_id)
    .bind(&l_types)
    .bind(&l_srcs)
    .bind(&l_dsts)
    .bind(&l_weights)
    .execute(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("batch upsert mapped xb_links failed: {err}")))?;

    Ok((
        i64::try_from(l_types.len()).unwrap_or(i64::MAX),
        touched_endpoints,
    ))
}

async fn resolve_or_create_targets_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    dataset_id: Uuid,
    needed_targets: &BTreeMap<(String, String), bool>,
) -> Result<BTreeMap<(String, String), Uuid>, AppError> {
    let mut q_types = Vec::with_capacity(needed_targets.len());
    let mut q_keys = Vec::with_capacity(needed_targets.len());
    for (t_type, t_key) in needed_targets.keys() {
        q_types.push(t_type.clone());
        q_keys.push(t_key.clone());
    }

    let found = sqlx::query(
        "SELECT n.id, n.type_name, n.node_key
           FROM xb_nodes n
           JOIN UNNEST($2::text[], $3::text[]) AS u(type_name, node_key)
             ON n.type_name = u.type_name AND n.node_key = u.node_key
          WHERE n.tenant_id = $1",
    )
    .bind(tenant_id)
    .bind(&q_types)
    .bind(&q_keys)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("resolve link target nodes failed: {err}")))?;

    let mut resolved = BTreeMap::new();
    for r in found {
        resolved.insert((r.get("type_name"), r.get("node_key")), r.get("id"));
    }

    let mut create_types = Vec::new();
    let mut create_keys = Vec::new();
    for ((t_type, t_key), &allow_create) in needed_targets {
        if allow_create && !resolved.contains_key(&(t_type.clone(), t_key.clone())) {
            create_types.push(t_type.clone());
            create_keys.push(t_key.clone());
        }
    }

    if !create_types.is_empty() {
        let created = sqlx::query(
            "INSERT INTO xb_nodes (tenant_id, type_name, node_key, title, source_dataset_id, source_row_key)
             SELECT $1, u.type_name, u.node_key, u.node_key, $2, u.node_key
               FROM UNNEST($3::text[], $4::text[]) AS u(type_name, node_key)
             ON CONFLICT (tenant_id, type_name, node_key) DO UPDATE SET
                 updated_at = NOW()
             RETURNING id, type_name, node_key",
        )
        .bind(tenant_id)
        .bind(dataset_id)
        .bind(&create_types)
        .bind(&create_keys)
        .fetch_all(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("create missing link targets failed: {err}")))?;

        for r in created {
            resolved.insert((r.get("type_name"), r.get("node_key")), r.get("id"));
        }
    }

    Ok(resolved)
}

async fn emit_chunk_events_and_jobs_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    type_name: &str,
    upserted_ids: &[Uuid],
    dirty_agg_nodes: &BTreeSet<Uuid>,
    changed_metric_names: &BTreeSet<String>,
) -> Result<(), AppError> {
    if !upserted_ids.is_empty() {
        let up_event = NewOutboxEvent {
            topic: OutboxTopic::NodeUpserted,
            payload: serde_json::json!({
                "node_ids": upserted_ids,
                "type_name": type_name,
            }),
        };
        let _ = insert_outbox_tx(tx, tenant_id, &up_event).await?;

        for batch in upserted_ids.chunks(500) {
            let job = NewJob::new(
                JobKind::EmbedNodes,
                serde_json::json!({ "node_ids": batch }),
            );
            let _ = enqueue_job_tx(tx, tenant_id, &job).await?;
        }
    }

    if !dirty_agg_nodes.is_empty() {
        let dirty_vec: Vec<Uuid> = dirty_agg_nodes.iter().copied().collect();
        let metrics_vec: Vec<String> = changed_metric_names.iter().cloned().collect();
        let m_event = NewOutboxEvent {
            topic: OutboxTopic::NodeMetricsChanged,
            payload: serde_json::json!({
                "node_ids": dirty_vec,
                "metrics": metrics_vec,
                "origin": "MAPPING",
            }),
        };
        let _ = insert_outbox_tx(tx, tenant_id, &m_event).await?;
    }

    Ok(())
}

async fn delete_nodes_by_keys_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    type_name: &str,
    del_keys: &[String],
    sync_to_knowledge: bool,
) -> Result<(i64, BTreeSet<Uuid>), AppError> {
    let id_rows = sqlx::query(
        "SELECT id FROM xb_nodes WHERE tenant_id = $1 AND type_name = $2 AND node_key = ANY($3)",
    )
    .bind(tenant_id)
    .bind(type_name)
    .bind(del_keys)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("lookup nodes to delete failed: {err}")))?;

    let target_ids: Vec<Uuid> = id_rows.iter().map(|r| r.get("id")).collect();
    if target_ids.is_empty() {
        return Ok((0, BTreeSet::new()));
    }
    delete_node_ids_and_collect_neighbors_tx(tx, tenant_id, &target_ids, sync_to_knowledge).await
}

async fn prune_unseen_full_nodes(
    store: &PgStore,
    tenant_id: &str,
    type_name: &str,
    dataset_id: Uuid,
    seen_keys: &BTreeSet<String>,
    sync_to_knowledge: bool,
) -> Result<i64, AppError> {
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let keep_vec: Vec<String> = seen_keys.iter().cloned().collect();
    let stale_rows = sqlx::query(
        "SELECT id FROM xb_nodes
          WHERE tenant_id = $1
            AND type_name = $2
            AND source_dataset_id = $3
            AND NOT (node_key = ANY($4))",
    )
    .bind(tenant_id)
    .bind(type_name)
    .bind(dataset_id)
    .bind(&keep_vec)
    .fetch_all(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("query stale full-mode nodes failed: {err}")))?;

    let stale_ids: Vec<Uuid> = stale_rows.iter().map(|r| r.get("id")).collect();
    if stale_ids.is_empty() {
        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit empty full prune tx failed: {err}"))
        })?;
        return Ok(0);
    }

    let (del_cnt, affected_neighbors) = delete_node_ids_and_collect_neighbors_tx(
        &mut tx,
        tenant_id,
        &stale_ids,
        sync_to_knowledge,
    )
    .await?;

    if !affected_neighbors.is_empty() {
        let dirty_vec: Vec<Uuid> = affected_neighbors.into_iter().collect();
        let m_event = NewOutboxEvent {
            topic: OutboxTopic::NodeMetricsChanged,
            payload: serde_json::json!({
                "node_ids": dirty_vec,
                "metrics": [],
                "origin": "MAPPING",
            }),
        };
        let _ = insert_outbox_tx(&mut tx, tenant_id, &m_event).await?;
    }

    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit full prune tx failed: {err}")))?;
    Ok(del_cnt)
}

async fn delete_node_ids_and_collect_neighbors_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    target_ids: &[Uuid],
    sync_to_knowledge: bool,
) -> Result<(i64, BTreeSet<Uuid>), AppError> {
    let link_rows = sqlx::query(
        "SELECT src_node_id, dst_node_id FROM xb_links
          WHERE tenant_id = $1 AND (src_node_id = ANY($2) OR dst_node_id = ANY($2))",
    )
    .bind(tenant_id)
    .bind(target_ids)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("query neighbor links before delete: {err}")))?;

    let deleting_set: BTreeSet<Uuid> = target_ids.iter().copied().collect();
    let mut neighbors = BTreeSet::new();
    for lr in link_rows {
        let s: Uuid = lr.get("src_node_id");
        let d: Uuid = lr.get("dst_node_id");
        if !deleting_set.contains(&s) {
            neighbors.insert(s);
        }
        if !deleting_set.contains(&d) {
            neighbors.insert(d);
        }
    }

    for sql in [
        "DELETE FROM xb_metric_series WHERE tenant_id = $1 AND node_id = ANY($2)",
        "DELETE FROM xb_metric_rollups WHERE tenant_id = $1 AND node_id = ANY($2)",
        "DELETE FROM xb_rollup_dirty WHERE tenant_id = $1 AND node_id = ANY($2)",
        "DELETE FROM hg_embeddings WHERE tenant_id = $1 AND node_type = 'XB_NODE' AND node_id = ANY($2)",
    ] {
        sqlx::query(sql)
            .bind(tenant_id)
            .bind(target_ids)
            .execute(&mut **tx)
            .await
            .map_err(|err| AppError::Database(format!("cleanup deleted mapped nodes: {err}")))?;
    }

    let res = sqlx::query("DELETE FROM xb_nodes WHERE tenant_id = $1 AND id = ANY($2)")
        .bind(tenant_id)
        .bind(target_ids)
        .execute(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("delete mapped xb_nodes failed: {err}")))?;

    if sync_to_knowledge {
        for &nid in target_ids {
            let payload = serde_json::to_value(KnowledgeSyncJobPayload {
                deleted_node_id: Some(nid),
                ..KnowledgeSyncJobPayload::default()
            })
            .unwrap_or_else(|_| serde_json::json!({ "deleted_node_id": nid }));
            let job = NewJob::new(JobKind::KnowledgeSync, payload)
                .with_idempotency_key(format!("ksync_del:{nid}"));
            let _ = enqueue_job_tx(tx, tenant_id, &job).await?;
        }
    }

    Ok((
        i64::try_from(res.rows_affected()).unwrap_or(i64::MAX),
        neighbors,
    ))
}

fn coerce_mapping_row(
    row: &JsonRow,
    spec: &MappingSpec,
    nt: &NodeTypeSpec,
    edge_weights: &BTreeMap<(String, String, String), f64>,
    default_ts: DateTime<Utc>,
) -> Result<CoercedRow, String> {
    let node_key = extract_non_empty_key(row, &spec.key_column)
        .ok_or_else(|| format!("missing or empty key_column '{}'", spec.key_column))?;

    let attributes = coerce_row_attributes(row, spec, nt)?;
    let metrics = coerce_row_metrics(row, spec, nt)?;
    let metric_ts = coerce_row_timestamp(row, spec.metric_time_column.as_deref(), default_ts)?;
    let geo = coerce_row_geo(row, spec)?;
    let links = coerce_row_links(row, spec, &nt.name, edge_weights)?;
    let title = render_mapping_title(&spec.title, &node_key, row, &attributes);

    Ok(CoercedRow {
        node_key,
        title,
        attributes,
        metrics,
        metric_ts,
        geo,
        links,
    })
}

fn coerce_row_attributes(
    row: &JsonRow,
    spec: &MappingSpec,
    nt: &NodeTypeSpec,
) -> Result<serde_json::Map<String, serde_json::Value>, String> {
    let prop_by_name: BTreeMap<&str, &PropertySpec> = nt
        .properties
        .iter()
        .filter(|p| p.kind == PropertyKind::Attribute)
        .map(|p| (p.name.as_str(), p))
        .collect();

    let mut out = serde_json::Map::new();
    for (attr_name, col_name) in &spec.attributes {
        let raw_val = row.get(col_name).unwrap_or(&serde_json::Value::Null);
        let prop_spec = prop_by_name.get(attr_name.as_str()).copied();
        if raw_val.is_null() {
            if prop_spec.is_some_and(|p| p.required) {
                return Err(format!("required attribute '{attr_name}' is null"));
            }
            continue;
        }
        let coerced = if let Some(ps) = prop_spec {
            coerce_json_by_data_type(raw_val, ps.data_type, ps.allowed_values.as_deref(), attr_name)?
        } else {
            raw_val.clone()
        };
        out.insert(attr_name.clone(), coerced);
    }

    for ps in prop_by_name.values() {
        if ps.required && !out.contains_key(&ps.name) {
            return Err(format!("required attribute '{}' is missing", ps.name));
        }
    }
    Ok(out)
}

fn coerce_json_by_data_type(
    val: &serde_json::Value,
    dt: DataType,
    allowed: Option<&[String]>,
    field_name: &str,
) -> Result<serde_json::Value, String> {
    let out = match dt {
        DataType::String | DataType::Date | DataType::Timestamp => {
            let s = scalar_to_string(val)
                .ok_or_else(|| format!("attribute '{field_name}' cannot convert to {dt}"))?;
            serde_json::Value::String(s)
        }
        DataType::Int64 => {
            let n = parse_i64_cell(val)
                .ok_or_else(|| format!("attribute '{field_name}' is not a valid INT64"))?;
            serde_json::Value::Number(serde_json::Number::from(n))
        }
        DataType::Float64 | DataType::Decimal => {
            let f = parse_f64_cell(val)
                .ok_or_else(|| format!("attribute '{field_name}' is not a finite number"))?;
            let num = serde_json::Number::from_f64(f)
                .ok_or_else(|| format!("attribute '{field_name}' is non-finite"))?;
            serde_json::Value::Number(num)
        }
        DataType::Boolean => {
            let b = parse_bool_cell(val)
                .ok_or_else(|| format!("attribute '{field_name}' is not a valid BOOLEAN"))?;
            serde_json::Value::Bool(b)
        }
        DataType::Binary | DataType::Json | DataType::List | DataType::Struct => val.clone(),
    };

    if let Some(choices) = allowed
        && !choices.is_empty()
        && let serde_json::Value::String(s) = &out
        && !choices.iter().any(|c| c == s)
    {
        return Err(format!(
            "attribute '{field_name}' value '{s}' is not in allowed_values"
        ));
    }

    Ok(out)
}

fn coerce_row_metrics(
    row: &JsonRow,
    spec: &MappingSpec,
    nt: &NodeTypeSpec,
) -> Result<BTreeMap<String, f64>, String> {
    let metric_specs: BTreeMap<&str, &PropertySpec> = nt
        .properties
        .iter()
        .filter(|p| p.kind == PropertyKind::Metric)
        .map(|p| (p.name.as_str(), p))
        .collect();

    let mut out = BTreeMap::new();
    for (m_name, col_name) in &spec.metrics {
        let raw_val = row.get(col_name).unwrap_or(&serde_json::Value::Null);
        if raw_val.is_null() {
            if metric_specs.get(m_name.as_str()).is_some_and(|p| p.required) {
                return Err(format!("required metric '{m_name}' is null"));
            }
            continue;
        }
        let v = parse_f64_cell(raw_val)
            .ok_or_else(|| format!("metric '{m_name}' column '{col_name}' is not a finite number"))?;
        out.insert(m_name.clone(), v);
    }
    Ok(out)
}

fn coerce_row_timestamp(
    row: &JsonRow,
    ts_col: Option<&str>,
    default_ts: DateTime<Utc>,
) -> Result<DateTime<Utc>, String> {
    let Some(col) = ts_col.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(default_ts);
    };
    let Some(val) = row.get(col) else {
        return Ok(default_ts);
    };
    if val.is_null() {
        return Ok(default_ts);
    }
    parse_timestamp_cell(val)
        .ok_or_else(|| format!("metric_time_column '{col}' is not a valid timestamp"))
}

fn coerce_row_geo(
    row: &JsonRow,
    spec: &MappingSpec,
) -> Result<Option<GeoPoint>, String> {
    let lng_col = spec.geo.lng_column.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let lat_col = spec.geo.lat_column.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let (Some(lng_c), Some(lat_c)) = (lng_col, lat_col) else {
        return Ok(None);
    };

    let lng_v = row.get(lng_c).unwrap_or(&serde_json::Value::Null);
    let lat_v = row.get(lat_c).unwrap_or(&serde_json::Value::Null);
    if lng_v.is_null() && lat_v.is_null() {
        return Ok(None);
    }
    let lng = parse_f64_cell(lng_v).ok_or_else(|| format!("invalid geo lng in '{lng_c}'"))?;
    let lat = parse_f64_cell(lat_v).ok_or_else(|| format!("invalid geo lat in '{lat_c}'"))?;
    let pt = GeoPoint { lng, lat };
    pt.validate().map_err(|e| e.client_message())?;
    Ok(Some(pt))
}

fn coerce_row_links(
    row: &JsonRow,
    spec: &MappingSpec,
    self_type: &str,
    edge_weights: &BTreeMap<(String, String, String), f64>,
) -> Result<Vec<CoercedLink>, String> {
    let mut out = Vec::new();
    for lk in &spec.links {
        let Some(target_key) = extract_non_empty_key(row, &lk.target_key_column) else {
            continue;
        };
        let edge_key = match lk.direction {
            LinkDirection::Out => (
                lk.link_type.clone(),
                String::from(self_type),
                lk.target_type.clone(),
            ),
            LinkDirection::In => (
                lk.link_type.clone(),
                lk.target_type.clone(),
                String::from(self_type),
            ),
        };
        let default_w = edge_weights.get(&edge_key).copied().unwrap_or(1.0);
        let weight = if let Some(wc) = lk.weight_column.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            let rv = row.get(wc).unwrap_or(&serde_json::Value::Null);
            if rv.is_null() {
                default_w
            } else {
                let parsed_w = parse_f64_cell(rv)
                    .ok_or_else(|| format!("invalid weight_column '{wc}'"))?;
                if parsed_w <= 0.0 {
                    return Err(format!("weight_column '{wc}' must be > 0"));
                }
                parsed_w
            }
        } else {
            default_w
        };
        out.push(CoercedLink {
            link_type: lk.link_type.clone(),
            direction: lk.direction,
            target_type: lk.target_type.clone(),
            target_key,
            weight,
            create_missing_target: lk.create_missing_target,
        });
    }
    Ok(out)
}

fn render_mapping_title(
    template: &str,
    node_key: &str,
    row: &JsonRow,
    attrs: &serde_json::Map<String, serde_json::Value>,
) -> String {
    let trimmed = template.trim();
    if trimmed.is_empty() {
        return String::from(node_key);
    }
    let mut rendered = String::with_capacity(trimmed.len().saturating_add(16));
    let mut chars = trimmed.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '{' {
            let mut placeholder = String::new();
            let mut closed = false;
            for inner in chars.by_ref() {
                if inner == '}' {
                    closed = true;
                    break;
                }
                placeholder.push(inner);
            }
            if closed {
                let token = placeholder.trim();
                let replacement = if token == "node_key" {
                    Some(String::from(node_key))
                } else if let Some(prop_name) = token.strip_prefix("prop.") {
                    attrs.get(prop_name).and_then(scalar_to_string)
                } else {
                    row.get(token).and_then(scalar_to_string)
                };
                if let Some(rep) = replacement {
                    rendered.push_str(&rep);
                }
            } else {
                rendered.push('{');
                rendered.push_str(&placeholder);
            }
        } else {
            rendered.push(ch);
        }
    }
    let final_title = rendered.trim();
    if final_title.is_empty() {
        String::from(node_key)
    } else {
        String::from(final_title)
    }
}

fn extract_non_empty_key(
    row: &JsonRow,
    col: &str,
) -> Option<String> {
    row.get(col)
        .and_then(scalar_to_string)
        .map(|s| String::from(s.trim()))
        .filter(|s| !s.is_empty())
}

fn scalar_to_string(val: &serde_json::Value) -> Option<String> {
    match val {
        serde_json::Value::Null => None,
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::Bool(b) => Some(b.to_string()),
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => Some(val.to_string()),
    }
}

fn parse_f64_cell(val: &serde_json::Value) -> Option<f64> {
    let raw = match val {
        serde_json::Value::Number(n) => n.as_f64()?,
        serde_json::Value::String(s) => s.trim().parse::<f64>().ok()?,
        _ => return None,
    };
    if raw.is_finite() {
        Some(if matches!(raw.classify(), FpCategory::Zero) {
            0.0
        } else {
            raw
        })
    } else {
        None
    }
}

fn parse_i64_cell(val: &serde_json::Value) -> Option<i64> {
    match val {
        serde_json::Value::Number(n) => n.as_i64(),
        serde_json::Value::String(s) => s.trim().parse::<i64>().ok(),
        _ => None,
    }
}

fn parse_bool_cell(val: &serde_json::Value) -> Option<bool> {
    match val {
        serde_json::Value::Bool(b) => Some(*b),
        serde_json::Value::String(s) => match s.trim().to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" => Some(true),
            "false" | "0" | "no" => Some(false),
            _ => None,
        },
        serde_json::Value::Number(n) => n.as_i64().map(|i| i != 0),
        _ => None,
    }
}

fn parse_timestamp_cell(val: &serde_json::Value) -> Option<DateTime<Utc>> {
    match val {
        serde_json::Value::String(s) => {
            let t = s.trim();
            if let Ok(dt) = DateTime::parse_from_rfc3339(t) {
                return Some(dt.with_timezone(&Utc));
            }
            for fmt in [
                "%Y-%m-%d %H:%M:%S%.f%:z",
                "%Y-%m-%d %H:%M:%S%:z",
                "%Y-%m-%d %H:%M:%S%.f%#z",
                "%Y-%m-%d %H:%M:%S%#z",
                "%Y-%m-%dT%H:%M:%S%.f",
                "%Y-%m-%d %H:%M:%S%.f",
                "%Y-%m-%d %H:%M:%S",
            ] {
                if let Ok(dt) = DateTime::parse_from_str(t, fmt) {
                    return Some(dt.with_timezone(&Utc));
                }
                if let Ok(ndt) = NaiveDateTime::parse_from_str(t, fmt) {
                    return Some(ndt.and_utc());
                }
            }
            if let Ok(nd) = NaiveDate::parse_from_str(t, "%Y-%m-%d") {
                return nd.and_hms_opt(0, 0, 0).map(|ndt| ndt.and_utc());
            }
            None
        }
        serde_json::Value::Number(n) => {
            let epoch = n.as_i64()?;
            if epoch > 1_000_000_000_000 {
                DateTime::from_timestamp_millis(epoch)
            } else {
                DateTime::from_timestamp(epoch, 0)
            }
        }
        _ => None,
    }
}

fn normalize_mapping_filter_ast(expr: Expr) -> Expr {
    match expr {
        Expr::MetricRef(col) | Expr::PropRef(col) => Expr::PropRef(col),
        Expr::Literal(v) => Expr::Literal(v),
        Expr::Unary { op, expr: inner } => Expr::Unary {
            op,
            expr: Box::new(normalize_mapping_filter_ast(*inner)),
        },
        Expr::Binary { op, left, right } => Expr::Binary {
            op,
            left: Box::new(normalize_mapping_filter_ast(*left)),
            right: Box::new(normalize_mapping_filter_ast(*right)),
        },
        Expr::Call { name, args } => Expr::Call {
            name,
            args: args.into_iter().map(normalize_mapping_filter_ast).collect(),
        },
    }
}

fn eval_row_filter(ast: &Expr, row: &JsonRow) -> bool {
    let mut props = BTreeMap::new();
    for (k, v) in row {
        let ev = match v {
            serde_json::Value::Bool(b) => ExprValue::Bool(*b),
            serde_json::Value::Number(n) => n
                .as_f64()
                .filter(|f| f.is_finite())
                .map_or(ExprValue::Null, ExprValue::Number),
            serde_json::Value::String(s) => ExprValue::Text(s.clone()),
            serde_json::Value::Null
            | serde_json::Value::Array(_)
            | serde_json::Value::Object(_) => ExprValue::Null,
        };
        props.insert(k.clone(), ev);
    }
    let env = EvalEnv {
        metrics: BTreeMap::new(),
        properties: props,
    };
    matches!(eval_expr(ast, &env).value, ExprValue::Bool(true))
}

fn try_translate_duckdb_filter(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Literal(ExprValue::Null) => Some(String::from("NULL")),
        Expr::Literal(ExprValue::Bool(b)) => Some(if *b {
            String::from("TRUE")
        } else {
            String::from("FALSE")
        }),
        Expr::Literal(ExprValue::Number(v)) => {
            if v.is_finite() {
                Some(format!("{v}"))
            } else {
                None
            }
        }
        Expr::Literal(ExprValue::Text(s)) => Some(format!("'{}'", escape_sql_literal(s))),
        Expr::PropRef(col) | Expr::MetricRef(col) => {
            if is_safe_sql_ident(col) {
                Some(quote_ident(col))
            } else {
                None
            }
        }
        Expr::Unary { op: UnaryOp::Not, expr: inner } => {
            Some(format!("(NOT ({}))", try_translate_duckdb_filter(inner)?))
        }
        Expr::Unary { op: UnaryOp::Neg, expr: inner } => {
            Some(format!("(-({}))", try_translate_duckdb_filter(inner)?))
        }
        Expr::Binary { op, left, right } => {
            let l = try_translate_duckdb_filter(left)?;
            let r = try_translate_duckdb_filter(right)?;
            let sql_op = match op {
                BinaryOp::Or => "OR",
                BinaryOp::And => "AND",
                BinaryOp::Eq => "=",
                BinaryOp::Ne => "<>",
                BinaryOp::Lt => "<",
                BinaryOp::Le => "<=",
                BinaryOp::Gt => ">",
                BinaryOp::Ge => ">=",
                BinaryOp::Add => "+",
                BinaryOp::Sub => "-",
                BinaryOp::Mul => "*",
                BinaryOp::Div | BinaryOp::Mod | BinaryOp::Pow => return None,
            };
            Some(format!("({l} {sql_op} {r})"))
        }
        Expr::Call { name, args } if name == "is_null" && args.len() == 1 => {
            let inner = try_translate_duckdb_filter(args.first()?)?;
            Some(format!("({inner} IS NULL)"))
        }
        Expr::Call { .. } => None,
    }
}

fn check_missing_columns(
    spec: &MappingSpec,
    available: &BTreeSet<String>,
    dataset_name: &str,
) -> Option<String> {
    if available.is_empty() {
        return None;
    }
    let avail_lower: BTreeSet<String> = available.iter().map(|c| c.to_ascii_lowercase()).collect();
    let mut referenced: BTreeSet<String> = BTreeSet::new();

    if !spec.key_column.trim().is_empty() {
        referenced.insert(spec.key_column.trim().to_owned());
    }
    for col in spec.attributes.values() {
        if !col.trim().is_empty() {
            referenced.insert(col.trim().to_owned());
        }
    }
    for col in spec.metrics.values() {
        if !col.trim().is_empty() {
            referenced.insert(col.trim().to_owned());
        }
    }
    if let Some(ts_col) = spec.metric_time_column.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        referenced.insert(String::from(ts_col));
    }
    if let Some(lng) = spec.geo.lng_column.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        referenced.insert(String::from(lng));
    }
    if let Some(lat) = spec.geo.lat_column.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        referenced.insert(String::from(lat));
    }
    for lk in &spec.links {
        if !lk.target_key_column.trim().is_empty() {
            referenced.insert(lk.target_key_column.trim().to_owned());
        }
        if let Some(wc) = lk.weight_column.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            referenced.insert(String::from(wc));
        }
    }
    if let Some(filter_raw) = spec.filter.as_deref().map(str::trim).filter(|s| !s.is_empty())
        && let Ok(ast) = parse_expr(filter_raw)
    {
        let (m_refs, p_refs) = collect_references(&ast);
        referenced.extend(m_refs);
        referenced.extend(p_refs);
    }

    let missing: Vec<String> = referenced
        .into_iter()
        .filter(|col| !avail_lower.contains(&col.to_ascii_lowercase()))
        .collect();

    if missing.is_empty() {
        None
    } else {
        Some(format!(
            "dataset '{dataset_name}' schema changed; missing referenced columns: {}",
            missing.join(", ")
        ))
    }
}

async fn resolve_lake_table<L: LakeInternalQueryPort>(
    lake: &L,
    tenant_id: &str,
    ds: &DatasetExecMeta,
) -> Result<ResolvedLakeTable, AppError> {
    let sanitized_tenant: String = tenant_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' })
        .collect();
    let tbl = if ds.lake_table.trim().is_empty() {
        ds.name.as_str()
    } else {
        ds.lake_table.as_str()
    };

    let mut candidates: Vec<LakeRefCandidate> = vec![
        (
            format!("lake.{}.{}", quote_ident(&format!("tenant_{sanitized_tenant}")), quote_ident(tbl)),
            Some((String::from("lake"), format!("tenant_{sanitized_tenant}"), String::from(tbl))),
        ),
        (
            format!("lake.{}.{}", quote_ident(&format!("tenant_{tenant_id}")), quote_ident(tbl)),
            Some((String::from("lake"), format!("tenant_{tenant_id}"), String::from(tbl))),
        ),
    ];
    if !ds.lake_schema.trim().is_empty() {
        candidates.push((
            format!("lake.{}.{}", quote_ident(&ds.lake_schema), quote_ident(tbl)),
            Some((String::from("lake"), ds.lake_schema.clone(), String::from(tbl))),
        ));
    }
    candidates.push((quote_ident(tbl), None));
    candidates.push((quote_ident(&ds.name), None));

    let mut last_err: Option<AppError> = None;
    for (full_ref, dl_args) in candidates {
        let probe_sql = format!("SELECT * FROM {full_ref} LIMIT 0");
        match lake.query_internal(tenant_id, &probe_sql, 0).await {
            Ok(qr) => {
                let physical_columns = qr.columns.into_iter().map(|c| c.name).collect();
                return Ok(ResolvedLakeTable {
                    full_ref,
                    ducklake_args: dl_args,
                    physical_columns,
                });
            }
            Err(err) => last_err = Some(err),
        }
    }

    Err(last_err.unwrap_or_else(|| {
        AppError::Database(format!("unable to resolve lake table for dataset '{}'", ds.name))
    }))
}

async fn select_execution_mode<L: LakeInternalQueryPort>(
    lake: &L,
    tenant_id: &str,
    requested: MappingMode,
    mapping: &MappingRecord,
    ds: &DatasetExecMeta,
    resolved: &ResolvedLakeTable,
) -> (MappingMode, Option<i64>, Option<i64>) {
    let to_snap = ds.last_snapshot_id;
    if requested != MappingMode::Incremental {
        return (MappingMode::Full, None, to_snap);
    }

    let (Some(from_s), Some(to_s)) = (mapping.last_snapshot_id, to_snap) else {
        return (MappingMode::Full, None, to_snap);
    };

    if from_s >= to_s {
        return (MappingMode::Incremental, Some(from_s), Some(to_s));
    }

    let Some((cat, sch, tbl)) = &resolved.ducklake_args else {
        return (MappingMode::Full, None, to_snap);
    };

    let start_s = from_s.saturating_add(1);
    let probe = format!(
        "SELECT * FROM ducklake_table_changes('{}', '{}', '{}', {start_s}, {to_s}) LIMIT 0",
        escape_sql_literal(cat),
        escape_sql_literal(sch),
        escape_sql_literal(tbl),
    );
    if lake.query_internal(tenant_id, &probe, 0).await.is_ok() {
        (MappingMode::Incremental, Some(from_s), Some(to_s))
    } else {
        (MappingMode::Full, None, to_snap)
    }
}

fn build_chunk_sql(
    eff_mode: MappingMode,
    resolved: &ResolvedLakeTable,
    from_snap: Option<i64>,
    to_snap: Option<i64>,
    sql_where: Option<&str>,
    offset: usize,
) -> Result<String, AppError> {
    match eff_mode {
        MappingMode::Full => {
            let where_part = sql_where.map_or_else(String::new, |w| format!(" WHERE {w}"));
            Ok(format!(
                "SELECT * FROM {}{where_part} LIMIT {CHUNK_ROWS} OFFSET {offset}",
                resolved.full_ref
            ))
        }
        MappingMode::Incremental => {
            let (Some((cat, sch, tbl)), Some(f), Some(t)) =
                (&resolved.ducklake_args, from_snap, to_snap)
            else {
                return Err(AppError::bad(
                    "INVALID_ARGUMENT",
                    "missing ducklake snapshot parameters for incremental query",
                ));
            };
            let start_snap = f.saturating_add(1);
            Ok(format!(
                "SELECT * FROM ducklake_table_changes('{}', '{}', '{}', {start_snap}, {t}) \
                 WHERE change_type IN ('insert', 'update_postimage', 'delete') \
                 ORDER BY snapshot_id ASC \
                 LIMIT {CHUNK_ROWS} OFFSET {offset}",
                escape_sql_literal(cat),
                escape_sql_literal(sch),
                escape_sql_literal(tbl),
            ))
        }
    }
}

async fn load_mapping_and_model(
    store: &PgStore,
    tenant_id: &str,
    mapping_id: Uuid,
) -> Result<(MappingRecord, DatasetExecMeta, MainModelLookup), AppError> {
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let m_row = sqlx::query(
        "SELECT m.id, m.subgraph_slug, m.name, m.dataset_id, d.name AS dataset_name,
                m.node_type, m.spec, m.enabled, m.schema_version, m.last_snapshot_id,
                m.created_at, m.updated_at,
                d.lake_schema, d.lake_table, d.last_snapshot_id AS ds_last_snapshot_id,
                d.schema_json
           FROM xb_mappings m
           JOIN ds_datasets d ON d.id = m.dataset_id AND d.tenant_id = m.tenant_id
          WHERE m.tenant_id = $1 AND m.id = $2",
    )
    .bind(tenant_id)
    .bind(mapping_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("load mapping for execution failed: {err}")))?
    .ok_or(AppError::NotFound("MAPPING_NOT_FOUND"))?;

    let mapping = decode_mapping_row(&m_row)?;
    let cols_json: serde_json::Value = m_row.get("schema_json");
    let parsed_cols: Vec<ColumnProfile> = serde_json::from_value(cols_json).unwrap_or_default();
    let ds_meta = DatasetExecMeta {
        id: mapping.dataset_id,
        name: m_row.get("dataset_name"),
        lake_schema: m_row.get("lake_schema"),
        lake_table: m_row.get("lake_table"),
        last_snapshot_id: m_row.get("ds_last_snapshot_id"),
        schema_columns: parsed_cols.into_iter().map(|c| c.name).collect(),
    };

    let nt_rows = sqlx::query(
        "SELECT id, name, display_name, description, icon, color, title_template,
                properties, agg_rules, derived_rules, sync_to_knowledge, pos_x, pos_y
           FROM xb_node_types
          WHERE tenant_id = $1 AND subgraph_slug = 'main'",
    )
    .bind(tenant_id)
    .fetch_all(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("load main xb_node_types failed: {err}")))?;

    let mut node_types = BTreeMap::new();
    for r in nt_rows {
        let nt = NodeTypeSpec {
            id: Some(r.get("id")),
            name: r.get("name"),
            display_name: r.get("display_name"),
            description: r.get("description"),
            icon: r.get("icon"),
            color: r.get("color"),
            title_template: r.get("title_template"),
            properties: serde_json::from_value(r.get("properties")).unwrap_or_default(),
            agg_rules: serde_json::from_value(r.get("agg_rules")).unwrap_or_default(),
            derived_rules: serde_json::from_value(r.get("derived_rules")).unwrap_or_default(),
            sync_to_knowledge: r.get("sync_to_knowledge"),
            pos_x: r.get("pos_x"),
            pos_y: r.get("pos_y"),
        };
        node_types.insert(nt.name.clone(), nt);
    }

    let et_rows = sqlx::query(
        "SELECT name, src_type_name, dst_type_name, impact_weight
           FROM xb_edge_types
          WHERE tenant_id = $1 AND subgraph_slug = 'main'",
    )
    .bind(tenant_id)
    .fetch_all(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("load main xb_edge_types failed: {err}")))?;

    let mut edge_weights = BTreeMap::new();
    for r in et_rows {
        let et_key = (
            r.get::<String, _>("name"),
            r.get::<String, _>("src_type_name"),
            r.get::<String, _>("dst_type_name"),
        );
        let w: f64 = r.get("impact_weight");
        let _ = EdgeTypeSpec::default_impact_weight_or(w);
        edge_weights.insert(et_key, w);
    }

    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit load mapping model failed: {err}")))?;
    Ok((
        mapping,
        ds_meta,
        MainModelLookup {
            node_types,
            edge_weights,
        },
    ))
}

trait EdgeWeightExt {
    fn default_impact_weight_or(w: f64) -> f64;
}

impl EdgeWeightExt for EdgeTypeSpec {
    fn default_impact_weight_or(w: f64) -> f64 {
        if w.is_finite() && w > 0.0 { w } else { 1.0 }
    }
}

async fn persist_schema_broken_failure(
    store: &PgStore,
    tenant_id: &str,
    mapping_id: Uuid,
    job_id: Uuid,
    mode: MappingMode,
    error_message: &str,
) -> Result<(), AppError> {
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    sqlx::query(
        "UPDATE xb_mappings
            SET enabled = FALSE, updated_at = NOW()
          WHERE tenant_id = $1 AND id = $2",
    )
    .bind(tenant_id)
    .bind(mapping_id)
    .execute(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("disable broken xb_mapping failed: {err}")))?;

    sqlx::query(
        "INSERT INTO xb_mapping_runs (
             tenant_id, mapping_id, job_id, mode, status, error_message, finished_at
         )
         VALUES ($1, $2, $3, $4, 'FAILED', $5, NOW())",
    )
    .bind(tenant_id)
    .bind(mapping_id)
    .bind(job_id)
    .bind(mode.as_str())
    .bind(error_message)
    .execute(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("insert failed xb_mapping_runs failed: {err}")))?;

    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit schema broken failure failed: {err}")))?;
    Ok(())
}

async fn create_running_record(
    store: &PgStore,
    tenant_id: &str,
    mapping_id: Uuid,
    job_id: Uuid,
    mode: MappingMode,
    from_snap: Option<i64>,
    to_snap: Option<i64>,
) -> Result<Uuid, AppError> {
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let row = sqlx::query(
        "INSERT INTO xb_mapping_runs (
             tenant_id, mapping_id, job_id, mode, from_snapshot, to_snapshot, status
         )
         VALUES ($1, $2, $3, $4, $5, $6, 'RUNNING')
         RETURNING id",
    )
    .bind(tenant_id)
    .bind(mapping_id)
    .bind(job_id)
    .bind(mode.as_str())
    .bind(from_snap)
    .bind(to_snap)
    .fetch_one(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("create running xb_mapping_runs failed: {err}")))?;
    let run_id: Uuid = row.get("id");
    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit create running record failed: {err}")))?;
    Ok(run_id)
}

async fn finalize_succeeded_run(
    store: &PgStore,
    tenant_id: &str,
    mapping_id: Uuid,
    run_id: Uuid,
    to_snap: Option<i64>,
    counters: &RunCounters,
) -> Result<MappingRunRecord, AppError> {
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    sqlx::query(
        "UPDATE xb_mappings
            SET last_snapshot_id = COALESCE($3, last_snapshot_id),
                updated_at = NOW()
          WHERE tenant_id = $1 AND id = $2",
    )
    .bind(tenant_id)
    .bind(mapping_id)
    .bind(to_snap)
    .execute(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("advance mapping last_snapshot_id failed: {err}")))?;

    let samples_json = serde_json::to_value(&counters.reject_samples)
        .map_err(|err| AppError::Database(format!("serialize reject_samples failed: {err}")))?;

    let row = sqlx::query(
        "UPDATE xb_mapping_runs
            SET nodes_upserted = $3,
                nodes_deleted = $4,
                links_upserted = $5,
                points_written = $6,
                rows_rejected = $7,
                reject_samples = $8,
                status = 'SUCCEEDED',
                finished_at = NOW()
          WHERE tenant_id = $1 AND id = $2
          RETURNING id, mapping_id, job_id, mode, from_snapshot, to_snapshot,
                    nodes_upserted, nodes_deleted, links_upserted, points_written,
                    rows_rejected, reject_samples, status, error_message, started_at, finished_at",
    )
    .bind(tenant_id)
    .bind(run_id)
    .bind(counters.nodes_upserted)
    .bind(counters.nodes_deleted)
    .bind(counters.links_upserted)
    .bind(counters.points_written)
    .bind(counters.rows_rejected)
    .bind(samples_json)
    .fetch_one(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("finalize succeeded xb_mapping_runs failed: {err}")))?;

    let rec = decode_mapping_run_row(&row)?;
    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit succeeded mapping run failed: {err}")))?;
    Ok(rec)
}

async fn finalize_failed_run(
    store: &PgStore,
    tenant_id: &str,
    run_id: Uuid,
    error_message: &str,
    counters: &RunCounters,
) -> Result<(), AppError> {
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let samples_json = serde_json::to_value(&counters.reject_samples)
        .unwrap_or_else(|_| serde_json::json!([]));
    sqlx::query(
        "UPDATE xb_mapping_runs
            SET nodes_upserted = $3,
                nodes_deleted = $4,
                links_upserted = $5,
                points_written = $6,
                rows_rejected = $7,
                reject_samples = $8,
                status = 'FAILED',
                error_message = $9,
                finished_at = NOW()
          WHERE tenant_id = $1 AND id = $2",
    )
    .bind(tenant_id)
    .bind(run_id)
    .bind(counters.nodes_upserted)
    .bind(counters.nodes_deleted)
    .bind(counters.links_upserted)
    .bind(counters.points_written)
    .bind(counters.rows_rejected)
    .bind(samples_json)
    .bind(error_message)
    .execute(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("finalize failed xb_mapping_runs failed: {err}")))?;
    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit failed mapping run failed: {err}")))?;
    Ok(())
}

fn decode_mapping_row(row: &sqlx::postgres::PgRow) -> Result<MappingRecord, AppError> {
    let spec_json: serde_json::Value = row.get("spec");
    let spec: MappingSpec = serde_json::from_value(spec_json)
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
    let mode_raw: String = row.get("mode");
    let status_raw: String = row.get("status");
    let samples_json: serde_json::Value = row.get("reject_samples");
    let reject_samples: Vec<MappingRejectSample> =
        serde_json::from_value(samples_json).unwrap_or_default();
    Ok(MappingRunRecord {
        id: row.get("id"),
        mapping_id: row.get("mapping_id"),
        job_id: row.get("job_id"),
        mode: MappingMode::from_str(&mode_raw)?,
        from_snapshot: row.get("from_snapshot"),
        to_snapshot: row.get("to_snapshot"),
        nodes_upserted: row.get("nodes_upserted"),
        nodes_deleted: row.get("nodes_deleted"),
        links_upserted: row.get("links_upserted"),
        points_written: row.get("points_written"),
        rows_rejected: row.get("rows_rejected"),
        reject_samples,
        status: MappingRunStatus::from_str(&status_raw)?,
        error_message: row.get("error_message"),
        started_at: row.get("started_at"),
        finished_at: row.get("finished_at"),
    })
}

fn is_safe_sql_ident(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn quote_ident(ident: &str) -> String {
    format!("\"{}\"", ident.replace('"', "\"\""))
}

fn escape_sql_literal(s: &str) -> String {
    s.replace('\'', "''")
}

#[cfg(test)]
mod tests {
    use super::*;
    use axpoint_domain::enums::PiiCategory;
    use axpoint_domain::studio::MappingGeoSpec;

    #[test]
    fn mapping_exec_timestamp_and_filter_coercion_handles_pg_and_rfc3339() {
        let ts1 = parse_timestamp_cell(&serde_json::json!("2026-11-15T05:40:00.125Z"));
        let ts2 = parse_timestamp_cell(&serde_json::json!("2026-11-15 05:40:00.125+00"));
        assert!(ts1.is_some());
        assert_eq!(ts1, ts2);

        let raw_ast = parse_expr("status != 'TEST' and amount > 10");
        assert!(raw_ast.is_ok());
        if let Ok(ast) = raw_ast {
            let norm = normalize_mapping_filter_ast(ast);
            let mut ok_row = JsonRow::new();
            ok_row.insert(String::from("status"), serde_json::json!("PAID"));
            ok_row.insert(String::from("amount"), serde_json::json!(25.0));
            assert!(eval_row_filter(&norm, &ok_row));

            let mut bad_row = JsonRow::new();
            bad_row.insert(String::from("status"), serde_json::json!("TEST"));
            bad_row.insert(String::from("amount"), serde_json::json!(25.0));
            assert!(!eval_row_filter(&norm, &bad_row));
        }
    }

    #[test]
    fn mapping_exec_missing_columns_detects_schema_drift() {
        let spec = MappingSpec {
            name: String::from("m1"),
            dataset: String::from("orders"),
            node_type: String::from("Order"),
            key_column: String::from("order_id"),
            title: String::from("{order_id}"),
            filter: Some(String::from("status != 'TEST'")),
            attributes: BTreeMap::new(),
            metrics: BTreeMap::from([(String::from("demand"), String::from("amount"))]),
            metric_time_column: None,
            geo: MappingGeoSpec::default(),
            links: Vec::new(),
            on_source_delete: OnSourceDelete::Keep,
            mode: MappingMode::Full,
        };
        let cols_ok = BTreeSet::from([
            String::from("order_id"),
            String::from("status"),
            String::from("amount"),
        ]);
        assert!(check_missing_columns(&spec, &cols_ok, "orders").is_none());

        let cols_missing = BTreeSet::from([String::from("order_id"), String::from("status")]);
        let drift = check_missing_columns(&spec, &cols_missing, "orders");
        assert!(drift.is_some_and(|m| m.contains("amount")));

        let prop = PropertySpec {
            name: String::from("tier"),
            display_name: String::from("tier"),
            description: String::new(),
            unit: None,
            data_type: DataType::Int64,
            kind: PropertyKind::Attribute,
            required: true,
            pii_category: PiiCategory::None,
            allowed_values: None,
        };
        assert_eq!(prop.data_type, DataType::Int64);
    }
}
