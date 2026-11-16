//! `AxFlow` DAG orchestration, versioning, deployments, runs, webhooks, approvals, and action log routes (`/api/axflow/*`).

use std::collections::BTreeMap;

use axpoint_domain::AppError;
use axpoint_domain::axhub::PaginatedList;
use axpoint_domain::enums::{AuditAction, Environment, Permission, TriggerType};
use axpoint_domain::flow::{
    ActionListQuery, ActionLogRecord, ApprovalListQuery, ApprovalRecord, DecideApprovalParams,
    DecideApprovalRequest, DeployFlowRequest, FlowDeploymentRecord, FlowGraph,
    FlowHookSecretResponse, FlowIssueSeverity, FlowListQuery, FlowVersionRecord,
    FlowVersionsListResponse, PublishFlowVersionRequest, RunListQuery, RunRecord,
    StartFlowRunRequest, StartFlowRunResponse, StartRunParams, StepRecord, UpdateAxFlowRequest,
    ValidateFlowResponse,
};
use axpoint_domain::platform::AuditRecord;
use axpoint_domain::ports::{ConnectionRepository as _, FlowRepository as _};
use axpoint_domain::topology::{AxFlowDef, CreateAxFlowRequest};
use axpoint_security::{AuthPrincipal, CredentialKeyring};
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Utc;
use serde::de::DeserializeOwned;
use subtle::ConstantTimeEq;
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::routes::open_secret_map;
use crate::state::{ApiJson, ApiPath, ApiQuery, AppState, HttpError};
use crate::telemetry::current_trace_id;

/// Maximum inbound JSON body size (`1 MiB`) accepted on public flow webhook endpoints.
const MAX_HOOK_BODY_BYTES: usize = 1_048_576;

/// Builds the `/axflow/*` router mounted under `/api` (`07-STEP07-08` §07.8).
pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/axflow/operators", get(list_operators_handler))
        .route(
            "/axflow/flows",
            get(list_flows_handler).post(create_flow_handler),
        )
        .route(
            "/axflow/flows/{id}",
            get(get_flow_handler)
                .put(update_flow_handler)
                .delete(delete_flow_handler),
        )
        .route("/axflow/flows/{id}/validate", post(validate_flow_handler))
        .route(
            "/axflow/flows/{id}/versions",
            get(list_flow_versions_handler).post(publish_flow_version_handler),
        )
        .route("/axflow/flows/{id}/deploy", post(deploy_flow_handler))
        .merge(runs_and_hooks_router())
}

fn runs_and_hooks_router() -> Router<AppState> {
    Router::new()
        .route(
            "/axflow/flows/{id}/runs",
            get(list_flow_runs_handler).post(start_flow_run_handler),
        )
        .route("/axflow/flows/{id}/run", post(run_flow_test_handler))
        .route("/axflow/runs/{run_id}", get(get_flow_run_handler))
        .route("/axflow/runs/{run_id}/steps", get(list_run_steps_handler))
        .route(
            "/axflow/runs/{run_id}/cancel",
            post(cancel_flow_run_handler),
        )
        .route("/axflow/runs/{run_id}/retry", post(retry_flow_run_handler))
        .route("/axflow/flows/{id}/hook", post(upsert_flow_hook_handler))
        .route(
            "/axflow/hooks/{tenant_id}/{slug}",
            post(invoke_flow_hook_handler),
        )
        .route("/axflow/approvals", get(list_approvals_handler))
        .route(
            "/axflow/approvals/{id}/decide",
            post(decide_approval_handler),
        )
        .route("/axflow/actions", get(list_actions_handler))
}

fn validate_page_limit(limit: Option<u32>) -> Result<(), AppError> {
    if let Some(val) = limit
        && !(1..=500).contains(&val)
    {
        return Err(AppError::bad(
            "LIMIT_OUT_OF_RANGE",
            format!("limit must be in 1..=500, got {val}"),
        ));
    }
    Ok(())
}

