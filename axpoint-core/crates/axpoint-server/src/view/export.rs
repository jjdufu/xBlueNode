//! `xBlueView` Step 12 full-format export document builder, pure-JSON `ECharts` option compiler,
//! `DOCX`/`PPTX` template placeholder scanner, `Parquet` spill writer, and `EXPORT` worker (`10-STEP11-12` §12.2–§12.5).

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axpoint_analytics::sql_guard::guard_with_ceiling;
use axpoint_analytics::{
    CatalogDatasetEntry, GuardOptions, LiveConnectionSpec, PivotExecuteOptions, SessionPool,
    TenantCatalog, TenantDb, execute_pivot,
};
use axpoint_domain::AppError;
use axpoint_domain::analytics::{
    AskTurnRecord, PivotResult, PivotRowKind, PivotSpec, QueryColumn, QueryResult, SavedQuery,
    SavedQueryKind,
};
use axpoint_domain::axhub::{DatasetListQuery, DatasetStatus};
use axpoint_domain::enums::{
    AuditAction, DataType, ExportFormat, JobStatus, MessageRole, Permission, TokenKind, WidgetKind,
};
use axpoint_domain::export::{
    CompleteExportJobParams, CreateExportRequest, EXPORT_INLINE_ROW_LIMIT,
    EXPORT_SAVED_QUERY_MAX_ROWS, ExportColumnSpec, ExportDocument, ExportEmbeddedTable,
    ExportJobRecord, ExportOptions, ExportSection, ExportSource, ExportTableKind,
    SidecarExportRequest,
};
use axpoint_domain::platform::{AuditRecord, BoxFuture, JobHandler, LeasedJob};
use axpoint_domain::ports::{
    AgentRepository, AnalyticsRepository, ConnectionRepository, CredentialStore, DatasetRepository,
    GraphQueryPort, GuardRepository, MetricStorePort, RulesRepository, SidecarPort, SimRepository,
    StudioRepository, ViewRepository,
};
use axpoint_domain::sim::ScenarioRecord;
use axpoint_domain::view::{
    DashboardDetail, DashboardTheme, DrillPathItem, WidgetColumn, WidgetDataRequest,
    WidgetDataResponse, WidgetPreviewRequest, WidgetRecord,
};
use axpoint_security::{AuditWriter, AuthPrincipal};
use chrono::{Duration, Utc};
use uuid::Uuid;

use super::cache::ViewCache;
use super::data::{WidgetExecEnv, execute_widget_data, execute_widget_preview};

/// Shared JSON chart specification shared between TypeScript (`xbluenode-web`) and Rust (`axpoint-core`).
const CHART_SPEC_JSON: &str = include_str!("../../../../../xbluenode-web/src/lib/chartSpec.json");

/// Maximum uploaded `.docx` or `.pptx` template archive size (`20 MiB`).
pub const MAX_EXPORT_TEMPLATE_BYTES: usize = 20 * 1024 * 1024;

/// Assembled [`ExportDocument`] paired with an optional temporary `Parquet` spill path for cleanup.
#[derive(Debug)]
pub struct BuiltExportPayload {
    /// Unified document sent to `Sidecar` `POST /export/{format}`.
    pub document: ExportDocument,
    /// Total tabular rows included across sections (for `EXPORT_FILE` audit logging).
    pub total_rows: usize,
    /// Source category string (`"DASHBOARD"`, `"SAVED_QUERY"`, etc.).
    pub source_kind: &'static str,
    /// Optional `.tmp_<export_id>.parquet` spill file to remove after `Sidecar` completes.
    pub temp_parquet_path: Option<PathBuf>,
}

/// Dependencies required to assemble an [`ExportDocument`] from any [`ExportSource`].
pub struct ExportBuildDeps<'a, S: ?Sized> {
    /// Multi-domain persistence store.
    pub store: &'a S,
    /// Per-tenant `DuckDB` session pool.
    pub analytics_pool: &'a Arc<SessionPool>,
    /// Widget response cache.
    pub cache: &'a ViewCache,
    /// `Cedar` `ABAC` policy authorizer.
    pub cedar: &'a axpoint_security::cedar_authz::CedarAuthorizer,
    /// `AXPOINT_DATA_ROOT` directory for templates and temporary `Parquet` spill files.
    pub data_root: &'a Path,
}

/// Background `ax_jobs` handler for `JobKind::Export` (`10-STEP11-12` §12.5.1).
pub struct ExportJobHandler<S, P> {
    /// Persistence store.
    pub store: S,
    /// `AxAnalytics` `DuckDB` session pool.
    pub analytics_pool: Arc<SessionPool>,
    /// `xBlueView` widget cache.
    pub cache: Arc<ViewCache>,
    /// `Cedar` `ABAC` policy authorizer.
    pub cedar: Arc<axpoint_security::cedar_authz::CedarAuthorizer>,
    /// `Sidecar` HTTP client.
    pub sidecar: Arc<P>,
    /// Asynchronous security audit writer.
    pub audit: AuditWriter,
    /// Configured `AXPOINT_DATA_ROOT`.
    pub data_root: PathBuf,
}

impl<S, P> JobHandler for ExportJobHandler<S, P>
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
        + Send
        + Sync
        + 'static,
    P: SidecarPort + 'static,
{
    fn handle<'a>(&'a self, job: &'a LeasedJob) -> BoxFuture<'a, serde_json::Value> {
        Box::pin(async move {
            let export_id = job
                .payload
                .get("export_id")
                .and_then(serde_json::Value::as_str)
                .and_then(|s| Uuid::parse_str(s).ok())
                .ok_or_else(|| {
                    AppError::bad("INVALID_ARGUMENT", "EXPORT job payload missing export_id")
                })?;

            match self.execute_export_job(&job.tenant_id, export_id).await {
                Ok(val) => Ok(val),
                Err(err) => {
                    let msg = err.client_message();
                    if let Err(db_err) = self
                        .store
                        .fail_export_job(&job.tenant_id, export_id, &msg)
                        .await
                    {
                        tracing::warn!(
                            %export_id,
                            error = %db_err,
                            "failed to mark xv_export_jobs FAILED"
                        );
                    }
                    Err(err)
                }
            }
        })
    }
}

impl<S, P> ExportJobHandler<S, P>
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
        + Send
        + Sync,
    P: SidecarPort,
{
    async fn execute_export_job(
        &self,
        tenant_id: &str,
        export_id: Uuid,
    ) -> Result<serde_json::Value, AppError> {
        let Some(record) = self
            .store
            .mark_export_job_running(tenant_id, export_id)
            .await?
        else {
            if let Some(existing) = self.store.get_export_job(tenant_id, export_id).await?
                && existing.status == JobStatus::Succeeded
            {
                return Ok(serde_json::json!({
                    "export_id": export_id,
                    "status": "SUCCEEDED",
                }));
            }
            return Err(AppError::NotFound("EXPORT_NOT_FOUND"));
        };

        let creator = load_masked_creator_principal(&self.store, tenant_id, record.created_by)
            .await?
            .ok_or(AppError::Forbidden("PERMISSION_DENIED"))?;

        let deps = ExportBuildDeps {
            store: &self.store,
            analytics_pool: &self.analytics_pool,
            cache: &self.cache,
            cedar: &self.cedar,
            data_root: &self.data_root,
        };
        let built = build_export_payload(
            &deps,
            &creator,
            export_id,
            record.export_format,
            &record.source,
            &record.options,
            record.template_id,
        )
        .await?;

        let tz = built.document.timezone.clone();
        let req = SidecarExportRequest {
            tenant_id: String::from(tenant_id),
            job_id: export_id,
            document: built.document,
        };
        let sidecar_res = self
            .sidecar
            .export_document(tenant_id, &tz, record.export_format, &req)
            .await;

        if let Some(ref tmp_parquet) = built.temp_parquet_path {
            cleanup_file_quietly(tmp_parquet).await;
        }
        let resp = sidecar_res?;

        let keep_days = self.store.get_export_keep_days(tenant_id).await?;
        let expires_at = Utc::now() + Duration::days(i64::from(keep_days.max(1)));
        let completed = self
            .store
            .complete_export_job(
                tenant_id,
                export_id,
                CompleteExportJobParams {
                    file_name: resp.file_name,
                    file_path: resp.file_path,
                    file_size: resp.file_size,
                    sha256: resp.sha256.clone(),
                    expires_at,
                },
            )
            .await?;

        self.emit_export_audit(&creator, &completed, built.source_kind, built.total_rows)
            .await;

        Ok(serde_json::json!({
            "export_id": completed.id,
            "file_size": completed.file_size,
            "sha256": completed.sha256,
        }))
    }

    async fn emit_export_audit(
        &self,
        creator: &AuthPrincipal,
        job: &ExportJobRecord,
        source_kind: &str,
        total_rows: usize,
    ) {
        let user_id = (!creator.user_id.is_nil()).then_some(creator.user_id);
        self.audit.record(AuditRecord {
            tenant_id: job.tenant_id.clone(),
            user_id,
            username: creator.username.clone(),
            action: AuditAction::ExportFile,
            resource_ref: format!("export:{}", job.id),
            status_code: 200,
            client_ip: String::from("127.0.0.1"),
            trace_id: String::new(),
            detail_json: serde_json::json!({
                "export_id": job.id,
                "format": job.export_format.as_str(),
                "source_kind": source_kind,
                "rows": total_rows,
                "file_size": job.file_size,
                "sha256": job.sha256,
            }),
            created_at: Utc::now(),
        });
        if let Err(err) = self.audit.flush().await {
            tracing::warn!(error = %err, "export audit flush failed");
        }
    }
}

/// Loads `user_id`'s current active principal from `sec_users` + `sec_roles`, verifying
/// `Permission::DataRead` and stripping `Permission::PiiRawRead` so exports and shares always
/// apply dynamic PII masking.
///
/// # Errors
/// Propagates [`AppError::Database`] if querying `CredentialStore` fails.
pub async fn load_masked_creator_principal<S>(
    store: &S,
    tenant_id: &str,
    user_id: Uuid,
) -> Result<Option<AuthPrincipal>, AppError>
where
    S: CredentialStore + ?Sized,
{
    let Some(rec) = store.load_principal(tenant_id, user_id).await? else {
        return Ok(None);
    };
    if !rec.permissions.contains(&Permission::DataRead) {
        return Ok(None);
    }
    let mut masked_perms = rec.permissions;
    let _ = masked_perms.remove(&Permission::PiiRawRead);
    Ok(Some(AuthPrincipal {
        user_id: rec.user_id,
        tenant_id: rec.tenant_id,
        username: rec.username,
        dept_id: rec.dept_id,
        roles: rec.roles,
        permissions: masked_perms,
        token_kind: TokenKind::Access,
    }))
}

/// Validates a [`CreateExportRequest`] against `principal`'s permissions and target resource existence.
///
/// # Errors
/// Returns `400 INVALID_ARGUMENT`, `403 PERMISSION_DENIED`, or `404 *_NOT_FOUND` on validation failure.
pub async fn validate_create_export_request<S>(
    store: &S,
    principal: &AuthPrincipal,
    req: &CreateExportRequest,
) -> Result<(), AppError>
where
    S: ViewRepository + AnalyticsRepository + SimRepository + AgentRepository + ?Sized,
{
    principal.require(Permission::ExportData)?;
    principal.require(Permission::DataRead)?;

    if let Some(tpl_id) = req.template_id {
        let tpl = store
            .get_export_template(&principal.tenant_id, tpl_id)
            .await?
            .ok_or(AppError::NotFound("TEMPLATE_NOT_FOUND"))?;
        if tpl.export_format != req.export_format {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                format!(
                    "template format {} does not match requested export_format {}",
                    tpl.export_format, req.export_format
                ),
            ));
        }
    }

    validate_export_source(store, principal, &req.source).await
}

async fn validate_export_source<S>(
    store: &S,
    principal: &AuthPrincipal,
    source: &ExportSource,
) -> Result<(), AppError>
where
    S: ViewRepository + AnalyticsRepository + SimRepository + AgentRepository + ?Sized,
{
    let require_pub = !principal.has(Permission::ViewEdit);
    match source {
        ExportSource::Dashboard { dashboard_id, .. } => {
            let _ = store
                .get_dashboard(&principal.tenant_id, *dashboard_id, require_pub)
                .await?
                .ok_or(AppError::NotFound("DASHBOARD_NOT_FOUND"))?;
        }
        ExportSource::Widget { widget_id, .. } => {
            let widget = store
                .get_widget(&principal.tenant_id, *widget_id)
                .await?
                .ok_or(AppError::NotFound("WIDGET_NOT_FOUND"))?;
            let _ = store
                .get_dashboard(&principal.tenant_id, widget.dashboard_id, require_pub)
                .await?
                .ok_or(AppError::NotFound("DASHBOARD_NOT_FOUND"))?;
        }
        ExportSource::SavedQuery { saved_query_id } => {
            let sq = store
                .get_saved_query(&principal.tenant_id, *saved_query_id)
                .await?
                .ok_or(AppError::NotFound("SAVED_QUERY_NOT_FOUND"))?;
            check_saved_query_access(principal, &sq)?;
        }
        ExportSource::Pivot { spec } => {
            let _: PivotSpec = serde_json::from_value(spec.clone()).map_err(|err| {
                AppError::bad("INVALID_ARGUMENT", format!("invalid pivot spec: {err}"))
            })?;
        }
        ExportSource::Scenario { scenario_ids } => {
            if !(1..=5).contains(&scenario_ids.len()) {
                return Err(AppError::bad(
                    "INVALID_ARGUMENT",
                    "scenario_ids must contain 1..=5 scenario UUIDs",
                ));
            }
            for &sid in scenario_ids {
                let _ = store
                    .get_scenario(&principal.tenant_id, sid)
                    .await?
                    .ok_or(AppError::NotFound("SCENARIO_NOT_FOUND"))?;
            }
        }
        ExportSource::Conversation { conversation_id } => {
            validate_conversation_readable(store, principal, *conversation_id).await?;
        }
    }
    Ok(())
}

fn check_saved_query_access(principal: &AuthPrincipal, sq: &SavedQuery) -> Result<(), AppError> {
    if !sq.shared
        && sq.owner_user_id != Some(principal.user_id)
        && !principal.has(Permission::DataAdmin)
    {
        return Err(AppError::NotFound("SAVED_QUERY_NOT_FOUND"));
    }
    Ok(())
}

