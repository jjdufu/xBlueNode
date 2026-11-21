//! `xBlueView` `axs_` external share link creation, `argon2id` password protection,
//! `CredentialKeyring` `X-Share-Session` tokens, per-IP rate limiting, locked-filter
//! enforcement, and dynamic share downloads (`10-STEP11-12` §12.5).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use argon2::Argon2;
use argon2::password_hash::phc::PasswordHash;
use argon2::password_hash::{PasswordHasher as _, PasswordVerifier as _};
use axpoint_agent::rate_limit::KeyedRateLimiter;
use axpoint_analytics::SessionPool;
use axpoint_domain::AppError;
use axpoint_domain::enums::{AuditAction, ExportFormat, JobStatus, Permission, WidgetKind};
use axpoint_domain::export::{
    CreateShareLinkParams, CreateShareRequest, CreateShareResponse, ExportOptions, ExportSource,
    ExportWatermarkOption, ResolvedShare, SharePublicResponse, ShareTargetKind,
    ShareUnlockResponse, SharedExportFileMeta, SidecarExportRequest,
};
use axpoint_domain::platform::AuditRecord;
use axpoint_domain::ports::{
    AgentRepository, AnalyticsRepository, ConnectionRepository, CredentialStore, DatasetRepository,
    GraphQueryPort, GuardRepository, MetricStorePort, RulesRepository, SidecarPort, SimRepository,
    StudioRepository, ViewRepository,
};
use axpoint_domain::view::{
    AssetRecord, DashboardDetail, DashboardStatus, WidgetDataRequest, WidgetDataResponse,
};
use axpoint_security::{AuditWriter, AuthPrincipal, CredentialKeyring};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{Duration, TimeZone as _, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use uuid::Uuid;

use super::cache::ViewCache;
use super::data::{WidgetExecEnv, execute_widget_data};
use super::export::{
    ExportBuildDeps, build_export_payload, cleanup_file_quietly, load_masked_creator_principal,
    resolve_data_root_path,
};

const BASE62_ALPHABET: &[u8; 62] =
    b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
const SHARE_PREFIX_LEN: usize = 8;
const SHARE_SECRET_LEN: usize = 32;
const UNLOCK_RPM_PER_IP: i32 = 10;
const ACCESS_RPM_PER_IP: i32 = 120;
const SHARE_SESSION_TTL_HOURS: i64 = 2;
const SHARE_SESSION_AAD: &str = "axpoint:share_session:v1";
const MAX_SHARE_HOURS: u32 = 720;
const MAX_PASSWORD_BYTES: usize = 256;

/// Per-IP token-bucket rate limiters guarding unauthenticated `/api/xblueview/share/*` endpoints (`D3`).
#[derive(Debug)]
pub struct ShareRateLimiters {
    unlock_ip: KeyedRateLimiter<String>,
    access_ip: KeyedRateLimiter<String>,
}

impl Default for ShareRateLimiters {
    fn default() -> Self {
        Self::new()
    }
}

impl ShareRateLimiters {
    /// Constructs [`ShareRateLimiters`] with `10 rpm/IP` for `/unlock` and `120 rpm/IP` for `/share/*`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            unlock_ip: KeyedRateLimiter::new(16_384),
            access_ip: KeyedRateLimiter::new(32_768),
        }
    }

    /// Enforces `10 rpm/IP` on `POST /api/xblueview/share/{token}/unlock`.
    ///
    /// # Errors
    /// Returns [`AppError::RateLimited`] (`429 RATE_LIMITED`) when the caller IP exceeds budget.
    pub fn check_unlock_ip(&self, client_ip: &str) -> Result<(), AppError> {
        self.unlock_ip
            .check_and_consume(normalize_ip_key(client_ip), UNLOCK_RPM_PER_IP)
    }

    /// Enforces `120 rpm/IP` on public `/api/xblueview/share/{token}*` reads and queries.
    ///
    /// # Errors
    /// Returns [`AppError::RateLimited`] (`429 RATE_LIMITED`) when the caller IP exceeds budget.
    pub fn check_access_ip(&self, client_ip: &str) -> Result<(), AppError> {
        self.access_ip
            .check_and_consume(normalize_ip_key(client_ip), ACCESS_RPM_PER_IP)
    }
}

/// Request metadata extracted from public `/api/xblueview/share/*` HTTP headers.
#[derive(Debug, Clone, Default)]
pub struct ShareRequestMeta {
    /// Optional `X-Share-Session` token obtained from `POST /unlock`.
    pub share_session: Option<String>,
    /// Normalized client IP address for rate limiting and `SHARE_ACCESS` audit logging.
    pub client_ip: String,
    /// Active OpenTelemetry trace identifier.
    pub trace_id: String,
}

