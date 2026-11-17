//! Alerting rules (`ax_rules`) CRUD, target/condition validation, and `RULE_DISABLED` auto-resolution.

use std::collections::BTreeSet;
use std::str::FromStr;

use axpoint_domain::AppError;
use axpoint_domain::axhub::PaginatedList;
use axpoint_domain::enums::{OutboxTopic, Severity};
use axpoint_domain::expr::parser::parse_str;
use axpoint_domain::platform::NewOutboxEvent;
use axpoint_domain::rules::{
    AlertEventKind, AlertResolveReason, Condition, CreateRuleRequest, ParsedRuleTarget, RuleDef,
    RuleListQuery, RuleScopeKind, UpdateRuleRequest,
};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

use super::alerts::insert_alert_event_tx;
use super::{
    encode_ts_uuid_cursor, is_unique_violation, parse_ts_uuid_cursor, validate_page_limit,
};
use crate::PgStore;
use crate::outbox::insert_outbox_tx;
use crate::tenant::begin_tenant_tx;

fn decode_rule_row(row: &sqlx::postgres::PgRow) -> Result<RuleDef, AppError> {
    let scope_kind = RuleScopeKind::from_str(&row.get::<String, _>("scope_kind"))?;
    let severity = Severity::from_str(&row.get::<String, _>("severity"))?;
    let cond_raw: serde_json::Value = row.get("condition");
    let condition: Condition = serde_json::from_value(cond_raw)
        .map_err(|err| AppError::Database(format!("decode ax_rules.condition failed: {err}")))?;

    Ok(RuleDef {
        id: row.get("id"),
        name: row.get("name"),
        description: row.get("description"),
        scope_kind,
        target: row.get("target"),
        condition,
        severity,
        title_template: row.get("title_template"),
        body_template: row.get("body_template"),
        cooldown_minutes: row.get("cooldown_minutes"),
        resolve_after_minutes: row.get("resolve_after_minutes"),
        escalate_after_minutes: row.get("escalate_after_minutes"),
        channel_ids: row.get("channel_ids"),
        escalation_channel_ids: row.get("escalation_channel_ids"),
        run_flow_id: row.get("run_flow_id"),
        enabled: row.get("enabled"),
        version: row.get("version"),
        created_by: row.get("created_by"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

fn validate_target(scope_kind: RuleScopeKind, target: &serde_json::Value) -> Result<(), AppError> {
    let parsed = ParsedRuleTarget::parse(scope_kind, target)?;
    match parsed {
        ParsedRuleTarget::Metric(m) => {
            if let Some(filter) = &m.node_filter
                && !filter.trim().is_empty()
            {
                parse_str(filter).map_err(|err| {
                    AppError::bad(
                        "INVALID_ARGUMENT",
                        format!("invalid METRIC node_filter expression: {}", err.message),
                    )
                })?;
            }
        }
        ParsedRuleTarget::DatasetQuality(_) => {}
        ParsedRuleTarget::Event(ev) => {
            OutboxTopic::from_str(&ev.topic)?;
            if let Some(filter) = &ev.filter
                && !filter.trim().is_empty()
            {
                parse_str(filter).map_err(|err| {
                    AppError::bad(
                        "INVALID_ARGUMENT",
                        format!("invalid EVENT filter expression: {}", err.message),
                    )
                })?;
            }
        }
    }
    Ok(())
}

fn validate_condition(scope_kind: RuleScopeKind, condition: &Condition) -> Result<(), AppError> {
    match condition {
        Condition::Threshold { value, .. } => {
            if scope_kind != RuleScopeKind::Metric || !value.is_finite() {
                return Err(AppError::bad(
                    "INVALID_ARGUMENT",
                    "THRESHOLD requires METRIC scope and finite value",
                ));
            }
        }
        Condition::ChangeRate {
            window_minutes,
            pct,
            ..
        } => {
            if scope_kind != RuleScopeKind::Metric || *window_minutes == 0 || !pct.is_finite() {
                return Err(AppError::bad(
                    "INVALID_ARGUMENT",
                    "CHANGE_RATE requires METRIC scope, window_minutes >= 1, and finite pct",
                ));
            }
        }
        Condition::Absence { minutes } => {
            if scope_kind != RuleScopeKind::Metric || *minutes == 0 {
                return Err(AppError::bad(
                    "INVALID_ARGUMENT",
                    "ABSENCE requires METRIC scope and minutes >= 1",
                ));
            }
        }
        Condition::AnomalyZscore {
            lookback_points,
            z,
            min_points,
        } => {
            if scope_kind != RuleScopeKind::Metric
                || *lookback_points == 0
                || *min_points == 0
                || *lookback_points < *min_points
                || !z.is_finite()
                || *z <= 0.0
            {
                return Err(AppError::bad(
                    "INVALID_ARGUMENT",
                    "ANOMALY_ZSCORE requires METRIC scope, lookback_points >= min_points >= 1, and z > 0",
                ));
            }
        }
        Condition::Expr { expression } => {
            if scope_kind != RuleScopeKind::Metric || expression.trim().is_empty() {
                return Err(AppError::bad(
                    "INVALID_ARGUMENT",
                    "EXPR requires METRIC scope and non-empty expression",
                ));
            }
            parse_str(expression).map_err(|err| {
                AppError::bad(
                    "INVALID_ARGUMENT",
                    format!("invalid EXPR condition: {}", err.message),
                )
            })?;
        }
        Condition::QualityFailed { .. } => {
            if scope_kind != RuleScopeKind::DatasetQuality {
                return Err(AppError::bad(
                    "INVALID_ARGUMENT",
                    "QUALITY_FAILED condition is only valid for DATASET_QUALITY scope",
                ));
            }
        }
        Condition::EventMatch {} => {
            if scope_kind != RuleScopeKind::Event {
                return Err(AppError::bad(
                    "INVALID_ARGUMENT",
                    "EVENT_MATCH condition is only valid for EVENT scope",
                ));
            }
        }
    }
    Ok(())
}

struct ValidatedRuleTimers<'a> {
    name: &'a str,
    title_template: &'a str,
    cooldown_minutes: i32,
    resolve_after_minutes: i32,
    escalate_after_minutes: Option<i32>,
}

fn validate_rule_scalars(timers: &ValidatedRuleTimers<'_>) -> Result<(), AppError> {
    if timers.name.trim().is_empty() {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "rule name must not be empty",
        ));
    }
    if timers.title_template.trim().is_empty() {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "title_template must not be empty",
        ));
    }
    if timers.cooldown_minutes < 0 {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "cooldown_minutes must be >= 0",
        ));
    }
    if timers.resolve_after_minutes < 0 {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "resolve_after_minutes must be >= 0",
        ));
    }
    if let Some(esc) = timers.escalate_after_minutes
        && esc <= 0
    {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "escalate_after_minutes must be > 0",
        ));
    }
    Ok(())
}

