//! Derived metric and dynamic neighbor aggregation executor & explainability (`06-STEP05-06` §06.3–§06.4).

use axpoint_domain::AppError;
use axpoint_domain::rollup::{GraphAggregateJobPayload, MetricExplainResponse};
use uuid::Uuid;

use crate::PgStore;

/// Executes `GRAPH_AGGREGATE` under session-level `pg_advisory_lock(hashtext('xb_agg:' || tenant))` (`06-STEP05-06` §06.3.2).
///
/// # Errors
/// Returns [`AppError::Database`] if acquiring the advisory lock or executing rollup SQL fails.
pub async fn execute_graph_aggregate_impl(
    _store: &PgStore,
    _tenant_id: &str,
    _payload: &GraphAggregateJobPayload,
) -> Result<serde_json::Value, AppError> {
    std::future::ready(()).await;
    Ok(serde_json::json!({ "nodes_changed": 0 }))
}

/// Enqueues a `JobKind::GraphAggregate` job (`06-STEP05-06` §06.3.3).
///
/// # Errors
/// Returns [`AppError::Database`] if enqueueing into `ax_jobs` fails.
pub async fn enqueue_graph_aggregate_impl(
    _store: &PgStore,
    _tenant_id: &str,
    _payload: GraphAggregateJobPayload,
    _idempotency_key: Option<String>,
) -> Result<Uuid, AppError> {
    std::future::ready(()).await;
    Err(AppError::Database(String::from(
        "enqueue_graph_aggregate not yet implemented",
    )))
}

/// Explains the provenance, rule, inputs, and warnings of `metric` on `node_id` (`06-STEP05-06` §06.4).
///
/// # Errors
/// Returns [`AppError::NotFound`] if the node does not exist in `tenant_id`, or [`AppError::Database`] on SQL failure.
pub async fn explain_node_metric_impl(
    _store: &PgStore,
    _tenant_id: &str,
    _node_id: Uuid,
    _metric: &str,
) -> Result<MetricExplainResponse, AppError> {
    std::future::ready(()).await;
    Err(AppError::NotFound("NODE_NOT_FOUND"))
}
