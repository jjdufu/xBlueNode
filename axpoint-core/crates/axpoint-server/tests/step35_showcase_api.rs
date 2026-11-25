//! Step 35 integration tests for `POST /api/axops/demo/showcase/load` verifying
//! `401` unauthenticated rejection, `403 PERMISSION_DENIED` RBAC enforcement for
//! non-`OPS_ADMIN` roles (`ANALYST`, `VIEWER`) and `BUILDER`-scoped tokens, and
//! `200` `ShowcaseLoadResponse` + `TEMPLATE_INSTALL` hash-chained audit logging.

use std::net::TcpListener;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use axpoint_domain::ops::ShowcaseLoadResponse;
use axpoint_security::sha256_hex;
use reqwest::StatusCode;
use sqlx::PgPool;
use sqlx::Row as _;
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

async fn test_pools() -> Result<Option<(String, PgPool)>, Box<dyn std::error::Error>> {
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
    drop(app_pool);

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

    Ok(Some((app_url, owner_pool)))
}

fn unique_tenant(prefix: &str) -> String {
    let hex_id = Uuid::new_v4().simple().to_string();
    let short: String = hex_id.chars().take(10).collect();
    format!("{prefix}_{short}")
}

fn write_stub_seed_script(dir: &Path) -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
    let scripts_dir = dir.join("scripts");
    std::fs::create_dir_all(&scripts_dir)?;
    let script_path = scripts_dir.join("seed-showcase.sh");
    std::fs::write(
        &script_path,
        "#!/usr/bin/env bash\nset -euo pipefail\n: \"${SHOWCASE_TENANT_ID:?}\"\n: \"${CORE_BASE_URL:?}\"\n: \"${AXPOINT_DEV_TOKEN:?}\"\necho \"stub showcase ok\"\n",
    )?;
    Ok(script_path)
}

async fn spawn_test_server(
    app_url: &str,
    seed_script: &Path,
) -> Result<(String, ServerGuard), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let bind_addr = listener.local_addr()?;
    drop(listener);

    let metrics_listener = TcpListener::bind("127.0.0.1:0")?;
    let metrics_addr = metrics_listener.local_addr()?;
    drop(metrics_listener);

    let data_root = std::env::temp_dir().join(format!("axpoint_step35_test_{}", Uuid::new_v4()));
    std::fs::create_dir_all(&data_root)?;

    let child = Command::new(env!("CARGO_BIN_EXE_axpoint-server"))
        .env_remove("OTEL_EXPORTER_OTLP_ENDPOINT")
        .env("AXPOINT_ENV", "test")
        .env("DATABASE_URL", app_url)
        .env("AXPOINT_BIND_ADDR", bind_addr.to_string())
        .env("AXPOINT_METRICS_ADDR", metrics_addr.to_string())
        .env("AXPOINT_DATA_ROOT", &data_root)
        .env("AXPOINT_SHOWCASE_SEED_SCRIPT", seed_script)
        .env("PARSER_BASE_URL", "http://127.0.0.1:9")
        .env(
            "AXPOINT_DEV_TOKEN",
            "0123456789abcdef0123456789abcdef_step35_dev",
        )
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;

    let guard = ServerGuard { child };
    let base_url = format!("http://{bind_addr}");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
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
    token_profile: &str,
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
    let raw_token = format!("axp_step3500_{raw_secret}");
    let token_hash = sha256_hex(&raw_token);
    sqlx::query(
        "INSERT INTO sec_api_tokens (
             tenant_id, user_id, name, token_prefix, token_hash, scopes, token_profile
         )
         VALUES ($1, $2, $3, 'step3500', $4, $5, $6)",
    )
    .bind(tenant_id)
    .bind(user_id)
    .bind(username)
    .bind(&token_hash)
    .bind(&perms_vec)
    .bind(token_profile)
    .execute(owner_pool)
    .await?;

    Ok(raw_token)
}