async fn validate_conversation_readable<S>(
    store: &S,
    principal: &AuthPrincipal,
    conversation_id: Uuid,
) -> Result<(), AppError>
where
    S: AgentRepository + AnalyticsRepository + ?Sized,
{
    if let Some(conv) = store
        .get_conversation(&principal.tenant_id, conversation_id)
        .await?
    {
        if conv.user_id != Some(principal.user_id) && !principal.has(Permission::DataAdmin) {
            return Err(AppError::NotFound("CONVERSATION_NOT_FOUND"));
        }
        return Ok(());
    }
    let turns = store
        .list_session_turns(&principal.tenant_id, conversation_id)
        .await?;
    if turns.is_empty() {
        return Err(AppError::NotFound("CONVERSATION_NOT_FOUND"));
    }
    Ok(())
}

/// Returns the static string discriminator for an [`ExportSource`].
#[must_use]
pub const fn export_source_kind_str(source: &ExportSource) -> &'static str {
    match source {
        ExportSource::Dashboard { .. } => "DASHBOARD",
        ExportSource::Widget { .. } => "WIDGET",
        ExportSource::SavedQuery { .. } => "SAVED_QUERY",
        ExportSource::Pivot { .. } => "PIVOT",
        ExportSource::Scenario { .. } => "SCENARIO",
        ExportSource::Conversation { .. } => "CONVERSATION",
    }
}

/// Builds a complete [`BuiltExportPayload`] from `source`, executing all queries under `creator`
/// with PII masking strictly enforced.
///
/// # Errors
/// Propagates missing target (`404`), invalid arguments (`400`), SQL guard/timeout, or DB errors.
pub async fn build_export_payload<S>(
    deps: &ExportBuildDeps<'_, S>,
    creator: &AuthPrincipal,
    export_id: Uuid,
    format: ExportFormat,
    source: &ExportSource,
    options: &ExportOptions,
    template_id: Option<Uuid>,
) -> Result<BuiltExportPayload, AppError>
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
{
    let now = Utc::now();
    let template_path =
        resolve_template_disk_path(deps, &creator.tenant_id, format, template_id).await?;

    let (default_title, mut sections, temp_parquet_path) =
        build_source_sections(deps, creator, export_id, format, source).await?;

    let resolved_title = options
        .title
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map_or(default_title, str::to_owned);

    if format == ExportFormat::Csv {
        sections = normalize_sections_for_csv(&resolved_title, sections);
    }
    let total_rows = count_section_rows(&sections);

    let timezone = options
        .timezone
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map_or_else(|| String::from("UTC"), str::to_owned);
    let watermark = options.watermark.resolve(&creator.username, now);

    let mut placeholders = options.placeholders.clone();
    placeholders
        .entry(String::from("title"))
        .or_insert_with(|| resolved_title.clone());
    placeholders
        .entry(String::from("generated_by"))
        .or_insert_with(|| creator.username.clone());
    placeholders
        .entry(String::from("generated_at"))
        .or_insert_with(|| now.format("%Y-%m-%d %H:%M").to_string());

    let document = ExportDocument {
        title: resolved_title,
        subtitle: options.subtitle.clone().unwrap_or_default(),
        generated_at: now,
        generated_by: creator.username.clone(),
        timezone,
        watermark,
        sections,
        template_path,
        placeholders,
        csv_bom: options.csv_bom,
        page_size: options.page_size,
        orientation: options.orientation,
    };

    Ok(BuiltExportPayload {
        document,
        total_rows,
        source_kind: export_source_kind_str(source),
        temp_parquet_path,
    })
}

async fn resolve_template_disk_path<S>(
    deps: &ExportBuildDeps<'_, S>,
    tenant_id: &str,
    format: ExportFormat,
    template_id: Option<Uuid>,
) -> Result<Option<String>, AppError>
where
    S: ViewRepository + ?Sized,
{
    let Some(tid) = template_id else {
        return Ok(None);
    };
    let tpl = deps
        .store
        .get_export_template(tenant_id, tid)
        .await?
        .ok_or(AppError::NotFound("TEMPLATE_NOT_FOUND"))?;
    if tpl.export_format != format {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "template export_format does not match job export_format",
        ));
    }
    let full = resolve_data_root_path(deps.data_root, &tpl.file_path)?;
    Ok(Some(full.to_string_lossy().into_owned()))
}

/// Resolves a stored relative or `AXPOINT_DATA_ROOT`-anchored path safely without parent-directory traversal.
///
/// # Errors
/// Returns `400 INVALID_ARGUMENT` if `stored_path` contains `..` or points outside `data_root`.
pub fn resolve_data_root_path(data_root: &Path, stored_path: &str) -> Result<PathBuf, AppError> {
    let candidate = Path::new(stored_path);
    if candidate
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "path traversal is forbidden",
        ));
    }
    if candidate.is_absolute() {
        let canon_root =
            std::fs::canonicalize(data_root).unwrap_or_else(|_| data_root.to_path_buf());
        let canon_cand =
            std::fs::canonicalize(candidate).unwrap_or_else(|_| candidate.to_path_buf());
        if !canon_cand.starts_with(&canon_root) && !candidate.starts_with(data_root) {
            return Err(AppError::Forbidden("PERMISSION_DENIED"));
        }
        Ok(candidate.to_path_buf())
    } else {
        Ok(data_root.join(candidate))
    }
}

async fn build_source_sections<S>(
    deps: &ExportBuildDeps<'_, S>,
    creator: &AuthPrincipal,
    export_id: Uuid,
    format: ExportFormat,
    source: &ExportSource,
) -> Result<(String, Vec<ExportSection>, Option<PathBuf>), AppError>
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
{
    let policy = crate::policy::build_policy_context_for_store(
        deps.store,
        deps.cedar,
        creator,
        false,
        "127.0.0.1",
    )
    .await?;

    match source {
        ExportSource::Dashboard {
            dashboard_id,
            filters,
        } => {
            let (title, sections) =
                build_dashboard_sections(deps, creator, &policy, *dashboard_id, filters).await?;
            Ok((title, sections, None))
        }
        ExportSource::Widget {
            widget_id,
            filters,
            drill_path,
        } => {
            let (title, sections) = build_single_widget_sections(
                deps, creator, &policy, *widget_id, filters, drill_path,
            )
            .await?;
            Ok((title, sections, None))
        }
        ExportSource::SavedQuery { saved_query_id } => {
            build_saved_query_sections(deps, creator, &policy, export_id, format, *saved_query_id)
                .await
        }
        ExportSource::Pivot { spec } => {
            let section = build_pivot_section(deps, creator, &policy, spec, "透视分析").await?;
            Ok((String::from("透视分析"), vec![section], None))
        }
        ExportSource::Scenario { scenario_ids } => {
            let (title, sections) =
                build_scenario_sections(deps.store, creator, scenario_ids).await?;
            Ok((title, sections, None))
        }
        ExportSource::Conversation { conversation_id } => {
            let (title, sections) =
                build_conversation_sections(deps.store, creator, *conversation_id).await?;
            Ok((title, sections, None))
        }
    }
}

async fn build_dashboard_sections<S>(
    deps: &ExportBuildDeps<'_, S>,
    creator: &AuthPrincipal,
    policy: &axpoint_domain::guard::PolicyContext,
    dashboard_id: Uuid,
    raw_filters: &BTreeMap<String, serde_json::Value>,
) -> Result<(String, Vec<ExportSection>), AppError>
where
    S: ViewRepository
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
    let require_pub = !creator.has(Permission::ViewEdit);
    let detail: DashboardDetail = deps
        .store
        .get_dashboard(&creator.tenant_id, dashboard_id, require_pub)
        .await?
        .ok_or(AppError::NotFound("DASHBOARD_NOT_FOUND"))?;

    let mut effective_filters = BTreeMap::new();
    for def in &detail.dashboard.filters {
        if let Some(ref def_val) = def.default
            && !def_val.is_null()
        {
            effective_filters.insert(def.key.clone(), def_val.clone());
        }
    }
    for (k, v) in raw_filters {
        effective_filters.insert(k.clone(), v.clone());
    }

    let mut sorted_widgets = detail.widgets;
    sorted_widgets.sort_by(|a, b| {
        a.position
            .y
            .cmp(&b.position.y)
            .then_with(|| a.position.x.cmp(&b.position.x))
            .then_with(|| a.created_at.cmp(&b.created_at))
    });

    let is_dark = matches!(
        detail.dashboard.theme,
        DashboardTheme::Dark | DashboardTheme::Screen
    );
    let env = WidgetExecEnv {
        store: deps.store,
        analytics_pool: deps.analytics_pool,
        cache: deps.cache,
        principal: creator,
        policy: Some(policy),
    };

    let mut sections = Vec::with_capacity(sorted_widgets.len());
    for widget in &sorted_widgets {
        if matches!(
            widget.widget_kind,
            WidgetKind::ActionButton | WidgetKind::ActionForm
        ) {
            continue;
        }
        if widget.widget_kind == WidgetKind::MarkdownNote {
            let md = extract_markdown_note(&widget.options);
            if !md.is_empty() {
                sections.push(ExportSection::Markdown { text: md, level: 2 });
            }
            continue;
        }

        let req = WidgetDataRequest {
            filters: effective_filters.clone(),
            drill_path: Vec::new(),
        };
        let data = execute_widget_data(&env, &detail.dashboard, widget, &req).await?;
        sections.push(widget_to_export_section(widget, &data, is_dark));
    }

    Ok((detail.dashboard.name, sections))
}

async fn build_single_widget_sections<S>(
    deps: &ExportBuildDeps<'_, S>,
    creator: &AuthPrincipal,
    policy: &axpoint_domain::guard::PolicyContext,
    widget_id: Uuid,
    filters: &BTreeMap<String, serde_json::Value>,
    drill_path: &[DrillPathItem],
) -> Result<(String, Vec<ExportSection>), AppError>
where
    S: ViewRepository
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
    let widget = deps
        .store
        .get_widget(&creator.tenant_id, widget_id)
        .await?
        .ok_or(AppError::NotFound("WIDGET_NOT_FOUND"))?;
    let require_pub = !creator.has(Permission::ViewEdit);
    let detail = deps
        .store
        .get_dashboard(&creator.tenant_id, widget.dashboard_id, require_pub)
        .await?
        .ok_or(AppError::NotFound("DASHBOARD_NOT_FOUND"))?;

    let title = if widget.title.trim().is_empty() {
        detail.dashboard.name.clone()
    } else {
        widget.title.clone()
    };

    if matches!(
        widget.widget_kind,
        WidgetKind::ActionButton | WidgetKind::ActionForm
    ) {
        return Ok((title, Vec::new()));
    }
    if widget.widget_kind == WidgetKind::MarkdownNote {
        let md = extract_markdown_note(&widget.options);
        return Ok((title, vec![ExportSection::Markdown { text: md, level: 1 }]));
    }

    let env = WidgetExecEnv {
        store: deps.store,
        analytics_pool: deps.analytics_pool,
        cache: deps.cache,
        principal: creator,
        policy: Some(policy),
    };
    let data = execute_widget_data(
        &env,
        &detail.dashboard,
        &widget,
        &WidgetDataRequest {
            filters: filters.clone(),
            drill_path: drill_path.to_vec(),
        },
    )
    .await?;
    let is_dark = matches!(
        detail.dashboard.theme,
        DashboardTheme::Dark | DashboardTheme::Screen
    );
    Ok((
        title,
        vec![widget_to_export_section(&widget, &data, is_dark)],
    ))
}

async fn build_saved_query_sections<S>(
    deps: &ExportBuildDeps<'_, S>,
    creator: &AuthPrincipal,
    policy: &axpoint_domain::guard::PolicyContext,
    export_id: Uuid,
    format: ExportFormat,
    saved_query_id: Uuid,
) -> Result<(String, Vec<ExportSection>, Option<PathBuf>), AppError>
where
    S: ViewRepository
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
    let sq = deps
        .store
        .get_saved_query(&creator.tenant_id, saved_query_id)
        .await?
        .ok_or(AppError::NotFound("SAVED_QUERY_NOT_FOUND"))?;
    check_saved_query_access(creator, &sq)?;

    let max_rows = if matches!(format, ExportFormat::Csv | ExportFormat::Xlsx) {
        EXPORT_SAVED_QUERY_MAX_ROWS
    } else {
        EXPORT_INLINE_ROW_LIMIT
    };

    match sq.kind {
        SavedQueryKind::Sql | SavedQueryKind::Ask => {
            let qr = execute_saved_sql_for_export(deps, creator, policy, &sq, max_rows).await?;
            let (section, spill_path) = query_result_to_table_or_parquet(
                deps.data_root,
                &creator.tenant_id,
                export_id,
                &sq.name,
                qr,
            )
            .await?;
            Ok((sq.name, vec![section], spill_path))
        }
        SavedQueryKind::Pivot => {
            let section = build_pivot_section(deps, creator, policy, &sq.spec, &sq.name).await?;
            Ok((sq.name, vec![section], None))
        }
        SavedQueryKind::Metric => {
            let env = WidgetExecEnv {
                store: deps.store,
                analytics_pool: deps.analytics_pool,
                cache: deps.cache,
                principal: creator,
                policy: Some(policy),
            };
            let preview = execute_widget_preview(
                &env,
                &WidgetPreviewRequest {
                    widget_kind: WidgetKind::Table,
                    data_source: axpoint_domain::view::WidgetDataSource::SavedQuery {
                        saved_query_id,
                    },
                    encoding: serde_json::json!({}),
                    options: serde_json::json!({}),
                    drill: Vec::new(),
                    filters: BTreeMap::new(),
                    drill_path: Vec::new(),
                },
            )
            .await?;
            let table = widget_resp_to_table_section(&sq.name, &preview);
            Ok((sq.name, vec![table], None))
        }
    }
}

async fn execute_saved_sql_for_export<S>(
    deps: &ExportBuildDeps<'_, S>,
    creator: &AuthPrincipal,
    policy: &axpoint_domain::guard::PolicyContext,
    sq: &SavedQuery,
    max_rows_ceiling: u32,
) -> Result<QueryResult, AppError>
where
    S: DatasetRepository + ConnectionRepository + ?Sized,
{
    let sql_text = sq
        .spec
        .get("sql")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::bad("INVALID_ARGUMENT", "saved query spec missing sql"))?;

    let mut params = BTreeMap::new();
    for decl in &sq.parameters {
        if let Some(ref def_val) = decl.default {
            params.insert(decl.name.clone(), def_val.clone());
        }
    }

    let has_live = creator.has(Permission::DataSync);
    let (session, catalog) = acquire_export_session(
        deps.store,
        deps.analytics_pool,
        &creator.tenant_id,
        has_live,
    )
    .await?;

    let guarded = guard_with_ceiling(
        sql_text,
        &catalog,
        &GuardOptions {
            max_rows: max_rows_ceiling,
            allow_live: has_live,
            params,
            policy: Some(policy.clone()),
        },
        max_rows_ceiling,
    )?;
    let out = session
        .execute_guarded(&guarded, &catalog, Some(120_000), false, false)
        .await?;
    Ok(out.result)
}

