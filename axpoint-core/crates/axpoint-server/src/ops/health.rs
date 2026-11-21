//! Deep operational health probes (`GET /api/axops/health/deep`, `§14.4.3`) and runtime system info.

use std::path::Path;
use std::time::Duration;

use axpoint_analytics::SessionPool;
use axpoint_domain::agent::LlmProviderRecord;
use axpoint_domain::enums::Purpose;
use axpoint_domain::ops::{
    DatabaseHealth, DeepHealthReport, DiskHealth, EmbedHealth, JobsHealthStats, LakeHealth,
    LlmHealth, MigrationsHealth, OutboxStats, ParserHealth, SystemInfoResponse,
};
use axpoint_domain::ports::{AgentRepository, OpsRepository};
use serde::Deserialize;

use super::OpsServices;
use super::templates::discover_packs_count;

/// Per-component timeout enforced on every deep health check (`§14.4.3`: 3 seconds).
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Deserialize)]
struct SidecarHealthBody {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    version: Option<String>,
}

/// Returns the total count of embedded `SQL` migration files in `deploy/migrations/`.
#[must_use]
pub fn embedded_migrations_count() -> usize {
    sqlx::migrate!("../../../deploy/migrations")
        .migrations
        .len()
}

/// Returns `true` when an `AXPOINT_LLM_CASSETTE` environment value enables offline cassette replay.
#[must_use]
pub fn is_cassette_mode_value(raw: Option<&str>) -> bool {
    raw.is_some_and(|s| !s.trim().is_empty())
}

/// Returns `true` when `AXPOINT_LLM_CASSETTE` is configured in the current process environment (`D4`).
#[must_use]
pub fn is_cassette_enabled() -> bool {
    let val = std::env::var("AXPOINT_LLM_CASSETTE").ok();
    is_cassette_mode_value(val.as_deref())
}

/// Parses available free bytes from `POSIX` `df -PB1 <path>` stdout without `unsafe` `libc` calls.
#[must_use]
pub fn parse_df_free_bytes(stdout: &str) -> Option<u64> {
    let mut lines = stdout.lines().map(str::trim).filter(|l| !l.is_empty());
    let _header = lines.next()?;
    let first_data = lines.next()?;
    let tokens: Vec<&str> = first_data.split_whitespace().collect();
    if let Some(avail) = tokens.get(3).and_then(|s| s.parse::<u64>().ok()) {
        return Some(avail);
    }
    let wrapped_line = lines.next()?;
    let wrapped_tokens: Vec<&str> = wrapped_line.split_whitespace().collect();
    wrapped_tokens.get(2).and_then(|s| s.parse::<u64>().ok())
}

impl<S, P> OpsServices<S, P> {
    /// Returns static and runtime metadata for `GET /api/axops/system/info` (`§14.8`).
    #[must_use]
    pub fn get_system_info(&self, tenant_id: &str) -> SystemInfoResponse {
        let env_mode = std::env::var("AXPOINT_ENV").unwrap_or_else(|_| String::from("development"));
        SystemInfoResponse {
            version: String::from(env!("CARGO_PKG_VERSION")),
            env: env_mode,
            tenant_id: String::from(tenant_id),
            uptime_s: self.started_at.elapsed().as_secs(),
            data_root: self.data_root.to_string_lossy().into_owned(),
            template_packs_count: discover_packs_count(),
        }
    }
}

impl<S, P> OpsServices<S, P>
where
    S: OpsRepository + AgentRepository,
{
    /// Executes all independent 3-second component probes concurrently for `GET /api/axops/health/deep` (`§14.4.3`).
    pub async fn probe_deep_health(&self, tenant_id: &str) -> DeepHealthReport {
        let cassette = is_cassette_enabled();
        let expected_migs = embedded_migrations_count();

        let (database, parser, lake, llm, embed, disk, jobs, outbox, migrations) = tokio::join!(
            probe_database(&self.store),
            probe_parser(&self.http, &self.parser_base_url),
            probe_lake(&self.analytics_pool, tenant_id),
            probe_llm(&self.store, &self.http, tenant_id, cassette),
            probe_embed(&self.store, &self.http, tenant_id, cassette),
            probe_disk(&self.data_root),
            probe_jobs(&self.store, tenant_id),
            probe_outbox(&self.store, tenant_id),
            probe_migrations(&self.store, expected_migs),
        );

        DeepHealthReport {
            database,
            parser,
            lake,
            llm,
            embed,
            disk,
            jobs,
            outbox,
            migrations,
            version: String::from(env!("CARGO_PKG_VERSION")),
        }
    }
}

