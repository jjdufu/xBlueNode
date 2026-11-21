//! Cross-department `sec_grants` approval workflow, `sec_sessions` listing, and tamper-evident
//! `sec_audit_logs` querying, chain verification, and aggregation (`11-STEP13` §13.9–§13.10).

use std::collections::BTreeMap;

use axpoint_domain::AppError;
use axpoint_domain::axhub::PaginatedList;
use axpoint_domain::enums::AuditAction;
use axpoint_domain::guard::{
    AuditLogEntry, AuditQueryFilter, AuditStatsResponse, AuditVerifyQuery, AuditVerifyResponse,
    CreateGrantRequest, GrantRecord, GrantResourceKind, GrantStatus, GranteeKind,
    SessionSummaryRecord,
};
use axpoint_security::audit_chain::verify_audit_chain;
use chrono::{DateTime, Duration, Utc};
use sqlx::postgres::PgRow;
use sqlx::{Postgres, QueryBuilder, Row as _};
use uuid::Uuid;

use super::policies::resolve_resource_ref_tx;
use super::{notify_policy_changed_tx, parse_auth_source, parse_permissions_vec};
use crate::PgStore;
use crate::tenant::begin_tenant_tx;

fn parse_grant_resource_kind(raw: &str) -> Result<GrantResourceKind, AppError> {
    match raw {
        "DATASET" => Ok(GrantResourceKind::Dataset),
        "NODE_TYPE" => Ok(GrantResourceKind::NodeType),
        "SUBGRAPH" => Ok(GrantResourceKind::Subgraph),
        "DASHBOARD" => Ok(GrantResourceKind::Dashboard),
        "AGENT" => Ok(GrantResourceKind::Agent),
        other => Err(AppError::Database(format!(
            "unexpected sec_grants.resource_kind: {other}"
        ))),
    }
}

fn parse_grantee_kind(raw: &str) -> Result<GranteeKind, AppError> {
    match raw {
        "DEPT" => Ok(GranteeKind::Dept),
        "ROLE" => Ok(GranteeKind::Role),
        "USER" => Ok(GranteeKind::User),
        other => Err(AppError::Database(format!(
            "unexpected sec_grants.grantee_kind: {other}"
        ))),
    }
}

fn parse_grant_status(raw: &str) -> Result<GrantStatus, AppError> {
    match raw {
        "PENDING" => Ok(GrantStatus::Pending),
        "ACTIVE" => Ok(GrantStatus::Active),
        "REJECTED" => Ok(GrantStatus::Rejected),
        "REVOKED" => Ok(GrantStatus::Revoked),
        "EXPIRED" => Ok(GrantStatus::Expired),
        other => Err(AppError::Database(format!(
            "unexpected sec_grants.status: {other}"
        ))),
    }
}

fn decode_grant_row(row: &PgRow) -> Result<GrantRecord, AppError> {
    let resource_kind_raw: String = row.get("resource_kind");
    let grantee_kind_raw: String = row.get("grantee_kind");
    let raw_status: String = row.get("status");
    let raw_actions: Vec<String> = row.get("actions");
    Ok(GrantRecord {
        id: row.get("id"),
        tenant_id: row.get("tenant_id"),
        resource_kind: parse_grant_resource_kind(&resource_kind_raw)?,
        resource_ref: row.get("resource_ref"),
        grantee_kind: parse_grantee_kind(&grantee_kind_raw)?,
        grantee_ref: row.get("grantee_ref"),
        actions: parse_permissions_vec(raw_actions)?,
        row_predicate: row.get("row_predicate"),
        reason: row.get("reason"),
        status: parse_grant_status(&raw_status)?,
        approval_id: row.get("approval_id"),
        requested_by: row.get("requested_by"),
        expires_at: row.get("expires_at"),
        created_at: row.get("created_at"),
        decided_at: row.get("decided_at"),
    })
}

