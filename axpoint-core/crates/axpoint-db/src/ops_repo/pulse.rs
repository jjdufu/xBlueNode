//! Step 32 single-tenant operational pulse aggregation (`GET /api/axops/pulse`, `02-STEP32` §32.1).

use std::str::FromStr;

use axpoint_domain::AppError;
use axpoint_domain::enums::{AlertStatus, DeadlineWindowStatus, RuntimeCertLevel, Severity};
use axpoint_domain::ops::{
    PlatformPulseResponse, PulseAlertItem, PulseApprovalItem, PulseHourlyBucket,
    PulseSimulationItem, PulseTotals, SubgraphPulseCard,
};
use chrono::{DateTime, Utc};
use sqlx::{Postgres, Row, Transaction};

use crate::PgStore;
use crate::tenant::begin_tenant_tx;

pub(super) async fn fetch_platform_pulse_impl(
    store: &PgStore,
    tenant_id: &str,
) -> Result<PlatformPulseResponse, AppError> {
    let mut tx = begin_tenant_tx(&store.pool, tenant_id).await?;

    let generated_at: DateTime<Utc> = sqlx::query_scalar("SELECT NOW()")
        .fetch_one(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("query pulse snapshot clock failed: {err}")))?;

    let totals = query_pulse_totals_tx(&mut tx, tenant_id, generated_at).await?;
    let hourly_series = query_hourly_series_tx(&mut tx, tenant_id, generated_at).await?;
    let subgraphs = query_subgraph_cards_tx(&mut tx, tenant_id).await?;
    let active_alerts = query_active_alerts_tx(&mut tx, tenant_id).await?;
    let pending_approvals = query_pending_approvals_tx(&mut tx, tenant_id).await?;
    let recent_simulations = query_recent_simulations_tx(&mut tx, tenant_id).await?;

    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit fetch_platform_pulse tx: {err}")))?;

    Ok(PlatformPulseResponse {
        generated_at,
        totals,
        hourly_series,
        subgraphs,
        active_alerts,
        pending_approvals,
        recent_simulations,
    })
}

async fn query_pulse_totals_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    now_utc: DateTime<Utc>,
) -> Result<PulseTotals, AppError> {
    let row = sqlx::query(
        "SELECT
             (SELECT COUNT(*)::BIGINT FROM xb_subgraphs
               WHERE tenant_id = $1 AND is_dev_sandbox = FALSE) AS subgraph_count,
             (SELECT COUNT(*)::BIGINT FROM xb_node_types nt
               LEFT JOIN xb_subgraphs sg
                 ON sg.tenant_id = nt.tenant_id AND sg.slug = nt.subgraph_slug
              WHERE nt.tenant_id = $1
                AND COALESCE(sg.is_dev_sandbox, FALSE) = FALSE) AS node_type_count,
             (SELECT COUNT(*)::BIGINT FROM xb_nodes n
               LEFT JOIN xb_subgraphs sg
                 ON sg.tenant_id = n.tenant_id AND sg.slug = n.subgraph_slug
              WHERE n.tenant_id = $1
                AND COALESCE(sg.is_dev_sandbox, FALSE) = FALSE) AS node_count,
             (SELECT COUNT(*)::BIGINT FROM xb_links l
               LEFT JOIN xb_subgraphs sg
                 ON sg.tenant_id = l.tenant_id AND sg.slug = l.subgraph_slug
              WHERE l.tenant_id = $1
                AND COALESCE(sg.is_dev_sandbox, FALSE) = FALSE) AS link_count,
             (SELECT COUNT(*)::BIGINT FROM ds_datasets
               WHERE tenant_id = $1) AS dataset_count,
             (SELECT COALESCE(SUM(row_count), 0)::BIGINT FROM ds_datasets
               WHERE tenant_id = $1) AS lake_total_rows,
             ((SELECT COUNT(*)::BIGINT FROM xb_metric_series
                WHERE tenant_id = $1 AND ts >= $2 - INTERVAL '24 hours')
              + (SELECT COUNT(*)::BIGINT FROM xb_link_metric_series
                  WHERE tenant_id = $1 AND ts >= $2 - INTERVAL '24 hours'))::BIGINT AS metric_points_24h,
             (SELECT COUNT(*)::BIGINT FROM ds_datasets
               WHERE tenant_id = $1
                 AND status <> 'DISABLED'
                 AND (continuous_stream = TRUE OR sync_mode = 'LOG_CDC')) AS active_stream_count,
             (SELECT COUNT(*)::BIGINT FROM ax_alerts
               WHERE tenant_id = $1 AND status IN ('OPEN', 'ACK')) AS open_alert_count,
             (SELECT COUNT(*)::BIGINT FROM ax_approvals
               WHERE tenant_id = $1 AND status = 'PENDING') AS pending_approval_count",
    )
    .bind(tenant_id)
    .bind(now_utc)
    .fetch_one(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("query pulse totals failed: {err}")))?;

    Ok(PulseTotals {
        subgraph_count: row.get("subgraph_count"),
        node_type_count: row.get("node_type_count"),
        node_count: row.get("node_count"),
        link_count: row.get("link_count"),
        dataset_count: row.get("dataset_count"),
        lake_total_rows: row.get("lake_total_rows"),
        metric_points_24h: row.get("metric_points_24h"),
        active_stream_count: row.get("active_stream_count"),
        open_alert_count: row.get("open_alert_count"),
        pending_approval_count: row.get("pending_approval_count"),
    })
}