/// Borrowed service dependencies for public `/api/xblueview/share/*` handlers.
pub struct PublicShareEnv<'a, S: ?Sized> {
    /// Multi-domain persistence store.
    pub store: &'a S,
    /// Per-tenant hardened `DuckDB` session pool.
    pub analytics_pool: &'a Arc<SessionPool>,
    /// `xBlueView` widget cache.
    pub cache: &'a Arc<ViewCache>,
    /// `Cedar` `ABAC` policy authorizer.
    pub cedar: &'a Arc<axpoint_security::cedar_authz::CedarAuthorizer>,
    /// Versioned AES-256-GCM keyring used to seal and open `X-Share-Session` tokens.
    pub keyring: &'a Arc<CredentialKeyring>,
    /// Security audit log writer.
    pub audit: &'a AuditWriter,
    /// Per-IP rate limiters.
    pub limiters: &'a Arc<ShareRateLimiters>,
    /// `AXPOINT_DATA_ROOT` directory for shared assets and export files.
    pub data_root: &'a Path,
}

/// Resolved file artifact ready to stream back from `GET /api/xblueview/share/{token}/download`.
#[derive(Debug)]
pub struct PreparedShareDownload {
    /// Verified filesystem path inside `AXPOINT_DATA_ROOT`.
    pub disk_path: PathBuf,
    /// User-facing attachment filename.
    pub file_name: String,
    /// Output format for fallback `Content-Type` resolution.
    pub export_format: ExportFormat,
    /// `true` when rendered on-the-fly for a dynamic `DASHBOARD`/`SCENARIO` share and should be deleted after reading.
    pub ephemeral: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct ShareSessionClaims {
    share_id: Uuid,
    exp: i64,
}

/// Creates a new `axs_<prefix8>_<secret32>` external share link (`POST /api/xblueview/shares`).
///
/// # Errors
/// Returns `403 PERMISSION_DENIED`, `400 INVALID_ARGUMENT`, or `404 *_NOT_FOUND` on validation failure.
pub async fn create_share<S>(
    store: &S,
    audit: &AuditWriter,
    principal: &AuthPrincipal,
    req: CreateShareRequest,
    client_ip: &str,
    trace_id: &str,
) -> Result<CreateShareResponse, AppError>
where
    S: ViewRepository + SimRepository + ?Sized,
{
    principal.require(Permission::ViewShare)?;
    principal.require(Permission::DataRead)?;

    if !(1..=MAX_SHARE_HOURS).contains(&req.expires_in_hours) {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            format!("expires_in_hours must be in 1..={MAX_SHARE_HOURS}"),
        ));
    }
    if req.max_views.is_some_and(|v| v < 1) {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "max_views must be >= 1 when specified",
        ));
    }

    validate_share_target(store, principal, req.target_kind, req.target_id).await?;

    let password_hash = match req
        .password
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(pwd) => Some(hash_share_password(pwd).await?),
        None => None,
    };

    let (prefix, token) = generate_axs_token();
    let token_hash = sha256_lower_hex(&token);
    let expires_at = Utc::now() + Duration::hours(i64::from(req.expires_in_hours));

    let record = store
        .create_share_link(
            &principal.tenant_id,
            CreateShareLinkParams {
                token_prefix: prefix,
                token_hash,
                target_kind: req.target_kind,
                target_id: req.target_id,
                locked_filters: req.locked_filters,
                allow_filters: req.allow_filters,
                allow_export: req.allow_export,
                password_hash,
                expires_at,
                max_views: req.max_views,
                created_by: principal.user_id,
            },
        )
        .await?;

    emit_share_create_audit(
        audit,
        principal,
        &record.id,
        req.target_kind,
        client_ip,
        trace_id,
    )
    .await;

    Ok(CreateShareResponse {
        id: record.id,
        url: format!("/share/{token}"),
        token,
        expires_at: record.expires_at,
    })
}

async fn validate_share_target<S>(
    store: &S,
    principal: &AuthPrincipal,
    target_kind: ShareTargetKind,
    target_id: Uuid,
) -> Result<(), AppError>
where
    S: ViewRepository + SimRepository + ?Sized,
{
    match target_kind {
        ShareTargetKind::Dashboard => {
            let detail = store
                .get_dashboard(&principal.tenant_id, target_id, false)
                .await?
                .ok_or(AppError::NotFound("DASHBOARD_NOT_FOUND"))?;
            if detail.dashboard.status != DashboardStatus::Published {
                return Err(AppError::bad(
                    "DASHBOARD_NOT_PUBLISHED",
                    "only PUBLISHED dashboards can be shared externally",
                ));
            }
        }
        ShareTargetKind::ExportFile => {
            let job = store
                .get_export_job(&principal.tenant_id, target_id)
                .await?
                .ok_or(AppError::NotFound("EXPORT_NOT_FOUND"))?;
            if job.created_by != principal.user_id && !principal.has(Permission::DataAdmin) {
                return Err(AppError::Forbidden("PERMISSION_DENIED"));
            }
            if job.status != JobStatus::Succeeded {
                return Err(AppError::bad(
                    "INVALID_ARGUMENT",
                    "only SUCCEEDED export files can be shared",
                ));
            }
        }
        ShareTargetKind::Scenario => {
            let _ = store
                .get_scenario(&principal.tenant_id, target_id)
                .await?
                .ok_or(AppError::NotFound("SCENARIO_NOT_FOUND"))?;
        }
    }
    Ok(())
}

