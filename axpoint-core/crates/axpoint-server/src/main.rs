//! `AxPoint` Core HTTP server (`Axum`) with `AxGuard` authentication, background workers, and SPA hosting.

mod agent_bridge;
mod auth;
mod background;
mod routes;
mod sidecar;
mod spa;
mod state;
mod telemetry;
mod workers;

use std::env;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axpoint_analytics::{AnalyticsConfig, SessionPool};
use axpoint_db::PgStore;
use axpoint_domain::AppError;
use axpoint_llm::{AiGateway, EmbeddingConfig, LlmConfig};
use axpoint_security::{AuditWriter, Authenticator, CredentialKeyring};
use reqwest::header::{HeaderMap, HeaderValue};
use sqlx::Connection as _;
use sqlx::PgConnection;
use sqlx::postgres::PgPoolOptions;
use tracing::info;

use crate::agent_bridge::{build_axagent_services, build_llm_router};
use crate::sidecar::HttpSidecarClient;
use crate::state::AppState;

/// Default maximum multipart upload body limit in megabytes (`AXPOINT_MAX_UPLOAD_MB`).
const DEFAULT_MAX_UPLOAD_MB: u64 = 512;

/// Minimum byte length required for `AXPOINT_INTERNAL_TOKEN` when configured.
const MIN_INTERNAL_TOKEN_LEN: usize = 32;

#[tokio::main]
async fn main() -> Result<(), AppError> {
    match env::args().nth(1).as_deref() {
        Some("healthcheck") => return run_cli_healthcheck().await,
        Some("install-duckdb-extensions") => return axpoint_analytics::install_duckdb_extensions(),
        Some("migrate") => return run_cli_migrate().await,
        _ => {}
    }

    let _dotenv_loaded = dotenvy::dotenv().is_ok();
    let otel_provider = telemetry::init_telemetry()?;
    axpoint_server::ops::validate_production_env()?;

    let database_url = env::var("DATABASE_URL")
        .map_err(|_| AppError::Config(String::from("DATABASE_URL must be set")))?;
    let bind_addr: SocketAddr = env::var("AXPOINT_BIND_ADDR")
        .unwrap_or_else(|_| String::from("0.0.0.0:9995"))
        .parse()
        .map_err(|err| AppError::Config(format!("invalid AXPOINT_BIND_ADDR: {err}")))?;
    let parser_base_url =
        env::var("PARSER_BASE_URL").unwrap_or_else(|_| String::from("http://127.0.0.1:8001"));
    let web_dist_dir =
        env::var("WEB_DIST_DIR").unwrap_or_else(|_| String::from("../xbluenode-web/dist"));
    let data_root = PathBuf::from(
        env::var("AXPOINT_DATA_ROOT").unwrap_or_else(|_| String::from("/var/lib/axpoint")),
    );

    maybe_run_migrations_on_start().await?;

    let max_upload_bytes = parse_max_upload_bytes()?;
    let state = assemble_runtime_state(&database_url, parser_base_url, data_root).await?;
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    background::spawn_background_workers(&state, &shutdown_rx);
    let shutdown_audit = state.audit.clone();

    let app = spa::build_app(state, &web_dist_dir, max_upload_bytes);

    info!("AxPoint Core listening on http://{bind_addr}");
    let listener = tokio::net::TcpListener::bind(bind_addr)
        .await
        .map_err(|err| AppError::Config(format!("failed to bind {bind_addr}: {err}")))?;

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal(shutdown_tx))
        .await
        .map_err(|err| AppError::Config(format!("server error: {err}")))?;

    if let Err(err) = shutdown_audit.flush().await {
        tracing::warn!(error = %err, "shutdown audit flush failed");
    }
    if let Some(provider) = otel_provider
        && let Err(err) = provider.shutdown()
    {
        tracing::warn!(error = %err, "opentelemetry provider shutdown failed");
    }

    Ok(())
}

