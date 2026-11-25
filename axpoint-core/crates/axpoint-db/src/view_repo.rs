//! `PostgreSQL` persistence for `xBlueView` dashboards, widgets, visual assets,
//! 2D/3D spatial twin bindings, GIN dependency queries, and atomic node writebacks (`10-STEP11-12` §11.1–§11.6).

use std::collections::{BTreeMap, HashSet};
use std::str::FromStr;

use axpoint_domain::AppError;
use axpoint_domain::axhub::PaginatedList;
use axpoint_domain::enums::{
    AssetKind, DataType, ExportFormat, JobKind, JobStatus, MetricSourceTag, OutboxTopic,
    SemanticZoneKind, Sensitivity, WidgetKind,
};
use axpoint_domain::export::{
    CompleteExportJobParams, CreateExportJobParams, CreateExportTemplateParams,
    CreateShareLinkParams, ExportJobRecord, ExportListQuery, ExportOptions, ExportSource,
    ExportTemplateRecord, ResolvedShare, ShareLinkRecord, ShareListQuery, ShareTargetKind,
};
use axpoint_domain::flow::{ActionKind, ActionLogRecord, ActionLogStatus, ActionSourceKind};
use axpoint_domain::platform::{NewJob, NewOutboxEvent};
use axpoint_domain::ports::ViewRepository;
use axpoint_domain::rules::SimplePageQuery;
use axpoint_domain::studio::{PropertyKind, PropertySpec, SchemaDoc};
use axpoint_domain::view::{
    AffectedWidgetRef, AssetRecord, CanvasConfig, CreateAssetParams, CreateDashboardRequest,
    CreateWidgetRecordParams, DashboardDetail, DashboardRecord, DashboardStatus, DashboardTheme,
    DrillLevel, FilterDef, FloorPlacementRecord, FloorPlanRow, LayoutUpdateItem, NodesGeoLinkRow,
    NodesGeoRow, PinChartToDashboardRequest, PinChartToDashboardResponse, PinnedSlotRequest,
    PutFloorPlacementItem, PutScene3dBindingItem, RefreshMode, SaveChartRequest, SavedChartRecord,
    Scene3dBindingRecord, Scene3dRow, UpdateDashboardRequest, UpdateSavedChartRequest,
    UpdateWidgetRecordParams, ViewWriteNodeOutcome, ViewWriteNodeParams, WidgetDataSource,
    WidgetDeps, WidgetPosition, WidgetRecord, WriteNodeTargetKind, allocate_pinned_widget_slot,
};
use chrono::{DateTime, Utc};
use sqlx::{Postgres, QueryBuilder, Row, Transaction};
use uuid::Uuid;

use crate::PgStore;
use crate::outbox::insert_outbox_tx;
use crate::tenant::begin_tenant_tx;

fn is_unique_violation(err: &sqlx::Error) -> bool {
    err.as_database_error()
        .and_then(sqlx::error::DatabaseError::code)
        .is_some_and(|code| code == "23505")
}

fn validate_slug(slug: &str) -> Result<(), AppError> {
    let bytes = slug.as_bytes();
    let Some(&first) = bytes.first() else {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "slug must match ^[a-z0-9][a-z0-9-]{0,62}$",
        ));
    };
    if bytes.len() > 63 || !(first.is_ascii_lowercase() || first.is_ascii_digit()) {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "slug must match ^[a-z0-9][a-z0-9-]{0,62}$",
        ));
    }
    if !bytes
        .iter()
        .all(|&b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "slug must match ^[a-z0-9][a-z0-9-]{0,62}$",
        ));
    }
    Ok(())
}

fn build_duplicate_slug(source_slug: &str) -> String {
    let suffix_full = Uuid::new_v4().simple().to_string();
    let suffix: String = suffix_full.chars().take(8).collect();
    let prefix: String = source_slug.chars().take(54).collect();
    let trimmed = prefix.trim_end_matches('-');
    if trimmed.is_empty() {
        format!("copy-{suffix}")
    } else {
        format!("{trimmed}-{suffix}")
    }
}

fn validate_page_limit(
    raw_limit: Option<u32>,
    default_limit: u32,
) -> Result<(usize, i64), AppError> {
    let limit = raw_limit.unwrap_or(default_limit);
    if !(1..=500).contains(&limit) {
        return Err(AppError::bad(
            "LIMIT_OUT_OF_RANGE",
            format!("limit must be in 1..=500, got {limit}"),
        ));
    }
    let limit_usize = usize::try_from(limit).unwrap_or(50);
    let fetch_limit = i64::from(limit).saturating_add(1);
    Ok((limit_usize, fetch_limit))
}

fn encode_ts_uuid_cursor(ts: DateTime<Utc>, id: Uuid) -> String {
    format!("{}|{id}", ts.to_rfc3339())
}

fn parse_ts_uuid_cursor(raw_cursor: &str) -> Result<(DateTime<Utc>, Uuid), AppError> {
    let Some((ts_part, id_part)) = raw_cursor.split_once('|') else {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "cursor must have format <rfc3339>|<uuid>",
        ));
    };
    let parsed_ts = DateTime::parse_from_rfc3339(ts_part)
        .map_err(|err| AppError::bad("INVALID_ARGUMENT", format!("invalid cursor ts: {err}")))?
        .with_timezone(&Utc);
    let parsed_id = Uuid::parse_str(id_part)
        .map_err(|err| AppError::bad("INVALID_ARGUMENT", format!("invalid cursor id: {err}")))?;
    Ok((parsed_ts, parsed_id))
}

fn validate_widget_position(pos: &WidgetPosition) -> Result<(), AppError> {
    if pos.x < 0 || pos.y < 0 || pos.w < 1 || pos.h < 1 || pos.x.saturating_add(pos.w) > 24 {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            format!(
                "widget position must satisfy x>=0, y>=0, w>=1, h>=1, x+w<=24, got ({}, {}, {}, {})",
                pos.x, pos.y, pos.w, pos.h
            ),
        ));
    }
    Ok(())
}

fn validate_canvas_config(canvas: &CanvasConfig) -> Result<(), AppError> {
    if canvas.cols <= 0
        || canvas.row_height <= 0
        || canvas.screen_width <= 0
        || canvas.screen_height <= 0
    {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            format!(
                "canvas cols, row_height, screen_width, and screen_height must be > 0, got ({}, {}, {}, {})",
                canvas.cols, canvas.row_height, canvas.screen_width, canvas.screen_height
            ),
        ));
    }
    Ok(())
}

fn validate_refresh_seconds(seconds: i32) -> Result<(), AppError> {
    if !(5..=86_400).contains(&seconds) {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            format!("refresh_seconds must be in 5..=86400, got {seconds}"),
        ));
    }
    Ok(())
}

fn decode_dashboard_row(row: &sqlx::postgres::PgRow) -> Result<DashboardRecord, AppError> {
    let filters: Vec<FilterDef> = serde_json::from_value(row.get("filters"))
        .map_err(|err| AppError::Database(format!("decode xv_dashboards.filters failed: {err}")))?;
    let canvas: CanvasConfig = serde_json::from_value(row.get("canvas"))
        .map_err(|err| AppError::Database(format!("decode xv_dashboards.canvas failed: {err}")))?;
    let theme_raw: String = row.get("theme");
    let status_raw: String = row.get("status");

    Ok(DashboardRecord {
        id: row.get("id"),
        tenant_id: row.get("tenant_id"),
        name: row.get("name"),
        slug: row.get("slug"),
        description: row.get("description"),
        filters,
        theme: DashboardTheme::from_db_str(&theme_raw),
        canvas,
        status: DashboardStatus::from_db_str(&status_raw),
        version: row.get("version"),
        created_by: row.get("created_by"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

fn decode_widget_row(row: &sqlx::postgres::PgRow) -> Result<WidgetRecord, AppError> {
    let kind_raw: String = row.get("widget_kind");
    let widget_kind = WidgetKind::from_str(&kind_raw)?;
    let position: WidgetPosition = serde_json::from_value(row.get("position"))
        .map_err(|err| AppError::Database(format!("decode xv_widgets.position failed: {err}")))?;
    let data_source: WidgetDataSource =
        serde_json::from_value(row.get("data_source")).map_err(|err| {
            AppError::Database(format!("decode xv_widgets.data_source failed: {err}"))
        })?;
    let drill: Vec<DrillLevel> = serde_json::from_value(row.get("drill"))
        .map_err(|err| AppError::Database(format!("decode xv_widgets.drill failed: {err}")))?;
    let deps: WidgetDeps = serde_json::from_value(row.get("deps"))
        .map_err(|err| AppError::Database(format!("decode xv_widgets.deps failed: {err}")))?;
    let refresh_raw: String = row.get("refresh_mode");

    Ok(WidgetRecord {
        id: row.get("id"),
        tenant_id: row.get("tenant_id"),
        dashboard_id: row.get("dashboard_id"),
        widget_kind,
        title: row.get("title"),
        position,
        data_source,
        encoding: row.get("encoding"),
        options: row.get("options"),
        drill,
        refresh_mode: RefreshMode::from_db_str(&refresh_raw),
        refresh_seconds: row.get("refresh_seconds"),
        deps,
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

fn decode_asset_row(row: &sqlx::postgres::PgRow) -> Result<AssetRecord, AppError> {
    let kind_raw: String = row.get("asset_kind");
    let asset_kind = AssetKind::from_str(&kind_raw)?;
    Ok(AssetRecord {
        id: row.get("id"),
        tenant_id: row.get("tenant_id"),
        asset_kind,
        name: row.get("name"),
        file_path: row.get("file_path"),
        mime_type: row.get("mime_type"),
        sha256: row.get("sha256"),
        width_px: row.get("width_px"),
        height_px: row.get("height_px"),
        created_at: row.get("created_at"),
    })
}

fn decode_floor_placement_row(row: &sqlx::postgres::PgRow) -> FloorPlacementRecord {
    FloorPlacementRecord {
        asset_id: row.get("asset_id"),
        node_id: row.get("node_id"),
        x_ratio: row.get("x_ratio"),
        y_ratio: row.get("y_ratio"),
    }
}

fn decode_scene3d_binding_row(row: &sqlx::postgres::PgRow) -> Scene3dBindingRecord {
    Scene3dBindingRecord {
        asset_id: row.get("asset_id"),
        node_id: row.get("node_id"),
        mesh_name: row.get("mesh_name"),
        pos_x: row.get("pos_x"),
        pos_y: row.get("pos_y"),
        pos_z: row.get("pos_z"),
        scale: row.get("scale"),
    }
}

fn parse_action_source_kind(raw: &str) -> Result<ActionSourceKind, AppError> {
    match raw {
        "FLOW" => Ok(ActionSourceKind::Flow),
        "SIM_APPLY" => Ok(ActionSourceKind::SimApply),
        "AGENT" => Ok(ActionSourceKind::Agent),
        "RULE" => Ok(ActionSourceKind::Rule),
        "VIEW" => Ok(ActionSourceKind::View),
        "OBJECT_ACTION" => Ok(ActionSourceKind::ObjectAction),
        _ => Err(AppError::Database(format!(
            "unknown ax_action_log.source_kind: {raw}"
        ))),
    }
}

fn parse_action_kind(raw: &str) -> Result<ActionKind, AppError> {
    match raw {
        "WRITE_NODE_METRIC" => Ok(ActionKind::WriteNodeMetric),
        "WRITE_NODE_PROPERTY" => Ok(ActionKind::WriteNodeProperty),
        "HTTP" => Ok(ActionKind::Http),
        "NOTIFY" => Ok(ActionKind::Notify),
        "WORK_ITEM" => Ok(ActionKind::WorkItem),
        "OBJECT_ACTION" => Ok(ActionKind::ObjectAction),
        _ => Err(AppError::Database(format!(
            "unknown ax_action_log.action_kind: {raw}"
        ))),
    }
}

fn parse_action_log_status(raw: &str) -> Result<ActionLogStatus, AppError> {
    match raw {
        "DONE" => Ok(ActionLogStatus::Done),
        "DRY_RUN" => Ok(ActionLogStatus::DryRun),
        "FAILED" => Ok(ActionLogStatus::Failed),
        _ => Err(AppError::Database(format!(
            "unknown ax_action_log.status: {raw}"
        ))),
    }
}

fn decode_action_log_row(row: &sqlx::postgres::PgRow) -> Result<ActionLogRecord, AppError> {
    let source_raw: String = row.get("source_kind");
    let kind_raw: String = row.get("action_kind");
    let status_raw: String = row.get("status");
    Ok(ActionLogRecord {
        id: row.get("id"),
        run_id: row.get("run_id"),
        node_id: row.get("node_id"),
        source_kind: parse_action_source_kind(&source_raw)?,
        action_kind: parse_action_kind(&kind_raw)?,
        target: row.get("target"),
        payload: row.get("payload"),
        status: parse_action_log_status(&status_raw)?,
        idempotency_key: row.get("idempotency_key"),
        error_message: row.get("error_message"),
        created_at: row.get("created_at"),
    })
}

async fn ensure_dashboard_exists_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    dashboard_id: Uuid,
) -> Result<(), AppError> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM xv_dashboards WHERE tenant_id = $1 AND id = $2)",
    )
    .bind(tenant_id)
    .bind(dashboard_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("check xv_dashboards exists failed: {err}")))?;
    if !exists {
        return Err(AppError::NotFound("DASHBOARD_NOT_FOUND"));
    }
    Ok(())
}

async fn ensure_asset_exists_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    asset_id: Uuid,
) -> Result<(), AppError> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM xv_assets WHERE tenant_id = $1 AND id = $2)",
    )
    .bind(tenant_id)
    .bind(asset_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("check xv_assets exists failed: {err}")))?;
    if !exists {
        return Err(AppError::NotFound("ASSET_NOT_FOUND"));
    }
    Ok(())
}

async fn ensure_asset_kind_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    asset_id: Uuid,
    expected: AssetKind,
) -> Result<(), AppError> {
    let kind_raw: Option<String> =
        sqlx::query_scalar("SELECT asset_kind FROM xv_assets WHERE tenant_id = $1 AND id = $2")
            .bind(tenant_id)
            .bind(asset_id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(|err| AppError::Database(format!("check xv_assets kind failed: {err}")))?;

    let raw = kind_raw.ok_or(AppError::NotFound("ASSET_NOT_FOUND"))?;
    let actual = AssetKind::from_str(&raw)?;
    if actual != expected {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            format!(
                "asset {asset_id} has kind {}, expected {}",
                actual.as_str(),
                expected.as_str()
            ),
        ));
    }
    Ok(())
}

async fn ensure_nodes_exist_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    node_ids: &[Uuid],
) -> Result<(), AppError> {
    if node_ids.is_empty() {
        return Ok(());
    }
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM xb_nodes WHERE tenant_id = $1 AND id = ANY($2::uuid[])",
    )
    .bind(tenant_id)
    .bind(node_ids)
    .fetch_one(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("check xb_nodes exists failed: {err}")))?;
    let expected = i64::try_from(node_ids.len()).unwrap_or(i64::MAX);
    if count != expected {
        return Err(AppError::NotFound("NODE_NOT_FOUND"));
    }
    Ok(())
}

