//! Step 32 integration tests for `GET /api/axops/pulse` verifying single-tenant scale aggregation,
//! 24-hour UTC bucket series, subgraph readiness cards, active alerts, pending approvals,
//! `AxSim` `deadline_step` redline warnings, strict cross-tenant RLS isolation, and `403` RBAC enforcement.

use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use axpoint_domain::enums::{AlertStatus, DeadlineWindowStatus, RuntimeCertLevel, Severity};
use axpoint_domain::ops::PlatformPulseResponse;
use axpoint_security::sha256_hex;
use reqwest::StatusCode;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const DEFAULT_APP_URL: &str = "postgres://axpoint_app:axpoint_app_secret@127.0.0.1:5432/xbluenode";
const DEFAULT_OWNER_URL: &str = "postgres://axpoint:axpoint_secret@127.0.0.1:5432/xbluenode";

struct ServerGuard {
    child: Child,
}

impl Drop for ServerGuard {
    fn drop(&mut self) {
        if let Err(err) = self.child.kill() {
            tracing::debug!(error = %err, "axpoint-server test child already exited");
        }
        if let Err(err) = self.child.wait() {
            tracing::debug!(error = %err, "axpoint-server test child wait failed");
        }
    }
}

struct SeededTenantAssets {
    subgraph_slug: String,
    dashboard_slug: String,
    sandbox_id: Uuid,
    scenario_id: Uuid,
    alert_id: Uuid,
    approval_id: Uuid,
}

