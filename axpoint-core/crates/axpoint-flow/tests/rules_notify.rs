//! Integration and cryptographic vector tests for Step 08 rules engine and notification channels (`07-STEP07-08` §08.7).

mod common;

use std::sync::Arc;

use axpoint_domain::AppError;
use axpoint_domain::axhub::{QualityOnFail, QualityReport, QualityRuleOutcome};
use axpoint_domain::enums::{AlertStatus, JobKind, JobStatus, NotifyKind, OutboxTopic, Severity};
use axpoint_domain::platform::{JobHandler as _, LeasedJob, OutboxEvent};
use axpoint_domain::rules::{
    AlertEventKind, AlertSilenceRecord, ComparisonOp, Condition, CreateRuleRequest,
    EventRuleTarget, NotifyDeliveryStatus, RuleEvalJobPayload, RuleScopeKind, SilenceMatcher,
};
use axpoint_flow::notify::dingtalk::compute_dingtalk_sign;
use axpoint_flow::notify::feishu::compute_feishu_sign;
use axpoint_flow::notify::template::{AlertTemplateVars, escape_html, render_alert_template};
use axpoint_flow::notify::webhook::compute_webhook_signature;
use axpoint_flow::rules::{
    eval_absence_points, eval_anomaly_zscore_points, eval_change_rate_points, eval_event_condition,
    eval_expr_condition, eval_quality_condition, eval_threshold_points,
};
use axpoint_flow::{
    AbsenceRuleScanner, AlertNotifySubscriber, NotificationDispatcher, RuleEvalJobHandler,
    RuleEvaluator, RuleFlowLauncherPort, RuleTriggerSubscriber, delivery_retry_delay,
    eval_node_filter,
};
use chrono::{Duration as ChronoDuration, TimeZone as _, Utc};
use common::{
    MemStore, MockSsrfGuard, make_channel, make_metric_rule, make_warehouse_node, spawn_http_sink,
};
use uuid::Uuid;

#[test]
fn rules_all_seven_conditions_and_node_filter_evaluate_accurately() -> Result<(), AppError> {
    let now = Utc
        .with_ymd_and_hms(2026, 6, 1, 12, 0, 0)
        .single()
        .ok_or_else(|| AppError::Config(String::new()))?;
    assert!(eval_threshold_points(ComparisonOp::Lt, 50.0, 0, &[(now, 10.0)], None, now).matched);
    assert!(!eval_threshold_points(ComparisonOp::Lt, 50.0, 5, &[], None, now).matched);
    let win_pts = [(now - ChronoDuration::minutes(3), 20.0), (now, 10.0)];
    assert!(eval_threshold_points(ComparisonOp::Lt, 50.0, 5, &win_pts, None, now).matched);

    let zero_start = [(now - ChronoDuration::minutes(5), 0.0), (now, 50.0)];
    assert!(!eval_change_rate_points(10, ComparisonOp::Gt, 0.1, &zero_start, now).matched);
    let drop_pts = [(now - ChronoDuration::minutes(5), 100.0), (now, 70.0)];
    assert!(eval_change_rate_points(10, ComparisonOp::Lt, -0.25, &drop_pts, now).matched);

    assert!(eval_absence_points(5, &[(now - ChronoDuration::minutes(10), 1.0)], now).matched);
    assert!(!eval_absence_points(5, &[(now - ChronoDuration::minutes(2), 1.0)], now).matched);

    let flat = [
        (now, 10.0),
        (now - ChronoDuration::minutes(1), 10.0),
        (now - ChronoDuration::minutes(2), 10.0),
        (now - ChronoDuration::minutes(3), 10.0),
    ];
    assert!(!eval_anomaly_zscore_points(5, 3.0, 3, &flat).matched);
    let spike = [
        (now, 20.0),
        (now - ChronoDuration::minutes(1), 10.0),
        (now - ChronoDuration::minutes(2), 12.0),
        (now - ChronoDuration::minutes(3), 10.0),
        (now - ChronoDuration::minutes(4), 12.0),
    ];
    assert!(eval_anomaly_zscore_points(4, 3.0, 3, &spike).matched);

    let mut props = serde_json::Map::new();
    props.insert(String::from("region"), serde_json::json!("华东"));
    props.insert(String::from("min_stock"), serde_json::json!(30.0));
    let node = make_warehouse_node(Uuid::new_v4(), "华东仓", 10.0, props, now);
    assert!(eval_node_filter("prop.region == '华东'", &node)?);
    assert!(eval_expr_condition("inventory < prop.min_stock", &node, "inventory", None)?.matched);

    let q_warn = QualityReport {
        passed: false,
        blocked: false,
        rules: vec![QualityRuleOutcome {
            rule_id: String::from("q1"),
            rule_kind: String::from("NOT_NULL"),
            column_name: None,
            passed: false,
            on_fail: QualityOnFail::Warn,
            detail: String::new(),
        }],
    };
    let block_cond = Condition::QualityFailed {
        min_severity: QualityOnFail::Block,
    };
    let warn_cond = Condition::QualityFailed {
        min_severity: QualityOnFail::Warn,
    };
    assert!(!eval_quality_condition(&block_cond, Some(&q_warn))?.matched);
    assert!(eval_quality_condition(&warn_cond, Some(&q_warn))?.matched);

    let ev_target = EventRuleTarget {
        topic: String::from("approval.decided"),
        filter: Some(String::from("status == 'REJECTED'")),
    };
    let ev_res = eval_event_condition(
        &ev_target,
        &Condition::EventMatch {},
        "approval.decided",
        &serde_json::json!({ "status": "REJECTED" }),
    )?;
    assert!(ev_res.matched);
    Ok(())
}

