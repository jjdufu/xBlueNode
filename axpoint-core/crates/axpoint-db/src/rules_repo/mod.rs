//! Operational alerting rules, deduplicated alerts, timeline events, notification channels,
//! delivery retries, and silences persistence (`07-STEP07-08` §08).

mod alert_actions;
mod alerts;
mod channels;
mod rules;
mod silences;

use std::str::FromStr;

use axpoint_domain::AppError;
use axpoint_domain::axhub::{
    ColumnProfile, Dataset, DatasetStatus, PaginatedList, QualityReport, SchemaPolicy,
    SealedCredential,
};
use axpoint_domain::enums::{SourceKind, SyncMode};
use axpoint_domain::ports::RulesRepository;
use axpoint_domain::rules::{
    AbsenceRuleItem, AckBatchResponse, AlertEventKind, AlertEventRecord, AlertEventsQuery,
    AlertListQuery, AlertRecord, AlertSilenceRecord, CreateRuleRequest, CreateSilenceRequest,
    DeliveryFailureUpdate, DueDeliveryItem, EnqueueDeliveryInput, FalseConditionOutcome,
    NotifyChannelRecord, NotifyDeliveryRecord, RuleDef, RuleListQuery, RuleScopeKind,
    SaveNotifyChannelInput, SimplePageQuery, TrueConditionInput, TrueConditionOutcome,
    UpdateRuleRequest,
};
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

use crate::PgStore;
use crate::tenant::begin_tenant_tx;

pub(super) fn is_unique_violation(err: &sqlx::Error) -> bool {
    err.as_database_error()
        .and_then(sqlx::error::DatabaseError::code)
        .is_some_and(|code| code == "23505")
}

