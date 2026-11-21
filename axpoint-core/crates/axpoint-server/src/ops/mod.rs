//! `AxOps` operational services: deep health probes, platform backups, industry template packs,
//! one-click demo scenario loading, delivery work items, and production startup preflight (`12-STEP14-OPERATIONS-TEMPLATES-ACCEPTANCE.md`).

pub mod backup;
pub mod demo;
pub mod health;
pub mod templates;
pub mod work_items;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use axpoint_analytics::SessionPool;
use axpoint_domain::AppError;
use axpoint_security::AuditWriter;

/// Minimum byte length required for `AXPOINT_INTERNAL_TOKEN` in production.
pub const MIN_PRODUCTION_INTERNAL_TOKEN_LEN: usize = 32;

/// Mandatory environment variables checked when `AXPOINT_ENV=production` (`§14.2.1`).
pub const REQUIRED_PRODUCTION_ENV_VARS: &[&str] = &[
    "AXPOINT_PUBLIC_HOST",
    "DATABASE_URL",
    "AXPOINT_INTERNAL_TOKEN",
    "AXPOINT_CREDENTIAL_KEYS",
    "AXPOINT_MASK_KEYS",
    "AXPOINT_JWT_PRIVATE_KEY_PATH",
    "AXPOINT_JWT_PUBLIC_KEYS_DIR",
    "LLM_API_BASE",
    "LLM_API_KEY",
    "LLM_MODEL",
    "EMBED_API_BASE",
    "EMBED_API_KEY",
    "EMBED_MODEL",
];

/// Optional database password variables also checked against placeholder values when set.
const OPTIONAL_SECRET_ENV_VARS: &[&str] = &[
    "POSTGRES_PASSWORD",
    "AXPOINT_APP_DB_PASSWORD",
    "AXPOINT_LAKE_DB_PASSWORD",
    "AXPOINT_OIDC_CLIENT_SECRET",
];

/// Forbidden example/placeholder substrings rejected in `AXPOINT_ENV=production`.
const FORBIDDEN_PLACEHOLDERS: &[&str] = &[
    "axpoint_secret",
    "axpoint_app_secret",
    "axpoint_lake_secret",
    "changeme",
    "replace_with_",
    "example.com",
];

/// Runtime filesystem and Sidecar endpoint settings for [`OpsServices`].
#[derive(Debug, Clone)]
pub struct OpsConfig {
    /// Base URL of the `axpoint-parser` Sidecar (e.g., `http://127.0.0.1:8001`).
    pub parser_base_url: String,
    /// Root filesystem directory (`AXPOINT_DATA_ROOT`).
    pub data_root: PathBuf,
}

/// Shared `AxOps` service bundle injected into `/api/axops/*` routes (`12-STEP14`).
#[derive(Clone)]
pub struct OpsServices<S, P> {
    /// Multi-domain persistence store implementing `OpsRepository` and domain ports.
    pub store: S,
    /// Sidecar client implementing `SidecarPort`.
    pub sidecar: Arc<P>,
    /// Per-tenant hardened `DuckDB` session pool.
    pub analytics_pool: Arc<SessionPool>,
    /// Asynchronous security audit writer.
    pub audit: AuditWriter,
    /// Preconfigured `HTTP` client for Sidecar `/health` and `LLM` probes.
    pub http: reqwest::Client,
    /// Normalized `axpoint-parser` base URL without trailing slash.
    pub parser_base_url: String,
    /// Configured `AXPOINT_DATA_ROOT` directory.
    pub data_root: PathBuf,
    /// Process boot instant used to report `uptime_s`.
    pub started_at: Instant,
}

impl<S, P> OpsServices<S, P> {
    /// Constructs a new [`OpsServices`] bundle and records the current process start [`Instant`].
    #[must_use]
    pub fn new(
        store: S,
        sidecar: Arc<P>,
        analytics_pool: Arc<SessionPool>,
        audit: AuditWriter,
        http: reqwest::Client,
        config: OpsConfig,
    ) -> Self {
        Self {
            store,
            sidecar,
            analytics_pool,
            audit,
            http,
            parser_base_url: config.parser_base_url.trim_end_matches('/').to_owned(),
            data_root: config.data_root,
            started_at: Instant::now(),
        }
    }
}