async fn query_result_to_table_or_parquet(
    data_root: &Path,
    tenant_id: &str,
    export_id: Uuid,
    title: &str,
    qr: QueryResult,
) -> Result<(ExportSection, Option<PathBuf>), AppError> {
    let columns: Vec<ExportColumnSpec> = qr
        .columns
        .iter()
        .map(|c| ExportColumnSpec {
            name: c.name.clone(),
            data_type: c.data_type,
            format: default_column_format(c.data_type),
        })
        .collect();

    let mut rows: Vec<Vec<serde_json::Value>> = Vec::with_capacity(qr.rows.len());
    for row_map in qr.rows {
        let mut row_vec = Vec::with_capacity(qr.columns.len());
        for col in &qr.columns {
            row_vec.push(
                row_map
                    .get(&col.name)
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
            );
        }
        rows.push(row_vec);
    }

    let inline_cap = usize::try_from(EXPORT_INLINE_ROW_LIMIT).unwrap_or(50_000);
    if rows.len() <= inline_cap {
        return Ok((
            ExportSection::Table {
                title: String::from(title),
                columns,
                rows,
                totals: None,
                rows_parquet_path: None,
                row_kinds: Vec::new(),
            },
            None,
        ));
    }

    let export_dir = data_root.join("exports").join(tenant_id);
    tokio::fs::create_dir_all(&export_dir)
        .await
        .map_err(|err| AppError::Config(format!("failed to create export dir: {err}")))?;
    let tmp_path = export_dir.join(format!(".tmp_{export_id}.parquet"));
    write_rows_to_temp_parquet(&qr.columns, rows, &tmp_path).await?;
    let path_str = tmp_path.to_string_lossy().into_owned();

    Ok((
        ExportSection::Table {
            title: String::from(title),
            columns,
            rows: Vec::new(),
            totals: None,
            rows_parquet_path: Some(path_str),
            row_kinds: Vec::new(),
        },
        Some(tmp_path),
    ))
}

async fn write_rows_to_temp_parquet(
    cols: &[QueryColumn],
    rows: Vec<Vec<serde_json::Value>>,
    out_path: &Path,
) -> Result<(), AppError> {
    let cols_owned = cols.to_vec();
    let target = out_path.to_path_buf();
    tokio::task::spawn_blocking(move || write_parquet_sync(&cols_owned, &rows, &target))
        .await
        .map_err(|err| AppError::Database(format!("parquet spill task join failed: {err}")))?
}

fn write_parquet_sync(
    cols: &[QueryColumn],
    rows: &[Vec<serde_json::Value>],
    out_path: &Path,
) -> Result<(), AppError> {
    let conn = axpoint_analytics::duckdb::Connection::open_in_memory()
        .map_err(|err| AppError::Config(format!("duckdb temp parquet open failed: {err}")))?;

    let col_defs: Vec<String> = if cols.is_empty() {
        vec![String::from("\"col_0\" VARCHAR")]
    } else {
        cols.iter()
            .enumerate()
            .map(|(idx, c)| {
                let col_ident = format!("c_{idx}");
                let duck_ty = match c.data_type {
                    DataType::Int64 => "BIGINT",
                    DataType::Float64 => "DOUBLE",
                    DataType::Boolean => "BOOLEAN",
                    _ => "VARCHAR",
                };
                format!("\"{col_ident}\" {duck_ty}")
            })
            .collect()
    };

    let create_sql = format!("CREATE TABLE export_spill ({});", col_defs.join(", "));
    conn.execute_batch(&create_sql)
        .map_err(|err| AppError::Database(format!("create export_spill failed: {err}")))?;

    if !cols.is_empty() && !rows.is_empty() {
        append_rows_to_duckdb(&conn, cols, rows)?;
    }

    let select_aliases: Vec<String> = if cols.is_empty() {
        vec![String::from("\"col_0\"")]
    } else {
        cols.iter()
            .enumerate()
            .map(|(idx, c)| {
                let escaped_alias = c.name.replace('"', "\"\"");
                format!("\"c_{idx}\" AS \"{escaped_alias}\"")
            })
            .collect()
    };
    let escaped_path = out_path.to_string_lossy().replace('\'', "''");
    let copy_sql = format!(
        "COPY (SELECT {} FROM export_spill) TO '{escaped_path}' (FORMAT PARQUET);",
        select_aliases.join(", ")
    );
    conn.execute_batch(&copy_sql)
        .map_err(|err| AppError::Database(format!("duckdb COPY TO PARQUET failed: {err}")))
}

fn append_rows_to_duckdb(
    conn: &axpoint_analytics::duckdb::Connection,
    cols: &[QueryColumn],
    rows: &[Vec<serde_json::Value>],
) -> Result<(), AppError> {
    use axpoint_analytics::duckdb::types::Value as DuckVal;

    let mut appender = conn
        .appender("export_spill")
        .map_err(|err| AppError::Database(format!("duckdb appender init failed: {err}")))?;

    for row in rows {
        let mut vals: Vec<DuckVal> = Vec::with_capacity(cols.len());
        for (idx, col) in cols.iter().enumerate() {
            let cell = row.get(idx).unwrap_or(&serde_json::Value::Null);
            let dv = match (col.data_type, cell) {
                (_, serde_json::Value::Null) => DuckVal::Null,
                (DataType::Int64, serde_json::Value::Number(n)) => n
                    .as_i64()
                    .map_or_else(|| DuckVal::Text(n.to_string()), DuckVal::BigInt),
                (DataType::Float64, serde_json::Value::Number(n)) => {
                    n.as_f64().map_or(DuckVal::Null, DuckVal::Double)
                }
                (DataType::Boolean, serde_json::Value::Bool(b)) => DuckVal::Boolean(*b),
                (_, serde_json::Value::String(s)) => DuckVal::Text(s.clone()),
                (_, other) => DuckVal::Text(other.to_string()),
            };
            vals.push(dv);
        }
        appender
            .append_row(axpoint_analytics::duckdb::appender_params_from_iter(vals))
            .map_err(|err| AppError::Database(format!("duckdb appender row failed: {err}")))?;
    }
    appender
        .flush()
        .map_err(|err| AppError::Database(format!("duckdb appender flush failed: {err}")))
}

async fn build_pivot_section<S>(
    deps: &ExportBuildDeps<'_, S>,
    creator: &AuthPrincipal,
    policy: &axpoint_domain::guard::PolicyContext,
    spec_json: &serde_json::Value,
    title: &str,
) -> Result<ExportSection, AppError>
where
    S: DatasetRepository + ConnectionRepository + ?Sized,
{
    let mut pivot_spec: PivotSpec = serde_json::from_value(spec_json.clone())
        .map_err(|err| AppError::bad("INVALID_ARGUMENT", format!("invalid pivot spec: {err}")))?;
    if pivot_spec.row_limit == 0 {
        pivot_spec.row_limit = 5_000;
    }

    let (session, catalog) =
        acquire_export_session(deps.store, deps.analytics_pool, &creator.tenant_id, false).await?;
    let pr: PivotResult = execute_pivot(
        &session,
        &catalog,
        &pivot_spec,
        PivotExecuteOptions {
            timeout_ms: Some(60_000),
            reveal_pii: false,
            has_pii_raw_read: false,
            policy: Some(policy.clone()),
        },
    )
    .await?;

    Ok(pivot_result_to_export_table(title, &pivot_spec, pr))
}

fn pivot_result_to_export_table(title: &str, spec: &PivotSpec, pr: PivotResult) -> ExportSection {
    let row_dim_count = pr.row_headers.first().map_or(spec.rows.len(), Vec::len);
    let mut columns = Vec::with_capacity(row_dim_count.saturating_add(pr.column_headers.len()));
    for idx in 0..row_dim_count {
        let name = spec
            .rows
            .get(idx)
            .map_or_else(|| format!("row_{idx}"), |r| r.field.clone());
        columns.push(ExportColumnSpec {
            name,
            data_type: DataType::String,
            format: None,
        });
    }
    for ch in &pr.column_headers {
        let name = if ch.is_empty() {
            String::from("value")
        } else {
            ch.join(" / ")
        };
        columns.push(ExportColumnSpec {
            name,
            data_type: DataType::Decimal,
            format: Some(String::from("#,##0.00")),
        });
    }

    let mut rows = Vec::with_capacity(pr.cells.len());
    let mut row_kinds = Vec::with_capacity(pr.cells.len());
    let mut totals: Option<Vec<serde_json::Value>> = None;

    for (idx, (rh, cells)) in pr.row_headers.into_iter().zip(pr.cells).enumerate() {
        let mut row = Vec::with_capacity(columns.len());
        for label in rh {
            row.push(serde_json::Value::String(label));
        }
        for cell in cells {
            row.push(serde_json::Value::String(cell));
        }
        let rk = match pr.row_kinds.get(idx).copied().unwrap_or(PivotRowKind::Data) {
            PivotRowKind::Data => "DETAIL",
            PivotRowKind::Subtotal => "SUBTOTAL",
            PivotRowKind::GrandTotal => "TOTAL",
        };
        if rk == "TOTAL" {
            totals = Some(row.clone());
        }
        row_kinds.push(String::from(rk));
        rows.push(row);
    }

    ExportSection::Table {
        title: String::from(title),
        columns,
        rows,
        totals,
        rows_parquet_path: None,
        row_kinds,
    }
}

async fn build_scenario_sections<S>(
    store: &S,
    creator: &AuthPrincipal,
    scenario_ids: &[Uuid],
) -> Result<(String, Vec<ExportSection>), AppError>
where
    S: SimRepository + ?Sized,
{
    if !(1..=5).contains(&scenario_ids.len()) {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "scenario_ids must contain 1..=5 scenario UUIDs",
        ));
    }

    let mut scenarios: Vec<ScenarioRecord> = Vec::with_capacity(scenario_ids.len());
    for &sid in scenario_ids {
        let sc = store
            .get_scenario(&creator.tenant_id, sid)
            .await?
            .ok_or(AppError::NotFound("SCENARIO_NOT_FOUND"))?;
        scenarios.push(sc);
    }

    let default_title = if scenarios.len() == 1 {
        scenarios
            .first()
            .map_or_else(|| String::from("推演方案报告"), |s| s.name.clone())
    } else {
        String::from("推演方案对比报告")
    };

    let mut sections = Vec::new();
    let mut summary_rows = Vec::with_capacity(scenarios.len());
    let mut impact_rows = Vec::new();

    for sc in &scenarios {
        let impacts = store
            .list_all_scenario_impacts(&creator.tenant_id, sc.id)
            .await?;
        summary_rows.push(vec![
            serde_json::Value::String(sc.name.clone()),
            serde_json::Value::String(String::from(sc.status.as_str())),
            serde_json::Value::Number(serde_json::Number::from(impacts.len())),
        ]);
        for imp in impacts {
            impact_rows.push(vec![
                serde_json::Value::String(sc.name.clone()),
                serde_json::Value::String(imp.type_name),
                serde_json::Value::String(imp.title),
                serde_json::Value::String(imp.metric),
                opt_f64_val(imp.before_val),
                opt_f64_val(Some(imp.after_val)),
                opt_f64_val(imp.delta),
                opt_f64_val(imp.delta_ratio),
            ]);
        }
    }

    sections.push(ExportSection::Heading {
        text: String::from("方案概览"),
        level: 2,
    });
    sections.push(ExportSection::Table {
        title: String::from("推演方案概览"),
        columns: vec![
            col_spec("方案名称", DataType::String),
            col_spec("状态", DataType::String),
            col_spec("影响指标数", DataType::Int64),
        ],
        rows: summary_rows,
        totals: None,
        rows_parquet_path: None,
        row_kinds: Vec::new(),
    });
    sections.push(ExportSection::Table {
        title: String::from("节点影响对比明细"),
        columns: vec![
            col_spec("方案名称", DataType::String),
            col_spec("对象类型", DataType::String),
            col_spec("对象名称", DataType::String),
            col_spec("指标", DataType::String),
            col_spec("推演前", DataType::Float64),
            col_spec("推演后", DataType::Float64),
            col_spec("变化量", DataType::Float64),
            col_spec("变化率", DataType::Float64),
        ],
        rows: impact_rows,
        totals: None,
        rows_parquet_path: None,
        row_kinds: Vec::new(),
    });

    Ok((default_title, sections))
}

async fn build_conversation_sections<S>(
    store: &S,
    creator: &AuthPrincipal,
    conversation_id: Uuid,
) -> Result<(String, Vec<ExportSection>), AppError>
where
    S: AgentRepository + AnalyticsRepository + ?Sized,
{
    if let Some(conv) = store
        .get_conversation(&creator.tenant_id, conversation_id)
        .await?
    {
        if conv.user_id != Some(creator.user_id) && !creator.has(Permission::DataAdmin) {
            return Err(AppError::NotFound("CONVERSATION_NOT_FOUND"));
        }
        let msgs = store
            .list_conversation_messages(&creator.tenant_id, conversation_id)
            .await?;
        let mut sections = Vec::new();
        for msg in msgs {
            match msg.role {
                MessageRole::User => {
                    sections.push(ExportSection::Heading {
                        text: format!("提问：{}", msg.content.trim()),
                        level: 2,
                    });
                }
                MessageRole::Assistant => {
                    if !msg.content.trim().is_empty() {
                        sections.push(ExportSection::Markdown {
                            text: msg.content,
                            level: 3,
                        });
                    }
                }
                MessageRole::Tool | MessageRole::System => {}
            }
        }
        let title = if conv.title.trim().is_empty() {
            String::from("智能体对话记录")
        } else {
            conv.title
        };
        return Ok((title, sections));
    }

    let turns = store
        .list_session_turns(&creator.tenant_id, conversation_id)
        .await?;
    if turns.is_empty() {
        return Err(AppError::NotFound("CONVERSATION_NOT_FOUND"));
    }
    Ok((String::from("智能问数记录"), ask_turns_to_sections(&turns)))
}