fn populate_flow_issues(flow: &mut AxFlowDef) {
    let graph = FlowGraph {
        nodes: flow.nodes.clone(),
        edges: flow.edges.clone(),
    };
    flow.issues =
        axpoint_flow::validate_flow_default(&graph, &flow.trigger_cfg, flow.slug.as_deref()).issues;
}

fn parse_optional_json<T: Default + DeserializeOwned>(body: &Bytes) -> Result<T, AppError> {
    if body.is_empty() || body.iter().all(u8::is_ascii_whitespace) {
        return Ok(T::default());
    }
    serde_json::from_slice::<T>(body)
        .map_err(|err| AppError::bad("INVALID_ARGUMENT", format!("invalid JSON payload: {err}")))
}

fn parse_hook_json_body(body: &[u8]) -> Result<serde_json::Value, AppError> {
    if body.is_empty() || body.iter().all(u8::is_ascii_whitespace) {
        return Ok(serde_json::Value::Object(serde_json::Map::new()));
    }
    serde_json::from_slice::<serde_json::Value>(body).map_err(|err| {
        AppError::bad(
            "INVALID_ARGUMENT",
            format!("webhook body must be valid JSON: {err}"),
        )
    })
}

fn normalize_optional_key(raw: Option<String>) -> Option<String> {
    raw.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty())
}

fn format_validation_failure(res: &ValidateFlowResponse) -> String {
    let messages: Vec<&str> = res
        .issues
        .iter()
        .filter(|issue| issue.severity == FlowIssueSeverity::Error)
        .map(|issue| issue.message.as_str())
        .take(5)
        .collect();
    if messages.is_empty() {
        String::from("flow draft failed static validation")
    } else {
        messages.join("; ")
    }
}

fn generate_hook_secret_hex() -> String {
    let mut bytes = [0_u8; 32];
    let left = Uuid::new_v4();
    let right = Uuid::new_v4();
    if let Some(first_half) = bytes.get_mut(..16) {
        first_half.copy_from_slice(left.as_bytes());
    }
    if let Some(second_half) = bytes.get_mut(16..) {
        second_half.copy_from_slice(right.as_bytes());
    }
    hex::encode(bytes)
}

fn extract_bearer_token(headers: &HeaderMap) -> Result<&str, AppError> {
    let auth_header = headers
        .get("authorization")
        .and_then(|val| val.to_str().ok())
        .map(str::trim)
        .filter(|val| !val.is_empty())
        .ok_or(AppError::Unauthorized("TOKEN_MISSING"))?;

    auth_header
        .strip_prefix("Bearer ")
        .or_else(|| auth_header.strip_prefix("bearer "))
        .map(str::trim)
        .filter(|tok| !tok.is_empty())
        .ok_or(AppError::Unauthorized("TOKEN_MISSING"))
}

fn verify_bearer_secret(provided: &str, expected: &str) -> Result<(), AppError> {
    if bool::from(provided.as_bytes().ct_eq(expected.as_bytes())) {
        Ok(())
    } else {
        Err(AppError::Unauthorized("TOKEN_INVALID"))
    }
}

async fn insert_hook_credential(
    state: &AppState,
    tenant_id: &str,
    secret: &str,
) -> Result<Uuid, AppError> {
    let cred_id = Uuid::new_v4();
    let mut secret_map = BTreeMap::new();
    secret_map.insert(String::from("token"), String::from(secret));
    let encoded =
        Zeroizing::new(serde_json::to_vec(&secret_map).map_err(|err| {
            AppError::Config(format!("serialize flow hook secret failed: {err}"))
        })?);
    let aad = CredentialKeyring::aad_for_credential(tenant_id, cred_id);
    let sealed = state
        .keyring
        .seal_credential(cred_id, &aad, encoded.as_slice())?;

    let mut tx = axpoint_db::begin_tenant_tx(state.store.pool(), tenant_id).await?;
    sqlx::query(
        "INSERT INTO ds_credentials (id, tenant_id, key_version, nonce, ciphertext)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(sealed.id)
    .bind(tenant_id)
    .bind(sealed.key_version)
    .bind(&sealed.nonce)
    .bind(&sealed.ciphertext)
    .execute(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("insert hook ds_credentials failed: {err}")))?;
    tx.commit().await.map_err(|err| {
        AppError::Database(format!("commit hook ds_credentials tx failed: {err}"))
    })?;

    Ok(cred_id)
}