/// Verifies the share password and issues a 2-hour `X-Share-Session` token (`POST /api/xblueview/share/{token}/unlock`).
///
/// # Errors
/// Returns `429 RATE_LIMITED`, `404 SHARE_NOT_FOUND`, or `401 PASSWORD_INCORRECT`.
pub async fn unlock_public_share<S>(
    store: &S,
    keyring: &CredentialKeyring,
    limiters: &ShareRateLimiters,
    raw_token: &str,
    candidate_password: &str,
    client_ip: &str,
) -> Result<ShareUnlockResponse, AppError>
where
    S: ViewRepository + CredentialStore + ?Sized,
{
    limiters.check_unlock_ip(client_ip)?;
    let (share, _creator) = resolve_active_share(store, raw_token).await?;

    if let Some(ref stored_phc) = share.password_hash {
        verify_share_password(stored_phc, candidate_password).await?;
    }

    let expires_at = Utc::now() + Duration::hours(SHARE_SESSION_TTL_HOURS);
    let share_session = issue_share_session(keyring, share.share_id, expires_at.timestamp())?;
    Ok(ShareUnlockResponse {
        share_session,
        expires_at,
    })
}

/// Resolves and loads the public share view (`GET /api/xblueview/share/{token}`).
///
/// When password-protected without a valid `X-Share-Session`, returns `password_required = true`
/// without incrementing `view_count`. Otherwise atomically increments `view_count` (`404` when capped)
/// and records `SHARE_ACCESS`.
///
/// # Errors
/// Returns `429 RATE_LIMITED` or `404 SHARE_NOT_FOUND` on invalid/expired/revoked/exhausted links.
pub async fn get_public_share<S>(
    env: &PublicShareEnv<'_, S>,
    raw_token: &str,
    meta: &ShareRequestMeta,
) -> Result<SharePublicResponse, AppError>
where
    S: ViewRepository + CredentialStore + SimRepository + ?Sized,
{
    env.limiters.check_access_ip(&meta.client_ip)?;
    let (share, _creator) = resolve_active_share(env.store, raw_token).await?;

    if share.password_hash.is_some()
        && !verify_share_session(env.keyring, meta.share_session.as_deref(), share.share_id)
    {
        return Ok(SharePublicResponse {
            password_required: true,
            target_kind: None,
            expires_at: Some(share.expires_at),
            locked_filters: BTreeMap::new(),
            allow_filters: false,
            allow_export: false,
            dashboard: None,
            export_file: None,
            scenario: None,
        });
    }

    if !env
        .store
        .increment_share_view_count(&share.tenant_id, share.share_id)
        .await?
    {
        return Err(AppError::NotFound("SHARE_NOT_FOUND"));
    }

    emit_share_access_audit(env.audit, &share, meta).await;
    load_public_target_payload(env.store, share).await
}

async fn load_public_target_payload<S>(
    store: &S,
    share: ResolvedShare,
) -> Result<SharePublicResponse, AppError>
where
    S: ViewRepository + SimRepository + ?Sized,
{
    let mut dashboard = None;
    let mut export_file = None;
    let mut scenario = None;

    match share.target_kind {
        ShareTargetKind::Dashboard => {
            let mut detail = store
                .get_dashboard(&share.tenant_id, share.target_id, true)
                .await?
                .ok_or(AppError::NotFound("SHARE_NOT_FOUND"))?;
            strip_interactive_action_widgets(&mut detail);
            dashboard = Some(detail);
        }
        ShareTargetKind::ExportFile => {
            let job = store
                .get_export_job(&share.tenant_id, share.target_id)
                .await?
                .ok_or(AppError::NotFound("SHARE_NOT_FOUND"))?;
            if job.status != JobStatus::Succeeded {
                return Err(AppError::NotFound("SHARE_NOT_FOUND"));
            }
            export_file = Some(SharedExportFileMeta {
                id: job.id,
                export_format: job.export_format,
                file_name: job
                    .file_name
                    .unwrap_or_else(|| format!("export-{}", job.id)),
                file_size: job.file_size.unwrap_or(0),
                finished_at: job.finished_at,
            });
        }
        ShareTargetKind::Scenario => {
            let sc = store
                .get_scenario(&share.tenant_id, share.target_id)
                .await?
                .ok_or(AppError::NotFound("SHARE_NOT_FOUND"))?;
            let val = serde_json::to_value(sc)
                .map_err(|err| AppError::Config(format!("serialize scenario failed: {err}")))?;
            scenario = Some(val);
        }
    }

    Ok(SharePublicResponse {
        password_required: false,
        target_kind: Some(share.target_kind),
        expires_at: Some(share.expires_at),
        locked_filters: share.locked_filters,
        allow_filters: share.allow_filters,
        allow_export: share.allow_export,
        dashboard,
        export_file,
        scenario,
    })
}

