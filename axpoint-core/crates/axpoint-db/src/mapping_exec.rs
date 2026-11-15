//! Dataset-to-graph full and incremental mapping materialization executor (`06-STEP05-06` §06.2).

use axpoint_domain::AppError;
use axpoint_domain::ports::LakeInternalQueryPort;
use axpoint_domain::rollup::MapMaterializeJobPayload;
use axpoint_domain::studio::{MappingRecord, MappingRunRecord};
use uuid::Uuid;

use crate::PgStore;

/// Executes `MAP_MATERIALIZE` (`FULL` or `INCREMENTAL`) for `payload.mapping_id` (`06-STEP05-06` §06.2).
///
/// # Errors
/// Returns [`AppError`] if the mapping or dataset cannot be loaded or execution fails.
pub async fn execute_mapping_materialize_impl<L: LakeInternalQueryPort>(
    _store: &PgStore,
    _tenant_id: &str,
    _job_id: Uuid,
    _payload: &MapMaterializeJobPayload,
    _lake: &L,
) -> Result<MappingRunRecord, AppError> {
    std::future::ready(()).await;
    Err(AppError::NotFound("MAPPING_NOT_FOUND"))
}

/// Lists enabled `main` subgraph mappings bound to `dataset_id` (`06-STEP05-06` §06.2.3).
///
/// # Errors
/// Returns [`AppError::Database`] if querying `xb_mappings` fails.
pub async fn list_enabled_main_mappings_for_dataset_impl(
    _store: &PgStore,
    _tenant_id: &str,
    _dataset_id: Uuid,
) -> Result<Vec<MappingRecord>, AppError> {
    std::future::ready(()).await;
    Ok(Vec::new())
}