fn record_flow_run_audit(
    state: &AppState,
    tenant_id: &str,
    user_id: Option<Uuid>,
    username: &str,
    run: &RunRecord,
) {
    let clean_user = user_id.filter(|uid| !uid.is_nil());
    state.audit.record(AuditRecord {
        tenant_id: String::from(tenant_id),
        user_id: clean_user,
        username: String::from(username),
        action: AuditAction::FlowRun,
        resource_ref: format!("axflow:run:{}", run.id),
        status_code: 202,
        client_ip: String::from("127.0.0.1"),
        trace_id: current_trace_id(),
        detail_json: serde_json::json!({
            "flow_id": run.flow_id,
            "version": run.version,
            "environment": run.environment,
            "trigger_source": run.trigger_source,
        }),
        created_at: Utc::now(),
    });
}

// ============================================================================
// Operators & Flow Draft CRUD / Validation
// ============================================================================

async fn list_operators_handler(
    principal: AuthPrincipal,
) -> Result<Json<serde_json::Value>, HttpError> {
    principal.require(Permission::DataRead)?;
    Ok(Json(serde_json::json!({
        "items": axpoint_flow::default_operator_specs(),
    })))
}

async fn list_flows_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiQuery(query): ApiQuery<FlowListQuery>,
) -> Result<Json<PaginatedList<AxFlowDef>>, HttpError> {
    principal.require(Permission::DataRead)?;
    validate_page_limit(query.limit)?;
    let mut page = state.store.list_flows(&principal.tenant_id, query).await?;
    for flow in &mut page.items {
        populate_flow_issues(flow);
    }
    Ok(Json(page))
}

async fn create_flow_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiJson(mut req): ApiJson<CreateAxFlowRequest>,
) -> Result<(StatusCode, Json<AxFlowDef>), HttpError> {
    principal.require(Permission::FlowEdit)?;
    let trimmed_name = req.name.trim();
    if trimmed_name.is_empty() {
        return Err(HttpError(AppError::bad(
            "INVALID_ARGUMENT",
            "flow name cannot be empty",
        )));
    }
    req.name = String::from(trimmed_name);

    let mut flow = state
        .store
        .create_flow(&principal.tenant_id, Some(principal.user_id), req)
        .await?;
    populate_flow_issues(&mut flow);
    Ok((StatusCode::CREATED, Json(flow)))
}

async fn get_flow_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<AxFlowDef>, HttpError> {
    principal.require(Permission::DataRead)?;
    let mut flow = state
        .store
        .get_flow(&principal.tenant_id, id)
        .await?
        .ok_or(AppError::NotFound("FLOW_NOT_FOUND"))?;
    populate_flow_issues(&mut flow);
    Ok(Json(flow))
}

async fn update_flow_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(mut req): ApiJson<UpdateAxFlowRequest>,
) -> Result<Json<AxFlowDef>, HttpError> {
    principal.require(Permission::FlowEdit)?;
    if let Some(ref name) = req.name {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return Err(HttpError(AppError::bad(
                "INVALID_ARGUMENT",
                "flow name cannot be empty",
            )));
        }
        req.name = Some(String::from(trimmed));
    }

    let mut flow = state
        .store
        .update_flow(&principal.tenant_id, id, req)
        .await?
        .ok_or(AppError::NotFound("FLOW_NOT_FOUND"))?;
    populate_flow_issues(&mut flow);
    Ok(Json(flow))
}