async fn fetch_dashboard_widgets_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    dashboard_id: Uuid,
) -> Result<Vec<WidgetRecord>, AppError> {
    let rows = sqlx::query(
        "SELECT id, tenant_id, dashboard_id, widget_kind, title, position, data_source,
                encoding, options, drill, refresh_mode, refresh_seconds, deps, created_at, updated_at
           FROM xv_widgets
          WHERE tenant_id = $1 AND dashboard_id = $2
          ORDER BY COALESCE((position->>'y')::int, 0) ASC,
                   COALESCE((position->>'x')::int, 0) ASC,
                   created_at ASC,
                   id ASC",
    )
    .bind(tenant_id)
    .bind(dashboard_id)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("list xv_widgets failed: {err}")))?;

    rows.iter().map(decode_widget_row).collect()
}

async fn insert_widget_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    widget_id: Option<Uuid>,
    params: &CreateWidgetRecordParams,
) -> Result<WidgetRecord, AppError> {
    validate_widget_position(&params.position)?;
    validate_refresh_seconds(params.refresh_seconds)?;

    let position_json = serde_json::to_value(&params.position)
        .map_err(|err| AppError::bad("INVALID_ARGUMENT", format!("serialize position: {err}")))?;
    let ds_json = serde_json::to_value(&params.data_source).map_err(|err| {
        AppError::bad("INVALID_ARGUMENT", format!("serialize data_source: {err}"))
    })?;
    let drill_json = serde_json::to_value(&params.drill)
        .map_err(|err| AppError::bad("INVALID_ARGUMENT", format!("serialize drill: {err}")))?;
    let deps_json = serde_json::to_value(&params.deps)
        .map_err(|err| AppError::bad("INVALID_ARGUMENT", format!("serialize deps: {err}")))?;

    let id = widget_id.unwrap_or_else(Uuid::new_v4);
    let row = sqlx::query(
        "INSERT INTO xv_widgets (
             id, tenant_id, dashboard_id, widget_kind, title, position, data_source,
             encoding, options, drill, refresh_mode, refresh_seconds, deps
         )
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
         RETURNING id, tenant_id, dashboard_id, widget_kind, title, position, data_source,
                   encoding, options, drill, refresh_mode, refresh_seconds, deps, created_at, updated_at",
    )
    .bind(id)
    .bind(tenant_id)
    .bind(params.dashboard_id)
    .bind(params.widget_kind.as_str())
    .bind(&params.title)
    .bind(position_json)
    .bind(ds_json)
    .bind(&params.encoding)
    .bind(&params.options)
    .bind(drill_json)
    .bind(params.refresh_mode.as_str())
    .bind(params.refresh_seconds)
    .bind(deps_json)
    .fetch_one(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("insert xv_widgets failed: {err}")))?;

    decode_widget_row(&row)
}

fn collect_dep_probes(filter_deps: &WidgetDeps) -> Vec<serde_json::Value> {
    let mut probes = Vec::new();
    for ds_id in &filter_deps.dataset_ids {
        probes.push(serde_json::json!({ "dataset_ids": [ds_id] }));
    }
    for tn in &filter_deps.type_names {
        let trimmed = tn.trim();
        if !trimmed.is_empty() {
            probes.push(serde_json::json!({ "type_names": [trimmed] }));
        }
    }
    for metric in &filter_deps.metrics {
        let trimmed = metric.trim();
        if !trimmed.is_empty() {
            probes.push(serde_json::json!({ "metrics": [trimmed] }));
        }
    }
    if filter_deps.alerts {
        probes.push(serde_json::json!({ "alerts": true }));
    }
    for sc_id in &filter_deps.scenario_ids {
        probes.push(serde_json::json!({ "scenario_ids": [sc_id] }));
    }
    probes
}

fn normalize_type_names(type_names: &[String]) -> Option<Vec<String>> {
    let cleaned: Vec<String> = type_names
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect();
    (!cleaned.is_empty()).then_some(cleaned)
}

fn normalize_color_metric(color_metric: Option<&str>) -> Option<&str> {
    color_metric.map(str::trim).filter(|s| !s.is_empty())
}

fn validate_write_property_value(
    prop: &PropertySpec,
    value: &serde_json::Value,
) -> Result<(), AppError> {
    if value.is_null() {
        if prop.required {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                format!("required property '{}' cannot be null", prop.name),
            ));
        }
        return Ok(());
    }

    let valid = match prop.data_type {
        DataType::String => {
            let Some(s) = value.as_str() else {
                return Err(AppError::bad(
                    "INVALID_ARGUMENT",
                    format!("property '{}' expects STRING", prop.name),
                ));
            };
            if let Some(ref allowed) = prop.allowed_values
                && !allowed.is_empty()
                && !allowed.iter().any(|item| item == s)
            {
                return Err(AppError::bad(
                    "INVALID_ARGUMENT",
                    format!(
                        "property '{}' value '{s}' is not in allowed_values",
                        prop.name
                    ),
                ));
            }
            true
        }
        DataType::Date | DataType::Timestamp | DataType::Binary => value.is_string(),
        DataType::Int64 => value.is_i64() || value.is_u64(),
        DataType::Float64 | DataType::Decimal => value.as_f64().is_some_and(f64::is_finite),
        DataType::Boolean => value.is_boolean(),
        DataType::List => value.is_array(),
        DataType::Struct => value.is_object(),
        DataType::Json => true,
    };

    if !valid {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            format!(
                "property '{}' expects data_type {}",
                prop.name,
                prop.data_type.as_str()
            ),
        ));
    }
    Ok(())
}

fn validate_write_metric_value(
    prop: &PropertySpec,
    value: &serde_json::Value,
    numeric_value: Option<f64>,
) -> Result<f64, AppError> {
    let val = numeric_value.or_else(|| value.as_f64()).ok_or_else(|| {
        AppError::bad(
            "INVALID_ARGUMENT",
            format!("metric '{}' requires a numeric value", prop.name),
        )
    })?;
    if !val.is_finite() {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            format!("metric '{}' value must be finite", prop.name),
        ));
    }

    match prop.data_type {
        DataType::Int64 => {
            if !(value.is_null() || value.is_i64() || value.is_u64() || val.fract() == 0.0) {
                return Err(AppError::bad(
                    "INVALID_ARGUMENT",
                    format!("metric '{}' expects INT64", prop.name),
                ));
            }
        }
        DataType::Float64 | DataType::Decimal => {}
        other => {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                format!(
                    "metric '{}' has non-numeric schema type {}",
                    prop.name,
                    other.as_str()
                ),
            ));
        }
    }

    Ok(val)
}

async fn validate_write_against_schema_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    type_name: &str,
    params: &ViewWriteNodeParams,
) -> Result<Option<f64>, AppError> {
    let schema_val: Option<serde_json::Value> = sqlx::query_scalar(
        "SELECT schema_doc
           FROM xb_schema_versions
          WHERE tenant_id = $1 AND subgraph_slug = 'main'
          ORDER BY version DESC
          LIMIT 1",
    )
    .bind(tenant_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("load xb_schema_versions failed: {err}")))?;

    let raw_doc = schema_val.ok_or_else(|| {
        AppError::bad(
            "INVALID_ARGUMENT",
            format!("no published main schema found for node type '{type_name}'"),
        )
    })?;
    let doc: SchemaDoc = serde_json::from_value(raw_doc).map_err(|err| {
        AppError::Database(format!(
            "decode xb_schema_versions.schema_doc failed: {err}"
        ))
    })?;

    let node_spec = doc
        .node_types
        .iter()
        .find(|nt| nt.name == type_name)
        .ok_or_else(|| {
            AppError::bad(
                "INVALID_ARGUMENT",
                format!("node type '{type_name}' is not declared in published main schema"),
            )
        })?;

    let key = params.key.trim();
    let prop = node_spec
        .properties
        .iter()
        .find(|p| p.name == key)
        .ok_or_else(|| {
            AppError::bad(
                "INVALID_ARGUMENT",
                format!("field '{key}' is not declared on node type '{type_name}'"),
            )
        })?;

    match params.target_kind {
        WriteNodeTargetKind::Property => {
            if prop.kind != PropertyKind::Attribute {
                return Err(AppError::bad(
                    "INVALID_ARGUMENT",
                    format!("field '{key}' on '{type_name}' is a METRIC, not a PROPERTY"),
                ));
            }
            validate_write_property_value(prop, &params.value)?;
            Ok(None)
        }
        WriteNodeTargetKind::Metric => {
            if prop.kind != PropertyKind::Metric {
                return Err(AppError::bad(
                    "INVALID_ARGUMENT",
                    format!("field '{key}' on '{type_name}' is an ATTRIBUTE, not a METRIC"),
                ));
            }
            let metric_val =
                validate_write_metric_value(prop, &params.value, params.numeric_value)?;
            Ok(Some(metric_val))
        }
    }
}

fn is_same_view_widget(source_kind: &str, target: &str, widget_id: Uuid) -> bool {
    if source_kind != "VIEW" {
        return false;
    }
    let exact = widget_id.to_string();
    let legacy_prefix = format!("widget:{widget_id}:");
    target == exact || target.starts_with(&legacy_prefix)
}

async fn check_write_idempotency_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    params: &ViewWriteNodeParams,
) -> Result<Option<ViewWriteNodeOutcome>, AppError> {
    let existing = sqlx::query(
        "SELECT id, source_kind, target, node_id, payload
           FROM ax_action_log
          WHERE tenant_id = $1 AND idempotency_key = $2",
    )
    .bind(tenant_id)
    .bind(&params.idempotency_key)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("check ax_action_log idempotency failed: {err}")))?;

    let Some(row) = existing else {
        return Ok(None);
    };

    let source_kind: String = row.get("source_kind");
    let target: String = row.get("target");
    if !is_same_view_widget(&source_kind, &target, params.widget_id) {
        return Err(AppError::Conflict("CONFLICT"));
    }

    let action_log_id: i64 = row.get("id");
    let payload: serde_json::Value = row.get("payload");
    let node_id = payload
        .get("node_id")
        .and_then(serde_json::Value::as_str)
        .and_then(|s| Uuid::parse_str(s).ok())
        .or_else(|| {
            let raw: Option<String> = row.get("node_id");
            raw.and_then(|s| Uuid::parse_str(&s).ok())
        })
        .unwrap_or(params.node_id);

    let type_name = match payload.get("type_name").and_then(serde_json::Value::as_str) {
        Some(tn) if !tn.is_empty() => String::from(tn),
        _ => sqlx::query_scalar::<_, String>(
            "SELECT type_name FROM xb_nodes WHERE tenant_id = $1 AND id = $2",
        )
        .bind(tenant_id)
        .bind(node_id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("lookup node type_name failed: {err}")))?
        .unwrap_or_default(),
    };

    Ok(Some(ViewWriteNodeOutcome {
        node_id,
        type_name,
        action_log_id,
        idempotency_replay: true,
    }))
}

fn validate_node_dept_abac(
    props: &serde_json::Map<String, serde_json::Value>,
    caller_dept_id: &str,
) -> Result<(), AppError> {
    let caller_dept = caller_dept_id.trim();
    if caller_dept.is_empty() {
        return Ok(());
    }
    if let Some(node_dept) = props
        .get("dept_id")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        && node_dept != caller_dept
    {
        return Err(AppError::Forbidden("PERMISSION_DENIED"));
    }
    Ok(())
}

async fn apply_node_write_mutation_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    params: &ViewWriteNodeParams,
    mut props: serde_json::Map<String, serde_json::Value>,
    mut metrics: serde_json::Map<String, serde_json::Value>,
    metric_val: Option<f64>,
) -> Result<(), AppError> {
    let key = params.key.trim();
    let now = Utc::now();

    match params.target_kind {
        WriteNodeTargetKind::Property => {
            if params.value.is_null() {
                props.remove(key);
            } else {
                props.insert(String::from(key), params.value.clone());
            }
            let props_json = serde_json::Value::Object(props);
            sqlx::query(
                "UPDATE xb_nodes
                    SET properties = $3,
                        row_version = row_version + 1,
                        updated_at = NOW()
                  WHERE tenant_id = $1 AND id = $2",
            )
            .bind(tenant_id)
            .bind(params.node_id)
            .bind(props_json)
            .execute(&mut **tx)
            .await
            .map_err(|err| {
                AppError::Database(format!("update xb_nodes.properties failed: {err}"))
            })?;
        }
        WriteNodeTargetKind::Metric => {
            let val = metric_val.ok_or_else(|| {
                AppError::bad("INVALID_ARGUMENT", "missing numeric value for metric write")
            })?;
            metrics.insert(String::from(key), serde_json::json!(val));
            let mut ts_obj = match props.remove("__metric_ts") {
                Some(serde_json::Value::Object(map)) => map,
                _ => serde_json::Map::new(),
            };
            ts_obj.insert(
                String::from(key),
                serde_json::Value::String(now.to_rfc3339()),
            );
            props.insert(
                String::from("__metric_ts"),
                serde_json::Value::Object(ts_obj),
            );

            sqlx::query(
                "UPDATE xb_nodes
                    SET metrics = $3,
                        properties = $4,
                        row_version = row_version + 1,
                        updated_at = NOW()
                  WHERE tenant_id = $1 AND id = $2",
            )
            .bind(tenant_id)
            .bind(params.node_id)
            .bind(serde_json::Value::Object(metrics))
            .bind(serde_json::Value::Object(props))
            .execute(&mut **tx)
            .await
            .map_err(|err| AppError::Database(format!("update xb_nodes.metrics failed: {err}")))?;

            sqlx::query(
                "INSERT INTO xb_metric_series (
                     tenant_id, node_id, metric, ts, value, value_text, source_tag, batch_id
                 )
                 VALUES ($1, $2, $3, $4, $5, NULL, $6, $7)
                 ON CONFLICT (tenant_id, node_id, metric, ts, source_tag) DO UPDATE SET
                     value = EXCLUDED.value,
                     batch_id = EXCLUDED.batch_id",
            )
            .bind(tenant_id)
            .bind(params.node_id)
            .bind(key)
            .bind(now)
            .bind(val)
            .bind(MetricSourceTag::Action.as_str())
            .bind(params.widget_id)
            .execute(&mut **tx)
            .await
            .map_err(|err| {
                AppError::Database(format!(
                    "insert xb_metric_series for VIEW action failed: {err}"
                ))
            })?;
        }
    }

    Ok(())
}

fn resolve_duplicate_name_slug(
    src: &DashboardRecord,
    new_name: Option<&str>,
    new_slug: Option<&str>,
) -> Result<(String, String), AppError> {
    let target_name = match new_name {
        Some(n) => {
            let trimmed = n.trim();
            if trimmed.is_empty() {
                return Err(AppError::bad(
                    "INVALID_ARGUMENT",
                    "dashboard name must not be empty",
                ));
            }
            String::from(trimmed)
        }
        None => format!("{} (Copy)", src.name),
    };
    let target_slug = match new_slug {
        Some(s) => {
            let trimmed = s.trim();
            validate_slug(trimmed)?;
            String::from(trimmed)
        }
        None => build_duplicate_slug(&src.slug),
    };
    Ok((target_name, target_slug))
}

fn remap_filters_for_duplicate(
    filters: &mut [FilterDef],
    widgets: &[WidgetRecord],
) -> std::collections::HashMap<String, Uuid> {
    let mut id_remap = std::collections::HashMap::with_capacity(widgets.len());
    for w in widgets {
        id_remap.insert(w.id.to_string(), Uuid::new_v4());
    }
    for filter in filters {
        let mut remapped = BTreeMap::new();
        for (k, v) in &filter.field_map {
            let new_key = id_remap.get(k).map_or_else(|| k.clone(), Uuid::to_string);
            remapped.insert(new_key, v.clone());
        }
        filter.field_map = remapped;
    }
    id_remap
}