fn strip_interactive_action_widgets(detail: &mut DashboardDetail) {
    detail.widgets.retain(|w| {
        !matches!(
            w.widget_kind,
            WidgetKind::ActionButton | WidgetKind::ActionForm
        )
    });
}

/// Executes widget data under the share creator's masked principal with `locked_filters` enforced
/// (`POST /api/xblueview/share/{token}/widgets/{widget_id}/data`).
///
/// # Errors
/// Returns `429 RATE_LIMITED`, `401 SHARE_LOCKED`, `404 SHARE_NOT_FOUND`/`WIDGET_NOT_FOUND`, or query errors.
pub async fn execute_shared_widget_data<S>(
    env: &PublicShareEnv<'_, S>,
    raw_token: &str,
    widget_id: Uuid,
    mut req: WidgetDataRequest,
    meta: &ShareRequestMeta,
) -> Result<WidgetDataResponse, AppError>
where
    S: ViewRepository
        + CredentialStore
        + GuardRepository
        + AnalyticsRepository
        + DatasetRepository
        + ConnectionRepository
        + GraphQueryPort
        + MetricStorePort
        + RulesRepository
        + SimRepository
        + StudioRepository
        + ?Sized,
{
    env.limiters.check_access_ip(&meta.client_ip)?;
    let (share, creator) = resolve_unlocked_share(env.store, env.keyring, raw_token, meta).await?;
    if share.target_kind != ShareTargetKind::Dashboard {
        return Err(AppError::NotFound("SHARE_NOT_FOUND"));
    }

    let detail = env
        .store
        .get_dashboard(&share.tenant_id, share.target_id, true)
        .await?
        .ok_or(AppError::NotFound("SHARE_NOT_FOUND"))?;

    let widget = env
        .store
        .get_widget(&share.tenant_id, widget_id)
        .await?
        .ok_or(AppError::NotFound("WIDGET_NOT_FOUND"))?;
    if widget.dashboard_id != share.target_id {
        return Err(AppError::NotFound("WIDGET_NOT_FOUND"));
    }
    if matches!(
        widget.widget_kind,
        WidgetKind::ActionButton | WidgetKind::ActionForm
    ) {
        return Err(AppError::Forbidden("PERMISSION_DENIED"));
    }

    let policy = crate::policy::build_policy_context_for_store(
        env.store,
        env.cedar,
        &creator,
        false,
        &meta.client_ip,
    )
    .await?;
    req.filters = enforce_share_filters(&req.filters, &share.locked_filters, share.allow_filters);
    let exec_env = WidgetExecEnv {
        store: env.store,
        analytics_pool: env.analytics_pool,
        cache: env.cache,
        principal: &creator,
        policy: Some(&policy),
    };
    execute_widget_data(&exec_env, &detail.dashboard, &widget, &req).await
}

/// Resolves a spatial asset (`FLOOR_PLAN` SVG/image or `SCENE_3D` `.glb`) referenced by a shared dashboard
/// (`GET /api/xblueview/share/{token}/assets/{asset_id}/raw`).
///
/// # Errors
/// Returns `429 RATE_LIMITED`, `401 SHARE_LOCKED`, or `404` when the asset is not bound to the shared dashboard.
pub async fn resolve_shared_dashboard_asset<S>(
    env: &PublicShareEnv<'_, S>,
    raw_token: &str,
    asset_id: Uuid,
    meta: &ShareRequestMeta,
) -> Result<(AssetRecord, PathBuf), AppError>
where
    S: ViewRepository + CredentialStore + ?Sized,
{
    env.limiters.check_access_ip(&meta.client_ip)?;
    let (share, _creator) = resolve_unlocked_share(env.store, env.keyring, raw_token, meta).await?;
    if share.target_kind != ShareTargetKind::Dashboard {
        return Err(AppError::NotFound("SHARE_NOT_FOUND"));
    }

    let widgets = env
        .store
        .list_widgets_by_asset(&share.tenant_id, asset_id)
        .await?;
    if !widgets.iter().any(|w| w.dashboard_id == share.target_id) {
        return Err(AppError::NotFound("ASSET_NOT_FOUND"));
    }

    let asset = env
        .store
        .get_asset(&share.tenant_id, asset_id)
        .await?
        .ok_or(AppError::NotFound("ASSET_NOT_FOUND"))?;
    let disk_path = resolve_data_root_path(env.data_root, &asset.file_path)?;
    Ok((asset, disk_path))
}