async fn probe_database<S: OpsRepository>(store: &S) -> DatabaseHealth {
    match tokio::time::timeout(PROBE_TIMEOUT, store.probe_database_health()).await {
        Ok(Ok(health)) => health,
        Ok(Err(err)) => DatabaseHealth {
            ok: false,
            latency_ms: 0,
            replication_lag_s: None,
            error: Some(err.to_string()),
        },
        Err(_) => DatabaseHealth {
            ok: false,
            latency_ms: 3_000,
            replication_lag_s: None,
            error: Some(String::from("database probe timed out after 3s")),
        },
    }
}

async fn probe_parser(http: &reqwest::Client, parser_base_url: &str) -> ParserHealth {
    let url = format!("{}/health", parser_base_url.trim_end_matches('/'));
    let fut = async {
        let resp = http
            .get(&url)
            .timeout(PROBE_TIMEOUT)
            .send()
            .await
            .map_err(|err| format!("sidecar /health request failed: {err}"))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(format!("sidecar /health returned HTTP {status}"));
        }
        let body = resp.json::<SidecarHealthBody>().await.ok();
        if let Some(ref b) = body
            && b.status
                .as_deref()
                .is_some_and(|st| st.eq_ignore_ascii_case("error"))
        {
            return Err(String::from("sidecar reported status=error"));
        }
        let version = body
            .and_then(|b| b.version)
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| String::from("0.1.0"));
        Ok(version)
    };

    match tokio::time::timeout(PROBE_TIMEOUT, fut).await {
        Ok(Ok(version)) => ParserHealth {
            ok: true,
            version,
            error: None,
        },
        Ok(Err(err)) => ParserHealth {
            ok: false,
            version: String::new(),
            error: Some(err),
        },
        Err(_) => ParserHealth {
            ok: false,
            version: String::new(),
            error: Some(String::from("sidecar /health probe timed out after 3s")),
        },
    }
}

async fn probe_lake(analytics_pool: &SessionPool, tenant_id: &str) -> LakeHealth {
    let sql = "SELECT COALESCE(MAX(snapshot_id), 0) AS snapshot_id FROM ducklake_snapshots('lake')";
    match tokio::time::timeout(
        PROBE_TIMEOUT,
        analytics_pool.run_internal(tenant_id, sql, 1),
    )
    .await
    {
        Ok(Ok(res)) => {
            let snapshot_id = res
                .rows
                .first()
                .and_then(|row| row.get("snapshot_id"))
                .and_then(|val| {
                    val.as_i64()
                        .or_else(|| val.as_str().and_then(|s| s.parse::<i64>().ok()))
                })
                .unwrap_or(0);
            LakeHealth {
                ok: true,
                snapshot_id,
                error: None,
            }
        }
        Ok(Err(err)) => LakeHealth {
            ok: false,
            snapshot_id: 0,
            error: Some(err.to_string()),
        },
        Err(_) => LakeHealth {
            ok: false,
            snapshot_id: 0,
            error: Some(String::from("lake catalog probe timed out after 3s")),
        },
    }
}

fn select_active_provider(
    providers: &[LlmProviderRecord],
    want_embed: bool,
) -> Option<&LlmProviderRecord> {
    providers.iter().find(|p| {
        p.enabled
            && if want_embed {
                p.purposes.contains(&Purpose::Embed)
            } else {
                p.purposes.is_empty() || p.purposes.iter().any(|purp| *purp != Purpose::Embed)
            }
    })
}