async fn try_insert_view_write_log_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    type_name: &str,
    params: &ViewWriteNodeParams,
) -> Result<Option<i64>, AppError> {
    let key = params.key.trim();
    let action_kind = match params.target_kind {
        WriteNodeTargetKind::Property => "WRITE_NODE_PROPERTY",
        WriteNodeTargetKind::Metric => "WRITE_NODE_METRIC",
    };
    let target = params.widget_id.to_string();
    let action_payload = serde_json::json!({
        "widget_id": params.widget_id,
        "node_id": params.node_id,
        "type_name": type_name,
        "target_kind": params.target_kind,
        "key": key,
        "value": params.value,
    });

    sqlx::query_scalar(
        "INSERT INTO ax_action_log (
             tenant_id, run_id, node_id, source_kind, action_kind,
             target, payload, status, idempotency_key
         )
         VALUES ($1, NULL, $2, 'VIEW', $3, $4, $5, 'DONE', $6)
         ON CONFLICT (tenant_id, idempotency_key) DO NOTHING
         RETURNING id",
    )
    .bind(tenant_id)
    .bind(params.node_id.to_string())
    .bind(action_kind)
    .bind(target)
    .bind(action_payload)
    .bind(&params.idempotency_key)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("insert ax_action_log failed: {err}")))
}

async fn emit_view_write_outbox_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    type_name: &str,
    params: &ViewWriteNodeParams,
) -> Result<(), AppError> {
    let key = params.key.trim();
    let (outbox_topic, outbox_payload) = match params.target_kind {
        WriteNodeTargetKind::Property => (
            OutboxTopic::NodeUpserted,
            serde_json::json!({
                "node_id": params.node_id,
                "node_ids": [params.node_id],
                "type_name": type_name,
                "type_names": [type_name],
                "properties": [key],
            }),
        ),
        WriteNodeTargetKind::Metric => (
            OutboxTopic::NodeMetricsChanged,
            serde_json::json!({
                "node_id": params.node_id,
                "node_ids": [params.node_id],
                "type_name": type_name,
                "type_names": [type_name],
                "metrics": [key],
            }),
        ),
    };

    insert_outbox_tx(
        tx,
        tenant_id,
        &NewOutboxEvent {
            topic: outbox_topic,
            payload: outbox_payload,
        },
    )
    .await
    .map(|_| ())
}

