//! `SOURCE_INGEST` operator triggering an `AxHub` dataset sync and optionally polling to completion (`07-STEP07-08` §07.4).

use std::sync::Arc;
use std::time::Duration;

use axpoint_domain::AppError;
use axpoint_domain::enums::{FlowOperator, JobStatus};
use axpoint_domain::flow::{Operator, OperatorContext, OperatorOutcome};
use axpoint_domain::platform::BoxFuture;
use axpoint_domain::ports::{DatasetRepository, JobQueuePort};
use uuid::Uuid;

use super::{
    parse_optional_bool, parse_optional_u64, parse_required_uuid, templated_bool_schema,
    templated_int_schema, templated_uuid_schema,
};

/// Studio palette display name for `SOURCE_INGEST`.
pub const DISPLAY_NAME: &str = "数据源同步";

/// Studio palette short description for `SOURCE_INGEST`.
pub const DESCRIPTION: &str = "触发 AxHub 数据集同步任务并可选等待 DuckLake 快照提交完成";

/// Returns the JSON Schema validating `SOURCE_INGEST` node `config`.
#[must_use]
pub fn config_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "required": ["dataset_id"],
        "properties": {
            "dataset_id": templated_uuid_schema(),
            "wait": templated_bool_schema(),
            "poll_ms": templated_int_schema(50, 10_000)
        },
        "additionalProperties": false
    })
}

/// `SOURCE_INGEST` operator implementation generic over [`DatasetRepository`] and [`JobQueuePort`].
pub struct SourceIngestOperator<D, J> {
    dataset_repo: Arc<D>,
    job_queue: Arc<J>,
}

impl<D, J> SourceIngestOperator<D, J>
where
    D: DatasetRepository + 'static,
    J: JobQueuePort + 'static,
{
    /// Constructs a new [`SourceIngestOperator`].
    #[must_use]
    pub const fn new(dataset_repo: Arc<D>, job_queue: Arc<J>) -> Self {
        Self {
            dataset_repo,
            job_queue,
        }
    }

    async fn run(&self, ctx: &OperatorContext) -> Result<OperatorOutcome, AppError> {
        if ctx.is_canceled() {
            return Err(AppError::Conflict("RUN_CANCELED"));
        }

        let dataset_id = parse_required_uuid(&ctx.config, "dataset_id")?;
        let wait = parse_optional_bool(&ctx.config, "wait")?.unwrap_or(true);
        let poll_ms = parse_optional_u64(&ctx.config, "poll_ms")?
            .unwrap_or(250)
            .clamp(50, 10_000);

        let initial_ds = self
            .dataset_repo
            .get_dataset(&ctx.tenant_id, dataset_id)
            .await?
            .ok_or(AppError::NotFound("DATASET_NOT_FOUND"))?;

        let sync_resp = self
            .dataset_repo
            .enqueue_sync_job(&ctx.tenant_id, dataset_id)
            .await?;

        if !wait {
            return Ok(OperatorOutcome::Completed(serde_json::json!({
                "dataset_id": dataset_id,
                "job_id": sync_resp.job_id,
                "existing": sync_resp.existing,
                "status": "PENDING",
                "rows_written": 0,
                "row_count": initial_ds.row_count,
                "snapshot_id": initial_ds.last_snapshot_id,
            })));
        }

        self.poll_sync_completion(ctx, dataset_id, sync_resp.job_id, poll_ms)
            .await
    }

    async fn poll_sync_completion(
        &self,
        ctx: &OperatorContext,
        dataset_id: Uuid,
        job_id: Uuid,
        poll_ms: u64,
    ) -> Result<OperatorOutcome, AppError> {
        let interval = Duration::from_millis(poll_ms);
        loop {
            if ctx.is_canceled() {
                return Err(AppError::Conflict("RUN_CANCELED"));
            }

            let job = self
                .job_queue
                .get_job(&ctx.tenant_id, job_id)
                .await?
                .ok_or(AppError::NotFound("JOB_NOT_FOUND"))?;

            match job.status {
                JobStatus::Pending | JobStatus::Running => {
                    tokio::time::sleep(interval).await;
                }
                JobStatus::Succeeded => {
                    let ds = self
                        .dataset_repo
                        .get_dataset(&ctx.tenant_id, dataset_id)
                        .await?
                        .ok_or(AppError::NotFound("DATASET_NOT_FOUND"))?;
                    let sync_jobs = self
                        .dataset_repo
                        .list_sync_jobs(&ctx.tenant_id, dataset_id, 1, None)
                        .await?;
                    let rows_written = sync_jobs
                        .items
                        .first()
                        .map_or(ds.row_count, |sj| sj.rows_written);
                    return Ok(OperatorOutcome::Completed(serde_json::json!({
                        "dataset_id": dataset_id,
                        "job_id": job_id,
                        "status": "SUCCEEDED",
                        "rows_written": rows_written,
                        "row_count": ds.row_count,
                        "snapshot_id": ds.last_snapshot_id,
                    })));
                }
                JobStatus::Failed | JobStatus::Canceled | JobStatus::Expired => {
                    let msg = job
                        .last_error
                        .unwrap_or_else(|| format!("sync job ended with status {}", job.status));
                    return Err(AppError::Upstream(msg));
                }
            }
        }
    }
}

impl<D, J> Operator for SourceIngestOperator<D, J>
where
    D: DatasetRepository + 'static,
    J: JobQueuePort + 'static,
{
    fn kind(&self) -> FlowOperator {
        FlowOperator::SourceIngest
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
