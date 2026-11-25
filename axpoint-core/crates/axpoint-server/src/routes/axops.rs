//! `AxOps` operations, deep health, retention, backups, industry templates,
//! demo scenarios, and delivery work-item routes (`/api/axops/*`, `12-STEP14` `§14.8`).

use axpoint_domain::AppError;
use axpoint_domain::axhub::PaginatedList;
use axpoint_domain::enums::{JobKind, Permission, RetentionTarget};
use axpoint_domain::ops::{
    BackupRecord, CreateBackupRequest, CreateWorkItemCommentRequest, CreateWorkItemRequest,
    DeepHealthReport, DemoLoadResponse, DemoScenario, InstallTemplateRequest,
    OnboardingStatusResponse, OutboxStats, PatchWorkItemRequest, PlatformPulseResponse,
    PutRetentionPolicyRequest, RetentionPolicyRecord, SystemInfoResponse, TemplateInstallRecord,
    TemplateManifest, TemplatePackDetail, TemplatePlanRequest, TemplatePreviewResponse,
    TemplateUninstallResponse, WorkItem, WorkItemDetail, WorkItemEvent, WorkItemListQuery,
};
use axpoint_domain::platform::{JobListQuery, JobListResponse, JobRecord, NewJob};
use axpoint_domain::ports::{
    JobQueuePort as _, OnboardingAndPackagingPort as _, OpsRepository as _,
};
use axpoint_security::AuthPrincipal;
use axpoint_server::ops::backup::BackupAuditContext;
use axpoint_server::ops::templates::TemplateAuditContext;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use uuid::Uuid;

use crate::routes::auth::extract_client_ip;
use crate::state::{ApiJson, ApiPath, ApiQuery, AppState, HttpError};
use crate::telemetry::current_trace_id;

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct BackupsListQuery {
    limit: Option<u32>,
    cursor: Option<String>,
}

/// Builds the `/axops/*` sub-router mounted under `/api` (27 routes per `A4` §11).
pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .merge(jobs_and_health_router())
        .merge(retention_and_backups_router())
        .merge(templates_and_demo_router())
        .merge(work_items_router())
}

fn jobs_and_health_router() -> Router<AppState> {
    Router::new()
        .route("/axops/pulse", get(pulse_handler))
        .route("/axops/onboarding-status", get(onboarding_status_handler))
        .route("/axops/jobs", get(list_jobs_handler))
        .route("/axops/jobs/rollup-now", post(rollup_now_handler))
        .route("/axops/jobs/{id}", get(get_job_handler))
        .route("/axops/jobs/{id}/retry", post(retry_job_handler))
        .route("/axops/jobs/{id}/cancel", post(cancel_job_handler))
        .route("/axops/health/deep", get(deep_health_handler))
        .route("/axops/outbox/stats", get(outbox_stats_handler))
        .route("/axops/system/info", get(system_info_handler))
}

fn retention_and_backups_router() -> Router<AppState> {
    Router::new()
        .route("/axops/retention", get(list_retention_handler))
        .route("/axops/retention/run-now", post(retention_run_now_handler))
        .route("/axops/retention/{target}", put(put_retention_handler))
        .route(
            "/axops/backups",
            get(list_backups_handler).post(create_backup_handler),
        )
        .route("/axops/backups/{id}", get(get_backup_handler))
}

fn templates_and_demo_router() -> Router<AppState> {
    Router::new()
        .route("/axops/templates", get(list_templates_handler))
        .route(
            "/axops/templates/installs",
            get(list_template_installs_handler),
        )
        .route("/axops/templates/{pack_id}", get(get_template_handler))
        .route(
            "/axops/templates/{pack_id}/preview",
            get(preview_template_handler),
        )
        .route(
            "/axops/templates/{pack_id}/plan",
            post(plan_template_handler),
        )
        .route(
            "/axops/templates/{pack_id}/install",
            post(install_template_handler),
        )
        .route(
            "/axops/templates/installs/{id}/apply",
            post(apply_template_handler),
        )
        .route(
            "/axops/templates/installs/{id}/uninstall",
            post(uninstall_template_handler),
        )
        .route("/axops/demo/{scenario}/load", post(load_demo_handler))
}

