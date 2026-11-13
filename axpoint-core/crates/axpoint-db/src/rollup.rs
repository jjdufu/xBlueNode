//! Multi-grain time-series rollup cascade (`MINUTE -> HOUR -> DAY -> MONTH`), late-arrival dirty
//! recomputation, and monthly partition lifecycle helpers for Step 03 (`AxGraph`).

use std::collections::{BTreeMap, BTreeSet};

use axpoint_domain::AppError;
use axpoint_domain::enums::RollupGrain;
use axpoint_domain::graph::{RollupRow, build_freq_map, build_hll_bytes, merge_rollup_rows};
use chrono::{DateTime, Duration, Timelike, Utc};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

use crate::PgStore;
use crate::tenant::begin_tenant_tx;

#[derive(Debug, Clone)]
struct RawPointSample {
    ts: DateTime<Utc>,
    value: Option<f64>,
    value_text: Option<String>,
}

fn truncate_to_minute(ts: DateTime<Utc>) -> DateTime<Utc> {
    ts.with_second(0)
        .and_then(|t| t.with_nanosecond(0))
        .unwrap_or(ts)
}

fn truncate_to_hour(ts: DateTime<Utc>) -> DateTime<Utc> {
    ts.with_minute(0)
        .and_then(|t| t.with_second(0))
        .and_then(|t| t.with_nanosecond(0))
        .unwrap_or(ts)
}

pub(crate) fn decode_rollup_row(row: &sqlx::postgres::PgRow, grain: RollupGrain) -> RollupRow {
    let freq_val: Option<serde_json::Value> = row.get("freq");
    let freq: Option<BTreeMap<String, u64>> = freq_val.and_then(|v| serde_json::from_value(v).ok());

    RollupRow {
        node_id: row.get("node_id"),
        metric: row.get("metric"),
        grain,
        bucket_start: row.get("bucket_start"),
        cnt: row.get("cnt"),
        sum_v: row.get("sum_v"),
        min_v: row.get("min_v"),
        max_v: row.get("max_v"),
        last_v: row.get("last_v"),
        last_text: row.get("last_text"),
        last_ts: row.get("last_ts"),
        hll: row.get("hll"),
        freq,
    }
}

/// Folds a non-empty slice of [`RollupRow`] items into a single merged [`RollupRow`] using [`merge_rollup_rows`].
pub(crate) fn merge_rollup_slice(
    rows: &[RollupRow],
    target_grain: RollupGrain,
    bucket_start: DateTime<Utc>,
) -> Result<Option<RollupRow>, AppError> {
    let Some((first, rest)) = rows.split_first() else {
        return Ok(None);
    };
    let mut acc = first.clone();
    acc.grain = target_grain;
    acc.bucket_start = bucket_start;
    for next in rest {
        acc = merge_rollup_rows(&acc, next, target_grain, bucket_start)?;
    }
    Ok(Some(acc))
}

fn aggregate_minute_bucket(
    node_id: Uuid,
    metric: String,
    bucket_start: DateTime<Utc>,
    samples: &[RawPointSample],
) -> Option<RollupRow> {
    let first = samples.first()?;
    let cnt = i64::try_from(samples.len()).unwrap_or(i64::MAX);

    let mut sum_v = 0.0_f64;
    let mut has_num = false;
    let mut min_v: Option<f64> = None;
    let mut max_v: Option<f64> = None;
    let mut last_num: Option<(DateTime<Utc>, f64)> = None;
    let mut last_txt: Option<(DateTime<Utc>, String)> = None;
    let mut last_ts = first.ts;
    let mut texts: Vec<String> = Vec::new();

    for s in samples {
        if s.ts > last_ts {
            last_ts = s.ts;
        }
        if let Some(v) = s.value {
            has_num = true;
            sum_v += v;
            min_v = Some(min_v.map_or(v, |cur| cur.min(v)));
            max_v = Some(max_v.map_or(v, |cur| cur.max(v)));
            if last_num.is_none_or(|(prev_ts, _)| s.ts >= prev_ts) {
                last_num = Some((s.ts, v));
            }
        }
        if let Some(ref txt) = s.value_text {
            texts.push(txt.clone());
            if last_txt
                .as_ref()
                .is_none_or(|(prev_ts, _)| s.ts >= *prev_ts)
            {
                last_txt = Some((s.ts, txt.clone()));
            }
        }
    }

    let (hll, freq) = if texts.is_empty() {
        (None, None)
    } else {
        let hll_bytes = build_hll_bytes(&texts).ok();
        let freq_map = build_freq_map(&texts);
        (hll_bytes, Some(freq_map))
    };

    Some(RollupRow {
        node_id,
        metric,
        grain: RollupGrain::Minute,
        bucket_start,
        cnt,
        sum_v: has_num.then_some(sum_v),
        min_v,
        max_v,
        last_v: last_num.map(|(_, v)| v),
        last_text: last_txt.map(|(_, t)| t),
        last_ts,
        hll,
        freq,
    })
}