async fn ensure_rule_refs_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    channel_ids: &[Uuid],
    escalation_channel_ids: &[Uuid],
    run_flow_id: Option<Uuid>,
) -> Result<(), AppError> {
    let unique_channels: BTreeSet<Uuid> = channel_ids
        .iter()
        .chain(escalation_channel_ids.iter())
        .copied()
        .collect();
    if !unique_channels.is_empty() {
        let ids_vec: Vec<Uuid> = unique_channels.into_iter().collect();
        let expected_len = i64::try_from(ids_vec.len()).unwrap_or(0);
        let found_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM ax_notify_channels
              WHERE tenant_id = $1 AND id = ANY($2)",
        )
        .bind(tenant_id)
        .bind(&ids_vec)
        .fetch_one(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("check ax_notify_channels failed: {err}")))?;
        if found_count != expected_len {
            return Err(AppError::NotFound("CHANNEL_NOT_FOUND"));
        }
    }

    if let Some(flow_id) = run_flow_id {
        let exists =
            sqlx::query_scalar::<_, i32>("SELECT 1 FROM ax_flows WHERE tenant_id = $1 AND id = $2")
                .bind(tenant_id)
                .bind(flow_id)
                .fetch_optional(&mut **tx)
                .await
                .map_err(|err| AppError::Database(format!("check ax_flows failed: {err}")))?;
        if exists.is_none() {
            return Err(AppError::NotFound("FLOW_NOT_FOUND"));
        }
    }
    Ok(())
}

