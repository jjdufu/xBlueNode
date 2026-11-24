//! Dependency graph extraction (`WidgetDeps`) for `xBlueView` widgets (`10-STEP11-12` §11.2.1).

use std::collections::{BTreeSet, HashMap};
use std::ops::ControlFlow;

use axpoint_domain::AppError;
use axpoint_domain::analytics::{MetricDef, PivotSpec, SavedQueryKind};
use axpoint_domain::axhub::{Dataset, DatasetListQuery};
use axpoint_domain::ports::{
    AnalyticsRepository, DatasetRepository, StudioRepository, ViewRepository,
};
use axpoint_domain::view::{WidgetDataSource, WidgetDeps};
use sqlparser::ast::{
    BinaryOperator, Expr, InTarget, ObjectName, Query, TableFactor, Value, Visit, Visitor,
};
use sqlparser::dialect::DuckDbDialect;
use sqlparser::parser::Parser;
use uuid::Uuid;

/// Extracts dataset, business-node type, and metric dependencies from a read-only `DuckDB` `SQL` query.
///
/// Uses `sqlparser` (`DuckDbDialect`) to inspect table references (`<table>`, `lake.<table>`,
/// `lake."tenant_<id>".<table>`, and `ax.nodes` / `ax.metric_*`) plus `type_name` / `metric`
/// filter predicates while ignoring `WITH` CTE aliases.
///
/// # Errors
/// Returns [`AppError::BadRequest`] (`SQL_REJECTED`) if `sql` cannot be parsed as `DuckDB` `SQL`.
pub fn extract_sql_deps(sql: &str, datasets: &[Dataset]) -> Result<WidgetDeps, AppError> {
    extract_sql_deps_with_schema(sql, datasets, &[])
}

/// Extracts `SQL` dependencies with fallback `schema_node_types` when `ax.nodes` is queried
/// without an explicit `type_name` filter.
///
/// # Errors
/// Returns [`AppError::BadRequest`] (`SQL_REJECTED`) if `sql` cannot be parsed as `DuckDB` `SQL`.
pub fn extract_sql_deps_with_schema(
    sql: &str,
    datasets: &[Dataset],
    schema_node_types: &[String],
) -> Result<WidgetDeps, AppError> {
    let trimmed = sql.trim().trim_end_matches(';').trim();
    if trimmed.is_empty() {
        return Ok(WidgetDeps::default());
    }

    let dialect = DuckDbDialect {};
    let statements = Parser::parse_sql(&dialect, trimmed)
        .map_err(|err| AppError::bad("SQL_REJECTED", format!("SQL parse failed: {err}")))?;

    let mut index = DatasetLookup::from_datasets(datasets);
    let mut collector = SqlDepsVisitor::new(&mut index);
    for stmt in &statements {
        let _flow = Visit::visit(stmt, &mut collector);
    }

    Ok(collector.into_deps(schema_node_types))
}

