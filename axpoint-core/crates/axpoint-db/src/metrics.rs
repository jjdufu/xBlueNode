//! Time-series metric ingestion (`xb_metric_series`), node snapshot synchronization,
//! automatic multi-grain queries, and tumbling/sliding window aggregations (`MetricStorePort`).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use axpoint_domain::AppError;
use axpoint_domain::enums::{AggFn, JobKind, OutboxTopic, RollupGrain};
use axpoint_domain::graph::{
    MetricCatalogEntry, MetricPointInput, MetricQueryGrain, MetricSeriesPoint,
    NodeMetricSeriesQuery, NodeRef, RollupRow, WindowBucketItem, WindowGroupBy, WindowKind,
    WindowQuery, WindowQueryResponse, WindowSpec, WriteMetricsRequest, WriteMetricsResponse,
    estimate_hll,
};
use axpoint_domain::platform::{NewJob, NewOutboxEvent};
use axpoint_domain::ports::MetricStorePort;
use chrono::{DateTime, Duration, Timelike, Utc};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

use crate::PgStore;
use crate::jobs::enqueue_job_tx;
use crate::outbox::insert_outbox_tx;
use crate::rollup::{decode_rollup_row, merge_rollup_slice};
use crate::tenant::begin_tenant_tx;

fn is_valid_metric_name(metric: &str) -> bool {
    let bytes = metric.as_bytes();
    let Some(&first) = bytes.first() else {
        return false;
    };
    if bytes.len() > 63 || !first.is_ascii_lowercase() {
        return false;
    }
    bytes
        .iter()
        .all(|&b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

fn i64_to_f64(val: i64) -> f64 {
    i32::try_from(val).map_or_else(|_| val.to_string().parse::<f64>().unwrap_or(0.0), f64::from)
}

fn usize_to_f64(val: usize) -> f64 {
    u32::try_from(val).map_or_else(|_| val.to_string().parse::<f64>().unwrap_or(0.0), f64::from)
}

fn truncate_to_minute(ts: DateTime<Utc>) -> DateTime<Utc> {
    ts.with_second(0)
        .and_then(|t| t.with_nanosecond(0))
        .unwrap_or(ts)
}

fn eval_rollup_agg(row: &RollupRow, agg: AggFn) -> Result<(Option<f64>, Option<String>), AppError> {
    match agg {
        AggFn::Sum => Ok((row.sum_v, None)),
        AggFn::Avg => Ok((
            row.sum_v.and_then(|s| {
                if row.cnt > 0 {
                    Some(s / i64_to_f64(row.cnt))
                } else {
                    None
                }
            }),
            None,
        )),
        AggFn::Count => Ok((Some(i64_to_f64(row.cnt)), None)),
        AggFn::Min => Ok((row.min_v, None)),
        AggFn::Max => Ok((row.max_v, None)),
        AggFn::Last => Ok((row.last_v, row.last_text.clone())),
        AggFn::CountDistinct => {
            let bytes = row.hll.as_deref().ok_or_else(|| {
                AppError::bad(
                    "INVALID_ARGUMENT",
                    "COUNT_DISTINCT is only valid for categorical (value_text) metrics",
                )
            })?;
            let est = estimate_hll(bytes)?;
            Ok((Some(est), None))
        }
        AggFn::Median => Err(AppError::bad(
            "INVALID_ARGUMENT",
            "MEDIAN aggregation is only supported at RAW grain",
        )),
    }
}

fn exact_median(mut values: Vec<f64>) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    let len = values.len();
    let mid = len / 2;
    if len % 2 == 1 {
        values.get(mid).copied()
    } else {
        let lo = values.get(mid.saturating_sub(1)).copied()?;
        let hi = values.get(mid).copied()?;
        Some(f64::midpoint(lo, hi))
    }
}

#[derive(Debug, Clone)]
struct ResolvedPoint {
    node_id: Uuid,
    metric: String,
    ts: DateTime<Utc>,
    value: Option<f64>,
    value_text: Option<String>,
}

impl MetricStorePort for PgStore {
    async fn write_metrics(
        &self,
        tenant_id: &str,
        req: WriteMetricsRequest,
    ) -> Result<WriteMetricsResponse, AppError> {
        if req.points.len() > 50_000 {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                format!(
                    "write_metrics accepts at most 50,000 points per call, got {}",
                    req.points.len()
                ),
            ));
        }
        if req.points.is_empty() {
            return Ok(WriteMetricsResponse {
                written: 0,
                dirty_minutes: 0,
                rollup_job_id: Uuid::nil(),
            });
        }

        validate_metric_inputs(&req.points)?;

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let deduped = resolve_and_dedup_points_tx(&mut tx, &req.points).await?;

        let cap = deduped.len();
        let mut nids = Vec::with_capacity(cap);
        let mut metrics = Vec::with_capacity(cap);
        let mut tss = Vec::with_capacity(cap);
        let mut vals: Vec<Option<f64>> = Vec::with_capacity(cap);
        let mut txts: Vec<Option<String>> = Vec::with_capacity(cap);

        for p in &deduped {
            nids.push(p.node_id);
            metrics.push(p.metric.clone());
            tss.push(p.ts);
            vals.push(p.value);
            txts.push(p.value_text.clone());
        }

        sqlx::query(
            "INSERT INTO xb_metric_series (
                 tenant_id, node_id, metric, ts, value, value_text, source_tag, batch_id
             )
             SELECT $1, u.node_id, u.metric, u.ts, u.value, u.value_text, $7, $8
               FROM UNNEST(
                   $2::uuid[], $3::text[], $4::timestamptz[], $5::float8[], $6::text[]
               ) AS u(node_id, metric, ts, value, value_text)
             ON CONFLICT (tenant_id, node_id, metric, ts, source_tag) DO UPDATE SET
                 value = EXCLUDED.value,
                 value_text = EXCLUDED.value_text,
                 batch_id = EXCLUDED.batch_id",
        )
        .bind(tenant_id)
        .bind(&nids)
        .bind(&metrics)
        .bind(&tss)
        .bind(&vals)
        .bind(&txts)
        .bind(req.source_tag.as_str())
        .bind(req.batch_id)
        .execute(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("bulk insert xb_metric_series failed: {err}")))?;

        sync_latest_node_snapshots_tx(&mut tx, tenant_id, &deduped).await?;
        let dirty_minutes = mark_dirty_minutes_tx(&mut tx, tenant_id, &deduped).await?;

        let minute_key = format!("{tenant_id}:{}", Utc::now().format("%Y-%m-%dT%H:%MZ"));
        let rollup_job =
            NewJob::new(JobKind::Rollup, serde_json::json!({})).with_idempotency_key(minute_key);
        let rollup_job_id = enqueue_job_tx(&mut tx, tenant_id, &rollup_job).await?;

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit write_metrics tx failed: {err}")))?;

        Ok(WriteMetricsResponse {
            written: cap,
            dirty_minutes,
            rollup_job_id,
        })
    }

    async fn query_node_metrics(
        &self,
        tenant_id: &str,
        node_id: Uuid,
        metric: &str,
        query: NodeMetricSeriesQuery,
    ) -> Result<Vec<MetricSeriesPoint>, AppError> {
        let metric_clean = metric.trim();
        if !is_valid_metric_name(metric_clean) {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                format!("invalid metric name '{metric_clean}'"),
            ));
        }
        if query.from > query.to {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "`from` must be less than or equal to `to`",
            ));
        }

        let mut current_grain = query
            .grain
            .unwrap_or_else(|| MetricQueryGrain::auto_select(query.from, query.to));

        if query.agg == AggFn::Median && current_grain != MetricQueryGrain::Raw {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "MEDIAN aggregation requires RAW grain",
            ));
        }

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let node_exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM xb_nodes WHERE id = $1)")
                .bind(node_id)
                .fetch_one(&mut *tx)
                .await
                .map_err(|err| AppError::Database(format!("check xb_nodes exists: {err}")))?;
        if !node_exists {
            return Err(AppError::NotFound("NODE_NOT_FOUND"));
        }

        loop {
            let mut series = match current_grain.as_rollup_grain() {
                None => {
                    query_raw_series_tx(
                        &mut tx,
                        node_id,
                        metric_clean,
                        query.from,
                        query.to,
                        query.agg,
                    )
                    .await?
                }
                Some(r_grain) => {
                    query_rollup_series_tx(
                        &mut tx,
                        node_id,
                        metric_clean,
                        r_grain,
                        query.from,
                        query.to,
                        query.agg,
                    )
                    .await?
                }
            };

            if series.len() <= 5_000 {
                tx.commit().await.map_err(|err| {
                    AppError::Database(format!("commit query_node_metrics tx: {err}"))
                })?;
                return Ok(series);
            }

            if let Some(promoted) = current_grain.promote() {
                current_grain = promoted;
            } else {
                series.truncate(5_000);
                tx.commit().await.map_err(|err| {
                    AppError::Database(format!("commit truncated query_node_metrics tx: {err}"))
                })?;
                return Ok(series);
            }
        }
    }

    async fn query_window(
        &self,
        tenant_id: &str,
        req: WindowQuery,
    ) -> Result<WindowQueryResponse, AppError> {
        let metric_clean = req.metric.trim();
        if !is_valid_metric_name(metric_clean) {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                format!("invalid metric name '{metric_clean}'"),
            ));
        }
        if req.from >= req.to {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "`from` must be strictly earlier than `to`",
            ));
        }
        if req.agg == AggFn::Median {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "MEDIAN aggregation is only supported at RAW grain",
            ));
        }

        let size_dur = WindowSpec::parse_duration(&req.window.size)?;
        let step_dur = match req.window.kind {
            WindowKind::Tumbling => size_dur,
            WindowKind::Sliding => match req.window.step.as_deref() {
                Some(step_str) => WindowSpec::parse_duration(step_str)?,
                None => size_dur,
            },
        };

        let span_secs = (req.to - req.from).num_seconds();
        let step_secs = step_dur.num_seconds().max(1);
        let approx_steps = span_secs / step_secs;
        if approx_steps > 10_000 {
            return Err(AppError::bad(
                "LIMIT_OUT_OF_RANGE",
                format!("window query produces too many steps ({approx_steps} > 10000)"),
            ));
        }

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let node_types = load_target_nodes_tx(
            &mut tx,
            req.node_ids.as_deref(),
            req.type_name
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty()),
        )
        .await?;

        if node_types.is_empty() {
            tx.commit().await.map_err(|err| {
                AppError::Database(format!("commit empty query_window tx: {err}"))
            })?;
            return Ok(WindowQueryResponse { items: Vec::new() });
        }

        let target_ids: Vec<Uuid> = node_types.keys().copied().collect();
        let use_hour_grain = size_dur.num_seconds() % 3600 == 0
            && step_dur.num_seconds() % 3600 == 0
            && req.from.minute() == 0
            && req.from.second() == 0;
        let source_grain = if use_hour_grain {
            RollupGrain::Hour
        } else {
            RollupGrain::Minute
        };

        let scan_start = match req.window.kind {
            WindowKind::Tumbling => req.from,
            WindowKind::Sliding => req.from - size_dur,
        };

        let rollup_rows = load_window_rollups_tx(
            &mut tx,
            &target_ids,
            metric_clean,
            source_grain,
            scan_start,
            req.to,
        )
        .await?;

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit query_window tx: {err}")))?;

        let items = evaluate_windows(
            &rollup_rows,
            &node_types,
            &req,
            size_dur,
            step_dur,
            source_grain,
        )?;
        Ok(WindowQueryResponse { items })
    }

    async fn list_metric_catalog(
        &self,
        tenant_id: &str,
    ) -> Result<Vec<MetricCatalogEntry>, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let rows = sqlx::query(
            "SELECT n.type_name, s.metric,
                    COUNT(*) AS point_count,
                    MIN(s.ts) AS min_ts,
                    MAX(s.ts) AS max_ts
               FROM xb_metric_series s
               JOIN xb_nodes n ON n.id = s.node_id
              GROUP BY n.type_name, s.metric
              ORDER BY n.type_name ASC, s.metric ASC",
        )
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("list_metric_catalog failed: {err}")))?;
        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit list_metric_catalog tx failed: {err}"))
        })?;

        Ok(rows
            .into_iter()
            .map(|r| MetricCatalogEntry {
                type_name: r.get("type_name"),
                metric: r.get("metric"),
                point_count: r.get("point_count"),
                min_ts: r.get("min_ts"),
                max_ts: r.get("max_ts"),
            })
            .collect())
    }

    async fn run_rollup_for_tenant(&self, tenant_id: &str) -> Result<u64, AppError> {
        crate::rollup::run_rollup_for_tenant_impl(self, tenant_id).await
    }
}