async fn query_hourly_series_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    now_utc: DateTime<Utc>,
) -> Result<Vec<PulseHourlyBucket>, AppError> {
    let rows = sqlx::query(
        "WITH bounds AS (
             SELECT (date_trunc('hour', $2 AT TIME ZONE 'UTC') AT TIME ZONE 'UTC') AS cur_hour
         ),
         buckets AS (
             SELECT gs AS hour_utc
               FROM bounds,
                    generate_series(
                        bounds.cur_hour - INTERVAL '23 hours',
                        bounds.cur_hour,
                        INTERVAL '1 hour'
                    ) AS gs
         ),
         node_pts AS (
             SELECT (date_trunc('hour', ts AT TIME ZONE 'UTC') AT TIME ZONE 'UTC') AS hour_utc,
                    COUNT(*)::BIGINT AS cnt
               FROM xb_metric_series, bounds
              WHERE tenant_id = $1
                AND ts >= bounds.cur_hour - INTERVAL '23 hours'
                AND ts < bounds.cur_hour + INTERVAL '1 hour'
              GROUP BY 1
         ),
         link_pts AS (
             SELECT (date_trunc('hour', ts AT TIME ZONE 'UTC') AT TIME ZONE 'UTC') AS hour_utc,
                    COUNT(*)::BIGINT AS cnt
               FROM xb_link_metric_series, bounds
              WHERE tenant_id = $1
                AND ts >= bounds.cur_hour - INTERVAL '23 hours'
                AND ts < bounds.cur_hour + INTERVAL '1 hour'
              GROUP BY 1
         ),
         sync_rows AS (
             SELECT (date_trunc('hour', COALESCE(finished_at, created_at) AT TIME ZONE 'UTC') AT TIME ZONE 'UTC') AS hour_utc,
                    COALESCE(SUM(rows_written), 0)::BIGINT AS rows_sum
               FROM ds_sync_jobs, bounds
              WHERE tenant_id = $1
                AND status NOT IN ('FAILED', 'CANCELED', 'EXPIRED')
                AND COALESCE(finished_at, created_at) >= bounds.cur_hour - INTERVAL '23 hours'
                AND COALESCE(finished_at, created_at) < bounds.cur_hour + INTERVAL '1 hour'
              GROUP BY 1
         )
         SELECT b.hour_utc,
                (COALESCE(np.cnt, 0) + COALESCE(lp.cnt, 0))::BIGINT AS metric_points,
                COALESCE(sr.rows_sum, 0)::BIGINT AS synced_rows
           FROM buckets b
           LEFT JOIN node_pts np ON np.hour_utc = b.hour_utc
           LEFT JOIN link_pts lp ON lp.hour_utc = b.hour_utc
           LEFT JOIN sync_rows sr ON sr.hour_utc = b.hour_utc
          ORDER BY b.hour_utc ASC",
    )
    .bind(tenant_id)
    .bind(now_utc)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("query pulse hourly_series failed: {err}")))?;

    Ok(rows
        .into_iter()
        .map(|row| PulseHourlyBucket {
            hour_utc: row.get("hour_utc"),
            metric_points: row.get("metric_points"),
            synced_rows: row.get("synced_rows"),
        })
        .collect())
}

