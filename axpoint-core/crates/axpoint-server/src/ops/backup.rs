//! Platform-level backup listing, manual backup trigger (`BACKUP_RUN` audit), and `default`-tenant guard (`§14.3`, `§14.8`).

use axpoint_domain::AppError;
use axpoint_domain::axhub::PaginatedList;
use axpoint_domain::enums::{AuditAction, JobKind, Permission};
use axpoint_domain::ops::{BackupKind, BackupRecord, CreateBackupRequest};
use axpoint_domain::platform::{AuditRecord, NewJob};
use axpoint_domain::ports::{JobQueuePort, OpsRepository};
use axpoint_security::AuthPrincipal;
use chrono::Utc;
use uuid::Uuid;

use super::OpsServices;

/// Audit metadata supplied when triggering a platform backup (`POST /api/axops/backups`).
#[derive(Debug, Clone)]
pub struct BackupAuditContext<'a> {
    /// Originating client IP address.
    pub client_ip: &'a str,
    /// Active `W3C` trace identifier.
    pub trace_id: &'a str,
}

/// Enforces that `principal` holds [`Permission::OpsAdmin`] AND belongs to the `"default"` tenant (`§14.1`, `§14.8`).
///
/// # Errors
/// Returns [`AppError::Forbidden`] (`PERMISSION_DENIED`) when `principal` lacks `OPS_ADMIN`
/// or belongs to a non-`"default"` tenant.
pub fn require_platform_backup_admin(principal: &AuthPrincipal) -> Result<(), AppError> {
    principal.require(Permission::OpsAdmin)?;
    if principal.tenant_id != "default" {
        return Err(AppError::Forbidden("PERMISSION_DENIED"));
    }
    Ok(())
}

/// Validates optional backup page `limit` (`1..=200`, defaulting to `50`).
///
/// # Errors
/// Returns [`AppError::BadRequest`] (`LIMIT_OUT_OF_RANGE`) if `limit` is outside `1..=200`.
pub fn validate_backup_limit(limit: Option<u32>) -> Result<u32, AppError> {
    let resolved = limit.unwrap_or(50);
    if !(1..=200).contains(&resolved) {
        return Err(AppError::bad(
            "LIMIT_OUT_OF_RANGE",
            format!("limit must be in 1..=200, got {resolved}"),
        ));
    }
    Ok(resolved)
}