/// Prepares either a static `EXPORT_FILE` or an on-the-fly watermarked export for `DASHBOARD`/`SCENARIO`
/// (`GET /api/xblueview/share/{token}/download?format=...`, `D2`).
///
/// # Errors
/// Returns `429 RATE_LIMITED`, `401 SHARE_LOCKED`, `403 PERMISSION_DENIED` (`allow_export == false`), or `404`.
pub async fn prepare_shared_download<S, P>(
    env: &PublicShareEnv<'_, S>,
    sidecar: &P,
    raw_token: &str,
    requested_format: Option<ExportFormat>,
    meta: &ShareRequestMeta,
) -> Result<PreparedShareDownload, AppError>
where
    S: ViewRepository
        + CredentialStore
        + GuardRepository
        + AnalyticsRepository
        + DatasetRepository
        + ConnectionRepository
        + GraphQueryPort
        + MetricStorePort
        + RulesRepository
        + SimRepository
        + StudioRepository
        + AgentRepository
        + ?Sized,
    P: SidecarPort + ?Sized,
{
    env.limiters.check_access_ip(&meta.client_ip)?;
    let (share, creator) = resolve_unlocked_share(env.store, env.keyring, raw_token, meta).await?;

    if share.target_kind == ShareTargetKind::ExportFile {
        return prepare_static_export_download(env, &share, meta).await;
    }

    if !share.allow_export {
        return Err(AppError::Forbidden("PERMISSION_DENIED"));
    }

    let format = requested_format.unwrap_or(ExportFormat::Pdf);
    let source = match share.target_kind {
        ShareTargetKind::Dashboard => ExportSource::Dashboard {
            dashboard_id: share.target_id,
            filters: share.locked_filters.clone(),
        },
        ShareTargetKind::Scenario => ExportSource::Scenario {
            scenario_ids: vec![share.target_id],
        },
        ShareTargetKind::ExportFile => return Err(AppError::NotFound("SHARE_NOT_FOUND")),
    };

    render_ephemeral_share_export(env, sidecar, &share, &creator, format, &source, meta).await
}

async fn prepare_static_export_download<S>(
    env: &PublicShareEnv<'_, S>,
    share: &ResolvedShare,
    meta: &ShareRequestMeta,
) -> Result<PreparedShareDownload, AppError>
where
    S: ViewRepository + ?Sized,
{
    let job = env
        .store
        .get_export_job(&share.tenant_id, share.target_id)
        .await?
        .ok_or(AppError::NotFound("SHARE_NOT_FOUND"))?;
    if job.status != JobStatus::Succeeded {
        return Err(AppError::NotFound("SHARE_NOT_FOUND"));
    }
    let rel = job
        .file_path
        .as_deref()
        .ok_or(AppError::NotFound("SHARE_NOT_FOUND"))?;
    let disk_path = resolve_data_root_path(env.data_root, rel)?;
    emit_share_download_audit(env.audit, share, job.export_format, meta).await;
    Ok(PreparedShareDownload {
        disk_path,
        file_name: job
            .file_name
            .unwrap_or_else(|| format!("export-{}", job.id)),
        export_format: job.export_format,
        ephemeral: false,
    })
}

async fn render_ephemeral_share_export<S, P>(
    env: &PublicShareEnv<'_, S>,
    sidecar: &P,
    share: &ResolvedShare,
    creator: &AuthPrincipal,
    format: ExportFormat,
    source: &ExportSource,
    meta: &ShareRequestMeta,
) -> Result<PreparedShareDownload, AppError>
where
    S: ViewRepository
        + GuardRepository
        + AnalyticsRepository
        + DatasetRepository
        + ConnectionRepository
        + GraphQueryPort
        + MetricStorePort
        + RulesRepository
        + SimRepository
        + StudioRepository
        + AgentRepository
        + ?Sized,
    P: SidecarPort + ?Sized,
{
    let temp_job_id = Uuid::new_v4();
    let watermark_text = format!(
        "share:{} ({}) {}",
        share.token_prefix,
        creator.username,
        Utc::now().format("%Y-%m-%d %H:%M")
    );
    let options = ExportOptions {
        watermark: ExportWatermarkOption::Text(watermark_text),
        ..ExportOptions::default()
    };
    let deps = ExportBuildDeps {
        store: env.store,
        analytics_pool: env.analytics_pool,
        cache: env.cache,
        cedar: env.cedar,
        data_root: env.data_root,
    };
    let built =
        build_export_payload(&deps, creator, temp_job_id, format, source, &options, None).await?;

    let tz = built.document.timezone.clone();
    let req = SidecarExportRequest {
        tenant_id: share.tenant_id.clone(),
        job_id: temp_job_id,
        document: built.document,
    };
    let res = sidecar
        .export_document(&share.tenant_id, &tz, format, &req)
        .await;
    if let Some(ref tmp_pq) = built.temp_parquet_path {
        cleanup_file_quietly(tmp_pq).await;
    }
    let exported = res?;
    let disk_path = resolve_data_root_path(env.data_root, &exported.file_path)?;
    emit_share_download_audit(env.audit, share, format, meta).await;
    Ok(PreparedShareDownload {
        disk_path,
        file_name: exported.file_name,
        export_format: format,
        ephemeral: true,
    })
}

