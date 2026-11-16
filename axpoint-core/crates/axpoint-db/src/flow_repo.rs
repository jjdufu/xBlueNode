//! `PostgreSQL` persistence for `AxFlow` DAG definitions, versions, deployments, runs,
//! step logs, human approvals, writeback action logs, and inbound flow webhooks (`07-STEP07-08` §07).

use std::collections::BTreeSet;
use std::str::FromStr;

use axpoint_domain::AppError;
use axpoint_domain::axhub::PaginatedList;
use axpoint_domain::enums::{
    AuditAction, Environment, JobKind, OutboxTopic, RunStatus, StepStatus, TriggerType,
};
use axpoint_domain::flow::{
    ActionKind, ActionListQuery, ActionLogRecord, ActionLogStatus, ActionSourceKind,
    ApprovalDecision, ApprovalListQuery, ApprovalRecord, ApprovalSourceKind, ApprovalStatus,
    DecideApprovalParams, DeployFlowRequest, EventFlowItem, FlowDeploymentRecord, FlowEdge,
    FlowListQuery, FlowNode, FlowStepJobPayload, FlowVersionRecord, FlowVersionsListResponse,
    NewActionLogInput, NewApprovalInput, ProgressionPlannerFn, PublishFlowVersionRequest,
    ResolvedFlowHook, RunListQuery, RunProgressionPlan, RunRecord, ScheduledFlowItem,
    StartRunParams, StepMutation, StepRecord, TriggerCfg, UpdateAxFlowRequest,
};
use axpoint_domain::platform::{NewJob, NewOutboxEvent};
use axpoint_domain::ports::FlowRepository;
use axpoint_domain::topology::{AxFlowDef, CreateAxFlowRequest};
use chrono::{DateTime, Utc};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

use crate::PgStore;
use crate::jobs::enqueue_job_tx;
use crate::outbox::insert_outbox_tx;
use crate::tenant::begin_tenant_tx;

fn is_unique_violation(err: &sqlx::Error) -> bool {
    err.as_database_error()
        .and_then(sqlx::error::DatabaseError::code)
        .is_some_and(|code| code == "23505")
}

fn validate_page_limit(raw_limit: Option<u32>) -> Result<(usize, i64), AppError> {
    let limit = raw_limit.unwrap_or(50);
    if !(1..=500).contains(&limit) {
        return Err(AppError::bad(
            "LIMIT_OUT_OF_RANGE",
            format!("limit must be in 1..=500, got {limit}"),
        ));
    }
    let limit_usize = usize::try_from(limit).unwrap_or(50);
    let fetch_limit = i64::from(limit).saturating_add(1);
    Ok((limit_usize, fetch_limit))
}

fn parse_ts_uuid_cursor(raw_cursor: &str) -> Result<(DateTime<Utc>, Uuid), AppError> {
    let Some((ts_part, id_part)) = raw_cursor.split_once('|') else {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "cursor must have format <rfc3339>|<uuid>",
        ));
    };
    let parsed_ts = DateTime::parse_from_rfc3339(ts_part)
        .map_err(|err| AppError::bad("INVALID_ARGUMENT", format!("invalid cursor ts: {err}")))?
        .with_timezone(&Utc);
    let parsed_id = Uuid::parse_str(id_part)
        .map_err(|err| AppError::bad("INVALID_ARGUMENT", format!("invalid cursor id: {err}")))?;
    Ok((parsed_ts, parsed_id))
}

fn parse_ts_i64_cursor(raw_cursor: &str) -> Result<(DateTime<Utc>, i64), AppError> {
    let Some((ts_part, id_part)) = raw_cursor.split_once('|') else {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "cursor must have format <rfc3339>|<id>",
        ));
    };
    let parsed_ts = DateTime::parse_from_rfc3339(ts_part)
        .map_err(|err| AppError::bad("INVALID_ARGUMENT", format!("invalid cursor ts: {err}")))?
        .with_timezone(&Utc);
    let parsed_id = id_part
        .parse::<i64>()
        .map_err(|err| AppError::bad("INVALID_ARGUMENT", format!("invalid cursor id: {err}")))?;
    Ok((parsed_ts, parsed_id))
}

const fn approval_source_kind_as_str(kind: ApprovalSourceKind) -> &'static str {
    match kind {
        ApprovalSourceKind::Flow => "FLOW",
        ApprovalSourceKind::SimApply => "SIM_APPLY",
        ApprovalSourceKind::Agent => "AGENT",
    }
}

fn parse_approval_source_kind(raw: &str) -> Result<ApprovalSourceKind, AppError> {
    match raw {
        "FLOW" => Ok(ApprovalSourceKind::Flow),
        "SIM_APPLY" => Ok(ApprovalSourceKind::SimApply),
        "AGENT" => Ok(ApprovalSourceKind::Agent),
        _ => Err(AppError::bad(
            "INVALID_ARGUMENT",
            format!("unknown ApprovalSourceKind value: {raw}"),
        )),
    }
}

const fn approval_status_as_str(status: ApprovalStatus) -> &'static str {
    match status {
        ApprovalStatus::Pending => "PENDING",
        ApprovalStatus::Approved => "APPROVED",
        ApprovalStatus::Rejected => "REJECTED",
        ApprovalStatus::Expired => "EXPIRED",
    }
}

fn parse_approval_status(raw: &str) -> Result<ApprovalStatus, AppError> {
    match raw {
        "PENDING" => Ok(ApprovalStatus::Pending),
        "APPROVED" => Ok(ApprovalStatus::Approved),
        "REJECTED" => Ok(ApprovalStatus::Rejected),
        "EXPIRED" => Ok(ApprovalStatus::Expired),
        _ => Err(AppError::bad(
            "INVALID_ARGUMENT",
            format!("unknown ApprovalStatus value: {raw}"),
        )),
    }
}

const fn action_source_kind_as_str(kind: ActionSourceKind) -> &'static str {
    match kind {
        ActionSourceKind::Flow => "FLOW",
        ActionSourceKind::SimApply => "SIM_APPLY",
        ActionSourceKind::Agent => "AGENT",
        ActionSourceKind::Rule => "RULE",
        ActionSourceKind::View => "VIEW",
    }
}

fn parse_action_source_kind(raw: &str) -> Result<ActionSourceKind, AppError> {
    match raw {
        "FLOW" => Ok(ActionSourceKind::Flow),
        "SIM_APPLY" => Ok(ActionSourceKind::SimApply),
        "AGENT" => Ok(ActionSourceKind::Agent),
        "RULE" => Ok(ActionSourceKind::Rule),
        "VIEW" => Ok(ActionSourceKind::View),
        _ => Err(AppError::bad(
            "INVALID_ARGUMENT",
            format!("unknown ActionSourceKind value: {raw}"),
        )),
    }
}

const fn action_kind_as_str(kind: ActionKind) -> &'static str {
    match kind {
        ActionKind::WriteNodeMetric => "WRITE_NODE_METRIC",
        ActionKind::WriteNodeProperty => "WRITE_NODE_PROPERTY",
        ActionKind::Http => "HTTP",
        ActionKind::Notify => "NOTIFY",
        ActionKind::WorkItem => "WORK_ITEM",
    }
}

fn parse_action_kind(raw: &str) -> Result<ActionKind, AppError> {
    match raw {
        "WRITE_NODE_METRIC" => Ok(ActionKind::WriteNodeMetric),
        "WRITE_NODE_PROPERTY" => Ok(ActionKind::WriteNodeProperty),
        "HTTP" => Ok(ActionKind::Http),
        "NOTIFY" => Ok(ActionKind::Notify),
        "WORK_ITEM" => Ok(ActionKind::WorkItem),
        _ => Err(AppError::bad(
            "INVALID_ARGUMENT",
            format!("unknown ActionKind value: {raw}"),
        )),
    }
}

const fn action_log_status_as_str(status: ActionLogStatus) -> &'static str {
    match status {
        ActionLogStatus::Done => "DONE",
        ActionLogStatus::DryRun => "DRY_RUN",
        ActionLogStatus::Failed => "FAILED",
    }
}

fn parse_action_log_status(raw: &str) -> Result<ActionLogStatus, AppError> {
    match raw {
        "DONE" => Ok(ActionLogStatus::Done),
        "DRY_RUN" => Ok(ActionLogStatus::DryRun),
        "FAILED" => Ok(ActionLogStatus::Failed),
        _ => Err(AppError::bad(
            "INVALID_ARGUMENT",
            format!("unknown ActionLogStatus value: {raw}"),
        )),
    }
}

fn decode_flow_def(row: &sqlx::postgres::PgRow) -> Result<AxFlowDef, AppError> {
    let nodes: Vec<FlowNode> = serde_json::from_value(row.get("nodes"))
        .map_err(|err| AppError::Database(format!("decode ax_flows.nodes failed: {err}")))?;
    let edges: Vec<FlowEdge> = serde_json::from_value(row.get("edges"))
        .map_err(|err| AppError::Database(format!("decode ax_flows.edges failed: {err}")))?;
    let trigger_cfg: TriggerCfg = serde_json::from_value(row.get("trigger_cfg"))
        .map_err(|err| AppError::Database(format!("decode ax_flows.trigger_cfg failed: {err}")))?;

    Ok(AxFlowDef {
        id: row.get("id"),
        name: row.get("name"),
        description: row.get("description"),
        slug: row.get("slug"),
        nodes,
        edges,
        trigger_cfg,
        status: row.get("status"),
        created_by: row.get("created_by"),
        last_run_at: row.get("last_run_at"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
        issues: Vec::new(),
    })
}

fn decode_flow_version(row: &sqlx::postgres::PgRow) -> Result<FlowVersionRecord, AppError> {
    let nodes: Vec<FlowNode> = serde_json::from_value(row.get("nodes")).map_err(|err| {
        AppError::Database(format!("decode ax_flow_versions.nodes failed: {err}"))
    })?;
    let edges: Vec<FlowEdge> = serde_json::from_value(row.get("edges")).map_err(|err| {
        AppError::Database(format!("decode ax_flow_versions.edges failed: {err}"))
    })?;
    let trigger_cfg: TriggerCfg =
        serde_json::from_value(row.get("trigger_cfg")).map_err(|err| {
            AppError::Database(format!("decode ax_flow_versions.trigger_cfg failed: {err}"))
        })?;

    Ok(FlowVersionRecord {
        id: row.get("id"),
        flow_id: row.get("flow_id"),
        version: row.get("version"),
        nodes,
        edges,
        trigger_cfg,
        note: row.get("note"),
        published_by: row.get("published_by"),
        published_at: row.get("published_at"),
    })
}

fn decode_flow_deployment(row: &sqlx::postgres::PgRow) -> Result<FlowDeploymentRecord, AppError> {
    let env_raw: String = row.get("environment");
    let environment = Environment::from_str(&env_raw)?;
    Ok(FlowDeploymentRecord {
        flow_id: row.get("flow_id"),
        environment,
        version: row.get("version"),
        deployed_by: row.get("deployed_by"),
        deployed_at: row.get("deployed_at"),
    })
}

fn decode_run_record(row: &sqlx::postgres::PgRow) -> Result<RunRecord, AppError> {
    let env_raw: String = row.get("environment");
    let trig_raw: String = row.get("trigger_source");
    let status_raw: String = row.get("status");

    Ok(RunRecord {
        id: row.get("id"),
        flow_id: row.get("flow_id"),
        version: row.get("version"),
        environment: Environment::from_str(&env_raw)?,
        trigger_source: TriggerType::from_str(&trig_raw)?,
        trigger_payload: row.get("trigger_payload"),
        idempotency_key: row.get("idempotency_key"),
        status: RunStatus::from_str(&status_raw)?,
        outputs: row.get("outputs"),
        error_message: row.get("error_message"),
        started_by: row.get("started_by"),
        created_at: row.get("created_at"),
        started_at: row.get("started_at"),
        finished_at: row.get("finished_at"),
    })
}

fn decode_step_record(row: &sqlx::postgres::PgRow) -> Result<StepRecord, AppError> {
    let status_raw: String = row.get("status");
    Ok(StepRecord {
        id: row.get("id"),
        run_id: row.get("run_id"),
        node_id: row.get("node_id"),
        attempt: row.get("attempt"),
        status: StepStatus::from_str(&status_raw)?,
        job_id: row.get("job_id"),
        input: row.get("input"),
        output: row.get("output"),
        output_ref: row.get("output_ref"),
        error_code: row.get("error_code"),
        error_message: row.get("error_message"),
        started_at: row.get("started_at"),
        finished_at: row.get("finished_at"),
    })
}

fn decode_approval_record(row: &sqlx::postgres::PgRow) -> Result<ApprovalRecord, AppError> {
    let source_kind_raw: String = row.get("source_kind");
    let status_raw: String = row.get("status");
    Ok(ApprovalRecord {
        id: row.get("id"),
        run_id: row.get("run_id"),
        node_id: row.get("node_id"),
        source_kind: parse_approval_source_kind(&source_kind_raw)?,
        source_ref: row.get("source_ref"),
        title: row.get("title"),
        detail: row.get("detail"),
        assignee_roles: row.get("assignee_roles"),
        assignee_users: row.get("assignee_users"),
        status: parse_approval_status(&status_raw)?,
        decided_by: row.get("decided_by"),
        decided_at: row.get("decided_at"),
        comment: row.get("comment"),
        expires_at: row.get("expires_at"),
        created_at: row.get("created_at"),
    })
}

fn decode_action_log_record(row: &sqlx::postgres::PgRow) -> Result<ActionLogRecord, AppError> {
    let source_kind_raw: String = row.get("source_kind");
    let action_kind_raw: String = row.get("action_kind");
    let status_raw: String = row.get("status");
    Ok(ActionLogRecord {
        id: row.get("id"),
        run_id: row.get("run_id"),
        node_id: row.get("node_id"),
        source_kind: parse_action_source_kind(&source_kind_raw)?,
        action_kind: parse_action_kind(&action_kind_raw)?,
        target: row.get("target"),
        payload: row.get("payload"),
        status: parse_action_log_status(&status_raw)?,
        idempotency_key: row.get("idempotency_key"),
        error_message: row.get("error_message"),
        created_at: row.get("created_at"),
    })
}

async fn ensure_flow_exists_tx(
    tx: &mut Transaction<'_, Postgres>,
    flow_id: Uuid,
) -> Result<(), AppError> {
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM ax_flows WHERE id = $1)")
        .bind(flow_id)
        .fetch_one(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("check ax_flows exists failed: {err}")))?;
    if !exists {
        return Err(AppError::NotFound("FLOW_NOT_FOUND"));
    }
    Ok(())
}