fn validate_metric_inputs(points: &[MetricPointInput]) -> Result<(), AppError> {
    for pt in points {
        if !is_valid_metric_name(pt.metric.trim()) {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                format!("metric '{}' must match ^[a-z][a-z0-9_]{{0,62}}$", pt.metric),
            ));
        }
        if pt.node_id.is_none() && pt.node.is_none() {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "metric point must specify either `node_id` or `node`",
            ));
        }
        let has_txt = pt.value_text.as_deref().is_some_and(|s| !s.is_empty());
        if pt.value.is_none() && !has_txt {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "metric point must specify `value` or non-empty `value_text`",
            ));
        }
        if let Some(v) = pt.value
            && !v.is_finite()
        {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "metric numeric value must be finite",
            ));
        }
    }
    Ok(())
}

async fn resolve_and_dedup_points_tx(
    tx: &mut Transaction<'_, Postgres>,
    points: &[MetricPointInput],
) -> Result<Vec<ResolvedPoint>, AppError> {
    let mut explicit_ids = BTreeSet::new();
    let mut key_refs = BTreeSet::new();

    for pt in points {
        if let Some(id) = pt.node_id {
            explicit_ids.insert(id);
        } else if let Some(NodeRef {
            type_name,
            node_key,
        }) = &pt.node
        {
            key_refs.insert((
                String::from(type_name.trim()),
                String::from(node_key.trim()),
            ));
        }
    }

    let valid_ids: BTreeSet<Uuid> = if explicit_ids.is_empty() {
        BTreeSet::new()
    } else {
        let vec_ids: Vec<Uuid> = explicit_ids.iter().copied().collect();
        let rows = sqlx::query("SELECT id FROM xb_nodes WHERE id = ANY($1)")
            .bind(&vec_ids)
            .fetch_all(&mut **tx)
            .await
            .map_err(|err| AppError::Database(format!("verify metric node_ids: {err}")))?;
        rows.into_iter().map(|r| r.get("id")).collect()
    };

    for req_id in &explicit_ids {
        if !valid_ids.contains(req_id) {
            return Err(AppError::NotFound("NODE_NOT_FOUND"));
        }
    }

    let key_map: HashMap<(String, String), Uuid> = if key_refs.is_empty() {
        HashMap::new()
    } else {
        let kt: Vec<String> = key_refs.iter().map(|(t, _)| t.clone()).collect();
        let kk: Vec<String> = key_refs.iter().map(|(_, k)| k.clone()).collect();
        let rows = sqlx::query(
            "SELECT x.id, x.type_name, x.node_key
               FROM xb_nodes x
               JOIN UNNEST($1::text[], $2::text[]) AS p(type_name, node_key)
                 ON x.type_name = p.type_name AND x.node_key = p.node_key",
        )
        .bind(&kt)
        .bind(&kk)
        .fetch_all(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("resolve metric NodeRefs: {err}")))?;
        rows.into_iter()
            .map(|r| ((r.get("type_name"), r.get("node_key")), r.get("id")))
            .collect()
    };

    for pair in &key_refs {
        if !key_map.contains_key(pair) {
            return Err(AppError::NotFound("NODE_NOT_FOUND"));
        }
    }

    let mut dedup: BTreeMap<(Uuid, String, DateTime<Utc>), ResolvedPoint> = BTreeMap::new();
    for pt in points {
        let nid = if let Some(id) = pt.node_id {
            id
        } else if let Some(NodeRef {
            type_name,
            node_key,
        }) = &pt.node
        {
            *key_map
                .get(&(
                    String::from(type_name.trim()),
                    String::from(node_key.trim()),
                ))
                .ok_or(AppError::NotFound("NODE_NOT_FOUND"))?
        } else {
            return Err(AppError::bad("INVALID_ARGUMENT", "missing node target"));
        };
        let m = String::from(pt.metric.trim());
        dedup.insert(
            (nid, m.clone(), pt.ts),
            ResolvedPoint {
                node_id: nid,
                metric: m,
                ts: pt.ts,
                value: pt.value,
                value_text: pt.value_text.clone(),
            },
        );
    }

    Ok(dedup.into_values().collect())
}