async fn seed_minimal_pulse_fixture(owner_pool: &PgPool, tenant_id: &str) -> TestResult {
    sqlx::query(
        "INSERT INTO xb_subgraphs (tenant_id, slug, display_name, description, is_main, is_dev_sandbox)
         VALUES ($1, 'power_grid', '新型电力与配网调度', '测试子图', TRUE, FALSE)",
    )
    .bind(tenant_id)
    .execute(owner_pool)
    .await?;

    sqlx::query(
        "INSERT INTO xb_node_types (tenant_id, subgraph_slug, name, display_name, domain_group)
         VALUES ($1, 'power_grid', 'Substation', '变电站', '输变电域')",
    )
    .bind(tenant_id)
    .execute(owner_pool)
    .await?;

    let n1 = Uuid::new_v4();
    let n2 = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO xb_nodes (id, tenant_id, subgraph_slug, type_name, node_key, title, metrics)
         VALUES
           ($2, $1, 'power_grid', 'Substation', 'SUB-01', '城北变', '{}'::jsonb),
           ($3, $1, 'power_grid', 'Substation', 'SUB-02', '城西变', '{}'::jsonb)",
    )
    .bind(tenant_id)
    .bind(n1)
    .bind(n2)
    .execute(owner_pool)
    .await?;

    sqlx::query(
        "INSERT INTO xb_edge_types (tenant_id, subgraph_slug, name, src_type_name, dst_type_name)
         VALUES ($1, 'power_grid', 'INTERCONNECTS', 'Substation', 'Substation')",
    )
    .bind(tenant_id)
    .execute(owner_pool)
    .await?;

    sqlx::query(
        "INSERT INTO xb_links (id, tenant_id, subgraph_slug, link_type, src_node_id, dst_node_id)
         VALUES ($1, $2, 'power_grid', 'INTERCONNECTS', $3, $4)",
    )
    .bind(Uuid::new_v4())
    .bind(tenant_id)
    .bind(n1)
    .bind(n2)
    .execute(owner_pool)
    .await?;

    let lake_schema = format!("tenant_{tenant_id}");
    sqlx::query(
        "INSERT INTO ds_datasets (
             id, tenant_id, name, display_name, source_kind, sync_mode, primary_key,
             lake_schema, lake_table, row_count, status
         )
         VALUES ($1, $2, 'grid_feeder_telemetry', '遥测明细', 'FILE_TABLE', 'SNAPSHOT', '{}', $3, 'grid_feeder_telemetry', 43200, 'READY')",
    )
    .bind(Uuid::new_v4())
    .bind(tenant_id)
    .bind(&lake_schema)
    .execute(owner_pool)
    .await?;

    sqlx::query(
        "INSERT INTO xb_metric_series (tenant_id, node_id, metric, ts, value, source_tag)
         VALUES ($1, $2, 'load_ratio', NOW() - INTERVAL '5 minutes', 0.88, 'SYNC')",
    )
    .bind(tenant_id)
    .bind(n1)
    .execute(owner_pool)
    .await?;

    Ok(())
}

async fn cleanup_tenant(owner_pool: &PgPool, tenant_id: &str) -> TestResult {
    for stmt in [
        "DELETE FROM sec_audit_logs WHERE tenant_id = $1",
        "DELETE FROM xb_metric_series WHERE tenant_id = $1",
        "DELETE FROM ds_datasets WHERE tenant_id = $1",
        "DELETE FROM xb_links WHERE tenant_id = $1",
        "DELETE FROM xb_edge_types WHERE tenant_id = $1",
        "DELETE FROM xb_nodes WHERE tenant_id = $1",
        "DELETE FROM xb_node_types WHERE tenant_id = $1",
        "DELETE FROM xb_subgraphs WHERE tenant_id = $1",
        "DELETE FROM sec_api_tokens WHERE tenant_id = $1",
        "DELETE FROM sec_user_roles WHERE tenant_id = $1",
        "DELETE FROM sec_users WHERE tenant_id = $1",
        "DELETE FROM sec_roles WHERE tenant_id = $1",
        "DELETE FROM sec_tenants WHERE tenant_id = $1",
    ] {
        sqlx::query(stmt)
            .bind(tenant_id)
            .execute(owner_pool)
            .await?;
    }
    Ok(())
}

async fn assert_showcase_rejections(
    client: &reqwest::Client,
    endpoint: &str,
    analyst_token: &str,
    viewer_token: &str,
    builder_token: &str,
) -> TestResult {
    // 1. Unauthenticated -> 401 UNAUTHORIZED
    let unauth_resp = client
        .post(endpoint)
        .json(&serde_json::json!({}))
        .send()
        .await?;
    assert_eq!(unauth_resp.status(), StatusCode::UNAUTHORIZED);

    // 2. ANALYST without OPS_ADMIN -> 403 PERMISSION_DENIED
    let analyst_resp = client
        .post(endpoint)
        .bearer_auth(analyst_token)
        .json(&serde_json::json!({}))
        .send()
        .await?;
    assert_eq!(analyst_resp.status(), StatusCode::FORBIDDEN);
    let analyst_err: serde_json::Value = analyst_resp.json().await?;
    assert_eq!(
        analyst_err
            .get("error_code")
            .and_then(serde_json::Value::as_str),
        Some("PERMISSION_DENIED")
    );

    // 3. VIEWER without OPS_ADMIN -> 403 PERMISSION_DENIED
    let viewer_resp = client
        .post(endpoint)
        .bearer_auth(viewer_token)
        .json(&serde_json::json!({}))
        .send()
        .await?;
    assert_eq!(viewer_resp.status(), StatusCode::FORBIDDEN);
    let viewer_err: serde_json::Value = viewer_resp.json().await?;
    assert_eq!(
        viewer_err
            .get("error_code")
            .and_then(serde_json::Value::as_str),
        Some("PERMISSION_DENIED")
    );

    // 4. BUILDER-scoped token -> 403 FORBIDDEN (prod mutation blocked)
    let builder_resp = client
        .post(endpoint)
        .bearer_auth(builder_token)
        .json(&serde_json::json!({}))
        .send()
        .await?;
    assert_eq!(builder_resp.status(), StatusCode::FORBIDDEN);
    Ok(())
}