async fn probe_gateway_models_endpoint(
    http: &reqwest::Client,
    base_url: &str,
    api_key: Option<&str>,
) -> Result<(), String> {
    let trimmed = base_url.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return Err(String::from("API base URL is empty"));
    }
    let url = format!("{trimmed}/models");
    let mut req = http.get(&url).timeout(PROBE_TIMEOUT);
    if let Some(key) = api_key.map(str::trim).filter(|k| !k.is_empty()) {
        req = req.bearer_auth(key);
    }
    let resp = req
        .send()
        .await
        .map_err(|err| format!("gateway probe failed: {err}"))?;
    let code = resp.status();
    if code.is_success() || (api_key.is_none() && code.as_u16() == 401) {
        Ok(())
    } else {
        Err(format!("gateway returned HTTP {code}"))
    }
}

async fn probe_llm<S: AgentRepository>(
    store: &S,
    http: &reqwest::Client,
    tenant_id: &str,
    cassette: bool,
) -> LlmHealth {
    if cassette {
        return LlmHealth {
            ok: true,
            provider: String::from("cassette"),
            error: None,
        };
    }

    let fut = async {
        let providers = store
            .list_llm_providers(tenant_id)
            .await
            .unwrap_or_default();
        if let Some(active) = select_active_provider(&providers, false) {
            let probe_res = probe_gateway_models_endpoint(http, &active.base_url, None).await;
            return match probe_res {
                Ok(()) => LlmHealth {
                    ok: true,
                    provider: active.name.clone(),
                    error: None,
                },
                Err(err) => LlmHealth {
                    ok: false,
                    provider: active.name.clone(),
                    error: Some(err),
                },
            };
        }

        let env_base = std::env::var("LLM_API_BASE").unwrap_or_default();
        let env_key = std::env::var("LLM_API_KEY")
            .ok()
            .filter(|s| !s.trim().is_empty());
        if env_key.is_none() {
            return LlmHealth {
                ok: false,
                provider: String::from("env_default"),
                error: Some(String::from("LLM_API_KEY is not configured")),
            };
        }
        match probe_gateway_models_endpoint(http, &env_base, env_key.as_deref()).await {
            Ok(()) => LlmHealth {
                ok: true,
                provider: String::from("env_default"),
                error: None,
            },
            Err(err) => LlmHealth {
                ok: false,
                provider: String::from("env_default"),
                error: Some(err),
            },
        }
    };

    if let Ok(res) = tokio::time::timeout(PROBE_TIMEOUT, fut).await {
        res
    } else {
        LlmHealth {
            ok: false,
            provider: String::from("timeout"),
            error: Some(String::from("LLM probe timed out after 3s")),
        }
    }
}

async fn probe_embed<S: AgentRepository>(
    store: &S,
    http: &reqwest::Client,
    tenant_id: &str,
    cassette: bool,
) -> EmbedHealth {
    if cassette {
        return EmbedHealth {
            ok: true,
            error: None,
        };
    }

    let fut = async {
        let providers = store
            .list_llm_providers(tenant_id)
            .await
            .unwrap_or_default();
        if let Some(active) = select_active_provider(&providers, true) {
            return match probe_gateway_models_endpoint(http, &active.base_url, None).await {
                Ok(()) => EmbedHealth {
                    ok: true,
                    error: None,
                },
                Err(err) => EmbedHealth {
                    ok: false,
                    error: Some(err),
                },
            };
        }

        let env_base = std::env::var("EMBED_API_BASE")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| std::env::var("LLM_API_BASE").ok())
            .unwrap_or_default();
        let env_key = std::env::var("EMBED_API_KEY")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| {
                std::env::var("LLM_API_KEY")
                    .ok()
                    .filter(|s| !s.trim().is_empty())
            });
        if env_key.is_none() {
            return EmbedHealth {
                ok: false,
                error: Some(String::from("EMBED_API_KEY is not configured")),
            };
        }
        match probe_gateway_models_endpoint(http, &env_base, env_key.as_deref()).await {
            Ok(()) => EmbedHealth {
                ok: true,
                error: None,
            },
            Err(err) => EmbedHealth {
                ok: false,
                error: Some(err),
            },
        }
    };

    if let Ok(res) = tokio::time::timeout(PROBE_TIMEOUT, fut).await {
        res
    } else {
        EmbedHealth {
            ok: false,
            error: Some(String::from("embedding probe timed out after 3s")),
        }
    }
}