impl ViewRepository for PgStore {
    async fn list_dashboards(
        &self,
        tenant_id: &str,
        only_published: bool,
        query: SimplePageQuery,
    ) -> Result<PaginatedList<DashboardRecord>, AppError> {
        let (limit_usize, fetch_limit) = validate_page_limit(query.limit, 50)?;
        let cursor = query
            .cursor
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .map(|c| parse_ts_uuid_cursor(c.trim()))
            .transpose()?;
        let cursor_ts = cursor.map(|(ts, _)| ts);
        let cursor_id = cursor.map(|(_, id)| id);

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let rows = sqlx::query(
            "SELECT id, tenant_id, name, slug, description, filters, theme, canvas,
                    status, version, created_by, created_at, updated_at
               FROM xv_dashboards
              WHERE tenant_id = $1
                AND (NOT $2 OR status = 'PUBLISHED')
                AND ($3::timestamptz IS NULL OR (updated_at, id) < ($3, $4::uuid))
              ORDER BY updated_at DESC, id DESC
              LIMIT $5",
        )
        .bind(tenant_id)
        .bind(only_published)
        .bind(cursor_ts)
        .bind(cursor_id)
        .bind(fetch_limit)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("list xv_dashboards failed: {err}")))?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit list_dashboards tx failed: {err}"))
        })?;

        let has_more = rows.len() > limit_usize;
        let mut items = Vec::with_capacity(rows.len().min(limit_usize));
        for row in rows.into_iter().take(limit_usize) {
            items.push(decode_dashboard_row(&row)?);
        }
        let next_cursor = if has_more {
            items
                .last()
                .map(|d| encode_ts_uuid_cursor(d.updated_at, d.id))
        } else {
            None
        };
        Ok(PaginatedList { items, next_cursor })
    }

    async fn create_dashboard(
        &self,
        tenant_id: &str,
        created_by: Option<Uuid>,
        req: CreateDashboardRequest,
    ) -> Result<DashboardDetail, AppError> {
        let name = req.name.trim();
        if name.is_empty() {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "dashboard name must not be empty",
            ));
        }
        let slug = req.slug.trim();
        validate_slug(slug)?;
        validate_canvas_config(&req.canvas)?;

        let filters_json = serde_json::to_value(&req.filters).map_err(|err| {
            AppError::bad("INVALID_ARGUMENT", format!("serialize filters: {err}"))
        })?;
        let canvas_json = serde_json::to_value(&req.canvas)
            .map_err(|err| AppError::bad("INVALID_ARGUMENT", format!("serialize canvas: {err}")))?;

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let row = sqlx::query(
            "INSERT INTO xv_dashboards (
                 tenant_id, name, slug, description, filters, theme, canvas, status, version, created_by
             )
             VALUES ($1, $2, $3, $4, $5, $6, $7, 'DRAFT', 1, $8)
             RETURNING id, tenant_id, name, slug, description, filters, theme, canvas,
                       status, version, created_by, created_at, updated_at",
        )
        .bind(tenant_id)
        .bind(name)
        .bind(slug)
        .bind(&req.description)
        .bind(filters_json)
        .bind(req.theme.as_str())
        .bind(canvas_json)
        .bind(created_by)
        .fetch_one(&mut *tx)
        .await
        .map_err(|err| {
            if is_unique_violation(&err) {
                AppError::Conflict("NAME_TAKEN")
            } else {
                AppError::Database(format!("insert xv_dashboards failed: {err}"))
            }
        })?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit create_dashboard tx failed: {err}"))
        })?;

        let dashboard = decode_dashboard_row(&row)?;
        Ok(DashboardDetail {
            dashboard,
            widgets: Vec::new(),
        })
    }

    async fn get_dashboard(
        &self,
        tenant_id: &str,
        dashboard_id: Uuid,
        require_published: bool,
    ) -> Result<Option<DashboardDetail>, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let maybe_row = sqlx::query(
            "SELECT id, tenant_id, name, slug, description, filters, theme, canvas,
                    status, version, created_by, created_at, updated_at
               FROM xv_dashboards
              WHERE tenant_id = $1
                AND id = $2
                AND (NOT $3 OR status = 'PUBLISHED')",
        )
        .bind(tenant_id)
        .bind(dashboard_id)
        .bind(require_published)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("get xv_dashboards failed: {err}")))?;

        let Some(row) = maybe_row else {
            tx.commit().await.map_err(|err| {
                AppError::Database(format!("commit get_dashboard tx failed: {err}"))
            })?;
            return Ok(None);
        };

        let dashboard = decode_dashboard_row(&row)?;
        let widgets = fetch_dashboard_widgets_tx(&mut tx, tenant_id, dashboard.id).await?;
        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit get_dashboard tx failed: {err}")))?;

        Ok(Some(DashboardDetail { dashboard, widgets }))
    }

    async fn get_dashboard_by_slug(
        &self,
        tenant_id: &str,
        slug: &str,
        require_published: bool,
    ) -> Result<Option<DashboardDetail>, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let maybe_row = sqlx::query(
            "SELECT id, tenant_id, name, slug, description, filters, theme, canvas,
                    status, version, created_by, created_at, updated_at
               FROM xv_dashboards
              WHERE tenant_id = $1
                AND slug = $2
                AND (NOT $3 OR status = 'PUBLISHED')",
        )
        .bind(tenant_id)
        .bind(slug.trim())
        .bind(require_published)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("get xv_dashboards by slug failed: {err}")))?;

        let Some(row) = maybe_row else {
            tx.commit().await.map_err(|err| {
                AppError::Database(format!("commit get_dashboard_by_slug tx failed: {err}"))
            })?;
            return Ok(None);
        };

        let dashboard = decode_dashboard_row(&row)?;
        let widgets = fetch_dashboard_widgets_tx(&mut tx, tenant_id, dashboard.id).await?;
        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit get_dashboard_by_slug tx failed: {err}"))
        })?;

        Ok(Some(DashboardDetail { dashboard, widgets }))
    }

    async fn update_dashboard(
        &self,
        tenant_id: &str,
        dashboard_id: Uuid,
        req: UpdateDashboardRequest,
    ) -> Result<DashboardRecord, AppError> {
        let name = match req.name {
            Some(ref n) => {
                let trimmed = n.trim();
                if trimmed.is_empty() {
                    return Err(AppError::bad(
                        "INVALID_ARGUMENT",
                        "dashboard name must not be empty",
                    ));
                }
                Some(String::from(trimmed))
            }
            None => None,
        };
        let slug = match req.slug {
            Some(ref s) => {
                let trimmed = s.trim();
                validate_slug(trimmed)?;
                Some(String::from(trimmed))
            }
            None => None,
        };
        if let Some(ref c) = req.canvas {
            validate_canvas_config(c)?;
        }
        let filters_json = req
            .filters
            .as_ref()
            .map(serde_json::to_value)
            .transpose()
            .map_err(|err| {
                AppError::bad("INVALID_ARGUMENT", format!("serialize filters: {err}"))
            })?;
        let canvas_json = req
            .canvas
            .as_ref()
            .map(serde_json::to_value)
            .transpose()
            .map_err(|err| AppError::bad("INVALID_ARGUMENT", format!("serialize canvas: {err}")))?;
        let theme_str = req.theme.map(DashboardTheme::as_str);

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let maybe_row = sqlx::query(
            "UPDATE xv_dashboards
                SET name        = COALESCE($3, name),
                    slug        = COALESCE($4, slug),
                    description = COALESCE($5, description),
                    filters     = COALESCE($6, filters),
                    theme       = COALESCE($7, theme),
                    canvas      = COALESCE($8, canvas),
                    updated_at  = NOW()
              WHERE tenant_id = $1 AND id = $2
              RETURNING id, tenant_id, name, slug, description, filters, theme, canvas,
                        status, version, created_by, created_at, updated_at",
        )
        .bind(tenant_id)
        .bind(dashboard_id)
        .bind(name)
        .bind(slug)
        .bind(req.description)
        .bind(filters_json)
        .bind(theme_str)
        .bind(canvas_json)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| {
            if is_unique_violation(&err) {
                AppError::Conflict("NAME_TAKEN")
            } else {
                AppError::Database(format!("update xv_dashboards failed: {err}"))
            }
        })?;

        let row = maybe_row.ok_or(AppError::NotFound("DASHBOARD_NOT_FOUND"))?;
        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit update_dashboard tx failed: {err}"))
        })?;

        decode_dashboard_row(&row)
    }

    async fn delete_dashboard(
        &self,
        tenant_id: &str,
        dashboard_id: Uuid,
    ) -> Result<bool, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let res = sqlx::query("DELETE FROM xv_dashboards WHERE tenant_id = $1 AND id = $2")
            .bind(tenant_id)
            .bind(dashboard_id)
            .execute(&mut *tx)
            .await
            .map_err(|err| AppError::Database(format!("delete xv_dashboards failed: {err}")))?;
        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit delete_dashboard tx failed: {err}"))
        })?;
        Ok(res.rows_affected() > 0)
    }

    async fn publish_dashboard(
        &self,
        tenant_id: &str,
        dashboard_id: Uuid,
    ) -> Result<DashboardRecord, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let maybe_row = sqlx::query(
            "UPDATE xv_dashboards
                SET status = 'PUBLISHED',
                    version = version + 1,
                    updated_at = NOW()
              WHERE tenant_id = $1 AND id = $2
              RETURNING id, tenant_id, name, slug, description, filters, theme, canvas,
                        status, version, created_by, created_at, updated_at",
        )
        .bind(tenant_id)
        .bind(dashboard_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("publish xv_dashboards failed: {err}")))?;

        let row = maybe_row.ok_or(AppError::NotFound("DASHBOARD_NOT_FOUND"))?;
        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit publish_dashboard tx failed: {err}"))
        })?;

        decode_dashboard_row(&row)
    }

    async fn duplicate_dashboard(
        &self,
        tenant_id: &str,
        dashboard_id: Uuid,
        new_name: Option<String>,
        new_slug: Option<String>,
        created_by: Option<Uuid>,
    ) -> Result<DashboardDetail, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let maybe_src = sqlx::query(
            "SELECT id, tenant_id, name, slug, description, filters, theme, canvas,
                    status, version, created_by, created_at, updated_at
               FROM xv_dashboards
              WHERE tenant_id = $1 AND id = $2",
        )
        .bind(tenant_id)
        .bind(dashboard_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("fetch source xv_dashboards failed: {err}")))?;

        let src_row = maybe_src.ok_or(AppError::NotFound("DASHBOARD_NOT_FOUND"))?;
        let mut src = decode_dashboard_row(&src_row)?;
        let src_widgets = fetch_dashboard_widgets_tx(&mut tx, tenant_id, dashboard_id).await?;

        let (target_name, target_slug) =
            resolve_duplicate_name_slug(&src, new_name.as_deref(), new_slug.as_deref())?;
        let id_remap = remap_filters_for_duplicate(&mut src.filters, &src_widgets);

        let filters_json = serde_json::to_value(&src.filters).map_err(|err| {
            AppError::bad("INVALID_ARGUMENT", format!("serialize filters: {err}"))
        })?;
        let canvas_json = serde_json::to_value(&src.canvas)
            .map_err(|err| AppError::bad("INVALID_ARGUMENT", format!("serialize canvas: {err}")))?;

        let new_row = sqlx::query(
            "INSERT INTO xv_dashboards (
                 tenant_id, name, slug, description, filters, theme, canvas, status, version, created_by
             )
             VALUES ($1, $2, $3, $4, $5, $6, $7, 'DRAFT', 1, $8)
             RETURNING id, tenant_id, name, slug, description, filters, theme, canvas,
                       status, version, created_by, created_at, updated_at",
        )
        .bind(tenant_id)
        .bind(&target_name)
        .bind(&target_slug)
        .bind(&src.description)
        .bind(filters_json)
        .bind(src.theme.as_str())
        .bind(canvas_json)
        .bind(created_by)
        .fetch_one(&mut *tx)
        .await
        .map_err(|err| {
            if is_unique_violation(&err) {
                AppError::Conflict("NAME_TAKEN")
            } else {
                AppError::Database(format!("insert duplicate xv_dashboards failed: {err}"))
            }
        })?;

        let dashboard = decode_dashboard_row(&new_row)?;
        let mut widgets = Vec::with_capacity(src_widgets.len());
        for w in src_widgets {
            let new_wid = id_remap.get(&w.id.to_string()).copied();
            let created_w = insert_widget_tx(
                &mut tx,
                tenant_id,
                new_wid,
                &CreateWidgetRecordParams {
                    dashboard_id: dashboard.id,
                    widget_kind: w.widget_kind,
                    title: w.title,
                    position: w.position,
                    data_source: w.data_source,
                    encoding: w.encoding,
                    options: w.options,
                    drill: w.drill,
                    refresh_mode: w.refresh_mode,
                    refresh_seconds: w.refresh_seconds,
                    deps: w.deps,
                },
            )
            .await?;
            widgets.push(created_w);
        }

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit duplicate_dashboard tx failed: {err}"))
        })?;
        Ok(DashboardDetail { dashboard, widgets })
    }

    async fn get_widget(
        &self,
        tenant_id: &str,
        widget_id: Uuid,
    ) -> Result<Option<WidgetRecord>, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let maybe_row = sqlx::query(
            "SELECT id, tenant_id, dashboard_id, widget_kind, title, position, data_source,
                    encoding, options, drill, refresh_mode, refresh_seconds, deps, created_at, updated_at
               FROM xv_widgets
              WHERE tenant_id = $1 AND id = $2",
        )
        .bind(tenant_id)
        .bind(widget_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("get xv_widgets failed: {err}")))?;

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit get_widget tx failed: {err}")))?;
        maybe_row.as_ref().map(decode_widget_row).transpose()
    }

    async fn create_widget(
        &self,
        tenant_id: &str,
        params: CreateWidgetRecordParams,
    ) -> Result<WidgetRecord, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        ensure_dashboard_exists_tx(&mut tx, tenant_id, params.dashboard_id).await?;
        let widget = insert_widget_tx(&mut tx, tenant_id, None, &params).await?;

        sqlx::query("UPDATE xv_dashboards SET updated_at = NOW() WHERE tenant_id = $1 AND id = $2")
            .bind(tenant_id)
            .bind(params.dashboard_id)
            .execute(&mut *tx)
            .await
            .map_err(|err| AppError::Database(format!("touch xv_dashboards failed: {err}")))?;

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit create_widget tx failed: {err}")))?;
        Ok(widget)
    }

    async fn update_widget(
        &self,
        tenant_id: &str,
        widget_id: Uuid,
        params: UpdateWidgetRecordParams,
    ) -> Result<WidgetRecord, AppError> {
        if let Some(ref pos) = params.position {
            validate_widget_position(pos)?;
        }
        if let Some(secs) = params.refresh_seconds {
            validate_refresh_seconds(secs)?;
        }

        let position_json = params
            .position
            .as_ref()
            .map(serde_json::to_value)
            .transpose()
            .map_err(|err| {
                AppError::bad("INVALID_ARGUMENT", format!("serialize position: {err}"))
            })?;
        let ds_json = params
            .data_source
            .as_ref()
            .map(serde_json::to_value)
            .transpose()
            .map_err(|err| {
                AppError::bad("INVALID_ARGUMENT", format!("serialize data_source: {err}"))
            })?;
        let drill_json = params
            .drill
            .as_ref()
            .map(serde_json::to_value)
            .transpose()
            .map_err(|err| AppError::bad("INVALID_ARGUMENT", format!("serialize drill: {err}")))?;
        let deps_json = params
            .deps
            .as_ref()
            .map(serde_json::to_value)
            .transpose()
            .map_err(|err| AppError::bad("INVALID_ARGUMENT", format!("serialize deps: {err}")))?;

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let maybe_row = sqlx::query(
            "UPDATE xv_widgets
                SET widget_kind     = COALESCE($3, widget_kind),
                    title           = COALESCE($4, title),
                    position        = COALESCE($5, position),
                    data_source     = COALESCE($6, data_source),
                    encoding        = COALESCE($7, encoding),
                    options         = COALESCE($8, options),
                    drill           = COALESCE($9, drill),
                    refresh_mode    = COALESCE($10, refresh_mode),
                    refresh_seconds = COALESCE($11, refresh_seconds),
                    deps            = COALESCE($12, deps),
                    updated_at      = NOW()
              WHERE tenant_id = $1 AND id = $2
              RETURNING id, tenant_id, dashboard_id, widget_kind, title, position, data_source,
                        encoding, options, drill, refresh_mode, refresh_seconds, deps, created_at, updated_at",
        )
        .bind(tenant_id)
        .bind(widget_id)
        .bind(params.widget_kind.map(WidgetKind::as_str))
        .bind(params.title)
        .bind(position_json)
        .bind(ds_json)
        .bind(params.encoding)
        .bind(params.options)
        .bind(drill_json)
        .bind(params.refresh_mode.map(RefreshMode::as_str))
        .bind(params.refresh_seconds)
        .bind(deps_json)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("update xv_widgets failed: {err}")))?;

        let row = maybe_row.ok_or(AppError::NotFound("WIDGET_NOT_FOUND"))?;
        let widget = decode_widget_row(&row)?;

        sqlx::query("UPDATE xv_dashboards SET updated_at = NOW() WHERE tenant_id = $1 AND id = $2")
            .bind(tenant_id)
            .bind(widget.dashboard_id)
            .execute(&mut *tx)
            .await
            .map_err(|err| AppError::Database(format!("touch xv_dashboards failed: {err}")))?;

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit update_widget tx failed: {err}")))?;
        Ok(widget)
    }

    async fn delete_widget(&self, tenant_id: &str, widget_id: Uuid) -> Result<bool, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let deleted_dashboard: Option<Uuid> = sqlx::query_scalar(
            "DELETE FROM xv_widgets WHERE tenant_id = $1 AND id = $2 RETURNING dashboard_id",
        )
        .bind(tenant_id)
        .bind(widget_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("delete xv_widgets failed: {err}")))?;

        if let Some(dashboard_id) = deleted_dashboard {
            sqlx::query(
                "UPDATE xv_dashboards SET updated_at = NOW() WHERE tenant_id = $1 AND id = $2",
            )
            .bind(tenant_id)
            .bind(dashboard_id)
            .execute(&mut *tx)
            .await
            .map_err(|err| AppError::Database(format!("touch xv_dashboards failed: {err}")))?;
        }

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit delete_widget tx failed: {err}")))?;
        Ok(deleted_dashboard.is_some())
    }

    async fn update_dashboard_layout(
        &self,
        tenant_id: &str,
        dashboard_id: Uuid,
        items: &[LayoutUpdateItem],
    ) -> Result<Vec<WidgetRecord>, AppError> {
        for item in items {
            validate_widget_position(&item.position)?;
        }

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        ensure_dashboard_exists_tx(&mut tx, tenant_id, dashboard_id).await?;

        for item in items {
            let pos_json = serde_json::to_value(&item.position).map_err(|err| {
                AppError::bad("INVALID_ARGUMENT", format!("serialize position: {err}"))
            })?;
            let updated: Option<Uuid> = sqlx::query_scalar(
                "UPDATE xv_widgets
                    SET position = $4,
                        updated_at = NOW()
                  WHERE tenant_id = $1 AND dashboard_id = $2 AND id = $3
                  RETURNING id",
            )
            .bind(tenant_id)
            .bind(dashboard_id)
            .bind(item.id)
            .bind(pos_json)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|err| AppError::Database(format!("update widget layout failed: {err}")))?;
            if updated.is_none() {
                return Err(AppError::NotFound("WIDGET_NOT_FOUND"));
            }
        }

        sqlx::query("UPDATE xv_dashboards SET updated_at = NOW() WHERE tenant_id = $1 AND id = $2")
            .bind(tenant_id)
            .bind(dashboard_id)
            .execute(&mut *tx)
            .await
            .map_err(|err| AppError::Database(format!("touch xv_dashboards failed: {err}")))?;

        let widgets = fetch_dashboard_widgets_tx(&mut tx, tenant_id, dashboard_id).await?;
        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit update_dashboard_layout tx failed: {err}"))
        })?;
        Ok(widgets)
    }

    async fn create_ai_dashboard(
        &self,
        tenant_id: &str,
        created_by: Option<Uuid>,
        dashboard: CreateDashboardRequest,
        widgets: Vec<CreateWidgetRecordParams>,
    ) -> Result<DashboardDetail, AppError> {
        let name = dashboard.name.trim();
        if name.is_empty() {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "dashboard name must not be empty",
            ));
        }
        let slug = dashboard.slug.trim();
        validate_slug(slug)?;
        validate_canvas_config(&dashboard.canvas)?;
        for w in &widgets {
            validate_widget_position(&w.position)?;
            validate_refresh_seconds(w.refresh_seconds)?;
        }

        let filters_json = serde_json::to_value(&dashboard.filters).map_err(|err| {
            AppError::bad("INVALID_ARGUMENT", format!("serialize filters: {err}"))
        })?;
        let canvas_json = serde_json::to_value(&dashboard.canvas)
            .map_err(|err| AppError::bad("INVALID_ARGUMENT", format!("serialize canvas: {err}")))?;

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let row = sqlx::query(
            "INSERT INTO xv_dashboards (
                 tenant_id, name, slug, description, filters, theme, canvas, status, version, created_by
             )
             VALUES ($1, $2, $3, $4, $5, $6, $7, 'DRAFT', 1, $8)
             RETURNING id, tenant_id, name, slug, description, filters, theme, canvas,
                       status, version, created_by, created_at, updated_at",
        )
        .bind(tenant_id)
        .bind(name)
        .bind(slug)
        .bind(&dashboard.description)
        .bind(filters_json)
        .bind(dashboard.theme.as_str())
        .bind(canvas_json)
        .bind(created_by)
        .fetch_one(&mut *tx)
        .await
        .map_err(|err| {
            if is_unique_violation(&err) {
                AppError::Conflict("NAME_TAKEN")
            } else {
                AppError::Database(format!("insert ai xv_dashboards failed: {err}"))
            }
        })?;

        let created_dashboard = decode_dashboard_row(&row)?;
        let mut created_widgets = Vec::with_capacity(widgets.len());
        for mut w in widgets {
            w.dashboard_id = created_dashboard.id;
            created_widgets.push(insert_widget_tx(&mut tx, tenant_id, None, &w).await?);
        }

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit create_ai_dashboard tx failed: {err}"))
        })?;

        Ok(DashboardDetail {
            dashboard: created_dashboard,
            widgets: created_widgets,
        })
    }

    async fn find_widgets_by_deps(
        &self,
        tenant_id: &str,
        filter_deps: &WidgetDeps,
    ) -> Result<Vec<AffectedWidgetRef>, AppError> {
        let probes = collect_dep_probes(filter_deps);
        if probes.is_empty() {
            return Ok(Vec::new());
        }

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let mut qb: QueryBuilder<Postgres> =
            QueryBuilder::new("SELECT id, dashboard_id FROM xv_widgets WHERE tenant_id = ");
        qb.push_bind(tenant_id);
        qb.push(" AND (");
        for (idx, probe) in probes.into_iter().enumerate() {
            if idx > 0 {
                qb.push(" OR ");
            }
            qb.push("deps @> ");
            qb.push_bind(probe);
        }
        qb.push(") ORDER BY dashboard_id ASC, id ASC");

        let rows =
            qb.build().fetch_all(&mut *tx).await.map_err(|err| {
                AppError::Database(format!("query xv_widgets by deps failed: {err}"))
            })?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit find_widgets_by_deps tx failed: {err}"))
        })?;

        Ok(rows
            .into_iter()
            .map(|r| AffectedWidgetRef {
                widget_id: r.get("id"),
                dashboard_id: r.get("dashboard_id"),
            })
            .collect())
    }

    async fn list_assets(
        &self,
        tenant_id: &str,
        asset_kind: Option<AssetKind>,
        query: SimplePageQuery,
    ) -> Result<PaginatedList<AssetRecord>, AppError> {
        let (limit_usize, fetch_limit) = validate_page_limit(query.limit, 50)?;
        let cursor = query
            .cursor
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .map(|c| parse_ts_uuid_cursor(c.trim()))
            .transpose()?;
        let cursor_ts = cursor.map(|(ts, _)| ts);
        let cursor_id = cursor.map(|(_, id)| id);

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let rows = sqlx::query(
            "SELECT id, tenant_id, asset_kind, name, file_path, mime_type,
                    sha256, width_px, height_px, created_at
               FROM xv_assets
              WHERE tenant_id = $1
                AND ($2::text IS NULL OR asset_kind = $2)
                AND ($3::timestamptz IS NULL OR (created_at, id) < ($3, $4::uuid))
              ORDER BY created_at DESC, id DESC
              LIMIT $5",
        )
        .bind(tenant_id)
        .bind(asset_kind.map(AssetKind::as_str))
        .bind(cursor_ts)
        .bind(cursor_id)
        .bind(fetch_limit)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("list xv_assets failed: {err}")))?;

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit list_assets tx failed: {err}")))?;

        let has_more = rows.len() > limit_usize;
        let mut items = Vec::with_capacity(rows.len().min(limit_usize));
        for row in rows.into_iter().take(limit_usize) {
            items.push(decode_asset_row(&row)?);
        }
        let next_cursor = if has_more {
            items
                .last()
                .map(|a| encode_ts_uuid_cursor(a.created_at, a.id))
        } else {
            None
        };
        Ok(PaginatedList { items, next_cursor })
    }

    async fn create_asset(
        &self,
        tenant_id: &str,
        params: CreateAssetParams,
    ) -> Result<AssetRecord, AppError> {
        let name = params.name.trim();
        if name.is_empty() {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "asset name must not be empty",
            ));
        }

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let row = sqlx::query(
            "INSERT INTO xv_assets (
                 id, tenant_id, asset_kind, name, file_path, mime_type, sha256, width_px, height_px
             )
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
             RETURNING id, tenant_id, asset_kind, name, file_path, mime_type,
                       sha256, width_px, height_px, created_at",
        )
        .bind(params.id)
        .bind(tenant_id)
        .bind(params.asset_kind.as_str())
        .bind(name)
        .bind(&params.file_path)
        .bind(&params.mime_type)
        .bind(&params.sha256)
        .bind(params.width_px)
        .bind(params.height_px)
        .fetch_one(&mut *tx)
        .await
        .map_err(|err| {
            if is_unique_violation(&err) {
                AppError::Conflict("NAME_TAKEN")
            } else {
                AppError::Database(format!("insert xv_assets failed: {err}"))
            }
        })?;

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit create_asset tx failed: {err}")))?;
        decode_asset_row(&row)
    }

    async fn get_asset(
        &self,
        tenant_id: &str,
        asset_id: Uuid,
    ) -> Result<Option<AssetRecord>, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let maybe_row = sqlx::query(
            "SELECT id, tenant_id, asset_kind, name, file_path, mime_type,
                    sha256, width_px, height_px, created_at
               FROM xv_assets
              WHERE tenant_id = $1 AND id = $2",
        )
        .bind(tenant_id)
        .bind(asset_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("get xv_assets failed: {err}")))?;

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit get_asset tx failed: {err}")))?;
        maybe_row.as_ref().map(decode_asset_row).transpose()
    }

    async fn delete_asset(
        &self,
        tenant_id: &str,
        asset_id: Uuid,
    ) -> Result<Option<AssetRecord>, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let maybe_row = sqlx::query(
            "DELETE FROM xv_assets
              WHERE tenant_id = $1 AND id = $2
              RETURNING id, tenant_id, asset_kind, name, file_path, mime_type,
                        sha256, width_px, height_px, created_at",
        )
        .bind(tenant_id)
        .bind(asset_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("delete xv_assets failed: {err}")))?;

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit delete_asset tx failed: {err}")))?;
        maybe_row.as_ref().map(decode_asset_row).transpose()
    }

    async fn list_floor_placements(
        &self,
        tenant_id: &str,
        asset_id: Uuid,
    ) -> Result<Vec<FloorPlacementRecord>, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        ensure_asset_exists_tx(&mut tx, tenant_id, asset_id).await?;

        let rows = sqlx::query(
            "SELECT asset_id, node_id, x_ratio, y_ratio
               FROM xv_floor_placements
              WHERE tenant_id = $1 AND asset_id = $2
              ORDER BY node_id ASC",
        )
        .bind(tenant_id)
        .bind(asset_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("list xv_floor_placements failed: {err}")))?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit list_floor_placements tx failed: {err}"))
        })?;
        Ok(rows.iter().map(decode_floor_placement_row).collect())
    }

    async fn replace_floor_placements(
        &self,
        tenant_id: &str,
        asset_id: Uuid,
        items: &[PutFloorPlacementItem],
    ) -> Result<Vec<FloorPlacementRecord>, AppError> {
        let mut seen = HashSet::with_capacity(items.len());
        let mut node_ids = Vec::with_capacity(items.len());
        let mut xs = Vec::with_capacity(items.len());
        let mut ys = Vec::with_capacity(items.len());

        for item in items {
            if !item.x_ratio.is_finite()
                || !(0.0..=1.0).contains(&item.x_ratio)
                || !item.y_ratio.is_finite()
                || !(0.0..=1.0).contains(&item.y_ratio)
            {
                return Err(AppError::bad(
                    "INVALID_ARGUMENT",
                    "floor placement x_ratio and y_ratio must be finite numbers in 0.0..=1.0",
                ));
            }
            if !seen.insert(item.node_id) {
                return Err(AppError::bad(
                    "INVALID_ARGUMENT",
                    format!("duplicate node_id {} in floor placements", item.node_id),
                ));
            }
            node_ids.push(item.node_id);
            xs.push(item.x_ratio);
            ys.push(item.y_ratio);
        }

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        ensure_asset_kind_tx(&mut tx, tenant_id, asset_id, AssetKind::FloorPlanImage).await?;
        ensure_nodes_exist_tx(&mut tx, tenant_id, &node_ids).await?;

        sqlx::query("DELETE FROM xv_floor_placements WHERE tenant_id = $1 AND asset_id = $2")
            .bind(tenant_id)
            .bind(asset_id)
            .execute(&mut *tx)
            .await
            .map_err(|err| {
                AppError::Database(format!("clear xv_floor_placements failed: {err}"))
            })?;

        if !items.is_empty() {
            sqlx::query(
                "INSERT INTO xv_floor_placements (tenant_id, asset_id, node_id, x_ratio, y_ratio)
                 SELECT $1, $2, u.node_id, u.x_ratio, u.y_ratio
                   FROM UNNEST($3::uuid[], $4::float8[], $5::float8[])
                     AS u(node_id, x_ratio, y_ratio)",
            )
            .bind(tenant_id)
            .bind(asset_id)
            .bind(&node_ids)
            .bind(&xs)
            .bind(&ys)
            .execute(&mut *tx)
            .await
            .map_err(|err| {
                AppError::Database(format!("insert xv_floor_placements failed: {err}"))
            })?;
        }

        let rows = sqlx::query(
            "SELECT asset_id, node_id, x_ratio, y_ratio
               FROM xv_floor_placements
              WHERE tenant_id = $1 AND asset_id = $2
              ORDER BY node_id ASC",
        )
        .bind(tenant_id)
        .bind(asset_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("reload xv_floor_placements failed: {err}")))?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit replace_floor_placements tx failed: {err}"))
        })?;
        Ok(rows.iter().map(decode_floor_placement_row).collect())
    }

    async fn list_scene3d_bindings(
        &self,
        tenant_id: &str,
        asset_id: Uuid,
    ) -> Result<Vec<Scene3dBindingRecord>, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        ensure_asset_exists_tx(&mut tx, tenant_id, asset_id).await?;

        let rows = sqlx::query(
            "SELECT asset_id, node_id, mesh_name, pos_x, pos_y, pos_z, scale
               FROM xv_scene3d_bindings
              WHERE tenant_id = $1 AND asset_id = $2
              ORDER BY node_id ASC",
        )
        .bind(tenant_id)
        .bind(asset_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("list xv_scene3d_bindings failed: {err}")))?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit list_scene3d_bindings tx failed: {err}"))
        })?;
        Ok(rows.iter().map(decode_scene3d_binding_row).collect())
    }

    async fn replace_scene3d_bindings(
        &self,
        tenant_id: &str,
        asset_id: Uuid,
        items: &[PutScene3dBindingItem],
    ) -> Result<Vec<Scene3dBindingRecord>, AppError> {
        let mut seen = HashSet::with_capacity(items.len());
        let mut node_ids = Vec::with_capacity(items.len());
        let mut meshes = Vec::with_capacity(items.len());
        let mut xs = Vec::with_capacity(items.len());
        let mut ys = Vec::with_capacity(items.len());
        let mut zs = Vec::with_capacity(items.len());
        let mut scales = Vec::with_capacity(items.len());

        for item in items {
            if !item.pos_x.is_finite()
                || !item.pos_y.is_finite()
                || !item.pos_z.is_finite()
                || !item.scale.is_finite()
                || item.scale <= 0.0
            {
                return Err(AppError::bad(
                    "INVALID_ARGUMENT",
                    "3D binding coordinates must be finite and scale must be > 0",
                ));
            }
            if !seen.insert(item.node_id) {
                return Err(AppError::bad(
                    "INVALID_ARGUMENT",
                    format!("duplicate node_id {} in 3D scene bindings", item.node_id),
                ));
            }
            node_ids.push(item.node_id);
            meshes.push(item.mesh_name.trim().to_owned());
            xs.push(item.pos_x);
            ys.push(item.pos_y);
            zs.push(item.pos_z);
            scales.push(item.scale);
        }

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        ensure_asset_kind_tx(&mut tx, tenant_id, asset_id, AssetKind::Model3d).await?;
        ensure_nodes_exist_tx(&mut tx, tenant_id, &node_ids).await?;

        sqlx::query("DELETE FROM xv_scene3d_bindings WHERE tenant_id = $1 AND asset_id = $2")
            .bind(tenant_id)
            .bind(asset_id)
            .execute(&mut *tx)
            .await
            .map_err(|err| {
                AppError::Database(format!("clear xv_scene3d_bindings failed: {err}"))
            })?;

        if !items.is_empty() {
            sqlx::query(
                "INSERT INTO xv_scene3d_bindings (
                     tenant_id, asset_id, node_id, mesh_name, pos_x, pos_y, pos_z, scale
                 )
                 SELECT $1, $2, u.node_id, u.mesh_name, u.pos_x, u.pos_y, u.pos_z, u.scale
                   FROM UNNEST(
                       $3::uuid[], $4::text[], $5::float8[], $6::float8[], $7::float8[], $8::float8[]
                   ) AS u(node_id, mesh_name, pos_x, pos_y, pos_z, scale)",
            )
            .bind(tenant_id)
            .bind(asset_id)
            .bind(&node_ids)
            .bind(&meshes)
            .bind(&xs)
            .bind(&ys)
            .bind(&zs)
            .bind(&scales)
            .execute(&mut *tx)
            .await
            .map_err(|err| {
                AppError::Database(format!("insert xv_scene3d_bindings failed: {err}"))
            })?;
        }

        let rows = sqlx::query(
            "SELECT asset_id, node_id, mesh_name, pos_x, pos_y, pos_z, scale
               FROM xv_scene3d_bindings
              WHERE tenant_id = $1 AND asset_id = $2
              ORDER BY node_id ASC",
        )
        .bind(tenant_id)
        .bind(asset_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("reload xv_scene3d_bindings failed: {err}")))?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit replace_scene3d_bindings tx failed: {err}"))
        })?;
        Ok(rows.iter().map(decode_scene3d_binding_row).collect())
    }

    async fn list_widgets_by_asset(
        &self,
        tenant_id: &str,
        asset_id: Uuid,
    ) -> Result<Vec<WidgetRecord>, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let rows = sqlx::query(
            "SELECT id, tenant_id, dashboard_id, widget_kind, title, position, data_source,
                    encoding, options, drill, refresh_mode, refresh_seconds, deps, created_at, updated_at
               FROM xv_widgets
              WHERE tenant_id = $1
                AND data_source->>'asset_id' = $2
                AND data_source->>'kind' IN ('FLOOR_PLAN', 'SCENE_3D', 'SCENE3D')
              ORDER BY dashboard_id ASC, id ASC",
        )
        .bind(tenant_id)
        .bind(asset_id.to_string())
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("list_widgets_by_asset failed: {err}")))?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit list_widgets_by_asset tx failed: {err}"))
        })?;
        rows.iter().map(decode_widget_row).collect()
    }

    async fn query_nodes_geo(
        &self,
        tenant_id: &str,
        type_names: &[String],
        color_metric: Option<&str>,
    ) -> Result<Vec<NodesGeoRow>, AppError> {
        let type_filter = normalize_type_names(type_names);
        let metric_key = normalize_color_metric(color_metric);

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let rows = sqlx::query(
            "SELECT n.id AS node_id,
                    n.type_name,
                    n.node_key,
                    n.title,
                    n.geo_lng AS lng,
                    n.geo_lat AS lat,
                    CASE
                        WHEN $3::text IS NOT NULL AND jsonb_typeof(n.metrics -> $3) = 'number'
                        THEN (n.metrics ->> $3)::double precision
                        ELSE NULL
                    END AS metric_value,
                    (
                        SELECT COUNT(*)
                          FROM ax_alerts a
                         WHERE a.tenant_id = $1
                           AND a.node_id = n.id
                           AND a.status IN ('OPEN', 'ACK')
                    ) AS open_alerts
               FROM xb_nodes n
               LEFT JOIN xb_subgraphs sg
                 ON sg.tenant_id = n.tenant_id AND sg.slug = n.subgraph_slug
              WHERE n.tenant_id = $1
                AND COALESCE(sg.is_dev_sandbox, FALSE) = FALSE
                AND n.geo_lng IS NOT NULL
                AND n.geo_lat IS NOT NULL
                AND ($2::text[] IS NULL OR n.type_name = ANY($2))
              ORDER BY n.type_name ASC, n.node_key ASC, n.id ASC",
        )
        .bind(tenant_id)
        .bind(type_filter)
        .bind(metric_key)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("query_nodes_geo failed: {err}")))?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit query_nodes_geo tx failed: {err}"))
        })?;

        Ok(rows
            .into_iter()
            .map(|r| NodesGeoRow {
                node_id: r.get("node_id"),
                type_name: r.get("type_name"),
                node_key: r.get("node_key"),
                title: r.get("title"),
                lng: r.get("lng"),
                lat: r.get("lat"),
                metric_value: r.get("metric_value"),
                open_alerts: r.get("open_alerts"),
                geo_shape: r
                    .try_get::<Option<serde_json::Value>, _>("geo_shape")
                    .ok()
                    .flatten()
                    .and_then(|v| serde_json::from_value(v).ok()),
            })
            .collect())
    }

    async fn query_nodes_geo_links(
        &self,
        tenant_id: &str,
        node_ids: &[Uuid],
        link_types: &[String],
    ) -> Result<Vec<NodesGeoLinkRow>, AppError> {
        if node_ids.is_empty() {
            return Ok(Vec::new());
        }
        let link_filter = normalize_type_names(link_types);
        if !link_types.is_empty() && link_filter.is_none() {
            return Ok(Vec::new());
        }

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let rows = sqlx::query(
            "SELECT l.id AS link_id,
                    l.link_type,
                    l.src_node_id,
                    l.dst_node_id,
                    src.geo_lng AS src_lng,
                    src.geo_lat AS src_lat,
                    dst.geo_lng AS dst_lng,
                    dst.geo_lat AS dst_lat,
                    l.weight
               FROM xb_links l
               JOIN xb_nodes src ON src.id = l.src_node_id AND src.tenant_id = l.tenant_id
               JOIN xb_nodes dst ON dst.id = l.dst_node_id AND dst.tenant_id = l.tenant_id
              WHERE l.tenant_id = $1
                AND l.src_node_id = ANY($2::uuid[])
                AND l.dst_node_id = ANY($2::uuid[])
                AND src.geo_lng IS NOT NULL
                AND src.geo_lat IS NOT NULL
                AND dst.geo_lng IS NOT NULL
                AND dst.geo_lat IS NOT NULL
                AND ($3::text[] IS NULL OR l.link_type = ANY($3))
              ORDER BY l.link_type ASC, l.id ASC",
        )
        .bind(tenant_id)
        .bind(node_ids)
        .bind(link_filter)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("query_nodes_geo_links failed: {err}")))?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit query_nodes_geo_links tx failed: {err}"))
        })?;

        Ok(rows
            .into_iter()
            .map(|r| NodesGeoLinkRow {
                link_id: r.get("link_id"),
                link_type: r.get("link_type"),
                src_node_id: r.get("src_node_id"),
                dst_node_id: r.get("dst_node_id"),
                src_lng: r.get("src_lng"),
                src_lat: r.get("src_lat"),
                dst_lng: r.get("dst_lng"),
                dst_lat: r.get("dst_lat"),
                weight: r.get("weight"),
            })
            .collect())
    }

    async fn query_floor_plan_rows(
        &self,
        tenant_id: &str,
        asset_id: Uuid,
        color_metric: Option<&str>,
    ) -> Result<Vec<FloorPlanRow>, AppError> {
        let metric_key = normalize_color_metric(color_metric);

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let rows = sqlx::query(
            "SELECT n.id AS node_id,
                    n.type_name,
                    n.node_key,
                    n.title,
                    p.x_ratio,
                    p.y_ratio,
                    CASE
                        WHEN $3::text IS NOT NULL AND jsonb_typeof(n.metrics -> $3) = 'number'
                        THEN (n.metrics ->> $3)::double precision
                        ELSE NULL
                    END AS metric_value,
                    (
                        SELECT COUNT(*)
                          FROM ax_alerts a
                         WHERE a.tenant_id = $1
                           AND a.node_id = n.id
                           AND a.status IN ('OPEN', 'ACK')
                    ) AS open_alerts
               FROM xv_floor_placements p
               JOIN xb_nodes n ON n.id = p.node_id AND n.tenant_id = p.tenant_id
              WHERE p.tenant_id = $1
                AND p.asset_id = $2
              ORDER BY n.type_name ASC, n.node_key ASC, n.id ASC",
        )
        .bind(tenant_id)
        .bind(asset_id)
        .bind(metric_key)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("query_floor_plan_rows failed: {err}")))?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit query_floor_plan_rows tx failed: {err}"))
        })?;

        Ok(rows
            .into_iter()
            .map(|r| FloorPlanRow {
                node_id: r.get("node_id"),
                type_name: r.get("type_name"),
                node_key: r.get("node_key"),
                title: r.get("title"),
                x_ratio: r.get("x_ratio"),
                y_ratio: r.get("y_ratio"),
                metric_value: r.get("metric_value"),
                open_alerts: r.get("open_alerts"),
            })
            .collect())
    }

    async fn query_scene3d_rows(
        &self,
        tenant_id: &str,
        asset_id: Uuid,
        color_metric: Option<&str>,
    ) -> Result<Vec<Scene3dRow>, AppError> {
        let metric_key = normalize_color_metric(color_metric);

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let rows = sqlx::query(
            "SELECT n.id AS node_id,
                    n.type_name,
                    n.node_key,
                    n.title,
                    b.mesh_name,
                    b.pos_x,
                    b.pos_y,
                    b.pos_z,
                    b.scale,
                    CASE
                        WHEN $3::text IS NOT NULL AND jsonb_typeof(n.metrics -> $3) = 'number'
                        THEN (n.metrics ->> $3)::double precision
                        ELSE NULL
                    END AS metric_value,
                    (
                        SELECT COUNT(*)
                          FROM ax_alerts a
                         WHERE a.tenant_id = $1
                           AND a.node_id = n.id
                           AND a.status IN ('OPEN', 'ACK')
                    ) AS open_alerts
               FROM xv_scene3d_bindings b
               JOIN xb_nodes n ON n.id = b.node_id AND n.tenant_id = b.tenant_id
              WHERE b.tenant_id = $1
                AND b.asset_id = $2
              ORDER BY n.type_name ASC, n.node_key ASC, n.id ASC",
        )
        .bind(tenant_id)
        .bind(asset_id)
        .bind(metric_key)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("query_scene3d_rows failed: {err}")))?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit query_scene3d_rows tx failed: {err}"))
        })?;

        Ok(rows
            .into_iter()
            .map(|r| Scene3dRow {
                node_id: r.get("node_id"),
                type_name: r.get("type_name"),
                node_key: r.get("node_key"),
                title: r.get("title"),
                mesh_name: r.get("mesh_name"),
                pos_x: r.get("pos_x"),
                pos_y: r.get("pos_y"),
                pos_z: r.get("pos_z"),
                scale: r.get("scale"),
                metric_value: r.get("metric_value"),
                open_alerts: r.get("open_alerts"),
            })
            .collect())
    }

    async fn list_widget_action_logs(
        &self,
        tenant_id: &str,
        widget_id: Uuid,
        limit: i64,
    ) -> Result<Vec<ActionLogRecord>, AppError> {
        if !(1..=500).contains(&limit) {
            return Err(AppError::bad(
                "LIMIT_OUT_OF_RANGE",
                format!("limit must be in 1..=500, got {limit}"),
            ));
        }

        let target_prefix = format!("widget:{widget_id}:%");
        let target_exact = widget_id.to_string();

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let rows = sqlx::query(
            "SELECT id, run_id, node_id, source_kind, action_kind,
                    target, payload, status, idempotency_key, error_message, created_at
               FROM ax_action_log
              WHERE tenant_id = $1
                AND source_kind = 'VIEW'
                AND (target LIKE $2 OR target = $3)
              ORDER BY created_at DESC, id DESC
              LIMIT $4",
        )
        .bind(tenant_id)
        .bind(target_prefix)
        .bind(target_exact)
        .bind(limit)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("list widget ax_action_log failed: {err}")))?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit list_widget_action_logs tx failed: {err}"))
        })?;
        rows.iter().map(decode_action_log_row).collect()
    }

    async fn execute_view_write_node(
        &self,
        tenant_id: &str,
        params: ViewWriteNodeParams,
    ) -> Result<ViewWriteNodeOutcome, AppError> {
        if params.idempotency_key.trim().is_empty() {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "idempotency_key must not be empty",
            ));
        }
        if params.key.trim().is_empty() {
            return Err(AppError::bad("INVALID_ARGUMENT", "key must not be empty"));
        }

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        if let Some(replay) = check_write_idempotency_tx(&mut tx, tenant_id, &params).await? {
            tx.commit().await.map_err(|err| {
                AppError::Database(format!("commit write_node replay tx failed: {err}"))
            })?;
            return Ok(replay);
        }

        let node_row = sqlx::query(
            "SELECT id, type_name, properties, metrics
               FROM xb_nodes
              WHERE tenant_id = $1 AND id = $2
              FOR UPDATE",
        )
        .bind(tenant_id)
        .bind(params.node_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("lock xb_nodes failed: {err}")))?
        .ok_or(AppError::NotFound("NODE_NOT_FOUND"))?;

        let type_name: String = node_row.get("type_name");
        let props = match node_row.get::<serde_json::Value, _>("properties") {
            serde_json::Value::Object(map) => map,
            _ => serde_json::Map::new(),
        };
        let metrics = match node_row.get::<serde_json::Value, _>("metrics") {
            serde_json::Value::Object(map) => map,
            _ => serde_json::Map::new(),
        };

        validate_node_dept_abac(&props, &params.caller_dept_id)?;
        let metric_val =
            validate_write_against_schema_tx(&mut tx, tenant_id, &type_name, &params).await?;

        let Some(action_log_id) =
            try_insert_view_write_log_tx(&mut tx, tenant_id, &type_name, &params).await?
        else {
            tx.rollback().await.map_err(|err| {
                AppError::Database(format!("rollback write_node conflict tx failed: {err}"))
            })?;
            return resolve_conflicted_write_replay(self, tenant_id, &params).await;
        };

        apply_node_write_mutation_tx(&mut tx, tenant_id, &params, props, metrics, metric_val)
            .await?;
        emit_view_write_outbox_tx(&mut tx, tenant_id, &type_name, &params).await?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit execute_view_write_node tx failed: {err}"))
        })?;

        Ok(ViewWriteNodeOutcome {
            node_id: params.node_id,
            type_name,
            action_log_id,
            idempotency_replay: false,
        })
    }

    async fn list_export_templates(
        &self,
        tenant_id: &str,
        query: SimplePageQuery,
    ) -> Result<PaginatedList<ExportTemplateRecord>, AppError> {
        let (limit_usize, fetch_limit) = validate_page_limit(query.limit, 50)?;
        let (cursor_ts, cursor_id) = parse_optional_cursor(query.cursor.as_deref())?;

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let rows = sqlx::query(
            "SELECT id, tenant_id, name, export_format, file_path, sha256,
                    placeholders, created_by, created_at
               FROM xv_export_templates
              WHERE tenant_id = $1
                AND ($2::timestamptz IS NULL OR (created_at, id) < ($2, $3::uuid))
              ORDER BY created_at DESC, id DESC
              LIMIT $4",
        )
        .bind(tenant_id)
        .bind(cursor_ts)
        .bind(cursor_id)
        .bind(fetch_limit)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("list xv_export_templates failed: {err}")))?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit list_export_templates tx failed: {err}"))
        })?;

        let mut items = Vec::with_capacity(rows.len());
        for row in &rows {
            items.push(decode_export_template_row(row)?);
        }
        let next_cursor = paginate_ts_uuid_items(&mut items, limit_usize, |r| (r.created_at, r.id));
        Ok(PaginatedList { items, next_cursor })
    }

    async fn create_export_template(
        &self,
        tenant_id: &str,
        params: CreateExportTemplateParams,
    ) -> Result<ExportTemplateRecord, AppError> {
        let trimmed_name = params.name.trim();
        if trimmed_name.is_empty() {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "template name must not be empty",
            ));
        }
        if !matches!(
            params.export_format,
            ExportFormat::Docx | ExportFormat::Pptx
        ) {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "export template format must be DOCX or PPTX",
            ));
        }

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let row = sqlx::query(
            "INSERT INTO xv_export_templates (
                 id, tenant_id, name, export_format, file_path, sha256, placeholders, created_by
             )
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
             RETURNING id, tenant_id, name, export_format, file_path, sha256,
                       placeholders, created_by, created_at",
        )
        .bind(params.id)
        .bind(tenant_id)
        .bind(trimmed_name)
        .bind(params.export_format.as_str())
        .bind(&params.file_path)
        .bind(&params.sha256)
        .bind(&params.placeholders)
        .bind(params.created_by)
        .fetch_one(&mut *tx)
        .await
        .map_err(|err| {
            if is_unique_violation(&err) {
                AppError::Conflict("CONFLICT")
            } else {
                AppError::Database(format!("insert xv_export_templates failed: {err}"))
            }
        })?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit create_export_template tx failed: {err}"))
        })?;
        decode_export_template_row(&row)
    }

    async fn get_export_template(
        &self,
        tenant_id: &str,
        template_id: Uuid,
    ) -> Result<Option<ExportTemplateRecord>, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let maybe_row = sqlx::query(
            "SELECT id, tenant_id, name, export_format, file_path, sha256,
                    placeholders, created_by, created_at
               FROM xv_export_templates
              WHERE tenant_id = $1 AND id = $2",
        )
        .bind(tenant_id)
        .bind(template_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("get xv_export_templates failed: {err}")))?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit get_export_template tx failed: {err}"))
        })?;
        maybe_row
            .as_ref()
            .map(decode_export_template_row)
            .transpose()
    }

    async fn delete_export_template(
        &self,
        tenant_id: &str,
        template_id: Uuid,
    ) -> Result<Option<ExportTemplateRecord>, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let maybe_row = sqlx::query(
            "DELETE FROM xv_export_templates
              WHERE tenant_id = $1 AND id = $2
              RETURNING id, tenant_id, name, export_format, file_path, sha256,
                        placeholders, created_by, created_at",
        )
        .bind(tenant_id)
        .bind(template_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("delete xv_export_templates failed: {err}")))?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit delete_export_template tx failed: {err}"))
        })?;
        maybe_row
            .as_ref()
            .map(decode_export_template_row)
            .transpose()
    }

    async fn create_export_job(
        &self,
        tenant_id: &str,
        params: CreateExportJobParams,
    ) -> Result<ExportJobRecord, AppError> {
        let source_json = serde_json::to_value(&params.source)
            .map_err(|err| AppError::bad("INVALID_ARGUMENT", format!("serialize source: {err}")))?;
        let options_json = serde_json::to_value(&params.options).map_err(|err| {
            AppError::bad("INVALID_ARGUMENT", format!("serialize options: {err}"))
        })?;

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let export_id: Uuid = sqlx::query_scalar(
            "INSERT INTO xv_export_jobs (
                 tenant_id, export_format, source, options, template_id, status, created_by
             )
             VALUES ($1, $2, $3, $4, $5, 'PENDING', $6)
             RETURNING id",
        )
        .bind(tenant_id)
        .bind(params.export_format.as_str())
        .bind(source_json)
        .bind(options_json)
        .bind(params.template_id)
        .bind(params.created_by)
        .fetch_one(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("insert xv_export_jobs failed: {err}")))?;

        let mut job_spec = NewJob::new(
            JobKind::Export,
            serde_json::json!({ "export_id": export_id }),
        )
        .with_idempotency_key(export_id.to_string());
        job_spec.priority = 50;
        job_spec.max_attempts = 3;
        let job_id = crate::jobs::enqueue_job_tx(&mut tx, tenant_id, &job_spec).await?;

        let row = sqlx::query(
            "UPDATE xv_export_jobs
                SET job_id = $3
              WHERE tenant_id = $1 AND id = $2
              RETURNING id, tenant_id, export_format, source, options, template_id, job_id,
                        status, file_name, file_path, file_size, sha256, error_message,
                        created_by, created_at, finished_at, expires_at",
        )
        .bind(tenant_id)
        .bind(export_id)
        .bind(job_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("link xv_export_jobs.job_id failed: {err}")))?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit create_export_job tx failed: {err}"))
        })?;
        decode_export_job_row(&row)
    }

    async fn get_export_job(
        &self,
        tenant_id: &str,
        export_id: Uuid,
    ) -> Result<Option<ExportJobRecord>, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let maybe_row = sqlx::query(
            "SELECT id, tenant_id, export_format, source, options, template_id, job_id,
                    status, file_name, file_path, file_size, sha256, error_message,
                    created_by, created_at, finished_at, expires_at
               FROM xv_export_jobs
              WHERE tenant_id = $1 AND id = $2",
        )
        .bind(tenant_id)
        .bind(export_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("get xv_export_jobs failed: {err}")))?;

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit get_export_job tx failed: {err}")))?;
        maybe_row.as_ref().map(decode_export_job_row).transpose()
    }

    async fn list_export_jobs(
        &self,
        tenant_id: &str,
        created_by_filter: Option<Uuid>,
        query: ExportListQuery,
    ) -> Result<PaginatedList<ExportJobRecord>, AppError> {
        let (limit_usize, fetch_limit) = validate_i64_page_limit(query.limit, 50, 200)?;
        let (cursor_ts, cursor_id) = parse_optional_cursor(query.cursor.as_deref())?;

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let rows = sqlx::query(
            "SELECT id, tenant_id, export_format, source, options, template_id, job_id,
                    status, file_name, file_path, file_size, sha256, error_message,
                    created_by, created_at, finished_at, expires_at
               FROM xv_export_jobs
              WHERE tenant_id = $1
                AND ($2::uuid IS NULL OR created_by = $2)
                AND ($3::timestamptz IS NULL OR (created_at, id) < ($3, $4::uuid))
              ORDER BY created_at DESC, id DESC
              LIMIT $5",
        )
        .bind(tenant_id)
        .bind(created_by_filter)
        .bind(cursor_ts)
        .bind(cursor_id)
        .bind(fetch_limit)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("list xv_export_jobs failed: {err}")))?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit list_export_jobs tx failed: {err}"))
        })?;

        let mut items = Vec::with_capacity(rows.len());
        for row in &rows {
            items.push(decode_export_job_row(row)?);
        }
        let next_cursor = paginate_ts_uuid_items(&mut items, limit_usize, |r| (r.created_at, r.id));
        Ok(PaginatedList { items, next_cursor })
    }

    async fn mark_export_job_running(
        &self,
        tenant_id: &str,
        export_id: Uuid,
    ) -> Result<Option<ExportJobRecord>, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let maybe_row = sqlx::query(
            "UPDATE xv_export_jobs
                SET status = 'RUNNING',
                    error_message = NULL
              WHERE tenant_id = $1
                AND id = $2
                AND status IN ('PENDING', 'RUNNING')
              RETURNING id, tenant_id, export_format, source, options, template_id, job_id,
                        status, file_name, file_path, file_size, sha256, error_message,
                        created_by, created_at, finished_at, expires_at",
        )
        .bind(tenant_id)
        .bind(export_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("mark xv_export_jobs RUNNING failed: {err}")))?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit mark_export_job_running tx failed: {err}"))
        })?;
        maybe_row.as_ref().map(decode_export_job_row).transpose()
    }

    async fn complete_export_job(
        &self,
        tenant_id: &str,
        export_id: Uuid,
        params: CompleteExportJobParams,
    ) -> Result<ExportJobRecord, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let maybe_row = sqlx::query(
            "UPDATE xv_export_jobs
                SET status = 'SUCCEEDED',
                    file_name = $3,
                    file_path = $4,
                    file_size = $5,
                    sha256 = $6,
                    error_message = NULL,
                    finished_at = NOW(),
                    expires_at = $7
              WHERE tenant_id = $1 AND id = $2
              RETURNING id, tenant_id, export_format, source, options, template_id, job_id,
                        status, file_name, file_path, file_size, sha256, error_message,
                        created_by, created_at, finished_at, expires_at",
        )
        .bind(tenant_id)
        .bind(export_id)
        .bind(&params.file_name)
        .bind(&params.file_path)
        .bind(params.file_size)
        .bind(&params.sha256)
        .bind(params.expires_at)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| {
            AppError::Database(format!("complete xv_export_jobs SUCCEEDED failed: {err}"))
        })?;

        let row = maybe_row.ok_or(AppError::NotFound("EXPORT_NOT_FOUND"))?;
        let record = decode_export_job_row(&row)?;

        let outbox_event = NewOutboxEvent {
            topic: OutboxTopic::ExportFinished,
            payload: serde_json::json!({
                "export_id": export_id,
                "aggregate_id": export_id.to_string(),
                "export_format": record.export_format.as_str(),
                "file_name": record.file_name,
                "file_size": record.file_size,
                "sha256": record.sha256,
            }),
        };
        let _ = insert_outbox_tx(&mut tx, tenant_id, &outbox_event).await?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit complete_export_job tx failed: {err}"))
        })?;
        Ok(record)
    }

    async fn fail_export_job(
        &self,
        tenant_id: &str,
        export_id: Uuid,
        error_message: &str,
    ) -> Result<(), AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        sqlx::query(
            "UPDATE xv_export_jobs
                SET status = 'FAILED',
                    error_message = $3,
                    finished_at = NOW()
              WHERE tenant_id = $1 AND id = $2",
        )
        .bind(tenant_id)
        .bind(export_id)
        .bind(error_message)
        .execute(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("mark xv_export_jobs FAILED failed: {err}")))?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit fail_export_job tx failed: {err}"))
        })?;
        Ok(())
    }

    async fn delete_export_job(
        &self,
        tenant_id: &str,
        export_id: Uuid,
    ) -> Result<Option<ExportJobRecord>, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let maybe_row = sqlx::query(
            "DELETE FROM xv_export_jobs
              WHERE tenant_id = $1 AND id = $2
              RETURNING id, tenant_id, export_format, source, options, template_id, job_id,
                        status, file_name, file_path, file_size, sha256, error_message,
                        created_by, created_at, finished_at, expires_at",
        )
        .bind(tenant_id)
        .bind(export_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("delete xv_export_jobs failed: {err}")))?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit delete_export_job tx failed: {err}"))
        })?;
        maybe_row.as_ref().map(decode_export_job_row).transpose()
    }

    async fn get_export_keep_days(&self, tenant_id: &str) -> Result<i32, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let maybe_days: Option<i32> = sqlx::query_scalar(
            "SELECT keep_days
               FROM ax_retention_policies
              WHERE tenant_id = $1 AND target = 'EXPORT_FILE'",
        )
        .bind(tenant_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("query EXPORT_FILE keep_days failed: {err}")))?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit get_export_keep_days tx failed: {err}"))
        })?;
        Ok(maybe_days.unwrap_or(30).max(1))
    }

    async fn create_share_link(
        &self,
        tenant_id: &str,
        params: CreateShareLinkParams,
    ) -> Result<ShareLinkRecord, AppError> {
        let locked_json = serde_json::to_value(&params.locked_filters).map_err(|err| {
            AppError::bad(
                "INVALID_ARGUMENT",
                format!("serialize locked_filters: {err}"),
            )
        })?;

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let row = sqlx::query(
            "INSERT INTO xv_share_links (
                 tenant_id, token_prefix, token_hash, target_kind, target_id,
                 locked_filters, allow_filters, allow_export, password_hash,
                 expires_at, max_views, created_by
             )
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
             RETURNING id, tenant_id, token_prefix, target_kind, target_id,
                       locked_filters, allow_filters, allow_export,
                       (password_hash IS NOT NULL) AS has_password,
                       expires_at, max_views, view_count, created_by, created_at, revoked_at",
        )
        .bind(tenant_id)
        .bind(&params.token_prefix)
        .bind(&params.token_hash)
        .bind(params.target_kind.as_str())
        .bind(params.target_id)
        .bind(locked_json)
        .bind(params.allow_filters)
        .bind(params.allow_export)
        .bind(&params.password_hash)
        .bind(params.expires_at)
        .bind(params.max_views)
        .bind(params.created_by)
        .fetch_one(&mut *tx)
        .await
        .map_err(|err| {
            if is_unique_violation(&err) {
                AppError::Conflict("CONFLICT")
            } else {
                AppError::Database(format!("insert xv_share_links failed: {err}"))
            }
        })?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit create_share_link tx failed: {err}"))
        })?;
        decode_share_link_row(&row)
    }

    async fn list_share_links(
        &self,
        tenant_id: &str,
        query: ShareListQuery,
    ) -> Result<PaginatedList<ShareLinkRecord>, AppError> {
        let (limit_usize, fetch_limit) = validate_i64_page_limit(query.limit, 50, 200)?;
        let (cursor_ts, cursor_id) = parse_optional_cursor(query.cursor.as_deref())?;

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let rows = sqlx::query(
            "SELECT id, tenant_id, token_prefix, target_kind, target_id,
                    locked_filters, allow_filters, allow_export,
                    (password_hash IS NOT NULL) AS has_password,
                    expires_at, max_views, view_count, created_by, created_at, revoked_at
               FROM xv_share_links
              WHERE tenant_id = $1
                AND ($2::uuid IS NULL OR target_id = $2)
                AND ($3::timestamptz IS NULL OR (created_at, id) < ($3, $4::uuid))
              ORDER BY created_at DESC, id DESC
              LIMIT $5",
        )
        .bind(tenant_id)
        .bind(query.target_id)
        .bind(cursor_ts)
        .bind(cursor_id)
        .bind(fetch_limit)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("list xv_share_links failed: {err}")))?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit list_share_links tx failed: {err}"))
        })?;

        let mut items = Vec::with_capacity(rows.len());
        for row in &rows {
            items.push(decode_share_link_row(row)?);
        }
        let next_cursor = paginate_ts_uuid_items(&mut items, limit_usize, |r| (r.created_at, r.id));
        Ok(PaginatedList { items, next_cursor })
    }

    async fn revoke_share_link(&self, tenant_id: &str, share_id: Uuid) -> Result<bool, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let res = sqlx::query(
            "UPDATE xv_share_links
                SET revoked_at = NOW()
              WHERE tenant_id = $1
                AND id = $2
                AND revoked_at IS NULL",
        )
        .bind(tenant_id)
        .bind(share_id)
        .execute(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("revoke xv_share_links failed: {err}")))?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit revoke_share_link tx failed: {err}"))
        })?;
        Ok(res.rows_affected() > 0)
    }

    async fn resolve_share_by_hash(
        &self,
        token_hash: &str,
    ) -> Result<Option<ResolvedShare>, AppError> {
        let maybe_fn_row = sqlx::query(
            "SELECT share_id, tenant_id, target_kind, target_id, created_by,
                    locked_filters, allow_filters, allow_export, password_hash,
                    max_views, view_count
               FROM ax_resolve_share($1)",
        )
        .bind(token_hash)
        .fetch_optional(self.pool())
        .await
        .map_err(|err| AppError::Database(format!("ax_resolve_share failed: {err}")))?;

        let Some(fn_row) = maybe_fn_row else {
            return Ok(None);
        };

        let share_id: Uuid = fn_row.get("share_id");
        let tenant_id: String = fn_row.get("tenant_id");
        let target_kind_raw: String = fn_row.get("target_kind");
        let target_kind = ShareTargetKind::from_str(&target_kind_raw)?;
        let locked_filters: BTreeMap<String, serde_json::Value> =
            serde_json::from_value(fn_row.get("locked_filters")).map_err(|err| {
                AppError::Database(format!("decode ax_resolve_share.locked_filters: {err}"))
            })?;

        let mut tx = begin_tenant_tx(self.pool(), &tenant_id).await?;
        let maybe_extra = sqlx::query(
            "SELECT token_prefix, expires_at
               FROM xv_share_links
              WHERE tenant_id = $1
                AND id = $2
                AND revoked_at IS NULL
                AND expires_at > NOW()",
        )
        .bind(&tenant_id)
        .bind(share_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("load xv_share_links metadata failed: {err}")))?;
        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit resolve_share_by_hash tx failed: {err}"))
        })?;

        let Some(extra) = maybe_extra else {
            return Ok(None);
        };

        Ok(Some(ResolvedShare {
            share_id,
            tenant_id,
            target_kind,
            target_id: fn_row.get("target_id"),
            created_by: fn_row.get("created_by"),
            locked_filters,
            allow_filters: fn_row.get("allow_filters"),
            allow_export: fn_row.get("allow_export"),
            password_hash: fn_row.get("password_hash"),
            max_views: fn_row.get("max_views"),
            view_count: fn_row.get("view_count"),
            token_prefix: extra.get("token_prefix"),
            expires_at: extra.get("expires_at"),
        }))
    }

    async fn increment_share_view_count(
        &self,
        tenant_id: &str,
        share_id: Uuid,
    ) -> Result<bool, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let maybe_id: Option<Uuid> = sqlx::query_scalar(
            "UPDATE xv_share_links
                SET view_count = view_count + 1
              WHERE tenant_id = $1
                AND id = $2
                AND revoked_at IS NULL
                AND expires_at > NOW()
                AND (max_views IS NULL OR view_count < max_views)
              RETURNING id",
        )
        .bind(tenant_id)
        .bind(share_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| {
            AppError::Database(format!("increment xv_share_links.view_count failed: {err}"))
        })?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!(
                "commit increment_share_view_count tx failed: {err}"
            ))
        })?;
        Ok(maybe_id.is_some())
    }

    async fn list_saved_charts(
        &self,
        tenant_id: &str,
        viewer_user_id: Uuid,
        include_all: bool,
        query: SimplePageQuery,
    ) -> Result<PaginatedList<SavedChartRecord>, AppError> {
        let (limit_usize, fetch_limit) = validate_page_limit(query.limit, 50)?;
        let (cursor_ts, cursor_id) = parse_optional_cursor(query.cursor.as_deref())?;

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let rows = sqlx::query(
            "SELECT id, tenant_id, name, description, widget_kind, data_source, encoding,
                    options, owner_dept, sensitivity, shared, created_by, created_at, updated_at
               FROM xv_charts
              WHERE tenant_id = $1
                AND ($2 OR shared = TRUE OR created_by = $3)
                AND ($4::timestamptz IS NULL OR (updated_at, id) < ($4, $5::uuid))
              ORDER BY updated_at DESC, id DESC
              LIMIT $6",
        )
        .bind(tenant_id)
        .bind(include_all)
        .bind(viewer_user_id)
        .bind(cursor_ts)
        .bind(cursor_id)
        .bind(fetch_limit)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("list xv_charts failed: {err}")))?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit list_saved_charts tx failed: {err}"))
        })?;

        let mut items = Vec::with_capacity(rows.len());
        for row in &rows {
            items.push(decode_saved_chart_row(row)?);
        }
        let next_cursor = paginate_ts_uuid_items(&mut items, limit_usize, |r| (r.updated_at, r.id));
        Ok(PaginatedList { items, next_cursor })
    }

    async fn get_saved_chart(
        &self,
        tenant_id: &str,
        chart_id: Uuid,
    ) -> Result<Option<SavedChartRecord>, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let maybe_row = fetch_saved_chart_row_tx(&mut tx, tenant_id, chart_id).await?;
        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit get_saved_chart tx failed: {err}"))
        })?;
        maybe_row.as_ref().map(decode_saved_chart_row).transpose()
    }

    async fn create_saved_chart(
        &self,
        tenant_id: &str,
        created_by: Option<Uuid>,
        owner_dept: &str,
        req: SaveChartRequest,
    ) -> Result<SavedChartRecord, AppError> {
        let trimmed_name = validate_chart_name(&req.name)?;
        ensure_json_object("encoding", &req.encoding)?;
        ensure_json_object("options", &req.options)?;
        let ds_json = serde_json::to_value(&req.data_source).map_err(|err| {
            AppError::bad("INVALID_ARGUMENT", format!("serialize data_source: {err}"))
        })?;
        let dept = normalize_owner_dept(owner_dept);

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let row = sqlx::query(
            "INSERT INTO xv_charts (
                 tenant_id, name, description, widget_kind, data_source, encoding,
                 options, owner_dept, sensitivity, shared, created_by
             )
             VALUES (
                 $1, $2, $3, $4, $5, $6,
                 $7, $8, $9, $10, (SELECT id FROM sec_users WHERE id = $11)
             )
             RETURNING id, tenant_id, name, description, widget_kind, data_source, encoding,
                       options, owner_dept, sensitivity, shared, created_by, created_at, updated_at",
        )
        .bind(tenant_id)
        .bind(trimmed_name)
        .bind(req.description.trim())
        .bind(req.widget_kind.as_str())
        .bind(ds_json)
        .bind(&req.encoding)
        .bind(&req.options)
        .bind(dept)
        .bind(req.sensitivity.as_str())
        .bind(req.shared)
        .bind(created_by)
        .fetch_one(&mut *tx)
        .await
        .map_err(|err| {
            if is_unique_violation(&err) {
                AppError::Conflict("CONFLICT")
            } else {
                AppError::Database(format!("insert xv_charts failed: {err}"))
            }
        })?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit create_saved_chart tx failed: {err}"))
        })?;
        decode_saved_chart_row(&row)
    }

    async fn update_saved_chart(
        &self,
        tenant_id: &str,
        chart_id: Uuid,
        req: UpdateSavedChartRequest,
    ) -> Result<Option<SavedChartRecord>, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let Some(existing_row) = fetch_saved_chart_row_tx(&mut tx, tenant_id, chart_id).await?
        else {
            tx.commit().await.map_err(|err| {
                AppError::Database(format!("commit update_saved_chart empty tx failed: {err}"))
            })?;
            return Ok(None);
        };
        let existing = decode_saved_chart_row(&existing_row)?;
        let row = execute_update_saved_chart_tx(&mut tx, tenant_id, &existing, req).await?;
        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit update_saved_chart tx failed: {err}"))
        })?;
        Ok(Some(decode_saved_chart_row(&row)?))
    }

    async fn delete_saved_chart(&self, tenant_id: &str, chart_id: Uuid) -> Result<bool, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let res = sqlx::query("DELETE FROM xv_charts WHERE tenant_id = $1 AND id = $2")
            .bind(tenant_id)
            .bind(chart_id)
            .execute(&mut *tx)
            .await
            .map_err(|err| AppError::Database(format!("delete xv_charts failed: {err}")))?;
        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit delete_saved_chart tx failed: {err}"))
        })?;
        Ok(res.rows_affected() > 0)
    }

    async fn pin_saved_chart_to_dashboard(
        &self,
        tenant_id: &str,
        chart_id: Uuid,
        req: PinChartToDashboardRequest,
        deps: WidgetDeps,
    ) -> Result<PinChartToDashboardResponse, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let res = execute_pin_saved_chart_tx(&mut tx, tenant_id, chart_id, req, deps).await?;
        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit pin_saved_chart_to_dashboard tx: {err}"))
        })?;
        Ok(res)
    }
}