impl<S, P> OpsServices<S, P>
where
    S: OpsRepository + JobQueuePort,
{
    /// Lists platform backups (`GET /api/axops/backups`, `OPS_ADMIN` in `"default"` tenant).
    ///
    /// # Errors
    /// Returns `PERMISSION_DENIED`, `LIMIT_OUT_OF_RANGE`, or database errors.
    pub async fn list_backups(
        &self,
        principal: &AuthPrincipal,
        limit: Option<u32>,
        cursor: Option<&str>,
    ) -> Result<PaginatedList<BackupRecord>, AppError> {
        require_platform_backup_admin(principal)?;
        let effective_limit = validate_backup_limit(limit)?;
        self.store.list_backups(effective_limit, cursor).await
    }

    /// Fetches a single platform backup record (`GET /api/axops/backups/{id}`, `OPS_ADMIN` in `"default"` tenant).
    ///
    /// # Errors
    /// Returns `PERMISSION_DENIED`, `BACKUP_NOT_FOUND`, or database errors.
    pub async fn get_backup(
        &self,
        principal: &AuthPrincipal,
        backup_id: Uuid,
    ) -> Result<BackupRecord, AppError> {
        require_platform_backup_admin(principal)?;
        self.store
            .get_backup(backup_id)
            .await?
            .ok_or(AppError::NotFound("BACKUP_NOT_FOUND"))
    }

    /// Creates a `PENDING` backup row, records `BACKUP_RUN` audit, and optionally enqueues `JobKind::Backup` (`§14.3.1`).
    ///
    /// # Errors
    /// Returns `PERMISSION_DENIED` or database errors.
    pub async fn trigger_backup(
        &self,
        principal: &AuthPrincipal,
        ctx: BackupAuditContext<'_>,
        req: CreateBackupRequest,
    ) -> Result<BackupRecord, AppError> {
        let env_cmd = std::env::var("AXPOINT_BACKUP_CMD").ok();
        self.trigger_backup_with_cmd(principal, ctx, req, env_cmd.as_deref())
            .await
    }

    /// Internal backup trigger accepting an explicit `AXPOINT_BACKUP_CMD` value for deterministic testing.
    ///
    /// # Errors
    /// Returns `PERMISSION_DENIED` or database errors.
    pub async fn trigger_backup_with_cmd(
        &self,
        principal: &AuthPrincipal,
        ctx: BackupAuditContext<'_>,
        req: CreateBackupRequest,
        backup_cmd: Option<&str>,
    ) -> Result<BackupRecord, AppError> {
        require_platform_backup_admin(principal)?;
        let kind = req.backup_kind.unwrap_or(BackupKind::Full);
        let record = self.store.create_backup(kind).await?;

        let actor_id = (!principal.user_id.is_nil()).then_some(principal.user_id);
        self.audit.record(AuditRecord {
            tenant_id: principal.tenant_id.clone(),
            user_id: actor_id,
            username: principal.username.clone(),
            action: AuditAction::BackupRun,
            resource_ref: format!("axops:backup:{}", record.id),
            status_code: 202,
            client_ip: String::from(ctx.client_ip),
            trace_id: String::from(ctx.trace_id),
            detail_json: serde_json::json!({
                "backup_id": record.id,
                "backup_kind": kind,
            }),
            created_at: Utc::now(),
        });
        if let Err(err) = self.audit.flush().await {
            tracing::warn!(error = %err, "backup audit flush failed");
        }

        if backup_cmd.is_some_and(|cmd| !cmd.trim().is_empty()) {
            let job = NewJob::new(
                JobKind::Backup,
                serde_json::json!({
                    "backup_id": record.id,
                    "backup_kind": kind,
                }),
            )
            .with_idempotency_key(format!("backup:{}", record.id));
            let _job_id = self.store.enqueue("default", job).await?;
        }

        Ok(record)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::sync::Arc;

    use axpoint_analytics::{AnalyticsConfig, SessionPool};
    use axpoint_domain::enums::{RetentionTarget, TokenKind};
    use axpoint_domain::ops::{
        BackupStatus, CreateWorkItemRequest, DatabaseHealth, JobsHealthStats, MigrationsHealth,
        NewTemplateInstallInput, OutboxStats, PatchWorkItemRequest, RetentionPolicyRecord,
        TemplateCreatedObject, TemplateInstallRecord, TemplateInstallStatus, WorkItem,
        WorkItemDetail, WorkItemEvent, WorkItemEventKind, WorkItemListQuery,
    };
    use axpoint_domain::platform::{JobListQuery, JobListResponse, JobRecord, LeasedJob};
    use axpoint_domain::ports::AuditSink;
    use axpoint_security::AuditWriter;
    use tokio::sync::Mutex;

    use super::*;
    use crate::ops::OpsConfig;

    #[derive(Clone, Default)]
    struct MockBackupStore {
        backups: Arc<Mutex<Vec<BackupRecord>>>,
        jobs: Arc<Mutex<Vec<(String, NewJob)>>>,
        audits: Arc<Mutex<Vec<AuditRecord>>>,
    }

    impl AuditSink for MockBackupStore {
        async fn write_batch(&self, records: Vec<AuditRecord>) -> Result<(), AppError> {
            let mut guard = self.audits.lock().await;
            guard.extend(records);
            Ok(())
        }
    }

    impl JobQueuePort for MockBackupStore {
        async fn enqueue(&self, tenant_id: &str, job: NewJob) -> Result<Uuid, AppError> {
            let mut guard = self.jobs.lock().await;
            guard.push((String::from(tenant_id), job));
            Ok(Uuid::new_v4())
        }

        fn lease(
            &self,
            _worker: &str,
            _kinds: &[JobKind],
            _limit: u32,
            _lease_secs: u32,
        ) -> impl Future<Output = Result<Vec<LeasedJob>, AppError>> + Send {
            std::future::ready(Ok(Vec::new()))
        }

        fn heartbeat(
            &self,
            _job: &LeasedJob,
            _lease_secs: u32,
        ) -> impl Future<Output = Result<(), AppError>> + Send {
            std::future::ready(Ok(()))
        }

        fn complete(
            &self,
            _job: &LeasedJob,
            _result: serde_json::Value,
        ) -> impl Future<Output = Result<(), AppError>> + Send {
            std::future::ready(Ok(()))
        }

        fn fail(
            &self,
            _job: &LeasedJob,
            _error: &str,
        ) -> impl Future<Output = Result<(), AppError>> + Send {
            std::future::ready(Ok(()))
        }

        fn list_jobs(
            &self,
            _tenant_id: &str,
            _query: JobListQuery,
        ) -> impl Future<Output = Result<JobListResponse, AppError>> + Send {
            std::future::ready(Ok(JobListResponse {
                items: Vec::new(),
                next_cursor: None,
            }))
        }

        fn get_job(
            &self,
            _tenant_id: &str,
            _job_id: Uuid,
        ) -> impl Future<Output = Result<Option<JobRecord>, AppError>> + Send {
            std::future::ready(Ok(None))
        }
    }

    impl OpsRepository for MockBackupStore {
        fn probe_database_health(
            &self,
        ) -> impl Future<Output = Result<DatabaseHealth, AppError>> + Send {
            std::future::ready(Err(AppError::Unsupported("UNIMPLEMENTED_IN_MOCK")))
        }

        fn query_jobs_health_stats(
            &self,
            _tenant_id: &str,
        ) -> impl Future<Output = Result<JobsHealthStats, AppError>> + Send {
            std::future::ready(Ok(JobsHealthStats::default()))
        }

        fn query_outbox_stats(
            &self,
            _tenant_id: &str,
        ) -> impl Future<Output = Result<OutboxStats, AppError>> + Send {
            std::future::ready(Ok(OutboxStats::default()))
        }

        fn query_migrations_health(
            &self,
            _expected_total: usize,
        ) -> impl Future<Output = Result<MigrationsHealth, AppError>> + Send {
            std::future::ready(Ok(MigrationsHealth {
                ok: true,
                current: 14,
                pending: 0,
            }))
        }

        fn list_retention_policies(
            &self,
            _tenant_id: &str,
        ) -> impl Future<Output = Result<Vec<RetentionPolicyRecord>, AppError>> + Send {
            std::future::ready(Ok(Vec::new()))
        }

        fn upsert_retention_policy(
            &self,
            _tenant_id: &str,
            target: RetentionTarget,
            keep_days: i32,
        ) -> impl Future<Output = Result<RetentionPolicyRecord, AppError>> + Send {
            std::future::ready(Ok(RetentionPolicyRecord { target, keep_days }))
        }

        fn drop_expired_metric_partitions(
            &self,
        ) -> impl Future<Output = Result<i64, AppError>> + Send {
            std::future::ready(Ok(0))
        }

        async fn create_backup(&self, backup_kind: BackupKind) -> Result<BackupRecord, AppError> {
            let rec = BackupRecord {
                id: Uuid::new_v4(),
                backup_kind,
                status: BackupStatus::Pending,
                location: String::new(),
                size_bytes: None,
                manifest: serde_json::json!({}),
                error_message: None,
                started_at: Utc::now(),
                finished_at: None,
                verified_at: None,
            };
            let mut guard = self.backups.lock().await;
            guard.push(rec.clone());
            Ok(rec)
        }

        async fn list_backups(
            &self,
            _limit: u32,
            _cursor: Option<&str>,
        ) -> Result<PaginatedList<BackupRecord>, AppError> {
            let guard = self.backups.lock().await;
            Ok(PaginatedList {
                items: guard.clone(),
                next_cursor: None,
            })
        }

        async fn get_backup(&self, backup_id: Uuid) -> Result<Option<BackupRecord>, AppError> {
            let guard = self.backups.lock().await;
            Ok(guard.iter().find(|b| b.id == backup_id).cloned())
        }

        fn create_template_install(
            &self,
            _tenant_id: &str,
            _input: NewTemplateInstallInput,
        ) -> impl Future<Output = Result<TemplateInstallRecord, AppError>> + Send {
            std::future::ready(Err(AppError::Unsupported("UNIMPLEMENTED_IN_MOCK")))
        }

        fn list_template_installs(
            &self,
            _tenant_id: &str,
        ) -> impl Future<Output = Result<Vec<TemplateInstallRecord>, AppError>> + Send {
            std::future::ready(Ok(Vec::new()))
        }

        fn get_template_install(
            &self,
            _tenant_id: &str,
            _install_id: Uuid,
        ) -> impl Future<Output = Result<Option<TemplateInstallRecord>, AppError>> + Send {
            std::future::ready(Ok(None))
        }

        fn update_template_install_progress(
            &self,
            _tenant_id: &str,
            _install_id: Uuid,
            _status: TemplateInstallStatus,
            _object_map: &BTreeMap<String, TemplateCreatedObject>,
            _error_message: Option<&str>,
        ) -> impl Future<Output = Result<TemplateInstallRecord, AppError>> + Send {
            std::future::ready(Err(AppError::Unsupported("UNIMPLEMENTED_IN_MOCK")))
        }

        fn create_work_item(
            &self,
            _tenant_id: &str,
            _created_by: Option<Uuid>,
            _req: CreateWorkItemRequest,
        ) -> impl Future<Output = Result<WorkItem, AppError>> + Send {
            std::future::ready(Err(AppError::Unsupported("UNIMPLEMENTED_IN_MOCK")))
        }

        fn list_work_items(
            &self,
            _tenant_id: &str,
            _query: WorkItemListQuery,
        ) -> impl Future<Output = Result<PaginatedList<WorkItem>, AppError>> + Send {
            std::future::ready(Ok(PaginatedList {
                items: Vec::new(),
                next_cursor: None,
            }))
        }

        fn get_work_item_detail(
            &self,
            _tenant_id: &str,
            _item_id: Uuid,
        ) -> impl Future<Output = Result<Option<WorkItemDetail>, AppError>> + Send {
            std::future::ready(Ok(None))
        }

        fn patch_work_item(
            &self,
            _tenant_id: &str,
            _item_id: Uuid,
            _actor: Option<Uuid>,
            _req: PatchWorkItemRequest,
        ) -> impl Future<Output = Result<Option<WorkItem>, AppError>> + Send {
            std::future::ready(Ok(None))
        }

        fn add_work_item_event(
            &self,
            _tenant_id: &str,
            _item_id: Uuid,
            _event_kind: WorkItemEventKind,
            _actor: Option<Uuid>,
            _detail: serde_json::Value,
        ) -> impl Future<Output = Result<Option<WorkItemEvent>, AppError>> + Send {
            std::future::ready(Ok(None))
        }

        fn link_resolved_alert_work_items(
            &self,
            _tenant_id: &str,
            _alert_id: Uuid,
        ) -> impl Future<Output = Result<u64, AppError>> + Send {
            std::future::ready(Ok(0))
        }
    }

    fn make_principal(tenant_id: &str, perms: BTreeSet<Permission>) -> AuthPrincipal {
        AuthPrincipal {
            user_id: Uuid::new_v4(),
            tenant_id: String::from(tenant_id),
            username: String::from("ops_admin"),
            dept_id: String::from("default"),
            roles: vec![String::from("ADMIN")],
            permissions: perms,
            token_kind: TokenKind::Access,
            token_profile: axpoint_domain::enums::TokenProfile::General,
            web_base_url: None,
        }
    }

    #[tokio::test]
    async fn backup_requires_default_tenant_and_ops_admin() -> Result<(), AppError> {
        let store = MockBackupStore::default();
        let audit = AuditWriter::new(store.clone());
        let ops = OpsServices::new(
            store.clone(),
            Arc::new(()),
            Arc::new(SessionPool::new(AnalyticsConfig::default())),
            audit,
            reqwest::Client::new(),
            OpsConfig {
                parser_base_url: String::from("http://127.0.0.1:8001"),
                data_root: std::env::temp_dir(),
            },
        );

        let non_default = make_principal("tenant_b", BTreeSet::from([Permission::OpsAdmin]));
        let denied_tenant = ops.list_backups(&non_default, None, None).await;
        assert!(matches!(
            denied_tenant,
            Err(AppError::Forbidden("PERMISSION_DENIED"))
        ));

        let no_perm = make_principal("default", BTreeSet::from([Permission::DataRead]));
        let denied_perm = ops.list_backups(&no_perm, None, None).await;
        assert!(matches!(
            denied_perm,
            Err(AppError::Forbidden("PERMISSION_DENIED"))
        ));

        let admin = make_principal("default", BTreeSet::from([Permission::OpsAdmin]));
        let created = ops
            .trigger_backup_with_cmd(
                &admin,
                BackupAuditContext {
                    client_ip: "127.0.0.1",
                    trace_id: "trace-1",
                },
                CreateBackupRequest {
                    backup_kind: Some(BackupKind::Database),
                },
                Some("scripts/ops/backup.sh"),
            )
            .await?;
        assert_eq!(created.backup_kind, BackupKind::Database);
        assert_eq!(created.status, BackupStatus::Pending);

        let jobs = store.jobs.lock().await;
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs.first().map(|(t, _)| t.as_str()), Some("default"));
        assert_eq!(jobs.first().map(|(_, j)| j.kind), Some(JobKind::Backup));
        drop(jobs);

        let audits = store.audits.lock().await;
        assert_eq!(audits.len(), 1);
        assert_eq!(
            audits.first().map(|a| a.action),
            Some(AuditAction::BackupRun)
        );
        drop(audits);

        let fetched = ops.get_backup(&admin, created.id).await?;
        assert_eq!(fetched.id, created.id);
        Ok(())
    }
}