pub(super) fn validate_page_limit(
    raw_limit: Option<u32>,
    default_limit: u32,
) -> Result<(usize, i64), AppError> {
    let limit = raw_limit.unwrap_or(default_limit);
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

pub(super) fn encode_ts_uuid_cursor(ts: DateTime<Utc>, id: Uuid) -> String {
    format!("{}|{id}", ts.to_rfc3339())
}

pub(super) fn parse_ts_uuid_cursor(raw_cursor: &str) -> Result<(DateTime<Utc>, Uuid), AppError> {
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

pub(super) fn parse_event_id_cursor(raw_cursor: &str) -> Result<i64, AppError> {
    let id_part = raw_cursor
        .split_once('|')
        .map_or(raw_cursor, |(_, suffix)| suffix);
    id_part
        .parse::<i64>()
        .map_err(|err| AppError::bad("INVALID_ARGUMENT", format!("invalid event cursor: {err}")))
}

fn decode_dataset_row(row: &sqlx::postgres::PgRow) -> Result<Dataset, AppError> {
    let source_kind = SourceKind::from_str(&row.get::<String, _>("source_kind"))?;
    let sync_mode = SyncMode::from_str(&row.get::<String, _>("sync_mode"))?;
    let schema_policy = match row.get::<String, _>("schema_policy").as_str() {
        "STRICT" => SchemaPolicy::Strict,
        "ADDITIVE" => SchemaPolicy::Additive,
        "EVOLVE" => SchemaPolicy::Evolve,
        other => {
            return Err(AppError::bad(
                "INVALID_ENUM",
                format!("unknown SchemaPolicy: {other}"),
            ));
        }
    };
    let status = match row.get::<String, _>("status").as_str() {
        "DRAFT" => DatasetStatus::Draft,
        "READY" => DatasetStatus::Ready,
        "SYNCING" => DatasetStatus::Syncing,
        "FAILED" => DatasetStatus::Failed,
        "DISABLED" => DatasetStatus::Disabled,
        other => {
            return Err(AppError::bad(
                "INVALID_ENUM",
                format!("unknown DatasetStatus: {other}"),
            ));
        }
    };
    let schema_val: serde_json::Value = row.get("schema_json");
    let schema_json: Vec<ColumnProfile> = serde_json::from_value(schema_val).unwrap_or_default();

    Ok(Dataset {
        id: row.get("id"),
        connection_id: row.get("connection_id"),
        name: row.get("name"),
        display_name: row.get("display_name"),
        description: row.get("description"),
        source_kind,
        source_ref: row.get("source_ref"),
        sync_mode,
        primary_key: row.get("primary_key"),
        cursor_column: row.get("cursor_column"),
        cursor_value: row.get("cursor_value"),
        delete_column: row.get("delete_column"),
        schema_policy,
        lake_schema: row.get("lake_schema"),
        lake_table: row.get("lake_table"),
        schema_json,
        schema_version: row.get("schema_version"),
        row_count: row.get("row_count"),
        last_snapshot_id: row.get("last_snapshot_id"),
        last_synced_at: row.get("last_synced_at"),
        schedule_cron: row.get("schedule_cron"),
        continuous_stream: row.try_get("continuous_stream").unwrap_or(false),
        cdc_slot_meta: row
            .try_get("cdc_slot_meta")
            .unwrap_or_else(|_| serde_json::json!({})),
        tags: row.get("tags"),
        owner_user_id: row.get("owner_user_id"),
        status,
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

impl RulesRepository for PgStore {
    async fn list_rules(
        &self,
        tenant_id: &str,
        query: RuleListQuery,
    ) -> Result<PaginatedList<RuleDef>, AppError> {
        rules::list_rules(self, tenant_id, query).await
    }

    async fn list_enabled_rules_by_scope(
        &self,
        tenant_id: &str,
        scope_kind: RuleScopeKind,
    ) -> Result<Vec<RuleDef>, AppError> {
        rules::list_enabled_rules_by_scope(self, tenant_id, scope_kind).await
    }

    async fn get_rule(&self, tenant_id: &str, rule_id: Uuid) -> Result<Option<RuleDef>, AppError> {
        rules::get_rule(self, tenant_id, rule_id).await
    }

    async fn create_rule(
        &self,
        tenant_id: &str,
        created_by: Option<Uuid>,
        req: CreateRuleRequest,
    ) -> Result<RuleDef, AppError> {
        rules::create_rule(self, tenant_id, created_by, req).await
    }

    async fn update_rule(
        &self,
        tenant_id: &str,
        rule_id: Uuid,
        req: UpdateRuleRequest,
    ) -> Result<Option<RuleDef>, AppError> {
        rules::update_rule(self, tenant_id, rule_id, req).await
    }

    async fn delete_rule(&self, tenant_id: &str, rule_id: Uuid) -> Result<bool, AppError> {
        rules::delete_rule(self, tenant_id, rule_id).await
    }

    async fn list_alerts(
        &self,
        tenant_id: &str,
        query: AlertListQuery,
    ) -> Result<PaginatedList<AlertRecord>, AppError> {
        alerts::list_alerts(self, tenant_id, query).await
    }

    async fn get_alert(
        &self,
        tenant_id: &str,
        alert_id: Uuid,
    ) -> Result<Option<AlertRecord>, AppError> {
        alerts::get_alert(self, tenant_id, alert_id).await
    }

    async fn list_active_alerts_for_rule(
        &self,
        tenant_id: &str,
        rule_id: Uuid,
    ) -> Result<Vec<AlertRecord>, AppError> {
        alerts::list_active_alerts_for_rule(self, tenant_id, rule_id).await
    }

    async fn list_alert_events(
        &self,
        tenant_id: &str,
        alert_id: Uuid,
        query: AlertEventsQuery,
    ) -> Result<PaginatedList<AlertEventRecord>, AppError> {
        alerts::list_alert_events(self, tenant_id, alert_id, query).await
    }

    async fn append_alert_event(
        &self,
        tenant_id: &str,
        alert_id: Uuid,
        event: AlertEventKind,
        detail: serde_json::Value,
        actor: Option<Uuid>,
    ) -> Result<AlertEventRecord, AppError> {
        alerts::append_alert_event(self, tenant_id, alert_id, event, detail, actor).await
    }

    async fn set_alert_flow_run(
        &self,
        tenant_id: &str,
        alert_id: Uuid,
        flow_id: Uuid,
        run_id: Uuid,
    ) -> Result<(), AppError> {
        alerts::set_alert_flow_run(self, tenant_id, alert_id, flow_id, run_id).await
    }

    async fn record_true_condition(
        &self,
        tenant_id: &str,
        input: TrueConditionInput,
    ) -> Result<TrueConditionOutcome, AppError> {
        alerts::record_true_condition(self, tenant_id, input).await
    }

    async fn record_false_condition(
        &self,
        tenant_id: &str,
        dedup_key: &str,
        resolve_after_minutes: i32,
        now: DateTime<Utc>,
    ) -> Result<FalseConditionOutcome, AppError> {
        alerts::record_false_condition(self, tenant_id, dedup_key, resolve_after_minutes, now).await
    }

    async fn escalate_due_alerts(
        &self,
        tenant_id: &str,
        rule_id: Uuid,
        escalate_after_minutes: i32,
        now: DateTime<Utc>,
    ) -> Result<Vec<AlertRecord>, AppError> {
        alert_actions::escalate_due_alerts(self, tenant_id, rule_id, escalate_after_minutes, now)
            .await
    }

    async fn ack_alert(
        &self,
        tenant_id: &str,
        alert_id: Uuid,
        actor: Option<Uuid>,
        comment: &str,
    ) -> Result<AlertRecord, AppError> {
        alert_actions::ack_alert(self, tenant_id, alert_id, actor, comment).await
    }

    async fn ack_alerts_batch(
        &self,
        tenant_id: &str,
        alert_ids: &[Uuid],
        actor: Option<Uuid>,
        comment: &str,
    ) -> Result<AckBatchResponse, AppError> {
        alert_actions::ack_alerts_batch(self, tenant_id, alert_ids, actor, comment).await
    }

    async fn resolve_alert_manual(
        &self,
        tenant_id: &str,
        alert_id: Uuid,
        actor: Option<Uuid>,
        comment: &str,
    ) -> Result<AlertRecord, AppError> {
        alert_actions::resolve_alert_manual(self, tenant_id, alert_id, actor, comment).await
    }

    async fn list_channels(
        &self,
        tenant_id: &str,
        query: SimplePageQuery,
    ) -> Result<PaginatedList<NotifyChannelRecord>, AppError> {
        channels::list_channels(self, tenant_id, query).await
    }

    async fn get_channel(
        &self,
        tenant_id: &str,
        channel_id: Uuid,
    ) -> Result<Option<(NotifyChannelRecord, Option<SealedCredential>)>, AppError> {
        channels::get_channel(self, tenant_id, channel_id).await
    }

    async fn create_channel(
        &self,
        tenant_id: &str,
        input: SaveNotifyChannelInput,
    ) -> Result<NotifyChannelRecord, AppError> {
        channels::create_channel(self, tenant_id, input).await
    }

    async fn update_channel(
        &self,
        tenant_id: &str,
        channel_id: Uuid,
        input: SaveNotifyChannelInput,
    ) -> Result<Option<NotifyChannelRecord>, AppError> {
        channels::update_channel(self, tenant_id, channel_id, input).await
    }

    async fn delete_channel(&self, tenant_id: &str, channel_id: Uuid) -> Result<bool, AppError> {
        channels::delete_channel(self, tenant_id, channel_id).await
    }

    async fn enqueue_delivery(
        &self,
        tenant_id: &str,
        input: EnqueueDeliveryInput,
    ) -> Result<(NotifyDeliveryRecord, bool), AppError> {
        channels::enqueue_delivery(self, tenant_id, input).await
    }

    async fn get_delivery(
        &self,
        tenant_id: &str,
        delivery_id: Uuid,
    ) -> Result<Option<NotifyDeliveryRecord>, AppError> {
        channels::get_delivery(self, tenant_id, delivery_id).await
    }

    async fn mark_delivery_sent(
        &self,
        tenant_id: &str,
        delivery_id: Uuid,
        attempt: i32,
        response_code: Option<i32>,
    ) -> Result<NotifyDeliveryRecord, AppError> {
        channels::mark_delivery_sent(self, tenant_id, delivery_id, attempt, response_code).await
    }

    async fn mark_delivery_failed(
        &self,
        tenant_id: &str,
        delivery_id: Uuid,
        update: DeliveryFailureUpdate,
    ) -> Result<NotifyDeliveryRecord, AppError> {
        channels::mark_delivery_failed(self, tenant_id, delivery_id, update).await
    }

    async fn list_silences(
        &self,
        tenant_id: &str,
        query: SimplePageQuery,
    ) -> Result<PaginatedList<AlertSilenceRecord>, AppError> {
        silences::list_silences(self, tenant_id, query).await
    }

    async fn list_active_silences(
        &self,
        tenant_id: &str,
        at: DateTime<Utc>,
    ) -> Result<Vec<AlertSilenceRecord>, AppError> {
        silences::list_active_silences(self, tenant_id, at).await
    }

    async fn create_silence(
        &self,
        tenant_id: &str,
        created_by: Option<Uuid>,
        req: CreateSilenceRequest,
    ) -> Result<AlertSilenceRecord, AppError> {
        silences::create_silence(self, tenant_id, created_by, req).await
    }

    async fn delete_silence(&self, tenant_id: &str, silence_id: Uuid) -> Result<bool, AppError> {
        silences::delete_silence(self, tenant_id, silence_id).await
    }

    async fn query_raw_metric_points(
        &self,
        tenant_id: &str,
        node_id: Uuid,
        metric: &str,
        since: Option<DateTime<Utc>>,
        limit: u32,
    ) -> Result<Vec<(DateTime<Utc>, f64)>, AppError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let rows = sqlx::query(
            "SELECT ts, value
               FROM xb_metric_series
              WHERE tenant_id = $1
                AND node_id = $2
                AND metric = $3
                AND value IS NOT NULL
                AND ($4::timestamptz IS NULL OR ts >= $4)
              ORDER BY ts DESC
              LIMIT $5",
        )
        .bind(tenant_id)
        .bind(node_id)
        .bind(metric)
        .bind(since)
        .bind(i64::from(limit))
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("query_raw_metric_points failed: {err}")))?;
        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit metric query tx failed: {err}")))?;

        Ok(rows
            .into_iter()
            .map(|row| (row.get("ts"), row.get("value")))
            .collect())
    }

    async fn get_latest_dataset_quality(
        &self,
        tenant_id: &str,
        dataset_id: Uuid,
    ) -> Result<Option<(Dataset, Option<QualityReport>)>, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let ds_row = sqlx::query(
            "SELECT * FROM ds_datasets
              WHERE tenant_id = $1 AND id = $2",
        )
        .bind(tenant_id)
        .bind(dataset_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("load ds_datasets failed: {err}")))?;

        let Some(ds_row) = ds_row else {
            tx.commit().await.map_err(|err| {
                AppError::Database(format!("commit dataset quality tx failed: {err}"))
            })?;
            return Ok(None);
        };
        let dataset = decode_dataset_row(&ds_row)?;

        let job_row = sqlx::query(
            "SELECT quality_result
               FROM ds_sync_jobs
              WHERE tenant_id = $1
                AND dataset_id = $2
                AND status = 'SUCCEEDED'
              ORDER BY finished_at DESC NULLS LAST, created_at DESC, id DESC
              LIMIT 1",
        )
        .bind(tenant_id)
        .bind(dataset_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("load latest ds_sync_jobs failed: {err}")))?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit dataset quality tx failed: {err}"))
        })?;

        let quality = match job_row {
            Some(row) => {
                let raw: serde_json::Value = row.get("quality_result");
                if raw.is_null() {
                    None
                } else {
                    Some(serde_json::from_value::<QualityReport>(raw).map_err(|err| {
                        AppError::Database(format!("decode quality_result failed: {err}"))
                    })?)
                }
            }
            None => None,
        };

        Ok(Some((dataset, quality)))
    }

    async fn list_absence_rules(&self) -> Result<Vec<AbsenceRuleItem>, AppError> {
        let rows = sqlx::query("SELECT tenant_id, rule_id FROM ax_absence_rules()")
            .fetch_all(self.pool())
            .await
            .map_err(|err| AppError::Database(format!("ax_absence_rules failed: {err}")))?;
        Ok(rows
            .into_iter()
            .map(|row| AbsenceRuleItem {
                tenant_id: row.get("tenant_id"),
                rule_id: row.get("rule_id"),
            })
            .collect())
    }

    async fn list_due_deliveries(&self, limit: i32) -> Result<Vec<DueDeliveryItem>, AppError> {
        let rows = sqlx::query("SELECT tenant_id, delivery_id FROM ax_due_notify_deliveries($1)")
            .bind(limit)
            .fetch_all(self.pool())
            .await
            .map_err(|err| AppError::Database(format!("ax_due_notify_deliveries failed: {err}")))?;
        Ok(rows
            .into_iter()
            .map(|row| DueDeliveryItem {
                tenant_id: row.get("tenant_id"),
                delivery_id: row.get("delivery_id"),
            })
            .collect())
    }
}