fn validate_i64_page_limit(
    raw_limit: Option<i64>,
    default_limit: i64,
    max_limit: i64,
) -> Result<(usize, i64), AppError> {
    let limit = raw_limit.unwrap_or(default_limit);
    if !(1..=max_limit).contains(&limit) {
        return Err(AppError::bad(
            "LIMIT_OUT_OF_RANGE",
            format!("limit must be in 1..={max_limit}, got {limit}"),
        ));
    }
    let limit_usize = usize::try_from(limit).unwrap_or(50);
    let fetch_limit = limit.saturating_add(1);
    Ok((limit_usize, fetch_limit))
}

fn parse_optional_cursor(
    raw_cursor: Option<&str>,
) -> Result<(Option<DateTime<Utc>>, Option<Uuid>), AppError> {
    match raw_cursor.map(str::trim).filter(|s| !s.is_empty()) {
        Some(raw) => {
            let (ts, cid) = parse_ts_uuid_cursor(raw)?;
            Ok((Some(ts), Some(cid)))
        }
        None => Ok((None, None)),
    }
}

fn paginate_ts_uuid_items<T>(
    items: &mut Vec<T>,
    limit_usize: usize,
    key_fn: impl Fn(&T) -> (DateTime<Utc>, Uuid),
) -> Option<String> {
    if items.len() > limit_usize {
        items.truncate(limit_usize);
        items.last().map(|last| {
            let (ts, id) = key_fn(last);
            encode_ts_uuid_cursor(ts, id)
        })
    } else {
        None
    }
}