async fn ensure_run_exists_tx(
    tx: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
) -> Result<(), AppError> {
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM ax_flow_runs WHERE id = $1)")
            .bind(run_id)
            .fetch_one(&mut **tx)
            .await
            .map_err(|err| {
                AppError::Database(format!("check ax_flow_runs exists failed: {err}"))
            })?;
    if !exists {
        return Err(AppError::NotFound("NOT_FOUND"));
    }
    Ok(())
}

async fn fetch_flow_version_tx(
    tx: &mut Transaction<'_, Postgres>,
    flow_id: Uuid,
    version: i32,
) -> Result<Option<FlowVersionRecord>, AppError> {
    let maybe_row = sqlx::query(
        "SELECT id, flow_id, version, nodes, edges, trigger_cfg, note, published_by, published_at
           FROM ax_flow_versions
          WHERE flow_id = $1 AND version = $2",
    )
    .bind(flow_id)
    .bind(version)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("fetch ax_flow_versions failed: {err}")))?;

    maybe_row.as_ref().map(decode_flow_version).transpose()
}

async fn resolve_run_version_tx(
    tx: &mut Transaction<'_, Postgres>,
    flow_id: Uuid,
    environment: Environment,
    explicit_version: Option<i32>,
) -> Result<FlowVersionRecord, AppError> {
    if let Some(version) = explicit_version {
        return fetch_flow_version_tx(tx, flow_id, version)
            .await?
            .ok_or(AppError::NotFound("NOT_FOUND"));
    }

    let deployed: Option<i32> = sqlx::query_scalar(
        "SELECT version FROM ax_flow_deployments WHERE flow_id = $1 AND environment = $2",
    )
    .bind(flow_id)
    .bind(environment.as_str())
    .fetch_optional(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("query ax_flow_deployments failed: {err}")))?;

    let target_version = if let Some(ver) = deployed {
        ver
    } else if environment == Environment::Test {
        let max_ver: Option<i32> =
            sqlx::query_scalar("SELECT MAX(version) FROM ax_flow_versions WHERE flow_id = $1")
                .bind(flow_id)
                .fetch_one(&mut **tx)
                .await
                .map_err(|err| {
                    AppError::Database(format!("query max ax_flow_versions failed: {err}"))
                })?;
        max_ver.ok_or_else(|| {
            AppError::unprocessable(
                "FLOW_NOT_PUBLISHED",
                "flow has no published or deployed version",
            )
        })?
    } else {
        return Err(AppError::unprocessable(
            "FLOW_NOT_PUBLISHED",
            "flow has no published or deployed version",
        ));
    };

    fetch_flow_version_tx(tx, flow_id, target_version)
        .await?
        .ok_or_else(|| {
            AppError::unprocessable(
                "FLOW_NOT_PUBLISHED",
                "flow has no published or deployed version",
            )
        })
}

async fn enqueue_flow_step_job_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    run_id: Uuid,
    node_id: &str,
    attempt: i32,
    next_run_at: Option<DateTime<Utc>>,
) -> Result<Uuid, AppError> {
    let step_payload = FlowStepJobPayload {
        run_id,
        node_id: String::from(node_id),
        attempt,
    };
    let idem_key = step_payload.idempotency_key();
    let payload_json = serde_json::to_value(&step_payload)
        .map_err(|err| AppError::Database(format!("serialize FlowStepJobPayload failed: {err}")))?;

    let job = NewJob {
        kind: JobKind::FlowStep,
        payload: payload_json,
        priority: 100,
        max_attempts: 5,
        next_run_at,
        idempotency_key: Some(idem_key),
    };
    enqueue_job_tx(tx, tenant_id, &job).await
}

async fn insert_pending_step_and_enqueue_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    run_id: Uuid,
    node_id: &str,
    attempt: i32,
    next_run_at: Option<DateTime<Utc>>,
) -> Result<bool, AppError> {
    let inserted: Option<i64> = sqlx::query_scalar(
        "INSERT INTO ax_flow_step_logs (tenant_id, run_id, node_id, attempt, status)
         VALUES ($1, $2, $3, $4, 'PENDING')
         ON CONFLICT (run_id, node_id, attempt) DO NOTHING
         RETURNING id",
    )
    .bind(tenant_id)
    .bind(run_id)
    .bind(node_id)
    .bind(attempt)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("insert pending ax_flow_step_logs failed: {err}")))?;

    let Some(step_log_id) = inserted else {
        return Ok(false);
    };

    let job_id =
        enqueue_flow_step_job_tx(tx, tenant_id, run_id, node_id, attempt, next_run_at).await?;
    sqlx::query("UPDATE ax_flow_step_logs SET job_id = $1 WHERE id = $2")
        .bind(job_id)
        .bind(step_log_id)
        .execute(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("set ax_flow_step_logs.job_id failed: {err}")))?;

    Ok(true)
}

async fn lock_run_for_update_tx(
    tx: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
) -> Result<RunRecord, AppError> {
    let row = sqlx::query(
        "SELECT id, flow_id, version, environment, trigger_source,
                trigger_payload, idempotency_key, status, outputs, error_message,
                started_by, created_at, started_at, finished_at
           FROM ax_flow_runs
          WHERE id = $1
          FOR UPDATE",
    )
    .bind(run_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("lock ax_flow_runs FOR UPDATE failed: {err}")))?
    .ok_or(AppError::NotFound("NOT_FOUND"))?;

    decode_run_record(&row)
}

async fn fetch_latest_run_steps_tx(
    tx: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
) -> Result<Vec<StepRecord>, AppError> {
    let rows = sqlx::query(
        "SELECT DISTINCT ON (node_id)
                id, run_id, node_id, attempt, status, job_id, input,
                output, output_ref, error_code, error_message, started_at, finished_at
           FROM ax_flow_step_logs
          WHERE run_id = $1
          ORDER BY node_id ASC, attempt DESC",
    )
    .bind(run_id)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("query latest ax_flow_step_logs failed: {err}")))?;

    let mut steps = Vec::with_capacity(rows.len());
    for row in rows {
        steps.push(decode_step_record(&row)?);
    }
    steps.sort_by_key(|s| s.id);
    Ok(steps)
}

async fn upsert_failed_step_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    run_id: Uuid,
    node_id: &str,
    attempt: i32,
    error_code: &str,
    error_message: &str,
) -> Result<(), AppError> {
    sqlx::query(
        "INSERT INTO ax_flow_step_logs (
             tenant_id, run_id, node_id, attempt, status, error_code, error_message,
             started_at, finished_at
         )
         VALUES ($1, $2, $3, $4, 'FAILED', $5, $6, NOW(), NOW())
         ON CONFLICT (run_id, node_id, attempt) DO UPDATE SET
             status = 'FAILED',
             error_code = $5,
             error_message = $6,
             started_at = COALESCE(ax_flow_step_logs.started_at, NOW()),
             finished_at = NOW()",
    )
    .bind(tenant_id)
    .bind(run_id)
    .bind(node_id)
    .bind(attempt)
    .bind(error_code)
    .bind(error_message)
    .execute(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("mark step FAILED failed: {err}")))?;
    Ok(())
}

async fn upsert_succeeded_step_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    run_id: Uuid,
    node_id: &str,
    attempt: i32,
    output: Option<&serde_json::Value>,
    output_ref: Option<&str>,
) -> Result<(), AppError> {
    sqlx::query(
        "INSERT INTO ax_flow_step_logs (
             tenant_id, run_id, node_id, attempt, status, output, output_ref,
             started_at, finished_at
         )
         VALUES ($1, $2, $3, $4, 'SUCCEEDED', $5, $6, NOW(), NOW())
         ON CONFLICT (run_id, node_id, attempt) DO UPDATE SET
             status = 'SUCCEEDED',
             output = $5,
             output_ref = $6,
             error_code = NULL,
             error_message = NULL,
             started_at = COALESCE(ax_flow_step_logs.started_at, NOW()),
             finished_at = NOW()",
    )
    .bind(tenant_id)
    .bind(run_id)
    .bind(node_id)
    .bind(attempt)
    .bind(output)
    .bind(output_ref)
    .execute(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("mark step SUCCEEDED failed: {err}")))?;
    Ok(())
}

async fn apply_step_mutation_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    run: &RunRecord,
    mutation: &StepMutation,
) -> Result<(), AppError> {
    match mutation {
        StepMutation::Succeeded {
            node_id,
            attempt,
            output,
            output_ref,
        } => {
            upsert_succeeded_step_tx(
                tx,
                tenant_id,
                run.id,
                node_id,
                *attempt,
                output.as_ref(),
                output_ref.as_deref(),
            )
            .await?;
        }
        StepMutation::Retry {
            node_id,
            attempt,
            error_code,
            error_message,
            next_run_at,
        } => {
            upsert_failed_step_tx(
                tx,
                tenant_id,
                run.id,
                node_id,
                *attempt,
                error_code,
                error_message,
            )
            .await?;
            if run.status != RunStatus::Canceled {
                let next_attempt = attempt.saturating_add(1);
                insert_pending_step_and_enqueue_tx(
                    tx,
                    tenant_id,
                    run.id,
                    node_id,
                    next_attempt,
                    Some(*next_run_at),
                )
                .await?;
            }
        }
        StepMutation::TerminalFailed {
            node_id,
            attempt,
            error_code,
            error_message,
        } => {
            upsert_failed_step_tx(
                tx,
                tenant_id,
                run.id,
                node_id,
                *attempt,
                error_code,
                error_message,
            )
            .await?;
        }
        StepMutation::Suspended { node_id, attempt } => {
            sqlx::query(
                "INSERT INTO ax_flow_step_logs (
                     tenant_id, run_id, node_id, attempt, status, started_at
                 )
                 VALUES ($1, $2, $3, $4, 'WAITING_APPROVAL', NOW())
                 ON CONFLICT (run_id, node_id, attempt) DO UPDATE SET
                     status = 'WAITING_APPROVAL',
                     started_at = COALESCE(ax_flow_step_logs.started_at, NOW()),
                     finished_at = NULL",
            )
            .bind(tenant_id)
            .bind(run.id)
            .bind(node_id)
            .bind(*attempt)
            .execute(&mut **tx)
            .await
            .map_err(|err| AppError::Database(format!("mark step WAITING_APPROVAL: {err}")))?;
        }
    }

    Ok(())
}