async fn upsert_rollup_rows_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    rows: &[RollupRow],
) -> Result<u64, AppError> {
    if rows.is_empty() {
        return Ok(0);
    }

    let cap = rows.len();
    let mut node_ids = Vec::with_capacity(cap);
    let mut metrics = Vec::with_capacity(cap);
    let mut grains = Vec::with_capacity(cap);
    let mut starts = Vec::with_capacity(cap);
    let mut cnts = Vec::with_capacity(cap);
    let mut sums: Vec<Option<f64>> = Vec::with_capacity(cap);
    let mut mins: Vec<Option<f64>> = Vec::with_capacity(cap);
    let mut maxs: Vec<Option<f64>> = Vec::with_capacity(cap);
    let mut last_vs: Vec<Option<f64>> = Vec::with_capacity(cap);
    let mut last_txts: Vec<Option<String>> = Vec::with_capacity(cap);
    let mut last_tss = Vec::with_capacity(cap);
    let mut hlls: Vec<Option<Vec<u8>>> = Vec::with_capacity(cap);
    let mut freqs: Vec<Option<serde_json::Value>> = Vec::with_capacity(cap);

    for r in rows {
        node_ids.push(r.node_id);
        metrics.push(r.metric.clone());
        grains.push(String::from(r.grain.as_str()));
        starts.push(r.bucket_start);
        cnts.push(r.cnt);
        sums.push(r.sum_v);
        mins.push(r.min_v);
        maxs.push(r.max_v);
        last_vs.push(r.last_v);
        last_txts.push(r.last_text.clone());
        last_tss.push(r.last_ts);
        hlls.push(r.hll.clone());
        let freq_json = match r.freq {
            Some(ref m) => Some(serde_json::to_value(m).map_err(|err| {
                AppError::bad("INVALID_ARGUMENT", format!("serialize freq failed: {err}"))
            })?),
            None => None,
        };
        freqs.push(freq_json);
    }

    let res = sqlx::query(
        "INSERT INTO xb_metric_rollups (
             tenant_id, node_id, metric, grain, bucket_start,
             cnt, sum_v, min_v, max_v, last_v, last_text, last_ts, hll, freq
         )
         SELECT $1, u.node_id, u.metric, u.grain, u.bucket_start,
                u.cnt, u.sum_v, u.min_v, u.max_v, u.last_v, u.last_text, u.last_ts, u.hll, u.freq
           FROM UNNEST(
               $2::uuid[], $3::text[], $4::text[], $5::timestamptz[],
               $6::bigint[], $7::float8[], $8::float8[], $9::float8[],
               $10::float8[], $11::text[], $12::timestamptz[], $13::bytea[], $14::jsonb[]
           ) AS u(
               node_id, metric, grain, bucket_start,
               cnt, sum_v, min_v, max_v, last_v, last_text, last_ts, hll, freq
           )
         ON CONFLICT (tenant_id, node_id, metric, grain, bucket_start) DO UPDATE SET
             cnt = EXCLUDED.cnt,
             sum_v = EXCLUDED.sum_v,
             min_v = EXCLUDED.min_v,
             max_v = EXCLUDED.max_v,
             last_v = EXCLUDED.last_v,
             last_text = EXCLUDED.last_text,
             last_ts = EXCLUDED.last_ts,
             hll = EXCLUDED.hll,
             freq = EXCLUDED.freq",
    )
    .bind(tenant_id)
    .bind(&node_ids)
    .bind(&metrics)
    .bind(&grains)
    .bind(&starts)
    .bind(&cnts)
    .bind(&sums)
    .bind(&mins)
    .bind(&maxs)
    .bind(&last_vs)
    .bind(&last_txts)
    .bind(&last_tss)
    .bind(&hlls)
    .bind(&freqs)
    .execute(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("upsert xb_metric_rollups failed: {err}")))?;

    Ok(res.rows_affected())
}