async fn sync_latest_node_snapshots_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    points: &[ResolvedPoint],
) -> Result<(), AppError> {
    let mut latest_by_node: BTreeMap<Uuid, BTreeMap<String, (DateTime<Utc>, f64)>> =
        BTreeMap::new();
    for pt in points {
        if let Some(v) = pt.value {
            let entry = latest_by_node.entry(pt.node_id).or_default();
            let replace = entry
                .get(&pt.metric)
                .is_none_or(|(prev_ts, _)| pt.ts >= *prev_ts);
            if replace {
                entry.insert(pt.metric.clone(), (pt.ts, v));
            }
        }
    }

    if latest_by_node.is_empty() {
        return Ok(());
    }

    let target_ids: Vec<Uuid> = latest_by_node.keys().copied().collect();
    let rows =
        sqlx::query("SELECT id, properties, metrics FROM xb_nodes WHERE id = ANY($1) FOR UPDATE")
            .bind(&target_ids)
            .fetch_all(&mut **tx)
            .await
            .map_err(|err| {
                AppError::Database(format!("lock xb_nodes for metric snapshot: {err}"))
            })?;

    let mut changed_nodes: Vec<Uuid> = Vec::new();
    for r in rows {
        let nid: Uuid = r.get("id");
        let Some(incoming) = latest_by_node.get(&nid) else {
            continue;
        };

        let mut props_map = match r.get::<serde_json::Value, _>("properties") {
            serde_json::Value::Object(m) => m,
            _ => serde_json::Map::new(),
        };
        let mut metrics_map: BTreeMap<String, f64> =
            serde_json::from_value(r.get("metrics")).unwrap_or_default();

        let mut ts_obj = match props_map.remove("__metric_ts") {
            Some(serde_json::Value::Object(m)) => m,
            _ => serde_json::Map::new(),
        };

        let mut node_modified = false;
        for (m_name, (pt_ts, pt_val)) in incoming {
            let prev_ts = ts_obj
                .get(m_name)
                .and_then(serde_json::Value::as_str)
                .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt.with_timezone(&Utc));

            if prev_ts.is_none_or(|prev| *pt_ts >= prev) {
                let val_diff = metrics_map
                    .get(m_name)
                    .is_none_or(|old_v| (*old_v - *pt_val).abs() > f64::EPSILON);
                metrics_map.insert(m_name.clone(), *pt_val);
                ts_obj.insert(
                    m_name.clone(),
                    serde_json::Value::String(pt_ts.to_rfc3339()),
                );
                if val_diff {
                    node_modified = true;
                }
            }
        }

        props_map.insert(
            String::from("__metric_ts"),
            serde_json::Value::Object(ts_obj),
        );
        let new_metrics_json = serde_json::to_value(&metrics_map).map_err(|err| {
            AppError::bad("INVALID_ARGUMENT", format!("serialize node metrics: {err}"))
        })?;

        sqlx::query(
            "UPDATE xb_nodes
                SET properties = $2,
                    metrics = $3,
                    updated_at = NOW()
              WHERE id = $1",
        )
        .bind(nid)
        .bind(serde_json::Value::Object(props_map))
        .bind(new_metrics_json)
        .execute(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("update node metric snapshot: {err}")))?;

        if node_modified {
            changed_nodes.push(nid);
        }
    }

    for chunk in changed_nodes.chunks(1_000) {
        insert_outbox_tx(
            tx,
            tenant_id,
            &NewOutboxEvent {
                topic: OutboxTopic::NodeMetricsChanged,
                payload: serde_json::json!({ "node_ids": chunk }),
            },
        )
        .await?;
    }

    Ok(())
}