async fn delete_flow_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<serde_json::Value>, HttpError> {
    principal.require(Permission::FlowEdit)?;
    let deleted = state.store.delete_flow(&principal.tenant_id, id).await?;
    if !deleted {
        return Err(HttpError(AppError::NotFound("FLOW_NOT_FOUND")));
    }
    Ok(Json(serde_json::json!({ "deleted": true })))
}

async fn validate_flow_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<ValidateFlowResponse>, HttpError> {
    principal.require(Permission::FlowEdit)?;
    let flow = state
        .store
        .get_flow(&principal.tenant_id, id)
        .await?
        .ok_or(AppError::NotFound("FLOW_NOT_FOUND"))?;
    let graph = FlowGraph {
        nodes: flow.nodes,
        edges: flow.edges,
    };
    let res = axpoint_flow::validate_flow_default(&graph, &flow.trigger_cfg, flow.slug.as_deref());
    Ok(Json(res))
}

// ============================================================================
// Versions & Deployments
// ============================================================================

async fn list_flow_versions_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<FlowVersionsListResponse>, HttpError> {
    principal.require(Permission::DataRead)?;
    state
        .store
        .get_flow(&principal.tenant_id, id)
        .await?
        .ok_or(AppError::NotFound("FLOW_NOT_FOUND"))?;
    let resp = state
        .store
        .list_flow_versions(&principal.tenant_id, id)
        .await?;
    Ok(Json(resp))
}

async fn publish_flow_version_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
    body: Bytes,
) -> Result<(StatusCode, Json<FlowVersionRecord>), HttpError> {
    principal.require(Permission::FlowPublish)?;
    let req: PublishFlowVersionRequest = parse_optional_json(&body)?;

    let flow = state
        .store
        .get_flow(&principal.tenant_id, id)
        .await?
        .ok_or(AppError::NotFound("FLOW_NOT_FOUND"))?;

    let graph = FlowGraph {
        nodes: flow.nodes,
        edges: flow.edges,
    };
    let validation =
        axpoint_flow::validate_flow_default(&graph, &flow.trigger_cfg, flow.slug.as_deref());
    if !validation.valid {
        return Err(HttpError(AppError::unprocessable(
            "FLOW_INVALID",
            format_validation_failure(&validation),
        )));
    }

    let version_rec = state
        .store
        .publish_flow_version(&principal.tenant_id, id, Some(principal.user_id), req)
        .await?;
    Ok((StatusCode::CREATED, Json(version_rec)))
}

async fn deploy_flow_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(req): ApiJson<DeployFlowRequest>,
) -> Result<Json<FlowDeploymentRecord>, HttpError> {
    match req.environment {
        Environment::Test => principal.require(Permission::FlowEdit)?,
        Environment::Prod => principal.require(Permission::FlowPublish)?,
    }
    if req.version <= 0 {
        return Err(HttpError(AppError::bad(
            "INVALID_ARGUMENT",
            "version must be >= 1",
        )));
    }

    let deployment = state
        .store
        .deploy_flow(&principal.tenant_id, id, Some(principal.user_id), req)
        .await?;
    Ok(Json(deployment))
}

// ============================================================================
// Runs & Step Logs
// ============================================================================

async fn start_manual_run(
    state: &AppState,
    principal: &AuthPrincipal,
    flow_id: Uuid,
    environment: Environment,
    req: StartFlowRunRequest,
) -> Result<(StatusCode, Json<StartFlowRunResponse>), HttpError> {
    let (run, existing) = state
        .store
        .start_flow_run(
            &principal.tenant_id,
            StartRunParams {
                flow_id,
                environment,
                explicit_version: req.version,
                trigger_source: TriggerType::Manual,
                trigger_payload: req.payload,
                idempotency_key: normalize_optional_key(req.idempotency_key),
                started_by: Some(principal.user_id),
            },
        )
        .await?;

    if !existing {
        record_flow_run_audit(
            state,
            &principal.tenant_id,
            Some(principal.user_id),
            &principal.username,
            &run,
        );
    }

    Ok((
        StatusCode::ACCEPTED,
        Json(StartFlowRunResponse {
            run_id: run.id,
            status: run.status,
            existing,
        }),
    ))
}