async fn upsert_watermark_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    grain: RollupGrain,
    done_until: DateTime<Utc>,
) -> Result<(), AppError> {
    sqlx::query(
        "INSERT INTO xb_rollup_watermarks (tenant_id, grain, done_until)
         VALUES ($1, $2, $3)
         ON CONFLICT (tenant_id, grain) DO UPDATE SET
             done_until = GREATEST(xb_rollup_watermarks.done_until, EXCLUDED.done_until)",
    )
    .bind(tenant_id)
    .bind(grain.as_str())
    .bind(done_until)
    .execute(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("upsert xb_rollup_watermarks failed: {err}")))?;
    Ok(())
}

impl PgStore {
    /// Lists active tenant identifiers via the `SECURITY DEFINER` function `ax_rollup_tenants()`.
    ///
    /// # Errors
    /// Returns [`AppError::Database`] if querying `ax_rollup_tenants()` fails.
    pub async fn list_rollup_tenants(&self) -> Result<Vec<String>, AppError> {
        let rows = sqlx::query("SELECT * FROM ax_rollup_tenants()")
            .fetch_all(self.pool())
            .await
            .map_err(|err| AppError::Database(format!("ax_rollup_tenants failed: {err}")))?;
        Ok(rows.into_iter().map(|r| r.get(0)).collect())
    }

    /// Ensures monthly partitions exist on `xb_metric_series` via `ax_ensure_metric_partitions`.
    ///
    /// # Errors
    /// Returns [`AppError::Database`] if calling `ax_ensure_metric_partitions` fails.
    pub async fn ensure_metric_partitions(
        &self,
        months_back: i32,
        months_ahead: i32,
    ) -> Result<i32, AppError> {
        let created: i32 = sqlx::query_scalar("SELECT ax_ensure_metric_partitions($1, $2)")
            .bind(months_back)
            .bind(months_ahead)
            .fetch_one(self.pool())
            .await
            .map_err(|err| {
                AppError::Database(format!("ax_ensure_metric_partitions failed: {err}"))
            })?;
        Ok(created)
    }

    /// Drops expired `xb_metric_series` monthly partitions beyond all active tenants' max retention.
    ///
    /// # Errors
    /// Returns [`AppError::Database`] if calling `ax_drop_expired_metric_partitions` fails.
    pub async fn drop_expired_metric_partitions(&self) -> Result<i32, AppError> {
        let dropped: i32 = sqlx::query_scalar("SELECT ax_drop_expired_metric_partitions()")
            .fetch_one(self.pool())
            .await
            .map_err(|err| {
                AppError::Database(format!("ax_drop_expired_metric_partitions failed: {err}"))
            })?;
        Ok(dropped)
    }
}