async fn probe_disk(data_root: &Path) -> DiskHealth {
    let fut = async {
        if !data_root.exists() {
            let _created = tokio::fs::create_dir_all(data_root).await.is_ok();
        }
        let target = if data_root.exists() {
            data_root
        } else {
            data_root.parent().unwrap_or_else(|| Path::new("/"))
        };
        let out = tokio::process::Command::new("df")
            .arg("-PB1")
            .arg(target)
            .output()
            .await
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&out.stdout);
        parse_df_free_bytes(&text)
    };

    match tokio::time::timeout(PROBE_TIMEOUT, fut).await {
        Ok(Some(free_bytes)) => DiskHealth {
            ok: free_bytes > 0,
            data_root_free_bytes: free_bytes,
        },
        _ => DiskHealth {
            ok: false,
            data_root_free_bytes: 0,
        },
    }
}

async fn probe_jobs<S: OpsRepository>(store: &S, tenant_id: &str) -> JobsHealthStats {
    match tokio::time::timeout(PROBE_TIMEOUT, store.query_jobs_health_stats(tenant_id)).await {
        Ok(Ok(stats)) => stats,
        _ => JobsHealthStats::default(),
    }
}

async fn probe_outbox<S: OpsRepository>(store: &S, tenant_id: &str) -> OutboxStats {
    match tokio::time::timeout(PROBE_TIMEOUT, store.query_outbox_stats(tenant_id)).await {
        Ok(Ok(stats)) => stats,
        _ => OutboxStats::default(),
    }
}

async fn probe_migrations<S: OpsRepository>(store: &S, expected_total: usize) -> MigrationsHealth {
    match tokio::time::timeout(PROBE_TIMEOUT, store.query_migrations_health(expected_total)).await {
        Ok(Ok(migs)) => migs,
        _ => MigrationsHealth {
            ok: false,
            current: 0,
            pending: expected_total,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_migrations_count_includes_step14() {
        assert!(embedded_migrations_count() >= 14);
    }

    #[test]
    fn cassette_mode_detection_trims_whitespace() {
        assert!(!is_cassette_mode_value(None));
        assert!(!is_cassette_mode_value(Some("")));
        assert!(!is_cassette_mode_value(Some("   ")));
        assert!(is_cassette_mode_value(Some(".gate/cassette")));
    }

    #[test]
    fn parse_df_free_bytes_handles_single_and_wrapped_lines() {
        let posix_single = "\
Filesystem     1048576-blocks        Used   Available Capacity Mounted on
/dev/sda1        107374182400 21474836480 85899345920      20% /var/lib/axpoint
";
        assert_eq!(parse_df_free_bytes(posix_single), Some(85_899_345_920));

        let posix_wrapped = "\
Filesystem     1-blocks Used Available Capacity Mounted on
/dev/mapper/very-long-volume-group-name
               50000000 10000000 40000000 20% /data
";
        assert_eq!(parse_df_free_bytes(posix_wrapped), Some(40_000_000));
        assert_eq!(parse_df_free_bytes(""), None);
        assert_eq!(parse_df_free_bytes("header only"), None);
    }

    #[tokio::test]
    async fn probe_disk_reports_positive_free_space_on_temp_dir() {
        let health = probe_disk(&std::env::temp_dir()).await;
        assert!(health.ok);
        assert!(health.data_root_free_bytes > 0);
    }

    #[tokio::test]
    async fn probe_parser_handles_unreachable_endpoint_cleanly() {
        let client = reqwest::Client::new();
        let res = probe_parser(&client, "http://127.0.0.1:9").await;
        assert!(!res.ok);
        assert!(res.error.is_some());
    }
}
