//! Operations & background job query routes (`/api/axops/*`).

use axpoint_domain::AppError;
use axpoint_domain::enums::Permission;
use axpoint_domain::platform::{JobListQuery, JobListResponse, JobRecord};
use axpoint_domain::ports::JobQueuePort as _;
use axpoint_security::AuthPrincipal;
use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use uuid::Uuid;

use crate::state::{ApiPath, ApiQuery, AppState, HttpError};

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/axops/jobs", get(list_jobs_handler))
        .route("/axops/jobs/{id}", get(get_job_handler))
}

async fn list_jobs_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiQuery(query): ApiQuery<JobListQuery>,
) -> Result<Json<JobListResponse>, HttpError> {
    principal.require(Permission::DataRead)?;
    let resp = state.store.list_jobs(&principal.tenant_id, query).await?;
    Ok(Json(resp))
}

async fn get_job_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<JobRecord>, HttpError> {
    principal.require(Permission::DataRead)?;
    let job = state
        .store
        .get_job(&principal.tenant_id, id)
        .await?
        .ok_or(AppError::NotFound("JOB_NOT_FOUND"))?;
    Ok(Json(job))
}