/// Merges `visitor_filters` and `locked_filters` according to `allow_filters` (`10-STEP11-12` §12.5.2).
#[must_use]
pub fn enforce_share_filters(
    visitor_filters: &BTreeMap<String, serde_json::Value>,
    locked_filters: &BTreeMap<String, serde_json::Value>,
    allow_filters: bool,
) -> BTreeMap<String, serde_json::Value> {
    let mut merged = if allow_filters {
        visitor_filters.clone()
    } else {
        BTreeMap::new()
    };
    for (key, val) in locked_filters {
        merged.insert(key.clone(), val.clone());
    }
    merged
}

async fn resolve_active_share<S>(
    store: &S,
    raw_token: &str,
) -> Result<(ResolvedShare, AuthPrincipal), AppError>
where
    S: ViewRepository + CredentialStore + ?Sized,
{
    let cleaned = validate_axs_token_syntax(raw_token)?;
    let token_hash = sha256_lower_hex(cleaned);
    let share = store
        .resolve_share_by_hash(&token_hash)
        .await?
        .ok_or(AppError::NotFound("SHARE_NOT_FOUND"))?;
    if share
        .max_views
        .is_some_and(|max_v| share.view_count >= max_v)
    {
        return Err(AppError::NotFound("SHARE_NOT_FOUND"));
    }
    let creator = load_masked_creator_principal(store, &share.tenant_id, share.created_by)
        .await?
        .ok_or(AppError::NotFound("SHARE_NOT_FOUND"))?;
    Ok((share, creator))
}

async fn resolve_unlocked_share<S>(
    store: &S,
    keyring: &CredentialKeyring,
    raw_token: &str,
    meta: &ShareRequestMeta,
) -> Result<(ResolvedShare, AuthPrincipal), AppError>
where
    S: ViewRepository + CredentialStore + ?Sized,
{
    let (share, creator) = resolve_active_share(store, raw_token).await?;
    if share.password_hash.is_some()
        && !verify_share_session(keyring, meta.share_session.as_deref(), share.share_id)
    {
        return Err(AppError::Unauthorized("SHARE_LOCKED"));
    }
    Ok((share, creator))
}

fn validate_axs_token_syntax(raw: &str) -> Result<&str, AppError> {
    let trimmed = raw.trim();
    let Some(rest) = trimmed.strip_prefix("axs_") else {
        return Err(AppError::NotFound("SHARE_NOT_FOUND"));
    };
    let Some((prefix, secret)) = rest.split_once('_') else {
        return Err(AppError::NotFound("SHARE_NOT_FOUND"));
    };
    let valid_chars = prefix.len() == SHARE_PREFIX_LEN
        && secret.len() == SHARE_SECRET_LEN
        && prefix.bytes().all(|b| b.is_ascii_alphanumeric())
        && secret.bytes().all(|b| b.is_ascii_alphanumeric());
    if !valid_chars {
        return Err(AppError::NotFound("SHARE_NOT_FOUND"));
    }
    Ok(trimmed)
}

/// Generates `(prefix8, "axs_<prefix8>_<secret32>")` using rejection-sampled OS CSPRNG bytes.
#[must_use]
pub fn generate_axs_token() -> (String, String) {
    let prefix = random_base62(SHARE_PREFIX_LEN);
    let secret = random_base62(SHARE_SECRET_LEN);
    let full = format!("axs_{prefix}_{secret}");
    (prefix, full)
}

fn random_base62(len: usize) -> String {
    let mut out = String::with_capacity(len);
    while out.len() < len {
        let bytes = *Uuid::new_v4().as_bytes();
        for b in bytes {
            if out.len() >= len {
                break;
            }
            if b < 248 {
                let idx = usize::from(b % 62);
                if let Some(&ch) = BASE62_ALPHABET.get(idx) {
                    out.push(char::from(ch));
                }
            }
        }
    }
    out
}

fn sha256_lower_hex(input: &str) -> String {
    hex::encode(Sha256::digest(input.as_bytes()))
}

fn normalize_ip_key(client_ip: &str) -> String {
    let trimmed = client_ip.trim();
    if trimmed.is_empty() {
        String::from("127.0.0.1")
    } else {
        String::from(trimmed)
    }
}