pub(super) async fn create_grant_impl(
    store: &PgStore,
    tenant_id: &str,
    requested_by: Uuid,
    req: CreateGrantRequest,
) -> Result<GrantRecord, AppError> {
    if !(1..=365).contains(&req.expires_in_days) {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "expires_in_days must be between 1 and 365",
        ));
    }
    if req.reason.trim().is_empty() {
        return Err(AppError::bad("INVALID_ARGUMENT", "reason cannot be empty"));
    }
    if req.actions.is_empty() {
        return Err(AppError::bad("INVALID_ARGUMENT", "actions cannot be empty"));
    }

    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let res_ref = resolve_resource_ref_tx(&mut tx, req.resource_kind, &req.resource_ref)
        .await?
        .ok_or(AppError::NotFound("RESOURCE_NOT_FOUND"))?;

    let grant_id = Uuid::new_v4();
    let expires_at = Utc::now() + Duration::days(i64::from(req.expires_in_days));
    let action_strs: Vec<String> = req
        .actions
        .iter()
        .map(|a| String::from(a.as_str()))
        .collect();
    let clean_pred = req
        .row_predicate
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from);

    let title = format!(
        "跨部门访问授权申请：{}:{} -> {}:{}",
        req.resource_kind.as_str(),
        res_ref.id,
        req.grantee_kind.as_str(),
        req.grantee_ref.trim()
    );
    let detail_json = serde_json::json!({
        "grant_id": grant_id,
        "resource_kind": req.resource_kind.as_str(),
        "resource_ref": res_ref.id,
        "owner_dept": res_ref.owner_dept,
        "sensitivity": res_ref.sensitivity.as_str(),
        "grantee_kind": req.grantee_kind.as_str(),
        "grantee_ref": req.grantee_ref.trim(),
        "actions": action_strs,
        "row_predicate": clean_pred,
        "reason": req.reason.trim(),
        "requested_by": requested_by,
    });

    let apr_row = sqlx::query(
        "INSERT INTO ax_approvals (
             tenant_id, source_kind, source_ref, title, detail,
             assignee_roles, status, expires_at
         )
         VALUES ($1, 'GRANT', $2, $3, $4, ARRAY['ADMIN'], 'PENDING', $5)
         RETURNING id",
    )
    .bind(tenant_id)
    .bind(grant_id.to_string())
    .bind(&title)
    .bind(&detail_json)
    .bind(expires_at)
    .fetch_one(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("insert grant ax_approvals failed: {err}")))?;
    let approval_id: Uuid = apr_row.get("id");

    let row = sqlx::query(
        "INSERT INTO sec_grants (
             id, tenant_id, resource_kind, resource_ref, grantee_kind, grantee_ref,
             actions, row_predicate, reason, status, approval_id, requested_by, expires_at
         )
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, 'PENDING', $10, $11, $12)
         RETURNING id, tenant_id, resource_kind, resource_ref, grantee_kind, grantee_ref,
                   actions, row_predicate, reason, status, approval_id, requested_by,
                   expires_at, created_at, decided_at",
    )
    .bind(grant_id)
    .bind(tenant_id)
    .bind(req.resource_kind.as_str())
    .bind(&res_ref.id)
    .bind(req.grantee_kind.as_str())
    .bind(req.grantee_ref.trim())
    .bind(&action_strs)
    .bind(&clean_pred)
    .bind(req.reason.trim())
    .bind(approval_id)
    .bind(requested_by)
    .bind(expires_at)
    .fetch_one(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("insert sec_grants failed: {err}")))?;

    let rec = decode_grant_row(&row)?;
    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit create_grant tx: {err}")))?;
    Ok(rec)
}

pub(super) async fn list_grants_impl(
    store: &PgStore,
    tenant_id: &str,
    viewer_filter: Option<(Uuid, &str)>,
) -> Result<Vec<GrantRecord>, AppError> {
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let rows = match viewer_filter {
        Some((uid, dept_id)) => {
            let uid_str = uid.to_string();
            sqlx::query(
                "SELECT id, tenant_id, resource_kind, resource_ref, grantee_kind, grantee_ref,
                        actions, row_predicate, reason, status, approval_id, requested_by,
                        expires_at, created_at, decided_at
                   FROM sec_grants
                  WHERE requested_by = $1
                     OR (grantee_kind = 'USER' AND grantee_ref = $2)
                     OR (grantee_kind = 'DEPT' AND grantee_ref = $3)
                  ORDER BY created_at DESC",
            )
            .bind(uid)
            .bind(uid_str)
            .bind(dept_id)
            .fetch_all(&mut *tx)
            .await
        }
        None => {
            sqlx::query(
                "SELECT id, tenant_id, resource_kind, resource_ref, grantee_kind, grantee_ref,
                        actions, row_predicate, reason, status, approval_id, requested_by,
                        expires_at, created_at, decided_at
                   FROM sec_grants
                  ORDER BY created_at DESC",
            )
            .fetch_all(&mut *tx)
            .await
        }
    }
    .map_err(|err| AppError::Database(format!("list sec_grants failed: {err}")))?;

    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit list_grants tx: {err}")))?;

    let mut out = Vec::with_capacity(rows.len());
    for r in &rows {
        out.push(decode_grant_row(r)?);
    }
    Ok(out)
}