fn decode_export_template_row(
    row: &sqlx::postgres::PgRow,
) -> Result<ExportTemplateRecord, AppError> {
    let fmt_raw: String = row.get("export_format");
    let export_format = ExportFormat::from_str(&fmt_raw)?;
    Ok(ExportTemplateRecord {
        id: row.get("id"),
        tenant_id: row.get("tenant_id"),
        name: row.get("name"),
        export_format,
        file_path: row.get("file_path"),
        sha256: row.get("sha256"),
        placeholders: row.get("placeholders"),
        created_by: row.get("created_by"),
        created_at: row.get("created_at"),
    })
}

fn decode_export_job_row(row: &sqlx::postgres::PgRow) -> Result<ExportJobRecord, AppError> {
    let fmt_raw: String = row.get("export_format");
    let export_format = ExportFormat::from_str(&fmt_raw)?;
    let status_raw: String = row.get("status");
    let status = JobStatus::from_str(&status_raw)?;
    let source: ExportSource = serde_json::from_value(row.get("source"))
        .map_err(|err| AppError::Database(format!("decode xv_export_jobs.source failed: {err}")))?;
    let options: ExportOptions = serde_json::from_value(row.get("options")).map_err(|err| {
        AppError::Database(format!("decode xv_export_jobs.options failed: {err}"))
    })?;

    Ok(ExportJobRecord {
        id: row.get("id"),
        tenant_id: row.get("tenant_id"),
        export_format,
        source,
        options,
        template_id: row.get("template_id"),
        job_id: row.get("job_id"),
        status,
        file_name: row.get("file_name"),
        file_path: row.get("file_path"),
        file_size: row.get("file_size"),
        sha256: row.get("sha256"),
        error_message: row.get("error_message"),
        created_by: row.get("created_by"),
        created_at: row.get("created_at"),
        finished_at: row.get("finished_at"),
        expires_at: row.get("expires_at"),
    })
}

