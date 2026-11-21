//! Integration tests for [`OpsRepository`] on [`PgStore`] (`12-STEP14`).

use std::collections::BTreeMap;

use axpoint_domain::AppError;
use axpoint_domain::enums::RetentionTarget;
use axpoint_domain::ops::{
    BackupKind, BackupStatus, CreateWorkItemRequest, NewTemplateInstallInput, PatchWorkItemRequest,
    TemplateConflictStrategy, TemplateInstallPlan, TemplateInstallStatus, WorkItemEventKind,
    WorkItemListQuery, WorkItemPriority, WorkItemSourceKind, WorkItemStatus,
};
use axpoint_domain::ports::OpsRepository;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

use crate::PgStore;

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
        .max_connections(8)
        .connect(&app_url)
        .await?;
    let owner_pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&owner_url)
        .await?;

    Ok(Some((app_pool, owner_pool)))
}

fn unique_tenant(prefix: &str) -> String {
    let hex_id = Uuid::new_v4().simple().to_string();
    let short: String = hex_id.chars().take(12).collect();
    format!("{prefix}_{short}")
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

async fn cleanup_ops_tenants(
    owner_pool: &PgPool,
    tenants: &[String],
    backup_ids: &[Uuid],
) -> TestResult {
    for sql in [
        "DELETE FROM ax_work_items WHERE tenant_id = ANY($1)",
        "DELETE FROM ax_template_installs WHERE tenant_id = ANY($1)",
        "DELETE FROM ax_retention_policies WHERE tenant_id = ANY($1)",
        "DELETE FROM sec_tenants WHERE tenant_id = ANY($1) AND tenant_id <> 'default'",
    ] {
        sqlx::query(sql).bind(tenants).execute(owner_pool).await?;
    }
    if !backup_ids.is_empty() {
        sqlx::query("DELETE FROM ax_backups WHERE id = ANY($1)")
            .bind(backup_ids)
            .execute(owner_pool)
            .await?;
    }
    Ok(())
}

#[tokio::test]
async fn test_health_retention_and_backups() -> TestResult {
    let Some((app_pool, owner_pool)) = test_pools().await? else {
        return Ok(());
    };
    let tenant = unique_tenant("t_ops_hlth");
    ensure_tenant(&owner_pool, &tenant).await?;

    let store = PgStore::new(app_pool);
    let db_health = store.probe_database_health().await?;
    assert!(db_health.ok);

    let _jobs_stats = store.query_jobs_health_stats(&tenant).await?;
    let _outbox_stats = store.query_outbox_stats(&tenant).await?;
    let _mig_health = store.query_migrations_health(14).await?;

    assert!(matches!(
        store
            .upsert_retention_policy(&tenant, RetentionTarget::Job, 0)
            .await,
        Err(AppError::BadRequest { .. })
    ));
    let upserted = store
        .upsert_retention_policy(&tenant, RetentionTarget::Job, 45)
        .await?;
    assert_eq!(upserted.keep_days, 45);
    let policies = store.list_retention_policies(&tenant).await?;
    assert_eq!(policies.len(), 1);

    let b1 = store.create_backup(BackupKind::Full).await?;
    let b2 = store.create_backup(BackupKind::Database).await?;
    assert_eq!(b1.status, BackupStatus::Pending);
    assert_eq!(store.get_backup(b1.id).await?.map(|b| b.id), Some(b1.id));

    let page = store.list_backups(1, None).await?;
    assert_eq!(page.items.len(), 1);
    assert!(page.next_cursor.is_some());

    cleanup_ops_tenants(&owner_pool, &[tenant], &[b1.id, b2.id]).await?;
    Ok(())
}

#[tokio::test]
async fn test_work_items_lifecycle_and_rls() -> TestResult {
    let Some((app_pool, owner_pool)) = test_pools().await? else {
        return Ok(());
    };
    let t1 = unique_tenant("t_ops_wi1");
    let t2 = unique_tenant("t_ops_wi2");
    ensure_tenant(&owner_pool, &t1).await?;
    ensure_tenant(&owner_pool, &t2).await?;

    let store = PgStore::new(app_pool);
    let alert_id = Uuid::new_v4();
    let wi1 = store
        .create_work_item(
            &t1,
            None,
            CreateWorkItemRequest {
                title: String::from("一号工作项"),
                description: String::from("desc"),
                item_kind: None,
                priority: Some(WorkItemPriority::P1),
                assignee: None,
                source_kind: Some(WorkItemSourceKind::Alert),
                source_ref: alert_id.to_string(),
                node_ids: Vec::new(),
                due_at: None,
            },
        )
        .await?;
    assert_eq!(wi1.item_no, 1);
    assert_eq!(wi1.code, "WI-1");

    let wi_t2 = store
        .create_work_item(
            &t2,
            None,
            CreateWorkItemRequest {
                title: String::from("二租户工作项"),
                description: String::new(),
                item_kind: None,
                priority: None,
                assignee: None,
                source_kind: None,
                source_ref: String::new(),
                node_ids: Vec::new(),
                due_at: None,
            },
        )
        .await?;
    assert_eq!(wi_t2.item_no, 1);
    assert!(store.get_work_item_detail(&t2, wi1.id).await?.is_none());

    let patched = store
        .patch_work_item(
            &t1,
            wi1.id,
            None,
            PatchWorkItemRequest {
                status: Some(WorkItemStatus::Done),
                ..PatchWorkItemRequest::default()
            },
        )
        .await?
        .ok_or("expected patched work item")?;
    assert_eq!(patched.status, WorkItemStatus::Done);
    assert!(patched.closed_at.is_some());

    let _comment = store
        .add_work_item_event(
            &t1,
            wi1.id,
            WorkItemEventKind::Comment,
            None,
            serde_json::json!({"comment": "fixed"}),
        )
        .await?;
    assert_eq!(
        store.link_resolved_alert_work_items(&t1, alert_id).await?,
        1
    );

    let detail = store
        .get_work_item_detail(&t1, wi1.id)
        .await?
        .ok_or("expected detail")?;
    assert_eq!(detail.events.len(), 4);
    assert_eq!(
        store
            .list_work_items(&t1, WorkItemListQuery::default())
            .await?
            .items
            .len(),
        1
    );

    cleanup_ops_tenants(&owner_pool, &[t1, t2], &[]).await?;
    Ok(())
}

#[tokio::test]
async fn test_template_installs_progress_and_uniqueness() -> TestResult {
    let Some((app_pool, owner_pool)) = test_pools().await? else {
        return Ok(());
    };
    let t1 = unique_tenant("t_ops_tpl");
    ensure_tenant(&owner_pool, &t1).await?;

    let store = PgStore::new(app_pool);
    let plan = TemplateInstallPlan {
        pack_id: String::from("retail_store"),
        pack_version: String::from("1.0.0"),
        on_conflict: TemplateConflictStrategy::Rename,
        objects: Vec::new(),
        conflicts: Vec::new(),
    };
    let inst = store
        .create_template_install(
            &t1,
            NewTemplateInstallInput {
                pack_id: String::from("retail_store"),
                pack_version: String::from("1.0.0"),
                plan: plan.clone(),
                installed_by: None,
            },
        )
        .await?;
    let active = store
        .update_template_install_progress(
            &t1,
            inst.id,
            TemplateInstallStatus::Installed,
            &BTreeMap::new(),
            None,
        )
        .await?;
    assert!(active.finished_at.is_some());

    // Second plan for same pack_id cannot transition to INSTALLING while first is INSTALLED.
    let inst2 = store
        .create_template_install(
            &t1,
            NewTemplateInstallInput {
                pack_id: String::from("retail_store"),
                pack_version: String::from("1.0.0"),
                plan,
                installed_by: None,
            },
        )
        .await?;
    assert!(matches!(
        store
            .update_template_install_progress(
                &t1,
                inst2.id,
                TemplateInstallStatus::Installing,
                &BTreeMap::new(),
                None,
            )
            .await,
        Err(AppError::Conflict("CONFLICT"))
    ));

    cleanup_ops_tenants(&owner_pool, &[t1], &[]).await?;
    Ok(())
}