async fn start_flow_run_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
    body: Bytes,
) -> Result<(StatusCode, Json<StartFlowRunResponse>), HttpError> {
    principal.require(Permission::FlowRun)?;
    let req: StartFlowRunRequest = parse_optional_json(&body)?;
    let environment = req.environment.unwrap_or(Environment::Test);
    start_manual_run(&state, &principal, id, environment, req).await
}

async fn run_flow_test_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
    body: Bytes,
) -> Result<(StatusCode, Json<StartFlowRunResponse>), HttpError> {
    principal.require(Permission::FlowRun)?;
    let req: StartFlowRunRequest = parse_optional_json(&body)?;
    start_manual_run(&state, &principal, id, Environment::Test, req).await
}

async fn list_flow_runs_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
    ApiQuery(query): ApiQuery<RunListQuery>,
) -> Result<Json<PaginatedList<RunRecord>>, HttpError> {
    principal.require(Permission::DataRead)?;
    validate_page_limit(query.limit)?;
    state
        .store
        .get_flow(&principal.tenant_id, id)
        .await?
        .ok_or(AppError::NotFound("FLOW_NOT_FOUND"))?;
    let page = state
        .store
        .list_flow_runs(&principal.tenant_id, id, query)
        .await?;
    Ok(Json(page))
}

async fn get_flow_run_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(run_id): ApiPath<Uuid>,
) -> Result<Json<RunRecord>, HttpError> {
    principal.require(Permission::DataRead)?;
    let run = state
        .store
        .get_flow_run(&principal.tenant_id, run_id)
        .await?
        .ok_or(AppError::NotFound("RUN_NOT_FOUND"))?;
    Ok(Json(run))
}

async fn list_run_steps_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(run_id): ApiPath<Uuid>,
) -> Result<Json<PaginatedList<StepRecord>>, HttpError> {
    principal.require(Permission::DataRead)?;
    state
        .store
        .get_flow_run(&principal.tenant_id, run_id)
        .await?
        .ok_or(AppError::NotFound("RUN_NOT_FOUND"))?;
    let steps = state
        .store
        .list_run_steps(&principal.tenant_id, run_id)
        .await?;
    Ok(Json(PaginatedList {
        items: steps,
        next_cursor: None,
    }))
}

async fn cancel_flow_run_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(run_id): ApiPath<Uuid>,
) -> Result<Json<RunRecord>, HttpError> {
    principal.require(Permission::FlowRun)?;
    let run = state
        .store
        .cancel_flow_run(&principal.tenant_id, run_id)
        .await?;
    axpoint_flow::signal_run_canceled(run_id);
    Ok(Json(run))
}

async fn retry_flow_run_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(run_id): ApiPath<Uuid>,
) -> Result<Json<RunRecord>, HttpError> {
    principal.require(Permission::FlowRun)?;
    let run = state
        .store
        .retry_flow_run(&principal.tenant_id, run_id)
        .await?;
    Ok(Json(run))
}

// ============================================================================
// Inbound Flow Webhooks
// ============================================================================

async fn upsert_flow_hook_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<FlowHookSecretResponse>, HttpError> {
    principal.require(Permission::FlowPublish)?;
    let flow = state
        .store
        .get_flow(&principal.tenant_id, id)
        .await?
        .ok_or(AppError::NotFound("FLOW_NOT_FOUND"))?;

    let slug = flow
        .slug
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            AppError::bad(
                "INVALID_ARGUMENT",
                "flow slug must be set before creating or rotating a webhook",
            )
        })?
        .to_owned();

    let secret = generate_hook_secret_hex();
    let credential_id = insert_hook_credential(&state, &principal.tenant_id, &secret).await?;
    let (hook_id, created_at) = state
        .store
        .upsert_flow_hook(&principal.tenant_id, id, credential_id)
        .await?;
    let path = format!("/api/axflow/hooks/{}/{slug}", principal.tenant_id);

    Ok(Json(FlowHookSecretResponse {
        id: hook_id,
        flow_id: id,
        slug,
        path,
        secret,
        created_at,
    }))
}