pub(crate) async fn run_rollup_for_tenant_impl(
    store: &PgStore,
    tenant_id: &str,
) -> Result<u64, AppError> {
    let tz: String = sqlx::query_scalar("SELECT timezone FROM sec_tenants WHERE tenant_id = $1")
        .bind(tenant_id)
        .fetch_optional(store.pool())
        .await
        .map_err(|err| AppError::Database(format!("query sec_tenants.timezone: {err}")))?
        .unwrap_or_else(|| String::from("Asia/Shanghai"));

    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;

    let bounds_row = sqlx::query(
        "SELECT
             date_trunc('minute', NOW()) - INTERVAL '1 minute' AS last_minute,
             date_trunc('hour', NOW()) - INTERVAL '1 hour' AS last_hour,
             (date_trunc('day', NOW() AT TIME ZONE $1) - INTERVAL '1 day') AT TIME ZONE $1 AS last_day,
             (date_trunc('month', NOW() AT TIME ZONE $1) - INTERVAL '1 month') AT TIME ZONE $1 AS last_month",
    )
    .bind(&tz)
    .fetch_one(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("compute rollup bounds failed: {err}")))?;

    let completed_minute: DateTime<Utc> = bounds_row.get("last_minute");
    let completed_hour: DateTime<Utc> = bounds_row.get("last_hour");
    let completed_day: DateTime<Utc> = bounds_row.get("last_day");
    let completed_month: DateTime<Utc> = bounds_row.get("last_month");

    let wm_rows = sqlx::query("SELECT grain, done_until FROM xb_rollup_watermarks FOR UPDATE")
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("load xb_rollup_watermarks: {err}")))?;

    let mut watermarks: BTreeMap<String, DateTime<Utc>> = BTreeMap::new();
    for r in wm_rows {
        watermarks.insert(r.get("grain"), r.get("done_until"));
    }

    let dirty_rows =
        sqlx::query("SELECT node_id, metric, minute_start FROM xb_rollup_dirty FOR UPDATE")
            .fetch_all(&mut *tx)
            .await
            .map_err(|err| AppError::Database(format!("load xb_rollup_dirty failed: {err}")))?;

    let mut dirty_minutes: BTreeSet<(Uuid, String, DateTime<Utc>)> = BTreeSet::new();
    for r in dirty_rows {
        dirty_minutes.insert((r.get("node_id"), r.get("metric"), r.get("minute_start")));
    }

    let has_any_raw: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM xb_metric_series LIMIT 1)")
            .fetch_one(&mut *tx)
            .await
            .map_err(|err| AppError::Database(format!("probe xb_metric_series: {err}")))?;

    if !has_any_raw && dirty_minutes.is_empty() && watermarks.is_empty() {
        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit noop rollup tx: {err}")))?;
        return Ok(0);
    }

    let (minute_written, touched_hours) = rollup_minutes_stage_tx(
        &mut tx,
        tenant_id,
        watermarks.get("MINUTE").copied(),
        &dirty_minutes,
        completed_minute,
    )
    .await?;

    let (hour_written, touched_hours_all) = rollup_hours_stage_tx(
        &mut tx,
        tenant_id,
        watermarks.get("HOUR").copied(),
        &touched_hours,
        completed_hour,
    )
    .await?;

    let (day_written, touched_days) = rollup_days_stage_tx(
        &mut tx,
        tenant_id,
        &tz,
        watermarks.get("DAY").copied(),
        &touched_hours_all,
        completed_day,
    )
    .await?;

    let month_written = rollup_months_stage_tx(
        &mut tx,
        tenant_id,
        &tz,
        watermarks.get("MONTH").copied(),
        &touched_days,
        completed_month,
    )
    .await?;

    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit rollup tx failed: {err}")))?;

    Ok(minute_written
        .saturating_add(hour_written)
        .saturating_add(day_written)
        .saturating_add(month_written))
}

async fn rollup_minutes_stage_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    wm_minute: Option<DateTime<Utc>>,
    dirty_minutes: &BTreeSet<(Uuid, String, DateTime<Utc>)>,
    completed_minute: DateTime<Utc>,
) -> Result<(u64, BTreeSet<DateTime<Utc>>), AppError> {
    let mut grouped: BTreeMap<(Uuid, String, DateTime<Utc>), Vec<RawPointSample>> = BTreeMap::new();

    let raw_rows = if let Some(done) = wm_minute {
        let start_from = done + Duration::minutes(1);
        sqlx::query(
            "SELECT node_id, metric, ts, value, value_text
               FROM xb_metric_series
              WHERE ts >= $1
              ORDER BY ts ASC",
        )
        .bind(start_from)
        .fetch_all(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("scan incremental xb_metric_series: {err}")))?
    } else {
        sqlx::query(
            "SELECT node_id, metric, ts, value, value_text
               FROM xb_metric_series
              ORDER BY ts ASC",
        )
        .fetch_all(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("scan initial xb_metric_series: {err}")))?
    };

    for r in raw_rows {
        let nid: Uuid = r.get("node_id");
        let metric: String = r.get("metric");
        let ts: DateTime<Utc> = r.get("ts");
        let min_start = truncate_to_minute(ts);
        grouped
            .entry((nid, metric, min_start))
            .or_default()
            .push(RawPointSample {
                ts,
                value: r.get("value"),
                value_text: r.get("value_text"),
            });
    }

    if !dirty_minutes.is_empty() {
        reload_dirty_minutes_tx(tx, dirty_minutes, &mut grouped).await?;
    }

    let mut touched_hours: BTreeSet<DateTime<Utc>> = BTreeSet::new();
    for (_, _, min_start) in dirty_minutes {
        touched_hours.insert(truncate_to_hour(*min_start));
    }

    let mut minute_rollups = Vec::with_capacity(grouped.len());
    for ((nid, metric, min_start), samples) in grouped {
        touched_hours.insert(truncate_to_hour(min_start));
        if let Some(row) = aggregate_minute_bucket(nid, metric, min_start, &samples) {
            minute_rollups.push(row);
        }
    }

    let written = upsert_rollup_rows_tx(tx, tenant_id, &minute_rollups).await?;
    upsert_watermark_tx(tx, tenant_id, RollupGrain::Minute, completed_minute).await?;
    Ok((written, touched_hours))
}