async fn assert_showcase_success_and_audit(
    client: &reqwest::Client,
    endpoint: &str,
    admin_token: &str,
    owner_pool: &PgPool,
    tenant: &str,
) -> TestResult {
    let ok_resp = client
        .post(endpoint)
        .bearer_auth(admin_token)
        .json(&serde_json::json!({}))
        .send()
        .await?;
    assert_eq!(ok_resp.status(), StatusCode::OK);
    let body: ShowcaseLoadResponse = ok_resp.json().await?;
    assert_eq!(
        body.loaded_subgraphs,
        vec![
            String::from("power_grid"),
            String::from("fin_risk"),
            String::from("semi_supply"),
            String::from("med_dispatch"),
        ]
    );
    assert_eq!(body.node_count, 2);
    assert_eq!(body.link_count, 1);
    assert_eq!(body.dataset_count, 1);
    assert_eq!(body.lake_total_rows, 43_200);
    assert_eq!(body.metric_points_24h, 1);

    let audit_row = sqlx::query(
        "SELECT action, resource_ref, detail_json
           FROM sec_audit_logs
          WHERE tenant_id = $1 AND action = 'TEMPLATE_INSTALL'
          ORDER BY id DESC
          LIMIT 1",
    )
    .bind(tenant)
    .fetch_optional(owner_pool)
    .await?;
    assert!(audit_row.is_some());
    if let Some(row) = audit_row {
        let resource_ref: String = row.get("resource_ref");
        let detail: serde_json::Value = row.get("detail_json");
        assert_eq!(resource_ref, "TEMPLATE:v4_industry_showcase");
        assert_eq!(
            detail
                .get("resource_kind")
                .and_then(serde_json::Value::as_str),
            Some("TEMPLATE")
        );
        assert_eq!(
            detail
                .get("resource_id")
                .and_then(serde_json::Value::as_str),
            Some("v4_industry_showcase")
        );
    }
    Ok(())
}

#[tokio::test]
async fn showcase_load_endpoint_enforces_auth_rbac_and_writes_template_audit() -> TestResult {
    let Some((app_url, owner_pool)) = test_pools().await? else {
        return Ok(());
    };

    let tenant = unique_tenant("step35_sc");
    let analyst_token = provision_api_token(
        &owner_pool,
        &tenant,
        "analyst_u",
        &["DATA_READ", "SIM_RUN", "FLOW_RUN"],
        "GENERAL",
    )
    .await?;
    let viewer_token =
        provision_api_token(&owner_pool, &tenant, "viewer_u", &["DATA_READ"], "GENERAL").await?;
    let builder_token = provision_api_token(
        &owner_pool,
        &tenant,
        "builder_u",
        &["DATA_READ", "OPS_ADMIN"],
        "BUILDER",
    )
    .await?;
    let admin_token = provision_api_token(
        &owner_pool,
        &tenant,
        "ops_admin_u",
        &["DATA_READ", "OPS_ADMIN"],
        "GENERAL",
    )
    .await?;

    seed_minimal_pulse_fixture(&owner_pool, &tenant).await?;

    let stub_root = std::env::temp_dir().join(format!("axpoint_step35_stub_{}", Uuid::new_v4()));
    let stub_script = write_stub_seed_script(&stub_root)?;
    let (base_url, _guard) = spawn_test_server(&app_url, &stub_script).await?;
    let client = reqwest::Client::new();
    let endpoint = format!("{base_url}/api/axops/demo/showcase/load");

    assert_showcase_rejections(
        &client,
        &endpoint,
        &analyst_token,
        &viewer_token,
        &builder_token,
    )
    .await?;
    assert_showcase_success_and_audit(&client, &endpoint, &admin_token, &owner_pool, &tenant)
        .await?;

    if let Err(err) = std::fs::remove_dir_all(&stub_root) {
        tracing::debug!(error = %err, "stub showcase dir cleanup skipped");
    }
    cleanup_tenant(&owner_pool, &tenant).await?;
    Ok(())
}