pub(super) async fn get_grant_by_approval_impl(
    store: &PgStore,
    tenant_id: &str,
    approval_id: Uuid,
) -> Result<Option<GrantRecord>, AppError> {
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let row_opt = sqlx::query(
        "SELECT id, tenant_id, resource_kind, resource_ref, grantee_kind, grantee_ref,
                actions, row_predicate, reason, status, approval_id, requested_by,
                expires_at, created_at, decided_at
           FROM sec_grants
          WHERE approval_id = $1",
    )
    .bind(approval_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("get_grant_by_approval failed: {err}")))?;

    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit get_grant_by_approval tx: {err}")))?;

    row_opt.as_ref().map(decode_grant_row).transpose()
}

pub(super) async fn apply_grant_decision_impl(
    store: &PgStore,
    tenant_id: &str,
    approval_id: Uuid,
    approved: bool,
) -> Result<Option<GrantRecord>, AppError> {
    let next_status = if approved { "ACTIVE" } else { "REJECTED" };
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let row_opt = sqlx::query(
        "UPDATE sec_grants
            SET status = $2,
                decided_at = NOW()
          WHERE approval_id = $1
      RETURNING id, tenant_id, resource_kind, resource_ref, grantee_kind, grantee_ref,
                actions, row_predicate, reason, status, approval_id, requested_by,
                expires_at, created_at, decided_at",
    )
    .bind(approval_id)
    .bind(next_status)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("apply_grant_decision failed: {err}")))?;

    if row_opt.is_some() {
        notify_policy_changed_tx(&mut tx, tenant_id).await?;
    }
    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit apply_grant_decision tx: {err}")))?;

    row_opt.as_ref().map(decode_grant_row).transpose()
}

pub(super) async fn revoke_grant_impl(
    store: &PgStore,
    tenant_id: &str,
    grant_id: Uuid,
) -> Result<bool, AppError> {
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let res = sqlx::query(
        "UPDATE sec_grants
            SET status = 'REVOKED'
          WHERE id = $1 AND status IN ('PENDING', 'ACTIVE')",
    )
    .bind(grant_id)
    .execute(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("revoke sec_grants failed: {err}")))?;

    let changed = res.rows_affected() > 0;
    if changed {
        sqlx::query(
            "UPDATE ax_approvals
                SET status = 'REJECTED', decided_at = COALESCE(decided_at, NOW())
              WHERE source_kind = 'GRANT' AND source_ref = $1 AND status = 'PENDING'",
        )
        .bind(grant_id.to_string())
        .execute(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("cancel grant approval failed: {err}")))?;
        notify_policy_changed_tx(&mut tx, tenant_id).await?;
    }

    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit revoke_grant tx: {err}")))?;
    Ok(changed)
}

pub(super) async fn list_active_grants_impl(
    store: &PgStore,
    tenant_id: &str,
) -> Result<Vec<GrantRecord>, AppError> {
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let rows = sqlx::query(
        "SELECT id, tenant_id, resource_kind, resource_ref, grantee_kind, grantee_ref,
                actions, row_predicate, reason, status, approval_id, requested_by,
                expires_at, created_at, decided_at
           FROM sec_grants
          WHERE status = 'ACTIVE' AND expires_at > NOW()
          ORDER BY created_at ASC",
    )
    .fetch_all(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("list_active_grants failed: {err}")))?;
    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit list_active_grants tx: {err}")))?;

    let mut out = Vec::with_capacity(rows.len());
    for r in &rows {
        out.push(decode_grant_row(r)?);
    }
    Ok(out)
}

pub(super) async fn list_sessions_impl(
    store: &PgStore,
    tenant_id: &str,
    user_id_filter: Option<Uuid>,
) -> Result<Vec<SessionSummaryRecord>, AppError> {
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let rows = sqlx::query(
        "SELECT s.id, s.family_id, s.user_id, u.username, s.auth_source,
                s.user_agent, s.client_ip, s.expires_at, s.rotated_at, s.revoked_at, s.created_at
           FROM sec_sessions s
           JOIN sec_users u ON u.id = s.user_id
          WHERE ($1::uuid IS NULL OR s.user_id = $1)
          ORDER BY s.created_at DESC
          LIMIT 500",
    )
    .bind(user_id_filter)
    .fetch_all(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("list sec_sessions failed: {err}")))?;
    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit list_sessions tx: {err}")))?;

    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let raw_src: String = r.get("auth_source");
        out.push(SessionSummaryRecord {
            id: r.get("id"),
            family_id: r.get("family_id"),
            user_id: r.get("user_id"),
            username: r.get("username"),
            auth_source: parse_auth_source(&raw_src)?,
            user_agent: r.get("user_agent"),
            client_ip: r.get("client_ip"),
            expires_at: r.get("expires_at"),
            rotated_at: r.get("rotated_at"),
            revoked_at: r.get("revoked_at"),
            created_at: r.get("created_at"),
        });
    }
    Ok(out)
}