/// Computes normalized [`WidgetDeps`] for any [`WidgetDataSource`] variant.
///
/// # Errors
/// Propagates storage errors from `ViewRepository`, `AnalyticsRepository`, `DatasetRepository`,
/// or `StudioRepository`, or `SQL_REJECTED` when an inline `SQL` source fails parsing.
pub async fn compute_widget_deps<S>(
    store: &S,
    tenant_id: &str,
    data_source: &WidgetDataSource,
) -> Result<WidgetDeps, AppError>
where
    S: ViewRepository + AnalyticsRepository + DatasetRepository + StudioRepository + ?Sized,
{
    match data_source {
        WidgetDataSource::Sql { sql } => {
            let datasets = load_all_datasets(store, tenant_id).await?;
            let schema_types = load_schema_node_types(store, tenant_id).await?;
            extract_sql_deps_with_schema(sql, &datasets, &schema_types)
        }
        WidgetDataSource::SavedQuery { saved_query_id } => {
            compute_saved_query_deps(store, tenant_id, *saved_query_id).await
        }
        WidgetDataSource::Metric { metric, .. } => {
            compute_metric_deps(store, tenant_id, metric).await
        }
        WidgetDataSource::Pivot { spec } => compute_pivot_deps(store, tenant_id, spec).await,
        WidgetDataSource::NodeMetric {
            type_name, metric, ..
        } => Ok(normalize_deps(WidgetDeps {
            type_names: vec![type_name.clone()],
            metrics: vec![metric.clone()],
            ..WidgetDeps::default()
        })),
        WidgetDataSource::Graph {
            type_names,
            color_metric,
            ..
        }
        | WidgetDataSource::NodesGeo {
            type_names,
            color_metric,
            ..
        } => {
            let resolved_types = if type_names.is_empty() {
                load_schema_node_types(store, tenant_id).await?
            } else {
                type_names.clone()
            };
            let mut metrics = Vec::new();
            if let Some(cm) = color_metric
                && !cm.trim().is_empty()
            {
                metrics.push(cm.trim().to_owned());
            }
            Ok(normalize_deps(WidgetDeps {
                type_names: resolved_types,
                metrics,
                ..WidgetDeps::default()
            }))
        }
        WidgetDataSource::Alerts { .. } => Ok(WidgetDeps {
            alerts: true,
            ..WidgetDeps::default()
        }),
        WidgetDataSource::SimScenarios {
            scenario_ids,
            metrics,
        } => Ok(normalize_deps(WidgetDeps {
            scenario_ids: scenario_ids.clone(),
            metrics: metrics.clone(),
            ..WidgetDeps::default()
        })),
        WidgetDataSource::FloorPlan {
            asset_id,
            color_metric,
        } => {
            compute_spatial_asset_deps(store, tenant_id, *asset_id, color_metric.as_deref(), false)
                .await
        }
        WidgetDataSource::Scene3d {
            asset_id,
            color_metric,
        } => {
            compute_spatial_asset_deps(store, tenant_id, *asset_id, color_metric.as_deref(), true)
                .await
        }
        WidgetDataSource::Action { .. } | WidgetDataSource::Static { .. } => {
            Ok(WidgetDeps::default())
        }
    }
}

async fn compute_spatial_asset_deps<S: ViewRepository + ?Sized>(
    store: &S,
    tenant_id: &str,
    asset_id: Uuid,
    color_metric: Option<&str>,
    is_scene3d: bool,
) -> Result<WidgetDeps, AppError> {
    let type_names: Vec<String> = if is_scene3d {
        store
            .query_scene3d_rows(tenant_id, asset_id, color_metric)
            .await?
            .into_iter()
            .map(|r| r.type_name)
            .collect()
    } else {
        store
            .query_floor_plan_rows(tenant_id, asset_id, color_metric)
            .await?
            .into_iter()
            .map(|r| r.type_name)
            .collect()
    };
    let metrics = color_metric
        .iter()
        .map(|m| m.trim().to_owned())
        .filter(|m| !m.is_empty())
        .collect();
    Ok(normalize_deps(WidgetDeps {
        type_names,
        metrics,
        alerts: is_scene3d,
        ..WidgetDeps::default()
    }))
}

async fn load_schema_node_types<S: StudioRepository + ?Sized>(
    store: &S,
    tenant_id: &str,
) -> Result<Vec<String>, AppError> {
    let published = store.get_published_schema(tenant_id, "main").await?;
    Ok(published
        .doc
        .node_types
        .into_iter()
        .map(|nt| nt.name)
        .collect())
}