async fn invoke_flow_hook_handler(
    State(state): State<AppState>,
    ApiPath((tenant_id, slug)): ApiPath<(String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<(StatusCode, Json<StartFlowRunResponse>), HttpError> {
    let resolved = state
        .store
        .resolve_flow_hook(&tenant_id, &slug)
        .await?
        .ok_or(AppError::NotFound("FLOW_NOT_FOUND"))?;

    let provided_token = extract_bearer_token(&headers)?;
    let sealed = state
        .store
        .get_credential(&tenant_id, resolved.credential_id)
        .await?
        .ok_or(AppError::Unauthorized("TOKEN_INVALID"))?;
    let secret_map = open_secret_map(&state.keyring, &tenant_id, Some(&sealed))?;
    let expected_token = secret_map
        .get("token")
        .ok_or(AppError::Unauthorized("TOKEN_INVALID"))?;
    verify_bearer_secret(provided_token, expected_token)?;

    if body.len() > MAX_HOOK_BODY_BYTES {
        return Err(HttpError(AppError::PayloadTooLarge("PAYLOAD_TOO_LARGE")));
    }

    let deployment = state
        .store
        .get_flow_deployment(&tenant_id, resolved.flow_id, Environment::Prod)
        .await?
        .ok_or_else(|| {
            AppError::unprocessable("FLOW_NOT_DEPLOYED", "flow has no active PROD deployment")
        })?;
    let version_rec = state
        .store
        .get_flow_version(&tenant_id, resolved.flow_id, deployment.version)
        .await?
        .ok_or_else(|| {
            AppError::unprocessable("FLOW_NOT_DEPLOYED", "deployed PROD version not found")
        })?;
    if version_rec.trigger_cfg.kind != TriggerType::Webhook {
        return Err(HttpError(AppError::unprocessable(
            "TRIGGER_MISMATCH",
            "deployed PROD flow version trigger type is not WEBHOOK",
        )));
    }

    let trigger_payload = parse_hook_json_body(&body)?;
    let idempotency_key = headers
        .get("idempotency-key")
        .and_then(|val| val.to_str().ok())
        .map(str::trim)
        .filter(|val| !val.is_empty())
        .map(String::from);

    let (run, existing) = state
        .store
        .start_flow_run(
            &tenant_id,
            StartRunParams {
                flow_id: resolved.flow_id,
                environment: Environment::Prod,
                explicit_version: Some(deployment.version),
                trigger_source: TriggerType::Webhook,
                trigger_payload,
                idempotency_key,
                started_by: None,
            },
        )
        .await?;

    if !existing {
        record_flow_run_audit(&state, &tenant_id, None, "webhook", &run);
    }

    Ok((
        StatusCode::ACCEPTED,
        Json(StartFlowRunResponse {
            run_id: run.id,
            status: run.status,
            existing,
        }),
    ))
}

// ============================================================================
// Human Approvals & Writeback Action Logs
// ============================================================================

async fn list_approvals_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiQuery(query): ApiQuery<ApprovalListQuery>,
) -> Result<Json<PaginatedList<ApprovalRecord>>, HttpError> {
    principal.require(Permission::DataRead)?;
    validate_page_limit(query.limit)?;
    let page = state
        .store
        .list_approvals(
            &principal.tenant_id,
            principal.user_id,
            &principal.roles,
            query,
        )
        .await?;
    Ok(Json(page))
}

async fn decide_approval_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(req): ApiJson<DecideApprovalRequest>,
) -> Result<Json<ApprovalRecord>, HttpError> {
    principal.require(Permission::ApprovalDecide)?;
    let approval = state
        .store
        .decide_approval(
            &principal.tenant_id,
            DecideApprovalParams {
                approval_id: id,
                actor_user_id: principal.user_id,
                actor_roles: principal.roles.clone(),
                decision: req.decision,
                comment: req.comment,
            },
        )
        .await?;
    Ok(Json(approval))
}