/// Validates process environment variables when `AXPOINT_ENV` equals `"production"` (`§14.2.1`).
///
/// # Errors
/// Returns [`AppError::Config`] listing every missing required variable, placeholder value,
/// or forbidden `AXPOINT_DEV_TOKEN` setting when `AXPOINT_ENV` is `"production"`.
pub fn validate_production_env() -> Result<(), AppError> {
    validate_production_env_with(|key| std::env::var(key).ok())
}

/// Validates an environment lookup closure under `AXPOINT_ENV=production` rules (`§14.2.1`).
///
/// Non-production environments (`development`, `test`, unset) return `Ok(())` immediately.
///
/// # Errors
/// Returns [`AppError::Config`] enumerating all missing or placeholder configuration entries.
pub fn validate_production_env_with<F>(lookup: F) -> Result<(), AppError>
where
    F: Fn(&str) -> Option<String>,
{
    let env_mode = lookup("AXPOINT_ENV").unwrap_or_else(|| String::from("development"));
    if !env_mode.trim().eq_ignore_ascii_case("production") {
        return Ok(());
    }

    let mut violations: Vec<String> = Vec::new();

    for &var_name in REQUIRED_PRODUCTION_ENV_VARS {
        match lookup(var_name).map(|v| v.trim().to_owned()) {
            None => violations.push(format!("{var_name} is missing")),
            Some(val) if val.is_empty() => violations.push(format!("{var_name} is empty")),
            Some(val) => {
                check_placeholder_violation(var_name, &val, &mut violations);
                if var_name == "AXPOINT_INTERNAL_TOKEN"
                    && val.len() < MIN_PRODUCTION_INTERNAL_TOKEN_LEN
                {
                    violations.push(format!(
                        "AXPOINT_INTERNAL_TOKEN must be at least {MIN_PRODUCTION_INTERNAL_TOKEN_LEN} bytes"
                    ));
                }
            }
        }
    }

    for &var_name in OPTIONAL_SECRET_ENV_VARS {
        if let Some(val) = lookup(var_name)
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty())
        {
            check_placeholder_violation(var_name, &val, &mut violations);
        }
    }

    if lookup("AXPOINT_DEV_TOKEN").is_some_and(|v| !v.trim().is_empty()) {
        violations.push(String::from(
            "AXPOINT_DEV_TOKEN must not be set when AXPOINT_ENV=production",
        ));
    }

    if violations.is_empty() {
        Ok(())
    } else {
        let joined = violations.join("; ");
        tracing::error!(violations = %joined, "production environment preflight check failed");
        Err(AppError::Config(format!(
            "production environment preflight failed: {joined}"
        )))
    }
}