fn decode_share_link_row(row: &sqlx::postgres::PgRow) -> Result<ShareLinkRecord, AppError> {
    let kind_raw: String = row.get("target_kind");
    let target_kind = ShareTargetKind::from_str(&kind_raw)?;
    let locked_filters: BTreeMap<String, serde_json::Value> =
        serde_json::from_value(row.get("locked_filters")).map_err(|err| {
            AppError::Database(format!("decode xv_share_links.locked_filters: {err}"))
        })?;

    Ok(ShareLinkRecord {
        id: row.get("id"),
        tenant_id: row.get("tenant_id"),
        token_prefix: row.get("token_prefix"),
        target_kind,
        target_id: row.get("target_id"),
        locked_filters,
        allow_filters: row.get("allow_filters"),
        allow_export: row.get("allow_export"),
        has_password: row.get("has_password"),
        expires_at: row.get("expires_at"),
        max_views: row.get("max_views"),
        view_count: row.get("view_count"),
        created_by: row.get("created_by"),
        created_at: row.get("created_at"),
        revoked_at: row.get("revoked_at"),
    })
}

async fn resolve_conflicted_write_replay(
    store: &PgStore,
    tenant_id: &str,
    params: &ViewWriteNodeParams,
) -> Result<ViewWriteNodeOutcome, AppError> {
    let mut replay_tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let maybe_replay = check_write_idempotency_tx(&mut replay_tx, tenant_id, params).await?;
    replay_tx.commit().await.map_err(|err| {
        AppError::Database(format!(
            "commit conflicted write_node replay tx failed: {err}"
        ))
    })?;
    maybe_replay.ok_or(AppError::Conflict("CONFLICT"))
}

