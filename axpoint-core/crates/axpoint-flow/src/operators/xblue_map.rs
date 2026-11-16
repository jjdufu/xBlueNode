//! `XBLUE_MAP` operator materializing an `xBlueStudio` dataset-to-graph mapping (`07-STEP07-08` §07.4).

use std::sync::Arc;

use axpoint_domain::AppError;
use axpoint_domain::enums::FlowOperator;
use axpoint_domain::flow::{Operator, OperatorContext, OperatorOutcome};
use axpoint_domain::platform::BoxFuture;
use axpoint_domain::ports::{LakeInternalQueryPort, StudioExecutionPort, StudioRepository};
use axpoint_domain::rollup::MapMaterializeJobPayload;
use axpoint_domain::studio::MappingMode;
use uuid::Uuid;

use super::{parse_optional_string_any, parse_required_uuid, templated_uuid_schema};

/// Studio palette display name for `XBLUE_MAP`.
pub const DISPLAY_NAME: &str = "业务节点映射";

/// Studio palette short description for `XBLUE_MAP`.
pub const DESCRIPTION: &str =
    "执行 xBlueStudio 映射规则将数据集行增量或全量物化为 BlueNode 与 BlueLink";

/// Returns the JSON Schema validating `XBLUE_MAP` node `config`.
#[must_use]
pub fn config_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "required": ["mapping_id"],
        "properties": {
            "mapping_id": templated_uuid_schema(),
            "mode": {
                "oneOf": [
                    { "type": "string", "enum": ["INCREMENTAL", "FULL"] },
                    { "type": "string", "pattern": "^\\s*\\{\\{.+\\}\\}\\s*$" }
                ]
            }
        },
        "additionalProperties": false
    })
}

/// `XBLUE_MAP` operator implementation generic over [`StudioRepository`], [`StudioExecutionPort`], and [`LakeInternalQueryPort`].
pub struct XblueMapOperator<S, L> {
    studio: Arc<S>,
    lake: Arc<L>,
}

impl<S, L> XblueMapOperator<S, L>
where
    S: StudioRepository + StudioExecutionPort + 'static,
    L: LakeInternalQueryPort + 'static,
{
    /// Constructs a new [`XblueMapOperator`].
    #[must_use]
    pub const fn new(studio: Arc<S>, lake: Arc<L>) -> Self {
        Self { studio, lake }
    }

    async fn run(&self, ctx: &OperatorContext) -> Result<OperatorOutcome, AppError> {
        if ctx.is_canceled() {
            return Err(AppError::Conflict("RUN_CANCELED"));
        }

        let mapping_id = parse_required_uuid(&ctx.config, "mapping_id")?;
        let mode = match parse_optional_string_any(&ctx.config, &["mode"])?.as_deref() {
            None | Some("INCREMENTAL") => MappingMode::Incremental,
            Some("FULL") => MappingMode::Full,
            Some(other) => {
                return Err(AppError::bad(
                    "INVALID_ARGUMENT",
                    format!("XBLUE_MAP mode must be INCREMENTAL or FULL, got '{other}'"),
                ));
            }
        };

        let _mapping = self
            .studio
            .get_mapping(&ctx.tenant_id, mapping_id)
            .await?
            .ok_or(AppError::NotFound("MAPPING_NOT_FOUND"))?;

        let payload = MapMaterializeJobPayload {
            mapping_id,
            mode,
            schema_version: None,
        };
        let run_rec = self
            .studio
            .execute_mapping_materialize(&ctx.tenant_id, Uuid::new_v4(), &payload, &*self.lake)
            .await?;

        Ok(OperatorOutcome::Completed(serde_json::json!({
            "mapping_id": run_rec.mapping_id,
            "mapping_run_id": run_rec.id,
            "job_id": run_rec.job_id,
            "mode": run_rec.mode,
            "from_snapshot": run_rec.from_snapshot,
            "to_snapshot": run_rec.to_snapshot,
            "status": run_rec.status,
            "nodes_upserted": run_rec.nodes_upserted,
            "nodes_deleted": run_rec.nodes_deleted,
            "links_upserted": run_rec.links_upserted,
            "points_written": run_rec.points_written,
            "rows_rejected": run_rec.rows_rejected,
        })))
    }
}

impl<S, L> Operator for XblueMapOperator<S, L>
where
    S: StudioRepository + StudioExecutionPort + 'static,
    L: LakeInternalQueryPort + 'static,
{
    fn kind(&self) -> FlowOperator {
        FlowOperator::XblueMap
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