async fn test_pools() -> Result<Option<(String, PgPool, PgPool)>, Box<dyn std::error::Error>> {
    let has_explicit_db = std::env::var("DATABASE_URL").is_ok_and(|s| !s.trim().is_empty());
    let require_db = has_explicit_db
        || std::env::var("AXPOINT_REQUIRE_DB_TESTS")
            .is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"));

    let app_url = std::env::var("DATABASE_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| String::from(DEFAULT_APP_URL));
    let owner_url = std::env::var("MIGRATION_DATABASE_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| String::from(DEFAULT_OWNER_URL));

    let app_pool = match PgPoolOptions::new()
        .max_connections(4)
        .connect(&app_url)
        .await
    {
        Ok(p) => p,
        Err(err) if require_db => return Err(Box::new(err)),
        Err(_) => return Ok(None),
    };
    let owner_pool = match PgPoolOptions::new()
        .max_connections(4)
        .connect(&owner_url)
        .await
    {
        Ok(p) => p,
        Err(err) if require_db => return Err(Box::new(err)),
        Err(_) => return Ok(None),
    };

    sqlx::migrate!("../../../deploy/migrations")
        .run(&owner_pool)
        .await?;

    Ok(Some((app_url, app_pool, owner_pool)))
}

fn unique_tenant(prefix: &str) -> String {
    let hex_id = Uuid::new_v4().simple().to_string();
    let short: String = hex_id.chars().take(10).collect();
    format!("{prefix}_{short}")
}

async fn spawn_test_server(
    app_url: &str,
) -> Result<(String, ServerGuard), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let bind_addr = listener.local_addr()?;
    drop(listener);

    let metrics_listener = TcpListener::bind("127.0.0.1:0")?;
    let metrics_addr = metrics_listener.local_addr()?;
    drop(metrics_listener);

    let data_root = std::env::temp_dir().join(format!("axpoint_step32_test_{}", Uuid::new_v4()));
    std::fs::create_dir_all(&data_root)?;

    let child = Command::new(env!("CARGO_BIN_EXE_axpoint-server"))
        .env_remove("OTEL_EXPORTER_OTLP_ENDPOINT")
        .env("AXPOINT_ENV", "test")
        .env("DATABASE_URL", app_url)
        .env("AXPOINT_BIND_ADDR", bind_addr.to_string())
        .env("AXPOINT_METRICS_ADDR", metrics_addr.to_string())
        .env("AXPOINT_DATA_ROOT", &data_root)
        .env("PARSER_BASE_URL", "http://127.0.0.1:9")
        .env(
            "AXPOINT_DEV_TOKEN",
            "0123456789abcdef0123456789abcdef_step32_dev",
        )
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;

    let guard = ServerGuard { child };
    let base_url = format!("http://{bind_addr}");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()?;

    let mut ready = false;
    for _ in 0..60_u8 {
        if let Ok(resp) = client.get(format!("{base_url}/api/health")).send().await
            && resp.status().is_success()
        {
            ready = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    if !ready {
        return Err("axpoint-server failed to become healthy within 6s".into());
    }

    Ok((base_url, guard))
}

async fn provision_api_token(
    owner_pool: &PgPool,
    tenant_id: &str,
    username: &str,
    permissions: &[&str],
) -> Result<String, Box<dyn std::error::Error>> {
    sqlx::query(
        "INSERT INTO sec_tenants (tenant_id, display_name)
         VALUES ($1, $1)
         ON CONFLICT (tenant_id) DO NOTHING",
    )
    .bind(tenant_id)
    .execute(owner_pool)
    .await?;

    let role_id = Uuid::new_v4();
    let role_name = format!("ROLE_{username}");
    let perms_vec: Vec<String> = permissions.iter().map(|p| (*p).to_owned()).collect();
    sqlx::query(
        "INSERT INTO sec_roles (id, tenant_id, name, permissions, is_builtin)
         VALUES ($1, $2, $3, $4, FALSE)",
    )
    .bind(role_id)
    .bind(tenant_id)
    .bind(&role_name)
    .bind(&perms_vec)
    .execute(owner_pool)
    .await?;

    let user_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO sec_users (id, tenant_id, username, display_name, clearance)
         VALUES ($1, $2, $3, $3, 'SECRET')",
    )
    .bind(user_id)
    .bind(tenant_id)
    .bind(username)
    .execute(owner_pool)
    .await?;

    sqlx::query(
        "INSERT INTO sec_user_roles (tenant_id, user_id, role_id)
         VALUES ($1, $2, $3)",
    )
    .bind(tenant_id)
    .bind(user_id)
    .bind(role_id)
    .execute(owner_pool)
    .await?;

    let raw_secret = Uuid::new_v4().simple().to_string();
    let raw_token = format!("axp_step3200_{raw_secret}");
    let token_hash = sha256_hex(&raw_token);
    sqlx::query(
        "INSERT INTO sec_api_tokens (tenant_id, user_id, name, token_prefix, token_hash, scopes)
         VALUES ($1, $2, $3, 'step3200', $4, $5)",
    )
    .bind(tenant_id)
    .bind(user_id)
    .bind(username)
    .bind(&token_hash)
    .bind(&perms_vec)
    .execute(owner_pool)
    .await?;

    Ok(raw_token)
}

async fn seed_tenant_a_graph_and_data(
    owner_pool: &PgPool,
    tenant_a: &str,
) -> Result<(Uuid, Uuid), Box<dyn std::error::Error>> {
    sqlx::query(
        "INSERT INTO xb_subgraphs (tenant_id, slug, display_name, description, is_main, is_dev_sandbox)
         VALUES
           ($1, 'power_grid', '新型电力与配网调度', '220kV 主变与馈线负荷模型', TRUE, FALSE),
           ($1, 'power_trial', '演练沙盒', '开发试灌子图', FALSE, TRUE)",
    )
    .bind(tenant_a)
    .execute(owner_pool)
    .await?;

    sqlx::query(
        "INSERT INTO xb_schema_heads (tenant_id, subgraph_slug, current_version)
         VALUES ($1, 'power_grid', 2)",
    )
    .bind(tenant_a)
    .execute(owner_pool)
    .await?;

    sqlx::query(
        "INSERT INTO xb_node_types (tenant_id, subgraph_slug, name, display_name, domain_group)
         VALUES
           ($1, 'power_grid', 'Substation', '变电站', '输配电域'),
           ($1, 'power_grid', 'Feeder', '馈线', '调度域'),
           ($1, 'power_trial', 'TrialNode', '试算节点', '试算域')",
    )
    .bind(tenant_a)
    .execute(owner_pool)
    .await?;

    sqlx::query(
        "INSERT INTO xb_edge_types (tenant_id, subgraph_slug, name, src_type_name, dst_type_name)
         VALUES ($1, 'power_grid', 'FEEDS_LINE', 'Substation', 'Feeder')",
    )
    .bind(tenant_a)
    .execute(owner_pool)
    .await?;

    let node1 = Uuid::new_v4();
    let node2 = Uuid::new_v4();
    let trial_node = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO xb_nodes (id, tenant_id, subgraph_slug, type_name, node_key, title, metrics)
         VALUES
           ($2, $1, 'power_grid', 'Substation', 'SUB-01', '220kV 城北变', '{\"load_pct\":94.2}'::jsonb),
           ($3, $1, 'power_grid', 'Feeder', 'FDR-01', '城北一馈线', '{\"current_a\":410.0}'::jsonb),
           ($4, $1, 'power_trial', 'TrialNode', 'TR-01', '沙盒节点', '{}'::jsonb)",
    )
    .bind(tenant_a)
    .bind(node1)
    .bind(node2)
    .bind(trial_node)
    .execute(owner_pool)
    .await?;

    let link1 = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO xb_links (id, tenant_id, subgraph_slug, link_type, src_node_id, dst_node_id)
         VALUES ($1, $2, 'power_grid', 'FEEDS_LINE', $3, $4)",
    )
    .bind(link1)
    .bind(tenant_a)
    .bind(node1)
    .bind(node2)
    .execute(owner_pool)
    .await?;

    seed_tenant_a_datasets_and_series(owner_pool, tenant_a, node1, link1).await?;
    Ok((node1, link1))
}