async fn resolve_rule_disabled_alerts_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    rule_id: Uuid,
) -> Result<(), AppError> {
    let rows = sqlx::query(
        "UPDATE ax_alerts
            SET status = 'RESOLVED',
                resolve_reason = 'RULE_DISABLED',
                resolved_at = NOW(),
                last_false_at = NULL
          WHERE tenant_id = $1
            AND rule_id = $2
            AND status <> 'RESOLVED'
          RETURNING id, severity, node_id, dataset_id",
    )
    .bind(tenant_id)
    .bind(rule_id)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("resolve RULE_DISABLED alerts failed: {err}")))?;

    for row in rows {
        let alert_id: Uuid = row.get("id");
        let severity = Severity::from_str(&row.get::<String, _>("severity"))?;
        let node_id: Option<Uuid> = row.get("node_id");
        let dataset_id: Option<Uuid> = row.get("dataset_id");
        insert_alert_event_tx(
            tx,
            tenant_id,
            alert_id,
            AlertEventKind::Resolved,
            serde_json::json!({ "resolve_reason": AlertResolveReason::RuleDisabled }),
            None,
            None,
        )
        .await?;
        insert_outbox_tx(
            tx,
            tenant_id,
            &NewOutboxEvent {
                topic: OutboxTopic::AlertResolved,
                payload: serde_json::json!({
                    "alert_id": alert_id,
                    "rule_id": rule_id,
                    "severity": severity,
                    "node_id": node_id,
                    "dataset_id": dataset_id,
                    "resolve_reason": AlertResolveReason::RuleDisabled,
                }),
            },
        )
        .await?;
    }
    Ok(())
}

pub(super) async fn list_rules(
    store: &PgStore,
    tenant_id: &str,
    query: RuleListQuery,
) -> Result<PaginatedList<RuleDef>, AppError> {
    let (limit_usize, fetch_limit) = validate_page_limit(query.limit, 50)?;
    let cursor = query
        .cursor
        .as_deref()
        .map(parse_ts_uuid_cursor)
        .transpose()?;
    let cursor_ts = cursor.map(|(ts, _)| ts);
    let cursor_id = cursor.map(|(_, id)| id);
    let search_pattern = query
        .q
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| format!("%{s}%"));

    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let rows = sqlx::query(
        "SELECT * FROM ax_rules
          WHERE tenant_id = $1
            AND ($2::text IS NULL OR scope_kind = $2)
            AND ($3::boolean IS NULL OR enabled = $3)
            AND ($4::text IS NULL OR name ILIKE $4 OR description ILIKE $4)
            AND ($5::timestamptz IS NULL OR (created_at, id) < ($5, $6))
          ORDER BY created_at DESC, id DESC
          LIMIT $7",
    )
    .bind(tenant_id)
    .bind(query.scope_kind.map(RuleScopeKind::as_str))
    .bind(query.enabled)
    .bind(search_pattern)
    .bind(cursor_ts)
    .bind(cursor_id)
    .bind(fetch_limit)
    .fetch_all(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("list ax_rules failed: {err}")))?;
    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit list ax_rules tx failed: {err}")))?;

    let has_more = rows.len() > limit_usize;
    let mut items = Vec::with_capacity(rows.len().min(limit_usize));
    for row in rows.into_iter().take(limit_usize) {
        items.push(decode_rule_row(&row)?);
    }
    let next_cursor = if has_more {
        items
            .last()
            .map(|r| encode_ts_uuid_cursor(r.created_at, r.id))
    } else {
        None
    };
    Ok(PaginatedList { items, next_cursor })
}

pub(super) async fn list_enabled_rules_by_scope(
    store: &PgStore,
    tenant_id: &str,
    scope_kind: RuleScopeKind,
) -> Result<Vec<RuleDef>, AppError> {
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let rows = sqlx::query(
        "SELECT * FROM ax_rules
          WHERE tenant_id = $1 AND scope_kind = $2 AND enabled = TRUE
          ORDER BY created_at ASC, id ASC",
    )
    .bind(tenant_id)
    .bind(scope_kind.as_str())
    .fetch_all(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("list enabled ax_rules failed: {err}")))?;
    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit enabled rules tx failed: {err}")))?;

    rows.iter().map(decode_rule_row).collect()
}