async fn run_cli_healthcheck() -> Result<(), AppError> {
    let url = env::var("AXPOINT_HEALTHCHECK_URL")
        .unwrap_or_else(|_| String::from("http://127.0.0.1:9995/api/health"));
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .map_err(|err| AppError::Config(format!("healthcheck client build failed: {err}")))?;
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|err| AppError::Upstream(format!("healthcheck request failed: {err}")))?;
    if resp.status().is_success() {
        Ok(())
    } else {
        Err(AppError::Upstream(format!(
            "healthcheck returned HTTP {}",
            resp.status()
        )))
    }
}

async fn run_cli_migrate() -> Result<(), AppError> {
    let _dotenv_loaded = dotenvy::dotenv().is_ok();
    let otel_provider = telemetry::init_telemetry()?;
    let migration_url = env::var("MIGRATION_DATABASE_URL").map_err(|_| {
        AppError::Config(String::from(
            "MIGRATION_DATABASE_URL must be set for `axpoint-server migrate`",
        ))
    })?;

    let mut conn = PgConnection::connect(&migration_url)
        .await
        .map_err(|err| AppError::Database(format!("migration connection failed: {err}")))?;

    sqlx::migrate!("../../../deploy/migrations")
        .run(&mut conn)
        .await
        .map_err(|err| AppError::Database(format!("CLI migration failed: {err}")))?;

    if let Err(err) = conn.close().await {
        tracing::warn!(error = %err, "migration connection close failed");
    }
    info!("Applied embedded SQL migrations via `axpoint-server migrate`");
    if let Some(provider) = otel_provider
        && let Err(err) = provider.shutdown()
    {
        tracing::warn!(error = %err, "opentelemetry provider shutdown failed");
    }
    Ok(())
}

async fn maybe_run_migrations_on_start() -> Result<(), AppError> {
    let enabled = env::var("AXPOINT_MIGRATE_ON_START")
        .is_ok_and(|v| matches!(v.trim(), "1" | "true" | "TRUE" | "yes"));
    if !enabled {
        return Ok(());
    }

    let migration_url = env::var("MIGRATION_DATABASE_URL").map_err(|_| {
        AppError::Config(String::from(
            "MIGRATION_DATABASE_URL must be set when AXPOINT_MIGRATE_ON_START=true",
        ))
    })?;

    let mut conn = PgConnection::connect(&migration_url)
        .await
        .map_err(|err| AppError::Database(format!("migration connection failed: {err}")))?;

    sqlx::migrate!("../../../deploy/migrations")
        .run(&mut conn)
        .await
        .map_err(|err| AppError::Database(format!("startup migration failed: {err}")))?;

    if let Err(err) = conn.close().await {
        tracing::warn!(error = %err, "migration connection close failed");
    }
    info!("Applied embedded SQL migrations via MIGRATION_DATABASE_URL");
    Ok(())
}

fn parse_max_upload_bytes() -> Result<usize, AppError> {
    let mb = env::var("AXPOINT_MAX_UPLOAD_MB")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(|s| {
            s.trim()
                .parse::<u64>()
                .map_err(|err| AppError::Config(format!("invalid AXPOINT_MAX_UPLOAD_MB: {err}")))
        })
        .transpose()?
        .unwrap_or(DEFAULT_MAX_UPLOAD_MB);

    let bytes_u64 = mb
        .checked_mul(1024)
        .and_then(|v| v.checked_mul(1024))
        .ok_or_else(|| AppError::Config(String::from("AXPOINT_MAX_UPLOAD_MB overflow")))?;

    usize::try_from(bytes_u64)
        .map_err(|err| AppError::Config(format!("AXPOINT_MAX_UPLOAD_MB exceeds usize: {err}")))
}