async fn query_subgraph_cards_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
) -> Result<Vec<SubgraphPulseCard>, AppError> {
    let rows = sqlx::query(SUBGRAPH_CARDS_SQL)
        .bind(tenant_id)
        .fetch_all(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("query pulse subgraphs failed: {err}")))?;

    Ok(rows.iter().map(decode_subgraph_card).collect())
}

const SUBGRAPH_CARDS_SQL: &str = "
SELECT
    sg.slug,
    sg.display_name,
    NULLIF(btrim(sg.description), '') AS description,
    sg.is_dev_sandbox,
    GREATEST(
        COALESCE(sh.current_version, 0),
        COALESCE((SELECT MAX(sv.version) FROM xb_schema_versions sv
                   WHERE sv.tenant_id = sg.tenant_id AND sv.subgraph_slug = sg.slug), 0)
    )::INT AS schema_version,
    (SELECT COUNT(*)::BIGINT FROM xb_node_types nt
      WHERE nt.tenant_id = sg.tenant_id AND nt.subgraph_slug = sg.slug) AS node_type_count,
    (SELECT COUNT(*)::BIGINT FROM xb_edge_types et
      WHERE et.tenant_id = sg.tenant_id AND et.subgraph_slug = sg.slug) AS edge_type_count,
    (SELECT COUNT(*)::BIGINT FROM xb_nodes n
      WHERE n.tenant_id = sg.tenant_id AND n.subgraph_slug = sg.slug) AS node_count,
    (SELECT COUNT(*)::BIGINT FROM xb_links l
      WHERE l.tenant_id = sg.tenant_id AND l.subgraph_slug = sg.slug) AS link_count,
    (SELECT COUNT(*)::BIGINT FROM ax_alerts a
       JOIN xb_nodes n ON n.id = a.node_id AND n.tenant_id = a.tenant_id
      WHERE a.tenant_id = sg.tenant_id
        AND a.status IN ('OPEN', 'ACK')
        AND n.subgraph_slug = sg.slug) AS alert_count,
    cert.cert_level,
    cert.readiness_score,
    ARRAY(
        SELECT DISTINCT btrim(nt.domain_group)
          FROM xb_node_types nt
         WHERE nt.tenant_id = sg.tenant_id
           AND nt.subgraph_slug = sg.slug
           AND nt.domain_group IS NOT NULL
           AND btrim(nt.domain_group) <> ''
         ORDER BY 1
    ) AS business_domains,
    (
        SELECT d.slug
          FROM xv_dashboards d
         WHERE d.tenant_id = sg.tenant_id
           AND (
               d.slug = sg.slug
               OR d.slug = replace(sg.slug, '_', '-')
               OR d.slug LIKE sg.slug || '-%'
               OR d.slug LIKE replace(sg.slug, '_', '-') || '-%'
               OR EXISTS (
                   SELECT 1
                     FROM xv_widgets w
                     JOIN xb_node_types nt
                       ON nt.tenant_id = d.tenant_id AND nt.subgraph_slug = sg.slug
                    WHERE w.dashboard_id = d.id
                      AND w.tenant_id = d.tenant_id
                      AND (
                          (jsonb_typeof(w.deps->'type_names') = 'array' AND (w.deps->'type_names') ? nt.name)
                          OR w.data_source->>'type_name' = nt.name
                          OR (jsonb_typeof(w.data_source->'type_names') = 'array' AND (w.data_source->'type_names') ? nt.name)
                      )
               )
           )
         ORDER BY CASE WHEN d.status = 'PUBLISHED' THEN 0 ELSE 1 END ASC,
                  d.updated_at DESC,
                  d.created_at DESC
         LIMIT 1
    ) AS primary_dashboard_slug,
    (
        SELECT sb.id
          FROM ax_sandboxes sb
         WHERE sb.tenant_id = sg.tenant_id
           AND sb.status <> 'ARCHIVED'
           AND (
               sb.scope->>'subgraph_slug' = sg.slug
               OR EXISTS (
                   SELECT 1
                     FROM ax_sim_base_nodes bn
                     JOIN xb_nodes n ON n.id = bn.node_id AND n.tenant_id = sb.tenant_id
                    WHERE bn.sandbox_id = sb.id
                      AND bn.tenant_id = sb.tenant_id
                      AND n.subgraph_slug = sg.slug
               )
               OR (
                   jsonb_typeof(sb.scope->'type_names') = 'array'
                   AND EXISTS (
                       SELECT 1
                         FROM jsonb_array_elements_text(sb.scope->'type_names') AS tn(type_name)
                         JOIN xb_node_types nt
                           ON nt.name = tn.type_name AND nt.tenant_id = sb.tenant_id
                        WHERE nt.subgraph_slug = sg.slug
                   )
               )
               OR sb.name ILIKE '%' || sg.slug || '%'
               OR (char_length(sg.display_name) >= 2 AND sb.name ILIKE '%' || sg.display_name || '%')
           )
         ORDER BY sb.created_at DESC
         LIMIT 1
    ) AS latest_sandbox_id
  FROM xb_subgraphs sg
  LEFT JOIN xb_schema_heads sh
    ON sh.tenant_id = sg.tenant_id AND sh.subgraph_slug = sg.slug
  LEFT JOIN LATERAL (
      SELECT rc.cert_level, rc.readiness_score
        FROM xb_runtime_certifications rc
       WHERE rc.tenant_id = sg.tenant_id
         AND rc.subgraph_slug = sg.slug
       ORDER BY rc.created_at DESC
       LIMIT 1
  ) cert ON TRUE
 WHERE sg.tenant_id = $1
 ORDER BY sg.is_dev_sandbox ASC, node_count DESC, sg.slug ASC