fn work_items_router() -> Router<AppState> {
    Router::new()
        .route(
            "/axops/work-items",
            get(list_work_items_handler).post(create_work_item_handler),
        )
        .route(
            "/axops/work-items/{id}",
            get(get_work_item_handler).patch(patch_work_item_handler),
        )
        .route(
            "/axops/work-items/{id}/comments",
            post(add_work_item_comment_handler),
        )
}

fn actor_opt(principal: &AuthPrincipal) -> Option<Uuid> {
    (!principal.user_id.is_nil()).then_some(principal.user_id)
}

fn parse_optional_json<T>(body: &Bytes) -> Result<T, HttpError>
where
    T: Default + DeserializeOwned,
{
    if body.iter().all(u8::is_ascii_whitespace) {
        return Ok(T::default());
    }
    serde_json::from_slice::<T>(body)
        .map_err(|err| HttpError(AppError::bad("INVALID_ARGUMENT", err.to_string())))
}

async fn pulse_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
) -> Result<Json<PlatformPulseResponse>, HttpError> {
    principal.require(Permission::DataRead)?;
    let resp = state
        .store
        .fetch_platform_pulse(&principal.tenant_id)
        .await?;
    Ok(Json(resp))
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

async fn retry_job_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<JobRecord>, HttpError> {
    principal.require(Permission::OpsAdmin)?;
    let job = state
        .store
        .retry_job(&principal.tenant_id, id)
        .await?
        .ok_or(AppError::NotFound("JOB_NOT_FOUND"))?;
    Ok(Json(job))
}

async fn cancel_job_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<JobRecord>, HttpError> {
    principal.require(Permission::OpsAdmin)?;
    let job = state
        .store
        .cancel_job(&principal.tenant_id, id)
        .await?
        .ok_or(AppError::NotFound("JOB_NOT_FOUND"))?;
    Ok(Json(job))
}

async fn rollup_now_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
) -> Result<(StatusCode, Json<serde_json::Value>), HttpError> {
    principal.require(Permission::OpsAdmin)?;
    let job_id = state
        .store
        .enqueue(
            &principal.tenant_id,
            NewJob::new(JobKind::Rollup, serde_json::json!({ "trigger": "manual" })),
        )
        .await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(serde_json::json!({ "job_id": job_id })),
    ))
}

async fn deep_health_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
) -> Result<Json<DeepHealthReport>, HttpError> {
    principal.require(Permission::OpsAdmin)?;
    let report = Box::pin(state.ops.probe_deep_health(&principal.tenant_id)).await;
    Ok(Json(report))
}

async fn outbox_stats_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
) -> Result<Json<OutboxStats>, HttpError> {
    principal.require(Permission::OpsAdmin)?;
    let outbox = state.store.query_outbox_stats(&principal.tenant_id).await?;
    Ok(Json(outbox))
}

async fn system_info_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
) -> Result<Json<SystemInfoResponse>, HttpError> {
    principal.require(Permission::OpsAdmin)?;
    Ok(Json(state.ops.get_system_info(&principal.tenant_id)))
}

async fn list_retention_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
) -> Result<Json<PaginatedList<RetentionPolicyRecord>>, HttpError> {
    principal.require(Permission::OpsAdmin)?;
    let items = state
        .store
        .list_retention_policies(&principal.tenant_id)
        .await?;
    Ok(Json(PaginatedList {
        items,
        next_cursor: None,
    }))
}

async fn put_retention_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(target): ApiPath<RetentionTarget>,
    ApiJson(req): ApiJson<PutRetentionPolicyRequest>,
) -> Result<Json<RetentionPolicyRecord>, HttpError> {
    principal.require(Permission::OpsAdmin)?;
    if req.keep_days <= 0 || req.keep_days > 36_500 {
        return Err(HttpError(AppError::bad(
            "INVALID_ARGUMENT",
            "keep_days must be in 1..=36500",
        )));
    }
    let updated = state
        .store
        .upsert_retention_policy(&principal.tenant_id, target, req.keep_days)
        .await?;
    Ok(Json(updated))
}