impl PgStore {
    /// Atomically updates `xv_dashboards.canvas` for `dashboard_id` and optionally replaces its
    /// widgets with generated preset slots inside a single tenant transaction (`06-STEP20` §20.3.1).
    ///
    /// # Errors
    /// Returns `400 INVALID_ARGUMENT`, `404 DASHBOARD_NOT_FOUND`, or [`AppError::Database`].
    pub async fn apply_dashboard_zone_preset(
        &self,
        tenant_id: &str,
        dashboard_id: Uuid,
        canvas: CanvasConfig,
        replace_widgets: Option<Vec<CreateWidgetRecordParams>>,
    ) -> Result<DashboardDetail, AppError> {
        validate_canvas_config(&canvas)?;
        if let Some(ref slots) = replace_widgets {
            for slot in slots {
                validate_widget_position(&slot.position)?;
                validate_refresh_seconds(slot.refresh_seconds)?;
            }
        }
        let canvas_json = serde_json::to_value(&canvas)
            .map_err(|err| AppError::bad("INVALID_ARGUMENT", format!("serialize canvas: {err}")))?;

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let maybe_row = sqlx::query(
            "UPDATE xv_dashboards
                SET canvas = $3,
                    updated_at = NOW()
              WHERE tenant_id = $1 AND id = $2
              RETURNING id, tenant_id, name, slug, description, filters, theme, canvas,
                        status, version, created_by, created_at, updated_at",
        )
        .bind(tenant_id)
        .bind(dashboard_id)
        .bind(canvas_json)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| {
            AppError::Database(format!("update xv_dashboards zone preset failed: {err}"))
        })?;

        let row = maybe_row.ok_or(AppError::NotFound("DASHBOARD_NOT_FOUND"))?;
        let dashboard = decode_dashboard_row(&row)?;

        let widgets = if let Some(slots) = replace_widgets {
            sqlx::query("DELETE FROM xv_widgets WHERE tenant_id = $1 AND dashboard_id = $2")
                .bind(tenant_id)
                .bind(dashboard_id)
                .execute(&mut *tx)
                .await
                .map_err(|err| {
                    AppError::Database(format!("clear xv_widgets before zone preset failed: {err}"))
                })?;

            let mut created = Vec::with_capacity(slots.len());
            for mut slot in slots {
                slot.dashboard_id = dashboard_id;
                created.push(insert_widget_tx(&mut tx, tenant_id, None, &slot).await?);
            }
            created
        } else {
            fetch_dashboard_widgets_tx(&mut tx, tenant_id, dashboard_id).await?
        };

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit apply_dashboard_zone_preset tx: {err}"))
        })?;

        Ok(DashboardDetail { dashboard, widgets })
    }
}

fn validate_chart_name(raw: &str) -> Result<&str, AppError> {
    let trimmed = raw.trim();
    let char_count = trimmed.chars().count();
    if !(1..=128).contains(&char_count) {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            format!("chart name length must be in 1..=128 characters, got {char_count}"),
        ));
    }
    Ok(trimmed)
}

fn ensure_json_object(field: &str, val: &serde_json::Value) -> Result<(), AppError> {
    if !val.is_object() {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            format!("{field} must be a JSON object"),
        ));
    }
    Ok(())
}

fn normalize_owner_dept(raw: &str) -> &str {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        "default"
    } else {
        trimmed
    }
}

fn decode_saved_chart_row(row: &sqlx::postgres::PgRow) -> Result<SavedChartRecord, AppError> {
    let kind_raw: String = row.get("widget_kind");
    let widget_kind = WidgetKind::from_str(&kind_raw)?;
    let sens_raw: String = row.get("sensitivity");
    let sensitivity = Sensitivity::from_str(&sens_raw)?;
    let data_source: WidgetDataSource = serde_json::from_value(row.get("data_source"))
        .map_err(|err| AppError::Database(format!("decode xv_charts.data_source failed: {err}")))?;

    Ok(SavedChartRecord {
        id: row.get("id"),
        tenant_id: row.get("tenant_id"),
        name: row.get("name"),
        description: row.get("description"),
        widget_kind,
        data_source,
        encoding: row.get("encoding"),
        options: row.get("options"),
        owner_dept: row.get("owner_dept"),
        sensitivity,
        shared: row.get("shared"),
        created_by: row.get("created_by"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

async fn fetch_saved_chart_row_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    chart_id: Uuid,
) -> Result<Option<sqlx::postgres::PgRow>, AppError> {
    sqlx::query(
        "SELECT id, tenant_id, name, description, widget_kind, data_source, encoding,
                options, owner_dept, sensitivity, shared, created_by, created_at, updated_at
           FROM xv_charts
          WHERE tenant_id = $1 AND id = $2",
    )
    .bind(tenant_id)
    .bind(chart_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("fetch xv_charts row failed: {err}")))
}

async fn execute_update_saved_chart_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    existing: &SavedChartRecord,
    req: UpdateSavedChartRequest,
) -> Result<sqlx::postgres::PgRow, AppError> {
    let name = match req.name {
        Some(ref raw) => String::from(validate_chart_name(raw)?),
        None => existing.name.clone(),
    };
    let description = req
        .description
        .map_or_else(|| existing.description.clone(), |d| String::from(d.trim()));
    let widget_kind = req.widget_kind.unwrap_or(existing.widget_kind);
    let data_source = req
        .data_source
        .unwrap_or_else(|| existing.data_source.clone());
    let encoding = req.encoding.unwrap_or_else(|| existing.encoding.clone());
    let options = req.options.unwrap_or_else(|| existing.options.clone());
    ensure_json_object("encoding", &encoding)?;
    ensure_json_object("options", &options)?;
    let sensitivity = req.sensitivity.unwrap_or(existing.sensitivity);
    let shared = req.shared.unwrap_or(existing.shared);
    let ds_json = serde_json::to_value(&data_source).map_err(|err| {
        AppError::bad("INVALID_ARGUMENT", format!("serialize data_source: {err}"))
    })?;

    sqlx::query(
        "UPDATE xv_charts
            SET name = $3,
                description = $4,
                widget_kind = $5,
                data_source = $6,
                encoding = $7,
                options = $8,
                sensitivity = $9,
                shared = $10,
                updated_at = NOW()
          WHERE tenant_id = $1 AND id = $2
          RETURNING id, tenant_id, name, description, widget_kind, data_source, encoding,
                    options, owner_dept, sensitivity, shared, created_by, created_at, updated_at",
    )
    .bind(tenant_id)
    .bind(existing.id)
    .bind(name)
    .bind(description)
    .bind(widget_kind.as_str())
    .bind(ds_json)
    .bind(encoding)
    .bind(options)
    .bind(sensitivity.as_str())
    .bind(shared)
    .fetch_one(&mut **tx)
    .await
    .map_err(|err| {
        if is_unique_violation(&err) {
            AppError::Conflict("CONFLICT")
        } else {
            AppError::Database(format!("update xv_charts failed: {err}"))
        }
    })
}

async fn shift_lower_widgets_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    existing_widgets: &[WidgetRecord],
    shift_from_y: i32,
    shift_delta: i32,
) -> Result<(), AppError> {
    if shift_delta <= 0 {
        return Ok(());
    }
    for w in existing_widgets {
        if w.position.y >= shift_from_y {
            let shifted = WidgetPosition {
                x: w.position.x,
                y: w.position.y.saturating_add(shift_delta),
                w: w.position.w,
                h: w.position.h,
            };
            let pos_json = serde_json::to_value(&shifted).map_err(|err| {
                AppError::bad("INVALID_ARGUMENT", format!("serialize position: {err}"))
            })?;
            sqlx::query(
                "UPDATE xv_widgets
                    SET position = $3,
                        updated_at = NOW()
                  WHERE tenant_id = $1 AND id = $2",
            )
            .bind(tenant_id)
            .bind(w.id)
            .bind(pos_json)
            .execute(&mut **tx)
            .await
            .map_err(|err| {
                AppError::Database(format!("shift xv_widgets position failed: {err}"))
            })?;
        }
    }
    Ok(())
}

fn merge_pinned_widget_options(
    chart_options: &serde_json::Value,
    chart_id: Uuid,
    semantic_zone: Option<SemanticZoneKind>,
) -> serde_json::Value {
    let mut map = chart_options
        .as_object()
        .cloned()
        .unwrap_or_else(serde_json::Map::new);
    map.insert(
        String::from("source_chart_id"),
        serde_json::Value::String(chart_id.to_string()),
    );
    if let Some(zone) = semantic_zone {
        map.insert(
            String::from("semantic_zone"),
            serde_json::Value::String(String::from(zone.as_str())),
        );
    }
    serde_json::Value::Object(map)
}

async fn execute_pin_saved_chart_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    chart_id: Uuid,
    req: PinChartToDashboardRequest,
    deps: WidgetDeps,
) -> Result<PinChartToDashboardResponse, AppError> {
    let maybe_dash = sqlx::query(
        "SELECT id, tenant_id, name, slug, description, filters, theme, canvas,
                status, version, created_by, created_at, updated_at
           FROM xv_dashboards
          WHERE tenant_id = $1 AND id = $2
          FOR UPDATE",
    )
    .bind(tenant_id)
    .bind(req.dashboard_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("lock xv_dashboards for pin failed: {err}")))?;
    let dash_row = maybe_dash.ok_or(AppError::NotFound("DASHBOARD_NOT_FOUND"))?;
    let dashboard = decode_dashboard_row(&dash_row)?;

    let chart_row = fetch_saved_chart_row_tx(tx, tenant_id, chart_id)
        .await?
        .ok_or(AppError::NotFound("CHART_NOT_FOUND"))?;
    let chart = decode_saved_chart_row(&chart_row)?;

    let existing_widgets = fetch_dashboard_widgets_tx(tx, tenant_id, dashboard.id).await?;
    let alloc = allocate_pinned_widget_slot(
        &dashboard.canvas,
        &existing_widgets,
        PinnedSlotRequest {
            widget_kind: chart.widget_kind,
            target_zone: req.target_zone,
            width: req.width,
            height: req.height,
        },
    )?;

    shift_lower_widgets_tx(
        tx,
        tenant_id,
        &existing_widgets,
        alloc.shift_from_y,
        alloc.shift_delta,
    )
    .await?;

    let merged_options = merge_pinned_widget_options(&chart.options, chart.id, alloc.semantic_zone);
    let widget = insert_widget_tx(
        tx,
        tenant_id,
        None,
        &CreateWidgetRecordParams {
            dashboard_id: dashboard.id,
            widget_kind: chart.widget_kind,
            title: chart.name,
            position: alloc.position,
            data_source: chart.data_source,
            encoding: chart.encoding,
            options: merged_options,
            drill: Vec::new(),
            refresh_mode: RefreshMode::Live,
            refresh_seconds: 60,
            deps,
        },
    )
    .await?;

    sqlx::query("UPDATE xv_dashboards SET updated_at = NOW() WHERE tenant_id = $1 AND id = $2")
        .bind(tenant_id)
        .bind(dashboard.id)
        .execute(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("touch xv_dashboards updated_at: {err}")))?;

    let deep_link = format!("/view/{}", dashboard.slug);
    Ok(PinChartToDashboardResponse {
        widget_id: widget.id,
        semantic_zone: alloc.semantic_zone,
        widget,
        dashboard_id: dashboard.id,
        dashboard_slug: dashboard.slug,
        deep_link,
    })
}