fn build_sidecar_http_client() -> Result<reqwest::Client, AppError> {
    let mut default_headers = HeaderMap::new();
    let env_mode = env::var("AXPOINT_ENV").unwrap_or_else(|_| String::from("development"));
    let internal_token = env::var("AXPOINT_INTERNAL_TOKEN")
        .ok()
        .filter(|s| !s.is_empty());

    if env_mode.eq_ignore_ascii_case("production") && internal_token.is_none() {
        return Err(AppError::Config(String::from(
            "AXPOINT_INTERNAL_TOKEN must be set when AXPOINT_ENV=production",
        )));
    }

    if let Some(token) = internal_token {
        if token.len() < MIN_INTERNAL_TOKEN_LEN {
            return Err(AppError::Config(format!(
                "AXPOINT_INTERNAL_TOKEN must be at least {MIN_INTERNAL_TOKEN_LEN} bytes"
            )));
        }
        let val = HeaderValue::from_str(&token).map_err(|err| {
            AppError::Config(format!(
                "invalid AXPOINT_INTERNAL_TOKEN header value: {err}"
            ))
        })?;
        default_headers.insert("x-axpoint-internal-token", val);
    }

    reqwest::Client::builder()
        .default_headers(default_headers)
        .build()
        .map_err(|err| AppError::Config(format!("failed to build HTTP client: {err}")))
}

fn load_ai_configs() -> Result<(LlmConfig, EmbeddingConfig), AppError> {
    let llm_cfg = LlmConfig {
        api_base: env::var("LLM_API_BASE")
            .unwrap_or_else(|_| String::from("https://dashscope.aliyuncs.com/compatible-mode/v1")),
        api_key: env::var("LLM_API_KEY").unwrap_or_default(),
        model: env::var("LLM_MODEL").unwrap_or_else(|_| String::from("qwen-max")),
    };

    let configured_dim = env::var("EMBED_DIM")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(|s| {
            s.trim()
                .parse::<usize>()
                .map_err(|err| AppError::Config(format!("invalid EMBED_DIM: {err}")))
        })
        .transpose()?;

    let embed_cfg = EmbeddingConfig {
        api_base: env::var("EMBED_API_BASE").unwrap_or_else(|_| llm_cfg.api_base.clone()),
        api_key: env::var("EMBED_API_KEY").unwrap_or_else(|_| llm_cfg.api_key.clone()),
        model: env::var("EMBED_MODEL").unwrap_or_else(|_| String::from("text-embedding-v4")),
        configured_dim,
    };

    Ok((llm_cfg, embed_cfg))
}

fn load_analytics_config(
    data_root: &std::path::Path,
    database_url: &str,
    keyring: CredentialKeyring,
) -> Result<AnalyticsConfig, AppError> {
    let lake_catalog_dsn = env::var("AXPOINT_LAKE_CATALOG_DSN")
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty());

    let tenant_cache_capacity = env::var("AXPOINT_DUCKDB_TENANT_CACHE")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(|s| {
            s.trim().parse::<usize>().map_err(|err| {
                AppError::Config(format!("invalid AXPOINT_DUCKDB_TENANT_CACHE: {err}"))
            })
        })
        .transpose()?
        .unwrap_or(32)
        .max(1);

    let memory_limit = env::var("AXPOINT_DUCKDB_MEMORY")
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| String::from("1GB"));

    let threads = env::var("AXPOINT_DUCKDB_THREADS")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(|s| {
            s.trim()
                .parse::<u32>()
                .map_err(|err| AppError::Config(format!("invalid AXPOINT_DUCKDB_THREADS: {err}")))
        })
        .transpose()?
        .unwrap_or(4)
        .max(1);

    Ok(AnalyticsConfig {
        data_root: data_root.to_path_buf(),
        lake_catalog_dsn,
        app_database_url: Some(String::from(database_url)),
        tenant_cache_capacity,
        memory_limit,
        threads,
        keyring: Some(keyring),
    })
}

struct GuardServices {
    jwt_keys: Arc<axpoint_security::jwt::JwtKeyManager>,
    oidc: Option<Arc<axpoint_security::oidc::OidcService>>,
    cedar: Arc<axpoint_security::cedar_authz::CedarAuthorizer>,
    mask_keys: Arc<axpoint_security::masking::MaskKeyRing>,
    auth_limiters: Arc<routes::auth::AuthRateLimiters>,
}

