//! Step 00 PostgreSQL integration tests for RLS tenant isolation, `ax_jobs`, `ax_outbox`, `sec_audit_logs`, and `sec_api_tokens`.

use std::collections::{BTreeMap, BTreeSet};

use axpoint_domain::AppError;
use axpoint_domain::enums::{AuditAction, JobKind, JobStatus, OutboxTopic, Permission};
use axpoint_domain::platform::{AuditRecord, JobListQuery, NewJob, NewOutboxEvent};
use axpoint_domain::ports::{AuditSink, CredentialStore, JobQueuePort, OutboxPort};
use axpoint_domain::topology::{BlueNodeTypeInput, SaveXBlueSchemaRequest, UpsertBlueNodeRequest};
use chrono::Utc;
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::PgStore;
use crate::outbox::insert_outbox_tx;
use crate::tenant::begin_tenant_tx;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Connects `(app_pool, owner_pool)` from `DATABASE_URL` and `MIGRATION_DATABASE_URL`.
///
/// Returns `Ok(None)` if `DATABASE_URL` is unset and `AXPOINT_REQUIRE_DB_TESTS` is not truthy.
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

#[tokio::test]
async fn rls_blocks_without_tenant() -> TestResult {
    let Some((app_pool, owner_pool)) = test_pools().await? else {
        return Ok(());
    };

    let tenant = unique_tenant("t_rls_blk");
    ensure_tenant(&owner_pool, &tenant).await?;

    let store = PgStore::new(app_pool.clone());
    store
        .save_xblue_schema(
            &tenant,
            SaveXBlueSchemaRequest {
                node_types: vec![BlueNodeTypeInput {
                    name: String::from("Warehouse"),
                    display_name: String::from("仓库"),
                    properties: Vec::new(),
                    agg_rules: Vec::new(),
                    pos_x: 0.0,
                    pos_y: 0.0,
                }],
                edge_types: Vec::new(),
            },
        )
        .await?;
    store
        .upsert_blue_node(
            &tenant,
            UpsertBlueNodeRequest {
                type_name: String::from("Warehouse"),
                node_key: String::from("W-RLS-1"),
                title: String::from("RLS Test Warehouse"),
                metrics: BTreeMap::from([(String::from("inventory"), 100.0)]),
                ..UpsertBlueNodeRequest::default()
            },
        )
        .await?;

    // Query directly as axpoint_app without setting app.tenant_id: RLS must hide all rows.
    let row = sqlx::query("SELECT COUNT(*) AS cnt FROM xb_nodes")
        .fetch_one(&app_pool)
        .await?;
    let count: i64 = row.get("cnt");
    assert_eq!(count, 0);

    for sql in [
        "DELETE FROM xb_nodes WHERE tenant_id = $1",
        "DELETE FROM xb_node_types WHERE tenant_id = $1",
        "DELETE FROM ax_jobs WHERE tenant_id = $1",
        "DELETE FROM ax_outbox WHERE tenant_id = $1",
    ] {
        sqlx::query(sql).bind(&tenant).execute(&owner_pool).await?;
    }

    Ok(())
}

