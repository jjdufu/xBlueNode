//! `MAP_MATERIALIZE` background job handler (`06-STEP05-06` §06.2).

use std::sync::Arc;

use axpoint_analytics::SessionPool;
use axpoint_db::PgStore;
use axpoint_domain::AppError;
use axpoint_domain::platform::{BoxFuture, JobHandler, LeasedJob};
use axpoint_domain::ports::StudioExecutionPort as _;
use axpoint_domain::rollup::MapMaterializeJobPayload;

/// `JobKind::MapMaterialize` handler materializing `DuckLake` datasets into `xb_nodes`, `xb_links`, and `xb_metric_series`.
pub(crate) struct MapMaterializeHandler {
    pub(crate) store: PgStore,
    pub(crate) analytics_pool: Arc<SessionPool>,
}

impl JobHandler for MapMaterializeHandler {
    fn handle<'a>(&'a self, job: &'a LeasedJob) -> BoxFuture<'a, serde_json::Value> {
        Box::pin(async move {
            let payload: MapMaterializeJobPayload = serde_json::from_value(job.payload.clone())
                .map_err(|err| {
                    AppError::bad(
                        "INVALID_ARGUMENT",
                        format!("invalid MAP_MATERIALIZE payload: {err}"),
                    )
                })?;

            let run = self
                .store
                .execute_mapping_materialize(
                    &job.tenant_id,
                    job.id,
                    &payload,
                    self.analytics_pool.as_ref(),
                )
                .await?;

            serde_json::to_value(&run).map_err(|err| {
                AppError::Database(format!("serialize MappingRunRecord failed: {err}"))
            })
        })
    }
}