fn decode_audit_entry(row: &PgRow) -> Result<AuditLogEntry, AppError> {
    let raw_action: String = row.get("action");
    let action: AuditAction = raw_action.parse()?;
    Ok(AuditLogEntry {
        id: row.get("id"),
        tenant_id: row.get("tenant_id"),
        user_id: row.get("user_id"),
        username: row.get("username"),
        action,
        resource_ref: row.get("resource_ref"),
        status_code: row.get("status_code"),
        client_ip: row.get("client_ip"),
        trace_id: row.get("trace_id"),
        detail_json: row.get("detail_json"),
        prev_hash: row.get("prev_hash"),
        row_hash: row.get("row_hash"),
        created_at: row.get("created_at"),
    })
}

fn push_audit_filters(qb: &mut QueryBuilder<Postgres>, filter: &AuditQueryFilter) {
    if let Some(act) = filter.action {
        qb.push(" AND action = ");
        qb.push_bind(act.as_str());
    }
    if let Some(uname) = filter
        .username
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        qb.push(" AND username ILIKE ");
        qb.push_bind(format!("%{uname}%"));
    }
    if let Some(res) = filter
        .resource
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        qb.push(" AND resource_ref ILIKE ");
        qb.push_bind(format!("%{res}%"));
    }
    if let Some(from_ts) = filter.from {
        qb.push(" AND created_at >= ");
        qb.push_bind(from_ts);
    }
    if let Some(to_ts) = filter.to {
        qb.push(" AND created_at <= ");
        qb.push_bind(to_ts);
    }
    if let Some(code) = filter.status_code {
        qb.push(" AND status_code = ");
        qb.push_bind(code);
    }
}

pub(super) async fn query_audit_logs_impl(
    store: &PgStore,
    tenant_id: &str,
    filter: &AuditQueryFilter,
) -> Result<PaginatedList<AuditLogEntry>, AppError> {
    let limit = filter.limit.unwrap_or(100);
    if !(1..=100_000).contains(&limit) {
        return Err(AppError::bad(
            "LIMIT_OUT_OF_RANGE",
            format!("limit must be in 1..=100000, got {limit}"),
        ));
    }
    let cursor_id = match filter
        .cursor
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(raw) => Some(
            raw.parse::<i64>()
                .map_err(|_| AppError::bad("INVALID_ARGUMENT", "invalid audit cursor"))?,
        ),
        None => None,
    };

    let limit_usize = usize::try_from(limit).unwrap_or(100);
    let fetch_limit = i64::from(limit).saturating_add(1);

    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(
        "SELECT id, tenant_id, user_id, username, action, resource_ref,
                status_code, client_ip, trace_id, detail_json, prev_hash, row_hash, created_at
           FROM sec_audit_logs
          WHERE TRUE",
    );
    push_audit_filters(&mut qb, filter);
    if let Some(cid) = cursor_id {
        qb.push(" AND id < ");
        qb.push_bind(cid);
    }
    qb.push(" ORDER BY id DESC LIMIT ");
    qb.push_bind(fetch_limit);

    let rows = qb
        .build()
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("query sec_audit_logs failed: {err}")))?;
    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit query_audit_logs tx: {err}")))?;

    let mut items = Vec::with_capacity(rows.len().min(limit_usize));
    for r in &rows {
        items.push(decode_audit_entry(r)?);
    }
    let next_cursor = if items.len() > limit_usize {
        items.truncate(limit_usize);
        items.last().map(|e| e.id.to_string())
    } else {
        None
    };

    Ok(PaginatedList { items, next_cursor })
}