async fn list_actions_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiQuery(query): ApiQuery<ActionListQuery>,
) -> Result<Json<PaginatedList<ActionLogRecord>>, HttpError> {
    principal.require(Permission::DataRead)?;
    validate_page_limit(query.limit)?;
    let page = state
        .store
        .list_action_logs(&principal.tenant_id, query)
        .await?;
    Ok(Json(page))
}

impl axpoint_domain::ports::FlowCredentialUnsealPort for AppState {
    fn unseal_credential_json<'a>(
        &'a self,
        tenant_id: &'a str,
        credential_id: Uuid,
    ) -> axpoint_domain::platform::BoxFuture<'a, serde_json::Value> {
        Box::pin(async move {
            let sealed = self
                .store
                .get_credential(tenant_id, credential_id)
                .await?
                .ok_or(AppError::NotFound("CREDENTIAL_NOT_FOUND"))?;
            let secret_map = open_secret_map(&self.keyring, tenant_id, Some(&sealed))?;
            let mut obj = serde_json::Map::with_capacity(secret_map.len());
            for (k, v) in &secret_map {
                obj.insert(k.clone(), serde_json::Value::String(v.clone()));
            }
            Ok(serde_json::Value::Object(obj))
        })
    }
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;

    #[test]
    fn optional_json_accepts_empty_and_rejects_unknown_fields() -> Result<(), AppError> {
        let empty = parse_optional_json::<PublishFlowVersionRequest>(&Bytes::new())?;
        assert_eq!(empty.note, "");

        let whitespace =
            parse_optional_json::<PublishFlowVersionRequest>(&Bytes::from_static(b"   \n\t "))?;
        assert_eq!(whitespace.note, "");

        let valid = parse_optional_json::<PublishFlowVersionRequest>(&Bytes::from_static(
            b"{\"note\":\"release v1\"}",
        ))?;
        assert_eq!(valid.note, "release v1");

        let bad = parse_optional_json::<PublishFlowVersionRequest>(&Bytes::from_static(
            b"{\"tenant_id\":\"other\"}",
        ));
        assert!(matches!(
            bad,
            Err(AppError::BadRequest {
                code: "INVALID_ARGUMENT",
                ..
            })
        ));
        Ok(())
    }

    #[test]
    fn hook_auth_and_secret_generation_work() -> Result<(), AppError> {
        let secret = generate_hook_secret_hex();
        assert_eq!(secret.len(), 64);
        assert!(secret.chars().all(|c| c.is_ascii_hexdigit()));

        let mut headers = HeaderMap::new();
        assert!(matches!(
            extract_bearer_token(&headers),
            Err(AppError::Unauthorized("TOKEN_MISSING"))
        ));

        let header_val = HeaderValue::from_str(&format!("Bearer {secret}"))
            .map_err(|err| AppError::Config(format!("header parse: {err}")))?;
        headers.insert("authorization", header_val);
        let parsed = extract_bearer_token(&headers)?;
        verify_bearer_secret(parsed, &secret)?;
        assert!(matches!(
            verify_bearer_secret("wrong_secret", &secret),
            Err(AppError::Unauthorized("TOKEN_INVALID"))
        ));
        Ok(())
    }

    #[test]
    fn hook_body_and_limit_validation_work() -> Result<(), AppError> {
        let empty = parse_hook_json_body(b"  ")?;
        assert_eq!(empty, serde_json::json!({}));

        let obj = parse_hook_json_body(b"{\"order_id\":42}")?;
        assert_eq!(obj, serde_json::json!({"order_id": 42}));

        assert!(parse_hook_json_body(b"{broken").is_err());
        validate_page_limit(None)?;
        validate_page_limit(Some(500))?;
        assert!(matches!(
            validate_page_limit(Some(0)),
            Err(AppError::BadRequest {
                code: "LIMIT_OUT_OF_RANGE",
                ..
            })
        ));
        Ok(())
    }
}