fn ask_turns_to_sections(turns: &[AskTurnRecord]) -> Vec<ExportSection> {
    let mut sections = Vec::new();
    for (idx, turn) in turns.iter().enumerate() {
        sections.push(ExportSection::Heading {
            text: format!("Q{}: {}", idx.saturating_add(1), turn.question),
            level: 2,
        });
        if !turn.answer.trim().is_empty() {
            sections.push(ExportSection::Markdown {
                text: turn.answer.clone(),
                level: 3,
            });
        }
        let effective_sql = turn
            .corrected_sql
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| turn.sql_text.as_deref().filter(|s| !s.trim().is_empty()));
        if let Some(sql) = effective_sql {
            sections.push(ExportSection::Markdown {
                text: format!("```sql\n{sql}\n```"),
                level: 3,
            });
        }
    }
    sections
}

async fn acquire_export_session<S>(
    store: &S,
    analytics_pool: &SessionPool,
    tenant_id: &str,
    has_live_permission: bool,
) -> Result<(Arc<TenantDb>, TenantCatalog), AppError>
where
    S: DatasetRepository + ConnectionRepository + ?Sized,
{
    let mut all_datasets = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let page = store
            .list_datasets(
                tenant_id,
                DatasetListQuery {
                    limit: Some(500),
                    cursor,
                    ..DatasetListQuery::default()
                },
            )
            .await?;
        all_datasets.extend(page.items);
        if page.next_cursor.is_none() {
            break;
        }
        cursor = page.next_cursor;
    }

    let mut catalog = TenantCatalog::new(tenant_id);
    catalog.has_live_permission = has_live_permission;
    for ds in &all_datasets {
        if ds.status == DatasetStatus::Disabled {
            continue;
        }
        let col_metas = store.list_columns(tenant_id, ds.id).await?;
        catalog.insert_dataset(CatalogDatasetEntry::from_dataset(ds, &col_metas));
    }

    let conns = store.list_connections(tenant_id).await?;
    let live_specs = LiveConnectionSpec::from_connections(&conns);
    for live in &live_specs {
        catalog.insert_live_connection(&live.name);
    }

    let session = analytics_pool
        .get_or_try_init(tenant_id, catalog, &live_specs)
        .await?;
    let mut snap = session.catalog();
    snap.has_live_permission = has_live_permission;
    Ok((session, snap))
}

fn widget_to_export_section(
    widget: &WidgetRecord,
    data: &WidgetDataResponse,
    is_dark: bool,
) -> ExportSection {
    match widget.widget_kind {
        WidgetKind::KpiCard => build_kpi_section(widget, data),
        WidgetKind::LineChart
        | WidgetKind::BarChart
        | WidgetKind::PieChart
        | WidgetKind::Scatter
        | WidgetKind::Sankey
        | WidgetKind::Heatmap
        | WidgetKind::Gauge
        | WidgetKind::Radar => {
            let echarts_option = chart_builder::build_echarts_option(widget, data, is_dark);
            let data_table = Some(widget_resp_to_embedded_table(&widget.title, data));
            ExportSection::Chart {
                title: widget.title.clone(),
                widget_kind: widget.widget_kind,
                echarts_option,
                data_table,
                width_px: 960,
                height_px: 540,
            }
        }
        WidgetKind::PivotTable => build_pivot_widget_table(&widget.title, data),
        WidgetKind::GraphTopology | WidgetKind::Map => {
            build_node_filtered_table(&widget.title, data)
        }
        WidgetKind::Table
        | WidgetKind::FloorPlan
        | WidgetKind::Scene3d
        | WidgetKind::AlertList
        | WidgetKind::SimCompare => widget_resp_to_table_section(&widget.title, data),
        WidgetKind::MarkdownNote => ExportSection::Markdown {
            text: extract_markdown_note(&widget.options),
            level: 2,
        },
        WidgetKind::ActionButton | WidgetKind::ActionForm => ExportSection::Paragraph {
            text: String::new(),
            level: 1,
        },
    }
}

fn build_kpi_section(widget: &WidgetRecord, data: &WidgetDataResponse) -> ExportSection {
    let label = if widget.title.trim().is_empty() {
        String::from("核心指标")
    } else {
        widget.title.clone()
    };

    let col_map = build_col_index_map(&data.columns);
    let val_field = read_opt_str(&widget.encoding, "value").unwrap_or("value");
    let cmp_field = read_opt_str(&widget.encoding, "compare_value").unwrap_or("compare_value");

    let val_idx = lookup_col_idx(&col_map, val_field).unwrap_or(0);
    let first_row = data.rows.first();
    let raw_val = first_row.and_then(|r| r.get(val_idx));
    let val_dt = data
        .columns
        .get(val_idx)
        .map_or(DataType::Float64, |c| c.data_type);

    let formatted_core = format_kpi_scalar(raw_val, val_dt, &widget.options);
    let prefix = read_opt_str(&widget.options, "prefix").unwrap_or("");
    let suffix = read_opt_str(&widget.options, "suffix").unwrap_or("");
    let unit = read_opt_str(&widget.options, "unit").unwrap_or("");

    let mut value = String::new();
    if !prefix.is_empty() {
        value.push_str(prefix);
    }
    value.push_str(&formatted_core);
    if !suffix.is_empty() {
        value.push_str(suffix);
    }
    if !unit.is_empty() {
        value.push(' ');
        value.push_str(unit);
    }

    let cmp_idx = lookup_col_idx(&col_map, cmp_field)
        .or_else(|| (data.columns.len() > 1 && val_idx == 0).then_some(1));
    let compare = cmp_idx
        .and_then(|idx| first_row.and_then(|r| r.get(idx)))
        .filter(|v| !v.is_null())
        .map(|v| format_kpi_scalar(Some(v), DataType::Float64, &widget.options));

    ExportSection::Kpi {
        label,
        value,
        compare,
    }
}

fn format_kpi_scalar(
    val: Option<&serde_json::Value>,
    data_type: DataType,
    options: &serde_json::Value,
) -> String {
    let Some(v) = val else {
        return String::from("0");
    };
    let explicit_dec = options
        .get("decimals")
        .and_then(serde_json::Value::as_u64)
        .and_then(|d| usize::try_from(d.min(8)).ok());

    match v {
        serde_json::Value::Null => String::from("0"),
        serde_json::Value::Number(n) => {
            if data_type == DataType::Int64
                && explicit_dec.is_none()
                && let Some(i) = n.as_i64()
            {
                return i.to_string();
            }
            if let Some(f) = n.as_f64() {
                let dec = explicit_dec.unwrap_or(2);
                return format!("{f:.dec$}");
            }
            n.to_string()
        }
        serde_json::Value::String(s) => {
            let trimmed = s.trim();
            if let Ok(f) = trimmed.parse::<f64>() {
                if data_type == DataType::Int64
                    && explicit_dec.is_none()
                    && let Ok(i) = trimmed.parse::<i64>()
                {
                    return i.to_string();
                }
                let dec = explicit_dec.unwrap_or(2);
                return format!("{f:.dec$}");
            }
            String::from(trimmed)
        }
        other => other.to_string(),
    }
}

fn build_pivot_widget_table(title: &str, data: &WidgetDataResponse) -> ExportSection {
    let rk_idx = data
        .columns
        .iter()
        .position(|c| c.name.eq_ignore_ascii_case("_row_kind"));
    let columns: Vec<ExportColumnSpec> = data
        .columns
        .iter()
        .enumerate()
        .filter(|(idx, _)| Some(*idx) != rk_idx)
        .map(|(_, c)| ExportColumnSpec {
            name: c.name.clone(),
            data_type: c.data_type,
            format: default_column_format(c.data_type),
        })
        .collect();

    let mut rows = Vec::with_capacity(data.rows.len());
    let mut row_kinds = Vec::with_capacity(data.rows.len());
    let mut totals: Option<Vec<serde_json::Value>> = None;

    for r in &data.rows {
        let rk = rk_idx
            .and_then(|idx| r.get(idx))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("DETAIL");
        let clean_row: Vec<serde_json::Value> = r
            .iter()
            .enumerate()
            .filter(|(idx, _)| Some(*idx) != rk_idx)
            .map(|(_, v)| v.clone())
            .collect();
        if rk == "TOTAL" {
            totals = Some(clean_row.clone());
        }
        row_kinds.push(String::from(rk));
        rows.push(clean_row);
    }

    ExportSection::Table {
        title: String::from(title),
        columns,
        rows,
        totals,
        rows_parquet_path: None,
        row_kinds,
    }
}

fn build_node_filtered_table(title: &str, data: &WidgetDataResponse) -> ExportSection {
    let rk_idx = data
        .columns
        .iter()
        .position(|c| c.name.eq_ignore_ascii_case("record_kind"));
    let Some(kind_col) = rk_idx else {
        return widget_resp_to_table_section(title, data);
    };

    let skip_names: &[&str] = &[
        "record_kind",
        "src_node_id",
        "dst_node_id",
        "link_type",
        "weight",
        "src_lng",
        "src_lat",
        "dst_lng",
        "dst_lat",
    ];
    let kept_indices: Vec<usize> = data
        .columns
        .iter()
        .enumerate()
        .filter(|(_, c)| {
            !skip_names
                .iter()
                .any(|skip| c.name.eq_ignore_ascii_case(skip))
        })
        .map(|(i, _)| i)
        .collect();

    let columns: Vec<ExportColumnSpec> = kept_indices
        .iter()
        .filter_map(|&i| data.columns.get(i))
        .map(|c| ExportColumnSpec {
            name: c.name.clone(),
            data_type: c.data_type,
            format: default_column_format(c.data_type),
        })
        .collect();

    let mut rows = Vec::new();
    for r in &data.rows {
        let is_node = r
            .get(kind_col)
            .and_then(serde_json::Value::as_str)
            .is_none_or(|s| s.eq_ignore_ascii_case("NODE"));
        if !is_node {
            continue;
        }
        let filtered: Vec<serde_json::Value> = kept_indices
            .iter()
            .map(|&i| r.get(i).cloned().unwrap_or(serde_json::Value::Null))
            .collect();
        rows.push(filtered);
    }

    ExportSection::Table {
        title: String::from(title),
        columns,
        rows,
        totals: None,
        rows_parquet_path: None,
        row_kinds: Vec::new(),
    }
}

fn widget_resp_to_embedded_table(title: &str, data: &WidgetDataResponse) -> ExportEmbeddedTable {
    let columns = data
        .columns
        .iter()
        .map(|c| ExportColumnSpec {
            name: c.name.clone(),
            data_type: c.data_type,
            format: default_column_format(c.data_type),
        })
        .collect();
    ExportEmbeddedTable {
        kind: ExportTableKind::Table,
        title: String::from(title),
        columns,
        rows: data.rows.clone(),
        totals: None,
        rows_parquet_path: None,
        row_kinds: Vec::new(),
    }
}

fn widget_resp_to_table_section(title: &str, data: &WidgetDataResponse) -> ExportSection {
    let embedded = widget_resp_to_embedded_table(title, data);
    ExportSection::Table {
        title: embedded.title,
        columns: embedded.columns,
        rows: embedded.rows,
        totals: embedded.totals,
        rows_parquet_path: embedded.rows_parquet_path,
        row_kinds: embedded.row_kinds,
    }
}

fn normalize_sections_for_csv(doc_title: &str, sections: Vec<ExportSection>) -> Vec<ExportSection> {
    let mut kpi_rows = Vec::new();
    let mut out = Vec::new();

    for sec in sections {
        match sec {
            ExportSection::Kpi {
                label,
                value,
                compare,
            } => {
                kpi_rows.push(vec![
                    serde_json::Value::String(label),
                    serde_json::Value::String(value),
                    compare.map_or(serde_json::Value::Null, serde_json::Value::String),
                ]);
            }
            ExportSection::Chart {
                title,
                data_table: Some(tbl),
                ..
            } => {
                out.push(ExportSection::Table {
                    title: if tbl.title.is_empty() {
                        title
                    } else {
                        tbl.title
                    },
                    columns: tbl.columns,
                    rows: tbl.rows,
                    totals: tbl.totals,
                    rows_parquet_path: tbl.rows_parquet_path,
                    row_kinds: tbl.row_kinds,
                });
            }
            ExportSection::Table { .. } => out.push(sec),
            ExportSection::Chart {
                data_table: None, ..
            }
            | ExportSection::Heading { .. }
            | ExportSection::Paragraph { .. }
            | ExportSection::Markdown { .. } => {}
        }
    }

    if !kpi_rows.is_empty() {
        let mut with_kpi = Vec::with_capacity(out.len().saturating_add(1));
        with_kpi.push(ExportSection::Table {
            title: format!("{doc_title} - 指标概览"),
            columns: vec![
                col_spec("指标名称", DataType::String),
                col_spec("指标值", DataType::String),
                col_spec("对比", DataType::String),
            ],
            rows: kpi_rows,
            totals: None,
            rows_parquet_path: None,
            row_kinds: Vec::new(),
        });
        with_kpi.extend(out);
        return with_kpi;
    }

    out
}

fn count_section_rows(sections: &[ExportSection]) -> usize {
    let mut total = 0_usize;
    for sec in sections {
        match sec {
            ExportSection::Table { rows, .. } => {
                total = total.saturating_add(rows.len());
            }
            ExportSection::Chart {
                data_table: Some(tbl),
                ..
            } => {
                total = total.saturating_add(tbl.rows.len());
            }
            ExportSection::Kpi { .. } => {
                total = total.saturating_add(1);
            }
            _ => {}
        }
    }
    total
}

fn default_column_format(dt: DataType) -> Option<String> {
    match dt {
        DataType::Decimal | DataType::Float64 => Some(String::from("#,##0.00")),
        DataType::Date => Some(String::from("yyyy-mm-dd")),
        _ => None,
    }
}

fn col_spec(name: &str, data_type: DataType) -> ExportColumnSpec {
    ExportColumnSpec {
        name: String::from(name),
        data_type,
        format: default_column_format(data_type),
    }
}

fn opt_f64_val(val: Option<f64>) -> serde_json::Value {
    val.and_then(serde_json::Number::from_f64)
        .map_or(serde_json::Value::Null, serde_json::Value::Number)
}

fn extract_markdown_note(options: &serde_json::Value) -> String {
    for key in ["markdown", "content", "text"] {
        if let Some(s) = options.get(key).and_then(serde_json::Value::as_str)
            && !s.trim().is_empty()
        {
            return String::from(s.trim());
        }
    }
    String::new()
}

fn build_col_index_map(cols: &[WidgetColumn]) -> HashMap<String, usize> {
    let mut map = HashMap::with_capacity(cols.len().saturating_mul(2));
    for (idx, col) in cols.iter().enumerate() {
        map.entry(col.name.clone()).or_insert(idx);
        map.entry(col.name.to_ascii_lowercase()).or_insert(idx);
    }
    map
}

