//! `SQL_TRANSFORM` operator running guarded `DuckDB` SQL and optionally materializing a `DERIVED_VIEW` (`07-STEP07-08` §07.4).

use std::collections::BTreeMap;
use std::sync::Arc;

use axpoint_domain::AppError;
use axpoint_domain::analytics::{AnalyticsPrincipal, QueryRequest};
use axpoint_domain::enums::FlowOperator;
use axpoint_domain::flow::{Operator, OperatorContext, OperatorOutcome};
use axpoint_domain::platform::BoxFuture;
use axpoint_domain::ports::{AnalyticsQueryPort, FlowSqlMaterializePort};

use super::{
    parse_optional_bool, parse_optional_string_any, parse_optional_u32, parse_optional_u64,
    parse_required_string, templated_bool_schema, templated_int_schema,
};

/// Studio palette display name for `SQL_TRANSFORM`.
pub const DISPLAY_NAME: &str = "SQL 分析变换";

/// Studio palette short description for `SQL_TRANSFORM`.
pub const DESCRIPTION: &str = "通过 AxAnalytics 受控执行 DuckDB SQL 查询并可选物化为派生数据集";

const RESERVED_OUTPUT_KEYS: &[&str] = &[
    "row_count",
    "rows",
    "columns",
    "first_row",
    "dataset_id",
    "dataset_name",
    "truncated",
    "duration_ms",
];

/// Returns the JSON Schema validating `SQL_TRANSFORM` node `config`.
#[must_use]
pub fn config_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "required": ["sql"],
        "properties": {
            "sql": { "type": "string", "minLength": 1 },
            "params": { "type": "object" },
            "output_dataset_name": { "type": "string", "minLength": 1, "maxLength": 128 },
            "materialize_as": { "type": "string", "minLength": 1, "maxLength": 128 },
            "dataset_name": { "type": "string", "minLength": 1, "maxLength": 128 },
            "max_rows": templated_int_schema(1, 100_000),
            "timeout_ms": templated_int_schema(100, 120_000),
            "allow_live": templated_bool_schema()
        },
        "additionalProperties": false
    })
}

/// `SQL_TRANSFORM` operator implementation generic over [`AnalyticsQueryPort`] and [`FlowSqlMaterializePort`].
pub struct SqlTransformOperator<Q, M> {
    analytics: Arc<Q>,
    materializer: Option<Arc<M>>,
}

impl<Q, M> SqlTransformOperator<Q, M>
where
    Q: AnalyticsQueryPort + 'static,
    M: FlowSqlMaterializePort + 'static,
{
    /// Constructs a new [`SqlTransformOperator`].
    #[must_use]
    pub const fn new(analytics: Arc<Q>, materializer: Option<Arc<M>>) -> Self {
        Self {
            analytics,
            materializer,
        }
    }

    async fn run(&self, ctx: &OperatorContext) -> Result<OperatorOutcome, AppError> {
        if ctx.is_canceled() {
            return Err(AppError::Conflict("RUN_CANCELED"));
        }

        let sql = parse_required_string(&ctx.config, "sql")?;
        let params: BTreeMap<String, serde_json::Value> = match ctx.config.get("params") {
            Some(serde_json::Value::Object(map)) => {
                map.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
            }
            None | Some(serde_json::Value::Null) => BTreeMap::new(),
            _ => {
                return Err(AppError::bad(
                    "INVALID_ARGUMENT",
                    "SQL_TRANSFORM config.params must be an object",
                ));
            }
        };
        let max_rows = parse_optional_u32(&ctx.config, "max_rows")?.or(Some(1_000));
        let timeout_ms = parse_optional_u64(&ctx.config, "timeout_ms")?;
        let allow_live = parse_optional_bool(&ctx.config, "allow_live")?.unwrap_or(false);

        let principal = AnalyticsPrincipal {
            user_id: ctx.principal.user_id,
            tenant_id: ctx.tenant_id.clone(),
            username: ctx.principal.username.clone(),
            permissions: ctx.principal.permissions.clone(),
        };

        let res = self
            .analytics
            .run_sql(
                &principal,
                QueryRequest {
                    sql,
                    params,
                    max_rows,
                    timeout_ms,
                    allow_live,
                    reveal_pii: false,
                },
            )
            .await?;

        let target_ds_name = parse_optional_string_any(
            &ctx.config,
            &["output_dataset_name", "materialize_as", "dataset_name"],
        )?;

        let dataset_id = if let Some(ds_name) = &target_ds_name {
            let mat = self.materializer.as_ref().ok_or_else(|| {
                AppError::Config(String::from("FlowSqlMaterializePort is not configured"))
            })?;
            Some(
                mat.materialize_sql_dataset(&principal, ds_name, &res.sql)
                    .await?,
            )
        } else {
            None
        };

        Ok(OperatorOutcome::Completed(build_sql_output(
            &res,
            dataset_id,
            target_ds_name.as_deref(),
        )))
    }
}

fn build_sql_output(
    res: &axpoint_domain::analytics::QueryResult,
    dataset_id: Option<uuid::Uuid>,
    dataset_name: Option<&str>,
) -> serde_json::Value {
    let first_row = res.rows.first().cloned();
    let mut map = serde_json::Map::new();
    if let Some(row) = &first_row {
        for (k, v) in row {
            if !RESERVED_OUTPUT_KEYS.contains(&k.as_str()) {
                map.insert(k.clone(), v.clone());
            }
        }
    }
    map.insert(String::from("row_count"), serde_json::json!(res.row_count));
    map.insert(String::from("rows"), serde_json::json!(res.rows));
    map.insert(String::from("columns"), serde_json::json!(res.columns));
    map.insert(
        String::from("first_row"),
        first_row.map_or(serde_json::Value::Null, serde_json::Value::Object),
    );
    map.insert(String::from("dataset_id"), serde_json::json!(dataset_id));
    map.insert(
        String::from("dataset_name"),
        serde_json::json!(dataset_name),
    );
    map.insert(String::from("truncated"), serde_json::json!(res.truncated));
    map.insert(
        String::from("duration_ms"),
        serde_json::json!(res.duration_ms),
    );
    serde_json::Value::Object(map)
}

impl<Q, M> Operator for SqlTransformOperator<Q, M>
where
    Q: AnalyticsQueryPort + 'static,
    M: FlowSqlMaterializePort + 'static,
{
    fn kind(&self) -> FlowOperator {
        FlowOperator::SqlTransform
    }

    fn display_name(&self) -> &'static str {
        DISPLAY_NAME
    }

    fn description(&self) -> &'static str {
        DESCRIPTION
    }

    fn config_schema(&self) -> serde_json::Value {
        config_schema()
    }

    fn execute<'a>(&'a self, ctx: &'a OperatorContext) -> BoxFuture<'a, OperatorOutcome> {
        Box::pin(async move { self.run(ctx).await })
    }
}