pub(super) async fn verify_audit_chain_impl(
    store: &PgStore,
    tenant_id: &str,
    query: &AuditVerifyQuery,
) -> Result<AuditVerifyResponse, AppError> {
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;

    let first_chained_opt: Option<i64> =
        sqlx::query("SELECT MIN(id) AS min_id FROM sec_audit_logs WHERE row_hash <> ''")
            .fetch_one(&mut *tx)
            .await
            .map_err(|err| AppError::Database(format!("find min chained audit id: {err}")))?
            .get("min_id");

    let head_opt = sqlx::query("SELECT last_id FROM sec_audit_chain_heads WHERE tenant_id = $1")
        .bind(tenant_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("read sec_audit_chain_heads: {err}")))?;
    let head_last_id = head_opt.map_or(0_i64, |r| r.get("last_id"));

    let Some(first_chained_id) = first_chained_opt else {
        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit empty audit verify tx: {err}")))?;
        if head_last_id > 0 {
            return Ok(AuditVerifyResponse {
                ok: false,
                checked_count: 0,
                first_broken_id: Some(head_last_id),
            });
        }
        return Ok(AuditVerifyResponse {
            ok: true,
            checked_count: 0,
            first_broken_id: None,
        });
    };

    let effective_from = query
        .from_id
        .map_or(first_chained_id, |f| f.max(first_chained_id));
    let expected_prev: String = if effective_from > first_chained_id {
        sqlx::query(
            "SELECT row_hash FROM sec_audit_logs WHERE id < $1 AND id >= $2 ORDER BY id DESC LIMIT 1",
        )
        .bind(effective_from)
        .bind(first_chained_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("load predecessor audit hash: {err}")))?
        .map_or_else(String::new, |r| r.get("row_hash"))
    } else {
        String::new()
    };

    let rows = sqlx::query(
        "SELECT id, tenant_id, user_id, username, action, resource_ref,
                status_code, client_ip, trace_id, detail_json, prev_hash, row_hash, created_at
           FROM sec_audit_logs
          WHERE id >= $1
            AND ($2::bigint IS NULL OR id <= $2)
          ORDER BY id ASC",
    )
    .bind(effective_from)
    .bind(query.to_id)
    .fetch_all(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("load audit chain rows failed: {err}")))?;

    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit verify_audit_chain tx: {err}")))?;

    let mut entries = Vec::with_capacity(rows.len());
    for r in &rows {
        entries.push(decode_audit_entry(r)?);
    }
    Ok(verify_audit_chain(&expected_prev, &entries))
}

pub(super) async fn query_audit_stats_impl(
    store: &PgStore,
    tenant_id: &str,
    from: Option<DateTime<Utc>>,
    to: Option<DateTime<Utc>>,
) -> Result<AuditStatsResponse, AppError> {
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;

    let action_rows = sqlx::query(
        "SELECT action, COUNT(*) AS cnt
           FROM sec_audit_logs
          WHERE ($1::timestamptz IS NULL OR created_at >= $1)
            AND ($2::timestamptz IS NULL OR created_at <= $2)
          GROUP BY action
          ORDER BY action ASC",
    )
    .bind(from)
    .bind(to)
    .fetch_all(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("audit stats by_action failed: {err}")))?;

    let user_rows = sqlx::query(
        "SELECT username, COUNT(*) AS cnt
           FROM sec_audit_logs
          WHERE ($1::timestamptz IS NULL OR created_at >= $1)
            AND ($2::timestamptz IS NULL OR created_at <= $2)
          GROUP BY username
          ORDER BY cnt DESC, username ASC
          LIMIT 20",
    )
    .bind(from)
    .bind(to)
    .fetch_all(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("audit stats by_user failed: {err}")))?;

    let day_rows = sqlx::query(
        "SELECT to_char(created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD') AS day_str, COUNT(*) AS cnt
           FROM sec_audit_logs
          WHERE ($1::timestamptz IS NULL OR created_at >= $1)
            AND ($2::timestamptz IS NULL OR created_at <= $2)
          GROUP BY day_str
          ORDER BY day_str ASC",
    )
    .bind(from)
    .bind(to)
    .fetch_all(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("audit stats by_day failed: {err}")))?;

    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit query_audit_stats tx: {err}")))?;

    let mut by_action = BTreeMap::new();
    for r in action_rows {
        by_action.insert(r.get("action"), r.get("cnt"));
    }
    let by_user = user_rows
        .into_iter()
        .map(|r| (r.get("username"), r.get("cnt")))
        .collect();
    let by_day = day_rows
        .into_iter()
        .map(|r| (r.get("day_str"), r.get("cnt")))
        .collect();

    Ok(AuditStatsResponse {
        by_action,
        by_user,
        by_day,
    })
}