async fn compute_saved_query_deps<S>(
    store: &S,
    tenant_id: &str,
    saved_query_id: Uuid,
) -> Result<WidgetDeps, AppError>
where
    S: AnalyticsRepository + DatasetRepository + StudioRepository + ?Sized,
{
    let Some(sq) = store.get_saved_query(tenant_id, saved_query_id).await? else {
        return Ok(WidgetDeps::default());
    };
    match sq.kind {
        SavedQueryKind::Sql | SavedQueryKind::Ask => {
            let Some(sql) = sq.spec.get("sql").and_then(serde_json::Value::as_str) else {
                return Ok(WidgetDeps::default());
            };
            let datasets = load_all_datasets(store, tenant_id).await?;
            let schema_types = load_schema_node_types(store, tenant_id).await?;
            extract_sql_deps_with_schema(sql, &datasets, &schema_types)
        }
        SavedQueryKind::Metric => {
            let metric_name = sq
                .spec
                .get("metric_name")
                .or_else(|| sq.spec.get("metric"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            if metric_name.trim().is_empty() {
                return Ok(WidgetDeps::default());
            }
            compute_metric_deps(store, tenant_id, metric_name).await
        }
        SavedQueryKind::Pivot => compute_pivot_deps(store, tenant_id, &sq.spec).await,
    }
}

async fn compute_metric_deps<S>(
    store: &S,
    tenant_id: &str,
    metric_name: &str,
) -> Result<WidgetDeps, AppError>
where
    S: AnalyticsRepository + ?Sized,
{
    let Some(metric_def): Option<MetricDef> = store
        .get_metric_by_name(tenant_id, metric_name.trim())
        .await?
    else {
        return Ok(WidgetDeps::default());
    };
    Ok(normalize_deps(WidgetDeps {
        dataset_ids: vec![metric_def.dataset_id],
        ..WidgetDeps::default()
    }))
}

async fn compute_pivot_deps<S>(
    store: &S,
    tenant_id: &str,
    spec_json: &serde_json::Value,
) -> Result<WidgetDeps, AppError>
where
    S: DatasetRepository + ?Sized,
{
    let Ok(spec) = serde_json::from_value::<PivotSpec>(spec_json.clone()) else {
        return Ok(WidgetDeps::default());
    };
    let datasets = load_all_datasets(store, tenant_id).await?;
    let lookup = DatasetLookup::from_datasets(&datasets);
    let ds_name = spec.dataset.trim();
    if let Some(ds_id) = lookup.resolve_relation_name(ds_name) {
        return Ok(normalize_deps(WidgetDeps {
            dataset_ids: vec![ds_id],
            ..WidgetDeps::default()
        }));
    }
    Ok(WidgetDeps::default())
}

async fn load_all_datasets<S: DatasetRepository + ?Sized>(
    store: &S,
    tenant_id: &str,
) -> Result<Vec<Dataset>, AppError> {
    let mut all = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let page = store
            .list_datasets(
                tenant_id,
                DatasetListQuery {
                    limit: Some(500),
                    cursor,
                    ..DatasetListQuery::default()
                },
            )
            .await?;
        all.extend(page.items);
        if page.next_cursor.is_none() {
            break;
        }
        cursor = page.next_cursor;
    }
    Ok(all)
}

/// Deduplicates and deterministically sorts all fields inside [`WidgetDeps`].
#[must_use]
pub fn normalize_deps(deps: WidgetDeps) -> WidgetDeps {
    let dataset_ids: Vec<Uuid> = deps
        .dataset_ids
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let type_names: Vec<String> = deps
        .type_names
        .into_iter()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let metrics: Vec<String> = deps
        .metrics
        .into_iter()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let scenario_ids: Vec<Uuid> = deps
        .scenario_ids
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();

    WidgetDeps {
        dataset_ids,
        type_names,
        metrics,
        alerts: deps.alerts,
        scenario_ids,
    }
}

struct DatasetLookup {
    by_name: HashMap<String, Uuid>,
    by_lake_table: HashMap<String, Uuid>,
}

impl DatasetLookup {
    fn from_datasets(datasets: &[Dataset]) -> Self {
        let mut by_name = HashMap::with_capacity(datasets.len());
        let mut by_lake_table = HashMap::with_capacity(datasets.len());
        for ds in datasets {
            by_name.insert(ds.name.to_ascii_lowercase(), ds.id);
            by_lake_table.insert(ds.lake_table.to_ascii_lowercase(), ds.id);
        }
        Self {
            by_name,
            by_lake_table,
        }
    }

    fn resolve_relation_name(&self, name: &str) -> Option<Uuid> {
        let lower = name.trim().to_ascii_lowercase();
        self.by_name
            .get(&lower)
            .copied()
            .or_else(|| self.by_lake_table.get(&lower).copied())
    }
}

struct SqlDepsVisitor<'a> {
    lookup: &'a mut DatasetLookup,
    cte_names: BTreeSet<String>,
    dataset_ids: BTreeSet<Uuid>,
    type_names: BTreeSet<String>,
    metrics: BTreeSet<String>,
    uses_ax_view: bool,
    uses_ax_nodes: bool,
}

impl<'a> SqlDepsVisitor<'a> {
    const fn new(lookup: &'a mut DatasetLookup) -> Self {
        Self {
            lookup,
            cte_names: BTreeSet::new(),
            dataset_ids: BTreeSet::new(),
            type_names: BTreeSet::new(),
            metrics: BTreeSet::new(),
            uses_ax_view: false,
            uses_ax_nodes: false,
        }
    }