async fn apply_progression_plan_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    run: &RunRecord,
    plan: &RunProgressionPlan,
) -> Result<RunRecord, AppError> {
    let run_id_str = run.id.to_string();

    for skip_node in &plan.nodes_to_skip {
        let updated_pending = sqlx::query(
            "UPDATE ax_flow_step_logs
                SET status = 'SKIPPED',
                    finished_at = COALESCE(finished_at, NOW())
              WHERE run_id = $1
                AND node_id = $2
                AND status = 'PENDING'",
        )
        .bind(run.id)
        .bind(skip_node)
        .execute(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("skip pending step failed: {err}")))?;

        if updated_pending.rows_affected() > 0 {
            sqlx::query(
                "UPDATE ax_jobs
                    SET status = 'CANCELED',
                        updated_at = NOW()
                  WHERE kind = 'FLOW_STEP'
                    AND status = 'PENDING'
                    AND payload->>'run_id' = $1
                    AND payload->>'node_id' = $2",
            )
            .bind(&run_id_str)
            .bind(skip_node)
            .execute(&mut **tx)
            .await
            .map_err(|err| AppError::Database(format!("cancel skipped node job failed: {err}")))?;
        } else {
            sqlx::query(
                "INSERT INTO ax_flow_step_logs (
                     tenant_id, run_id, node_id, attempt, status, finished_at
                 )
                 SELECT $1, $2, $3, 1, 'SKIPPED', NOW()
                  WHERE NOT EXISTS (
                      SELECT 1 FROM ax_flow_step_logs WHERE run_id = $2 AND node_id = $3
                  )
                 ON CONFLICT (run_id, node_id, attempt) DO NOTHING",
            )
            .bind(tenant_id)
            .bind(run.id)
            .bind(skip_node)
            .execute(&mut **tx)
            .await
            .map_err(|err| AppError::Database(format!("insert skipped step failed: {err}")))?;
        }
    }

    for (next_node, next_attempt) in &plan.steps_to_enqueue {
        insert_pending_step_and_enqueue_tx(tx, tenant_id, run.id, next_node, *next_attempt, None)
            .await?;
    }

    let Some(next_status) = plan.next_run_status else {
        return Ok(run.clone());
    };

    let is_terminal = matches!(
        next_status,
        RunStatus::Succeeded | RunStatus::Failed | RunStatus::Canceled
    );
    let updated_row = sqlx::query(
        "UPDATE ax_flow_runs
            SET status = $2,
                outputs = COALESCE($3, outputs),
                error_message = CASE
                    WHEN $4::text IS NOT NULL THEN $4
                    WHEN $2 = 'SUCCEEDED' THEN NULL
                    ELSE error_message
                END,
                finished_at = CASE
                    WHEN $5 THEN COALESCE(finished_at, NOW())
                    ELSE NULL
                END
          WHERE id = $1
        RETURNING id, flow_id, version, environment, trigger_source,
                  trigger_payload, idempotency_key, status, outputs, error_message,
                  started_by, created_at, started_at, finished_at",
    )
    .bind(run.id)
    .bind(next_status.as_str())
    .bind(&plan.run_outputs)
    .bind(&plan.run_error_message)
    .bind(is_terminal)
    .fetch_one(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("update ax_flow_runs progression failed: {err}")))?;

    decode_run_record(&updated_row)
}

async fn cleanup_orphan_credential_tx(
    tx: &mut Transaction<'_, Postgres>,
    credential_id: Uuid,
) -> Result<(), AppError> {
    sqlx::query(
        "DELETE FROM ds_credentials c
          WHERE c.id = $1
            AND NOT EXISTS (SELECT 1 FROM ax_flow_hooks h WHERE h.credential_id = c.id)
            AND NOT EXISTS (SELECT 1 FROM ds_connections cn WHERE cn.credential_id = c.id)
            AND NOT EXISTS (SELECT 1 FROM ds_webhooks w WHERE w.credential_id = c.id)",
    )
    .bind(credential_id)
    .execute(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("cleanup orphan ds_credentials failed: {err}")))?;
    Ok(())
}

struct MergedFlowUpdate {
    name: String,
    description: String,
    slug: Option<String>,
    nodes: serde_json::Value,
    edges: serde_json::Value,
    trigger_cfg: serde_json::Value,
}

fn merge_flow_update(
    existing: &sqlx::postgres::PgRow,
    req: UpdateAxFlowRequest,
) -> Result<MergedFlowUpdate, AppError> {
    let name = req
        .name
        .as_deref()
        .map(str::trim)
        .map_or_else(|| existing.get("name"), String::from);
    let description = req
        .description
        .as_deref()
        .map(str::trim)
        .map_or_else(|| existing.get("description"), String::from);
    let slug: Option<String> = match req.slug {
        None => existing.get("slug"),
        Some(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(String::from(trimmed))
            }
        }
    };
    let nodes = match req.nodes {
        Some(items) => serde_json::to_value(&items)
            .map_err(|err| AppError::bad("INVALID_ARGUMENT", format!("invalid nodes: {err}")))?,
        None => existing.get("nodes"),
    };
    let edges = match req.edges {
        Some(items) => serde_json::to_value(&items)
            .map_err(|err| AppError::bad("INVALID_ARGUMENT", format!("invalid edges: {err}")))?,
        None => existing.get("edges"),
    };
    let trigger_cfg = match req.trigger_cfg {
        Some(tc) => serde_json::to_value(&tc).map_err(|err| {
            AppError::bad("INVALID_ARGUMENT", format!("invalid trigger_cfg: {err}"))
        })?,
        None => existing.get("trigger_cfg"),
    };

    Ok(MergedFlowUpdate {
        name,
        description,
        slug,
        nodes,
        edges,
        trigger_cfg,
    })
}

async fn find_idempotent_run_tx(
    tx: &mut Transaction<'_, Postgres>,
    flow_id: Uuid,
    idem_key: Option<&str>,
) -> Result<Option<RunRecord>, AppError> {
    let Some(key) = idem_key else {
        return Ok(None);
    };
    let maybe_row = sqlx::query(
        "SELECT id, flow_id, version, environment, trigger_source,
                trigger_payload, idempotency_key, status, outputs, error_message,
                started_by, created_at, started_at, finished_at
           FROM ax_flow_runs
          WHERE flow_id = $1 AND idempotency_key = $2",
    )
    .bind(flow_id)
    .bind(key)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("check idempotent ax_flow_runs failed: {err}")))?;

    maybe_row.as_ref().map(decode_run_record).transpose()
}