fn lookup_col_idx(map: &HashMap<String, usize>, name: &str) -> Option<usize> {
    map.get(name)
        .copied()
        .or_else(|| map.get(&name.to_ascii_lowercase()).copied())
}

fn read_opt_str<'a>(obj: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    obj.get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// Removes a file if it exists, ignoring `NotFound`.
pub async fn cleanup_file_quietly(path: &Path) {
    if let Err(err) = tokio::fs::remove_file(path).await
        && err.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(path = %path.display(), error = %err, "failed to clean up file");
    }
}

/// Returns the canonical HTTP `Content-Type` for a downloaded export artifact.
#[must_use]
pub fn content_type_for_export(file_name: &str, fallback_format: ExportFormat) -> &'static str {
    if let Some(ext) = Path::new(file_name).extension().and_then(|e| e.to_str()) {
        if ext.eq_ignore_ascii_case("zip") {
            return "application/zip";
        }
        if ext.eq_ignore_ascii_case("xlsx") {
            return "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet";
        }
        if ext.eq_ignore_ascii_case("csv") {
            return "text/csv; charset=utf-8";
        }
        if ext.eq_ignore_ascii_case("pdf") {
            return "application/pdf";
        }
        if ext.eq_ignore_ascii_case("docx") {
            return "application/vnd.openxmlformats-officedocument.wordprocessingml.document";
        }
        if ext.eq_ignore_ascii_case("pptx") {
            return "application/vnd.openxmlformats-officedocument.presentationml.presentation";
        }
        if ext.eq_ignore_ascii_case("md") || ext.eq_ignore_ascii_case("markdown") {
            return "text/markdown; charset=utf-8";
        }
    }
    match fallback_format {
        ExportFormat::Xlsx => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        ExportFormat::Csv => "text/csv; charset=utf-8",
        ExportFormat::Pdf => "application/pdf",
        ExportFormat::Docx => {
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
        }
        ExportFormat::Pptx => {
            "application/vnd.openxmlformats-officedocument.presentationml.presentation"
        }
        ExportFormat::Markdown => "text/markdown; charset=utf-8",
    }
}

/// Formats an RFC 5987 `Content-Disposition: attachment; filename="..."; filename*=UTF-8''...` header value.
#[must_use]
pub fn build_content_disposition(file_name: &str) -> String {
    let clean_name = file_name
        .chars()
        .filter(|c| !c.is_control())
        .collect::<String>();
    let fallback_name = if clean_name.trim().is_empty() {
        String::from("export.bin")
    } else {
        clean_name
    };

    let ascii_safe: String = fallback_name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect();

    let mut pct = String::with_capacity(fallback_name.len().saturating_mul(3));
    for &b in fallback_name.as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            pct.push(char::from(b));
        } else {
            let hi = hex_nibble(b >> 4);
            let lo = hex_nibble(b & 0x0F);
            pct.push('%');
            pct.push(hi);
            pct.push(lo);
        }
    }

    format!("attachment; filename=\"{ascii_safe}\"; filename*=UTF-8''{pct}")
}

const fn hex_nibble(nibble: u8) -> char {
    match nibble & 0x0F {
        0 => '0',
        1 => '1',
        2 => '2',
        3 => '3',
        4 => '4',
        5 => '5',
        6 => '6',
        7 => '7',
        8 => '8',
        9 => '9',
        10 => 'A',
        11 => 'B',
        12 => 'C',
        13 => 'D',
        14 => 'E',
        _ => 'F',
    }
}

/// Validates an uploaded `.docx` or `.pptx` ZIP archive and extracts deduplicated `{{placeholder}}`
/// keys from its XML parts (`word/*.xml` or `ppt/**/*.xml`), coalescing split XML text runs.
///
/// # Errors
/// - Returns `413 UPLOAD_TOO_LARGE` if the archive or decompressed XML exceeds size limits.
/// - Returns `415 UNSUPPORTED_FORMAT` if the file lacks a valid ZIP magic header.
/// - Returns `400 INVALID_ARGUMENT` if the ZIP structure, DEFLATE stream, or OOXML layout is invalid.
pub fn extract_ooxml_placeholders(
    format: ExportFormat,
    raw_bytes: &[u8],
) -> Result<Vec<String>, AppError> {
    if raw_bytes.is_empty() {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "uploaded template file cannot be empty",
        ));
    }
    if raw_bytes.len() > MAX_EXPORT_TEMPLATE_BYTES {
        return Err(AppError::PayloadTooLarge("UPLOAD_TOO_LARGE"));
    }
    if !raw_bytes.starts_with(b"PK\x03\x04") && !raw_bytes.starts_with(b"PK\x05\x06") {
        return Err(AppError::Unsupported("UNSUPPORTED_FORMAT"));
    }
    let required_prefix = match format {
        ExportFormat::Docx => "word/",
        ExportFormat::Pptx => "ppt/",
        _ => {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "only DOCX and PPTX templates are supported",
            ));
        }
    };

    zip_ooxml::scan_archive_placeholders(raw_bytes, required_prefix)
}

mod chart_builder {
    use std::collections::{HashMap, HashSet};
    use std::sync::OnceLock;

    use axpoint_domain::enums::{DataType, WidgetKind};
    use axpoint_domain::view::{WidgetDataResponse, WidgetRecord};
    use serde::Deserialize;
    use serde_json::{Value, json};

    use super::{CHART_SPEC_JSON, build_col_index_map, lookup_col_idx, read_opt_str};

    #[derive(Debug, Deserialize)]
    struct ChartSpec {
        palette: ThemePalettes,
        tokens: ThemeTokensMap,
        #[serde(rename = "fontFamily")]
        font_family: String,
        grid: GridSpec,
        scatter: ScatterSpec,
        pie: PieSpec,
        gauge: GaugeSpec,
        #[serde(rename = "encodingFallbacks")]
        encoding_fallbacks: EncodingFallbacks,
    }

    #[derive(Debug, Deserialize)]
    struct ThemePalettes {
        light: Vec<String>,
        dark: Vec<String>,
    }

    #[derive(Debug, Deserialize)]
    struct ThemeTokensMap {
        light: ThemeTokens,
        dark: ThemeTokens,
    }

    #[derive(Debug, Deserialize)]
    struct ThemeTokens {
        text: String,
        subtext: String,
        #[serde(rename = "axisLine")]
        axis_line: String,
        #[serde(rename = "splitLine")]
        split_line: String,
        #[serde(rename = "tooltipBg")]
        tooltip_bg: String,
        #[serde(rename = "tooltipBorder")]
        tooltip_border: String,
    }

    #[derive(Debug, Deserialize)]
    struct GridSpec {
        top: u32,
        right: u32,
        bottom: u32,
        left: u32,
        #[serde(rename = "containLabel")]
        contain_label: bool,
    }

    #[derive(Debug, Deserialize)]
    struct ScatterSpec {
        #[serde(rename = "defaultSymbolSize")]
        default: f64,
        #[serde(rename = "minSymbolSize")]
        min: f64,
        #[serde(rename = "maxSymbolSize")]
        max: f64,
    }

    #[derive(Debug, Deserialize)]
    struct PieSpec {
        #[serde(rename = "innerRadius")]
        inner_radius: String,
        #[serde(rename = "outerRadius")]
        outer_radius: String,
    }

    #[derive(Debug, Deserialize)]
    struct GaugeSpec {
        #[serde(rename = "defaultMin")]
        default_min: f64,
        #[serde(rename = "defaultMax")]
        default_max: f64,
    }

    #[derive(Debug, Deserialize)]
    struct EncodingFallbacks {
        #[serde(rename = "categoryCandidates")]
        category: Vec<String>,
        #[serde(rename = "timeCandidates")]
        time: Vec<String>,
        #[serde(rename = "seriesCandidates")]
        series: Vec<String>,
        #[serde(rename = "valueCandidates")]
        value: Vec<String>,
    }