    fn into_deps(self, schema_node_types: &[String]) -> WidgetDeps {
        let (type_names, metrics) = if self.uses_ax_view {
            let resolved_types: Vec<String> = if self.uses_ax_nodes && self.type_names.is_empty() {
                schema_node_types
                    .iter()
                    .map(|s| s.trim().to_owned())
                    .filter(|s| !s.is_empty())
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect()
            } else {
                self.type_names.into_iter().collect()
            };
            (resolved_types, self.metrics.into_iter().collect())
        } else {
            (Vec::new(), Vec::new())
        };

        WidgetDeps {
            dataset_ids: self.dataset_ids.into_iter().collect(),
            type_names,
            metrics,
            alerts: false,
            scenario_ids: Vec::new(),
        }
    }

    fn handle_table_name(&mut self, name: &ObjectName) {
        let parts: Vec<String> = name
            .0
            .iter()
            .filter_map(|part| part.as_ident().map(|ident| ident.value.clone()))
            .collect();
        match parts.as_slice() {
            [single] => {
                let lower = single.to_ascii_lowercase();
                if !self.cte_names.contains(&lower)
                    && let Some(id) = self.lookup.resolve_relation_name(&lower)
                {
                    self.dataset_ids.insert(id);
                }
            }
            [first, second] => {
                if first.eq_ignore_ascii_case("lake") {
                    if let Some(id) = self.lookup.resolve_relation_name(second) {
                        self.dataset_ids.insert(id);
                    }
                } else if first.eq_ignore_ascii_case("ax") && is_ax_platform_view(second) {
                    self.uses_ax_view = true;
                    if second.eq_ignore_ascii_case("nodes") {
                        self.uses_ax_nodes = true;
                    }
                }
            }
            [first, _schema, table] => {
                if first.eq_ignore_ascii_case("lake")
                    && let Some(id) = self.lookup.resolve_relation_name(table)
                {
                    self.dataset_ids.insert(id);
                }
            }
            _ => {}
        }
    }