#[tokio::test]
async fn rls_isolates_tenants() -> TestResult {
    let Some((app_pool, owner_pool)) = test_pools().await? else {
        return Ok(());
    };

    let t1 = unique_tenant("t1");
    let t2 = unique_tenant("t2");
    ensure_tenant(&owner_pool, &t1).await?;
    ensure_tenant(&owner_pool, &t2).await?;

    let store = PgStore::new(app_pool.clone());
    store
        .save_xblue_schema(
            &t1,
            SaveXBlueSchemaRequest {
                node_types: vec![BlueNodeTypeInput {
                    name: String::from("Warehouse"),
                    display_name: String::from("仓库"),
                    properties: Vec::new(),
                    agg_rules: Vec::new(),
                    pos_x: 0.0,
                    pos_y: 0.0,
                }],
                edge_types: Vec::new(),
            },
        )
        .await?;
    let created = store
        .upsert_blue_node(
            &t1,
            UpsertBlueNodeRequest {
                type_name: String::from("Warehouse"),
                node_key: String::from("W-T1"),
                title: String::from("Tenant 1 Node"),
                ..UpsertBlueNodeRequest::default()
            },
        )
        .await?;

    let snap_t1 = store.get_runtime_snapshot(&t1).await?;
    assert_eq!(snap_t1.nodes.len(), 1);
    assert_eq!(snap_t1.nodes.first().map(|n| n.id), Some(created.id));

    // Tenant t2 cannot observe t1's node.
    let snap_t2 = store.get_runtime_snapshot(&t2).await?;
    assert!(snap_t2.nodes.is_empty());

    // WITH CHECK policy must reject writing a t1 row inside a t2 transaction.
    let mut tx_t2 = begin_tenant_tx(&app_pool, &t2).await?;
    let cross_write = sqlx::query(
        "INSERT INTO xb_nodes (tenant_id, type_name, node_key, title)
         VALUES ($1, 'Warehouse', 'W-SPOOF', 'Spoofed Cross-Tenant Node')",
    )
    .bind(&t1)
    .execute(&mut *tx_t2)
    .await;
    assert!(
        cross_write.is_err(),
        "RLS WITH CHECK must reject cross-tenant INSERT"
    );
    drop(tx_t2);

    let tenants = vec![t1, t2];
    for sql in [
        "DELETE FROM xb_nodes WHERE tenant_id = ANY($1)",
        "DELETE FROM xb_node_types WHERE tenant_id = ANY($1)",
        "DELETE FROM ax_jobs WHERE tenant_id = ANY($1)",
        "DELETE FROM ax_outbox WHERE tenant_id = ANY($1)",
    ] {
        sqlx::query(sql).bind(&tenants).execute(&owner_pool).await?;
    }

    Ok(())
}

#[tokio::test]
async fn jobs_lease_is_exclusive() -> TestResult {
    let Some((app_pool, owner_pool)) = test_pools().await? else {
        return Ok(());
    };

    let tenant = unique_tenant("t_jobs_excl");
    ensure_tenant(&owner_pool, &tenant).await?;

    let store = PgStore::new(app_pool);
    let future_run = Utc::now() + chrono::Duration::hours(1);

    // Enqueue 110 jobs in the future with top priority (-1_000_000) so even if a live
    // background server worker leases a small batch, both 50-item test workers get full batches.
    let mut idem_job = NewJob::new(JobKind::LakeMaintenance, serde_json::json!({"seq": 0}))
        .with_idempotency_key("idem-0");
    idem_job.priority = -1_000_000;
    idem_job.next_run_at = Some(future_run);
    let first_id = store.enqueue(&tenant, idem_job.clone()).await?;
    let dup_id = store.enqueue(&tenant, idem_job).await?;
    assert_eq!(first_id, dup_id);

    for seq in 1..110_i32 {
        let mut job = NewJob::new(JobKind::LakeMaintenance, serde_json::json!({"seq": seq}));
        job.priority = -1_000_000;
        job.next_run_at = Some(future_run);
        store.enqueue(&tenant, job).await?;
    }

    // Atomically mark all 100 jobs ready right before the concurrent lease race.
    sqlx::query(
        "UPDATE ax_jobs SET next_run_at = NOW() - INTERVAL '1 second' WHERE tenant_id = $1",
    )
    .bind(&tenant)
    .execute(&owner_pool)
    .await?;

    let (res_a, res_b) = tokio::join!(
        store.lease("worker-a", &[JobKind::LakeMaintenance], 50, 60),
        store.lease("worker-b", &[JobKind::LakeMaintenance], 50, 60),
    );
    let batch_a = res_a?;
    let batch_b = res_b?;

    assert_eq!(batch_a.len(), 50);
    assert_eq!(batch_b.len(), 50);
    assert!(batch_a.iter().all(|j| j.tenant_id == tenant));
    assert!(batch_b.iter().all(|j| j.tenant_id == tenant));

    let ids_a: BTreeSet<Uuid> = batch_a.iter().map(|j| j.id).collect();
    let ids_b: BTreeSet<Uuid> = batch_b.iter().map(|j| j.id).collect();
    assert_eq!(ids_a.len(), 50);
    assert_eq!(ids_b.len(), 50);
    assert!(
        ids_a.is_disjoint(&ids_b),
        "concurrent lease batches must be strictly disjoint"
    );

    // Verify heartbeat and complete work within tenant RLS context.
    if let Some(sample_job) = batch_a.first() {
        store.heartbeat(sample_job, 120).await?;
        store
            .complete(sample_job, serde_json::json!({"ok": true}))
            .await?;
        let fetched = store
            .get_job(&tenant, sample_job.id)
            .await?
            .ok_or("completed job missing")?;
        assert_eq!(fetched.status, JobStatus::Succeeded);
    }

    // Verify paginated list_jobs works.
    let page1 = store
        .list_jobs(
            &tenant,
            JobListQuery {
                kind: Some(JobKind::LakeMaintenance),
                status: None,
                limit: Some(40),
                cursor: None,
            },
        )
        .await?;
    assert_eq!(page1.items.len(), 40);
    assert!(page1.next_cursor.is_some());

    sqlx::query("DELETE FROM ax_jobs WHERE tenant_id = $1")
        .bind(&tenant)
        .execute(&owner_pool)
        .await?;

    Ok(())
}