async fn rollup_hours_stage_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    wm_hour: Option<DateTime<Utc>>,
    dirty_hours: &BTreeSet<DateTime<Utc>>,
    completed_hour: DateTime<Utc>,
) -> Result<(u64, BTreeSet<DateTime<Utc>>), AppError> {
    let mut target_hours = dirty_hours.clone();

    let new_hr_rows = if let Some(done) = wm_hour {
        let start_from = done + Duration::hours(1);
        sqlx::query(
            "SELECT DISTINCT date_trunc('hour', bucket_start) AS hr
               FROM xb_metric_rollups
              WHERE grain = 'MINUTE' AND bucket_start >= $1",
        )
        .bind(start_from)
        .fetch_all(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("discover incremental hours: {err}")))?
    } else {
        sqlx::query(
            "SELECT DISTINCT date_trunc('hour', bucket_start) AS hr
               FROM xb_metric_rollups
              WHERE grain = 'MINUTE'",
        )
        .fetch_all(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("discover initial hours: {err}")))?
    };

    for r in new_hr_rows {
        target_hours.insert(r.get("hr"));
    }

    if target_hours.is_empty() {
        upsert_watermark_tx(tx, tenant_id, RollupGrain::Hour, completed_hour).await?;
        return Ok((0, target_hours));
    }

    let hr_vec: Vec<DateTime<Utc>> = target_hours.iter().copied().collect();
    let child_rows = sqlx::query(
        "SELECT node_id, metric, bucket_start, cnt, sum_v, min_v, max_v,
                last_v, last_text, last_ts, hll, freq,
                date_trunc('hour', bucket_start) AS parent_bucket
           FROM xb_metric_rollups
          WHERE grain = 'MINUTE'
            AND date_trunc('hour', bucket_start) = ANY($1)
          ORDER BY bucket_start ASC",
    )
    .bind(&hr_vec)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("load MINUTE rows for HOUR rollup: {err}")))?;

    let mut grouped: BTreeMap<(Uuid, String, DateTime<Utc>), Vec<RollupRow>> = BTreeMap::new();
    for r in child_rows {
        let parent: DateTime<Utc> = r.get("parent_bucket");
        let child = decode_rollup_row(&r, RollupGrain::Minute);
        grouped
            .entry((child.node_id, child.metric.clone(), parent))
            .or_default()
            .push(child);
    }

    let mut out = Vec::with_capacity(grouped.len());
    for ((_, _, hr_start), children) in grouped {
        if let Some(merged) = merge_rollup_slice(&children, RollupGrain::Hour, hr_start)? {
            out.push(merged);
        }
    }

    let written = upsert_rollup_rows_tx(tx, tenant_id, &out).await?;
    upsert_watermark_tx(tx, tenant_id, RollupGrain::Hour, completed_hour).await?;
    Ok((written, target_hours))
}