#[test]
fn notify_templates_and_official_hmac_vectors_match_specifications() -> Result<(), AppError> {
    let vars = AlertTemplateVars {
        rule_name: String::from("库存红线"),
        node_title: Some(String::from("{{rule.name}}仓")),
        node_type_name: Some(String::from("Warehouse")),
        node_key: Some(String::from("W-1")),
        metric: Some(String::from("inventory")),
        value: Some(10.0),
        threshold: Some(50.0),
        severity: Some(Severity::Critical),
        dataset_name: Some(String::from("orders")),
        link: Some(String::from("/alerts")),
    };
    let tpl = "[{{severity}}] {{rule.name}} {{node.title}}({{node.type_name}}/{{node.key}}) {{metric}}={{value}}<{{threshold}} {{dataset.name}} {{link}}";
    assert_eq!(
        render_alert_template(tpl, &vars),
        "[CRITICAL] 库存红线 {{rule.name}}仓(Warehouse/W-1) inventory=10<50 orders /alerts"
    );
    assert_eq!(
        escape_html("<b a=\"1\">x&'y'</b>"),
        "&lt;b a=&quot;1&quot;&gt;x&amp;&#39;y&#39;&lt;/b&gt;"
    );

    assert_eq!(
        compute_webhook_signature(
            "whsec_axpoint_step08",
            "1700000000",
            b"{\"event\":\"OPENED\",\"tenant_id\":\"default\"}"
        )?,
        "sha256=f6cfe2a14f6f00da5fd260c47a413c9afd4bf3fef4dc28e838436d716a011901"
    );
    assert_eq!(
        compute_dingtalk_sign(1_577_836_800_000, "SECtest_secret_123456")?,
        "qWJB3SeLLH1aUWbyC%2Fj2Fc0lrbrccGXIOtfMwtoJniU%3D"
    );
    assert_eq!(
        compute_feishu_sign(1_599_360_473, "demo")?,
        "l1N0gAcBjdwBvGm1xMjOF0XSyaLRpR7tuO5dHfhAYc8="
    );
    Ok(())
}