pub(super) async fn get_rule(
    store: &PgStore,
    tenant_id: &str,
    rule_id: Uuid,
) -> Result<Option<RuleDef>, AppError> {
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let row = sqlx::query("SELECT * FROM ax_rules WHERE tenant_id = $1 AND id = $2")
        .bind(tenant_id)
        .bind(rule_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("get ax_rules failed: {err}")))?;
    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit get rule tx failed: {err}")))?;
    row.as_ref().map(decode_rule_row).transpose()
}

pub(super) async fn create_rule(
    store: &PgStore,
    tenant_id: &str,
    created_by: Option<Uuid>,
    req: CreateRuleRequest,
) -> Result<RuleDef, AppError> {
    validate_rule_scalars(&ValidatedRuleTimers {
        name: &req.name,
        title_template: &req.title_template,
        cooldown_minutes: req.cooldown_minutes,
        resolve_after_minutes: req.resolve_after_minutes,
        escalate_after_minutes: req.escalate_after_minutes,
    })?;
    validate_target(req.scope_kind, &req.target)?;
    validate_condition(req.scope_kind, &req.condition)?;
    let cond_json = serde_json::to_value(&req.condition)
        .map_err(|err| AppError::bad("INVALID_ARGUMENT", format!("serialize condition: {err}")))?;

    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    ensure_rule_refs_tx(
        &mut tx,
        tenant_id,
        &req.channel_ids,
        &req.escalation_channel_ids,
        req.run_flow_id,
    )
    .await?;

    let row = sqlx::query(
        "INSERT INTO ax_rules (
             tenant_id, name, description, scope_kind, target, condition, severity,
             title_template, body_template, cooldown_minutes, resolve_after_minutes,
             escalate_after_minutes, channel_ids, escalation_channel_ids, run_flow_id,
             enabled, version, created_by
         ) VALUES (
             $1, $2, $3, $4, $5, $6, $7,
             $8, $9, $10, $11,
             $12, $13, $14, $15,
             $16, 1, $17
         )
         RETURNING *",
    )
    .bind(tenant_id)
    .bind(req.name.trim())
    .bind(&req.description)
    .bind(req.scope_kind.as_str())
    .bind(&req.target)
    .bind(&cond_json)
    .bind(req.severity.as_str())
    .bind(req.title_template.trim())
    .bind(&req.body_template)
    .bind(req.cooldown_minutes)
    .bind(req.resolve_after_minutes)
    .bind(req.escalate_after_minutes)
    .bind(&req.channel_ids)
    .bind(&req.escalation_channel_ids)
    .bind(req.run_flow_id)
    .bind(req.enabled)
    .bind(created_by)
    .fetch_one(&mut *tx)
    .await
    .map_err(|err| {
        if is_unique_violation(&err) {
            AppError::Conflict("RULE_NAME_CONFLICT")
        } else {
            AppError::Database(format!("insert ax_rules failed: {err}"))
        }
    })?;

    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit create_rule tx failed: {err}")))?;
    decode_rule_row(&row)
}

fn merge_and_validate_rule_update(
    mut rule: RuleDef,
    req: UpdateRuleRequest,
) -> Result<(RuleDef, serde_json::Value), AppError> {
    rule.name = req.name.unwrap_or(rule.name);
    rule.description = req.description.unwrap_or(rule.description);
    rule.scope_kind = req.scope_kind.unwrap_or(rule.scope_kind);
    rule.target = req.target.unwrap_or(rule.target);
    rule.condition = req.condition.unwrap_or(rule.condition);
    rule.severity = req.severity.unwrap_or(rule.severity);
    rule.title_template = req.title_template.unwrap_or(rule.title_template);
    rule.body_template = req.body_template.unwrap_or(rule.body_template);
    rule.cooldown_minutes = req.cooldown_minutes.unwrap_or(rule.cooldown_minutes);
    rule.resolve_after_minutes = req
        .resolve_after_minutes
        .unwrap_or(rule.resolve_after_minutes);
    rule.escalate_after_minutes = req
        .escalate_after_minutes
        .apply_to(rule.escalate_after_minutes);
    rule.channel_ids = req.channel_ids.unwrap_or(rule.channel_ids);
    rule.escalation_channel_ids = req
        .escalation_channel_ids
        .unwrap_or(rule.escalation_channel_ids);
    rule.run_flow_id = req.run_flow_id.apply_to(rule.run_flow_id);
    rule.enabled = req.enabled.unwrap_or(rule.enabled);

    validate_rule_scalars(&ValidatedRuleTimers {
        name: &rule.name,
        title_template: &rule.title_template,
        cooldown_minutes: rule.cooldown_minutes,
        resolve_after_minutes: rule.resolve_after_minutes,
        escalate_after_minutes: rule.escalate_after_minutes,
    })?;
    validate_target(rule.scope_kind, &rule.target)?;
    validate_condition(rule.scope_kind, &rule.condition)?;
    let cond_json = serde_json::to_value(&rule.condition)
        .map_err(|err| AppError::bad("INVALID_ARGUMENT", format!("serialize condition: {err}")))?;
    Ok((rule, cond_json))
}