async fn rollup_days_stage_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    tz: &str,
    wm_day: Option<DateTime<Utc>>,
    touched_hours: &BTreeSet<DateTime<Utc>>,
    completed_day: DateTime<Utc>,
) -> Result<(u64, BTreeSet<DateTime<Utc>>), AppError> {
    let mut target_days: BTreeSet<DateTime<Utc>> = BTreeSet::new();

    if !touched_hours.is_empty() {
        let hr_vec: Vec<DateTime<Utc>> = touched_hours.iter().copied().collect();
        let rows = sqlx::query(
            "SELECT DISTINCT (date_trunc('day', h AT TIME ZONE $1) AT TIME ZONE $1) AS d
               FROM UNNEST($2::timestamptz[]) AS h",
        )
        .bind(tz)
        .bind(&hr_vec)
        .fetch_all(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("map touched hours to days: {err}")))?;
        for r in rows {
            target_days.insert(r.get("d"));
        }
    }

    let disc_rows = if let Some(done) = wm_day {
        sqlx::query(
            "SELECT DISTINCT (date_trunc('day', bucket_start AT TIME ZONE $1) AT TIME ZONE $1) AS d
               FROM xb_metric_rollups
              WHERE grain = 'HOUR'
                AND (date_trunc('day', bucket_start AT TIME ZONE $1) AT TIME ZONE $1) > $2",
        )
        .bind(tz)
        .bind(done)
        .fetch_all(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("discover incremental days: {err}")))?
    } else {
        sqlx::query(
            "SELECT DISTINCT (date_trunc('day', bucket_start AT TIME ZONE $1) AT TIME ZONE $1) AS d
               FROM xb_metric_rollups
              WHERE grain = 'HOUR'",
        )
        .bind(tz)
        .fetch_all(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("discover initial days: {err}")))?
    };

    for r in disc_rows {
        target_days.insert(r.get("d"));
    }

    if target_days.is_empty() {
        upsert_watermark_tx(tx, tenant_id, RollupGrain::Day, completed_day).await?;
        return Ok((0, target_days));
    }

    let day_vec: Vec<DateTime<Utc>> = target_days.iter().copied().collect();
    let child_rows = sqlx::query(
        "SELECT node_id, metric, bucket_start, cnt, sum_v, min_v, max_v,
                last_v, last_text, last_ts, hll, freq,
                (date_trunc('day', bucket_start AT TIME ZONE $1) AT TIME ZONE $1) AS parent_bucket
           FROM xb_metric_rollups
          WHERE grain = 'HOUR'
            AND (date_trunc('day', bucket_start AT TIME ZONE $1) AT TIME ZONE $1) = ANY($2)
          ORDER BY bucket_start ASC",
    )
    .bind(tz)
    .bind(&day_vec)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("load HOUR rows for DAY rollup: {err}")))?;

    let mut grouped: BTreeMap<(Uuid, String, DateTime<Utc>), Vec<RollupRow>> = BTreeMap::new();
    for r in child_rows {
        let parent: DateTime<Utc> = r.get("parent_bucket");
        let child = decode_rollup_row(&r, RollupGrain::Hour);
        grouped
            .entry((child.node_id, child.metric.clone(), parent))
            .or_default()
            .push(child);
    }

    let mut out = Vec::with_capacity(grouped.len());
    for ((_, _, day_start), children) in grouped {
        if let Some(merged) = merge_rollup_slice(&children, RollupGrain::Day, day_start)? {
            out.push(merged);
        }
    }

    let written = upsert_rollup_rows_tx(tx, tenant_id, &out).await?;
    upsert_watermark_tx(tx, tenant_id, RollupGrain::Day, completed_day).await?;
    Ok((written, target_days))
}

