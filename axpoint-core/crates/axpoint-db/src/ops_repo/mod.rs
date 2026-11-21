//! `AxOps` deep health, retention policy, platform backup, template installation,
//! and delivery work-item persistence (`OpsRepository` on [`PgStore`], `12-STEP14`).

mod backups_templates;
mod health_retention;
mod work_items;

#[cfg(test)]
mod tests;

use std::collections::BTreeMap;

use axpoint_domain::AppError;
use axpoint_domain::axhub::PaginatedList;
use axpoint_domain::enums::RetentionTarget;
use axpoint_domain::ops::{
    BackupKind, BackupRecord, CreateWorkItemRequest, DatabaseHealth, JobsHealthStats,
    MigrationsHealth, NewTemplateInstallInput, OutboxStats, PatchWorkItemRequest,
    RetentionPolicyRecord, TemplateCreatedObject, TemplateInstallRecord, TemplateInstallStatus,
    WorkItem, WorkItemDetail, WorkItemEvent, WorkItemEventKind, WorkItemListQuery,
};
use axpoint_domain::ports::OpsRepository;
use uuid::Uuid;

use crate::PgStore;

pub(super) fn is_sqlstate(err: &sqlx::Error, expected: &str) -> bool {
    err.as_database_error()
        .and_then(sqlx::error::DatabaseError::code)
        .as_deref()
        == Some(expected)
}

pub(super) fn validate_page_limit(raw_limit: Option<u32>) -> Result<(usize, i64), AppError> {
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

impl OpsRepository for PgStore {
    async fn probe_database_health(&self) -> Result<DatabaseHealth, AppError> {
        health_retention::probe_database_health_impl(self).await
    }

    async fn query_jobs_health_stats(&self, tenant_id: &str) -> Result<JobsHealthStats, AppError> {
        health_retention::query_jobs_health_stats_impl(self, tenant_id).await
    }

    async fn query_outbox_stats(&self, tenant_id: &str) -> Result<OutboxStats, AppError> {
        health_retention::query_outbox_stats_impl(self, tenant_id).await
    }

    async fn query_migrations_health(
        &self,
        expected_total: usize,
    ) -> Result<MigrationsHealth, AppError> {
        health_retention::query_migrations_health_impl(self, expected_total).await
    }

    async fn list_retention_policies(
        &self,
        tenant_id: &str,
    ) -> Result<Vec<RetentionPolicyRecord>, AppError> {
        health_retention::list_retention_policies_impl(self, tenant_id).await
    }

    async fn upsert_retention_policy(
        &self,
        tenant_id: &str,
        target: RetentionTarget,
        keep_days: i32,
    ) -> Result<RetentionPolicyRecord, AppError> {
        health_retention::upsert_retention_policy_impl(self, tenant_id, target, keep_days).await
    }

    async fn drop_expired_metric_partitions(&self) -> Result<i64, AppError> {
        health_retention::drop_expired_metric_partitions_impl(self).await
    }

    async fn create_backup(&self, backup_kind: BackupKind) -> Result<BackupRecord, AppError> {
        backups_templates::create_backup_impl(self, backup_kind).await
    }

    async fn list_backups(
        &self,
        limit: u32,
        cursor: Option<&str>,
    ) -> Result<PaginatedList<BackupRecord>, AppError> {
        backups_templates::list_backups_impl(self, limit, cursor).await
    }

    async fn get_backup(&self, backup_id: Uuid) -> Result<Option<BackupRecord>, AppError> {
        backups_templates::get_backup_impl(self, backup_id).await
    }

    async fn create_template_install(
        &self,
        tenant_id: &str,
        input: NewTemplateInstallInput,
    ) -> Result<TemplateInstallRecord, AppError> {
        backups_templates::create_template_install_impl(self, tenant_id, &input).await
    }

    async fn list_template_installs(
        &self,
        tenant_id: &str,
    ) -> Result<Vec<TemplateInstallRecord>, AppError> {
        backups_templates::list_template_installs_impl(self, tenant_id).await
    }

    async fn get_template_install(
        &self,
        tenant_id: &str,
        install_id: Uuid,
    ) -> Result<Option<TemplateInstallRecord>, AppError> {
        backups_templates::get_template_install_impl(self, tenant_id, install_id).await
    }

    async fn update_template_install_progress(
        &self,
        tenant_id: &str,
        install_id: Uuid,
        status: TemplateInstallStatus,
        object_map: &BTreeMap<String, TemplateCreatedObject>,
        error_message: Option<&str>,
    ) -> Result<TemplateInstallRecord, AppError> {
        backups_templates::update_template_install_progress_impl(
            self,
            tenant_id,
            install_id,
            status,
            object_map,
            error_message,
        )
        .await
    }

    async fn create_work_item(
        &self,
        tenant_id: &str,
        created_by: Option<Uuid>,
        req: CreateWorkItemRequest,
    ) -> Result<WorkItem, AppError> {
        work_items::create_work_item_impl(self, tenant_id, created_by, &req).await
    }

    async fn list_work_items(
        &self,
        tenant_id: &str,
        query: WorkItemListQuery,
    ) -> Result<PaginatedList<WorkItem>, AppError> {
        work_items::list_work_items_impl(self, tenant_id, &query).await
    }

    async fn get_work_item_detail(
        &self,
        tenant_id: &str,
        item_id: Uuid,
    ) -> Result<Option<WorkItemDetail>, AppError> {
        work_items::get_work_item_detail_impl(self, tenant_id, item_id).await
    }

    async fn patch_work_item(
        &self,
        tenant_id: &str,
        item_id: Uuid,
        actor: Option<Uuid>,
        req: PatchWorkItemRequest,
    ) -> Result<Option<WorkItem>, AppError> {
        work_items::patch_work_item_impl(self, tenant_id, item_id, actor, &req).await
    }

    async fn add_work_item_event(
        &self,
        tenant_id: &str,
        item_id: Uuid,
        event_kind: WorkItemEventKind,
        actor: Option<Uuid>,
        detail: serde_json::Value,
    ) -> Result<Option<WorkItemEvent>, AppError> {
        work_items::add_work_item_event_impl(self, tenant_id, item_id, event_kind, actor, &detail)
            .await
    }

    async fn link_resolved_alert_work_items(
        &self,
        tenant_id: &str,
        alert_id: Uuid,
    ) -> Result<u64, AppError> {
        work_items::link_resolved_alert_work_items_impl(self, tenant_id, alert_id).await
    }
}