async fn retention_run_now_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
) -> Result<(StatusCode, Json<serde_json::Value>), HttpError> {
    principal.require(Permission::OpsAdmin)?;
    let job_id = state
        .store
        .enqueue(
            &principal.tenant_id,
            NewJob::new(
                JobKind::Retention,
                serde_json::json!({ "trigger": "manual" }),
            ),
        )
        .await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(serde_json::json!({ "job_id": job_id })),
    ))
}

async fn list_backups_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiQuery(query): ApiQuery<BackupsListQuery>,
) -> Result<Json<PaginatedList<BackupRecord>>, HttpError> {
    let page = state
        .ops
        .list_backups(&principal, query.limit, query.cursor.as_deref())
        .await?;
    Ok(Json(page))
}

async fn create_backup_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthPrincipal,
    body: Bytes,
) -> Result<(StatusCode, Json<BackupRecord>), HttpError> {
    let req: CreateBackupRequest = parse_optional_json(&body)?;
    let client_ip = extract_client_ip(&headers);
    let trace_id = current_trace_id();
    let created = state
        .ops
        .trigger_backup(
            &principal,
            BackupAuditContext {
                client_ip: &client_ip,
                trace_id: &trace_id,
            },
            req,
        )
        .await?;
    Ok((StatusCode::ACCEPTED, Json(created)))
}

async fn get_backup_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<BackupRecord>, HttpError> {
    let rec = state.ops.get_backup(&principal, id).await?;
    Ok(Json(rec))
}

async fn list_templates_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
) -> Result<Json<PaginatedList<TemplateManifest>>, HttpError> {
    principal.require(Permission::DataRead)?;
    Ok(Json(state.ops.list_templates()?))
}

async fn get_template_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(pack_id): ApiPath<String>,
) -> Result<Json<TemplatePackDetail>, HttpError> {
    principal.require(Permission::DataRead)?;
    Ok(Json(state.ops.get_template(&pack_id)?))
}

async fn onboarding_status_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
) -> Result<Json<OnboardingStatusResponse>, HttpError> {
    principal.require(Permission::DataRead)?;
    let status = state
        .store
        .get_onboarding_status(&principal.tenant_id)
        .await?;
    Ok(Json(status))
}

async fn preview_template_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(pack_id): ApiPath<String>,
) -> Result<Json<TemplatePreviewResponse>, HttpError> {
    principal.require(Permission::DataRead)?;
    Ok(Json(state.ops.preview_template(&pack_id)?))
}

async fn install_template_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthPrincipal,
    ApiPath(pack_id): ApiPath<String>,
    body: Bytes,
) -> Result<Json<TemplateInstallRecord>, HttpError> {
    principal.enforce_no_prod_mutation()?;
    principal.require(Permission::OpsAdmin)?;
    let req: InstallTemplateRequest = parse_optional_json(&body)?;
    let planned = state
        .ops
        .plan_template_install(
            &principal.tenant_id,
            actor_opt(&principal),
            &pack_id,
            Some(req.on_conflict),
        )
        .await?;
    if req.dry_run {
        return Ok(Json(planned));
    }
    let client_ip = extract_client_ip(&headers);
    let trace_id = current_trace_id();
    let installed = state
        .ops
        .apply_template_install(
            &principal,
            TemplateAuditContext {
                client_ip: &client_ip,
                trace_id: &trace_id,
            },
            planned.id,
        )
        .await?;
    Ok(Json(installed))
}

async fn list_template_installs_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
) -> Result<Json<PaginatedList<TemplateInstallRecord>>, HttpError> {
    principal.require(Permission::DataRead)?;
    let list = state
        .ops
        .list_template_installs(&principal.tenant_id)
        .await?;
    Ok(Json(list))
}