async fn hash_share_password(password: &str) -> Result<String, AppError> {
    if password.len() > MAX_PASSWORD_BYTES {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            format!("password exceeds {MAX_PASSWORD_BYTES} bytes"),
        ));
    }
    let owned = password.as_bytes().to_vec();
    tokio::task::spawn_blocking(move || {
        let hashed: PasswordHash = Argon2::default()
            .hash_password(&owned)
            .map_err(|err| AppError::Config(format!("argon2id hash failed: {err}")))?;
        Ok(hashed.to_string())
    })
    .await
    .map_err(|err| AppError::Config(format!("argon2id task panicked: {err}")))?
}

async fn verify_share_password(stored_phc: &str, candidate: &str) -> Result<(), AppError> {
    if candidate.is_empty() || candidate.len() > MAX_PASSWORD_BYTES {
        return Err(AppError::Unauthorized("PASSWORD_INCORRECT"));
    }
    let phc_owned = String::from(stored_phc);
    let cand_owned = candidate.as_bytes().to_vec();
    tokio::task::spawn_blocking(move || {
        let parsed = PasswordHash::new(&phc_owned)
            .map_err(|_| AppError::Unauthorized("PASSWORD_INCORRECT"))?;
        Argon2::default()
            .verify_password(&cand_owned, &parsed)
            .map_err(|_| AppError::Unauthorized("PASSWORD_INCORRECT"))
    })
    .await
    .map_err(|err| AppError::Config(format!("argon2id verify task failed: {err}")))?
}

fn issue_share_session(
    keyring: &CredentialKeyring,
    share_id: Uuid,
    exp_unix: i64,
) -> Result<String, AppError> {
    let claims = ShareSessionClaims {
        share_id,
        exp: exp_unix,
    };
    let raw_json = serde_json::to_vec(&claims)
        .map_err(|err| AppError::Config(format!("serialize share session failed: {err}")))?;
    let (ver, nonce, ct) = keyring.seal(SHARE_SESSION_AAD, &raw_json)?;
    Ok(format!(
        "v{ver}.{}.{}",
        URL_SAFE_NO_PAD.encode(nonce),
        URL_SAFE_NO_PAD.encode(ct)
    ))
}

fn verify_share_session(
    keyring: &CredentialKeyring,
    raw_header: Option<&str>,
    expected_share_id: Uuid,
) -> bool {
    verify_share_session_at(
        keyring,
        raw_header,
        expected_share_id,
        Utc::now().timestamp(),
    )
}