fn load_guard_services() -> Result<GuardServices, AppError> {
    let env_mode = env::var("AXPOINT_ENV").unwrap_or_else(|_| String::from("development"));
    let jwt_keys = Arc::new(axpoint_security::jwt::JwtKeyManager::from_env());
    if env_mode.eq_ignore_ascii_case("production") {
        let _ = jwt_keys.jwks_json()?;
    }
    let oidc = axpoint_security::oidc::OidcService::from_env()?.map(Arc::new);
    let cedar = Arc::new(axpoint_security::cedar_authz::CedarAuthorizer::new()?);
    let mask_keys = Arc::new(axpoint_security::masking::MaskKeyRing::from_env()?);
    let auth_limiters = Arc::new(routes::auth::AuthRateLimiters::new());
    Ok(GuardServices {
        jwt_keys,
        oidc,
        cedar,
        mask_keys,
        auth_limiters,
    })
}

async fn assemble_runtime_state(
    database_url: &str,
    parser_base_url: String,
    data_root: PathBuf,
) -> Result<AppState, AppError> {
    let http = build_sidecar_http_client()?;
    let (llm_cfg, embed_cfg) = load_ai_configs()?;
    let keyring = Arc::new(CredentialKeyring::from_env()?);
    let guard = load_guard_services()?;
    let sidecar = Arc::new(HttpSidecarClient::new(&parser_base_url, http.clone()));
    let analytics_pool = Arc::new(SessionPool::new(load_analytics_config(
        &data_root,
        database_url,
        (*keyring).clone(),
    )?));

    let pool = PgPoolOptions::new()
        .max_connections(16)
        .connect(database_url)
        .await
        .map_err(|err| AppError::Database(format!("postgres connection failed: {err}")))?;

    let store = PgStore::new(pool);
    if let Err(err) = store.ensure_metric_partitions(24, 3).await {
        tracing::warn!(error = %err, "startup ensure_metric_partitions skipped");
    }

    let authenticator =
        Arc::new(Authenticator::from_env(store.clone())?.with_jwt(Arc::clone(&guard.jwt_keys)));
    let audit = AuditWriter::new(store.clone());
    let llm_router = build_llm_router(&store, &keyring, llm_cfg.clone(), embed_cfg.clone());
    let ai = AiGateway::new(llm_cfg, embed_cfg).with_router(Arc::clone(&llm_router));
    let sim_port: Arc<dyn axpoint_domain::ports::SimExecutionPort> =
        Arc::new(routes::axsim::ServerSimPort {
            store: store.clone(),
        });
    let axagent = build_axagent_services(&store, &llm_router, &keyring, &audit, Some(sim_port));
    let view = Arc::new(axpoint_server::view::ViewServices::new(
        store.clone(),
        Arc::clone(&analytics_pool),
        Arc::clone(&llm_router),
        data_root.clone(),
    ));
    let ops = Arc::new(axpoint_server::ops::OpsServices::new(
        store.clone(),
        Arc::clone(&sidecar),
        Arc::clone(&analytics_pool),
        audit.clone(),
        http.clone(),
        axpoint_server::ops::OpsConfig {
            parser_base_url: parser_base_url.clone(),
            data_root: data_root.clone(),
        },
    ));

    let state = AppState {
        store,
        ai,
        llm_router,
        axagent: Arc::clone(&axagent),
        authenticator,
        jwt_keys: guard.jwt_keys,
        oidc: guard.oidc,
        cedar: guard.cedar,
        mask_keys: guard.mask_keys,
        auth_limiters: guard.auth_limiters,
        audit,
        keyring,
        sidecar,
        analytics_pool,
        view,
        ops,
        data_root,
        parser_base_url,
        http,
    };
    axagent.capability.bind_state(state.clone());
    Ok(state)
}