async fn seed_tenant_a_datasets_and_series(
    owner_pool: &PgPool,
    tenant_a: &str,
    node1: Uuid,
    link1: Uuid,
) -> TestResult {
    let ds1 = Uuid::new_v4();
    let ds2 = Uuid::new_v4();
    let lake_schema = format!("tenant_{tenant_a}");
    sqlx::query(
        "INSERT INTO ds_datasets (
             id, tenant_id, name, display_name, source_kind, sync_mode, primary_key,
             lake_schema, lake_table, row_count, continuous_stream, status
         )
         VALUES
           ($1, $3, 'scada_telemetry', 'SCADA 遥测流', 'STREAM', 'LOG_CDC', ARRAY['id'], $4, 'scada_telemetry', 1200, TRUE, 'READY'),
           ($2, $3, 'grid_assets', '配网台账表', 'FILE_TABLE', 'SNAPSHOT', '{}', $4, 'grid_assets', 350, FALSE, 'READY')",
    )
    .bind(ds1)
    .bind(ds2)
    .bind(tenant_a)
    .bind(&lake_schema)
    .execute(owner_pool)
    .await?;

    sqlx::query(
        "INSERT INTO ds_sync_jobs (
             tenant_id, dataset_id, job_id, batch_id, sync_mode, status, rows_read, rows_written, finished_at
         )
         VALUES ($1, $2, $3, $4, 'LOG_CDC', 'SUCCEEDED', 500, 480, NOW() - INTERVAL '15 minutes')",
    )
    .bind(tenant_a)
    .bind(ds1)
    .bind(Uuid::new_v4())
    .bind(Uuid::new_v4())
    .execute(owner_pool)
    .await?;

    sqlx::query(
        "INSERT INTO xb_metric_series (tenant_id, node_id, metric, ts, value, source_tag)
         VALUES
           ($1, $2, 'load_pct', NOW() - INTERVAL '10 minutes', 94.2, 'SYNC'),
           ($1, $2, 'load_pct', NOW() - INTERVAL '2 hours', 88.5, 'SYNC')",
    )
    .bind(tenant_a)
    .bind(node1)
    .execute(owner_pool)
    .await?;

    sqlx::query(
        "INSERT INTO xb_link_metric_series (tenant_id, link_id, metric_key, ts, val_num)
         VALUES ($1, $2, 'line_loss', NOW() - INTERVAL '15 minutes', 3.1)",
    )
    .bind(tenant_a)
    .bind(link1)
    .execute(owner_pool)
    .await?;

    Ok(())
}