#[tokio::test]
async fn jobs_retry_backoff() -> TestResult {
    let Some((app_pool, owner_pool)) = test_pools().await? else {
        return Ok(());
    };

    let tenant = unique_tenant("t_jobs_retry");
    ensure_tenant(&owner_pool, &tenant).await?;

    let store = PgStore::new(app_pool);
    let mut new_job = NewJob::new(JobKind::CredentialRotate, serde_json::json!({"key": 1}));
    new_job.priority = -1_000_000;
    new_job.max_attempts = 3;
    new_job.next_run_at = Some(Utc::now() + chrono::Duration::hours(1));

    let job_id = store.enqueue(&tenant, new_job).await?;
    let initial = store
        .get_job(&tenant, job_id)
        .await?
        .ok_or("enqueued job not found")?;
    assert_eq!(initial.status, JobStatus::Pending);

    let lease_target_attempt = |prev_attempt: i32| {
        let store = store.clone();
        let owner_pool = owner_pool.clone();
        async move {
            for _ in 0..10_u8 {
                sqlx::query(
                    "UPDATE ax_jobs
                        SET status = 'PENDING',
                            attempt = $2,
                            lease_owner = NULL,
                            lease_until = NULL,
                            next_run_at = NOW() - INTERVAL '1 second'
                      WHERE id = $1",
                )
                .bind(job_id)
                .bind(prev_attempt)
                .execute(&owner_pool)
                .await?;

                let leased = store
                    .lease("worker-retry", &[JobKind::CredentialRotate], 1, 60)
                    .await?;
                if let Some(j) = leased.into_iter().find(|j| j.id == job_id) {
                    return Result::<_, Box<dyn std::error::Error>>::Ok(j);
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            Err(format!("expected attempt {} lease", prev_attempt + 1).into())
        }
    };

    // Attempt 1 (< max_attempts=3) -> PENDING with ~2^1 = 2s backoff.
    let j1 = lease_target_attempt(0).await?;
    assert_eq!(j1.id, job_id);
    assert_eq!(j1.attempt, 1);

    let before_fail_1 = Utc::now();
    store.fail(&j1, "attempt 1 failed").await?;
    let after_1 = store
        .get_job(&tenant, job_id)
        .await?
        .ok_or("job missing after fail 1")?;
    assert_eq!(after_1.status, JobStatus::Pending);
    assert!(after_1.next_run_at > before_fail_1);
    let delay_1 = after_1.next_run_at - after_1.updated_at;

    // Attempt 2 (< max_attempts=3) -> PENDING with ~2^2 = 4s backoff (> delay_1).
    let j2 = lease_target_attempt(1).await?;
    assert_eq!(j2.attempt, 2);

    store.fail(&j2, "attempt 2 failed").await?;
    let after_2 = store
        .get_job(&tenant, job_id)
        .await?
        .ok_or("job missing after fail 2")?;
    assert_eq!(after_2.status, JobStatus::Pending);
    assert!(after_2.next_run_at > after_1.next_run_at);
    let delay_2 = after_2.next_run_at - after_2.updated_at;
    assert!(delay_2 > delay_1);

    // Terminal attempt 3 (== max_attempts=3) -> FAILED.
    let j3 = lease_target_attempt(2).await?;
    assert_eq!(j3.attempt, 3);

    store.fail(&j3, "attempt 3 terminal error").await?;
    let after_3 = store
        .get_job(&tenant, job_id)
        .await?
        .ok_or("job missing after fail 3")?;
    assert_eq!(after_3.status, JobStatus::Failed);
    assert_eq!(
        after_3.last_error.as_deref(),
        Some("attempt 3 terminal error")
    );

    sqlx::query("DELETE FROM ax_jobs WHERE id = $1")
        .bind(job_id)
        .execute(&owner_pool)
        .await?;

    Ok(())
}

#[tokio::test]
async fn outbox_claim_and_ack() -> TestResult {
    let Some((app_pool, owner_pool)) = test_pools().await? else {
        return Ok(());
    };

    let tenant = unique_tenant("t_outbox");
    ensure_tenant(&owner_pool, &tenant).await?;

    let mut tx = begin_tenant_tx(&app_pool, &tenant).await?;
    let event_id = insert_outbox_tx(
        &mut tx,
        &tenant,
        &NewOutboxEvent {
            topic: OutboxTopic::NodeUpserted,
            payload: serde_json::json!({"node_key": "W-100"}),
        },
    )
    .await?;
    // Hold a temporary prep claim across commit so trg_ax_outbox_notify's NOTIFY wakeup
    // cannot let a background axpoint-server OutboxDispatcher steal the test row before claim().
    sqlx::query(
        "UPDATE ax_outbox
            SET claimed_by = 'test-prep',
                claimed_until = NOW() + INTERVAL '1 hour'
          WHERE id = $1",
    )
    .bind(event_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    tokio::time::sleep(std::time::Duration::from_millis(15)).await;
    sqlx::query(
        "UPDATE ax_outbox
            SET claimed_by = NULL,
                claimed_until = NULL
          WHERE id = $1",
    )
    .bind(event_id)
    .execute(&owner_pool)
    .await?;

    let store = PgStore::new(app_pool);
    let claimed = store.claim("outbox-worker-1", 500, 60).await?;
    let matched = claimed
        .iter()
        .find(|ev| ev.id == event_id)
        .ok_or("inserted outbox event was not claimed")?;
    assert_eq!(matched.tenant_id, tenant);
    assert_eq!(matched.topic, OutboxTopic::NodeUpserted);

    // While leased, another worker must not re-claim the same event.
    let second_claim = store.claim("outbox-worker-2", 200, 60).await?;
    assert!(second_claim.iter().all(|ev| ev.id != event_id));

    // Acknowledge within tenant context.
    store.ack(&tenant, &[event_id]).await?;

    // Even if claimed_until is cleared, processed_at IS NOT NULL prevents future claims.
    sqlx::query("UPDATE ax_outbox SET claimed_until = NULL WHERE id = $1")
        .bind(event_id)
        .execute(&owner_pool)
        .await?;
    let after_ack_claim = store.claim("outbox-worker-3", 200, 60).await?;
    assert!(after_ack_claim.iter().all(|ev| ev.id != event_id));

    sqlx::query("DELETE FROM ax_outbox WHERE id = $1")
        .bind(event_id)
        .execute(&owner_pool)
        .await?;

    Ok(())
}

#[tokio::test]
async fn credential_store_resolves_and_loads_principal() -> TestResult {
    let Some((app_pool, owner_pool)) = test_pools().await? else {
        return Ok(());
    };

    let tenant = unique_tenant("t_cred");
    ensure_tenant(&owner_pool, &tenant).await?;

    let store = PgStore::new(app_pool.clone());
    let user_id = Uuid::new_v4();
    let role_id = Uuid::new_v4();
    let token_id = Uuid::new_v4();
    let token_hash = format!("test_hash_{}", Uuid::new_v4().simple());

    let mut tx = begin_tenant_tx(&app_pool, &tenant).await?;
    sqlx::query(
        "INSERT INTO sec_roles (id, tenant_id, name, permissions, is_builtin)
         VALUES ($1, $2, 'ANALYST', ARRAY['DATA_READ', 'STUDIO_EDIT'], TRUE)",
    )
    .bind(role_id)
    .bind(&tenant)
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        "INSERT INTO sec_users (id, tenant_id, username, display_name, dept_id)
         VALUES ($1, $2, 'bob', 'Bob Analyst', 'ops')",
    )
    .bind(user_id)
    .bind(&tenant)
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        "INSERT INTO sec_user_roles (tenant_id, user_id, role_id)
         VALUES ($1, $2, $3)",
    )
    .bind(&tenant)
    .bind(user_id)
    .bind(role_id)
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        "INSERT INTO sec_api_tokens (id, tenant_id, user_id, name, token_prefix, token_hash, scopes)
         VALUES ($1, $2, $3, 'cli', 'axp_test', $4, ARRAY['DATA_READ'])",
    )
    .bind(token_id)
    .bind(&tenant)
    .bind(user_id)
    .bind(&token_hash)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    let tok = store
        .resolve_api_token(&token_hash)
        .await?
        .ok_or(AppError::Unauthorized("TOKEN_INVALID"))?;
    assert_eq!(tok.token_id, token_id);
    assert_eq!(tok.tenant_id, tenant);
    assert_eq!(tok.user_id, user_id);
    assert_eq!(tok.scopes, BTreeSet::from([Permission::DataRead]));

    store.touch_api_token(&tenant, token_id).await?;

    let principal = store
        .load_principal(&tenant, user_id)
        .await?
        .ok_or(AppError::Unauthorized("TOKEN_INVALID"))?;
    assert_eq!(principal.username, "bob");
    assert_eq!(principal.dept_id, "ops");
    assert_eq!(principal.roles, vec![String::from("ANALYST")]);
    assert_eq!(
        principal.permissions,
        BTreeSet::from([Permission::DataRead, Permission::StudioEdit])
    );

    sqlx::query("DELETE FROM sec_users WHERE id = $1")
        .bind(user_id)
        .execute(&owner_pool)
        .await?;
    sqlx::query("DELETE FROM sec_roles WHERE id = $1")
        .bind(role_id)
        .execute(&owner_pool)
        .await?;

    Ok(())
}