async fn mark_dirty_minutes_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    points: &[ResolvedPoint],
) -> Result<usize, AppError> {
    let maybe_wm: Option<DateTime<Utc>> =
        sqlx::query_scalar("SELECT done_until FROM xb_rollup_watermarks WHERE grain = 'MINUTE'")
            .fetch_optional(&mut **tx)
            .await
            .map_err(|err| AppError::Database(format!("query MINUTE watermark failed: {err}")))?;

    let Some(wm) = maybe_wm else {
        return Ok(0);
    };

    let mut dirty_set: BTreeSet<(Uuid, String, DateTime<Utc>)> = BTreeSet::new();
    for pt in points {
        let min_start = truncate_to_minute(pt.ts);
        if min_start <= wm {
            dirty_set.insert((pt.node_id, pt.metric.clone(), min_start));
        }
    }

    if dirty_set.is_empty() {
        return Ok(0);
    }

    let d_nids: Vec<Uuid> = dirty_set.iter().map(|(n, _, _)| *n).collect();
    let d_metrics: Vec<String> = dirty_set.iter().map(|(_, m, _)| m.clone()).collect();
    let d_starts: Vec<DateTime<Utc>> = dirty_set.iter().map(|(_, _, s)| *s).collect();

    sqlx::query(
        "INSERT INTO xb_rollup_dirty (tenant_id, node_id, metric, minute_start)
         SELECT $1, u.node_id, u.metric, u.minute_start
           FROM UNNEST($2::uuid[], $3::text[], $4::timestamptz[])
                AS u(node_id, metric, minute_start)
         ON CONFLICT (tenant_id, node_id, metric, minute_start) DO NOTHING",
    )
    .bind(tenant_id)
    .bind(&d_nids)
    .bind(&d_metrics)
    .bind(&d_starts)
    .execute(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("insert xb_rollup_dirty failed: {err}")))?;

    Ok(dirty_set.len())
}