    fn spec() -> Option<&'static ChartSpec> {
        static SPEC: OnceLock<Option<ChartSpec>> = OnceLock::new();
        SPEC.get_or_init(|| serde_json::from_str::<ChartSpec>(CHART_SPEC_JSON).ok())
            .as_ref()
    }

    struct ChartThemeCtx<'a> {
        cfg: &'a ChartSpec,
        palette: &'a [String],
        tok: &'a ThemeTokens,
    }

    pub(super) fn build_echarts_option(
        widget: &WidgetRecord,
        data: &WidgetDataResponse,
        is_dark: bool,
    ) -> Value {
        let Some(cfg) = spec() else {
            return json!({ "animation": false, "series": [] });
        };
        let palette = if is_dark {
            &cfg.palette.dark
        } else {
            &cfg.palette.light
        };
        let tok = if is_dark {
            &cfg.tokens.dark
        } else {
            &cfg.tokens.light
        };
        let theme = ChartThemeCtx { cfg, palette, tok };

        match widget.widget_kind {
            WidgetKind::LineChart | WidgetKind::BarChart => build_cartesian(widget, data, &theme),
            WidgetKind::PieChart => build_pie(widget, data, cfg, palette, tok),
            WidgetKind::Scatter => build_scatter(widget, data, cfg, palette, tok),
            WidgetKind::Sankey => build_sankey(widget, data, cfg, palette, tok),
            WidgetKind::Heatmap => build_heatmap(widget, data, cfg, tok, is_dark),
            WidgetKind::Gauge => build_gauge(widget, data, cfg, tok),
            WidgetKind::Radar => build_radar(widget, data, cfg, palette, tok),
            _ => json!({ "animation": false, "series": [] }),
        }
    }

    fn build_cartesian(
        widget: &WidgetRecord,
        data: &WidgetDataResponse,
        theme: &ChartThemeCtx<'_>,
    ) -> Value {
        let col_map = build_col_index_map(&data.columns);
        let x_idx = resolve_x_col(widget, data, &col_map, theme.cfg);
        let series_idx = resolve_series_col(widget, data, &col_map, x_idx, theme.cfg);
        let chart_type = if widget.widget_kind == WidgetKind::LineChart {
            "line"
        } else {
            "bar"
        };

        if let Some(s_idx) = series_idx {
            return build_grouped_cartesian(
                widget,
                data,
                &col_map,
                (x_idx, s_idx),
                chart_type,
                theme,
            );
        }

        let y_indices = resolve_y_cols(widget, data, &col_map, &[x_idx], theme.cfg);
        let categories: Vec<String> = data.rows.iter().map(|r| cell_str(r.get(x_idx))).collect();

        let series: Vec<Value> = y_indices
            .iter()
            .map(|&yi| {
                let name = data
                    .columns
                    .get(yi)
                    .map_or_else(|| String::from("value"), |c| c.name.clone());
                let vals: Vec<f64> = data
                    .rows
                    .iter()
                    .map(|r| cell_num(r.get(yi)).unwrap_or(0.0))
                    .collect();
                json!({
                    "name": name,
                    "type": chart_type,
                    "smooth": chart_type == "line",
                    "barMaxWidth": 40,
                    "data": vals,
                })
            })
            .collect();

        base_cartesian_json(
            &widget.title,
            &categories,
            &series,
            theme.cfg,
            theme.palette,
            theme.tok,
        )
    }

    fn build_grouped_cartesian(
        widget: &WidgetRecord,
        data: &WidgetDataResponse,
        col_map: &HashMap<String, usize>,
        (x_idx, s_idx): (usize, usize),
        chart_type: &str,
        theme: &ChartThemeCtx<'_>,
    ) -> Value {
        let y_indices = resolve_y_cols(widget, data, col_map, &[x_idx, s_idx], theme.cfg);
        let y_idx = y_indices.first().copied().unwrap_or(0);

        let mut categories = Vec::new();
        let mut cat_seen = HashSet::new();
        let mut series_names = Vec::new();
        let mut ser_seen = HashSet::new();
        let mut matrix: HashMap<(String, String), f64> = HashMap::new();

        for r in &data.rows {
            let xv = cell_str(r.get(x_idx));
            let sv = cell_str(r.get(s_idx));
            let yv = cell_num(r.get(y_idx)).unwrap_or(0.0);
            if cat_seen.insert(xv.clone()) {
                categories.push(xv.clone());
            }
            if ser_seen.insert(sv.clone()) {
                series_names.push(sv.clone());
            }
            let entry = matrix.entry((sv, xv)).or_insert(0.0);
            *entry += yv;
        }

        let series: Vec<Value> = series_names
            .into_iter()
            .map(|sn| {
                let vals: Vec<f64> = categories
                    .iter()
                    .map(|cat| {
                        matrix
                            .get(&(sn.clone(), cat.clone()))
                            .copied()
                            .unwrap_or(0.0)
                    })
                    .collect();
                json!({
                    "name": sn,
                    "type": chart_type,
                    "smooth": chart_type == "line",
                    "barMaxWidth": 36,
                    "data": vals,
                })
            })
            .collect();

        base_cartesian_json(
            &widget.title,
            &categories,
            &series,
            theme.cfg,
            theme.palette,
            theme.tok,
        )
    }

    fn base_cartesian_json(
        title: &str,
        categories: &[String],
        series: &[Value],
        cfg: &ChartSpec,
        palette: &[String],
        tok: &ThemeTokens,
    ) -> Value {
        let show_legend = series.len() > 1;
        json!({
            "animation": false,
            "color": palette,
            "textStyle": { "fontFamily": cfg.font_family, "color": tok.text },
            "title": {
                "text": title,
                "left": 12,
                "top": 4,
                "textStyle": { "color": tok.text, "fontSize": 13, "fontWeight": 600 }
            },
            "tooltip": {
                "trigger": "axis",
                "backgroundColor": tok.tooltip_bg,
                "borderColor": tok.tooltip_border
            },
            "legend": {
                "show": show_legend,
                "top": 6,
                "right": 16,
                "textStyle": { "color": tok.subtext, "fontSize": 11 }
            },
            "grid": {
                "top": cfg.grid.top,
                "right": cfg.grid.right,
                "bottom": cfg.grid.bottom,
                "left": cfg.grid.left,
                "containLabel": cfg.grid.contain_label
            },
            "xAxis": {
                "type": "category",
                "data": categories,
                "axisLabel": { "color": tok.subtext, "fontSize": 11 },
                "axisLine": { "lineStyle": { "color": tok.axis_line } }
            },
            "yAxis": {
                "type": "value",
                "axisLabel": { "color": tok.subtext, "fontSize": 11 },
                "splitLine": { "lineStyle": { "color": tok.split_line, "type": "dashed" } }
            },
            "series": series
        })
    }

    fn build_pie(
        widget: &WidgetRecord,
        data: &WidgetDataResponse,
        cfg: &ChartSpec,
        palette: &[String],
        tok: &ThemeTokens,
    ) -> Value {
        let col_map = build_col_index_map(&data.columns);
        let cat_idx = read_opt_str(&widget.encoding, "category")
            .or_else(|| read_opt_str(&widget.encoding, "x"))
            .and_then(|s| lookup_col_idx(&col_map, s))
            .unwrap_or(0);
        let val_idx = read_opt_str(&widget.encoding, "value")
            .or_else(|| read_opt_str(&widget.encoding, "y"))
            .and_then(|s| lookup_col_idx(&col_map, s))
            .or_else(|| first_numeric_col(data, &[cat_idx]))
            .unwrap_or(1);

        let pie_data: Vec<Value> = data
            .rows
            .iter()
            .map(|r| {
                json!({
                    "name": cell_str(r.get(cat_idx)),
                    "value": cell_num(r.get(val_idx)).unwrap_or(0.0),
                })
            })
            .collect();

        json!({
            "animation": false,
            "color": palette,
            "textStyle": { "fontFamily": cfg.font_family, "color": tok.text },
            "title": { "text": widget.title, "left": 12, "top": 4, "textStyle": { "color": tok.text, "fontSize": 13 } },
            "legend": { "bottom": 4, "textStyle": { "color": tok.subtext, "fontSize": 11 } },
            "series": [{
                "type": "pie",
                "radius": [cfg.pie.inner_radius, cfg.pie.outer_radius],
                "center": ["50%", "48%"],
                "label": { "color": tok.text, "fontSize": 11 },
                "data": pie_data
            }]
        })
    }

    fn build_scatter(
        widget: &WidgetRecord,
        data: &WidgetDataResponse,
        cfg: &ChartSpec,
        palette: &[String],
        tok: &ThemeTokens,
    ) -> Value {
        let col_map = build_col_index_map(&data.columns);
        let x_idx = read_opt_str(&widget.encoding, "x")
            .and_then(|s| lookup_col_idx(&col_map, s))
            .unwrap_or(0);
        let y_idx = read_opt_str(&widget.encoding, "y")
            .and_then(|s| lookup_col_idx(&col_map, s))
            .unwrap_or(1);
        let size_idx =
            read_opt_str(&widget.encoding, "size").and_then(|s| lookup_col_idx(&col_map, s));

        let points: Vec<Value> = data
            .rows
            .iter()
            .map(|r| {
                let x = cell_num(r.get(x_idx)).unwrap_or(0.0);
                let y = cell_num(r.get(y_idx)).unwrap_or(0.0);
                let sz = size_idx
                    .and_then(|si| cell_num(r.get(si)))
                    .map_or(cfg.scatter.default, |raw| {
                        raw.clamp(cfg.scatter.min, cfg.scatter.max)
                    });
                json!({
                    "value": [x, y],
                    "symbolSize": sz,
                })
            })
            .collect();

        json!({
            "animation": false,
            "color": palette,
            "textStyle": { "fontFamily": cfg.font_family, "color": tok.text },
            "title": { "text": widget.title, "left": 12, "top": 4, "textStyle": { "color": tok.text, "fontSize": 13 } },
            "grid": {
                "top": cfg.grid.top,
                "right": cfg.grid.right,
                "bottom": cfg.grid.bottom,
                "left": cfg.grid.left,
                "containLabel": true
            },
            "xAxis": { "type": "value", "axisLabel": { "color": tok.subtext }, "splitLine": { "lineStyle": { "color": tok.split_line } } },
            "yAxis": { "type": "value", "axisLabel": { "color": tok.subtext }, "splitLine": { "lineStyle": { "color": tok.split_line } } },
            "series": [{ "type": "scatter", "data": points }]
        })
    }

    fn build_sankey(
        widget: &WidgetRecord,
        data: &WidgetDataResponse,
        cfg: &ChartSpec,
        palette: &[String],
        tok: &ThemeTokens,
    ) -> Value {
        let col_map = build_col_index_map(&data.columns);
        let src_idx = read_opt_str(&widget.encoding, "source")
            .and_then(|s| lookup_col_idx(&col_map, s))
            .unwrap_or(0);
        let dst_idx = read_opt_str(&widget.encoding, "target")
            .and_then(|s| lookup_col_idx(&col_map, s))
            .unwrap_or(1);
        let val_idx = read_opt_str(&widget.encoding, "value")
            .and_then(|s| lookup_col_idx(&col_map, s))
            .unwrap_or(2);

        let mut node_names = Vec::new();
        let mut seen = HashSet::new();
        let mut links = Vec::new();

        for r in &data.rows {
            let s = cell_str(r.get(src_idx));
            let t = cell_str(r.get(dst_idx));
            let v = cell_num(r.get(val_idx)).unwrap_or(1.0).max(0.01);
            if !s.is_empty() && !t.is_empty() && s != t {
                if seen.insert(s.clone()) {
                    node_names.push(json!({ "name": s }));
                }
                if seen.insert(t.clone()) {
                    node_names.push(json!({ "name": t }));
                }
                links.push(json!({ "source": s, "target": t, "value": v }));
            }
        }

        json!({
            "animation": false,
            "color": palette,
            "textStyle": { "fontFamily": cfg.font_family, "color": tok.text },
            "title": { "text": widget.title, "left": 12, "top": 4, "textStyle": { "color": tok.text, "fontSize": 13 } },
            "series": [{
                "type": "sankey",
                "label": { "color": tok.text, "fontSize": 11 },
                "data": node_names,
                "links": links
            }]
        })
    }

    fn build_heatmap(
        widget: &WidgetRecord,
        data: &WidgetDataResponse,
        cfg: &ChartSpec,
        tok: &ThemeTokens,
        is_dark: bool,
    ) -> Value {
        let col_map = build_col_index_map(&data.columns);
        let x_idx = read_opt_str(&widget.encoding, "x")
            .and_then(|s| lookup_col_idx(&col_map, s))
            .unwrap_or(0);
        let y_idx = read_opt_str(&widget.encoding, "y")
            .and_then(|s| lookup_col_idx(&col_map, s))
            .unwrap_or(1);
        let val_idx = read_opt_str(&widget.encoding, "value")
            .and_then(|s| lookup_col_idx(&col_map, s))
            .unwrap_or(2);

        let mut x_cats = Vec::new();
        let mut y_cats = Vec::new();
        let mut x_map = HashMap::new();
        let mut y_map = HashMap::new();
        let mut min_v = f64::INFINITY;
        let mut max_v = f64::NEG_INFINITY;
        let mut heat_data = Vec::new();

        for r in &data.rows {
            let xv = cell_str(r.get(x_idx));
            let yv = cell_str(r.get(y_idx));
            let val = cell_num(r.get(val_idx)).unwrap_or(0.0);
            let xi = *x_map.entry(xv.clone()).or_insert_with(|| {
                let idx = x_cats.len();
                x_cats.push(xv);
                idx
            });
            let yi = *y_map.entry(yv.clone()).or_insert_with(|| {
                let idx = y_cats.len();
                y_cats.push(yv);
                idx
            });
            min_v = min_v.min(val);
            max_v = max_v.max(val);
            heat_data.push(json!([xi, yi, val]));
        }

        let lo = if min_v.is_finite() { min_v } else { 0.0 };
        let hi = if max_v.is_finite() && max_v > lo {
            max_v
        } else {
            lo + 100.0
        };
        let ramp = if is_dark {
            json!(["#18181b", "#3b82f6"])
        } else {
            json!(["#eff6ff", "#2563eb"])
        };

        json!({
            "animation": false,
            "textStyle": { "fontFamily": cfg.font_family, "color": tok.text },
            "grid": { "top": 36, "right": 24, "bottom": 44, "left": 64 },
            "xAxis": { "type": "category", "data": x_cats, "axisLabel": { "color": tok.subtext } },
            "yAxis": { "type": "category", "data": y_cats, "axisLabel": { "color": tok.subtext } },
            "visualMap": { "min": lo, "max": hi, "orient": "horizontal", "left": "center", "bottom": 0, "inRange": { "color": ramp } },
            "series": [{ "type": "heatmap", "data": heat_data }]
        })
    }

    fn build_gauge(
        widget: &WidgetRecord,
        data: &WidgetDataResponse,
        cfg: &ChartSpec,
        tok: &ThemeTokens,
    ) -> Value {
        let col_map = build_col_index_map(&data.columns);
        let val_idx = read_opt_str(&widget.encoding, "value")
            .and_then(|s| lookup_col_idx(&col_map, s))
            .unwrap_or(0);
        let val = data
            .rows
            .first()
            .and_then(|r| cell_num(r.get(val_idx)))
            .unwrap_or(0.0);
        let min = widget
            .options
            .get("min")
            .and_then(Value::as_f64)
            .unwrap_or(cfg.gauge.default_min);
        let max = widget
            .options
            .get("max")
            .and_then(Value::as_f64)
            .unwrap_or(cfg.gauge.default_max);
        let unit = read_opt_str(&widget.options, "unit").unwrap_or("");
        let formatter = if unit.is_empty() {
            String::from("{value}")
        } else {
            format!("{{value}} {unit}")
        };

        json!({
            "animation": false,
            "textStyle": { "fontFamily": cfg.font_family, "color": tok.text },
            "series": [{
                "type": "gauge",
                "min": min,
                "max": max,
                "progress": { "show": true, "width": 10 },
                "detail": { "valueAnimation": false, "formatter": formatter, "color": tok.text, "fontSize": 18 },
                "data": [{ "value": val, "name": widget.title }]
            }]
        })
    }

    fn build_radar(
        widget: &WidgetRecord,
        data: &WidgetDataResponse,
        cfg: &ChartSpec,
        palette: &[String],
        tok: &ThemeTokens,
    ) -> Value {
        let col_map = build_col_index_map(&data.columns);
        let ind_idx = read_opt_str(&widget.encoding, "indicator")
            .and_then(|s| lookup_col_idx(&col_map, s))
            .unwrap_or(0);
        let val_idx = read_opt_str(&widget.encoding, "value")
            .and_then(|s| lookup_col_idx(&col_map, s))
            .unwrap_or(1);
        let ser_idx =
            read_opt_str(&widget.encoding, "series").and_then(|s| lookup_col_idx(&col_map, s));

        let mut indicators = Vec::new();
        let mut ind_seen = HashSet::new();
        let mut series_order = Vec::new();
        let mut series_vals: HashMap<String, HashMap<String, f64>> = HashMap::new();
        let mut max_val = 10.0_f64;

        for r in &data.rows {
            let ind = cell_str(r.get(ind_idx));
            if ind.is_empty() {
                continue;
            }
            let val = cell_num(r.get(val_idx)).unwrap_or(0.0);
            let ser = ser_idx
                .map(|si| cell_str(r.get(si)))
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| widget.title.clone());
            max_val = max_val.max(val * 1.15);
            if ind_seen.insert(ind.clone()) {
                indicators.push(ind.clone());
            }
            if !series_vals.contains_key(&ser) {
                series_order.push(ser.clone());
            }
            series_vals.entry(ser).or_default().insert(ind, val);
        }

        let ind_objs: Vec<Value> = if indicators.is_empty() {
            vec![json!({ "name": "默认", "max": 100 })]
        } else {
            indicators
                .iter()
                .map(|name| json!({ "name": name, "max": max_val.ceil() }))
                .collect()
        };
        let radar_data: Vec<Value> = series_order
            .into_iter()
            .map(|sn| {
                let vmap = series_vals.remove(&sn).unwrap_or_default();
                let vals: Vec<f64> = indicators
                    .iter()
                    .map(|ind| vmap.get(ind).copied().unwrap_or(0.0))
                    .collect();
                json!({ "name": sn, "value": vals })
            })
            .collect();

        json!({
            "animation": false,
            "color": palette,
            "textStyle": { "fontFamily": cfg.font_family, "color": tok.text },
            "radar": { "indicator": ind_objs },
            "series": [{ "type": "radar", "data": radar_data }]
        })
    }

    fn resolve_x_col(
        widget: &WidgetRecord,
        _data: &WidgetDataResponse,
        col_map: &HashMap<String, usize>,
        cfg: &ChartSpec,
    ) -> usize {
        if let Some(explicit) = read_opt_str(&widget.encoding, "x")
            && let Some(idx) = lookup_col_idx(col_map, explicit)
        {
            return idx;
        }
        for cand in cfg
            .encoding_fallbacks
            .time
            .iter()
            .chain(cfg.encoding_fallbacks.category.iter())
        {
            if let Some(idx) = lookup_col_idx(col_map, cand) {
                return idx;
            }
        }
        0
    }

    fn resolve_series_col(
        widget: &WidgetRecord,
        data: &WidgetDataResponse,
        col_map: &HashMap<String, usize>,
        x_idx: usize,
        cfg: &ChartSpec,
    ) -> Option<usize> {
        if let Some(explicit) = read_opt_str(&widget.encoding, "series") {
            return lookup_col_idx(col_map, explicit).filter(|&i| i != x_idx);
        }
        for cand in &cfg.encoding_fallbacks.series {
            if let Some(idx) = lookup_col_idx(col_map, cand)
                && idx != x_idx
            {
                let has_non_null = data
                    .rows
                    .iter()
                    .any(|r| r.get(idx).is_some_and(|v| !v.is_null()));
                if has_non_null {
                    return Some(idx);
                }
            }
        }
        None
    }

    fn resolve_y_cols(
        widget: &WidgetRecord,
        data: &WidgetDataResponse,
        col_map: &HashMap<String, usize>,
        excluded: &[usize],
        cfg: &ChartSpec,
    ) -> Vec<usize> {
        let mut out = Vec::new();
        if let Some(y_val) = widget.encoding.get("y") {
            match y_val {
                Value::String(s) => {
                    if let Some(idx) = lookup_col_idx(col_map, s.trim()) {
                        out.push(idx);
                    }
                }
                Value::Array(arr) => {
                    for item in arr {
                        if let Some(s) = item.as_str()
                            && let Some(idx) = lookup_col_idx(col_map, s.trim())
                        {
                            out.push(idx);
                        }
                    }
                }
                _ => {}
            }
        }
        if !out.is_empty() {
            return out;
        }
        for cand in &cfg.encoding_fallbacks.value {
            if let Some(idx) = lookup_col_idx(col_map, cand)
                && !excluded.contains(&idx)
            {
                return vec![idx];
            }
        }
        if let Some(num_idx) = first_numeric_col(data, excluded) {
            return vec![num_idx];
        }
        vec![usize::from(data.columns.len() > 1)]
    }

    fn first_numeric_col(data: &WidgetDataResponse, excluded: &[usize]) -> Option<usize> {
        data.columns
            .iter()
            .enumerate()
            .find(|(idx, c)| {
                !excluded.contains(idx)
                    && matches!(
                        c.data_type,
                        DataType::Int64 | DataType::Float64 | DataType::Decimal
                    )
            })
            .map(|(idx, _)| idx)
    }

    fn cell_str(val: Option<&Value>) -> String {
        match val {
            None | Some(Value::Null) => String::new(),
            Some(Value::String(s)) => s.clone(),
            Some(Value::Number(n)) => n.to_string(),
            Some(Value::Bool(b)) => b.to_string(),
            Some(other) => other.to_string(),
        }
    }

    fn cell_num(val: Option<&Value>) -> Option<f64> {
        match val? {
            Value::Number(n) => n.as_f64(),
            Value::String(s) => s.trim().parse::<f64>().ok(),
            _ => None,
        }
    }
}