fn verify_share_session_at(
    keyring: &CredentialKeyring,
    raw_header: Option<&str>,
    expected_share_id: Uuid,
    now_unix: i64,
) -> bool {
    let Some(token) = raw_header.map(str::trim).filter(|s| !s.is_empty()) else {
        return false;
    };
    let mut parts = token.splitn(3, '.');
    let (Some(ver_part), Some(nonce_b64), Some(ct_b64)) =
        (parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    let Some(ver_num) = ver_part
        .strip_prefix('v')
        .and_then(|v| v.parse::<i16>().ok())
    else {
        return false;
    };
    let (Ok(nonce), Ok(ct)) = (
        URL_SAFE_NO_PAD.decode(nonce_b64),
        URL_SAFE_NO_PAD.decode(ct_b64),
    ) else {
        return false;
    };
    let Ok(plaintext) = keyring.open(ver_num, &nonce, SHARE_SESSION_AAD, &ct) else {
        return false;
    };
    let Ok(claims) = serde_json::from_slice::<ShareSessionClaims>(plaintext.as_slice()) else {
        return false;
    };
    let _ = Utc.timestamp_opt(claims.exp, 0);
    claims.share_id == expected_share_id && claims.exp > now_unix
}

async fn emit_share_create_audit(
    audit: &AuditWriter,
    principal: &AuthPrincipal,
    share_id: &Uuid,
    target_kind: ShareTargetKind,
    client_ip: &str,
    trace_id: &str,
) {
    let user_id = (!principal.user_id.is_nil()).then_some(principal.user_id);
    audit.record(AuditRecord {
        tenant_id: principal.tenant_id.clone(),
        user_id,
        username: principal.username.clone(),
        action: AuditAction::ShareCreate,
        resource_ref: format!("share:{share_id}"),
        status_code: 201,
        client_ip: normalize_ip_key(client_ip),
        trace_id: String::from(trace_id),
        detail_json: serde_json::json!({
            "share_id": share_id,
            "target_kind": target_kind.as_str(),
        }),
        created_at: Utc::now(),
    });
    if let Err(err) = audit.flush().await {
        tracing::warn!(error = %err, "share_create audit flush failed");
    }
}

async fn emit_share_access_audit(
    audit: &AuditWriter,
    share: &ResolvedShare,
    meta: &ShareRequestMeta,
) {
    audit.record(AuditRecord {
        tenant_id: share.tenant_id.clone(),
        user_id: None,
        username: format!("share:{}", share.token_prefix),
        action: AuditAction::ShareAccess,
        resource_ref: format!("share:{}", share.share_id),
        status_code: 200,
        client_ip: normalize_ip_key(&meta.client_ip),
        trace_id: meta.trace_id.clone(),
        detail_json: serde_json::json!({
            "share_id": share.share_id,
            "target_kind": share.target_kind.as_str(),
            "target_id": share.target_id,
        }),
        created_at: Utc::now(),
    });
    if let Err(err) = audit.flush().await {
        tracing::warn!(error = %err, "share_access audit flush failed");
    }
}

async fn emit_share_download_audit(
    audit: &AuditWriter,
    share: &ResolvedShare,
    format: ExportFormat,
    meta: &ShareRequestMeta,
) {
    audit.record(AuditRecord {
        tenant_id: share.tenant_id.clone(),
        user_id: None,
        username: format!("share:{}", share.token_prefix),
        action: AuditAction::ExportFile,
        resource_ref: format!("share:{}", share.share_id),
        status_code: 200,
        client_ip: normalize_ip_key(&meta.client_ip),
        trace_id: meta.trace_id.clone(),
        detail_json: serde_json::json!({
            "share_id": share.share_id,
            "target_kind": share.target_kind.as_str(),
            "format": format.as_str(),
        }),
        created_at: Utc::now(),
    });
    if let Err(err) = audit.flush().await {
        tracing::warn!(error = %err, "share_download audit flush failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn axs_token_generator_produces_valid_prefix_and_secret() -> Result<(), AppError> {
        let (prefix, token) = generate_axs_token();
        assert_eq!(prefix.len(), SHARE_PREFIX_LEN);
        let validated = validate_axs_token_syntax(&token)?;
        assert_eq!(validated, token);
        assert!(
            validate_axs_token_syntax("axe_12345678_12345678901234567890123456789012").is_err()
        );
        assert!(validate_axs_token_syntax("axs_short_secret").is_err());
        Ok(())
    }

    #[test]
    fn share_session_seals_verifies_and_rejects_tampering_or_expiry() -> Result<(), AppError> {
        let keyring = CredentialKeyring::ephemeral()?;
        let share_id = Uuid::new_v4();
        let other_id = Uuid::new_v4();
        let session = issue_share_session(&keyring, share_id, 2_000)?;

        assert!(verify_share_session_at(
            &keyring,
            Some(&session),
            share_id,
            1_500
        ));
        assert!(!verify_share_session_at(
            &keyring,
            Some(&session),
            share_id,
            2_000
        ));
        assert!(!verify_share_session_at(
            &keyring,
            Some(&session),
            other_id,
            1_500
        ));
        assert!(!verify_share_session_at(
            &keyring,
            Some("v1.bad.token"),
            share_id,
            1_500
        ));
        Ok(())
    }

    #[test]
    fn enforce_share_filters_locks_pinned_keys_and_respects_allow_filters() {
        let visitor = BTreeMap::from([
            (String::from("region"), serde_json::json!("WEST")),
            (String::from("status"), serde_json::json!("OPEN")),
        ]);
        let locked = BTreeMap::from([(String::from("region"), serde_json::json!("EAST"))]);

        let strict = enforce_share_filters(&visitor, &locked, false);
        assert_eq!(strict.len(), 1);
        assert_eq!(strict.get("region"), Some(&serde_json::json!("EAST")));

        let flexible = enforce_share_filters(&visitor, &locked, true);
        assert_eq!(flexible.len(), 2);
        assert_eq!(flexible.get("region"), Some(&serde_json::json!("EAST")));
        assert_eq!(flexible.get("status"), Some(&serde_json::json!("OPEN")));
    }

    #[test]
    fn share_rate_limiter_enforces_unlock_10_rpm_budget() -> Result<(), AppError> {
        let limiters = ShareRateLimiters::new();
        for _ in 0..10 {
            limiters.check_unlock_ip("203.0.113.9")?;
        }
        let eleventh = limiters.check_unlock_ip("203.0.113.9");
        assert!(matches!(eleventh, Err(AppError::RateLimited)));
        limiters.check_unlock_ip("203.0.113.10")?;
        Ok(())
    }

    #[tokio::test]
    async fn argon2id_share_password_roundtrip() -> Result<(), AppError> {
        let phc = hash_share_password("p@ss-123456").await?;
        assert!(phc.starts_with("$argon2id$"));
        verify_share_password(&phc, "p@ss-123456").await?;
        let wrong = verify_share_password(&phc, "wrong-pass").await;
        assert!(matches!(
            wrong,
            Err(AppError::Unauthorized("PASSWORD_INCORRECT"))
        ));
        Ok(())
    }
}