type RawSamplePair = (Option<f64>, Option<String>);

async fn query_raw_series_tx(
    tx: &mut Transaction<'_, Postgres>,
    node_id: Uuid,
    metric: &str,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    agg: AggFn,
) -> Result<Vec<MetricSeriesPoint>, AppError> {
    let rows = sqlx::query(
        "SELECT ts, value, value_text
           FROM xb_metric_series
          WHERE node_id = $1
            AND metric = $2
            AND ts >= $3
            AND (ts < $4 OR ($3 = $4 AND ts = $4))
          ORDER BY ts ASC",
    )
    .bind(node_id)
    .bind(metric)
    .bind(from)
    .bind(to)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("query raw xb_metric_series failed: {err}")))?;

    if rows.is_empty() {
        return Ok(Vec::new());
    }

    if agg == AggFn::Median {
        let mut all_vals = Vec::with_capacity(rows.len());
        for r in &rows {
            if let Some(v) = r.get::<Option<f64>, _>("value") {
                all_vals.push(v);
            }
        }
        return Ok(match exact_median(all_vals) {
            Some(med) => vec![MetricSeriesPoint {
                ts: from,
                value: Some(med),
                value_text: None,
            }],
            None => Vec::new(),
        });
    }

    if agg == AggFn::CountDistinct {
        let mut by_ts: BTreeMap<DateTime<Utc>, BTreeSet<String>> = BTreeMap::new();
        for r in rows {
            let ts: DateTime<Utc> = r.get("ts");
            let txt: Option<String> = r.get("value_text");
            let Some(s) = txt else {
                return Err(AppError::bad(
                    "INVALID_ARGUMENT",
                    "COUNT_DISTINCT is only valid for categorical (value_text) metrics",
                ));
            };
            by_ts.entry(ts).or_default().insert(s);
        }
        return Ok(by_ts
            .into_iter()
            .map(|(ts, set)| MetricSeriesPoint {
                ts,
                value: Some(usize_to_f64(set.len())),
                value_text: None,
            })
            .collect());
    }

    let mut by_ts: BTreeMap<DateTime<Utc>, Vec<RawSamplePair>> = BTreeMap::new();
    for r in rows {
        by_ts
            .entry(r.get("ts"))
            .or_default()
            .push((r.get("value"), r.get("value_text")));
    }

    let mut out = Vec::with_capacity(by_ts.len());
    for (ts, samples) in by_ts {
        let nums: Vec<f64> = samples.iter().filter_map(|(v, _)| *v).collect();
        let last_txt: Option<String> = samples.iter().rev().find_map(|(_, t)| t.clone());

        let (value, value_text) = match agg {
            AggFn::Count => (Some(usize_to_f64(samples.len())), None),
            AggFn::Sum => ((!nums.is_empty()).then(|| nums.iter().sum()), None),
            AggFn::Avg => (
                (!nums.is_empty()).then(|| nums.iter().sum::<f64>() / usize_to_f64(nums.len())),
                None,
            ),
            AggFn::Min => (nums.iter().copied().reduce(f64::min), None),
            AggFn::Max => (nums.iter().copied().reduce(f64::max), None),
            AggFn::Last => (nums.last().copied(), last_txt),
            AggFn::Median | AggFn::CountDistinct => (None, None),
        };
        out.push(MetricSeriesPoint {
            ts,
            value,
            value_text,
        });
    }
    Ok(out)
}