mod zip_ooxml {
    use std::collections::BTreeSet;

    use axpoint_domain::AppError;

    const EOCD_SIG: u32 = 0x0605_4b50;
    const CEN_SIG: u32 = 0x0201_4b50;
    const LOC_SIG: u32 = 0x0403_4b50;
    const MAX_ENTRIES: usize = 2_000;
    const MAX_SINGLE_XML_BYTES: usize = 16 * 1024 * 1024;
    const MAX_TOTAL_XML_BYTES: usize = 64 * 1024 * 1024;

    pub(super) fn scan_archive_placeholders(
        archive: &[u8],
        required_prefix: &str,
    ) -> Result<Vec<String>, AppError> {
        let (cd_offset, cd_count) = find_eocd(archive)?;
        if cd_count > MAX_ENTRIES {
            return Err(AppError::PayloadTooLarge("UPLOAD_TOO_LARGE"));
        }

        let mut pos = cd_offset;
        let mut total_uncompressed = 0_usize;
        let mut saw_ooxml_part = false;
        let mut keys = Vec::new();
        let mut seen = BTreeSet::new();

        for _ in 0..cd_count {
            if read_u32_le(archive, pos)? != CEN_SIG {
                return Err(AppError::bad(
                    "INVALID_ARGUMENT",
                    "corrupt ZIP central directory",
                ));
            }
            let method = read_u16_le(archive, pos.saturating_add(10))?;
            let comp_size = usize::try_from(read_u32_le(archive, pos.saturating_add(20))?)
                .map_err(|_| AppError::PayloadTooLarge("UPLOAD_TOO_LARGE"))?;
            let uncomp_size = usize::try_from(read_u32_le(archive, pos.saturating_add(24))?)
                .map_err(|_| AppError::PayloadTooLarge("UPLOAD_TOO_LARGE"))?;
            let name_len = usize::from(read_u16_le(archive, pos.saturating_add(28))?);
            let extra_len = usize::from(read_u16_le(archive, pos.saturating_add(30))?);
            let comment_len = usize::from(read_u16_le(archive, pos.saturating_add(32))?);
            let local_off = usize::try_from(read_u32_le(archive, pos.saturating_add(42))?)
                .map_err(|_| AppError::bad("INVALID_ARGUMENT", "invalid ZIP local offset"))?;

            let name_start = pos.saturating_add(46);
            let name_end = name_start.saturating_add(name_len);
            let name_bytes = archive
                .get(name_start..name_end)
                .ok_or_else(|| AppError::bad("INVALID_ARGUMENT", "truncated ZIP entry name"))?;
            let entry_name = std::str::from_utf8(name_bytes).unwrap_or_default();

            pos = name_end
                .saturating_add(extra_len)
                .saturating_add(comment_len);

            let is_xml = std::path::Path::new(entry_name)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("xml"));
            if !entry_name.starts_with(required_prefix) || !is_xml {
                continue;
            }
            saw_ooxml_part = true;
            if uncomp_size > MAX_SINGLE_XML_BYTES {
                return Err(AppError::PayloadTooLarge("UPLOAD_TOO_LARGE"));
            }
            total_uncompressed = total_uncompressed.saturating_add(uncomp_size);
            if total_uncompressed > MAX_TOTAL_XML_BYTES {
                return Err(AppError::PayloadTooLarge("UPLOAD_TOO_LARGE"));
            }

            let xml_bytes =
                extract_local_entry(archive, local_off, method, comp_size, uncomp_size)?;
            let xml_text = std::str::from_utf8(&xml_bytes)
                .map_err(|_| AppError::bad("INVALID_ARGUMENT", "invalid UTF-8 in OOXML part"))?;
            let coalesced = coalesce_xml_runs(xml_text);
            collect_placeholders(&coalesced, &mut seen, &mut keys);
        }