#[tokio::test]
async fn audit_sink_writes_batches_and_blocks_delete() -> TestResult {
    let Some((app_pool, owner_pool)) = test_pools().await? else {
        return Ok(());
    };

    let tenant = unique_tenant("t_audit");
    let tenant_b = unique_tenant("t_audit_b");
    ensure_tenant(&owner_pool, &tenant).await?;
    ensure_tenant(&owner_pool, &tenant_b).await?;

    let store = PgStore::new(app_pool.clone());
    store
        .write_batch(vec![
            AuditRecord {
                tenant_id: tenant.clone(),
                user_id: None,
                username: String::from("bob"),
                action: AuditAction::Login,
                resource_ref: String::from("session:1"),
                status_code: 200,
                client_ip: String::from("127.0.0.1"),
                trace_id: String::from("tr-1"),
                detail_json: serde_json::json!({"method": "token"}),
                created_at: Utc::now(),
            },
            AuditRecord {
                tenant_id: tenant_b.clone(),
                user_id: None,
                username: String::from("dev"),
                action: AuditAction::RagAsk,
                resource_ref: String::from("chat"),
                status_code: 200,
                client_ip: String::from("127.0.0.1"),
                trace_id: String::from("tr-2"),
                detail_json: serde_json::json!({}),
                created_at: Utc::now(),
            },
        ])
        .await?;

    let mut audit_tx = begin_tenant_tx(&app_pool, &tenant).await?;
    let del_attempt = sqlx::query("DELETE FROM sec_audit_logs WHERE tenant_id = $1")
        .bind(&tenant)
        .execute(&mut *audit_tx)
        .await;
    assert!(
        del_attempt.is_err(),
        "axpoint_app must not have DELETE privilege on sec_audit_logs"
    );
    drop(audit_tx);

    sqlx::query("DELETE FROM sec_audit_chain_heads WHERE tenant_id IN ($1, $2)")
        .bind(&tenant)
        .bind(&tenant_b)
        .execute(&owner_pool)
        .await?;
    sqlx::query("DELETE FROM sec_audit_logs WHERE tenant_id IN ($1, $2)")
        .bind(&tenant)
        .bind(&tenant_b)
        .execute(&owner_pool)
        .await?;

    Ok(())
}