";

fn decode_subgraph_card(row: &sqlx::postgres::PgRow) -> SubgraphPulseCard {
    let cert_raw: Option<String> = row.get("cert_level");
    let cert_level = cert_raw
        .as_deref()
        .and_then(|s| RuntimeCertLevel::from_str(s).ok());

    SubgraphPulseCard {
        slug: row.get("slug"),
        display_name: row.get("display_name"),
        description: row.get("description"),
        is_dev_sandbox: row.get("is_dev_sandbox"),
        schema_version: row.get("schema_version"),
        node_type_count: row.get("node_type_count"),
        edge_type_count: row.get("edge_type_count"),
        node_count: row.get("node_count"),
        link_count: row.get("link_count"),
        alert_count: row.get("alert_count"),
        cert_level,
        readiness_score: row.get("readiness_score"),
        business_domains: row.get("business_domains"),
        primary_dashboard_slug: row.get("primary_dashboard_slug"),
        latest_sandbox_id: row.get("latest_sandbox_id"),
    }
}

async fn query_active_alerts_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
) -> Result<Vec<PulseAlertItem>, AppError> {
    let rows = sqlx::query(
        "SELECT a.id, a.severity, a.status, a.title, a.node_id,
                n.subgraph_slug, a.last_value, a.occurrences, a.last_seen_at
           FROM ax_alerts a
           LEFT JOIN xb_nodes n
             ON n.id = a.node_id AND n.tenant_id = a.tenant_id
          WHERE a.tenant_id = $1
            AND a.status IN ('OPEN', 'ACK')
          ORDER BY a.last_seen_at DESC, a.id DESC
          LIMIT 10",
    )
    .bind(tenant_id)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("query pulse active_alerts failed: {err}")))?;

    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let sev_str: String = row.get("severity");
        let status_str: String = row.get("status");
        let severity = Severity::from_str(&sev_str)?;
        let status = AlertStatus::from_str(&status_str)?;
        out.push(PulseAlertItem {
            id: row.get("id"),
            severity,
            status,
            title: row.get("title"),
            node_id: row.get("node_id"),
            subgraph_slug: row.get("subgraph_slug"),
            last_value: row.get("last_value"),
            occurrences: row.get("occurrences"),
            last_seen_at: row.get("last_seen_at"),
        });
    }
    Ok(out)
}

async fn query_pending_approvals_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
) -> Result<Vec<PulseApprovalItem>, AppError> {
    let rows = sqlx::query(
        "SELECT id, source_kind, source_ref, title, run_id, created_at, expires_at
           FROM ax_approvals
          WHERE tenant_id = $1
            AND status = 'PENDING'
          ORDER BY created_at DESC, id DESC
          LIMIT 10",
    )
    .bind(tenant_id)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("query pulse pending_approvals failed: {err}")))?;

    Ok(rows
        .into_iter()
        .map(|row| PulseApprovalItem {
            id: row.get("id"),
            source_kind: row.get("source_kind"),
            source_ref: row.get("source_ref"),
            title: row.get("title"),
            run_id: row.get("run_id"),
            created_at: row.get("created_at"),
            expires_at: row.get("expires_at"),
        })
        .collect())
}