        if !saw_ooxml_part {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                format!("template archive missing required '{required_prefix}*.xml' entries"),
            ));
        }
        Ok(keys)
    }

    fn find_eocd(archive: &[u8]) -> Result<(usize, usize), AppError> {
        if archive.len() < 22 {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "ZIP archive shorter than EOCD",
            ));
        }
        let max_back = archive.len().min(65_557);
        let search_start = archive.len().saturating_sub(max_back);
        let mut cursor = archive.len().saturating_sub(22);
        loop {
            if read_u32_le(archive, cursor)? == EOCD_SIG {
                let count = usize::from(read_u16_le(archive, cursor.saturating_add(10))?);
                let cd_off = usize::try_from(read_u32_le(archive, cursor.saturating_add(16))?)
                    .map_err(|_| AppError::bad("INVALID_ARGUMENT", "invalid EOCD offset"))?;
                return Ok((cd_off, count));
            }
            if cursor <= search_start {
                break;
            }
            cursor = cursor.saturating_sub(1);
        }
        Err(AppError::bad(
            "INVALID_ARGUMENT",
            "missing ZIP End of Central Directory",
        ))
    }

    fn extract_local_entry(
        archive: &[u8],
        local_off: usize,
        method: u16,
        comp_size: usize,
        uncomp_size: usize,
    ) -> Result<Vec<u8>, AppError> {
        if read_u32_le(archive, local_off)? != LOC_SIG {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "invalid ZIP local file header",
            ));
        }
        let loc_name_len = usize::from(read_u16_le(archive, local_off.saturating_add(26))?);
        let loc_extra_len = usize::from(read_u16_le(archive, local_off.saturating_add(28))?);
        let data_start = local_off
            .saturating_add(30)
            .saturating_add(loc_name_len)
            .saturating_add(loc_extra_len);
        let data_end = data_start.saturating_add(comp_size);
        let raw_slice = archive
            .get(data_start..data_end)
            .ok_or_else(|| AppError::bad("INVALID_ARGUMENT", "truncated ZIP entry data"))?;

        match method {
            0 => Ok(raw_slice.to_vec()),
            8 => inflate::inflate_deflate(raw_slice, uncomp_size.clamp(64, MAX_SINGLE_XML_BYTES)),
            _ => Err(AppError::bad(
                "INVALID_ARGUMENT",
                format!("unsupported ZIP compression method {method}"),
            )),
        }
    }

    fn coalesce_xml_runs(xml: &str) -> String {
        let mut out = String::with_capacity(xml.len() / 2);
        let mut in_tag = false;
        let mut tag_buf = String::with_capacity(32);

        for ch in xml.chars() {
            if in_tag {
                if ch == '>' {
                    in_tag = false;
                    let t = tag_buf.trim();
                    if matches!(t, "/w:p" | "/a:p" | "/w:tc" | "/a:tc" | "/w:tr") {
                        out.push('\n');
                    }
                    tag_buf.clear();
                } else if tag_buf.len() < 16 {
                    tag_buf.push(ch);
                }
            } else if ch == '<' {
                in_tag = true;
                tag_buf.clear();
            } else {
                out.push(ch);
            }
        }
        out
    }

    fn collect_placeholders(text: &str, seen: &mut BTreeSet<String>, keys: &mut Vec<String>) {
        let mut rest = text;
        while let Some(open_idx) = rest.find("{{") {
            let after_open = rest.get(open_idx.saturating_add(2)..).unwrap_or_default();
            let Some(close_idx) = after_open.find("}}") else {
                break;
            };
            let candidate = after_open.get(..close_idx).unwrap_or_default().trim();
            if is_valid_placeholder_key(candidate) && seen.insert(String::from(candidate)) {
                keys.push(String::from(candidate));
            }
            rest = after_open
                .get(close_idx.saturating_add(2)..)
                .unwrap_or_default();
        }
    }

    fn is_valid_placeholder_key(s: &str) -> bool {
        !s.is_empty()
            && s.len() <= 128
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
    }

    fn read_u16_le(buf: &[u8], offset: usize) -> Result<u16, AppError> {
        let slice = buf
            .get(offset..offset.saturating_add(2))
            .ok_or_else(|| AppError::bad("INVALID_ARGUMENT", "unexpected EOF in ZIP header"))?;
        let arr = <[u8; 2]>::try_from(slice)
            .map_err(|_| AppError::bad("INVALID_ARGUMENT", "invalid u16 slice"))?;
        Ok(u16::from_le_bytes(arr))
    }

    fn read_u32_le(buf: &[u8], offset: usize) -> Result<u32, AppError> {
        let slice = buf
            .get(offset..offset.saturating_add(4))
            .ok_or_else(|| AppError::bad("INVALID_ARGUMENT", "unexpected EOF in ZIP header"))?;
        let arr = <[u8; 4]>::try_from(slice)
            .map_err(|_| AppError::bad("INVALID_ARGUMENT", "invalid u32 slice"))?;
        Ok(u32::from_le_bytes(arr))
    }

    mod inflate {
        use axpoint_domain::AppError;

        use super::MAX_SINGLE_XML_BYTES;

        const LEN_BASE: [u16; 29] = [
            3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99,
            115, 131, 163, 195, 227, 258,
        ];
        const LEN_EXTRA: [u8; 29] = [
            0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
        ];
        const DIST_BASE: [u16; 30] = [
            1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025,
            1537, 2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
        ];
        const DIST_EXTRA: [u8; 30] = [
            0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12,
            12, 13, 13,
        ];
        const CL_ORDER: [usize; 19] = [
            16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
        ];

        struct BitReader<'a> {
            src: &'a [u8],
            byte_pos: usize,
            bit_buf: u32,
            bit_cnt: u8,
        }

        impl<'a> BitReader<'a> {
            const fn new(src: &'a [u8]) -> Self {
                Self {
                    src,
                    byte_pos: 0,
                    bit_buf: 0,
                    bit_cnt: 0,
                }
            }

            fn read_bits(&mut self, n: u8) -> Result<u32, AppError> {
                while self.bit_cnt < n {
                    let b = *self.src.get(self.byte_pos).ok_or_else(|| {
                        AppError::bad("INVALID_ARGUMENT", "truncated DEFLATE bitstream")
                    })?;
                    self.byte_pos = self.byte_pos.saturating_add(1);
                    self.bit_buf |= u32::from(b) << self.bit_cnt;
                    self.bit_cnt = self.bit_cnt.saturating_add(8);
                }
                let mask = if n == 32 { u32::MAX } else { (1_u32 << n) - 1 };
                let out = self.bit_buf & mask;
                self.bit_buf >>= n;
                self.bit_cnt = self.bit_cnt.saturating_sub(n);
                Ok(out)
            }

            fn align_byte(&mut self) {
                let rem = self.bit_cnt % 8;
                if rem > 0 {
                    self.bit_buf >>= rem;
                    self.bit_cnt = self.bit_cnt.saturating_sub(rem);
                }
            }

            fn read_bytes_aligned(&mut self, len: usize) -> Result<&'a [u8], AppError> {
                self.align_byte();
                while self.bit_cnt >= 8 {
                    self.byte_pos = self.byte_pos.saturating_sub(1);
                    self.bit_cnt = self.bit_cnt.saturating_sub(8);
                }
                self.bit_buf = 0;
                let end = self.byte_pos.saturating_add(len);
                let slice = self.src.get(self.byte_pos..end).ok_or_else(|| {
                    AppError::bad("INVALID_ARGUMENT", "truncated DEFLATE stored block")
                })?;
                self.byte_pos = end;
                Ok(slice)
            }
        }

        struct HuffmanTable {
            counts: [u16; 16],
            symbols: Vec<u16>,
        }

        impl HuffmanTable {
            fn from_lengths(lengths: &[u8]) -> Result<Self, AppError> {
                let mut counts = [0_u16; 16];
                for &len in lengths {
                    let idx = usize::from(len);
                    let Some(slot) = counts.get_mut(idx) else {
                        return Err(AppError::bad(
                            "INVALID_ARGUMENT",
                            "invalid Huffman bit length",
                        ));
                    };
                    if idx > 0 {
                        *slot = slot.saturating_add(1);
                    }
                }

                let mut offsets = [0_u16; 16];
                let mut total = 0_u16;
                for bits in 1..16_usize {
                    if let Some(off_slot) = offsets.get_mut(bits) {
                        *off_slot = total;
                    }
                    total = total.saturating_add(counts.get(bits).copied().unwrap_or(0));
                }

                let mut symbols = vec![0_u16; usize::from(total)];
                for (sym, &len) in lengths.iter().enumerate() {
                    let b = usize::from(len);
                    if b == 0 {
                        continue;
                    }
                    let Some(off) = offsets.get_mut(b) else {
                        continue;
                    };
                    let sym_u16 = u16::try_from(sym)
                        .map_err(|_| AppError::bad("INVALID_ARGUMENT", "symbol index overflow"))?;
                    if let Some(target) = symbols.get_mut(usize::from(*off)) {
                        *target = sym_u16;
                    }
                    *off = off.saturating_add(1);
                }

                Ok(Self { counts, symbols })
            }

            fn decode(&self, br: &mut BitReader<'_>) -> Result<u16, AppError> {
                let mut code = 0_i32;
                let mut first = 0_i32;
                let mut index = 0_i32;
                for len in 1..16_usize {
                    let bit = i32::try_from(br.read_bits(1)?).unwrap_or(0);
                    code |= bit;
                    let count = i32::from(self.counts.get(len).copied().unwrap_or(0));
                    if code - count < first {
                        let sym_idx = usize::try_from(index + (code - first)).map_err(|_| {
                            AppError::bad("INVALID_ARGUMENT", "invalid Huffman decode index")
                        })?;
                        return self.symbols.get(sym_idx).copied().ok_or_else(|| {
                            AppError::bad("INVALID_ARGUMENT", "Huffman symbol out of range")
                        });
                    }
                    index += count;
                    first = (first + count) << 1;
                    code <<= 1;
                }
                Err(AppError::bad("INVALID_ARGUMENT", "unmatched Huffman code"))
            }
        }

        pub(super) fn inflate_deflate(src: &[u8], initial_cap: usize) -> Result<Vec<u8>, AppError> {
            let mut br = BitReader::new(src);
            let mut out = Vec::with_capacity(initial_cap);

            loop {
                let bfinal = br.read_bits(1)?;
                let btype = br.read_bits(2)?;
                match btype {
                    0 => inflate_stored(&mut br, &mut out)?,
                    1 => {
                        let (lit, dist) = fixed_tables()?;
                        inflate_huffman(&mut br, &lit, &dist, &mut out)?;
                    }
                    2 => {
                        let (lit, dist) = read_dynamic_tables(&mut br)?;
                        inflate_huffman(&mut br, &lit, &dist, &mut out)?;
                    }
                    _ => {
                        return Err(AppError::bad(
                            "INVALID_ARGUMENT",
                            "invalid DEFLATE block type 3",
                        ));
                    }
                }
                if bfinal == 1 {
                    break;
                }
            }
            Ok(out)
        }

        fn inflate_stored(br: &mut BitReader<'_>, out: &mut Vec<u8>) -> Result<(), AppError> {
            let hdr = br.read_bytes_aligned(4)?;
            let len_arr = <[u8; 2]>::try_from(hdr.get(0..2).unwrap_or_default())
                .map_err(|_| AppError::bad("INVALID_ARGUMENT", "invalid stored len"))?;
            let nlen_arr = <[u8; 2]>::try_from(hdr.get(2..4).unwrap_or_default())
                .map_err(|_| AppError::bad("INVALID_ARGUMENT", "invalid stored nlen"))?;
            let len = u16::from_le_bytes(len_arr);
            let nlen = u16::from_le_bytes(nlen_arr);
            if len != !nlen {
                return Err(AppError::bad(
                    "INVALID_ARGUMENT",
                    "DEFLATE stored block length check failed",
                ));
            }
            let chunk = br.read_bytes_aligned(usize::from(len))?;
            if out.len().saturating_add(chunk.len()) > MAX_SINGLE_XML_BYTES {
                return Err(AppError::PayloadTooLarge("UPLOAD_TOO_LARGE"));
            }
            out.extend_from_slice(chunk);
            Ok(())
        }

        fn fixed_tables() -> Result<(HuffmanTable, HuffmanTable), AppError> {
            let mut lit_lens = vec![0_u8; 288];
            for (i, slot) in lit_lens.iter_mut().enumerate() {
                *slot = if i <= 143 {
                    8
                } else if i <= 255 {
                    9
                } else if i <= 279 {
                    7
                } else {
                    8
                };
            }
            let dist_lens = vec![5_u8; 32];
            Ok((
                HuffmanTable::from_lengths(&lit_lens)?,
                HuffmanTable::from_lengths(&dist_lens)?,
            ))
        }

        fn read_dynamic_tables(
            br: &mut BitReader<'_>,
        ) -> Result<(HuffmanTable, HuffmanTable), AppError> {
            let hlit = usize::try_from(br.read_bits(5)?)
                .unwrap_or(0)
                .saturating_add(257);
            let hdist = usize::try_from(br.read_bits(5)?)
                .unwrap_or(0)
                .saturating_add(1);
            let hclen = usize::try_from(br.read_bits(4)?)
                .unwrap_or(0)
                .saturating_add(4);

            let mut cl_lens = [0_u8; 19];
            for i in 0..hclen {
                let target_idx = CL_ORDER.get(i).copied().unwrap_or(0);
                let val = u8::try_from(br.read_bits(3)?).unwrap_or(0);
                if let Some(slot) = cl_lens.get_mut(target_idx) {
                    *slot = val;
                }
            }
            let cl_table = HuffmanTable::from_lengths(&cl_lens)?;
            let total_codes = hlit.saturating_add(hdist);
            let combined = decode_code_lengths(br, &cl_table, total_codes)?;
            let lit_slice = combined.get(..hlit).ok_or_else(|| {
                AppError::bad("INVALID_ARGUMENT", "invalid dynamic literal table")
            })?;
            let dist_slice = combined.get(hlit..total_codes).ok_or_else(|| {
                AppError::bad("INVALID_ARGUMENT", "invalid dynamic distance table")
            })?;
            Ok((
                HuffmanTable::from_lengths(lit_slice)?,
                HuffmanTable::from_lengths(dist_slice)?,
            ))
        }

        fn decode_code_lengths(
            br: &mut BitReader<'_>,
            cl_table: &HuffmanTable,
            total_codes: usize,
        ) -> Result<Vec<u8>, AppError> {
            let mut lens = Vec::with_capacity(total_codes);
            while lens.len() < total_codes {
                let sym = cl_table.decode(br)?;
                match sym {
                    0..=15 => lens.push(u8::try_from(sym).unwrap_or(0)),
                    16 => {
                        let prev = *lens.last().ok_or_else(|| {
                            AppError::bad("INVALID_ARGUMENT", "repeat code 16 at start")
                        })?;
                        let rep = usize::try_from(br.read_bits(2)?)
                            .unwrap_or(0)
                            .saturating_add(3);
                        for _ in 0..rep {
                            lens.push(prev);
                        }
                    }
                    17 => {
                        let rep = usize::try_from(br.read_bits(3)?)
                            .unwrap_or(0)
                            .saturating_add(3);
                        lens.resize(lens.len().saturating_add(rep), 0);
                    }
                    18 => {
                        let rep = usize::try_from(br.read_bits(7)?)
                            .unwrap_or(0)
                            .saturating_add(11);
                        lens.resize(lens.len().saturating_add(rep), 0);
                    }
                    _ => return Err(AppError::bad("INVALID_ARGUMENT", "invalid code-length sym")),
                }
            }
            lens.truncate(total_codes);
            Ok(lens)
        }

        fn inflate_huffman(
            br: &mut BitReader<'_>,
            lit_table: &HuffmanTable,
            dist_table: &HuffmanTable,
            out: &mut Vec<u8>,
        ) -> Result<(), AppError> {
            loop {
                let sym = lit_table.decode(br)?;
                if sym < 256 {
                    if out.len() >= MAX_SINGLE_XML_BYTES {
                        return Err(AppError::PayloadTooLarge("UPLOAD_TOO_LARGE"));
                    }
                    out.push(u8::try_from(sym).unwrap_or(0));
                    continue;
                }
                if sym == 256 {
                    break;
                }
                let len_idx = usize::from(sym.saturating_sub(257));
                let base_len = usize::from(*LEN_BASE.get(len_idx).ok_or_else(|| {
                    AppError::bad("INVALID_ARGUMENT", "invalid DEFLATE length symbol")
                })?);
                let extra_bits = *LEN_EXTRA.get(len_idx).unwrap_or(&0);
                let extra_len = usize::try_from(br.read_bits(extra_bits)?).unwrap_or(0);
                let copy_len = base_len.saturating_add(extra_len);

                let dist_sym = usize::from(dist_table.decode(br)?);
                let base_dist = usize::from(*DIST_BASE.get(dist_sym).ok_or_else(|| {
                    AppError::bad("INVALID_ARGUMENT", "invalid DEFLATE distance symbol")
                })?);
                let dist_extra = *DIST_EXTRA.get(dist_sym).unwrap_or(&0);
                let extra_d = usize::try_from(br.read_bits(dist_extra)?).unwrap_or(0);
                let dist = base_dist.saturating_add(extra_d);

                if dist == 0 || dist > out.len() {
                    return Err(AppError::bad(
                        "INVALID_ARGUMENT",
                        "DEFLATE back-reference before start of output",
                    ));
                }
                if out.len().saturating_add(copy_len) > MAX_SINGLE_XML_BYTES {
                    return Err(AppError::PayloadTooLarge("UPLOAD_TOO_LARGE"));
                }
                for _ in 0..copy_len {
                    let src_idx = out.len().saturating_sub(dist);
                    let byte = *out.get(src_idx).unwrap_or(&0);
                    out.push(byte);
                }
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axpoint_domain::view::{
        RefreshMode, WidgetColumn, WidgetDataSource, WidgetDeps, WidgetPosition,
    };

    fn make_stored_zip(entry_name: &str, content: &[u8]) -> Vec<u8> {
        let name_b = entry_name.as_bytes();
        let name_len = u16::try_from(name_b.len()).unwrap_or(0);
        let size_u32 = u32::try_from(content.len()).unwrap_or(0);

        let mut zip = Vec::new();
        // Local file header
        zip.extend_from_slice(&0x0403_4b50_u32.to_le_bytes());
        zip.extend_from_slice(&20_u16.to_le_bytes());
        zip.extend_from_slice(&0_u16.to_le_bytes());
        zip.extend_from_slice(&0_u16.to_le_bytes()); // method 0 = stored
        zip.extend_from_slice(&0_u16.to_le_bytes());
        zip.extend_from_slice(&0_u16.to_le_bytes());
        zip.extend_from_slice(&0_u32.to_le_bytes());
        zip.extend_from_slice(&size_u32.to_le_bytes());
        zip.extend_from_slice(&size_u32.to_le_bytes());
        zip.extend_from_slice(&name_len.to_le_bytes());
        zip.extend_from_slice(&0_u16.to_le_bytes());
        zip.extend_from_slice(name_b);
        zip.extend_from_slice(content);

        let cd_offset = u32::try_from(zip.len()).unwrap_or(0);
        // Central directory header
        zip.extend_from_slice(&0x0201_4b50_u32.to_le_bytes());
        zip.extend_from_slice(&20_u16.to_le_bytes());
        zip.extend_from_slice(&20_u16.to_le_bytes());
        zip.extend_from_slice(&0_u16.to_le_bytes());
        zip.extend_from_slice(&0_u16.to_le_bytes());
        zip.extend_from_slice(&0_u16.to_le_bytes());
        zip.extend_from_slice(&0_u16.to_le_bytes());
        zip.extend_from_slice(&0_u32.to_le_bytes());
        zip.extend_from_slice(&size_u32.to_le_bytes());
        zip.extend_from_slice(&size_u32.to_le_bytes());
        zip.extend_from_slice(&name_len.to_le_bytes());
        zip.extend_from_slice(&0_u16.to_le_bytes());
        zip.extend_from_slice(&0_u16.to_le_bytes());
        zip.extend_from_slice(&0_u16.to_le_bytes());
        zip.extend_from_slice(&0_u16.to_le_bytes());
        zip.extend_from_slice(&0_u32.to_le_bytes());
        zip.extend_from_slice(&0_u32.to_le_bytes()); // local offset 0
        zip.extend_from_slice(name_b);

        let cd_size = u32::try_from(zip.len())
            .unwrap_or(0)
            .saturating_sub(cd_offset);
        // EOCD
        zip.extend_from_slice(&0x0605_4b50_u32.to_le_bytes());
        zip.extend_from_slice(&0_u16.to_le_bytes());
        zip.extend_from_slice(&0_u16.to_le_bytes());
        zip.extend_from_slice(&1_u16.to_le_bytes());
        zip.extend_from_slice(&1_u16.to_le_bytes());
        zip.extend_from_slice(&cd_size.to_le_bytes());
        zip.extend_from_slice(&cd_offset.to_le_bytes());
        zip.extend_from_slice(&0_u16.to_le_bytes());
        zip
    }

    #[test]
    fn extract_ooxml_placeholders_coalesces_split_xml_runs() -> Result<(), AppError> {
        let split_docx_xml = br"<w:document><w:body><w:p><w:r><w:t>{{</w:t></w:r><w:r><w:t>report_title</w:t></w:r><w:r><w:t>}}</w:t></w:r></w:p><w:p><w:r><w:t>Body {{ sections }} and {{author.name}}</w:t></w:r></w:p></w:body></w:document>";
        let zip_bytes = make_stored_zip("word/document.xml", split_docx_xml);
        let placeholders = extract_ooxml_placeholders(ExportFormat::Docx, &zip_bytes)?;
        assert_eq!(
            placeholders,
            vec![
                String::from("report_title"),
                String::from("sections"),
                String::from("author.name"),
            ]
        );
        Ok(())
    }

    #[test]
    fn scatter_echarts_option_uses_numeric_symbol_sizes_without_functions() {
        let widget = WidgetRecord {
            id: Uuid::nil(),
            tenant_id: String::from("default"),
            dashboard_id: Uuid::nil(),
            widget_kind: WidgetKind::Scatter,
            title: String::from("库存散点"),
            position: WidgetPosition {
                x: 0,
                y: 0,
                w: 8,
                h: 6,
            },
            data_source: WidgetDataSource::Static {
                columns: Vec::new(),
                rows: Vec::new(),
            },
            encoding: serde_json::json!({ "x": "x", "y": "y", "size": "sz" }),
            options: serde_json::json!({}),
            drill: Vec::new(),
            refresh_mode: RefreshMode::Manual,
            refresh_seconds: 60,
            deps: WidgetDeps::default(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let resp = WidgetDataResponse {
            columns: vec![
                WidgetColumn {
                    name: String::from("x"),
                    data_type: DataType::Float64,
                },
                WidgetColumn {
                    name: String::from("y"),
                    data_type: DataType::Float64,
                },
                WidgetColumn {
                    name: String::from("sz"),
                    data_type: DataType::Float64,
                },
            ],
            rows: vec![vec![
                serde_json::json!(10.0),
                serde_json::json!(20.0),
                serde_json::json!(500.0),
            ]],
            row_count: 1,
            truncated: false,
            generated_at: Utc::now(),
            data_version: String::from("v1"),
            drill_level: 0,
        };

        let sec = widget_to_export_section(&widget, &resp, false);
        assert!(matches!(sec, ExportSection::Chart { .. }));
        if let ExportSection::Chart {
            echarts_option,
            data_table,
            ..
        } = sec
        {
            assert!(data_table.is_some());
            let pt_size = echarts_option
                .pointer("/series/0/data/0/symbolSize")
                .and_then(serde_json::Value::as_f64);
            assert_eq!(pt_size, Some(36.0));
        }
    }

    #[test]
    fn rfc5987_content_disposition_encodes_utf8_filename() {
        let header = build_content_disposition("供应概览.xlsx");
        assert!(header.starts_with("attachment; filename=\"____.xlsx\"; filename*=UTF-8''"));
        assert!(header.contains("%E4%BE%9B%E5%BA%94%E6%A6%82%E8%A7%88.xlsx"));
    }
}
