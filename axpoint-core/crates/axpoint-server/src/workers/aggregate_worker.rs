//! `GRAPH_AGGREGATE` background job handler (`06-STEP05-06` §06.3).

use axpoint_db::PgStore;
use axpoint_domain::AppError;
use axpoint_domain::platform::{BoxFuture, JobHandler, LeasedJob};
use axpoint_domain::ports::StudioExecutionPort as _;
use axpoint_domain::rollup::GraphAggregateJobPayload;

/// `JobKind::GraphAggregate` handler executing derived metric expressions and neighbor rollups in topological order.
pub(crate) struct GraphAggregateHandler {
    pub(crate) store: PgStore,
}

impl JobHandler for GraphAggregateHandler {
    fn handle<'a>(&'a self, job: &'a LeasedJob) -> BoxFuture<'a, serde_json::Value> {
        Box::pin(async move {
            let payload: GraphAggregateJobPayload = serde_json::from_value(job.payload.clone())
                .map_err(|err| {
                    AppError::bad(
                        "INVALID_ARGUMENT",
                        format!("invalid GRAPH_AGGREGATE payload: {err}"),
                    )
                })?;

            self.store
                .execute_graph_aggregate(&job.tenant_id, &payload)
                .await
        })
    }
}