impl FlowRepository for PgStore {
    async fn list_flows(
        &self,
        tenant_id: &str,
        query: FlowListQuery,
    ) -> Result<PaginatedList<AxFlowDef>, AppError> {
        let (limit_usize, fetch_limit) = validate_page_limit(query.limit)?;
        let (cursor_ts, cursor_id) = match query.cursor.as_deref().filter(|s| !s.trim().is_empty())
        {
            Some(c) => {
                let (ts, id) = parse_ts_uuid_cursor(c.trim())?;
                (Some(ts), Some(id))
            }
            None => (None, None),
        };
        let like_pattern = query
            .q
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|q| format!("%{q}%"));

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let rows = sqlx::query(
            "SELECT f.id, f.name, f.description, f.slug, f.nodes, f.edges, f.trigger_cfg,
                    f.status, f.created_by,
                    GREATEST(
                        f.last_run_at,
                        (SELECT max(r.created_at) FROM ax_flow_runs r WHERE r.flow_id = f.id)
                    ) AS last_run_at,
                    f.created_at, f.updated_at
               FROM ax_flows f
              WHERE ($1::text IS NULL OR f.name ILIKE $1 OR f.description ILIKE $1 OR COALESCE(f.slug, '') ILIKE $1)
                AND ($2::timestamptz IS NULL OR (f.created_at, f.id) < ($2, $3::uuid))
              ORDER BY f.created_at DESC, f.id DESC
              LIMIT $4",
        )
        .bind(like_pattern)
        .bind(cursor_ts)
        .bind(cursor_id)
        .bind(fetch_limit)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("list ax_flows failed: {err}")))?;

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit list_flows tx failed: {err}")))?;

        let mut items: Vec<AxFlowDef> =
            rows.iter().map(decode_flow_def).collect::<Result<_, _>>()?;
        let next_cursor = if items.len() > limit_usize {
            items.truncate(limit_usize);
            items
                .last()
                .map(|f| format!("{}|{}", f.created_at.to_rfc3339(), f.id))
        } else {
            None
        };

        Ok(PaginatedList { items, next_cursor })
    }

    async fn get_flow(
        &self,
        tenant_id: &str,
        flow_id: Uuid,
    ) -> Result<Option<AxFlowDef>, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let maybe_row = sqlx::query(
            "SELECT f.id, f.name, f.description, f.slug, f.nodes, f.edges, f.trigger_cfg,
                    f.status, f.created_by,
                    GREATEST(
                        f.last_run_at,
                        (SELECT max(r.created_at) FROM ax_flow_runs r WHERE r.flow_id = f.id)
                    ) AS last_run_at,
                    f.created_at, f.updated_at
               FROM ax_flows f
              WHERE f.id = $1",
        )
        .bind(flow_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("get ax_flow failed: {err}")))?;

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit get_flow tx failed: {err}")))?;

        maybe_row.as_ref().map(decode_flow_def).transpose()
    }

    async fn create_flow(
        &self,
        tenant_id: &str,
        created_by: Option<Uuid>,
        req: CreateAxFlowRequest,
    ) -> Result<AxFlowDef, AppError> {
        let trimmed_name = req.name.trim();
        if trimmed_name.is_empty() {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "flow name must not be empty",
            ));
        }

        let (nodes, edges) = req.resolve_dag();
        let nodes_json = serde_json::to_value(&nodes)
            .map_err(|err| AppError::bad("INVALID_ARGUMENT", format!("invalid nodes: {err}")))?;
        let edges_json = serde_json::to_value(&edges)
            .map_err(|err| AppError::bad("INVALID_ARGUMENT", format!("invalid edges: {err}")))?;
        let trigger_json = serde_json::to_value(&req.trigger_cfg).map_err(|err| {
            AppError::bad("INVALID_ARGUMENT", format!("invalid trigger_cfg: {err}"))
        })?;
        let slug = req
            .slug
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from);

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let insert_res = sqlx::query(
            "INSERT INTO ax_flows (
                 tenant_id, name, description, slug, nodes, edges, trigger_cfg, status, created_by
             )
             VALUES ($1, $2, $3, $4, $5, $6, $7, 'DRAFT', $8)
             RETURNING id, name, description, slug, nodes, edges, trigger_cfg,
                       status, created_by, last_run_at, created_at, updated_at",
        )
        .bind(tenant_id)
        .bind(trimmed_name)
        .bind(req.description.trim())
        .bind(slug)
        .bind(nodes_json)
        .bind(edges_json)
        .bind(trigger_json)
        .bind(created_by)
        .fetch_one(&mut *tx)
        .await;

        let row = match insert_res {
            Ok(row) => row,
            Err(err) if is_unique_violation(&err) => {
                return Err(AppError::Conflict("CONFLICT"));
            }
            Err(err) => {
                return Err(AppError::Database(format!("insert ax_flows failed: {err}")));
            }
        };

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit create_flow tx failed: {err}")))?;

        decode_flow_def(&row)
    }

    async fn update_flow(
        &self,
        tenant_id: &str,
        flow_id: Uuid,
        req: UpdateAxFlowRequest,
    ) -> Result<Option<AxFlowDef>, AppError> {
        if let Some(ref name) = req.name
            && name.trim().is_empty()
        {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "flow name must not be empty",
            ));
        }

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let existing_opt = sqlx::query(
            "SELECT name, description, slug, nodes, edges, trigger_cfg
               FROM ax_flows
              WHERE id = $1
              FOR UPDATE",
        )
        .bind(flow_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("select ax_flows FOR UPDATE failed: {err}")))?;

        let Some(existing) = existing_opt else {
            tx.commit().await.map_err(|err| {
                AppError::Database(format!("commit empty update_flow tx failed: {err}"))
            })?;
            return Ok(None);
        };

        let merged = merge_flow_update(&existing, req)?;
        let update_res = sqlx::query(
            "UPDATE ax_flows f
                SET name = $2,
                    description = $3,
                    slug = $4,
                    nodes = $5,
                    edges = $6,
                    trigger_cfg = $7,
                    updated_at = NOW()
              WHERE f.id = $1
          RETURNING f.id, f.name, f.description, f.slug, f.nodes, f.edges, f.trigger_cfg,
                    f.status, f.created_by,
                    GREATEST(
                        f.last_run_at,
                        (SELECT max(r.created_at) FROM ax_flow_runs r WHERE r.flow_id = f.id)
                    ) AS last_run_at,
                    f.created_at, f.updated_at",
        )
        .bind(flow_id)
        .bind(merged.name)
        .bind(merged.description)
        .bind(merged.slug)
        .bind(merged.nodes)
        .bind(merged.edges)
        .bind(merged.trigger_cfg)
        .fetch_one(&mut *tx)
        .await;

        let row = match update_res {
            Ok(row) => row,
            Err(err) if is_unique_violation(&err) => {
                return Err(AppError::Conflict("CONFLICT"));
            }
            Err(err) => {
                return Err(AppError::Database(format!("update ax_flows failed: {err}")));
            }
        };

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit update_flow tx failed: {err}")))?;

        Ok(Some(decode_flow_def(&row)?))
    }

    async fn delete_flow(&self, tenant_id: &str, flow_id: Uuid) -> Result<bool, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;

        let hook_cred_id: Option<Uuid> =
            sqlx::query_scalar("SELECT credential_id FROM ax_flow_hooks WHERE flow_id = $1")
                .bind(flow_id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(|err| {
                    AppError::Database(format!("query ax_flow_hooks credential failed: {err}"))
                })?;

        sqlx::query(
            "UPDATE ax_jobs
                SET status = 'CANCELED',
                    updated_at = NOW()
              WHERE kind = 'FLOW_STEP'
                AND status = 'PENDING'
                AND payload->>'run_id' IN (
                    SELECT id::text FROM ax_flow_runs WHERE flow_id = $1
                )",
        )
        .bind(flow_id)
        .execute(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("cancel flow jobs on delete failed: {err}")))?;

        let res = sqlx::query("DELETE FROM ax_flows WHERE id = $1")
            .bind(flow_id)
            .execute(&mut *tx)
            .await
            .map_err(|err| AppError::Database(format!("delete ax_flows failed: {err}")))?;

        if let Some(cid) = hook_cred_id {
            cleanup_orphan_credential_tx(&mut tx, cid).await?;
        }

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit delete_flow tx failed: {err}")))?;

        Ok(res.rows_affected() > 0)
    }

    async fn publish_flow_version(
        &self,
        tenant_id: &str,
        flow_id: Uuid,
        published_by: Option<Uuid>,
        req: PublishFlowVersionRequest,
    ) -> Result<FlowVersionRecord, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;

        let flow_row =
            sqlx::query("SELECT nodes, edges, trigger_cfg FROM ax_flows WHERE id = $1 FOR UPDATE")
                .bind(flow_id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(|err| {
                    AppError::Database(format!("lock ax_flows for publish failed: {err}"))
                })?
                .ok_or(AppError::NotFound("FLOW_NOT_FOUND"))?;

        let nodes: serde_json::Value = flow_row.get("nodes");
        let edges: serde_json::Value = flow_row.get("edges");
        let trigger_cfg: serde_json::Value = flow_row.get("trigger_cfg");

        let next_version: i32 = sqlx::query_scalar(
            "SELECT COALESCE((SELECT MAX(version) FROM ax_flow_versions WHERE flow_id = $1), 0) + 1",
        )
        .bind(flow_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("compute next flow version failed: {err}")))?;

        let ver_row = sqlx::query(
            "INSERT INTO ax_flow_versions (
                 tenant_id, flow_id, version, nodes, edges, trigger_cfg, note, published_by
             )
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
             RETURNING id, flow_id, version, nodes, edges, trigger_cfg, note, published_by, published_at",
        )
        .bind(tenant_id)
        .bind(flow_id)
        .bind(next_version)
        .bind(nodes)
        .bind(edges)
        .bind(trigger_cfg)
        .bind(req.note.trim())
        .bind(published_by)
        .fetch_one(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("insert ax_flow_versions failed: {err}")))?;

        sqlx::query("UPDATE ax_flows SET status = 'PUBLISHED', updated_at = NOW() WHERE id = $1")
            .bind(flow_id)
            .execute(&mut *tx)
            .await
            .map_err(|err| AppError::Database(format!("mark ax_flows PUBLISHED failed: {err}")))?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit publish_flow_version tx failed: {err}"))
        })?;

        decode_flow_version(&ver_row)
    }

    async fn list_flow_versions(
        &self,
        tenant_id: &str,
        flow_id: Uuid,
    ) -> Result<FlowVersionsListResponse, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        ensure_flow_exists_tx(&mut tx, flow_id).await?;

        let ver_rows = sqlx::query(
            "SELECT id, flow_id, version, nodes, edges, trigger_cfg, note, published_by, published_at
               FROM ax_flow_versions
              WHERE flow_id = $1
              ORDER BY version DESC",
        )
        .bind(flow_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("list ax_flow_versions failed: {err}")))?;

        let dep_rows = sqlx::query(
            "SELECT flow_id, environment, version, deployed_by, deployed_at
               FROM ax_flow_deployments
              WHERE flow_id = $1
              ORDER BY environment ASC",
        )
        .bind(flow_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("list ax_flow_deployments failed: {err}")))?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit list_flow_versions tx failed: {err}"))
        })?;

        let items = ver_rows
            .iter()
            .map(decode_flow_version)
            .collect::<Result<Vec<_>, _>>()?;
        let deployments = dep_rows
            .iter()
            .map(decode_flow_deployment)
            .collect::<Result<Vec<_>, _>>()?;

        Ok(FlowVersionsListResponse {
            items,
            deployments,
            next_cursor: None,
        })
    }

    async fn get_flow_version(
        &self,
        tenant_id: &str,
        flow_id: Uuid,
        version: i32,
    ) -> Result<Option<FlowVersionRecord>, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let ver = fetch_flow_version_tx(&mut tx, flow_id, version).await?;
        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit get_flow_version tx failed: {err}"))
        })?;
        Ok(ver)
    }

    async fn deploy_flow(
        &self,
        tenant_id: &str,
        flow_id: Uuid,
        deployed_by: Option<Uuid>,
        req: DeployFlowRequest,
    ) -> Result<FlowDeploymentRecord, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;

        let ver_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM ax_flow_versions WHERE flow_id = $1 AND version = $2)",
        )
        .bind(flow_id)
        .bind(req.version)
        .fetch_one(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("check flow version exists failed: {err}")))?;

        if !ver_exists {
            return Err(AppError::NotFound("NOT_FOUND"));
        }

        let dep_row = sqlx::query(
            "INSERT INTO ax_flow_deployments (
                 tenant_id, flow_id, environment, version, deployed_by, deployed_at
             )
             VALUES ($1, $2, $3, $4, $5, NOW())
             ON CONFLICT (flow_id, environment) DO UPDATE SET
                 version = EXCLUDED.version,
                 deployed_by = EXCLUDED.deployed_by,
                 deployed_at = NOW()
             RETURNING flow_id, environment, version, deployed_by, deployed_at",
        )
        .bind(tenant_id)
        .bind(flow_id)
        .bind(req.environment.as_str())
        .bind(req.version)
        .bind(deployed_by)
        .fetch_one(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("upsert ax_flow_deployments failed: {err}")))?;

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit deploy_flow tx failed: {err}")))?;

        decode_flow_deployment(&dep_row)
    }

    async fn get_flow_deployment(
        &self,
        tenant_id: &str,
        flow_id: Uuid,
        environment: Environment,
    ) -> Result<Option<FlowDeploymentRecord>, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let maybe_row = sqlx::query(
            "SELECT flow_id, environment, version, deployed_by, deployed_at
               FROM ax_flow_deployments
              WHERE flow_id = $1 AND environment = $2",
        )
        .bind(flow_id)
        .bind(environment.as_str())
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("get ax_flow_deployments failed: {err}")))?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit get_flow_deployment tx failed: {err}"))
        })?;

        maybe_row.as_ref().map(decode_flow_deployment).transpose()
    }

    async fn start_flow_run(
        &self,
        tenant_id: &str,
        params: StartRunParams,
    ) -> Result<(RunRecord, bool), AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;

        let locked_flow: Option<Uuid> =
            sqlx::query_scalar("SELECT id FROM ax_flows WHERE id = $1 FOR UPDATE")
                .bind(params.flow_id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(|err| {
                    AppError::Database(format!("lock ax_flows for run start failed: {err}"))
                })?;
        if locked_flow.is_none() {
            return Err(AppError::NotFound("FLOW_NOT_FOUND"));
        }

        let idem_key = params
            .idempotency_key
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from);

        if let Some(existing_run) =
            find_idempotent_run_tx(&mut tx, params.flow_id, idem_key.as_deref()).await?
        {
            tx.commit().await.map_err(|err| {
                AppError::Database(format!("commit existing start_flow_run tx failed: {err}"))
            })?;
            return Ok((existing_run, true));
        }

        let ver = resolve_run_version_tx(
            &mut tx,
            params.flow_id,
            params.environment,
            params.explicit_version,
        )
        .await?;

        let targets: BTreeSet<&str> = ver.edges.iter().map(|e| e.to.as_str()).collect();
        let root_ids: Vec<&str> = ver
            .nodes
            .iter()
            .filter(|n| !targets.contains(n.id.as_str()))
            .map(|n| n.id.as_str())
            .collect();
        let initial_status = if root_ids.is_empty() {
            RunStatus::Succeeded
        } else {
            RunStatus::Running
        };

        let run_row = sqlx::query(
            "INSERT INTO ax_flow_runs (
                 tenant_id, flow_id, version, environment, trigger_source,
                 trigger_payload, idempotency_key, status, outputs,
                 started_by, started_at, finished_at
             )
             VALUES (
                 $1, $2, $3, $4, $5, $6, $7, $8, '{}'::jsonb,
                 $9, NOW(), CASE WHEN $8 = 'SUCCEEDED' THEN NOW() ELSE NULL END
             )
             RETURNING id, flow_id, version, environment, trigger_source,
                       trigger_payload, idempotency_key, status, outputs, error_message,
                       started_by, created_at, started_at, finished_at",
        )
        .bind(tenant_id)
        .bind(params.flow_id)
        .bind(ver.version)
        .bind(params.environment.as_str())
        .bind(params.trigger_source.as_str())
        .bind(&params.trigger_payload)
        .bind(idem_key)
        .bind(initial_status.as_str())
        .bind(params.started_by)
        .fetch_one(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("insert ax_flow_runs failed: {err}")))?;

        let run = decode_run_record(&run_row)?;
        sqlx::query("UPDATE ax_flows SET last_run_at = NOW() WHERE id = $1")
            .bind(params.flow_id)
            .execute(&mut *tx)
            .await
            .map_err(|err| AppError::Database(format!("update ax_flows.last_run_at: {err}")))?;

        for root_node_id in root_ids {
            insert_pending_step_and_enqueue_tx(&mut tx, tenant_id, run.id, root_node_id, 1, None)
                .await?;
        }

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit start_flow_run tx failed: {err}")))?;

        Ok((run, false))
    }

    async fn get_flow_run(
        &self,
        tenant_id: &str,
        run_id: Uuid,
    ) -> Result<Option<RunRecord>, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let maybe_row = sqlx::query(
            "SELECT id, flow_id, version, environment, trigger_source,
                    trigger_payload, idempotency_key, status, outputs, error_message,
                    started_by, created_at, started_at, finished_at
               FROM ax_flow_runs
              WHERE id = $1",
        )
        .bind(run_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("get ax_flow_runs failed: {err}")))?;

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit get_flow_run tx failed: {err}")))?;

        maybe_row.as_ref().map(decode_run_record).transpose()
    }

    async fn list_flow_runs(
        &self,
        tenant_id: &str,
        flow_id: Uuid,
        query: RunListQuery,
    ) -> Result<PaginatedList<RunRecord>, AppError> {
        let (limit_usize, fetch_limit) = validate_page_limit(query.limit)?;
        let (cursor_ts, cursor_id) = match query.cursor.as_deref().filter(|s| !s.trim().is_empty())
        {
            Some(c) => {
                let (ts, id) = parse_ts_uuid_cursor(c.trim())?;
                (Some(ts), Some(id))
            }
            None => (None, None),
        };
        let env_filter = query.environment.map(Environment::as_str);
        let status_filter = query.status.map(RunStatus::as_str);

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        ensure_flow_exists_tx(&mut tx, flow_id).await?;

        let rows = sqlx::query(
            "SELECT id, flow_id, version, environment, trigger_source,
                    trigger_payload, idempotency_key, status, outputs, error_message,
                    started_by, created_at, started_at, finished_at
               FROM ax_flow_runs
              WHERE flow_id = $1
                AND ($2::text IS NULL OR environment = $2)
                AND ($3::text IS NULL OR status = $3)
                AND ($4::timestamptz IS NULL OR (created_at, id) < ($4, $5::uuid))
              ORDER BY created_at DESC, id DESC
              LIMIT $6",
        )
        .bind(flow_id)
        .bind(env_filter)
        .bind(status_filter)
        .bind(cursor_ts)
        .bind(cursor_id)
        .bind(fetch_limit)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("list ax_flow_runs failed: {err}")))?;

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit list_flow_runs tx failed: {err}")))?;

        let mut items: Vec<RunRecord> = rows
            .iter()
            .map(decode_run_record)
            .collect::<Result<_, _>>()?;
        let next_cursor = if items.len() > limit_usize {
            items.truncate(limit_usize);
            items
                .last()
                .map(|r| format!("{}|{}", r.created_at.to_rfc3339(), r.id))
        } else {
            None
        };

        Ok(PaginatedList { items, next_cursor })
    }

    async fn list_run_steps(
        &self,
        tenant_id: &str,
        run_id: Uuid,
    ) -> Result<Vec<StepRecord>, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        ensure_run_exists_tx(&mut tx, run_id).await?;

        let rows = sqlx::query(
            "SELECT id, run_id, node_id, attempt, status, job_id, input,
                    output, output_ref, error_code, error_message, started_at, finished_at
               FROM ax_flow_step_logs
              WHERE run_id = $1
              ORDER BY id ASC",
        )
        .bind(run_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("list ax_flow_step_logs failed: {err}")))?;

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit list_run_steps tx failed: {err}")))?;

        rows.iter().map(decode_step_record).collect()
    }

    async fn list_latest_run_steps(
        &self,
        tenant_id: &str,
        run_id: Uuid,
    ) -> Result<Vec<StepRecord>, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        ensure_run_exists_tx(&mut tx, run_id).await?;
        let steps = fetch_latest_run_steps_tx(&mut tx, run_id).await?;
        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit list_latest_run_steps tx failed: {err}"))
        })?;
        Ok(steps)
    }

    async fn mark_step_running(
        &self,
        tenant_id: &str,
        run_id: Uuid,
        node_id: &str,
        attempt: i32,
        job_id: Option<Uuid>,
        rendered_input: &serde_json::Value,
    ) -> Result<StepRecord, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        ensure_run_exists_tx(&mut tx, run_id).await?;

        let row = sqlx::query(
            "INSERT INTO ax_flow_step_logs (
                 tenant_id, run_id, node_id, attempt, status, job_id, input, started_at
             )
             VALUES ($1, $2, $3, $4, 'RUNNING', $5, $6, NOW())
             ON CONFLICT (run_id, node_id, attempt) DO UPDATE SET
                 status = 'RUNNING',
                 job_id = COALESCE($5, ax_flow_step_logs.job_id),
                 input = $6,
                 started_at = COALESCE(ax_flow_step_logs.started_at, NOW()),
                 finished_at = NULL,
                 error_code = NULL,
                 error_message = NULL
             RETURNING id, run_id, node_id, attempt, status, job_id, input,
                       output, output_ref, error_code, error_message, started_at, finished_at",
        )
        .bind(tenant_id)
        .bind(run_id)
        .bind(node_id)
        .bind(attempt)
        .bind(job_id)
        .bind(rendered_input)
        .fetch_one(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("mark_step_running failed: {err}")))?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit mark_step_running tx failed: {err}"))
        })?;

        decode_step_record(&row)
    }

    async fn find_cached_step_output(
        &self,
        tenant_id: &str,
        run_id: Uuid,
        node_id: &str,
        rendered_input: &serde_json::Value,
    ) -> Result<Option<serde_json::Value>, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let cached: Option<serde_json::Value> = sqlx::query_scalar(
            "SELECT output
               FROM ax_flow_step_logs
              WHERE run_id = $1
                AND node_id = $2
                AND input = $3
                AND output IS NOT NULL
              ORDER BY attempt DESC
              LIMIT 1",
        )
        .bind(run_id)
        .bind(node_id)
        .bind(rendered_input)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("find_cached_step_output failed: {err}")))?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit find_cached_step_output tx failed: {err}"))
        })?;

        Ok(cached)
    }

    async fn apply_step_mutation_and_progress(
        &self,
        tenant_id: &str,
        run_id: Uuid,
        mutation: StepMutation,
        planner: ProgressionPlannerFn<'_>,
    ) -> Result<RunRecord, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let run = lock_run_for_update_tx(&mut tx, run_id).await?;

        apply_step_mutation_tx(&mut tx, tenant_id, &run, &mutation).await?;

        if run.status == RunStatus::Canceled {
            tx.commit().await.map_err(|err| {
                AppError::Database(format!(
                    "commit canceled apply_step_mutation tx failed: {err}"
                ))
            })?;
            return Ok(run);
        }

        let ver = fetch_flow_version_tx(&mut tx, run.flow_id, run.version)
            .await?
            .ok_or_else(|| AppError::Database(String::from("pinned ax_flow_versions missing")))?;
        let latest_steps = fetch_latest_run_steps_tx(&mut tx, run.id).await?;
        let plan = planner(&run, &ver, &latest_steps)?;
        let updated_run = apply_progression_plan_tx(&mut tx, tenant_id, &run, &plan).await?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!(
                "commit apply_step_mutation_and_progress tx failed: {err}"
            ))
        })?;

        Ok(updated_run)
    }

    async fn cancel_flow_run(&self, tenant_id: &str, run_id: Uuid) -> Result<RunRecord, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let _locked = lock_run_for_update_tx(&mut tx, run_id).await?;

        let updated_row = sqlx::query(
            "UPDATE ax_flow_runs
                SET status = 'CANCELED',
                    finished_at = COALESCE(finished_at, NOW())
              WHERE id = $1
          RETURNING id, flow_id, version, environment, trigger_source,
                    trigger_payload, idempotency_key, status, outputs, error_message,
                    started_by, created_at, started_at, finished_at",
        )
        .bind(run_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("cancel ax_flow_runs failed: {err}")))?;

        sqlx::query(
            "UPDATE ax_flow_step_logs
                SET status = 'SKIPPED',
                    finished_at = COALESCE(finished_at, NOW())
              WHERE run_id = $1
                AND status IN ('PENDING', 'WAITING_APPROVAL')",
        )
        .bind(run_id)
        .execute(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("skip steps on cancel failed: {err}")))?;

        sqlx::query(
            "UPDATE ax_jobs
                SET status = 'CANCELED',
                    updated_at = NOW()
              WHERE kind = 'FLOW_STEP'
                AND status = 'PENDING'
                AND payload->>'run_id' = $1",
        )
        .bind(run_id.to_string())
        .execute(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("cancel FLOW_STEP jobs failed: {err}")))?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit cancel_flow_run tx failed: {err}"))
        })?;

        decode_run_record(&updated_row)
    }

    async fn retry_flow_run(&self, tenant_id: &str, run_id: Uuid) -> Result<RunRecord, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let run = lock_run_for_update_tx(&mut tx, run_id).await?;

        if run.status != RunStatus::Failed {
            return Err(AppError::Conflict("CONFLICT"));
        }

        sqlx::query("DELETE FROM ax_flow_step_logs WHERE run_id = $1 AND status = 'SKIPPED'")
            .bind(run_id)
            .execute(&mut *tx)
            .await
            .map_err(|err| {
                AppError::Database(format!("delete SKIPPED steps on retry failed: {err}"))
            })?;

        let failed_rows = sqlx::query(
            "SELECT node_id, attempt
               FROM (
                   SELECT DISTINCT ON (node_id) node_id, attempt, status
                     FROM ax_flow_step_logs
                    WHERE run_id = $1
                    ORDER BY node_id ASC, attempt DESC
               ) latest
              WHERE latest.status = 'FAILED'
              ORDER BY latest.node_id ASC",
        )
        .bind(run_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("query latest FAILED steps failed: {err}")))?;

        for row in failed_rows {
            let node_id: String = row.get("node_id");
            let attempt: i32 = row.get("attempt");
            let next_attempt = attempt.saturating_add(1);
            insert_pending_step_and_enqueue_tx(
                &mut tx,
                tenant_id,
                run_id,
                &node_id,
                next_attempt,
                None,
            )
            .await?;
        }

        let updated_row = sqlx::query(
            "UPDATE ax_flow_runs
                SET status = 'RUNNING',
                    finished_at = NULL,
                    error_message = NULL
              WHERE id = $1
          RETURNING id, flow_id, version, environment, trigger_source,
                    trigger_payload, idempotency_key, status, outputs, error_message,
                    started_by, created_at, started_at, finished_at",
        )
        .bind(run_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("reset ax_flow_runs on retry failed: {err}")))?;

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit retry_flow_run tx failed: {err}")))?;

        decode_run_record(&updated_row)
    }

    async fn create_or_get_approval(
        &self,
        tenant_id: &str,
        input: NewApprovalInput,
    ) -> Result<ApprovalRecord, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;

        if let (Some(run_id), Some(node_id)) = (input.run_id, input.node_id.as_deref()) {
            let _locked = lock_run_for_update_tx(&mut tx, run_id).await?;
            let existing_row = sqlx::query(
                "SELECT id, run_id, node_id, source_kind, source_ref, title, detail,
                        assignee_roles, assignee_users, status, decided_by, decided_at,
                        comment, expires_at, created_at
                   FROM ax_approvals
                  WHERE run_id = $1 AND node_id = $2
                  ORDER BY created_at DESC
                  LIMIT 1",
            )
            .bind(run_id)
            .bind(node_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|err| {
                AppError::Database(format!("lookup existing ax_approvals failed: {err}"))
            })?;

            if let Some(row) = existing_row {
                let rec = decode_approval_record(&row)?;
                tx.commit().await.map_err(|err| {
                    AppError::Database(format!(
                        "commit existing create_or_get_approval tx failed: {err}"
                    ))
                })?;
                return Ok(rec);
            }
        }

        let row = sqlx::query(
            "INSERT INTO ax_approvals (
                 tenant_id, run_id, node_id, source_kind, source_ref, title, detail,
                 assignee_roles, assignee_users, status, expires_at
             )
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, 'PENDING', $10)
             RETURNING id, run_id, node_id, source_kind, source_ref, title, detail,
                       assignee_roles, assignee_users, status, decided_by, decided_at,
                       comment, expires_at, created_at",
        )
        .bind(tenant_id)
        .bind(input.run_id)
        .bind(input.node_id)
        .bind(approval_source_kind_as_str(input.source_kind))
        .bind(input.source_ref)
        .bind(input.title)
        .bind(input.detail)
        .bind(&input.assignee_roles)
        .bind(&input.assignee_users)
        .bind(input.expires_at)
        .fetch_one(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("insert ax_approvals failed: {err}")))?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit create_or_get_approval tx failed: {err}"))
        })?;

        decode_approval_record(&row)
    }

    async fn get_approval(
        &self,
        tenant_id: &str,
        approval_id: Uuid,
    ) -> Result<Option<ApprovalRecord>, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let maybe_row = sqlx::query(
            "SELECT id, run_id, node_id, source_kind, source_ref, title, detail,
                    assignee_roles, assignee_users, status, decided_by, decided_at,
                    comment, expires_at, created_at
               FROM ax_approvals
              WHERE id = $1",
        )
        .bind(approval_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("get ax_approvals failed: {err}")))?;

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit get_approval tx failed: {err}")))?;

        maybe_row.as_ref().map(decode_approval_record).transpose()
    }

    async fn list_approvals(
        &self,
        tenant_id: &str,
        viewer_user_id: Uuid,
        viewer_roles: &[String],
        query: ApprovalListQuery,
    ) -> Result<PaginatedList<ApprovalRecord>, AppError> {
        let (limit_usize, fetch_limit) = validate_page_limit(query.limit)?;
        let (cursor_ts, cursor_id) = match query.cursor.as_deref().filter(|s| !s.trim().is_empty())
        {
            Some(c) => {
                let (ts, id) = parse_ts_uuid_cursor(c.trim())?;
                (Some(ts), Some(id))
            }
            None => (None, None),
        };
        let status_filter = query.status.map(approval_status_as_str);
        let mine_only = query.mine.unwrap_or(false);

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let rows = sqlx::query(
            "SELECT id, run_id, node_id, source_kind, source_ref, title, detail,
                    assignee_roles, assignee_users, status, decided_by, decided_at,
                    comment, expires_at, created_at
               FROM ax_approvals
              WHERE ($1::text IS NULL OR status = $1)
                AND ($2::uuid IS NULL OR run_id = $2)
                AND (NOT $3 OR (
                    (cardinality(assignee_roles) = 0 AND cardinality(assignee_users) = 0)
                    OR $4 = ANY(assignee_users)
                    OR assignee_roles && $5::text[]
                ))
                AND ($6::timestamptz IS NULL OR (created_at, id) < ($6, $7::uuid))
              ORDER BY created_at DESC, id DESC
              LIMIT $8",
        )
        .bind(status_filter)
        .bind(query.run_id)
        .bind(mine_only)
        .bind(viewer_user_id)
        .bind(viewer_roles)
        .bind(cursor_ts)
        .bind(cursor_id)
        .bind(fetch_limit)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("list ax_approvals failed: {err}")))?;

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit list_approvals tx failed: {err}")))?;

        let mut items: Vec<ApprovalRecord> = rows
            .iter()
            .map(decode_approval_record)
            .collect::<Result<_, _>>()?;
        let next_cursor = if items.len() > limit_usize {
            items.truncate(limit_usize);
            items
                .last()
                .map(|a| format!("{}|{}", a.created_at.to_rfc3339(), a.id))
        } else {
            None
        };

        Ok(PaginatedList { items, next_cursor })
    }

    async fn decide_approval(
        &self,
        tenant_id: &str,
        params: DecideApprovalParams,
    ) -> Result<ApprovalRecord, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;

        let locked_row = sqlx::query(
            "SELECT id, run_id, node_id, source_kind, source_ref, title, detail,
                    assignee_roles, assignee_users, status, decided_by, decided_at,
                    comment, expires_at, created_at
               FROM ax_approvals
              WHERE id = $1
              FOR UPDATE",
        )
        .bind(params.approval_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("lock ax_approvals FOR UPDATE failed: {err}")))?
        .ok_or(AppError::NotFound("NOT_FOUND"))?;

        let existing = decode_approval_record(&locked_row)?;
        if existing.status != ApprovalStatus::Pending {
            return Err(AppError::Conflict("CONFLICT"));
        }

        if !existing.assignee_roles.is_empty() || !existing.assignee_users.is_empty() {
            let user_ok = existing.assignee_users.contains(&params.actor_user_id);
            let role_ok = params
                .actor_roles
                .iter()
                .any(|r| existing.assignee_roles.contains(r));
            if !user_ok && !role_ok {
                return Err(AppError::Forbidden("PERMISSION_DENIED"));
            }
        }

        let next_status = match params.decision {
            ApprovalDecision::Approve => ApprovalStatus::Approved,
            ApprovalDecision::Reject => ApprovalStatus::Rejected,
        };

        let updated_row = sqlx::query(
            "UPDATE ax_approvals
                SET status = $2,
                    decided_by = $3,
                    decided_at = NOW(),
                    comment = $4
              WHERE id = $1
          RETURNING id, run_id, node_id, source_kind, source_ref, title, detail,
                    assignee_roles, assignee_users, status, decided_by, decided_at,
                    comment, expires_at, created_at",
        )
        .bind(params.approval_id)
        .bind(approval_status_as_str(next_status))
        .bind(params.actor_user_id)
        .bind(&params.comment)
        .fetch_one(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("update ax_approvals decision failed: {err}")))?;

        let updated = decode_approval_record(&updated_row)?;
        let outbox_payload = serde_json::json!({
            "approval_id": updated.id,
            "run_id": updated.run_id,
            "node_id": updated.node_id,
            "source_kind": updated.source_kind,
            "source_ref": updated.source_ref,
            "status": updated.status,
            "decided_by": updated.decided_by,
            "comment": updated.comment,
        });

        insert_outbox_tx(
            &mut tx,
            tenant_id,
            &NewOutboxEvent {
                topic: OutboxTopic::ApprovalDecided,
                payload: outbox_payload.clone(),
            },
        )
        .await?;

        sqlx::query(
            "INSERT INTO sec_audit_logs (
                 tenant_id, user_id, username, action, resource_ref, status_code, detail_json
             )
             VALUES (
                 $1,
                 $2,
                 COALESCE((SELECT username FROM sec_users WHERE id = $2), $2::text),
                 $3,
                 $4,
                 200,
                 $5
             )",
        )
        .bind(tenant_id)
        .bind(params.actor_user_id)
        .bind(AuditAction::ApprovalDecide.as_str())
        .bind(format!("approval:{}", updated.id))
        .bind(outbox_payload)
        .execute(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("insert APPROVAL_DECIDE audit failed: {err}")))?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit decide_approval tx failed: {err}"))
        })?;

        Ok(updated)
    }

    async fn expire_overdue_approvals(&self, tenant_id: &str) -> Result<u64, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;

        let expired_rows = sqlx::query(
            "UPDATE ax_approvals
                SET status = 'EXPIRED',
                    decided_at = NOW()
              WHERE status = 'PENDING'
                AND expires_at IS NOT NULL
                AND expires_at <= NOW()
          RETURNING id, run_id, node_id, source_kind, source_ref, title, detail,
                    assignee_roles, assignee_users, status, decided_by, decided_at,
                    comment, expires_at, created_at",
        )
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("expire overdue ax_approvals failed: {err}")))?;

        let count = u64::try_from(expired_rows.len()).unwrap_or(u64::MAX);
        for row in expired_rows {
            let expired = decode_approval_record(&row)?;
            let outbox_payload = serde_json::json!({
                "approval_id": expired.id,
                "run_id": expired.run_id,
                "node_id": expired.node_id,
                "source_kind": expired.source_kind,
                "source_ref": expired.source_ref,
                "status": ApprovalStatus::Expired,
                "decided_by": expired.decided_by,
                "comment": expired.comment,
            });
            insert_outbox_tx(
                &mut tx,
                tenant_id,
                &NewOutboxEvent {
                    topic: OutboxTopic::ApprovalDecided,
                    payload: outbox_payload,
                },
            )
            .await?;
        }

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit expire_overdue_approvals tx failed: {err}"))
        })?;

        Ok(count)
    }

    async fn record_action_log(
        &self,
        tenant_id: &str,
        input: NewActionLogInput,
    ) -> Result<(ActionLogRecord, bool), AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;

        let inserted_opt = sqlx::query(
            "INSERT INTO ax_action_log (
                 tenant_id, run_id, node_id, source_kind, action_kind,
                 target, payload, status, idempotency_key, error_message
             )
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
             ON CONFLICT (tenant_id, idempotency_key) DO NOTHING
             RETURNING id, run_id, node_id, source_kind, action_kind,
                       target, payload, status, idempotency_key, error_message, created_at",
        )
        .bind(tenant_id)
        .bind(input.run_id)
        .bind(&input.node_id)
        .bind(action_source_kind_as_str(input.source_kind))
        .bind(action_kind_as_str(input.action_kind))
        .bind(&input.target)
        .bind(&input.payload)
        .bind(action_log_status_as_str(input.status))
        .bind(&input.idempotency_key)
        .bind(&input.error_message)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("insert ax_action_log failed: {err}")))?;

        if let Some(row) = inserted_opt {
            let rec = decode_action_log_record(&row)?;
            tx.commit().await.map_err(|err| {
                AppError::Database(format!("commit record_action_log tx failed: {err}"))
            })?;
            return Ok((rec, false));
        }

        let existing_row = sqlx::query(
            "SELECT id, run_id, node_id, source_kind, action_kind,
                    target, payload, status, idempotency_key, error_message, created_at
               FROM ax_action_log
              WHERE idempotency_key = $1",
        )
        .bind(&input.idempotency_key)
        .fetch_one(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("fetch existing ax_action_log failed: {err}")))?;

        let rec = decode_action_log_record(&existing_row)?;
        tx.commit().await.map_err(|err| {
            AppError::Database(format!(
                "commit existing record_action_log tx failed: {err}"
            ))
        })?;
        Ok((rec, true))
    }

    async fn get_action_log_by_key(
        &self,
        tenant_id: &str,
        idempotency_key: &str,
    ) -> Result<Option<ActionLogRecord>, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let maybe_row = sqlx::query(
            "SELECT id, run_id, node_id, source_kind, action_kind,
                    target, payload, status, idempotency_key, error_message, created_at
               FROM ax_action_log
              WHERE idempotency_key = $1",
        )
        .bind(idempotency_key)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("get ax_action_log by key failed: {err}")))?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit get_action_log_by_key tx failed: {err}"))
        })?;

        maybe_row.as_ref().map(decode_action_log_record).transpose()
    }

    async fn list_action_logs(
        &self,
        tenant_id: &str,
        query: ActionListQuery,
    ) -> Result<PaginatedList<ActionLogRecord>, AppError> {
        let (limit_usize, fetch_limit) = validate_page_limit(query.limit)?;
        let (cursor_ts, cursor_id) = match query.cursor.as_deref().filter(|s| !s.trim().is_empty())
        {
            Some(c) => {
                let (ts, id) = parse_ts_i64_cursor(c.trim())?;
                (Some(ts), Some(id))
            }
            None => (None, None),
        };
        let status_filter = query.status.map(action_log_status_as_str);

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let rows = sqlx::query(
            "SELECT id, run_id, node_id, source_kind, action_kind,
                    target, payload, status, idempotency_key, error_message, created_at
               FROM ax_action_log
              WHERE ($1::uuid IS NULL OR run_id = $1)
                AND ($2::text IS NULL OR status = $2)
                AND ($3::timestamptz IS NULL OR (created_at, id) < ($3, $4::bigint))
              ORDER BY created_at DESC, id DESC
              LIMIT $5",
        )
        .bind(query.run_id)
        .bind(status_filter)
        .bind(cursor_ts)
        .bind(cursor_id)
        .bind(fetch_limit)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("list ax_action_log failed: {err}")))?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit list_action_logs tx failed: {err}"))
        })?;

        let mut items: Vec<ActionLogRecord> = rows
            .iter()
            .map(decode_action_log_record)
            .collect::<Result<_, _>>()?;
        let next_cursor = if items.len() > limit_usize {
            items.truncate(limit_usize);
            items
                .last()
                .map(|a| format!("{}|{}", a.created_at.to_rfc3339(), a.id))
        } else {
            None
        };

        Ok(PaginatedList { items, next_cursor })
    }

    async fn upsert_flow_hook(
        &self,
        tenant_id: &str,
        flow_id: Uuid,
        credential_id: Uuid,
    ) -> Result<(Uuid, DateTime<Utc>), AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;

        let locked_flow: Option<Uuid> =
            sqlx::query_scalar("SELECT id FROM ax_flows WHERE id = $1 FOR UPDATE")
                .bind(flow_id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(|err| {
                    AppError::Database(format!("lock ax_flows for hook upsert failed: {err}"))
                })?;
        if locked_flow.is_none() {
            return Err(AppError::NotFound("FLOW_NOT_FOUND"));
        }

        let old_cred: Option<Uuid> = sqlx::query_scalar(
            "SELECT credential_id FROM ax_flow_hooks WHERE flow_id = $1 FOR UPDATE",
        )
        .bind(flow_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("select ax_flow_hooks FOR UPDATE: {err}")))?;

        let row = sqlx::query(
            "INSERT INTO ax_flow_hooks (tenant_id, flow_id, credential_id, enabled, created_at)
             VALUES ($1, $2, $3, TRUE, NOW())
             ON CONFLICT (flow_id) DO UPDATE SET
                 credential_id = EXCLUDED.credential_id,
                 enabled = TRUE,
                 created_at = NOW()
             RETURNING id, created_at",
        )
        .bind(tenant_id)
        .bind(flow_id)
        .bind(credential_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("upsert ax_flow_hooks failed: {err}")))?;

        if let Some(old_id) = old_cred
            && old_id != credential_id
        {
            cleanup_orphan_credential_tx(&mut tx, old_id).await?;
        }

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit upsert_flow_hook tx failed: {err}"))
        })?;

        Ok((row.get("id"), row.get("created_at")))
    }

    async fn resolve_flow_hook(
        &self,
        tenant_id: &str,
        slug: &str,
    ) -> Result<Option<ResolvedFlowHook>, AppError> {
        let maybe_row =
            sqlx::query("SELECT flow_id, credential_id FROM ax_resolve_flow_hook($1, $2)")
                .bind(tenant_id)
                .bind(slug)
                .fetch_optional(self.pool())
                .await
                .map_err(|err| AppError::Database(format!("ax_resolve_flow_hook failed: {err}")))?;

        Ok(maybe_row.map(|row| ResolvedFlowHook {
            flow_id: row.get("flow_id"),
            credential_id: row.get("credential_id"),
        }))
    }

    async fn list_scheduled_flows(&self) -> Result<Vec<ScheduledFlowItem>, AppError> {
        let rows = sqlx::query(
            "SELECT tenant_id, flow_id, version, trigger_cfg, last_run_at
               FROM ax_scheduled_flows()",
        )
        .fetch_all(self.pool())
        .await
        .map_err(|err| AppError::Database(format!("ax_scheduled_flows failed: {err}")))?;

        let mut items = Vec::with_capacity(rows.len());
        for row in rows {
            let trigger_cfg: TriggerCfg =
                serde_json::from_value(row.get("trigger_cfg")).map_err(|err| {
                    AppError::Database(format!(
                        "decode ax_scheduled_flows trigger_cfg failed: {err}"
                    ))
                })?;
            items.push(ScheduledFlowItem {
                tenant_id: row.get("tenant_id"),
                flow_id: row.get("flow_id"),
                version: row.get("version"),
                trigger_cfg,
                last_run_at: row.get("last_run_at"),
            });
        }
        Ok(items)
    }

    async fn list_event_flows(&self, topic: &str) -> Result<Vec<EventFlowItem>, AppError> {
        let rows =
            sqlx::query("SELECT tenant_id, flow_id, version, trigger_cfg FROM ax_event_flows($1)")
                .bind(topic)
                .fetch_all(self.pool())
                .await
                .map_err(|err| AppError::Database(format!("ax_event_flows failed: {err}")))?;

        let mut items = Vec::with_capacity(rows.len());
        for row in rows {
            let trigger_cfg: TriggerCfg =
                serde_json::from_value(row.get("trigger_cfg")).map_err(|err| {
                    AppError::Database(format!("decode ax_event_flows trigger_cfg failed: {err}"))
                })?;
            items.push(EventFlowItem {
                tenant_id: row.get("tenant_id"),
                flow_id: row.get("flow_id"),
                version: row.get("version"),
                trigger_cfg,
            });
        }
        Ok(items)
    }

    async fn list_active_tenants(&self) -> Result<Vec<String>, AppError> {
        Self::list_active_tenants(self).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axpoint_domain::enums::FlowOperator;
    use axpoint_domain::flow::{JoinMode, OnFailure, Pos, RetryPolicy};
    use sqlx::PgPool;
    use sqlx::postgres::PgPoolOptions;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    async fn test_pools() -> Result<Option<(PgPool, PgPool)>, Box<dyn std::error::Error>> {
        let require_db = std::env::var("AXPOINT_REQUIRE_DB_TESTS")
            .is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"));

        let app_url = match std::env::var("DATABASE_URL") {
            Ok(url) if !url.trim().is_empty() => url,
            _ if require_db => {
                return Err("DATABASE_URL must be set when AXPOINT_REQUIRE_DB_TESTS=1".into());
            }
            _ => return Ok(None),
        };

        let owner_url = match std::env::var("MIGRATION_DATABASE_URL") {
            Ok(url) if !url.trim().is_empty() => url,
            _ => app_url.replace("axpoint_app:axpoint_app_secret", "axpoint:axpoint_secret"),
        };

        let app_pool = PgPoolOptions::new()
            .max_connections(10)
            .connect(&app_url)
            .await?;
        let owner_pool = PgPoolOptions::new()
            .max_connections(4)
            .connect(&owner_url)
            .await?;

        Ok(Some((app_pool, owner_pool)))
    }

    async fn ensure_tenant(owner_pool: &PgPool, tenant_id: &str) -> TestResult {
        sqlx::query(
            "INSERT INTO sec_tenants (tenant_id, display_name)
             VALUES ($1, $2)
             ON CONFLICT (tenant_id) DO NOTHING",
        )
        .bind(tenant_id)
        .bind(tenant_id)
        .execute(owner_pool)
        .await?;
        Ok(())
    }

    fn unique_tenant(prefix: &str) -> String {
        let hex_id = Uuid::new_v4().simple().to_string();
        let short: String = hex_id.chars().take(12).collect();
        format!("{prefix}_{short}")
    }

    fn sample_node(id: &str, kind: FlowOperator) -> FlowNode {
        FlowNode {
            id: String::from(id),
            kind,
            label: String::from(id),
            config: serde_json::json!({}),
            retry: RetryPolicy::default(),
            timeout_s: 600,
            on_failure: OnFailure::FailRun,
            join: JoinMode::AllSuccess,
            pos: Pos { x: 100.0, y: 120.0 },
        }
    }

    async fn cleanup_tenants(owner_pool: &PgPool, tenants: &[String]) -> TestResult {
        for sql in [
            "DELETE FROM ax_action_log WHERE tenant_id = ANY($1)",
            "DELETE FROM ax_approvals WHERE tenant_id = ANY($1)",
            "DELETE FROM ax_flow_hooks WHERE tenant_id = ANY($1)",
            "DELETE FROM ax_flows WHERE tenant_id = ANY($1)",
            "DELETE FROM ds_credentials WHERE tenant_id = ANY($1)",
            "DELETE FROM ax_jobs WHERE tenant_id = ANY($1)",
            "DELETE FROM ax_outbox WHERE tenant_id = ANY($1)",
            "DELETE FROM sec_audit_logs WHERE tenant_id = ANY($1)",
            "DELETE FROM sec_tenants WHERE tenant_id = ANY($1)",
        ] {
            sqlx::query(sql).bind(tenants).execute(owner_pool).await?;
        }
        Ok(())
    }

    #[tokio::test]
    async fn test_flow_repo_rls_isolates_tenants() -> TestResult {
        let Some((app_pool, owner_pool)) = test_pools().await? else {
            return Ok(());
        };

        let t1 = unique_tenant("t_flw_rls1");
        let t2 = unique_tenant("t_flw_rls2");
        ensure_tenant(&owner_pool, &t1).await?;
        ensure_tenant(&owner_pool, &t2).await?;

        let store = PgStore::new(app_pool.clone());
        let flow = store
            .create_flow(
                &t1,
                None,
                CreateAxFlowRequest {
                    name: String::from("rls_flow"),
                    description: String::from("isolated flow"),
                    slug: Some(String::from("rls-slug")),
                    nodes: vec![sample_node("root_a", FlowOperator::AggTrigger)],
                    edges: Vec::new(),
                    trigger_cfg: TriggerCfg::manual(),
                    steps: None,
                },
            )
            .await?;

        let ver = store
            .publish_flow_version(
                &t1,
                flow.id,
                None,
                PublishFlowVersionRequest {
                    note: String::from("v1"),
                },
            )
            .await?;
        store
            .deploy_flow(
                &t1,
                flow.id,
                None,
                DeployFlowRequest {
                    environment: Environment::Test,
                    version: ver.version,
                },
            )
            .await?;

        let (run, existing) = store
            .start_flow_run(
                &t1,
                StartRunParams {
                    flow_id: flow.id,
                    environment: Environment::Test,
                    explicit_version: None,
                    trigger_source: TriggerType::Manual,
                    trigger_payload: serde_json::json!({}),
                    idempotency_key: None,
                    started_by: None,
                },
            )
            .await?;
        assert!(!existing);

        let approval = store
            .create_or_get_approval(
                &t1,
                NewApprovalInput {
                    run_id: Some(run.id),
                    node_id: Some(String::from("root_a")),
                    source_kind: ApprovalSourceKind::Flow,
                    source_ref: String::new(),
                    title: String::from("Approve RLS"),
                    detail: serde_json::json!({}),
                    assignee_roles: Vec::new(),
                    assignee_users: Vec::new(),
                    expires_at: None,
                },
            )
            .await?;

        assert!(store.get_flow(&t2, flow.id).await?.is_none());
        assert!(
            store
                .list_flows(&t2, FlowListQuery::default())
                .await?
                .items
                .is_empty()
        );
        assert!(store.get_flow_version(&t2, flow.id, 1).await?.is_none());
        assert!(store.get_flow_run(&t2, run.id).await?.is_none());
        assert!(store.get_approval(&t2, approval.id).await?.is_none());
        assert!(store.list_flow_versions(&t2, flow.id).await.is_err());

        let unauthenticated_cnt: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ax_flows")
            .fetch_one(&app_pool)
            .await?;
        assert_eq!(unauthenticated_cnt, 0);

        cleanup_tenants(&owner_pool, &[t1, t2]).await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_start_flow_run_idempotency() -> TestResult {
        let Some((app_pool, owner_pool)) = test_pools().await? else {
            return Ok(());
        };

        let tenant = unique_tenant("t_flw_idem");
        ensure_tenant(&owner_pool, &tenant).await?;

        let store = PgStore::new(app_pool);
        let flow = store
            .create_flow(
                &tenant,
                None,
                CreateAxFlowRequest {
                    name: String::from("idem_flow"),
                    description: String::new(),
                    slug: Some(String::from("   ")),
                    nodes: vec![sample_node("n1", FlowOperator::AggTrigger)],
                    edges: Vec::new(),
                    trigger_cfg: TriggerCfg::manual(),
                    steps: None,
                },
            )
            .await?;
        assert!(flow.slug.is_none());

        store
            .publish_flow_version(
                &tenant,
                flow.id,
                None,
                PublishFlowVersionRequest {
                    note: String::from("v1"),
                },
            )
            .await?;

        // Undeployed TEST falls back to MAX(version) in ax_flow_versions.
        let params = StartRunParams {
            flow_id: flow.id,
            environment: Environment::Test,
            explicit_version: None,
            trigger_source: TriggerType::Manual,
            trigger_payload: serde_json::json!({"batch": 7}),
            idempotency_key: Some(String::from("idem-run-001")),
            started_by: None,
        };

        let (run_first, existing_first) = store.start_flow_run(&tenant, params.clone()).await?;
        assert!(!existing_first);
        assert_eq!(run_first.status, RunStatus::Running);
        assert_eq!(run_first.version, 1);

        let (run_dup, existing_dup) = store.start_flow_run(&tenant, params).await?;
        assert!(existing_dup);
        assert_eq!(run_dup.id, run_first.id);

        let steps = store.list_run_steps(&tenant, run_first.id).await?;
        assert_eq!(steps.len(), 1);
        assert!(steps.first().and_then(|s| s.job_id).is_some());

        let job_cnt: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM ax_jobs WHERE tenant_id = $1 AND kind = 'FLOW_STEP'",
        )
        .bind(&tenant)
        .fetch_one(&owner_pool)
        .await?;
        assert_eq!(job_cnt, 1);

        cleanup_tenants(&owner_pool, &[tenant]).await?;
        Ok(())
    }

    async fn create_published_test_run(
        store: &PgStore,
        tenant: &str,
        name: &str,
        nodes: Vec<FlowNode>,
        edges: Vec<FlowEdge>,
    ) -> Result<(AxFlowDef, RunRecord), AppError> {
        let flow = store
            .create_flow(
                tenant,
                None,
                CreateAxFlowRequest {
                    name: String::from(name),
                    description: String::new(),
                    slug: None,
                    nodes,
                    edges,
                    trigger_cfg: TriggerCfg::manual(),
                    steps: None,
                },
            )
            .await?;
        store
            .publish_flow_version(tenant, flow.id, None, PublishFlowVersionRequest::default())
            .await?;
        let (run, _) = store
            .start_flow_run(
                tenant,
                StartRunParams {
                    flow_id: flow.id,
                    environment: Environment::Test,
                    explicit_version: None,
                    trigger_source: TriggerType::Manual,
                    trigger_payload: serde_json::json!({}),
                    idempotency_key: None,
                    started_by: None,
                },
            )
            .await?;
        Ok((flow, run))
    }

    #[tokio::test]
    async fn test_concurrent_two_predecessor_join_enqueues_successor_once() -> TestResult {
        let Some((app_pool, owner_pool)) = test_pools().await? else {
            return Ok(());
        };
        let tenant = unique_tenant("t_flw_join");
        ensure_tenant(&owner_pool, &tenant).await?;

        let store = PgStore::new(app_pool);
        let (_, run) = create_published_test_run(
            &store,
            &tenant,
            "diamond_join",
            vec![
                sample_node("pred_a", FlowOperator::AggTrigger),
                sample_node("pred_b", FlowOperator::AggTrigger),
                sample_node("join_c", FlowOperator::ActionEmit),
            ],
            vec![
                FlowEdge {
                    from: String::from("pred_a"),
                    to: String::from("join_c"),
                    when: None,
                },
                FlowEdge {
                    from: String::from("pred_b"),
                    to: String::from("join_c"),
                    when: None,
                },
            ],
        )
        .await?;

        let planner = |_r: &RunRecord,
                       _v: &FlowVersionRecord,
                       steps: &[StepRecord]|
         -> Result<RunProgressionPlan, AppError> {
            let a_done = steps
                .iter()
                .any(|s| s.node_id == "pred_a" && s.status == StepStatus::Succeeded);
            let b_done = steps
                .iter()
                .any(|s| s.node_id == "pred_b" && s.status == StepStatus::Succeeded);
            let c_exists = steps.iter().any(|s| s.node_id == "join_c");
            let mut plan = RunProgressionPlan {
                next_run_status: Some(RunStatus::Running),
                ..RunProgressionPlan::default()
            };
            if a_done && b_done && !c_exists {
                plan.steps_to_enqueue.push((String::from("join_c"), 1));
            }
            Ok(plan)
        };

        let (res_a, res_b) = tokio::join!(
            store.apply_step_mutation_and_progress(
                &tenant,
                run.id,
                StepMutation::Succeeded {
                    node_id: String::from("pred_a"),
                    attempt: 1,
                    output: Some(serde_json::json!({"ok": "a"})),
                    output_ref: None,
                },
                &planner,
            ),
            store.apply_step_mutation_and_progress(
                &tenant,
                run.id,
                StepMutation::Succeeded {
                    node_id: String::from("pred_b"),
                    attempt: 1,
                    output: Some(serde_json::json!({"ok": "b"})),
                    output_ref: None,
                },
                &planner,
            ),
        );
        res_a?;
        res_b?;

        let steps = store.list_run_steps(&tenant, run.id).await?;
        let join_steps: Vec<&StepRecord> = steps.iter().filter(|s| s.node_id == "join_c").collect();
        assert_eq!(join_steps.len(), 1);
        assert_eq!(
            join_steps.first().map(|s| s.status),
            Some(StepStatus::Pending)
        );

        let join_jobs: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM ax_jobs
              WHERE tenant_id = $1
                AND kind = 'FLOW_STEP'
                AND payload->>'run_id' = $2
                AND payload->>'node_id' = 'join_c'",
        )
        .bind(&tenant)
        .bind(run.id.to_string())
        .fetch_one(&owner_pool)
        .await?;
        assert_eq!(join_jobs, 1);

        cleanup_tenants(&owner_pool, &[tenant]).await?;
        Ok(())
    }

    async fn assert_approval_outbox_and_audit(
        owner_pool: &PgPool,
        tenant: &str,
        approval_id: Uuid,
        actor_id: Uuid,
    ) -> TestResult {
        let outbox_payload: serde_json::Value = sqlx::query_scalar(
            "SELECT payload FROM ax_outbox WHERE tenant_id = $1 AND topic = 'approval.decided'",
        )
        .bind(tenant)
        .fetch_one(owner_pool)
        .await?;
        assert_eq!(
            outbox_payload
                .get("approval_id")
                .and_then(serde_json::Value::as_str),
            Some(approval_id.to_string().as_str())
        );
        assert_eq!(
            outbox_payload
                .get("status")
                .and_then(serde_json::Value::as_str),
            Some("APPROVED")
        );

        let audit_cnt: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sec_audit_logs
              WHERE tenant_id = $1 AND action = 'APPROVAL_DECIDE' AND user_id = $2",
        )
        .bind(tenant)
        .bind(actor_id)
        .fetch_one(owner_pool)
        .await?;
        assert_eq!(audit_cnt, 1);
        Ok(())
    }

    #[tokio::test]
    async fn test_decide_approval_rbac_outbox_and_audit() -> TestResult {
        let Some((app_pool, owner_pool)) = test_pools().await? else {
            return Ok(());
        };
        let tenant = unique_tenant("t_flw_aprv");
        ensure_tenant(&owner_pool, &tenant).await?;

        let store = PgStore::new(app_pool);
        let (_, run) = create_published_test_run(
            &store,
            &tenant,
            "approval_flow",
            vec![sample_node("gate", FlowOperator::HumanApproval)],
            Vec::new(),
        )
        .await?;

        let designated_user = Uuid::new_v4();
        let ap_input = NewApprovalInput {
            run_id: Some(run.id),
            node_id: Some(String::from("gate")),
            source_kind: ApprovalSourceKind::Flow,
            source_ref: String::from("gate-ref"),
            title: String::from("Release Gate"),
            detail: serde_json::json!({"amount": 500}),
            assignee_roles: vec![String::from("OPERATOR")],
            assignee_users: vec![designated_user],
            expires_at: None,
        };
        let ap1 = store
            .create_or_get_approval(&tenant, ap_input.clone())
            .await?;
        let ap2 = store.create_or_get_approval(&tenant, ap_input).await?;
        assert_eq!(ap1.id, ap2.id);

        let forbidden_res = store
            .decide_approval(
                &tenant,
                DecideApprovalParams {
                    approval_id: ap1.id,
                    actor_user_id: Uuid::new_v4(),
                    actor_roles: vec![String::from("VIEWER")],
                    decision: ApprovalDecision::Approve,
                    comment: String::from("intruder"),
                },
            )
            .await;
        assert!(matches!(
            forbidden_res,
            Err(AppError::Forbidden("PERMISSION_DENIED"))
        ));

        let operator_actor = Uuid::new_v4();
        let decided = store
            .decide_approval(
                &tenant,
                DecideApprovalParams {
                    approval_id: ap1.id,
                    actor_user_id: operator_actor,
                    actor_roles: vec![String::from("OPERATOR")],
                    decision: ApprovalDecision::Approve,
                    comment: String::from("approved by operator"),
                },
            )
            .await?;
        assert_eq!(decided.status, ApprovalStatus::Approved);
        assert_eq!(decided.decided_by, Some(operator_actor));

        let replay_res = store
            .decide_approval(
                &tenant,
                DecideApprovalParams {
                    approval_id: ap1.id,
                    actor_user_id: designated_user,
                    actor_roles: Vec::new(),
                    decision: ApprovalDecision::Reject,
                    comment: String::from("too late"),
                },
            )
            .await;
        assert!(matches!(replay_res, Err(AppError::Conflict("CONFLICT"))));

        assert_approval_outbox_and_audit(&owner_pool, &tenant, ap1.id, operator_actor).await?;

        cleanup_tenants(&owner_pool, &[tenant]).await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_cancel_and_retry_flow_run() -> TestResult {
        let Some((app_pool, owner_pool)) = test_pools().await? else {
            return Ok(());
        };
        let tenant = unique_tenant("t_flw_rtry");
        ensure_tenant(&owner_pool, &tenant).await?;

        let store = PgStore::new(app_pool);
        let (flow, run_cancel) = create_published_test_run(
            &store,
            &tenant,
            "lifecycle_flow",
            vec![
                sample_node("step_1", FlowOperator::AggTrigger),
                sample_node("step_2", FlowOperator::ActionEmit),
            ],
            vec![FlowEdge {
                from: String::from("step_1"),
                to: String::from("step_2"),
                when: None,
            }],
        )
        .await?;

        let canceled = store.cancel_flow_run(&tenant, run_cancel.id).await?;
        assert_eq!(canceled.status, RunStatus::Canceled);
        assert!(matches!(
            store.retry_flow_run(&tenant, run_cancel.id).await,
            Err(AppError::Conflict("CONFLICT"))
        ));

        let (run_fail, _) = store
            .start_flow_run(
                &tenant,
                StartRunParams {
                    flow_id: flow.id,
                    environment: Environment::Test,
                    explicit_version: None,
                    trigger_source: TriggerType::Manual,
                    trigger_payload: serde_json::json!({}),
                    idempotency_key: None,
                    started_by: None,
                },
            )
            .await?;
        let fail_planner = |_r: &RunRecord,
                            _v: &FlowVersionRecord,
                            _s: &[StepRecord]|
         -> Result<RunProgressionPlan, AppError> {
            Ok(RunProgressionPlan {
                nodes_to_skip: vec![String::from("step_2")],
                steps_to_enqueue: Vec::new(),
                next_run_status: Some(RunStatus::Failed),
                run_outputs: None,
                run_error_message: Some(String::from("step_1 boom")),
            })
        };
        let failed = store
            .apply_step_mutation_and_progress(
                &tenant,
                run_fail.id,
                StepMutation::TerminalFailed {
                    node_id: String::from("step_1"),
                    attempt: 1,
                    error_code: String::from("BOOM"),
                    error_message: String::from("step_1 boom"),
                },
                &fail_planner,
            )
            .await?;
        assert_eq!(failed.status, RunStatus::Failed);

        let retried = store.retry_flow_run(&tenant, run_fail.id).await?;
        assert_eq!(retried.status, RunStatus::Running);
        let latest = store.list_latest_run_steps(&tenant, run_fail.id).await?;
        assert_eq!(latest.len(), 1);
        assert_eq!(latest.first().map(|s| s.attempt), Some(2));
        assert_eq!(latest.first().map(|s| s.status), Some(StepStatus::Pending));

        cleanup_tenants(&owner_pool, &[tenant]).await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_expire_approvals_action_logs_and_step_cache() -> TestResult {
        let Some((app_pool, owner_pool)) = test_pools().await? else {
            return Ok(());
        };
        let tenant = unique_tenant("t_flw_actn");
        ensure_tenant(&owner_pool, &tenant).await?;

        let store = PgStore::new(app_pool);
        let (_, run) = create_published_test_run(
            &store,
            &tenant,
            "cache_and_action_flow",
            vec![sample_node("extract_1", FlowOperator::LlmExtract)],
            Vec::new(),
        )
        .await?;

        store
            .create_or_get_approval(
                &tenant,
                NewApprovalInput {
                    run_id: None,
                    node_id: None,
                    source_kind: ApprovalSourceKind::Flow,
                    source_ref: String::from("exp-1"),
                    title: String::from("Overdue"),
                    detail: serde_json::json!({}),
                    assignee_roles: Vec::new(),
                    assignee_users: Vec::new(),
                    expires_at: Some(Utc::now() - chrono::Duration::minutes(5)),
                },
            )
            .await?;
        assert_eq!(store.expire_overdue_approvals(&tenant).await?, 1);

        let act_in = NewActionLogInput {
            run_id: Some(run.id),
            node_id: Some(String::from("extract_1")),
            source_kind: ActionSourceKind::Flow,
            action_kind: ActionKind::WriteNodeMetric,
            target: String::from("W-1"),
            payload: serde_json::json!({"metric": "flag", "value": 1}),
            status: ActionLogStatus::DryRun,
            idempotency_key: format!("{}:extract_1:0", run.id),
            error_message: None,
        };
        let (a1, ex1) = store.record_action_log(&tenant, act_in.clone()).await?;
        let (a2, ex2) = store.record_action_log(&tenant, act_in).await?;
        assert!(!ex1 && ex2 && a1.id == a2.id);

        let rendered_in = serde_json::json!({"prompt": "hello"});
        store
            .mark_step_running(&tenant, run.id, "extract_1", 1, None, &rendered_in)
            .await?;
        let noop_planner = |_r: &RunRecord,
                            _v: &FlowVersionRecord,
                            _s: &[StepRecord]|
         -> Result<RunProgressionPlan, AppError> {
            Ok(RunProgressionPlan::default())
        };
        store
            .apply_step_mutation_and_progress(
                &tenant,
                run.id,
                StepMutation::Succeeded {
                    node_id: String::from("extract_1"),
                    attempt: 1,
                    output: Some(serde_json::json!({"entities": 3})),
                    output_ref: None,
                },
                &noop_planner,
            )
            .await?;
        let cached = store
            .find_cached_step_output(&tenant, run.id, "extract_1", &rendered_in)
            .await?;
        assert_eq!(cached, Some(serde_json::json!({"entities": 3})));

        cleanup_tenants(&owner_pool, &[tenant]).await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_hooks_and_scheduled_flows() -> TestResult {
        let Some((app_pool, owner_pool)) = test_pools().await? else {
            return Ok(());
        };
        let tenant = unique_tenant("t_flw_hook");
        ensure_tenant(&owner_pool, &tenant).await?;

        let store = PgStore::new(app_pool.clone());
        let flow = store
            .create_flow(
                &tenant,
                None,
                CreateAxFlowRequest {
                    name: String::from("cron_hook_flow"),
                    description: String::new(),
                    slug: Some(String::from("inbound-hook")),
                    nodes: vec![sample_node("extract_1", FlowOperator::LlmExtract)],
                    edges: Vec::new(),
                    trigger_cfg: TriggerCfg {
                        kind: TriggerType::Cron,
                        cron: Some(String::from("0 */1 * * *")),
                        timezone: Some(String::from("UTC")),
                        catch_up: Some(false),
                        topic: None,
                        filter: None,
                    },
                    steps: None,
                },
            )
            .await?;
        let ver = store
            .publish_flow_version(&tenant, flow.id, None, PublishFlowVersionRequest::default())
            .await?;
        store
            .deploy_flow(
                &tenant,
                flow.id,
                None,
                DeployFlowRequest {
                    environment: Environment::Prod,
                    version: ver.version,
                },
            )
            .await?;

        let scheduled = store.list_scheduled_flows().await?;
        assert!(scheduled.iter().any(|item| item.flow_id == flow.id));

        let mut cred_tx = begin_tenant_tx(&app_pool, &tenant).await?;
        let cred_id: Uuid = sqlx::query_scalar(
            "INSERT INTO ds_credentials (tenant_id, key_version, nonce, ciphertext)
             VALUES ($1, 1, $2, $3)
             RETURNING id",
        )
        .bind(&tenant)
        .bind(vec![0_u8; 12])
        .bind(vec![1_u8; 16])
        .fetch_one(&mut *cred_tx)
        .await?;
        cred_tx.commit().await?;

        store.upsert_flow_hook(&tenant, flow.id, cred_id).await?;
        let resolved = store.resolve_flow_hook(&tenant, "inbound-hook").await?;
        assert_eq!(resolved.map(|h| h.flow_id), Some(flow.id));
        assert!(store.delete_flow(&tenant, flow.id).await?);

        cleanup_tenants(&owner_pool, &[tenant]).await?;
        Ok(())
    }
}
