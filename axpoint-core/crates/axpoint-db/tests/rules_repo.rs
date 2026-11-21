//! Live PostgreSQL + Row-Level Security integration tests for `axpoint-db::rules_repo` (`RulesRepository`).

use axpoint_db::{PgStore, begin_tenant_tx};
use axpoint_domain::AppError;
use axpoint_domain::axhub::QualityOnFail;
use axpoint_domain::enums::{AlertStatus, NotifyKind, OutboxTopic, Severity};
use axpoint_domain::ports::RulesRepository as _;
use axpoint_domain::rules::{
    AlertEventKind, AlertEventsQuery, AlertListQuery, AlertRecord, AlertResolveReason,
    ComparisonOp, Condition, CreateRuleRequest, CreateSilenceRequest, DeliveryFailureUpdate,
    EnqueueDeliveryInput, FalseConditionOutcome, NotifyDeliveryEvent, NotifyDeliveryStatus,
    NullablePatch, RuleListQuery, RuleScopeKind, SaveNotifyChannelInput, SilenceMatcher,
    SimplePageQuery, TrueConditionInput, TrueConditionOutcome, UpdateRuleRequest,
};
use chrono::{DateTime, Duration, TimeZone, Utc};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const DEFAULT_APP_URL: &str = "postgres://axpoint_app:axpoint_app_secret@127.0.0.1:5432/xbluenode";
const DEFAULT_OWNER_URL: &str = "postgres://axpoint:axpoint_secret@127.0.0.1:5432/xbluenode";