async fn seed_tenant_a_ops_and_sim(
    owner_pool: &PgPool,
    tenant_a: &str,
    node1: Uuid,
) -> Result<SeededTenantAssets, Box<dyn std::error::Error>> {
    sqlx::query(
        "INSERT INTO xb_runtime_certifications (
             tenant_id, subgraph_slug, schema_version, cert_level, readiness_score, trial_tick_passed
         )
         VALUES ($1, 'power_grid', 2, 'L4_CERTIFIED', 96, TRUE)",
    )
    .bind(tenant_a)
    .execute(owner_pool)
    .await?;

    let dashboard_slug = String::from("power-grid-cockpit");
    sqlx::query(
        "INSERT INTO xv_dashboards (tenant_id, name, slug, status)
         VALUES ($1, '配网调度驾驶舱', $2, 'PUBLISHED')",
    )
    .bind(tenant_a)
    .bind(&dashboard_slug)
    .execute(owner_pool)
    .await?;

    let rule_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO ax_rules (id, tenant_id, name, scope_kind, target, condition, severity, title_template)
         VALUES ($1, $2, 'transformer_overload', 'METRIC', '{\"type_name\":\"Substation\"}'::jsonb, '{}'::jsonb, 'CRITICAL', '主变负载率越限')",
    )
    .bind(rule_id)
    .bind(tenant_a)
    .execute(owner_pool)
    .await?;

    let alert_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO ax_alerts (id, tenant_id, rule_id, dedup_key, severity, status, title, node_id, last_value, occurrences)
         VALUES ($1, $2, $3, 'sub-01-overload', 'CRITICAL', 'OPEN', '220kV 城北变 主变负载率越限', $4, 94.2, 3)",
    )
    .bind(alert_id)
    .bind(tenant_a)
    .bind(rule_id)
    .bind(node1)
    .execute(owner_pool)
    .await?;

    let approval_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO ax_approvals (id, tenant_id, source_kind, source_ref, title, status)
         VALUES ($1, $2, 'OBJECT_ACTION', 'Substation:transfer_load', '城北变负荷跨站转供审批', 'PENDING')",
    )
    .bind(approval_id)
    .bind(tenant_a)
    .execute(owner_pool)
    .await?;

    let sandbox_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO ax_sandboxes (id, tenant_id, name, scope, status, node_count, link_count)
         VALUES ($1, $2, 'power_grid 晚高峰负荷推演沙盘', '{\"type_names\":[\"Substation\",\"Feeder\"]}'::jsonb, 'READY', 2, 1)",
    )
    .bind(sandbox_id)
    .bind(tenant_a)
    .execute(owner_pool)
    .await?;

    let scenario_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO ax_sim_scenarios (
             id, tenant_id, sandbox_id, name, actions, params, overlay_hash,
             result, deadline_report, status
         )
         VALUES (
             $1, $2, $3, '主变跳闸 N-1 晚高峰推演', '[]'::jsonb, '{}'::jsonb, 'h1',
             '{\"affected_count\": 14}'::jsonb,
             '{\"all_satisfied\": false, \"earliest_breach_step\": 6, \"earliest_deadline_step\": 3, \"status\": \"ACTIONABLE_WINDOW\"}'::jsonb,
             'SUCCEEDED'
         )",
    )
    .bind(scenario_id)
    .bind(tenant_a)
    .bind(sandbox_id)
    .execute(owner_pool)
    .await?;

    Ok(SeededTenantAssets {
        subgraph_slug: String::from("power_grid"),
        dashboard_slug,
        sandbox_id,
        scenario_id,
        alert_id,
        approval_id,
    })
}