    fn handle_expr(&mut self, expr: &Expr) {
        match expr {
            Expr::BinaryOp {
                left,
                op: BinaryOperator::Eq,
                right,
            } => {
                if let (Some(col), Some(lit)) = (extract_col_name(left), extract_str_literal(right))
                {
                    self.record_col_filter(col, lit);
                } else if let (Some(col), Some(lit)) =
                    (extract_col_name(right), extract_str_literal(left))
                {
                    self.record_col_filter(col, lit);
                }
            }
            Expr::In {
                expr: inner,
                target: InTarget::List(list),
                negated: false,
                global: _,
            } => {
                if let Some(col) = extract_col_name(inner) {
                    for item in list {
                        if let Some(lit) = extract_str_literal(item) {
                            self.record_col_filter(col, lit);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn record_col_filter(&mut self, col: &str, value: &str) {
        let val = value.trim();
        if val.is_empty() {
            return;
        }
        if col.eq_ignore_ascii_case("type_name") {
            self.type_names.insert(val.to_owned());
        } else if col.eq_ignore_ascii_case("metric") {
            self.metrics.insert(val.to_owned());
        }
    }
}

fn is_ax_platform_view(view: &str) -> bool {
    view.eq_ignore_ascii_case("nodes")
        || view.eq_ignore_ascii_case("links")
        || view.eq_ignore_ascii_case("metric_points")
        || view.eq_ignore_ascii_case("metric_series")
        || view.eq_ignore_ascii_case("metric_rollups")
}

impl Visitor for SqlDepsVisitor<'_> {
    type Break = ();

    fn pre_visit_query(&mut self, query: &Query) -> ControlFlow<Self::Break> {
        if let Some(with) = &query.with {
            for cte in &with.cte_tables {
                self.cte_names
                    .insert(cte.alias.name.value.to_ascii_lowercase());
            }
        }
        ControlFlow::Continue(())
    }

    fn pre_visit_table_factor(&mut self, tf: &TableFactor) -> ControlFlow<Self::Break> {
        if let TableFactor::Table { name, .. } = tf {
            self.handle_table_name(name);
        }
        ControlFlow::Continue(())
    }

    fn pre_visit_expr(&mut self, expr: &Expr) -> ControlFlow<Self::Break> {
        self.handle_expr(expr);
        ControlFlow::Continue(())
    }
}

fn extract_col_name(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::Identifier(ident) => Some(ident.value.as_str()),
        Expr::CompoundIdentifier(parts) => parts.last().map(|ident| ident.value.as_str()),
        _ => None,
    }
}

fn extract_str_literal(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::Value(vws) => match &vws.value {
            Value::SingleQuotedString(s) | Value::DoubleQuotedString(s) => Some(s.as_str()),
            _ => None,
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use axpoint_domain::axhub::{DatasetStatus, SchemaPolicy};
    use axpoint_domain::enums::{SourceKind, SyncMode};
    use chrono::Utc;

    use super::*;

    fn sample_dataset(id: Uuid, name: &str, lake_table: &str) -> Dataset {
        let now = Utc::now();
        Dataset {
            id,
            connection_id: None,
            name: name.to_owned(),
            display_name: name.to_owned(),
            description: String::new(),
            tags: Vec::new(),
            source_kind: SourceKind::SqlTable,
            source_ref: serde_json::json!({}),
            sync_mode: SyncMode::Snapshot,
            cursor_column: None,
            cursor_value: None,
            primary_key: Vec::new(),
            delete_column: None,
            schema_policy: SchemaPolicy::Evolve,
            lake_schema: String::from("tenant_default"),
            schedule_cron: None,
            continuous_stream: false,
            cdc_slot_meta: serde_json::json!({}),
            lake_table: lake_table.to_owned(),
            schema_version: 1,
            schema_json: Vec::new(),
            row_count: 10,
            last_snapshot_id: None,
            last_synced_at: Some(now),
            status: DatasetStatus::Ready,
            owner_user_id: None,
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn extract_sql_deps_resolves_bare_lake_tenant_and_platform_views() -> Result<(), AppError> {
        let ds_orders = Uuid::from_u128(101);
        let ds_inv = Uuid::from_u128(102);
        let datasets = vec![
            sample_dataset(ds_orders, "demo_orders", "demo_orders_tbl"),
            sample_dataset(ds_inv, "inventory_snap", "inv_tbl"),
        ];

        let sql = r#"
            WITH recent AS (
                SELECT order_id, amount FROM lake."tenant_default"."demo_orders_tbl"
            )
            SELECT r.order_id, i.qty
            FROM recent r
            JOIN lake.inventory_snap i ON r.order_id = i.order_id
            UNION ALL
            SELECT n.node_key, m.value
            FROM ax.nodes n
            JOIN ax.metric_points m ON m.node_id = n.id
            WHERE n.type_name = 'Warehouse' AND m.metric IN ('inventory', 'utilization')
        "#;

        let deps = extract_sql_deps(sql, &datasets)?;
        assert_eq!(deps.dataset_ids, vec![ds_orders, ds_inv]);
        assert_eq!(deps.type_names, vec![String::from("Warehouse")]);
        assert_eq!(
            deps.metrics,
            vec![String::from("inventory"), String::from("utilization")]
        );
        assert!(!deps.alerts);
        Ok(())
    }

    #[test]
    fn extract_sql_deps_ignores_non_ax_columns_and_hydrates_unfiltered_ax_nodes()
    -> Result<(), AppError> {
        let ds_orders = Uuid::from_u128(201);
        let datasets = vec![sample_dataset(ds_orders, "demo_orders", "demo_orders")];
        let schema_types = vec![String::from("Warehouse"), String::from("Factory")];

        let lake_only =
            "SELECT * FROM demo_orders WHERE type_name = 'B2B_ORDER' AND metric = 'gmv'";
        let deps_lake = extract_sql_deps_with_schema(lake_only, &datasets, &schema_types)?;
        assert_eq!(deps_lake.dataset_ids, vec![ds_orders]);
        assert_eq!(deps_lake.type_names, Vec::<String>::new());
        assert_eq!(deps_lake.metrics, Vec::<String>::new());

        let unfiltered_nodes = "SELECT id, title FROM ax.nodes LIMIT 50";
        let deps_nodes = extract_sql_deps_with_schema(unfiltered_nodes, &datasets, &schema_types)?;
        assert_eq!(
            deps_nodes.type_names,
            vec![String::from("Factory"), String::from("Warehouse")]
        );
        Ok(())
    }

    #[test]
    fn extract_sql_deps_rejects_invalid_sql() {
        let err = extract_sql_deps("SELECT FROM WHERE ???", &[]);
        assert!(matches!(
            err,
            Err(AppError::BadRequest {
                code: "SQL_REJECTED",
                ..
            })
        ));
    }
}