async fn query_rollup_series_tx(
    tx: &mut Transaction<'_, Postgres>,
    node_id: Uuid,
    metric: &str,
    grain: RollupGrain,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    agg: AggFn,
) -> Result<Vec<MetricSeriesPoint>, AppError> {
    let rows = sqlx::query(
        "SELECT node_id, metric, bucket_start, cnt, sum_v, min_v, max_v,
                last_v, last_text, last_ts, hll, freq
           FROM xb_metric_rollups
          WHERE node_id = $1
            AND metric = $2
            AND grain = $3
            AND bucket_start >= $4
            AND (bucket_start < $5 OR ($4 = $5 AND bucket_start = $5))
          ORDER BY bucket_start ASC",
    )
    .bind(node_id)
    .bind(metric)
    .bind(grain.as_str())
    .bind(from)
    .bind(to)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("query xb_metric_rollups failed: {err}")))?;

    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let rollup = decode_rollup_row(&r, grain);
        let (value, value_text) = eval_rollup_agg(&rollup, agg)?;
        out.push(MetricSeriesPoint {
            ts: rollup.bucket_start,
            value,
            value_text,
        });
    }
    Ok(out)
}

async fn load_target_nodes_tx(
    tx: &mut Transaction<'_, Postgres>,
    node_ids: Option<&[Uuid]>,
    type_name: Option<&str>,
) -> Result<BTreeMap<Uuid, String>, AppError> {
    let rows = match node_ids.filter(|ids| !ids.is_empty()) {
        None => sqlx::query(
            "SELECT id, type_name FROM xb_nodes WHERE ($1::text IS NULL OR type_name = $1)",
        )
        .bind(type_name)
        .fetch_all(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("load window target nodes: {err}")))?,
        Some(ids) => sqlx::query(
            "SELECT id, type_name FROM xb_nodes
                  WHERE id = ANY($1) AND ($2::text IS NULL OR type_name = $2)",
        )
        .bind(ids)
        .bind(type_name)
        .fetch_all(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("load window explicit nodes: {err}")))?,
    };

    Ok(rows
        .into_iter()
        .map(|r| (r.get("id"), r.get("type_name")))
        .collect())
}