async fn plan_template_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(pack_id): ApiPath<String>,
    body: Bytes,
) -> Result<Json<TemplateInstallRecord>, HttpError> {
    principal.enforce_no_prod_mutation()?;
    principal.require(Permission::OpsAdmin)?;
    let req: TemplatePlanRequest = parse_optional_json(&body)?;
    let planned = state
        .ops
        .plan_template_install(
            &principal.tenant_id,
            actor_opt(&principal),
            &pack_id,
            req.on_conflict,
        )
        .await?;
    Ok(Json(planned))
}

async fn apply_template_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<TemplateInstallRecord>, HttpError> {
    principal.enforce_no_prod_mutation()?;
    principal.require(Permission::OpsAdmin)?;
    let client_ip = extract_client_ip(&headers);
    let trace_id = current_trace_id();
    let installed = state
        .ops
        .apply_template_install(
            &principal,
            TemplateAuditContext {
                client_ip: &client_ip,
                trace_id: &trace_id,
            },
            id,
        )
        .await?;
    Ok(Json(installed))
}

async fn uninstall_template_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<TemplateUninstallResponse>, HttpError> {
    principal.enforce_no_prod_mutation()?;
    principal.require(Permission::OpsAdmin)?;
    let res = state
        .ops
        .uninstall_template(&principal.tenant_id, actor_opt(&principal), id)
        .await?;
    Ok(Json(res))
}

async fn load_demo_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthPrincipal,
    ApiPath(scenario): ApiPath<DemoScenario>,
) -> Result<Json<DemoLoadResponse>, HttpError> {
    principal.enforce_no_prod_mutation()?;
    principal.require(Permission::OpsAdmin)?;
    let client_ip = extract_client_ip(&headers);
    let trace_id = current_trace_id();
    let res = state
        .ops
        .load_demo_scenario(
            &principal,
            TemplateAuditContext {
                client_ip: &client_ip,
                trace_id: &trace_id,
            },
            scenario,
        )
        .await?;
    Ok(Json(res))
}

async fn list_work_items_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiQuery(query): ApiQuery<WorkItemListQuery>,
) -> Result<Json<PaginatedList<WorkItem>>, HttpError> {
    principal.require(Permission::DataRead)?;
    let page = state
        .ops
        .list_work_items(&principal.tenant_id, query)
        .await?;
    Ok(Json(page))
}

async fn create_work_item_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiJson(req): ApiJson<CreateWorkItemRequest>,
) -> Result<(StatusCode, Json<WorkItem>), HttpError> {
    principal.require(Permission::FlowRun)?;
    let created = state
        .ops
        .create_work_item(&principal.tenant_id, actor_opt(&principal), req)
        .await?;
    Ok((StatusCode::CREATED, Json(created)))
}

async fn get_work_item_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<WorkItemDetail>, HttpError> {
    principal.require(Permission::DataRead)?;
    let detail = state.ops.get_work_item(&principal.tenant_id, id).await?;
    Ok(Json(detail))
}

async fn patch_work_item_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(req): ApiJson<PatchWorkItemRequest>,
) -> Result<Json<WorkItem>, HttpError> {
    principal.require(Permission::FlowRun)?;
    let updated = state
        .ops
        .patch_work_item(&principal.tenant_id, id, actor_opt(&principal), req)
        .await?;
    Ok(Json(updated))
}

async fn add_work_item_comment_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(req): ApiJson<CreateWorkItemCommentRequest>,
) -> Result<(StatusCode, Json<WorkItemEvent>), HttpError> {
    principal.require(Permission::FlowRun)?;
    let ev = state
        .ops
        .add_work_item_comment(&principal.tenant_id, id, actor_opt(&principal), req)
        .await?;
    Ok((StatusCode::CREATED, Json(ev)))
}

#[cfg(test)]
mod tests {
    #[test]
    fn axops_templates_router_builds_without_param_conflict() {
        let _router = super::router();
    }
}