async fn cleanup_test_tenants(owner_pool: &PgPool, tenants: &[&str]) -> TestResult {
    for tenant in tenants {
        for stmt in [
            "DELETE FROM ax_sim_scenarios WHERE tenant_id = $1",
            "DELETE FROM ax_sandboxes WHERE tenant_id = $1",
            "DELETE FROM ax_approvals WHERE tenant_id = $1",
            "DELETE FROM ax_alerts WHERE tenant_id = $1",
            "DELETE FROM ax_rules WHERE tenant_id = $1",
            "DELETE FROM xv_dashboards WHERE tenant_id = $1",
            "DELETE FROM xb_runtime_certifications WHERE tenant_id = $1",
            "DELETE FROM xb_link_metric_series WHERE tenant_id = $1",
            "DELETE FROM xb_metric_series WHERE tenant_id = $1",
            "DELETE FROM ds_sync_jobs WHERE tenant_id = $1",
            "DELETE FROM ds_datasets WHERE tenant_id = $1",
            "DELETE FROM xb_links WHERE tenant_id = $1",
            "DELETE FROM xb_nodes WHERE tenant_id = $1",
            "DELETE FROM xb_edge_types WHERE tenant_id = $1",
            "DELETE FROM xb_node_types WHERE tenant_id = $1",
            "DELETE FROM xb_schema_heads WHERE tenant_id = $1",
            "DELETE FROM xb_subgraphs WHERE tenant_id = $1",
            "DELETE FROM sec_api_tokens WHERE tenant_id = $1",
            "DELETE FROM sec_user_roles WHERE tenant_id = $1",
            "DELETE FROM sec_users WHERE tenant_id = $1",
            "DELETE FROM sec_roles WHERE tenant_id = $1",
            "DELETE FROM sec_tenants WHERE tenant_id = $1",
        ] {
            sqlx::query(stmt).bind(tenant).execute(owner_pool).await?;
        }
    }
    Ok(())
}

fn assert_tenant_a_pulse(pulse: &PlatformPulseResponse, expected: &SeededTenantAssets) {
    assert_eq!(pulse.totals.subgraph_count, 1);
    assert_eq!(pulse.totals.node_type_count, 2);
    assert_eq!(pulse.totals.node_count, 2);
    assert_eq!(pulse.totals.link_count, 1);
    assert_eq!(pulse.totals.dataset_count, 2);
    assert_eq!(pulse.totals.lake_total_rows, 1550);
    assert_eq!(pulse.totals.metric_points_24h, 3);
    assert_eq!(pulse.totals.active_stream_count, 1);
    assert_eq!(pulse.totals.open_alert_count, 1);
    assert_eq!(pulse.totals.pending_approval_count, 1);

    assert_eq!(pulse.hourly_series.len(), 24);
    let series_points_sum: i64 = pulse.hourly_series.iter().map(|b| b.metric_points).sum();
    let series_rows_sum: i64 = pulse.hourly_series.iter().map(|b| b.synced_rows).sum();
    assert_eq!(series_points_sum, 3);
    assert_eq!(series_rows_sum, 480);

    let prod_sg = pulse
        .subgraphs
        .iter()
        .find(|s| s.slug == expected.subgraph_slug);
    assert!(prod_sg.is_some());
    if let Some(sg) = prod_sg {
        assert!(!sg.is_dev_sandbox);
        assert_eq!(sg.schema_version, 2);
        assert_eq!(sg.node_type_count, 2);
        assert_eq!(sg.edge_type_count, 1);
        assert_eq!(sg.node_count, 2);
        assert_eq!(sg.link_count, 1);
        assert_eq!(sg.alert_count, 1);
        assert_eq!(sg.cert_level, Some(RuntimeCertLevel::L4Certified));
        assert_eq!(sg.readiness_score, Some(96));
        assert_eq!(
            sg.business_domains,
            vec![String::from("调度域"), String::from("输配电域")]
        );
        assert_eq!(
            sg.primary_dashboard_slug.as_deref(),
            Some(expected.dashboard_slug.as_str())
        );
        assert_eq!(sg.latest_sandbox_id, Some(expected.sandbox_id));
    }

    assert_eq!(pulse.active_alerts.len(), 1);
    if let Some(alert) = pulse.active_alerts.first() {
        assert_eq!(alert.id, expected.alert_id);
        assert_eq!(alert.severity, Severity::Critical);
        assert_eq!(alert.status, AlertStatus::Open);
        assert_eq!(alert.subgraph_slug.as_deref(), Some("power_grid"));
    }

    assert_eq!(pulse.pending_approvals.len(), 1);
    if let Some(appr) = pulse.pending_approvals.first() {
        assert_eq!(appr.id, expected.approval_id);
        assert_eq!(appr.source_kind, "OBJECT_ACTION");
    }

    assert_eq!(pulse.recent_simulations.len(), 1);
    if let Some(sim) = pulse.recent_simulations.first() {
        assert_eq!(sim.scenario_id, expected.scenario_id);
        assert_eq!(sim.sandbox_id, expected.sandbox_id);
        assert_eq!(sim.subgraph_slug, "power_grid");
        assert_eq!(sim.affected_count, 14);
        assert_eq!(sim.earliest_breach_step, Some(6));
        assert_eq!(sim.earliest_deadline_step, Some(3));
        assert_eq!(
            sim.deadline_status,
            Some(DeadlineWindowStatus::ActionableWindow)
        );
    }
}