async fn rollup_months_stage_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    tz: &str,
    wm_month: Option<DateTime<Utc>>,
    touched_days: &BTreeSet<DateTime<Utc>>,
    completed_month: DateTime<Utc>,
) -> Result<u64, AppError> {
    let mut target_months: BTreeSet<DateTime<Utc>> = BTreeSet::new();

    if !touched_days.is_empty() {
        let day_vec: Vec<DateTime<Utc>> = touched_days.iter().copied().collect();
        let rows = sqlx::query(
            "SELECT DISTINCT (date_trunc('month', d AT TIME ZONE $1) AT TIME ZONE $1) AS m
               FROM UNNEST($2::timestamptz[]) AS d",
        )
        .bind(tz)
        .bind(&day_vec)
        .fetch_all(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("map touched days to months: {err}")))?;
        for r in rows {
            target_months.insert(r.get("m"));
        }
    }

    let disc_rows = if let Some(done) = wm_month {
        sqlx::query(
            "SELECT DISTINCT (date_trunc('month', bucket_start AT TIME ZONE $1) AT TIME ZONE $1) AS m
               FROM xb_metric_rollups
              WHERE grain = 'DAY'
                AND (date_trunc('month', bucket_start AT TIME ZONE $1) AT TIME ZONE $1) > $2",
        )
        .bind(tz)
        .bind(done)
        .fetch_all(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("discover incremental months: {err}")))?
    } else {
        sqlx::query(
            "SELECT DISTINCT (date_trunc('month', bucket_start AT TIME ZONE $1) AT TIME ZONE $1) AS m
               FROM xb_metric_rollups
              WHERE grain = 'DAY'",
        )
        .bind(tz)
        .fetch_all(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("discover initial months: {err}")))?
    };

    for r in disc_rows {
        target_months.insert(r.get("m"));
    }

    if target_months.is_empty() {
        upsert_watermark_tx(tx, tenant_id, RollupGrain::Month, completed_month).await?;
        return Ok(0);
    }

    let mon_vec: Vec<DateTime<Utc>> = target_months.into_iter().collect();
    let child_rows = sqlx::query(
        "SELECT node_id, metric, bucket_start, cnt, sum_v, min_v, max_v,
                last_v, last_text, last_ts, hll, freq,
                (date_trunc('month', bucket_start AT TIME ZONE $1) AT TIME ZONE $1) AS parent_bucket
           FROM xb_metric_rollups
          WHERE grain = 'DAY'
            AND (date_trunc('month', bucket_start AT TIME ZONE $1) AT TIME ZONE $1) = ANY($2)
          ORDER BY bucket_start ASC",
    )
    .bind(tz)
    .bind(&mon_vec)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("load DAY rows for MONTH rollup: {err}")))?;

    let mut grouped: BTreeMap<(Uuid, String, DateTime<Utc>), Vec<RollupRow>> = BTreeMap::new();
    for r in child_rows {
        let parent: DateTime<Utc> = r.get("parent_bucket");
        let child = decode_rollup_row(&r, RollupGrain::Day);
        grouped
            .entry((child.node_id, child.metric.clone(), parent))
            .or_default()
            .push(child);
    }

    let mut out = Vec::with_capacity(grouped.len());
    for ((_, _, mon_start), children) in grouped {
        if let Some(merged) = merge_rollup_slice(&children, RollupGrain::Month, mon_start)? {
            out.push(merged);
        }
    }

    let written = upsert_rollup_rows_tx(tx, tenant_id, &out).await?;
    upsert_watermark_tx(tx, tenant_id, RollupGrain::Month, completed_month).await?;
    Ok(written)
}

async fn reload_dirty_minutes_tx(
    tx: &mut Transaction<'_, Postgres>,
    dirty_minutes: &BTreeSet<(Uuid, String, DateTime<Utc>)>,
    grouped: &mut BTreeMap<(Uuid, String, DateTime<Utc>), Vec<RawPointSample>>,
) -> Result<(), AppError> {
    let d_nids: Vec<Uuid> = dirty_minutes.iter().map(|(n, _, _)| *n).collect();
    let d_metrics: Vec<String> = dirty_minutes.iter().map(|(_, m, _)| m.clone()).collect();
    let d_starts: Vec<DateTime<Utc>> = dirty_minutes.iter().map(|(_, _, s)| *s).collect();

    for key in dirty_minutes {
        grouped.remove(key);
    }

    let dirty_raw = sqlx::query(
        "SELECT s.node_id, s.metric, s.ts, s.value, s.value_text, d.minute_start
           FROM UNNEST($1::uuid[], $2::text[], $3::timestamptz[])
                AS d(node_id, metric, minute_start)
           JOIN xb_metric_series s
             ON s.node_id = d.node_id
            AND s.metric = d.metric
            AND s.ts >= d.minute_start
            AND s.ts < d.minute_start + INTERVAL '1 minute'
          ORDER BY s.ts ASC",
    )
    .bind(&d_nids)
    .bind(&d_metrics)
    .bind(&d_starts)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("reload dirty xb_metric_series: {err}")))?;

    for r in dirty_raw {
        let key = (
            r.get::<Uuid, _>("node_id"),
            r.get::<String, _>("metric"),
            r.get::<DateTime<Utc>, _>("minute_start"),
        );
        grouped.entry(key).or_default().push(RawPointSample {
            ts: r.get("ts"),
            value: r.get("value"),
            value_text: r.get("value_text"),
        });
    }

    for key in dirty_minutes {
        if !grouped.contains_key(key) {
            sqlx::query(
                "DELETE FROM xb_metric_rollups
                  WHERE node_id = $1 AND metric = $2 AND grain = 'MINUTE' AND bucket_start = $3",
            )
            .bind(key.0)
            .bind(&key.1)
            .bind(key.2)
            .execute(&mut **tx)
            .await
            .map_err(|err| AppError::Database(format!("delete empty dirty minute: {err}")))?;
        }
    }

    sqlx::query("DELETE FROM xb_rollup_dirty")
        .execute(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("clear xb_rollup_dirty failed: {err}")))?;
    Ok(())
}