fn check_placeholder_violation(var_name: &str, value: &str, violations: &mut Vec<String>) {
    let lower = value.to_ascii_lowercase();
    for &needle in FORBIDDEN_PLACEHOLDERS {
        if lower.contains(needle) {
            violations.push(format!(
                "{var_name} contains example/placeholder value '{needle}'"
            ));
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn valid_prod_map() -> HashMap<String, String> {
        HashMap::from([
            (String::from("AXPOINT_ENV"), String::from("production")),
            (
                String::from("AXPOINT_PUBLIC_HOST"),
                String::from("ops.axpoint.internal"),
            ),
            (
                String::from("DATABASE_URL"),
                String::from(
                    "postgres://axpoint_app:S3cur3Pr0dP@ssw0rd99887766@pg-rw:5432/xbluenode",
                ),
            ),
            (
                String::from("AXPOINT_INTERNAL_TOKEN"),
                String::from("prod_internal_token_9876543210_abcdef_123456"),
            ),
            (
                String::from("AXPOINT_CREDENTIAL_KEYS"),
                String::from("1:MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY="),
            ),
            (
                String::from("AXPOINT_MASK_KEYS"),
                String::from("1:ZmVkY2JhOTg3NjU0MzIxMGZlZGNiYTk4NzY1NDMyMTA="),
            ),
            (
                String::from("AXPOINT_JWT_PRIVATE_KEY_PATH"),
                String::from("/etc/axpoint/jwt/private.pem"),
            ),
            (
                String::from("AXPOINT_JWT_PUBLIC_KEYS_DIR"),
                String::from("/etc/axpoint/jwt/public"),
            ),
            (
                String::from("LLM_API_BASE"),
                String::from("https://dashscope.aliyuncs.com/compatible-mode/v1"),
            ),
            (
                String::from("LLM_API_KEY"),
                String::from("sk-prod-real-llm-key-0123456789"),
            ),
            (String::from("LLM_MODEL"), String::from("qwen-max")),
            (
                String::from("EMBED_API_BASE"),
                String::from("https://dashscope.aliyuncs.com/compatible-mode/v1"),
            ),
            (
                String::from("EMBED_API_KEY"),
                String::from("sk-prod-real-embed-key-0123456789"),
            ),
            (
                String::from("EMBED_MODEL"),
                String::from("text-embedding-v4"),
            ),
        ])
    }

    #[test]
    fn non_production_env_skips_strict_preflight() {
        let empty: HashMap<String, String> = HashMap::new();
        assert!(validate_production_env_with(|k| empty.get(k).cloned()).is_ok());

        let test_map = HashMap::from([
            (String::from("AXPOINT_ENV"), String::from("test")),
            (
                String::from("AXPOINT_DEV_TOKEN"),
                String::from("0123456789abcdef0123456789abcdef"),
            ),
        ]);
        assert!(validate_production_env_with(|k| test_map.get(k).cloned()).is_ok());
    }

    #[test]
    fn valid_production_env_passes_preflight() {
        let env_map = valid_prod_map();
        assert!(validate_production_env_with(|k| env_map.get(k).cloned()).is_ok());
    }

    #[test]
    fn production_env_reports_all_missing_placeholder_and_dev_token_issues() {
        let mut env_map = valid_prod_map();
        env_map.remove("LLM_API_KEY");
        env_map.remove("AXPOINT_MASK_KEYS");
        env_map.insert(
            String::from("AXPOINT_PUBLIC_HOST"),
            String::from("xbluenode.example.com"),
        );
        env_map.insert(
            String::from("DATABASE_URL"),
            String::from("postgres://axpoint_app:axpoint_app_secret@db:5432/xbluenode"),
        );
        env_map.insert(
            String::from("EMBED_API_KEY"),
            String::from("REPLACE_WITH_PRODUCTION_EMBED_API_KEY"),
        );
        env_map.insert(
            String::from("POSTGRES_PASSWORD"),
            String::from("changeme_now"),
        );
        env_map.insert(
            String::from("AXPOINT_INTERNAL_TOKEN"),
            String::from("too_short"),
        );
        env_map.insert(
            String::from("AXPOINT_DEV_TOKEN"),
            String::from("0123456789abcdef0123456789abcdef"),
        );

        let res = validate_production_env_with(|k| env_map.get(k).cloned());
        assert!(matches!(res, Err(AppError::Config(_))));
        if let Err(AppError::Config(msg)) = res {
            assert!(msg.contains("LLM_API_KEY is missing"));
            assert!(msg.contains("AXPOINT_MASK_KEYS is missing"));
            assert!(msg.contains("AXPOINT_PUBLIC_HOST"));
            assert!(msg.contains("DATABASE_URL"));
            assert!(msg.contains("EMBED_API_KEY"));
            assert!(msg.contains("POSTGRES_PASSWORD"));
            assert!(msg.contains("AXPOINT_INTERNAL_TOKEN must be at least 32 bytes"));
            assert!(msg.contains("AXPOINT_DEV_TOKEN must not be set"));
        }
    }
}