pub(super) async fn update_rule(
    store: &PgStore,
    tenant_id: &str,
    rule_id: Uuid,
    req: UpdateRuleRequest,
) -> Result<Option<RuleDef>, AppError> {
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let existing_row =
        sqlx::query("SELECT * FROM ax_rules WHERE tenant_id = $1 AND id = $2 FOR UPDATE")
            .bind(tenant_id)
            .bind(rule_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|err| AppError::Database(format!("lock ax_rules failed: {err}")))?;

    let Some(existing_row) = existing_row else {
        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit update_rule tx failed: {err}")))?;
        return Ok(None);
    };
    let (merged, cond_json) = merge_and_validate_rule_update(decode_rule_row(&existing_row)?, req)?;

    ensure_rule_refs_tx(
        &mut tx,
        tenant_id,
        &merged.channel_ids,
        &merged.escalation_channel_ids,
        merged.run_flow_id,
    )
    .await?;

    let updated_row = sqlx::query(
        "UPDATE ax_rules
            SET name = $3,
                description = $4,
                scope_kind = $5,
                target = $6,
                condition = $7,
                severity = $8,
                title_template = $9,
                body_template = $10,
                cooldown_minutes = $11,
                resolve_after_minutes = $12,
                escalate_after_minutes = $13,
                channel_ids = $14,
                escalation_channel_ids = $15,
                run_flow_id = $16,
                enabled = $17,
                version = version + 1,
                updated_at = NOW()
          WHERE tenant_id = $1 AND id = $2
          RETURNING *",
    )
    .bind(tenant_id)
    .bind(rule_id)
    .bind(merged.name.trim())
    .bind(&merged.description)
    .bind(merged.scope_kind.as_str())
    .bind(&merged.target)
    .bind(&cond_json)
    .bind(merged.severity.as_str())
    .bind(merged.title_template.trim())
    .bind(&merged.body_template)
    .bind(merged.cooldown_minutes)
    .bind(merged.resolve_after_minutes)
    .bind(merged.escalate_after_minutes)
    .bind(&merged.channel_ids)
    .bind(&merged.escalation_channel_ids)
    .bind(merged.run_flow_id)
    .bind(merged.enabled)
    .fetch_one(&mut *tx)
    .await
    .map_err(|err| {
        if is_unique_violation(&err) {
            AppError::Conflict("RULE_NAME_CONFLICT")
        } else {
            AppError::Database(format!("update ax_rules failed: {err}"))
        }
    })?;

    if !merged.enabled {
        resolve_rule_disabled_alerts_tx(&mut tx, tenant_id, rule_id).await?;
    }

    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit update_rule tx failed: {err}")))?;
    Ok(Some(decode_rule_row(&updated_row)?))
}

pub(super) async fn delete_rule(
    store: &PgStore,
    tenant_id: &str,
    rule_id: Uuid,
) -> Result<bool, AppError> {
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let res = sqlx::query("DELETE FROM ax_rules WHERE tenant_id = $1 AND id = $2")
        .bind(tenant_id)
        .bind(rule_id)
        .execute(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("delete ax_rules failed: {err}")))?;
    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit delete_rule tx failed: {err}")))?;
    Ok(res.rows_affected() > 0)
}