async fn query_recent_simulations_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
) -> Result<Vec<PulseSimulationItem>, AppError> {
    let rows = sqlx::query(RECENT_SIMULATIONS_SQL)
        .bind(tenant_id)
        .fetch_all(&mut **tx)
        .await
        .map_err(|err| {
            AppError::Database(format!("query pulse recent_simulations failed: {err}"))
        })?;

    Ok(rows.iter().map(decode_simulation_item).collect())
}

const RECENT_SIMULATIONS_SQL: &str = "
SELECT
    sc.id AS scenario_id,
    sc.sandbox_id,
    sb.name AS sandbox_name,
    COALESCE(
        NULLIF(btrim(sb.scope->>'subgraph_slug'), ''),
        (SELECT n.subgraph_slug
           FROM ax_sim_base_nodes bn
           JOIN xb_nodes n ON n.id = bn.node_id AND n.tenant_id = sb.tenant_id
          WHERE bn.sandbox_id = sb.id AND bn.tenant_id = sb.tenant_id
          LIMIT 1),
        (SELECT nt.subgraph_slug
           FROM jsonb_array_elements_text(
               CASE WHEN jsonb_typeof(sb.scope->'type_names') = 'array'
                    THEN sb.scope->'type_names'
                    ELSE '[]'::jsonb
               END
           ) AS tn(type_name)
           JOIN xb_node_types nt ON nt.name = tn.type_name AND nt.tenant_id = sb.tenant_id
          LIMIT 1),
        (SELECT sg.slug
           FROM xb_subgraphs sg
          WHERE sg.tenant_id = sb.tenant_id
            AND (sb.name ILIKE '%' || sg.slug || '%'
                 OR (char_length(sg.display_name) >= 2 AND sb.name ILIKE '%' || sg.display_name || '%'))
          ORDER BY sg.is_main ASC, char_length(sg.slug) DESC
          LIMIT 1),
        'main'
    ) AS subgraph_slug,
    sc.name,
    sc.status,
    sc.result,
    sc.deadline_report,
    (SELECT COUNT(*)::BIGINT FROM ax_sim_impacts imp
      WHERE imp.scenario_id = sc.id AND imp.tenant_id = sc.tenant_id) AS impact_row_count,
    sc.created_at
  FROM ax_sim_scenarios sc
  JOIN ax_sandboxes sb
    ON sb.id = sc.sandbox_id AND sb.tenant_id = sc.tenant_id
 WHERE sc.tenant_id = $1
 ORDER BY sc.created_at DESC, sc.id DESC
 LIMIT 8
";

fn decode_simulation_item(row: &sqlx::postgres::PgRow) -> PulseSimulationItem {
    let result_json: Option<serde_json::Value> = row.get("result");
    let deadline_col: Option<serde_json::Value> = row.get("deadline_report");
    let impact_row_count: i64 = row.get("impact_row_count");

    let affected_count = result_json
        .as_ref()
        .and_then(|v| v.get("affected_count"))
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(impact_row_count);

    let report_val = deadline_col.as_ref().filter(|v| !v.is_null()).or_else(|| {
        result_json
            .as_ref()
            .and_then(|v| v.get("deadline_report"))
            .filter(|v| !v.is_null())
    });

    let (earliest_breach_step, earliest_deadline_step, deadline_status) =
        extract_deadline_fields(report_val);

    PulseSimulationItem {
        scenario_id: row.get("scenario_id"),
        sandbox_id: row.get("sandbox_id"),
        sandbox_name: row.get("sandbox_name"),
        subgraph_slug: row.get("subgraph_slug"),
        name: row.get("name"),
        status: row.get("status"),
        affected_count,
        earliest_breach_step,
        earliest_deadline_step,
        deadline_status,
        created_at: row.get("created_at"),
    }
}

fn extract_deadline_fields(
    report_val: Option<&serde_json::Value>,
) -> (Option<u32>, Option<i32>, Option<DeadlineWindowStatus>) {
    let Some(report) = report_val else {
        return (None, None, None);
    };
    let breach = report
        .get("earliest_breach_step")
        .and_then(serde_json::Value::as_u64)
        .and_then(|n| u32::try_from(n).ok());
    let deadline = report
        .get("earliest_deadline_step")
        .and_then(serde_json::Value::as_i64)
        .and_then(|n| i32::try_from(n).ok());
    let status = report
        .get("status")
        .and_then(serde_json::Value::as_str)
        .and_then(|s| DeadlineWindowStatus::from_str(s).ok());
    (breach, deadline, status)
}