#[tokio::test]
async fn retention_cleanup_purges_expired_jobs_and_outbox() -> TestResult {
    let Some((app_pool, owner_pool)) = test_pools().await? else {
        return Ok(());
    };

    let tenant = unique_tenant("t_ret");
    ensure_tenant(&owner_pool, &tenant).await?;

    let store = PgStore::new(app_pool.clone());
    let active_tenants = store.list_active_tenants().await?;
    assert!(active_tenants.contains(&tenant));

    let mut tx = begin_tenant_tx(&app_pool, &tenant).await?;
    sqlx::query(
        "INSERT INTO ax_retention_policies (tenant_id, target, keep_days)
         VALUES ($1, 'JOB', 7), ($1, 'OUTBOX', 3)",
    )
    .bind(&tenant)
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        "INSERT INTO ax_jobs (tenant_id, kind, status, updated_at)
         VALUES ($1, 'RETENTION', 'SUCCEEDED', NOW() - INTERVAL '10 days'),
                ($1, 'RETENTION', 'SUCCEEDED', NOW() - INTERVAL '1 day')",
    )
    .bind(&tenant)
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        "INSERT INTO ax_outbox (tenant_id, topic, payload, processed_at)
         VALUES ($1, 'node.upserted', '{}'::jsonb, NOW() - INTERVAL '5 days'),
                ($1, 'node.upserted', '{}'::jsonb, NOW() - INTERVAL '1 day')",
    )
    .bind(&tenant)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    let stats = store.run_retention_cleanup(&tenant).await?;
    assert_eq!(stats.get("jobs_deleted"), Some(&serde_json::json!(1)));
    assert_eq!(stats.get("outbox_deleted"), Some(&serde_json::json!(1)));

    sqlx::query("DELETE FROM ax_jobs WHERE tenant_id = $1")
        .bind(&tenant)
        .execute(&owner_pool)
        .await?;
    sqlx::query("DELETE FROM ax_outbox WHERE tenant_id = $1")
        .bind(&tenant)
        .execute(&owner_pool)
        .await?;
    sqlx::query("DELETE FROM ax_retention_policies WHERE tenant_id = $1")
        .bind(&tenant)
        .execute(&owner_pool)
        .await?;

    Ok(())
}