async fn load_window_rollups_tx(
    tx: &mut Transaction<'_, Postgres>,
    node_ids: &[Uuid],
    metric: &str,
    grain: RollupGrain,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> Result<Vec<RollupRow>, AppError> {
    let rows = sqlx::query(
        "SELECT node_id, metric, bucket_start, cnt, sum_v, min_v, max_v,
                last_v, last_text, last_ts, hll, freq
           FROM xb_metric_rollups
          WHERE node_id = ANY($1)
            AND metric = $2
            AND grain = $3
            AND bucket_start >= $4
            AND bucket_start < $5
          ORDER BY bucket_start ASC",
    )
    .bind(node_ids)
    .bind(metric)
    .bind(grain.as_str())
    .bind(from)
    .bind(to)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("load window xb_metric_rollups: {err}")))?;

    Ok(rows.iter().map(|r| decode_rollup_row(r, grain)).collect())
}

fn evaluate_windows(
    rollups: &[RollupRow],
    node_types: &BTreeMap<Uuid, String>,
    req: &WindowQuery,
    size_dur: Duration,
    step_dur: Duration,
    source_grain: RollupGrain,
) -> Result<Vec<WindowBucketItem>, AppError> {
    let mut grouped_series: BTreeMap<Option<String>, Vec<&RollupRow>> = BTreeMap::new();
    match req.group_by {
        WindowGroupBy::Node => {
            for nid in node_types.keys() {
                grouped_series.insert(Some(nid.to_string()), Vec::new());
            }
            for r in rollups {
                grouped_series
                    .entry(Some(r.node_id.to_string()))
                    .or_default()
                    .push(r);
            }
        }
        WindowGroupBy::Type => {
            let distinct_types: BTreeSet<String> = node_types.values().cloned().collect();
            for t in distinct_types {
                grouped_series.insert(Some(t), Vec::new());
            }
            for r in rollups {
                if let Some(tname) = node_types.get(&r.node_id) {
                    grouped_series
                        .entry(Some(tname.clone()))
                        .or_default()
                        .push(r);
                }
            }
        }
        WindowGroupBy::None => {
            grouped_series.insert(None, rollups.iter().collect());
        }
    }

    let mut out = Vec::new();
    let mut cursor = req.from;

    while cursor < req.to {
        let (win_start, win_end) = match req.window.kind {
            WindowKind::Tumbling => {
                let end = (cursor + size_dur).min(req.to);
                (cursor, end)
            }
            WindowKind::Sliding => {
                let end = (cursor + step_dur).min(req.to);
                (end - size_dur, end)
            }
        };

        for (group_key, series) in &grouped_series {
            let slice: Vec<RollupRow> = series
                .iter()
                .filter(|r| r.bucket_start >= win_start && r.bucket_start < win_end)
                .map(|r| (*r).clone())
                .collect();

            let value = if slice.is_empty() {
                if req.agg == AggFn::Count {
                    Some(0.0)
                } else {
                    None
                }
            } else if let Some(merged) = merge_rollup_slice(&slice, source_grain, cursor)? {
                eval_rollup_agg(&merged, req.agg)?.0
            } else {
                None
            };

            out.push(WindowBucketItem {
                ts: cursor,
                group_key: group_key.clone(),
                value,
            });
        }

        cursor += step_dur;
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_median_handles_odd_and_even_samples() {
        assert_eq!(exact_median(Vec::new()), None);
        assert_eq!(exact_median(vec![30.0, 10.0, 20.0]), Some(20.0));
        assert_eq!(exact_median(vec![40.0, 10.0, 20.0, 30.0]), Some(25.0));
    }

    #[test]
    fn metric_name_validation_enforces_snake_case_identifier() {
        assert!(is_valid_metric_name("inventory"));
        assert!(is_valid_metric_name("algo_pagerank_1"));
        assert!(!is_valid_metric_name(""));
        assert!(!is_valid_metric_name("1_bad"));
        assert!(!is_valid_metric_name("BadUpper"));
        assert!(!is_valid_metric_name("bad-dash"));
    }
}