#[tokio::test]
async fn notify_webhook_headers_bot_errcodes_ssrf_and_retry_schedule_work() -> Result<(), AppError>
{
    assert_eq!(delivery_retry_delay(1), Some(ChronoDuration::seconds(30)));
    assert_eq!(delivery_retry_delay(2), Some(ChronoDuration::minutes(2)));
    assert_eq!(delivery_retry_delay(3), Some(ChronoDuration::minutes(10)));
    assert_eq!(delivery_retry_delay(4), Some(ChronoDuration::minutes(30)));
    assert_eq!(delivery_retry_delay(5), None);

    let store = Arc::new(MemStore::default());
    let dispatcher = Arc::new(NotificationDispatcher::new(
        Arc::clone(&store),
        Arc::new(MockSsrfGuard),
        Arc::clone(&store),
    ));

    let (wh_url, wh_cap) = spawn_http_sink(200, r#"{"ok":true}"#, 1).await?;
    let cred_id = Uuid::new_v4();
    let ch_wh = Uuid::new_v4();
    store.lock_st().secrets.insert(
        cred_id,
        serde_json::json!({ "signing_secret": "s3cr3t", "bearer_token": "tok_abc" }),
    );
    store.lock_st().channels.insert(
        ch_wh,
        make_channel(
            ch_wh,
            NotifyKind::Webhook,
            serde_json::json!({ "url": wh_url }),
            Some(cred_id),
            true,
        ),
    );
    assert!(dispatcher.test_channel("t1", ch_wh).await?.ok);

    {
        let cap_guard = wh_cap
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (hdrs, raw_body) = cap_guard
            .first()
            .ok_or_else(|| AppError::Config(String::new()))?;
        assert!(hdrs.contains("x-axpoint-event: test"));
        assert!(hdrs.contains("authorization: bearer tok_abc"));
        assert!(hdrs.contains("idempotency-key:"));
        let ts_val = hdrs
            .lines()
            .find_map(|l| l.strip_prefix("x-axpoint-timestamp:"))
            .map_or("", str::trim);
        let expected_sig = compute_webhook_signature("s3cr3t", ts_val, raw_body.as_bytes())?;
        assert!(hdrs.contains(&format!("x-axpoint-signature: {expected_sig}")));
    }

    let (err_url, _) =
        spawn_http_sink(200, r#"{"errcode":40001,"errmsg":"bad","code":19021}"#, 3).await?;
    for kind in [NotifyKind::Wecom, NotifyKind::Dingtalk, NotifyKind::Feishu] {
        let cid = Uuid::new_v4();
        store.lock_st().channels.insert(
            cid,
            make_channel(
                cid,
                kind,
                serde_json::json!({ "webhook_url": err_url }),
                None,
                true,
            ),
        );
        assert!(!dispatcher.test_channel("t1", cid).await?.ok);
    }

    let ssrf_ch = Uuid::new_v4();
    store.lock_st().channels.insert(
        ssrf_ch,
        make_channel(
            ssrf_ch,
            NotifyKind::Webhook,
            serde_json::json!({ "url": "http://169.254.169.254/latest/meta-data" }),
            None,
            true,
        ),
    );
    assert!(matches!(
        dispatcher.test_channel("t1", ssrf_ch).await,
        Err(AppError::Forbidden(_))
    ));
    Ok(())
}

fn seed_warehouse_fixture(
    store: &MemStore,
    channel_id: Uuid,
    flow_id: Option<Uuid>,
) -> (Uuid, Uuid) {
    let rule_id = Uuid::new_v4();
    let node_id = Uuid::new_v4();
    let past = Utc::now() - ChronoDuration::minutes(5);
    let mut g = store.lock_st();
    g.nodes.insert(
        node_id,
        make_warehouse_node(node_id, "一号仓", 10.0, serde_json::Map::new(), past),
    );
    g.raw_points
        .insert((node_id, String::from("inventory")), vec![(past, 10.0)]);
    let cond = Condition::Threshold {
        op: ComparisonOp::Lt,
        value: 50.0,
        for_minutes: 0,
    };
    g.rules.insert(
        rule_id,
        make_metric_rule(rule_id, cond, channel_id, flow_id, past),
    );
    (rule_id, node_id)
}

#[tokio::test]
async fn rules_evaluator_dry_run_triggers_and_linked_flow_work() -> Result<(), AppError> {
    let store = Arc::new(MemStore::default());
    let store_launcher = Arc::clone(&store);
    let launcher: Arc<dyn RuleFlowLauncherPort> = store_launcher;
    let evaluator = Arc::new(RuleEvaluator::new(Arc::clone(&store), Some(launcher)));

    let (rule_id, node_id) = seed_warehouse_fixture(&store, Uuid::new_v4(), Some(Uuid::new_v4()));

    let adhoc_req = CreateRuleRequest {
        name: String::from("预览规则"),
        description: String::new(),
        scope_kind: RuleScopeKind::Metric,
        target: serde_json::json!({ "type_name": "Warehouse", "metric": "inventory" }),
        condition: Condition::Threshold {
            op: ComparisonOp::Lt,
            value: 50.0,
            for_minutes: 0,
        },
        severity: Severity::Warn,
        title_template: String::from("{{node.title}}"),
        body_template: String::new(),
        cooldown_minutes: 30,
        resolve_after_minutes: 10,
        escalate_after_minutes: None,
        channel_ids: Vec::new(),
        escalation_channel_ids: Vec::new(),
        run_flow_id: None,
        enabled: true,
    };
    assert_eq!(
        evaluator
            .dry_run_request("t1", &adhoc_req)
            .await?
            .matched_count,
        1
    );
    assert_eq!(
        evaluator.dry_run_rule("t1", rule_id).await?.matched_count,
        1
    );
    assert!(store.lock_st().alerts.is_empty());

    let trig = RuleTriggerSubscriber::new(Arc::clone(&store));
    let ev = OutboxEvent {
        id: 7,
        tenant_id: String::from("t1"),
        topic: OutboxTopic::NodeMetricsChanged,
        payload: serde_json::json!({ "node_ids": [node_id] }),
        claimed_by: None,
        claimed_until: None,
        created_at: Utc::now(),
    };
    assert_eq!(trig.dispatch_event(&ev).await?, 1);
    assert_eq!(
        AbsenceRuleScanner::new(Arc::clone(&store))
            .scan_once(Utc::now())
            .await?,
        1
    );

    let handler = RuleEvalJobHandler::new(Arc::clone(&evaluator));
    let job = LeasedJob {
        id: Uuid::new_v4(),
        tenant_id: String::from("t1"),
        kind: JobKind::RuleEval,
        payload: serde_json::json!({ "rule_id": rule_id, "node_ids": [node_id] }),
        status: JobStatus::Running,
        priority: 20,
        attempt: 1,
        max_attempts: 3,
        next_run_at: Utc::now(),
        lease_owner: String::from("w"),
        lease_until: Utc::now(),
        idempotency_key: None,
        last_error: None,
        created_at: Utc::now(),
        updated_at: Utc::now(),
    };
    let _ = handler.handle(&job).await?;
    assert_eq!(store.lock_st().launched_flows.len(), 1);
    assert_eq!(store.lock_st().alerts.len(), 1);
    Ok(())
}

#[tokio::test]
async fn rules_evaluator_silence_retry_exhaustion_and_auto_resolve_work() -> Result<(), AppError> {
    let store = Arc::new(MemStore::default());
    let dispatcher = Arc::new(NotificationDispatcher::new(
        Arc::clone(&store),
        Arc::new(MockSsrfGuard),
        Arc::clone(&store),
    ));
    let evaluator = Arc::new(
        RuleEvaluator::new(Arc::clone(&store), None).with_escalation_notifier(dispatcher.clone()),
    );

    let (fail_url, _) = spawn_http_sink(500, r#"{"error":"boom"}"#, 8).await?;
    let ch_fail = Uuid::new_v4();
    store.lock_st().channels.insert(
        ch_fail,
        make_channel(
            ch_fail,
            NotifyKind::Webhook,
            serde_json::json!({ "url": fail_url }),
            None,
            true,
        ),
    );
    let (rule_id, node_id) = seed_warehouse_fixture(&store, ch_fail, None);

    let eval_payload = RuleEvalJobPayload {
        rule_id,
        node_ids: vec![node_id],
        ..RuleEvalJobPayload::default()
    };
    let _ = evaluator.evaluate_job("t1", &eval_payload).await?;
    let alert_id = store
        .lock_st()
        .alerts
        .keys()
        .copied()
        .next()
        .ok_or_else(|| AppError::Config(String::new()))?;

    let now = Utc::now();
    store.lock_st().silences.push(AlertSilenceRecord {
        id: Uuid::new_v4(),
        matcher: SilenceMatcher {
            rule_ids: vec![rule_id],
            ..SilenceMatcher::default()
        },
        starts_at: now - ChronoDuration::minutes(5),
        ends_at: now + ChronoDuration::minutes(5),
        reason: String::from("maint"),
        created_by: None,
        created_at: now,
    });
    let sub = AlertNotifySubscriber::new(Arc::clone(&dispatcher));
    let open_ev = OutboxEvent {
        id: 8,
        tenant_id: String::from("t1"),
        topic: OutboxTopic::AlertOpened,
        payload: serde_json::json!({ "alert_id": alert_id }),
        claimed_by: None,
        claimed_until: None,
        created_at: now,
    };
    assert!(sub.handle_event(&open_ev).await?.is_empty());
    assert!(
        store
            .lock_st()
            .events
            .iter()
            .any(|e| e.event == AlertEventKind::Silenced)
    );

    store.lock_st().silences.clear();
    let _ = sub.handle_event(&open_ev).await?;
    for _ in 0..4 {
        let _ = dispatcher.poll_due_deliveries(10).await?;
    }
    assert!(
        store
            .lock_st()
            .deliveries
            .values()
            .any(|d| d.attempt == 5 && d.status == NotifyDeliveryStatus::Failed)
    );

    store.lock_st().raw_points.insert(
        (node_id, String::from("inventory")),
        vec![(Utc::now(), 80.0)],
    );
    let _ = evaluator.evaluate_job("t1", &eval_payload).await?;
    assert_eq!(
        store.lock_st().alerts.get(&alert_id).map(|a| a.status),
        Some(AlertStatus::Resolved)
    );
    Ok(())
}