async fn test_pools() -> Result<Option<(PgPool, PgPool)>, Box<dyn std::error::Error>> {
    let req = std::env::var("AXPOINT_REQUIRE_DB_TESTS")
        .is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"));
    let app_url = std::env::var("DATABASE_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| String::from(DEFAULT_APP_URL));
    let own_url = std::env::var("MIGRATION_DATABASE_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| String::from(DEFAULT_OWNER_URL));
    let app = match PgPoolOptions::new()
        .max_connections(6)
        .connect(&app_url)
        .await
    {
        Ok(p) => p,
        Err(e) if req => return Err(Box::new(e)),
        Err(_) => return Ok(None),
    };
    let own = match PgPoolOptions::new()
        .max_connections(4)
        .connect(&own_url)
        .await
    {
        Ok(p) => p,
        Err(e) if req => return Err(Box::new(e)),
        Err(_) => return Ok(None),
    };
    Ok(Some((app, own)))
}

fn unique_tenant(prefix: &str) -> String {
    let short: String = Uuid::new_v4()
        .simple()
        .to_string()
        .chars()
        .take(10)
        .collect();
    format!("{prefix}_{short}")
}

async fn setup_tenant(owner_pool: &PgPool, tenant_id: &str) -> TestResult {
    sqlx::query(
        "INSERT INTO sec_tenants (tenant_id, display_name) VALUES ($1, $1) ON CONFLICT DO NOTHING",
    )
    .bind(tenant_id)
    .execute(owner_pool)
    .await?;
    Ok(())
}

async fn cleanup_tenant(owner_pool: &PgPool, tenant_id: &str) -> TestResult {
    for stmt in [
        "DELETE FROM ax_outbox WHERE tenant_id = $1",
        "DELETE FROM ax_notify_deliveries WHERE tenant_id = $1",
        "DELETE FROM ax_alert_events WHERE tenant_id = $1",
        "DELETE FROM ax_alerts WHERE tenant_id = $1",
        "DELETE FROM ax_alert_silences WHERE tenant_id = $1",
        "DELETE FROM ax_rules WHERE tenant_id = $1",
        "DELETE FROM ax_notify_channels WHERE tenant_id = $1",
        "DELETE FROM ds_credentials WHERE tenant_id = $1",
        "DELETE FROM ds_sync_jobs WHERE tenant_id = $1",
        "DELETE FROM ds_datasets WHERE tenant_id = $1",
        "DELETE FROM xb_metric_series WHERE tenant_id = $1",
        "DELETE FROM xb_nodes WHERE tenant_id = $1",
        "DELETE FROM ax_flows WHERE tenant_id = $1",
        "DELETE FROM sec_tenants WHERE tenant_id = $1",
    ] {
        sqlx::query(stmt)
            .bind(tenant_id)
            .execute(owner_pool)
            .await?;
    }
    Ok(())
}

fn rule_req(name: &str, ch_ids: Vec<Uuid>, flow_id: Option<Uuid>) -> CreateRuleRequest {
    CreateRuleRequest {
        name: String::from(name),
        description: String::from("warehouse low stock"),
        scope_kind: RuleScopeKind::Metric,
        target: serde_json::json!({"type_name": "Warehouse", "metric": "inventory", "node_filter": "prop.region == 'east'"}),
        condition: Condition::Threshold {
            op: ComparisonOp::Lt,
            value: 50.0,
            for_minutes: 0,
        },
        severity: Severity::Critical,
        title_template: String::from("Low stock on {{node.title}}"),
        body_template: String::from("Inventory={{value}}"),
        cooldown_minutes: 30,
        resolve_after_minutes: 10,
        escalate_after_minutes: Some(15),
        channel_ids: ch_ids,
        escalation_channel_ids: Vec::new(),
        run_flow_id: flow_id,
        enabled: true,
    }
}

fn true_in(
    rule_id: Uuid,
    dedup: &str,
    node_id: Option<Uuid>,
    val: f64,
    ts: DateTime<Utc>,
) -> TrueConditionInput {
    TrueConditionInput {
        rule_id,
        dedup_key: String::from(dedup),
        severity: Severity::Critical,
        title: String::from("Low stock"),
        body: String::from("Body"),
        node_id,
        dataset_id: None,
        last_value: Some(val),
        detail: serde_json::json!({"metric": "inventory"}),
        cooldown_minutes: 30,
        now: ts,
    }
}

async fn open_alert(
    store: &PgStore,
    tenant: &str,
    input: TrueConditionInput,
) -> Result<AlertRecord, Box<dyn std::error::Error>> {
    match store.record_true_condition(tenant, input).await? {
        TrueConditionOutcome::Opened(a) => Ok(a),
        other => Err(format!("expected Opened, got {other:?}").into()),
    }
}

async fn seed_node(app_pool: &PgPool, tenant: &str, key: &str) -> TestResult {
    let mut tx = begin_tenant_tx(app_pool, tenant).await?;
    sqlx::query("INSERT INTO xb_nodes (tenant_id, node_key, type_name, title) VALUES ($1, $2, 'Warehouse', 'WH')")
        .bind(tenant)
        .bind(key)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

#[tokio::test]
async fn rules_repo_crud_validation_and_rls_isolation() -> TestResult {
    let Some((app_pool, owner_pool)) = test_pools().await? else {
        return Ok(());
    };
    let store = PgStore::new(app_pool.clone());
    let (ta, tb) = (unique_tenant("rules_a"), unique_tenant("rules_b"));
    setup_tenant(&owner_pool, &ta).await?;
    setup_tenant(&owner_pool, &tb).await?;

    let mut bad = rule_req("bad", Vec::new(), None);
    bad.condition = Condition::QualityFailed {
        min_severity: QualityOnFail::Warn,
    };
    assert!(matches!(
        store.create_rule(&ta, None, bad).await,
        Err(AppError::BadRequest {
            code: "INVALID_ARGUMENT",
            ..
        })
    ));
    assert!(matches!(
        store
            .create_rule(&ta, None, rule_req("mc", vec![Uuid::new_v4()], None))
            .await,
        Err(AppError::NotFound("CHANNEL_NOT_FOUND"))
    ));
    assert!(matches!(
        store
            .create_rule(&ta, None, rule_req("mf", Vec::new(), Some(Uuid::new_v4())))
            .await,
        Err(AppError::NotFound("FLOW_NOT_FOUND"))
    ));

    let ch = store
        .create_channel(
            &ta,
            SaveNotifyChannelInput {
                name: String::from("hook"),
                kind: NotifyKind::Webhook,
                config: serde_json::json!({"url": "https://example.com"}),
                credential_id: None,
                new_sealed_secret: None,
                clear_credential: false,
                allow_private_network: false,
                enabled: true,
            },
        )
        .await?;
    let flow_id = Uuid::new_v4();
    let mut tx = begin_tenant_tx(&app_pool, &ta).await?;
    sqlx::query("INSERT INTO ax_flows (id, tenant_id, name) VALUES ($1, $2, 'f1')")
        .bind(flow_id)
        .bind(&ta)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    let created = store
        .create_rule(&ta, None, rule_req("low_inv", vec![ch.id], Some(flow_id)))
        .await?;
    assert!(matches!(
        store
            .create_rule(&ta, None, rule_req("low_inv", Vec::new(), None))
            .await,
        Err(AppError::Conflict("RULE_NAME_CONFLICT"))
    ));

    let upd = UpdateRuleRequest {
        escalate_after_minutes: NullablePatch::Clear,
        cooldown_minutes: Some(20),
        ..UpdateRuleRequest::default()
    };
    let updated = store
        .update_rule(&ta, created.id, upd)
        .await?
        .ok_or("missing")?;
    assert_eq!((updated.version, updated.escalate_after_minutes), (2, None));
    assert!(store.get_rule(&tb, created.id).await?.is_none());
    assert!(
        store
            .list_rules(&tb, RuleListQuery::default())
            .await?
            .items
            .is_empty()
    );
    assert_eq!(
        store
            .list_enabled_rules_by_scope(&ta, RuleScopeKind::Metric)
            .await?
            .len(),
        1
    );
    assert!(store.delete_rule(&ta, created.id).await?);

    cleanup_tenant(&owner_pool, &ta).await?;
    cleanup_tenant(&owner_pool, &tb).await?;
    Ok(())
}

async fn check_resolve_and_cooldown(
    store: &PgStore,
    t: &str,
    rule_id: Uuid,
    aid: Uuid,
    dedup: &str,
    t_false: DateTime<Utc>,
) -> TestResult {
    assert_eq!(
        store.ack_alert(t, aid, None, "ack").await?.status,
        AlertStatus::Ack
    );
    assert!(matches!(
        store.record_false_condition(t, dedup, 10, t_false).await?,
        FalseConditionOutcome::PendingAutoResolve(_)
    ));
    let res = store
        .record_false_condition(t, dedup, 10, t_false + Duration::minutes(10))
        .await?;
    assert!(
        matches!(res, FalseConditionOutcome::AutoResolved(ref a) if a.resolve_reason == Some(AlertResolveReason::Auto))
    );
    assert!(matches!(
        store.ack_alert(t, aid, None, "").await,
        Err(AppError::Conflict("ALERT_ALREADY_RESOLVED"))
    ));
    assert!(matches!(
        store
            .record_true_condition(
                t,
                true_in(rule_id, dedup, None, 5.0, t_false + Duration::minutes(20))
            )
            .await?,
        TrueConditionOutcome::SuppressedByCooldown(_)
    ));
    assert!(matches!(
        store
            .record_true_condition(
                t,
                true_in(rule_id, dedup, None, 5.0, t_false + Duration::minutes(45))
            )
            .await?,
        TrueConditionOutcome::Opened(_)
    ));
    Ok(())
}

#[tokio::test]
async fn rules_repo_alert_dedup_merge_cooldown_and_auto_resolve() -> TestResult {
    let Some((app_pool, owner_pool)) = test_pools().await? else {
        return Ok(());
    };
    let store = PgStore::new(app_pool.clone());
    let t = unique_tenant("alerts_sm");
    setup_tenant(&owner_pool, &t).await?;

    let rule = store
        .create_rule(&t, None, rule_req("inv_rule", Vec::new(), None))
        .await?;
    let nid = Uuid::new_v4();
    let t0 = Utc
        .with_ymd_and_hms(2026, 6, 1, 10, 0, 0)
        .single()
        .ok_or("ts")?;
    let mut tx = begin_tenant_tx(&app_pool, &t).await?;
    sqlx::query("INSERT INTO xb_metric_series (tenant_id, node_id, metric, ts, value, source_tag) VALUES ($1, $2, 'inventory', $3, 12.5, 'MANUAL')")
        .bind(&t).bind(nid).bind(t0).execute(&mut *tx).await?;
    tx.commit().await?;
    assert_eq!(
        store
            .query_raw_metric_points(&t, nid, "inventory", None, 10)
            .await?
            .len(),
        1
    );

    let dedup = format!("{}:{nid}", rule.id);
    let opened = open_alert(&store, &t, true_in(rule.id, &dedup, None, 12.5, t0)).await?;
    let run_id = Uuid::new_v4();
    store
        .set_alert_flow_run(&t, opened.id, Uuid::new_v4(), run_id)
        .await?;
    let _ = store
        .record_true_condition(
            &t,
            true_in(rule.id, &dedup, None, 10.0, t0 + Duration::seconds(20)),
        )
        .await?;
    let retrig = match store
        .record_true_condition(
            &t,
            true_in(rule.id, &dedup, None, 9.0, t0 + Duration::seconds(45)),
        )
        .await?
    {
        TrueConditionOutcome::RetriggeredActive(a) => a,
        other => return Err(format!("unexpected {other:?}").into()),
    };
    assert_eq!(retrig.occurrences, 3);
    assert_eq!(
        retrig
            .detail
            .get("flow_run_id")
            .and_then(serde_json::Value::as_str),
        Some(run_id.to_string().as_str())
    );
    let evs = store
        .list_alert_events(&t, opened.id, AlertEventsQuery::default())
        .await?;
    assert_eq!(
        evs.items
            .iter()
            .filter(|e| e.event == AlertEventKind::Retriggered)
            .count(),
        1
    );

    check_resolve_and_cooldown(
        &store,
        &t,
        rule.id,
        opened.id,
        &dedup,
        t0 + Duration::minutes(2),
    )
    .await?;
    let topics: Vec<String> =
        sqlx::query_scalar("SELECT topic FROM ax_outbox WHERE tenant_id = $1")
            .bind(&t)
            .fetch_all(&owner_pool)
            .await?;
    assert!(
        topics
            .iter()
            .any(|tp| tp == OutboxTopic::AlertOpened.as_str())
            && topics
                .iter()
                .any(|tp| tp == OutboxTopic::AlertResolved.as_str())
    );

    cleanup_tenant(&owner_pool, &t).await?;
    Ok(())
}

#[tokio::test]
async fn rules_repo_escalation_batch_ack_manual_resolve_and_rule_disable() -> TestResult {
    let Some((app_pool, owner_pool)) = test_pools().await? else {
        return Ok(());
    };
    let store = PgStore::new(app_pool.clone());
    let t = unique_tenant("alerts_esc");
    setup_tenant(&owner_pool, &t).await?;
    seed_node(&app_pool, &t, "wh-2").await?;
    let mut tx = begin_tenant_tx(&app_pool, &t).await?;
    let nid: Uuid = sqlx::query_scalar("SELECT id FROM xb_nodes WHERE tenant_id = $1 LIMIT 1")
        .bind(&t)
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;

    let rule = store
        .create_rule(&t, None, rule_req("esc_rule", Vec::new(), None))
        .await?;
    let t0 = Utc
        .with_ymd_and_hms(2026, 6, 2, 8, 0, 0)
        .single()
        .ok_or("ts")?;
    let a1 = open_alert(
        &store,
        &t,
        true_in(rule.id, &format!("{}:n1", rule.id), Some(nid), 20.0, t0),
    )
    .await?;
    let a2 = open_alert(
        &store,
        &t,
        true_in(rule.id, &format!("{}:n2", rule.id), Some(nid), 20.0, t0),
    )
    .await?;

    assert!(
        store
            .list_absence_rules()
            .await?
            .iter()
            .any(|it| it.tenant_id == t && it.rule_id == rule.id)
    );
    assert!(
        store
            .escalate_due_alerts(&t, rule.id, 15, t0 + Duration::minutes(10))
            .await?
            .is_empty()
    );
    assert_eq!(
        store
            .escalate_due_alerts(&t, rule.id, 15, t0 + Duration::minutes(16))
            .await?
            .len(),
        2
    );
    assert_eq!(
        store
            .list_alerts(
                &t,
                AlertListQuery {
                    type_name: Some(String::from("Warehouse")),
                    ..AlertListQuery::default()
                }
            )
            .await?
            .items
            .len(),
        2
    );
    assert_eq!(
        store
            .ack_alerts_batch(&t, &[a1.id], None, "batch")
            .await?
            .acked_count,
        1
    );
    assert_eq!(
        store
            .resolve_alert_manual(&t, a1.id, None, "fixed")
            .await?
            .resolve_reason,
        Some(AlertResolveReason::Manual)
    );

    store
        .update_rule(
            &t,
            rule.id,
            UpdateRuleRequest {
                enabled: Some(false),
                ..UpdateRuleRequest::default()
            },
        )
        .await?;
    assert_eq!(
        store
            .get_alert(&t, a2.id)
            .await?
            .ok_or("a2")?
            .resolve_reason,
        Some(AlertResolveReason::RuleDisabled)
    );

    cleanup_tenant(&owner_pool, &t).await?;
    Ok(())
}

#[tokio::test]
async fn rules_repo_channels_credentials_and_deliveries() -> TestResult {
    let Some((app_pool, owner_pool)) = test_pools().await? else {
        return Ok(());
    };
    let store = PgStore::new(app_pool);
    let t = unique_tenant("rules_ch");
    setup_tenant(&owner_pool, &t).await?;

    let cid = Uuid::new_v4();
    let ch = store
        .create_channel(
            &t,
            SaveNotifyChannelInput {
                name: String::from("feishu"),
                kind: NotifyKind::Feishu,
                config: serde_json::json!({"__secret_fields": ["sign_secret"]}),
                credential_id: Some(cid),
                new_sealed_secret: Some((1, vec![1_u8; 12], vec![9_u8; 32])),
                clear_credential: false,
                allow_private_network: false,
                enabled: true,
            },
        )
        .await?;
    assert_eq!(ch.secret_fields_set, vec!["sign_secret"]);
    let (_, cred) = store.get_channel(&t, ch.id).await?.ok_or("ch")?;
    assert_eq!(cred.ok_or("cred")?.id, cid);

    let rule = store
        .create_rule(&t, None, rule_req("deliv_rule", vec![ch.id], None))
        .await?;
    let now = Utc::now();
    let alert = open_alert(
        &store,
        &t,
        true_in(rule.id, &format!("{}:o", rule.id), None, 1.0, now),
    )
    .await?;
    let (d1, ex1) = store
        .enqueue_delivery(
            &t,
            EnqueueDeliveryInput {
                alert_id: Some(alert.id),
                channel_id: ch.id,
                event: NotifyDeliveryEvent::Opened,
                now: now - Duration::seconds(5),
            },
        )
        .await?;
    let (d2, ex2) = store
        .enqueue_delivery(
            &t,
            EnqueueDeliveryInput {
                alert_id: Some(alert.id),
                channel_id: ch.id,
                event: NotifyDeliveryEvent::Opened,
                now,
            },
        )
        .await?;
    assert!(!ex1 && ex2 && d1.id == d2.id);
    assert!(
        store
            .list_due_deliveries(50)
            .await?
            .iter()
            .any(|d| d.delivery_id == d1.id)
    );

    let fail_upd = DeliveryFailureUpdate {
        attempt: 1,
        response_code: Some(502),
        error_message: String::from("err"),
        next_retry_at: Some(now + Duration::seconds(30)),
    };
    assert_eq!(
        store
            .mark_delivery_failed(&t, d1.id, fail_upd)
            .await?
            .status,
        NotifyDeliveryStatus::Pending
    );
    assert_eq!(
        store
            .mark_delivery_sent(&t, d1.id, 2, Some(200))
            .await?
            .status,
        NotifyDeliveryStatus::Sent
    );
    assert_eq!(
        store
            .get_alert(&t, alert.id)
            .await?
            .ok_or("a")?
            .deliveries
            .len(),
        1
    );

    cleanup_tenant(&owner_pool, &t).await?;
    Ok(())
}

#[tokio::test]
async fn rules_repo_silences_and_dataset_quality() -> TestResult {
    let Some((app_pool, owner_pool)) = test_pools().await? else {
        return Ok(());
    };
    let store = PgStore::new(app_pool.clone());
    let t = unique_tenant("rules_sil");
    setup_tenant(&owner_pool, &t).await?;

    let now = Utc::now();
    assert!(
        store
            .create_silence(
                &t,
                None,
                CreateSilenceRequest {
                    matcher: SilenceMatcher::default(),
                    starts_at: now,
                    ends_at: now,
                    reason: String::from("bad"),
                },
            )
            .await
            .is_err()
    );
    let sil = store
        .create_silence(
            &t,
            None,
            CreateSilenceRequest {
                matcher: SilenceMatcher::default(),
                starts_at: now - Duration::minutes(5),
                ends_at: now + Duration::minutes(5),
                reason: String::from("m"),
            },
        )
        .await?;
    assert_eq!(store.list_active_silences(&t, now).await?.len(), 1);
    assert_eq!(
        store
            .list_silences(&t, SimplePageQuery::default())
            .await?
            .items
            .len(),
        1
    );
    assert!(store.delete_silence(&t, sil.id).await?);

    let ds_id = Uuid::new_v4();
    let mut tx = begin_tenant_tx(&app_pool, &t).await?;
    sqlx::query("INSERT INTO ds_datasets (id, tenant_id, name, source_kind, lake_schema, lake_table, status) VALUES ($1, $2, 'ord_tbl', 'FILE_TABLE', 'tx', 'ord_tbl', 'READY')")
        .bind(ds_id)
        .bind(&t)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO ds_sync_jobs (tenant_id, dataset_id, job_id, batch_id, sync_mode, status, quality_result) VALUES ($1, $2, $3, $4, 'SNAPSHOT', 'SUCCEEDED', '{\"passed\":false,\"blocked\":false,\"rules\":[]}'::jsonb)")
        .bind(&t)
        .bind(ds_id)
        .bind(Uuid::new_v4())
        .bind(Uuid::new_v4())
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    let (_, qr) = store
        .get_latest_dataset_quality(&t, ds_id)
        .await?
        .ok_or("ds")?;
    assert!(!qr.ok_or("qr")?.passed);

    cleanup_tenant(&owner_pool, &t).await?;
    Ok(())
}