#[tokio::test]
async fn pulse_api_aggregates_live_metrics_and_enforces_rls_and_rbac() -> TestResult {
    let Some((app_url, _app_pool, owner_pool)) = test_pools().await? else {
        return Ok(());
    };

    let tenant_a = unique_tenant("step32_a");
    let tenant_b = unique_tenant("step32_b");

    let token_a = provision_api_token(&owner_pool, &tenant_a, "reader_a", &["DATA_READ"]).await?;
    let token_b = provision_api_token(&owner_pool, &tenant_b, "reader_b", &["DATA_READ"]).await?;
    let token_no_read =
        provision_api_token(&owner_pool, &tenant_a, "flow_only", &["FLOW_RUN"]).await?;

    let (node1, _link1) = seed_tenant_a_graph_and_data(&owner_pool, &tenant_a).await?;
    let seeded = seed_tenant_a_ops_and_sim(&owner_pool, &tenant_a, node1).await?;

    let (base_url, _server_guard) = spawn_test_server(&app_url).await?;
    let client = reqwest::Client::new();
    let pulse_url = format!("{base_url}/api/axops/pulse");

    // 1. Missing DATA_READ permission -> 403 FORBIDDEN
    let forbidden_resp = client
        .get(&pulse_url)
        .bearer_auth(&token_no_read)
        .send()
        .await?;
    assert_eq!(forbidden_resp.status(), StatusCode::FORBIDDEN);

    // 2. Tenant A receives exact production counts, 24h series, and operational feeds
    let resp_a = client.get(&pulse_url).bearer_auth(&token_a).send().await?;
    assert_eq!(resp_a.status(), StatusCode::OK);
    let pulse_a: PlatformPulseResponse = resp_a.json().await?;
    assert_tenant_a_pulse(&pulse_a, &seeded);

    // 3. Tenant B receives zeroed totals and zero leaked assets from Tenant A
    let resp_b = client.get(&pulse_url).bearer_auth(&token_b).send().await?;
    assert_eq!(resp_b.status(), StatusCode::OK);
    let pulse_b: PlatformPulseResponse = resp_b.json().await?;
    assert_eq!(pulse_b.totals.subgraph_count, 0);
    assert_eq!(pulse_b.totals.node_type_count, 0);
    assert_eq!(pulse_b.totals.node_count, 0);
    assert_eq!(pulse_b.totals.link_count, 0);
    assert_eq!(pulse_b.totals.dataset_count, 0);
    assert_eq!(pulse_b.totals.lake_total_rows, 0);
    assert_eq!(pulse_b.totals.metric_points_24h, 0);
    assert_eq!(pulse_b.totals.active_stream_count, 0);
    assert_eq!(pulse_b.totals.open_alert_count, 0);
    assert_eq!(pulse_b.totals.pending_approval_count, 0);
    assert_eq!(pulse_b.hourly_series.len(), 24);
    assert!(
        pulse_b
            .hourly_series
            .iter()
            .all(|b| b.metric_points == 0 && b.synced_rows == 0)
    );
    assert_eq!(pulse_b.subgraphs, Vec::new());
    assert_eq!(pulse_b.active_alerts, Vec::new());
    assert_eq!(pulse_b.pending_approvals, Vec::new());
    assert_eq!(pulse_b.recent_simulations, Vec::new());

    cleanup_test_tenants(&owner_pool, &[&tenant_a, &tenant_b]).await?;
    Ok(())
}