async fn shutdown_signal(shutdown_tx: tokio::sync::watch::Sender<bool>) {
    if let Err(err) = tokio::signal::ctrl_c().await {
        tracing::warn!(error = %err, "ctrl_c signal wait failed");
    }
    let _sent = shutdown_tx.send(true).is_ok();
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::Arc;

    use axpoint_analytics::{AnalyticsConfig, SessionPool};
    use axpoint_db::PgStore;
    use axpoint_domain::AppError;
    use axpoint_llm::{AiGateway, EmbeddingConfig, LlmConfig};
    use axpoint_security::{AuditWriter, Authenticator, CredentialKeyring};
    use sqlx::postgres::PgPoolOptions;
    use uuid::Uuid;

    use crate::agent_bridge::{build_axagent_services, build_llm_router};
    use crate::sidecar::HttpSidecarClient;
    use crate::spa::build_app;
    use crate::state::AppState;

    const TEST_DEV_TOKEN: &str = "0123456789abcdef0123456789abcdef_server_test";

    fn build_test_state() -> Result<AppState, AppError> {
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://axpoint_app:secret@127.0.0.1:5432/xbluenode")
            .map_err(|err| AppError::Database(err.to_string()))?;
        let store = PgStore::new(pool);
        let jwt_keys = Arc::new(axpoint_security::jwt::JwtKeyManager::from_env());
        let authenticator = Arc::new(
            Authenticator::new(store.clone(), "test", Some(String::from(TEST_DEV_TOKEN)))?
                .with_jwt(Arc::clone(&jwt_keys)),
        );
        let cedar = Arc::new(axpoint_security::cedar_authz::CedarAuthorizer::new()?);
        let mask_keys = Arc::new(axpoint_security::masking::MaskKeyRing::dev_default());
        let auth_limiters = Arc::new(crate::routes::auth::AuthRateLimiters::new());
        let audit = AuditWriter::new(store.clone());
        let llm_cfg = LlmConfig {
            api_base: String::from("http://127.0.0.1:9"),
            api_key: String::new(),
            model: String::from("test-llm"),
        };
        let embed_cfg = EmbeddingConfig {
            api_base: String::from("http://127.0.0.1:9"),
            api_key: String::new(),
            model: String::from("test-embed"),
            configured_dim: Some(64),
        };
        let keyring = Arc::new(CredentialKeyring::ephemeral()?);
        let llm_router = build_llm_router(&store, &keyring, llm_cfg.clone(), embed_cfg.clone());
        let ai = AiGateway::new(llm_cfg, embed_cfg).with_router(Arc::clone(&llm_router));
        let sim_port: Arc<dyn axpoint_domain::ports::SimExecutionPort> =
            Arc::new(crate::routes::axsim::ServerSimPort {
                store: store.clone(),
            });
        let axagent = build_axagent_services(&store, &llm_router, &keyring, &audit, Some(sim_port));
        let http = reqwest::Client::new();
        let parser_base_url = String::from("http://127.0.0.1:8001");
        let sidecar = Arc::new(HttpSidecarClient::new(&parser_base_url, http.clone()));
        let analytics_pool = Arc::new(SessionPool::new(AnalyticsConfig::default()));
        let data_root = std::env::temp_dir();
        let view = Arc::new(axpoint_server::view::ViewServices::new(
            store.clone(),
            Arc::clone(&analytics_pool),
            Arc::clone(&llm_router),
            data_root.clone(),
        ));
        let ops = Arc::new(axpoint_server::ops::OpsServices::new(
            store.clone(),
            Arc::clone(&sidecar),
            Arc::clone(&analytics_pool),
            audit.clone(),
            http.clone(),
            axpoint_server::ops::OpsConfig {
                parser_base_url: parser_base_url.clone(),
                data_root: data_root.clone(),
            },
        ));
        let state = AppState {
            store,
            ai,
            llm_router,
            axagent: Arc::clone(&axagent),
            authenticator,
            jwt_keys,
            oidc: None,
            cedar,
            mask_keys,
            auth_limiters,
            audit,
            keyring,
            sidecar,
            analytics_pool,
            view,
            ops,
            data_root,
            parser_base_url,
            http,
        };
        axagent.capability.bind_state(state.clone());
        Ok(state)
    }

    #[tokio::test]
    async fn api_fallback_is_json_404() -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = std::env::temp_dir().join(format!("axpoint-spa-{}", Uuid::new_v4()));
        fs::create_dir_all(&temp_dir)?;
        fs::write(
            temp_dir.join("index.html"),
            "<!doctype html><title>xBlueNode</title>",
        )?;

        let dist_str = temp_dir.to_string_lossy().into_owned();
        let app = build_app(build_test_state()?, &dist_str, 8 * 1024 * 1024);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let server_task = tokio::spawn(async move {
            let _served = axum::serve(listener, app).await.is_ok();
        });

        let client = reqwest::Client::new();
        let resp = client
            .get(format!("http://{addr}/api/does-not-exist"))
            .header("Authorization", format!("Bearer {TEST_DEV_TOKEN}"))
            .send()
            .await?;

        assert_eq!(resp.status().as_u16(), 404);
        let body: serde_json::Value = resp.json().await?;
        assert_eq!(
            body.get("error_code").and_then(serde_json::Value::as_str),
            Some("ROUTE_NOT_FOUND")
        );
        let trace_id = body
            .get("trace_id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        assert_ne!(trace_id, "");

        // Also verify #[serde(deny_unknown_fields)] returns HTTP 400 INVALID_ARGUMENT JSON.
        let bad_resp = client
            .post(format!("http://{addr}/api/documents/ingest"))
            .header("Authorization", format!("Bearer {TEST_DEV_TOKEN}"))
            .json(&serde_json::json!({
                "tenant_id": "x",
                "title": "t",
                "source_name": "t.md",
                "content": "c"
            }))
            .send()
            .await?;
        assert_eq!(bad_resp.status().as_u16(), 400);
        let bad_body: serde_json::Value = bad_resp.json().await?;
        assert_eq!(
            bad_body
                .get("error_code")
                .and_then(serde_json::Value::as_str),
            Some("INVALID_ARGUMENT")
        );

        server_task.abort();
        fs::remove_dir_all(&temp_dir)?;
        Ok(())
    }

    #[tokio::test]
    async fn spa_fallback_serves_index() -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = std::env::temp_dir().join(format!("axpoint-spa-{}", Uuid::new_v4()));
        let assets_dir = temp_dir.join("assets");
        fs::create_dir_all(&assets_dir)?;
        let index_html =
            "<!doctype html><html><body><div id=\"root\">xBlueNode</div></body></html>";
        fs::write(temp_dir.join("index.html"), index_html)?;
        fs::write(assets_dir.join("app-abc123.js"), "console.log('axpoint');")?;

        let dist_str = temp_dir.to_string_lossy().into_owned();
        let app = build_app(build_test_state()?, &dist_str, 8 * 1024 * 1024);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let server_task = tokio::spawn(async move {
            let _served = axum::serve(listener, app).await.is_ok();
        });

        let client = reqwest::Client::new();
        let spa_resp = client.get(format!("http://{addr}/graph")).send().await?;
        assert_eq!(spa_resp.status().as_u16(), 200);
        assert_eq!(
            spa_resp
                .headers()
                .get("cache-control")
                .and_then(|v| v.to_str().ok()),
            Some("no-cache")
        );
        let html = spa_resp.text().await?;
        assert_eq!(html, index_html);

        let asset_resp = client
            .head(format!("http://{addr}/assets/app-abc123.js"))
            .send()
            .await?;
        assert_eq!(asset_resp.status().as_u16(), 200);
        let cc = asset_resp
            .headers()
            .get("cache-control")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        assert!(cc.contains("immutable"));

        server_task.abort();
        fs::remove_dir_all(&temp_dir)?;
        Ok(())
    }
}
