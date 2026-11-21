//! Industry template pack discovery, `SHA-256` checksum verification, conflict planning,
//! breakpoint-resumable installation, and content-hash diffing uninstallation (`§14.5`, `D2`).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use axpoint_domain::AppError;
use axpoint_domain::agent::{AgentRecord, CreateAgentRequest};
use axpoint_domain::analytics::{CreateMetricRequest, MetricDef};
use axpoint_domain::axhub::{
    CommitStagingItem, CommitStagingRequest, CreateDatasetRequest, Dataset, DatasetListQuery,
    PaginatedList, SchemaPolicy,
};
use axpoint_domain::enums::{AuditAction, Environment, ExportFormat, SourceKind};
use axpoint_domain::export::{CreateExportTemplateParams, ExportTemplateRecord};
use axpoint_domain::flow::{DeployFlowRequest, FlowListQuery, PublishFlowVersionRequest};
use axpoint_domain::ops::{
    CreateWorkItemRequest, NewTemplateInstallInput, PatchWorkItemRequest, TemplateConflictStrategy,
    TemplateCreatedObject, TemplateDatasetEntry, TemplateInstallPlan, TemplateInstallRecord,
    TemplateInstallStatus, TemplateManifest, TemplatePackDetail, TemplatePlannedObject,
    TemplateUninstallResponse, WorkItem, WorkItemKind, WorkItemPriority, WorkItemSourceKind,
    WorkItemStatus,
};
use axpoint_domain::platform::{AuditRecord, BoxFuture};
use axpoint_domain::ports::{
    AgentRepository, AnalyticsRepository, DatasetRepository, FlowRepository, OpsRepository,
    RulesRepository, SidecarPort, StudioExecutionPort, StudioRepository, ViewRepository,
};
use axpoint_domain::rollup::{GraphAggregateJobPayload, MapMaterializeJobPayload};
use axpoint_domain::rules::{CreateRuleRequest, RuleDef, RuleListQuery, SimplePageQuery};
use axpoint_domain::studio::{
    CreateDraftRequest, DraftSource, MappingMode, MappingSpec, NodeTypeSpec, PublishDraftRequest,
    SaveSchemaDirectRequest, SchemaDoc,
};
use axpoint_domain::topology::{AxFlowDef, CreateAxFlowRequest};
use axpoint_domain::view::{
    CanvasConfig, CreateDashboardRequest, CreateWidgetRecordParams, CreateWidgetRequest,
    DashboardDetail, DashboardTheme, FilterDef,
};
use axpoint_security::AuthPrincipal;
use chrono::Utc;
use serde::Deserialize;
use sha2::{Digest as _, Sha256};
use uuid::Uuid;

use super::OpsServices;
use crate::view::deps::compute_widget_deps;
use crate::view::export::extract_ooxml_placeholders;

/// Sentinel hash stored on `SKIP`-reused tenant objects so `uninstall` never deletes them.
pub const SKIPPED_HASH_SENTINEL: &str = "SKIPPED_EXISTING";

/// Audit context passed when applying a template pack (`AuditAction::TemplateInstall`).
#[derive(Debug, Clone, Copy)]
pub struct TemplateAuditContext<'a> {
    /// Originating client IP address.
    pub client_ip: &'a str,
    /// Active `W3C` trace identifier.
    pub trace_id: &'a str,
}

#[derive(Debug, Deserialize)]
struct PackDashboardDef {
    #[serde(default)]
    key: Option<String>,
    name: String,
    slug: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    filters: Vec<FilterDef>,
    #[serde(default)]
    theme: DashboardTheme,
    #[serde(default)]
    canvas: CanvasConfig,
    #[serde(default)]
    widgets: Vec<CreateWidgetRequest>,
}

struct PlanCollector<'a> {
    objects: &'a mut Vec<TemplatePlannedObject>,
    conflicts: &'a mut Vec<String>,
    strategy: TemplateConflictStrategy,
    pack_id: &'a str,
}

impl PlanCollector<'_> {
    fn push(&mut self, category: &str, key: &str, source_name: &str, existing: &BTreeSet<String>) {
        let is_conflict = existing.contains(source_name);
        let (target_name, action) = if !is_conflict {
            (String::from(source_name), String::from("CREATE"))
        } else {
            self.conflicts.push(format!("{category}:{source_name}"));
            match self.strategy {
                TemplateConflictStrategy::Skip => (String::from(source_name), String::from("SKIP")),
                TemplateConflictStrategy::Rename => {
                    let mut cand = compute_renamed_target(category, source_name, self.pack_id);
                    let mut idx = 2_u32;
                    while existing.contains(&cand) {
                        cand = compute_renamed_target(
                            category,
                            source_name,
                            &format!("{}_{idx}", self.pack_id),
                        );
                        idx = idx.saturating_add(1);
                    }
                    (cand, String::from("RENAME"))
                }
            }
        };
        self.objects.push(TemplatePlannedObject {
            category: String::from(category),
            key: String::from(key),
            source_name: String::from(source_name),
            target_name,
            conflict: is_conflict,
            action,
        });
    }
}

struct ApplySession {
    tenant_id: String,
    actor: Option<Uuid>,
    install_id: Uuid,
    pack_dir: PathBuf,
    manifest: TemplateManifest,
    plan: TemplateInstallPlan,
    node_type_renames: BTreeMap<String, String>,
    object_map: BTreeMap<String, TemplateCreatedObject>,
}

impl ApplySession {
    fn planned_for(&self, category: &str, key: &str) -> Option<&TemplatePlannedObject> {
        self.plan
            .objects
            .iter()
            .find(|o| o.category == category && o.key == key)
    }
}

/// Resolves the directory containing built-in industry template packs (`§14.5.1`).
#[must_use]
pub fn resolve_templates_dir() -> Option<PathBuf> {
    if let Some(env_dir) = std::env::var("AXPOINT_TEMPLATES_DIR")
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
    {
        let p = PathBuf::from(env_dir);
        if p.join("packs").is_dir() {
            return Some(p.join("packs"));
        }
        if p.is_dir() {
            return Some(p);
        }
    }

    let candidates = [
        PathBuf::from("./templates/packs"),
        PathBuf::from("../templates/packs"),
        PathBuf::from("/app/templates/packs"),
        PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../templates/packs"
        )),
    ];
    candidates.into_iter().find(|c| c.is_dir())
}

/// Counts valid template packs discovered in [`resolve_templates_dir`].
#[must_use]
pub fn discover_packs_count() -> usize {
    let Some(dir) = resolve_templates_dir() else {
        return 0;
    };
    list_templates_in_dir(&dir).map_or(0, |v| v.len())
}

/// Lists all valid [`TemplateManifest`]s inside `catalog_dir` sorted by `id`.
///
/// # Errors
/// Returns [`AppError::Config`] or [`AppError::Unprocessable`] if a discovered `manifest.json` is malformed.
pub fn list_templates_in_dir(catalog_dir: &Path) -> Result<Vec<TemplateManifest>, AppError> {
    if !catalog_dir.is_dir() {
        return Ok(Vec::new());
    }
    let entries = std::fs::read_dir(catalog_dir)
        .map_err(|err| AppError::Config(format!("read templates dir failed: {err}")))?;
    let mut manifests = Vec::new();
    for entry_res in entries {
        let entry =
            entry_res.map_err(|err| AppError::Config(format!("dir entry failed: {err}")))?;
        let path = entry.path();
        if path.is_dir() && path.join("manifest.json").is_file() {
            manifests.push(read_manifest(&path)?);
        }
    }
    manifests.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(manifests)
}

/// Loads [`TemplatePackDetail`] (`manifest.json` + `README.md`) for `pack_id` inside `catalog_dir`.
///
/// # Errors
/// Returns `TEMPLATE_NOT_FOUND` if `pack_id` is invalid or missing, or parse errors on invalid `JSON`.
pub fn get_template_detail_in_dir(
    catalog_dir: &Path,
    pack_id: &str,
) -> Result<TemplatePackDetail, AppError> {
    let pack_dir = resolve_pack_dir(catalog_dir, pack_id)?;
    let manifest = read_manifest(&pack_dir)?;
    let readme_path = pack_dir.join("README.md");
    let readme = if readme_path.is_file() {
        std::fs::read_to_string(&readme_path)
            .map_err(|err| AppError::Config(format!("read template README.md failed: {err}")))?
    } else {
        String::new()
    };
    Ok(TemplatePackDetail { manifest, readme })
}

/// Verifies every relative file entry in `manifest.sha256` against its actual bytes on disk (`§14.5.3`).
///
/// # Errors
/// Returns [`AppError::Unprocessable`] (`CHECKSUM_MISMATCH`) if any listed file is missing,
/// traverses outside `pack_dir`, or has a mismatching `SHA-256` digest.
pub fn verify_manifest_checksums(
    pack_dir: &Path,
    manifest: &TemplateManifest,
) -> Result<(), AppError> {
    for (rel_path, expected_hex) in &manifest.sha256 {
        let file_path = safe_pack_file(pack_dir, rel_path)?;
        let bytes = std::fs::read(&file_path).map_err(|_| {
            AppError::unprocessable(
                "CHECKSUM_MISMATCH",
                format!("template file '{rel_path}' listed in sha256 is missing"),
            )
        })?;
        let actual_hex = sha256_bytes_hex(&bytes);
        if !actual_hex.eq_ignore_ascii_case(expected_hex.trim()) {
            return Err(AppError::unprocessable(
                "CHECKSUM_MISMATCH",
                format!(
                    "SHA-256 mismatch for '{rel_path}': expected {}, got {actual_hex}",
                    expected_hex.trim()
                ),
            ));
        }
    }
    Ok(())
}

/// Computes the renamed target identifier when `on_conflict == RENAME` (`§14.5.3`).
#[must_use]
pub fn compute_renamed_target(category: &str, source_name: &str, pack_id: &str) -> String {
    match category {
        "node_type" => {
            let suffix = to_pascal_case(pack_id);
            truncate_chars(&format!("{source_name}{suffix}"), 62)
        }
        "flow" | "dashboard" => {
            let kebab_pack = pack_id
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() {
                        c.to_ascii_lowercase()
                    } else {
                        '-'
                    }
                })
                .collect::<String>();
            let clean = kebab_pack.trim_matches('-');
            truncate_chars(&format!("{source_name}-{clean}"), 62)
        }
        "dataset" | "mapping" | "metric" | "agent" | "rule" | "export_template" => {
            let snake_pack = pack_id
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() {
                        c.to_ascii_lowercase()
                    } else {
                        '_'
                    }
                })
                .collect::<String>();
            let clean = snake_pack.trim_matches('_');
            truncate_chars(&format!("{source_name}_{clean}"), 62)
        }
        _ => format!("{source_name}_{pack_id}"),
    }
}

/// Recursively resolves `{"$ref": "<category>:<key>"}` objects inside `value` using `object_map` (`§14.5.1`).
///
/// # Errors
/// Returns [`AppError::Unprocessable`] (`UNRESOLVED_TEMPLATE_REF`) if a referenced key is absent from `object_map`.
pub fn resolve_json_refs(
    value: &mut serde_json::Value,
    parent_field: Option<&str>,
    object_map: &BTreeMap<String, TemplateCreatedObject>,
) -> Result<(), AppError> {
    match value {
        serde_json::Value::Object(map) => {
            if map.len() == 1
                && let Some(ref_str) = map.get("$ref").and_then(serde_json::Value::as_str)
            {
                let target = object_map.get(ref_str).ok_or_else(|| {
                    AppError::unprocessable(
                        "UNRESOLVED_TEMPLATE_REF",
                        format!("unresolved template $ref '{ref_str}'"),
                    )
                })?;
                let replacement = select_ref_value(ref_str, parent_field, target);
                *value = serde_json::Value::String(replacement);
                return Ok(());
            }
            let keys: Vec<String> = map.keys().cloned().collect();
            for key in keys {
                if let Some(child) = map.get_mut(&key) {
                    resolve_json_refs(child, Some(key.as_str()), object_map)?;
                }
            }
            Ok(())
        }
        serde_json::Value::Array(items) => {
            for item in items.iter_mut() {
                resolve_json_refs(item, parent_field, object_map)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn select_ref_value(
    ref_str: &str,
    parent_field: Option<&str>,
    target: &TemplateCreatedObject,
) -> String {
    let category = ref_str.split(':').next().unwrap_or_default();
    let wants_name = matches!(
        parent_field,
        Some(
            "dataset"
                | "dataset_names"
                | "metric"
                | "metric_name"
                | "metric_names"
                | "type_name"
                | "type_names"
                | "node_type"
                | "target_type"
                | "source_type_name"
                | "slug"
        )
    );
    if wants_name || category == "node_type" {
        target.target_name.clone()
    } else if category == "metric" && parent_field != Some("target_id") {
        target.target_name.clone()
    } else {
        target.id.clone()
    }
}

/// Computes a deterministic `SHA-256` hex digest of any `JSON`-serializable value (`BTreeMap`-canonicalized).
#[must_use]
pub fn canonical_json_hash<T: serde::Serialize>(val: &T) -> String {
    let json_val = serde_json::to_value(val).unwrap_or(serde_json::Value::Null);
    let canonical = canonicalize_value(json_val);
    let encoded = serde_json::to_vec(&canonical).unwrap_or_default();
    sha256_bytes_hex(&encoded)
}

fn canonicalize_value(val: serde_json::Value) -> serde_json::Value {
    match val {
        serde_json::Value::Object(map) => {
            let mut sorted: BTreeMap<String, serde_json::Value> = BTreeMap::new();
            for (k, v) in map {
                sorted.insert(k, canonicalize_value(v));
            }
            let mut out = serde_json::Map::with_capacity(sorted.len());
            for (k, v) in sorted {
                out.insert(k, v);
            }
            serde_json::Value::Object(out)
        }
        serde_json::Value::Array(arr) => {
            serde_json::Value::Array(arr.into_iter().map(canonicalize_value).collect())
        }
        other => other,
    }
}

/// Existing tenant object names collected ahead of template conflict detection (`§14.5.3`).
#[derive(Debug, Default)]
pub struct ExistingTenantNames {
    /// Existing `main` `NodeTypeSpec.name`s.
    pub node_types: BTreeSet<String>,
    /// Existing dataset `name`s.
    pub datasets: BTreeSet<String>,
    /// Existing `main` mapping `name`s.
    pub mappings: BTreeSet<String>,
    /// Existing semantic metric `name`s.
    pub metrics: BTreeSet<String>,
    /// Existing flow `slug`s and `name`s.
    pub flows: BTreeSet<String>,
    /// Existing rule `name`s.
    pub rules: BTreeSet<String>,
    /// Existing dashboard `slug`s.
    pub dashboards: BTreeSet<String>,
    /// Existing agent `name`s.
    pub agents: BTreeSet<String>,
    /// Existing export template `name`s.
    pub export_templates: BTreeSet<String>,
}

/// Builds a [`TemplateInstallPlan`] for `manifest` inside `pack_dir` against `existing` tenant names.
///
/// # Errors
/// Returns `CHECKSUM_MISMATCH` or `PARSE_FAILED` if pack files fail validation.
pub fn build_install_plan_from_dir(
    pack_dir: &Path,
    manifest: &TemplateManifest,
    strategy: TemplateConflictStrategy,
    existing: &ExistingTenantNames,
) -> Result<TemplateInstallPlan, AppError> {
    verify_manifest_checksums(pack_dir, manifest)?;
    let mut objects: Vec<TemplatePlannedObject> = Vec::new();
    let mut conflicts: Vec<String> = Vec::new();
    let mut collector = PlanCollector {
        objects: &mut objects,
        conflicts: &mut conflicts,
        strategy,
        pack_id: &manifest.id,
    };

    plan_pack_domain_items(pack_dir, manifest, existing, &mut collector)?;

    let wi_title = format!("绑定「{}」告警通知渠道", manifest.name);
    objects.push(TemplatePlannedObject {
        category: String::from("work_item"),
        key: String::from("post_install"),
        source_name: wi_title.clone(),
        target_name: wi_title,
        conflict: false,
        action: String::from("CREATE"),
    });

    Ok(TemplateInstallPlan {
        pack_id: manifest.id.clone(),
        pack_version: manifest.version.clone(),
        on_conflict: strategy,
        objects,
        conflicts,
    })
}

fn plan_pack_domain_items(
    pack_dir: &Path,
    manifest: &TemplateManifest,
    existing: &ExistingTenantNames,
    collector: &mut PlanCollector<'_>,
) -> Result<(), AppError> {
    let schema_doc: SchemaDoc =
        serde_json::from_value(read_pack_json(pack_dir, &manifest.contents.schema)?).map_err(
            |err| AppError::unprocessable("PARSE_FAILED", format!("invalid schema.json: {err}")),
        )?;
    for nt in &schema_doc.node_types {
        collector.push("node_type", &nt.name, &nt.name, &existing.node_types);
    }
    for ds in &manifest.contents.datasets {
        let src = ds
            .name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(&ds.key);
        collector.push("dataset", &ds.key, src, &existing.datasets);
    }

    let c = &manifest.contents;
    plan_optional_json_items(
        pack_dir,
        c.mappings.as_deref(),
        ("mappings", "mapping"),
        &existing.mappings,
        collector,
    )?;
    plan_optional_json_items(
        pack_dir,
        c.metrics.as_deref(),
        ("metrics", "metric"),
        &existing.metrics,
        collector,
    )?;
    plan_file_list_items(
        pack_dir,
        &c.flows,
        "flow",
        &["slug", "name"],
        &existing.flows,
        collector,
    )?;
    plan_optional_json_items(
        pack_dir,
        c.rules.as_deref(),
        ("rules", "rule"),
        &existing.rules,
        collector,
    )?;
    plan_file_list_items(
        pack_dir,
        &c.dashboards,
        "dashboard",
        &["slug", "name"],
        &existing.dashboards,
        collector,
    )?;
    plan_file_list_items(
        pack_dir,
        &c.agents,
        "agent",
        &["name"],
        &existing.agents,
        collector,
    )?;
    plan_export_template_items(
        pack_dir,
        &c.export_templates,
        &existing.export_templates,
        collector,
    )
}

fn plan_optional_json_items(
    pack_dir: &Path,
    rel_opt: Option<&str>,
    envelope: (&str, &str),
    existing: &BTreeSet<String>,
    collector: &mut PlanCollector<'_>,
) -> Result<(), AppError> {
    let Some(rel_path) = rel_opt.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(());
    };
    let (wrapper_key, category) = envelope;
    let items = extract_json_array(read_pack_json(pack_dir, rel_path)?, wrapper_key)?;
    for item in &items {
        let name = item
            .get("name")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                AppError::unprocessable(
                    "PARSE_FAILED",
                    format!("missing 'name' in '{rel_path}' item"),
                )
            })?;
        let key = item
            .get("key")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(name);
        collector.push(category, key, name, existing);
    }
    Ok(())
}

fn plan_file_list_items(
    pack_dir: &Path,
    rel_paths: &[String],
    category: &str,
    name_fields: &[&str],
    existing: &BTreeSet<String>,
    collector: &mut PlanCollector<'_>,
) -> Result<(), AppError> {
    for rel_path in rel_paths {
        let val = read_pack_json(pack_dir, rel_path)?;
        let stem = file_stem_key(rel_path);
        let key = val
            .get("key")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map_or_else(|| stem.clone(), String::from);
        let src_name = name_fields
            .iter()
            .find_map(|f| {
                val.get(*f)
                    .and_then(serde_json::Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
            })
            .unwrap_or(key.as_str());
        collector.push(category, &key, src_name, existing);
    }
    Ok(())
}

fn plan_export_template_items(
    pack_dir: &Path,
    rel_paths: &[String],
    existing: &BTreeSet<String>,
    collector: &mut PlanCollector<'_>,
) -> Result<(), AppError> {
    for rel_path in rel_paths {
        let full = safe_pack_file(pack_dir, rel_path)?;
        if !full.is_file() {
            return Err(AppError::unprocessable(
                "PARSE_FAILED",
                format!("export template '{rel_path}' is missing"),
            ));
        }
        let stem = file_stem_key(rel_path);
        collector.push("export_template", &stem, &stem, existing);
    }
    Ok(())
}

impl<S, P> OpsServices<S, P> {
    /// Lists all discovered industry template packs (`GET /api/axops/templates`).
    ///
    /// # Errors
    /// Returns [`AppError`] if a pack `manifest.json` on disk cannot be read or parsed.
    pub fn list_templates(&self) -> Result<PaginatedList<TemplateManifest>, AppError> {
        let Some(dir) = resolve_templates_dir() else {
            return Ok(PaginatedList {
                items: Vec::new(),
                next_cursor: None,
            });
        };
        let items = list_templates_in_dir(&dir)?;
        Ok(PaginatedList {
            items,
            next_cursor: None,
        })
    }

    /// Returns [`TemplatePackDetail`] (`manifest.json` + `README.md`) for `pack_id`.
    ///
    /// # Errors
    /// Returns [`AppError::NotFound`] (`TEMPLATE_NOT_FOUND`) when `pack_id` does not exist.
    pub fn get_template(&self, pack_id: &str) -> Result<TemplatePackDetail, AppError> {
        let dir = resolve_templates_dir().ok_or(AppError::NotFound("TEMPLATE_NOT_FOUND"))?;
        get_template_detail_in_dir(&dir, pack_id)
    }
}

impl<S, P> OpsServices<S, P>
where
    S: OpsRepository
        + StudioRepository
        + StudioExecutionPort
        + DatasetRepository
        + AnalyticsRepository
        + FlowRepository
        + RulesRepository
        + ViewRepository
        + AgentRepository
        + Sync,
    P: SidecarPort + Sync,
{
    /// Lists template installation records in `tenant_id` (`GET /api/axops/templates/installs`).
    ///
    /// # Errors
    /// Propagates errors from [`OpsRepository::list_template_installs`].
    pub async fn list_template_installs(
        &self,
        tenant_id: &str,
    ) -> Result<PaginatedList<TemplateInstallRecord>, AppError> {
        let items = self.store.list_template_installs(tenant_id).await?;
        Ok(PaginatedList {
            items,
            next_cursor: None,
        })
    }

    /// Generates and persists a `PLANNED` [`TemplateInstallRecord`] (`POST /api/axops/templates/{pack_id}/plan`).
    pub fn plan_template_install<'a>(
        &'a self,
        tenant_id: &'a str,
        actor: Option<Uuid>,
        pack_id: &'a str,
        on_conflict: Option<TemplateConflictStrategy>,
    ) -> BoxFuture<'a, TemplateInstallRecord> {
        Box::pin(async move {
            let catalog_dir =
                resolve_templates_dir().ok_or(AppError::NotFound("TEMPLATE_NOT_FOUND"))?;
            let pack_dir = resolve_pack_dir(&catalog_dir, pack_id)?;
            let manifest = read_manifest(&pack_dir)?;
            let existing = self.collect_existing_tenant_names(tenant_id).await?;
            let strategy = on_conflict.unwrap_or(TemplateConflictStrategy::Rename);
            let plan = build_install_plan_from_dir(&pack_dir, &manifest, strategy, &existing)?;

            self.store
                .create_template_install(
                    tenant_id,
                    NewTemplateInstallInput {
                        pack_id: manifest.id,
                        pack_version: manifest.version,
                        plan,
                        installed_by: actor,
                    },
                )
                .await
        })
    }

    /// Executes breakpoint-resumable template installation (`POST /api/axops/templates/installs/{id}/apply`, `D2`).
    pub fn apply_template_install<'a>(
        &'a self,
        principal: &'a AuthPrincipal,
        ctx: TemplateAuditContext<'a>,
        install_id: Uuid,
    ) -> BoxFuture<'a, TemplateInstallRecord> {
        Box::pin(async move {
            let tenant_id = principal.tenant_id.as_str();
            let record = self
                .store
                .get_template_install(tenant_id, install_id)
                .await?
                .ok_or(AppError::NotFound("TEMPLATE_INSTALL_NOT_FOUND"))?;
            if record.status == TemplateInstallStatus::Installed {
                return Ok(record);
            }
            if record.status == TemplateInstallStatus::Uninstalled {
                return Err(AppError::Conflict("TEMPLATE_ALREADY_UNINSTALLED"));
            }

            let catalog_dir =
                resolve_templates_dir().ok_or(AppError::NotFound("TEMPLATE_NOT_FOUND"))?;
            let pack_dir = resolve_pack_dir(&catalog_dir, &record.pack_id)?;
            let manifest = read_manifest(&pack_dir)?;
            verify_manifest_checksums(&pack_dir, &manifest)?;

            let node_type_renames = extract_node_type_renames(&record.plan);
            let mut session = ApplySession {
                tenant_id: principal.tenant_id.clone(),
                actor: (!principal.user_id.is_nil()).then_some(principal.user_id),
                install_id,
                pack_dir,
                manifest,
                plan: record.plan,
                node_type_renames,
                object_map: record.object_map,
            };

            let _running = self
                .store
                .update_template_install_progress(
                    tenant_id,
                    install_id,
                    TemplateInstallStatus::Installing,
                    &session.object_map,
                    None,
                )
                .await?;

            if let Err(err) = self.run_apply_pipeline(&mut session).await {
                let err_msg = err.client_message();
                if let Err(save_err) = self
                    .store
                    .update_template_install_progress(
                        tenant_id,
                        install_id,
                        TemplateInstallStatus::Failed,
                        &session.object_map,
                        Some(&err_msg),
                    )
                    .await
                {
                    tracing::warn!(install_id = %install_id, error = %save_err, "failed to record FAILED template install status");
                }
                return Err(err);
            }

            let finished = self
                .store
                .update_template_install_progress(
                    tenant_id,
                    install_id,
                    TemplateInstallStatus::Installed,
                    &session.object_map,
                    None,
                )
                .await?;

            self.emit_template_install_audit(principal, ctx, install_id, &finished)
                .await;
            Ok(finished)
        })
    }

    async fn emit_template_install_audit(
        &self,
        principal: &AuthPrincipal,
        ctx: TemplateAuditContext<'_>,
        install_id: Uuid,
        finished: &TemplateInstallRecord,
    ) {
        let actor_id = (!principal.user_id.is_nil()).then_some(principal.user_id);
        self.audit.record(AuditRecord {
            tenant_id: principal.tenant_id.clone(),
            user_id: actor_id,
            username: principal.username.clone(),
            action: AuditAction::TemplateInstall,
            resource_ref: format!("axops:template_install:{install_id}"),
            status_code: 200,
            client_ip: String::from(ctx.client_ip),
            trace_id: String::from(ctx.trace_id),
            detail_json: serde_json::json!({
                "pack_id": finished.pack_id,
                "pack_version": finished.pack_version,
                "objects_count": finished.object_map.len(),
            }),
            created_at: Utc::now(),
        });
        if let Err(err) = self.audit.flush().await {
            tracing::warn!(install_id = %install_id, error = %err, "template install audit flush failed");
        }
    }

    /// Uninstalls unmodified template objects while keeping user-modified objects (`§14.5.3`).
    pub fn uninstall_template<'a>(
        &'a self,
        tenant_id: &'a str,
        actor: Option<Uuid>,
        install_id: Uuid,
    ) -> BoxFuture<'a, TemplateUninstallResponse> {
        Box::pin(async move {
            let record = self
                .store
                .get_template_install(tenant_id, install_id)
                .await?
                .ok_or(AppError::NotFound("TEMPLATE_INSTALL_NOT_FOUND"))?;
            if record.status == TemplateInstallStatus::Uninstalled {
                return Ok(TemplateUninstallResponse {
                    id: record.id,
                    status: TemplateInstallStatus::Uninstalled,
                    deleted: Vec::new(),
                    kept: Vec::new(),
                });
            }

            let mut deleted: Vec<String> = Vec::new();
            let mut kept: Vec<String> = Vec::new();

            self.uninstall_work_items_and_exports(
                tenant_id,
                actor,
                &record.object_map,
                &mut deleted,
                &mut kept,
            )
            .await?;
            self.uninstall_agents_and_dashboards(
                tenant_id,
                &record.object_map,
                &mut deleted,
                &mut kept,
            )
            .await?;
            self.uninstall_rules_flows_metrics(
                tenant_id,
                &record.object_map,
                &mut deleted,
                &mut kept,
            )
            .await?;
            self.uninstall_schema_objects(
                tenant_id,
                actor,
                &record.object_map,
                &mut deleted,
                &mut kept,
            )
            .await?;
            self.uninstall_datasets(tenant_id, &record.object_map, &mut deleted, &mut kept)
                .await?;

            let _updated = self
                .store
                .update_template_install_progress(
                    tenant_id,
                    install_id,
                    TemplateInstallStatus::Uninstalled,
                    &record.object_map,
                    None,
                )
                .await?;

            Ok(TemplateUninstallResponse {
                id: install_id,
                status: TemplateInstallStatus::Uninstalled,
                deleted,
                kept,
            })
        })
    }

    async fn collect_existing_tenant_names(
        &self,
        tenant_id: &str,
    ) -> Result<ExistingTenantNames, AppError> {
        let mut out = ExistingTenantNames::default();
        let published = self.store.get_published_schema(tenant_id, "main").await?;
        for nt in published.doc.node_types {
            out.node_types.insert(nt.name);
        }
        for mp in published.doc.mappings {
            out.mappings.insert(mp.name);
        }

        let ds_page = self
            .store
            .list_datasets(
                tenant_id,
                DatasetListQuery {
                    limit: Some(500),
                    ..DatasetListQuery::default()
                },
            )
            .await?;
        for ds in ds_page.items {
            out.datasets.insert(ds.name);
        }

        for m in self.store.list_metrics(tenant_id).await? {
            out.metrics.insert(m.name);
        }

        self.populate_existing_app_names(tenant_id, &mut out)
            .await?;
        Ok(out)
    }

    async fn populate_existing_app_names(
        &self,
        tenant_id: &str,
        out: &mut ExistingTenantNames,
    ) -> Result<(), AppError> {
        let flows_page = self
            .store
            .list_flows(
                tenant_id,
                FlowListQuery {
                    limit: Some(500),
                    ..FlowListQuery::default()
                },
            )
            .await?;
        for fl in flows_page.items {
            out.flows.insert(fl.name);
            if let Some(slug) = fl.slug {
                out.flows.insert(slug);
            }
        }

        let rules_page = self
            .store
            .list_rules(
                tenant_id,
                RuleListQuery {
                    limit: Some(500),
                    ..RuleListQuery::default()
                },
            )
            .await?;
        for rl in rules_page.items {
            out.rules.insert(rl.name);
        }

        let dash_page = self
            .store
            .list_dashboards(
                tenant_id,
                false,
                SimplePageQuery {
                    limit: Some(200),
                    cursor: None,
                },
            )
            .await?;
        for db in dash_page.items {
            out.dashboards.insert(db.slug);
        }

        for ag in self.store.list_agents(tenant_id).await? {
            out.agents.insert(ag.name);
        }

        let tpl_page = self
            .store
            .list_export_templates(
                tenant_id,
                SimplePageQuery {
                    limit: Some(200),
                    cursor: None,
                },
            )
            .await?;
        for et in tpl_page.items {
            out.export_templates.insert(et.name);
        }
        Ok(())
    }

    async fn save_checkpoint(&self, session: &ApplySession) -> Result<(), AppError> {
        let _saved = self
            .store
            .update_template_install_progress(
                &session.tenant_id,
                session.install_id,
                TemplateInstallStatus::Installing,
                &session.object_map,
                None,
            )
            .await?;
        Ok(())
    }

    async fn run_apply_pipeline(&self, session: &mut ApplySession) -> Result<(), AppError> {
        self.apply_schema_step(session).await?;
        self.apply_datasets_step(session).await?;
        self.apply_mappings_step(session).await?;
        self.apply_metrics_step(session).await?;
        if flows_reference_agents(&session.pack_dir, &session.manifest.contents.flows) {
            self.apply_agents_step(session).await?;
            self.apply_flows_step(session).await?;
        } else {
            self.apply_flows_step(session).await?;
            self.apply_rules_step(session).await?;
            self.apply_dashboards_step(session).await?;
            self.apply_agents_step(session).await?;
            self.apply_export_templates_step(session).await?;
            self.apply_work_item_step(session).await?;
            return Ok(());
        }
        self.apply_rules_step(session).await?;
        self.apply_dashboards_step(session).await?;
        self.apply_export_templates_step(session).await?;
        self.apply_work_item_step(session).await
    }

    async fn apply_schema_step(&self, session: &mut ApplySession) -> Result<(), AppError> {
        let pack_doc: SchemaDoc = serde_json::from_value(read_pack_json(
            &session.pack_dir,
            &session.manifest.contents.schema,
        )?)
        .map_err(|err| AppError::unprocessable("PARSE_FAILED", format!("invalid schema: {err}")))?;

        let all_done = pack_doc.node_types.iter().all(|nt| {
            session
                .object_map
                .contains_key(&format!("node_type:{}", nt.name))
        });
        if all_done {
            return Ok(());
        }

        let current = self
            .store
            .get_published_schema(&session.tenant_id, "main")
            .await?;
        let merged_doc = merge_pack_schema(&current.doc, &pack_doc, session);

        let draft = self
            .store
            .create_draft(
                &session.tenant_id,
                "main",
                session.actor,
                CreateDraftRequest {
                    title: format!("Template {} schema", session.manifest.id),
                    schema_doc: merged_doc,
                    base_version: Some(current.version),
                    source: Some(DraftSource::Template),
                    ai_context: None,
                },
            )
            .await?;
        let published = self
            .store
            .publish_draft(
                &session.tenant_id,
                draft.id,
                session.actor,
                PublishDraftRequest {
                    change_summary: format!("Install template pack {}", session.manifest.id),
                    force: true,
                    cascade: false,
                },
            )
            .await?;

        for nt in &pack_doc.node_types {
            let map_key = format!("node_type:{}", nt.name);
            let planned = session.planned_for("node_type", &nt.name);
            let target_name = planned.map_or_else(|| nt.name.clone(), |p| p.target_name.clone());
            let is_skipped = planned.is_some_and(|p| p.action == "SKIP");
            let hash = if is_skipped {
                String::from(SKIPPED_HASH_SENTINEL)
            } else {
                published
                    .schema_doc
                    .node_types
                    .iter()
                    .find(|item| item.name == target_name)
                    .map_or_else(|| canonical_node_type_hash(nt), canonical_node_type_hash)
            };
            session.object_map.insert(
                map_key,
                TemplateCreatedObject {
                    id: target_name.clone(),
                    category: String::from("node_type"),
                    target_name,
                    content_hash: hash,
                },
            );
        }
        self.save_checkpoint(session).await
    }

    async fn apply_datasets_step(&self, session: &mut ApplySession) -> Result<(), AppError> {
        let timezone = self.store.get_tenant_timezone(&session.tenant_id).await?;
        let mut any_committed = false;

        for ds_entry in session.manifest.contents.datasets.clone() {
            let map_key = format!("dataset:{}", ds_entry.key);
            if session.object_map.contains_key(&map_key) {
                continue;
            }
            let planned = session.planned_for("dataset", &ds_entry.key);
            let target_name =
                planned.map_or_else(|| ds_entry.key.clone(), |p| p.target_name.clone());
            let is_skipped = planned.is_some_and(|p| p.action == "SKIP");

            if is_skipped
                && let Some(existing) = self
                    .store
                    .get_dataset_by_name(&session.tenant_id, &target_name)
                    .await?
            {
                session.object_map.insert(
                    map_key,
                    TemplateCreatedObject {
                        id: existing.id.to_string(),
                        category: String::from("dataset"),
                        target_name: existing.name,
                        content_hash: String::from(SKIPPED_HASH_SENTINEL),
                    },
                );
                self.save_checkpoint(session).await?;
                continue;
            }

            let created_ds = self
                .ingest_single_pack_dataset(session, &timezone, &ds_entry, &target_name)
                .await?;
            any_committed = true;
            session.object_map.insert(
                map_key,
                TemplateCreatedObject {
                    id: created_ds.id.to_string(),
                    category: String::from("dataset"),
                    target_name: created_ds.name.clone(),
                    content_hash: canonical_dataset_hash(&created_ds),
                },
            );
            self.save_checkpoint(session).await?;
        }

        if any_committed {
            let _evicted = self
                .analytics_pool
                .invalidate_tenant(&session.tenant_id)
                .await;
        }
        Ok(())
    }

    async fn ingest_single_pack_dataset(
        &self,
        session: &ApplySession,
        timezone: &str,
        ds_entry: &TemplateDatasetEntry,
        target_name: &str,
    ) -> Result<Dataset, AppError> {
        let csv_path = safe_pack_file(&session.pack_dir, &ds_entry.file)?;
        let csv_bytes = std::fs::read(&csv_path).map_err(|err| {
            AppError::Config(format!(
                "read sample dataset '{}' failed: {err}",
                ds_entry.file
            ))
        })?;
        let file_name = csv_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("sample.csv");

        let parsed = self
            .sidecar
            .parse_unified(
                &session.tenant_id,
                timezone,
                file_name,
                csv_bytes,
                &BTreeMap::new(),
            )
            .await?;
        let tbl = parsed.tables.first().ok_or_else(|| {
            AppError::unprocessable(
                "PARSE_FAILED",
                format!("no structured table extracted from '{}'", ds_entry.file),
            )
        })?;

        let ds = self
            .ensure_pack_dataset_row(session, ds_entry, target_name)
            .await?;
        let sj = self
            .store
            .start_sync_execution(&session.tenant_id, ds.id, Uuid::new_v4())
            .await?;
        let commit_req = CommitStagingRequest {
            tenant_id: session.tenant_id.clone(),
            parse_id: parsed.parse_id,
            items: vec![CommitStagingItem {
                staging_path: tbl.staging_path.clone(),
                dataset_id: ds.id.to_string(),
                lake_table: ds.lake_table.clone(),
                batch_id: sj.batch_id.to_string(),
                schema_policy: ds.schema_policy,
                expected_schema: ds.schema_json.clone(),
            }],
        };
        let commit_results = self
            .sidecar
            .commit_staging(&session.tenant_id, timezone, &commit_req)
            .await?;
        let sync_res = commit_results.first().ok_or_else(|| {
            AppError::Database(format!("empty commit_staging response for '{target_name}'"))
        })?;
        self.store
            .complete_sync_execution(&session.tenant_id, sj.id, ds.id, sync_res)
            .await?;

        self.store
            .get_dataset(&session.tenant_id, ds.id)
            .await?
            .ok_or(AppError::NotFound("DATASET_NOT_FOUND"))
    }

    async fn ensure_pack_dataset_row(
        &self,
        session: &ApplySession,
        ds_entry: &TemplateDatasetEntry,
        target_name: &str,
    ) -> Result<Dataset, AppError> {
        if let Some(existing) = self
            .store
            .get_dataset_by_name(&session.tenant_id, target_name)
            .await?
        {
            return Ok(existing);
        }
        self.store
            .create_dataset(
                &session.tenant_id,
                session.actor,
                CreateDatasetRequest {
                    connection_id: None,
                    name: String::from(target_name),
                    display_name: format!("{} · {target_name}", session.manifest.name),
                    description: format!("Template {} sample dataset", session.manifest.id),
                    source_kind: SourceKind::FileTable,
                    source_ref: serde_json::json!({
                        "template_pack": session.manifest.id,
                        "file": ds_entry.file,
                    }),
                    sync_mode: ds_entry.sync_mode,
                    primary_key: ds_entry.primary_key.clone(),
                    cursor_column: None,
                    delete_column: None,
                    schema_policy: SchemaPolicy::Evolve,
                    schedule_cron: None,
                    tags: vec![String::from("template"), session.manifest.id.clone()],
                },
            )
            .await
    }

    async fn apply_mappings_step(&self, session: &mut ApplySession) -> Result<(), AppError> {
        let Some(rel_path) = session.manifest.contents.mappings.clone() else {
            return Ok(());
        };
        let raw_items =
            extract_json_array(read_pack_json(&session.pack_dir, &rel_path)?, "mappings")?;
        if raw_items.is_empty() {
            return Ok(());
        }

        let current = self
            .store
            .get_published_schema(&session.tenant_id, "main")
            .await?;
        let mut merged_doc = current.doc;
        let mut pending_specs: Vec<(String, MappingSpec)> = Vec::new();

        for mut val in raw_items {
            let src_name = json_required_str(&val, "name", &rel_path)?.to_owned();
            let key = json_opt_str(&val, "key").unwrap_or_else(|| src_name.clone());
            let map_key = format!("mapping:{key}");
            if session.object_map.contains_key(&map_key) {
                continue;
            }
            resolve_json_refs(&mut val, None, &session.object_map)?;
            strip_key_field(&mut val);
            let mut spec: MappingSpec = serde_json::from_value(val).map_err(|err| {
                AppError::unprocessable(
                    "PARSE_FAILED",
                    format!("invalid mapping '{src_name}': {err}"),
                )
            })?;
            rewrite_mapping_node_types(&mut spec, &session.node_type_renames);

            let planned = session.planned_for("mapping", &key);
            if planned.is_some_and(|p| p.action == "SKIP") {
                session.object_map.insert(
                    map_key,
                    TemplateCreatedObject {
                        id: spec.name.clone(),
                        category: String::from("mapping"),
                        target_name: spec.name,
                        content_hash: String::from(SKIPPED_HASH_SENTINEL),
                    },
                );
                continue;
            }
            if let Some(p) = planned {
                spec.name = p.target_name.clone();
            }
            merged_doc.mappings.retain(|m| m.name != spec.name);
            merged_doc.mappings.push(spec.clone());
            pending_specs.push((map_key, spec));
        }

        if pending_specs.is_empty() {
            return Ok(());
        }

        self.publish_pack_mappings_draft(session, merged_doc, current.version)
            .await?;
        self.materialize_published_mappings(session, &pending_specs)
            .await
    }

    async fn publish_pack_mappings_draft(
        &self,
        session: &ApplySession,
        merged_doc: SchemaDoc,
        base_version: i32,
    ) -> Result<(), AppError> {
        let draft = self
            .store
            .create_draft(
                &session.tenant_id,
                "main",
                session.actor,
                CreateDraftRequest {
                    title: format!("Template {} mappings", session.manifest.id),
                    schema_doc: merged_doc,
                    base_version: Some(base_version),
                    source: Some(DraftSource::Template),
                    ai_context: None,
                },
            )
            .await?;
        let _pub_res = self
            .store
            .publish_draft(
                &session.tenant_id,
                draft.id,
                session.actor,
                PublishDraftRequest {
                    change_summary: format!("Publish mappings for {}", session.manifest.id),
                    force: true,
                    cascade: false,
                },
            )
            .await?;
        Ok(())
    }

    async fn materialize_published_mappings(
        &self,
        session: &mut ApplySession,
        pending_specs: &[(String, MappingSpec)],
    ) -> Result<(), AppError> {
        let records = self.store.list_mappings(&session.tenant_id, "main").await?;
        let _evicted = self
            .analytics_pool
            .invalidate_tenant(&session.tenant_id)
            .await;

        for (map_key, spec) in pending_specs {
            let rec = records
                .iter()
                .find(|r| r.name == spec.name)
                .ok_or_else(|| {
                    AppError::Database(format!("published mapping '{}' not found", spec.name))
                })?;
            let _run = self
                .store
                .execute_mapping_materialize(
                    &session.tenant_id,
                    Uuid::new_v4(),
                    &MapMaterializeJobPayload {
                        mapping_id: rec.id,
                        mode: MappingMode::Full,
                        schema_version: None,
                    },
                    self.analytics_pool.as_ref(),
                )
                .await?;
            session.object_map.insert(
                map_key.clone(),
                TemplateCreatedObject {
                    id: rec.id.to_string(),
                    category: String::from("mapping"),
                    target_name: spec.name.clone(),
                    content_hash: canonical_mapping_hash(spec),
                },
            );
            self.save_checkpoint(session).await?;
        }

        let _agg = self
            .store
            .execute_graph_aggregate(
                &session.tenant_id,
                &GraphAggregateJobPayload {
                    full: true,
                    full_type: None,
                    node_ids: Vec::new(),
                    metrics: Vec::new(),
                    schema_version: None,
                },
            )
            .await?;
        Ok(())
    }

    async fn apply_metrics_step(&self, session: &mut ApplySession) -> Result<(), AppError> {
        let Some(rel_path) = session.manifest.contents.metrics.clone() else {
            return Ok(());
        };
        let raw_items =
            extract_json_array(read_pack_json(&session.pack_dir, &rel_path)?, "metrics")?;

        for mut val in raw_items {
            let src_name = json_required_str(&val, "name", &rel_path)?.to_owned();
            let key = json_opt_str(&val, "key").unwrap_or_else(|| src_name.clone());
            let map_key = format!("metric:{key}");
            if session.object_map.contains_key(&map_key) {
                continue;
            }
            normalize_metric_dataset_field(&mut val);
            resolve_json_refs(&mut val, None, &session.object_map)?;
            strip_key_field(&mut val);

            let planned = session.planned_for("metric", &key);
            if planned.is_some_and(|p| p.action == "SKIP")
                && let Some(existing) = self
                    .store
                    .get_metric_by_name(&session.tenant_id, &src_name)
                    .await?
            {
                session.object_map.insert(
                    map_key,
                    TemplateCreatedObject {
                        id: existing.id.to_string(),
                        category: String::from("metric"),
                        target_name: existing.name,
                        content_hash: String::from(SKIPPED_HASH_SENTINEL),
                    },
                );
                self.save_checkpoint(session).await?;
                continue;
            }

            let mut req: CreateMetricRequest = serde_json::from_value(val).map_err(|err| {
                AppError::unprocessable(
                    "PARSE_FAILED",
                    format!("invalid metric '{src_name}': {err}"),
                )
            })?;
            if let Some(p) = planned {
                req.name = p.target_name.clone();
            }
            let ds = self
                .store
                .get_dataset(&session.tenant_id, req.dataset_id)
                .await?
                .ok_or(AppError::NotFound("DATASET_NOT_FOUND"))?;
            let created = self
                .store
                .create_metric(&session.tenant_id, session.actor, ds.schema_version, req)
                .await?;
            session.object_map.insert(
                map_key,
                TemplateCreatedObject {
                    id: created.id.to_string(),
                    category: String::from("metric"),
                    target_name: created.name.clone(),
                    content_hash: canonical_metric_hash(&created),
                },
            );
            self.save_checkpoint(session).await?;
        }
        Ok(())
    }

    async fn apply_flows_step(&self, session: &mut ApplySession) -> Result<(), AppError> {
        let rel_paths = session.manifest.contents.flows.clone();
        for rel_path in rel_paths {
            let mut val = read_pack_json(&session.pack_dir, &rel_path)?;
            let stem = file_stem_key(&rel_path);
            let key = json_opt_str(&val, "key").unwrap_or(stem);
            let map_key = format!("flow:{key}");
            if session.object_map.contains_key(&map_key) {
                continue;
            }
            resolve_json_refs(&mut val, None, &session.object_map)?;
            rewrite_json_node_type_refs(&mut val, &session.node_type_renames);
            strip_key_field(&mut val);

            let mut req: CreateAxFlowRequest = serde_json::from_value(val).map_err(|err| {
                AppError::unprocessable("PARSE_FAILED", format!("invalid flow '{rel_path}': {err}"))
            })?;
            let planned = session.planned_for("flow", &key);
            if planned.is_some_and(|p| p.action == "SKIP")
                && let Some(existing) = self.find_existing_flow(&session.tenant_id, &req).await?
            {
                session.object_map.insert(
                    map_key,
                    TemplateCreatedObject {
                        id: existing.id.to_string(),
                        category: String::from("flow"),
                        target_name: existing.slug.unwrap_or(existing.name),
                        content_hash: String::from(SKIPPED_HASH_SENTINEL),
                    },
                );
                self.save_checkpoint(session).await?;
                continue;
            }
            if let Some(p) = planned
                && p.action == "RENAME"
            {
                req.slug = Some(p.target_name.clone());
                req.name = format!("{} ({})", req.name, session.manifest.id);
            }

            let reloaded = self.create_and_deploy_pack_flow(session, req).await?;
            let target_name = reloaded
                .slug
                .clone()
                .unwrap_or_else(|| reloaded.name.clone());
            session.object_map.insert(
                map_key,
                TemplateCreatedObject {
                    id: reloaded.id.to_string(),
                    category: String::from("flow"),
                    target_name,
                    content_hash: canonical_flow_hash(&reloaded),
                },
            );
            self.save_checkpoint(session).await?;
        }
        Ok(())
    }

    async fn create_and_deploy_pack_flow(
        &self,
        session: &ApplySession,
        req: CreateAxFlowRequest,
    ) -> Result<AxFlowDef, AppError> {
        let flow = self
            .store
            .create_flow(&session.tenant_id, session.actor, req)
            .await?;
        let ver = self
            .store
            .publish_flow_version(
                &session.tenant_id,
                flow.id,
                session.actor,
                PublishFlowVersionRequest {
                    note: format!("Template {} install", session.manifest.id),
                },
            )
            .await?;
        for env in [Environment::Test, Environment::Prod] {
            let _dep = self
                .store
                .deploy_flow(
                    &session.tenant_id,
                    flow.id,
                    session.actor,
                    DeployFlowRequest {
                        environment: env,
                        version: ver.version,
                    },
                )
                .await?;
        }
        let reloaded = self
            .store
            .get_flow(&session.tenant_id, flow.id)
            .await?
            .unwrap_or(flow);
        Ok(reloaded)
    }

    async fn find_existing_flow(
        &self,
        tenant_id: &str,
        req: &CreateAxFlowRequest,
    ) -> Result<Option<AxFlowDef>, AppError> {
        let page = self
            .store
            .list_flows(
                tenant_id,
                FlowListQuery {
                    limit: Some(500),
                    ..FlowListQuery::default()
                },
            )
            .await?;
        Ok(page
            .items
            .into_iter()
            .find(|fl| fl.name == req.name || (req.slug.is_some() && fl.slug == req.slug)))
    }

    async fn apply_rules_step(&self, session: &mut ApplySession) -> Result<(), AppError> {
        let Some(rel_path) = session.manifest.contents.rules.clone() else {
            return Ok(());
        };
        let raw_items = extract_json_array(read_pack_json(&session.pack_dir, &rel_path)?, "rules")?;

        for mut val in raw_items {
            let src_name = json_required_str(&val, "name", &rel_path)?.to_owned();
            let key = json_opt_str(&val, "key").unwrap_or_else(|| src_name.clone());
            let map_key = format!("rule:{key}");
            if session.object_map.contains_key(&map_key) {
                continue;
            }
            resolve_json_refs(&mut val, None, &session.object_map)?;
            rewrite_json_node_type_refs(&mut val, &session.node_type_renames);
            strip_key_field(&mut val);

            let planned = session.planned_for("rule", &key);
            if planned.is_some_and(|p| p.action == "SKIP")
                && let Some(existing) = self
                    .find_existing_rule(&session.tenant_id, &src_name)
                    .await?
            {
                session.object_map.insert(
                    map_key,
                    TemplateCreatedObject {
                        id: existing.id.to_string(),
                        category: String::from("rule"),
                        target_name: existing.name,
                        content_hash: String::from(SKIPPED_HASH_SENTINEL),
                    },
                );
                self.save_checkpoint(session).await?;
                continue;
            }

            let mut req: CreateRuleRequest = serde_json::from_value(val).map_err(|err| {
                AppError::unprocessable("PARSE_FAILED", format!("invalid rule '{src_name}': {err}"))
            })?;
            if let Some(p) = planned {
                req.name = p.target_name.clone();
            }
            let created = self
                .store
                .create_rule(&session.tenant_id, session.actor, req)
                .await?;
            session.object_map.insert(
                map_key,
                TemplateCreatedObject {
                    id: created.id.to_string(),
                    category: String::from("rule"),
                    target_name: created.name.clone(),
                    content_hash: canonical_rule_hash(&created),
                },
            );
            self.save_checkpoint(session).await?;
        }
        Ok(())
    }

    async fn find_existing_rule(
        &self,
        tenant_id: &str,
        name: &str,
    ) -> Result<Option<RuleDef>, AppError> {
        let page = self
            .store
            .list_rules(
                tenant_id,
                RuleListQuery {
                    limit: Some(500),
                    ..RuleListQuery::default()
                },
            )
            .await?;
        Ok(page.items.into_iter().find(|r| r.name == name))
    }

    async fn apply_dashboards_step(&self, session: &mut ApplySession) -> Result<(), AppError> {
        let rel_paths = session.manifest.contents.dashboards.clone();
        for rel_path in rel_paths {
            let mut val = read_pack_json(&session.pack_dir, &rel_path)?;
            let stem = file_stem_key(&rel_path);
            let key = json_opt_str(&val, "key").unwrap_or(stem);
            let map_key = format!("dashboard:{key}");
            if session.object_map.contains_key(&map_key) {
                continue;
            }
            resolve_json_refs(&mut val, None, &session.object_map)?;
            rewrite_json_node_type_refs(&mut val, &session.node_type_renames);
            let mut def: PackDashboardDef = serde_json::from_value(val).map_err(|err| {
                AppError::unprocessable(
                    "PARSE_FAILED",
                    format!("invalid dashboard '{rel_path}': {err}"),
                )
            })?;
            let _unused_key = def.key.take();

            let planned = session.planned_for("dashboard", &key);
            if planned.is_some_and(|p| p.action == "SKIP")
                && let Some(existing) = self
                    .store
                    .get_dashboard_by_slug(&session.tenant_id, &def.slug, false)
                    .await?
            {
                session.object_map.insert(
                    map_key,
                    TemplateCreatedObject {
                        id: existing.dashboard.id.to_string(),
                        category: String::from("dashboard"),
                        target_name: existing.dashboard.slug,
                        content_hash: String::from(SKIPPED_HASH_SENTINEL),
                    },
                );
                self.save_checkpoint(session).await?;
                continue;
            }
            if let Some(p) = planned {
                def.slug = p.target_name.clone();
            }

            let created = self.create_and_publish_pack_dashboard(session, def).await?;
            session.object_map.insert(
                map_key,
                TemplateCreatedObject {
                    id: created.dashboard.id.to_string(),
                    category: String::from("dashboard"),
                    target_name: created.dashboard.slug.clone(),
                    content_hash: canonical_dashboard_hash(&created),
                },
            );
            self.save_checkpoint(session).await?;
        }
        Ok(())
    }

    async fn create_and_publish_pack_dashboard(
        &self,
        session: &ApplySession,
        def: PackDashboardDef,
    ) -> Result<DashboardDetail, AppError> {
        let initial = self
            .store
            .create_dashboard(
                &session.tenant_id,
                session.actor,
                CreateDashboardRequest {
                    name: def.name,
                    slug: def.slug,
                    description: def.description,
                    filters: def.filters,
                    theme: def.theme,
                    canvas: def.canvas,
                },
            )
            .await?;
        let dashboard_id = initial.dashboard.id;
        for w in def.widgets {
            let deps = compute_widget_deps(&self.store, &session.tenant_id, &w.data_source).await?;
            let _widget = self
                .store
                .create_widget(
                    &session.tenant_id,
                    CreateWidgetRecordParams {
                        dashboard_id,
                        widget_kind: w.widget_kind,
                        title: w.title,
                        position: w.position,
                        data_source: w.data_source,
                        encoding: w.encoding,
                        options: w.options,
                        drill: w.drill,
                        refresh_mode: w.refresh_mode,
                        refresh_seconds: w.refresh_seconds,
                        deps,
                    },
                )
                .await?;
        }
        let _pub_rec = self
            .store
            .publish_dashboard(&session.tenant_id, dashboard_id)
            .await?;
        self.store
            .get_dashboard(&session.tenant_id, dashboard_id, false)
            .await?
            .ok_or(AppError::NotFound("DASHBOARD_NOT_FOUND"))
    }

    async fn apply_agents_step(&self, session: &mut ApplySession) -> Result<(), AppError> {
        let rel_paths = session.manifest.contents.agents.clone();
        for rel_path in rel_paths {
            let mut val = read_pack_json(&session.pack_dir, &rel_path)?;
            let stem = file_stem_key(&rel_path);
            let key = json_opt_str(&val, "key").unwrap_or(stem);
            let map_key = format!("agent:{key}");
            if session.object_map.contains_key(&map_key) {
                continue;
            }
            resolve_json_refs(&mut val, None, &session.object_map)?;
            strip_key_field(&mut val);
            normalize_agent_spec_envelope(&mut val);

            let mut req: CreateAgentRequest = serde_json::from_value(val).map_err(|err| {
                AppError::unprocessable(
                    "PARSE_FAILED",
                    format!("invalid agent '{rel_path}': {err}"),
                )
            })?;
            let planned = session.planned_for("agent", &key);
            if planned.is_some_and(|p| p.action == "SKIP")
                && let Some(existing) = self
                    .store
                    .get_agent_by_name(&session.tenant_id, &req.name)
                    .await?
            {
                session.object_map.insert(
                    map_key,
                    TemplateCreatedObject {
                        id: existing.id.to_string(),
                        category: String::from("agent"),
                        target_name: existing.name,
                        content_hash: String::from(SKIPPED_HASH_SENTINEL),
                    },
                );
                self.save_checkpoint(session).await?;
                continue;
            }
            if let Some(p) = planned {
                req.name = p.target_name.clone();
            }

            let created = self
                .store
                .create_agent(&session.tenant_id, session.actor, req)
                .await?;
            let _ver = self
                .store
                .publish_agent_version(&session.tenant_id, created.id, session.actor)
                .await?;
            let reloaded = self
                .store
                .get_agent(&session.tenant_id, created.id)
                .await?
                .unwrap_or(created);
            session.object_map.insert(
                map_key,
                TemplateCreatedObject {
                    id: reloaded.id.to_string(),
                    category: String::from("agent"),
                    target_name: reloaded.name.clone(),
                    content_hash: canonical_agent_hash(&reloaded),
                },
            );
            self.save_checkpoint(session).await?;
        }
        Ok(())
    }

    async fn apply_export_templates_step(
        &self,
        session: &mut ApplySession,
    ) -> Result<(), AppError> {
        let rel_paths = session.manifest.contents.export_templates.clone();
        for rel_path in rel_paths {
            let key = file_stem_key(&rel_path);
            let map_key = format!("export_template:{key}");
            if session.object_map.contains_key(&map_key) {
                continue;
            }
            let planned = session.planned_for("export_template", &key);
            if planned.is_some_and(|p| p.action == "SKIP") {
                session.object_map.insert(
                    map_key,
                    TemplateCreatedObject {
                        id: key.clone(),
                        category: String::from("export_template"),
                        target_name: key,
                        content_hash: String::from(SKIPPED_HASH_SENTINEL),
                    },
                );
                self.save_checkpoint(session).await?;
                continue;
            }

            let src_file = safe_pack_file(&session.pack_dir, &rel_path)?;
            let raw_bytes = std::fs::read(&src_file).map_err(|err| {
                AppError::Config(format!("read export template '{rel_path}' failed: {err}"))
            })?;
            let export_format = if rel_path.to_ascii_lowercase().ends_with(".pptx") {
                ExportFormat::Pptx
            } else {
                ExportFormat::Docx
            };
            let placeholders = extract_ooxml_placeholders(export_format, &raw_bytes)?;
            let sha256 = sha256_bytes_hex(&raw_bytes);
            let tpl_id = Uuid::new_v4();
            let ext = if export_format == ExportFormat::Pptx {
                "pptx"
            } else {
                "docx"
            };
            let dst_dir = self
                .data_root
                .join("export_templates")
                .join(&session.tenant_id);
            tokio::fs::create_dir_all(&dst_dir).await.map_err(|err| {
                AppError::Config(format!("create export_templates dir failed: {err}"))
            })?;
            let dst_path = dst_dir.join(format!("{tpl_id}.{ext}"));
            tokio::fs::write(&dst_path, &raw_bytes)
                .await
                .map_err(|err| AppError::Config(format!("write export template failed: {err}")))?;

            let target_name = planned.map_or_else(|| key.clone(), |p| p.target_name.clone());
            let rec = self
                .store
                .create_export_template(
                    &session.tenant_id,
                    CreateExportTemplateParams {
                        id: tpl_id,
                        name: target_name.clone(),
                        export_format,
                        file_path: dst_path.to_string_lossy().into_owned(),
                        sha256,
                        placeholders,
                        created_by: session.actor,
                    },
                )
                .await?;
            session.object_map.insert(
                map_key,
                TemplateCreatedObject {
                    id: rec.id.to_string(),
                    category: String::from("export_template"),
                    target_name: rec.name.clone(),
                    content_hash: canonical_export_template_hash(&rec),
                },
            );
            self.save_checkpoint(session).await?;
        }
        Ok(())
    }

    async fn apply_work_item_step(&self, session: &mut ApplySession) -> Result<(), AppError> {
        let map_key = String::from("work_item:post_install");
        if session.object_map.contains_key(&map_key) {
            return Ok(());
        }
        let wi = self
            .store
            .create_work_item(
                &session.tenant_id,
                session.actor,
                CreateWorkItemRequest {
                    title: format!("绑定「{}」告警通知渠道", session.manifest.name),
                    description: format!(
                        "行业模板包「{}」（{}）已完成安装，请前往告警中心配置通知渠道并绑定规则。",
                        session.manifest.name, session.manifest.id
                    ),
                    item_kind: Some(WorkItemKind::Task),
                    priority: Some(WorkItemPriority::P2),
                    assignee: None,
                    source_kind: Some(WorkItemSourceKind::Template),
                    source_ref: session.manifest.id.clone(),
                    node_ids: Vec::new(),
                    due_at: None,
                },
            )
            .await?;
        session.object_map.insert(
            map_key,
            TemplateCreatedObject {
                id: wi.id.to_string(),
                category: String::from("work_item"),
                target_name: wi.code.clone(),
                content_hash: canonical_work_item_hash(&wi),
            },
        );
        self.save_checkpoint(session).await
    }

    async fn uninstall_work_items_and_exports(
        &self,
        tenant_id: &str,
        actor: Option<Uuid>,
        object_map: &BTreeMap<String, TemplateCreatedObject>,
        deleted: &mut Vec<String>,
        kept: &mut Vec<String>,
    ) -> Result<(), AppError> {
        for (map_key, obj) in object_map {
            if obj.category == "work_item" {
                if obj.content_hash == SKIPPED_HASH_SENTINEL {
                    kept.push(map_key.clone());
                    continue;
                }
                let Ok(wi_id) = Uuid::parse_str(&obj.id) else {
                    deleted.push(map_key.clone());
                    continue;
                };
                match self.store.get_work_item_detail(tenant_id, wi_id).await? {
                    None => deleted.push(map_key.clone()),
                    Some(detail) if canonical_work_item_hash(&detail.item) == obj.content_hash => {
                        let _patched = self
                            .store
                            .patch_work_item(
                                tenant_id,
                                wi_id,
                                actor,
                                PatchWorkItemRequest {
                                    status: Some(WorkItemStatus::Canceled),
                                    ..PatchWorkItemRequest::default()
                                },
                            )
                            .await?;
                        deleted.push(map_key.clone());
                    }
                    Some(_) => kept.push(map_key.clone()),
                }
            } else if obj.category == "export_template" {
                if obj.content_hash == SKIPPED_HASH_SENTINEL {
                    kept.push(map_key.clone());
                    continue;
                }
                let Ok(tpl_id) = Uuid::parse_str(&obj.id) else {
                    deleted.push(map_key.clone());
                    continue;
                };
                match self.store.get_export_template(tenant_id, tpl_id).await? {
                    None => deleted.push(map_key.clone()),
                    Some(tpl) if canonical_export_template_hash(&tpl) == obj.content_hash => {
                        if let Some(removed) =
                            self.store.delete_export_template(tenant_id, tpl_id).await?
                        {
                            let _rm = tokio::fs::remove_file(&removed.file_path).await;
                        }
                        deleted.push(map_key.clone());
                    }
                    Some(_) => kept.push(map_key.clone()),
                }
            }
        }
        Ok(())
    }

    async fn uninstall_agents_and_dashboards(
        &self,
        tenant_id: &str,
        object_map: &BTreeMap<String, TemplateCreatedObject>,
        deleted: &mut Vec<String>,
        kept: &mut Vec<String>,
    ) -> Result<(), AppError> {
        for (map_key, obj) in object_map {
            if obj.category == "agent" {
                if obj.content_hash == SKIPPED_HASH_SENTINEL {
                    kept.push(map_key.clone());
                    continue;
                }
                let Ok(ag_id) = Uuid::parse_str(&obj.id) else {
                    deleted.push(map_key.clone());
                    continue;
                };
                match self.store.get_agent(tenant_id, ag_id).await? {
                    None => deleted.push(map_key.clone()),
                    Some(ag) if canonical_agent_hash(&ag) == obj.content_hash => {
                        let _removed = self.store.delete_agent(tenant_id, ag_id).await?;
                        deleted.push(map_key.clone());
                    }
                    Some(_) => kept.push(map_key.clone()),
                }
            } else if obj.category == "dashboard" {
                if obj.content_hash == SKIPPED_HASH_SENTINEL {
                    kept.push(map_key.clone());
                    continue;
                }
                let Ok(db_id) = Uuid::parse_str(&obj.id) else {
                    deleted.push(map_key.clone());
                    continue;
                };
                match self.store.get_dashboard(tenant_id, db_id, false).await? {
                    None => deleted.push(map_key.clone()),
                    Some(db) if canonical_dashboard_hash(&db) == obj.content_hash => {
                        let _removed = self.store.delete_dashboard(tenant_id, db_id).await?;
                        deleted.push(map_key.clone());
                    }
                    Some(_) => kept.push(map_key.clone()),
                }
            }
        }
        Ok(())
    }

    async fn uninstall_rules_flows_metrics(
        &self,
        tenant_id: &str,
        object_map: &BTreeMap<String, TemplateCreatedObject>,
        deleted: &mut Vec<String>,
        kept: &mut Vec<String>,
    ) -> Result<(), AppError> {
        for (map_key, obj) in object_map {
            if obj.category == "rule" {
                if obj.content_hash == SKIPPED_HASH_SENTINEL {
                    kept.push(map_key.clone());
                    continue;
                }
                let Ok(rule_id) = Uuid::parse_str(&obj.id) else {
                    deleted.push(map_key.clone());
                    continue;
                };
                match self.store.get_rule(tenant_id, rule_id).await? {
                    None => deleted.push(map_key.clone()),
                    Some(rl) if canonical_rule_hash(&rl) == obj.content_hash => {
                        let _removed = self.store.delete_rule(tenant_id, rule_id).await?;
                        deleted.push(map_key.clone());
                    }
                    Some(_) => kept.push(map_key.clone()),
                }
            }
        }

        for (map_key, obj) in object_map {
            if obj.category == "flow" {
                if obj.content_hash == SKIPPED_HASH_SENTINEL {
                    kept.push(map_key.clone());
                    continue;
                }
                let Ok(flow_id) = Uuid::parse_str(&obj.id) else {
                    deleted.push(map_key.clone());
                    continue;
                };
                match self.store.get_flow(tenant_id, flow_id).await? {
                    None => deleted.push(map_key.clone()),
                    Some(fl) if canonical_flow_hash(&fl) == obj.content_hash => {
                        let _removed = self.store.delete_flow(tenant_id, flow_id).await?;
                        deleted.push(map_key.clone());
                    }
                    Some(_) => kept.push(map_key.clone()),
                }
            }
        }

        for (map_key, obj) in object_map {
            if obj.category == "metric" {
                if obj.content_hash == SKIPPED_HASH_SENTINEL {
                    kept.push(map_key.clone());
                    continue;
                }
                match self
                    .store
                    .get_metric_by_name(tenant_id, &obj.target_name)
                    .await?
                {
                    None => deleted.push(map_key.clone()),
                    Some(mt) if canonical_metric_hash(&mt) == obj.content_hash => {
                        let _removed = self
                            .store
                            .delete_metric(tenant_id, &obj.target_name)
                            .await?;
                        deleted.push(map_key.clone());
                    }
                    Some(_) => kept.push(map_key.clone()),
                }
            }
        }
        Ok(())
    }

    async fn uninstall_schema_objects(
        &self,
        tenant_id: &str,
        actor: Option<Uuid>,
        object_map: &BTreeMap<String, TemplateCreatedObject>,
        deleted: &mut Vec<String>,
        kept: &mut Vec<String>,
    ) -> Result<(), AppError> {
        let current = self.store.get_published_schema(tenant_id, "main").await?;
        let mut next_doc = current.doc.clone();
        let mut mappings_to_drop: BTreeSet<String> = BTreeSet::new();
        let mut node_types_to_drop: BTreeSet<String> = BTreeSet::new();

        for (map_key, obj) in object_map {
            if obj.category == "mapping" {
                if obj.content_hash == SKIPPED_HASH_SENTINEL {
                    kept.push(map_key.clone());
                    continue;
                }
                let found = current
                    .doc
                    .mappings
                    .iter()
                    .find(|m| m.name == obj.target_name);
                match found {
                    None => deleted.push(map_key.clone()),
                    Some(mp) if canonical_mapping_hash(mp) == obj.content_hash => {
                        mappings_to_drop.insert(obj.target_name.clone());
                        deleted.push(map_key.clone());
                    }
                    Some(_) => kept.push(map_key.clone()),
                }
            }
        }

        for (map_key, obj) in object_map {
            if obj.category == "node_type" {
                if obj.content_hash == SKIPPED_HASH_SENTINEL {
                    kept.push(map_key.clone());
                    continue;
                }
                let still_mapped = current.doc.mappings.iter().any(|m| {
                    !mappings_to_drop.contains(&m.name)
                        && (m.node_type == obj.target_name
                            || m.links.iter().any(|l| l.target_type == obj.target_name))
                });
                let found = current
                    .doc
                    .node_types
                    .iter()
                    .find(|nt| nt.name == obj.target_name);
                match found {
                    None => deleted.push(map_key.clone()),
                    Some(nt)
                        if !still_mapped && canonical_node_type_hash(nt) == obj.content_hash =>
                    {
                        node_types_to_drop.insert(obj.target_name.clone());
                        deleted.push(map_key.clone());
                    }
                    Some(_) => kept.push(map_key.clone()),
                }
            }
        }

        if mappings_to_drop.is_empty() && node_types_to_drop.is_empty() {
            return Ok(());
        }

        prune_schema_for_uninstall(&mut next_doc, &mappings_to_drop, &node_types_to_drop);
        let _saved = self
            .store
            .save_schema_direct(
                tenant_id,
                "main",
                actor,
                SaveSchemaDirectRequest {
                    doc: next_doc,
                    base_version: Some(current.version),
                    change_summary: Some(String::from("Uninstall template schema objects")),
                    force: true,
                    cascade: true,
                },
            )
            .await?;
        Ok(())
    }

    async fn uninstall_datasets(
        &self,
        tenant_id: &str,
        object_map: &BTreeMap<String, TemplateCreatedObject>,
        deleted: &mut Vec<String>,
        kept: &mut Vec<String>,
    ) -> Result<(), AppError> {
        let mut any_dropped = false;
        for (map_key, obj) in object_map {
            if obj.category != "dataset" {
                continue;
            }
            if obj.content_hash == SKIPPED_HASH_SENTINEL {
                kept.push(map_key.clone());
                continue;
            }
            let Ok(ds_id) = Uuid::parse_str(&obj.id) else {
                deleted.push(map_key.clone());
                continue;
            };
            match self.store.get_dataset(tenant_id, ds_id).await? {
                None => deleted.push(map_key.clone()),
                Some(ds) if canonical_dataset_hash(&ds) == obj.content_hash => {
                    match self.store.delete_dataset(tenant_id, ds_id).await {
                        Ok(Some(removed)) => {
                            if let Err(err) = self
                                .sidecar
                                .drop_lake_table(tenant_id, &removed.lake_table)
                                .await
                            {
                                tracing::warn!(lake_table = %removed.lake_table, error = %err, "drop_lake_table during uninstall failed");
                            }
                            any_dropped = true;
                            deleted.push(map_key.clone());
                        }
                        Ok(None) => deleted.push(map_key.clone()),
                        Err(_) => kept.push(map_key.clone()),
                    }
                }
                Some(_) => kept.push(map_key.clone()),
            }
        }
        if any_dropped {
            let _evicted = self.analytics_pool.invalidate_tenant(tenant_id).await;
        }
        Ok(())
    }
}

fn prune_schema_for_uninstall(
    doc: &mut SchemaDoc,
    mappings_to_drop: &BTreeSet<String>,
    node_types_to_drop: &BTreeSet<String>,
) {
    doc.mappings.retain(|m| !mappings_to_drop.contains(&m.name));
    doc.node_types
        .retain(|nt| !node_types_to_drop.contains(&nt.name));
    doc.edge_types.retain(|et| {
        !node_types_to_drop.contains(&et.src_type_name)
            && !node_types_to_drop.contains(&et.dst_type_name)
    });
    for nt in &mut doc.node_types {
        nt.agg_rules
            .retain(|ar| !node_types_to_drop.contains(&ar.source_type_name));
    }
}

fn merge_pack_schema(
    current: &SchemaDoc,
    pack_doc: &SchemaDoc,
    session: &ApplySession,
) -> SchemaDoc {
    let mut merged = current.clone();
    for nt in &pack_doc.node_types {
        let planned = session.planned_for("node_type", &nt.name);
        if planned.is_some_and(|p| p.action == "SKIP") {
            continue;
        }
        let mut cloned = nt.clone();
        cloned.id = None;
        if let Some(p) = planned {
            cloned.name = p.target_name.clone();
        }
        for agg in &mut cloned.agg_rules {
            if let Some(renamed) = session.node_type_renames.get(&agg.source_type_name) {
                agg.source_type_name = renamed.clone();
            }
        }
        merged
            .node_types
            .retain(|existing| existing.name != cloned.name);
        merged.node_types.push(cloned);
    }

    for et in &pack_doc.edge_types {
        let mut cloned = et.clone();
        cloned.id = None;
        if let Some(renamed) = session.node_type_renames.get(&cloned.src_type_name) {
            cloned.src_type_name = renamed.clone();
        }
        if let Some(renamed) = session.node_type_renames.get(&cloned.dst_type_name) {
            cloned.dst_type_name = renamed.clone();
        }
        let already_exists = merged.edge_types.iter().any(|existing| {
            existing.name == cloned.name
                && existing.src_type_name == cloned.src_type_name
                && existing.dst_type_name == cloned.dst_type_name
        });
        if !already_exists {
            if merged.edge_types.iter().any(|e| e.name == cloned.name) {
                cloned.name = format!(
                    "{}_{}",
                    cloned.name,
                    session.manifest.id.to_ascii_uppercase()
                );
            }
            merged.edge_types.push(cloned);
        }
    }
    merged
}

fn extract_node_type_renames(plan: &TemplateInstallPlan) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for obj in &plan.objects {
        if obj.category == "node_type" && obj.source_name != obj.target_name {
            map.insert(obj.source_name.clone(), obj.target_name.clone());
        }
    }
    map
}

fn rewrite_mapping_node_types(spec: &mut MappingSpec, renames: &BTreeMap<String, String>) {
    if let Some(nt) = renames.get(&spec.node_type) {
        spec.node_type = nt.clone();
    }
    for link in &mut spec.links {
        if let Some(tt) = renames.get(&link.target_type) {
            link.target_type = tt.clone();
        }
    }
}

fn rewrite_json_node_type_refs(val: &mut serde_json::Value, renames: &BTreeMap<String, String>) {
    if renames.is_empty() {
        return;
    }
    match val {
        serde_json::Value::Object(map) => {
            for field in ["type_name", "node_type", "target_type", "source_type_name"] {
                if let Some(serde_json::Value::String(cur)) = map.get_mut(field)
                    && let Some(renamed) = renames.get(cur.as_str())
                {
                    *cur = renamed.clone();
                }
            }
            if let Some(serde_json::Value::Array(arr)) = map.get_mut("type_names") {
                for elem in arr {
                    if let serde_json::Value::String(cur) = elem
                        && let Some(renamed) = renames.get(cur.as_str())
                    {
                        *cur = renamed.clone();
                    }
                }
            }
            for child in map.values_mut() {
                rewrite_json_node_type_refs(child, renames);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                rewrite_json_node_type_refs(item, renames);
            }
        }
        _ => {}
    }
}

fn flows_reference_agents(pack_dir: &Path, flow_files: &[String]) -> bool {
    flow_files.iter().any(|rel| {
        safe_pack_file(pack_dir, rel)
            .ok()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .is_some_and(|s| s.contains("\"agent:"))
    })
}

fn normalize_metric_dataset_field(val: &mut serde_json::Value) {
    if let serde_json::Value::Object(map) = val
        && !map.contains_key("dataset_id")
        && let Some(ds_val) = map.remove("dataset")
    {
        map.insert(String::from("dataset_id"), ds_val);
    }
}

fn normalize_agent_spec_envelope(val: &mut serde_json::Value) {
    let serde_json::Value::Object(map) = val else {
        return;
    };
    if map.contains_key("spec") {
        return;
    }
    let mut spec_map = serde_json::Map::new();
    for field in [
        "system_prompt",
        "provider_purpose",
        "provider_id",
        "temperature",
        "max_steps",
        "memory_turns",
        "tools",
        "approval_required_tools",
        "knowledge_scope",
        "output_schema",
    ] {
        if let Some(v) = map.remove(field) {
            spec_map.insert(String::from(field), v);
        }
    }
    map.insert(String::from("spec"), serde_json::Value::Object(spec_map));
}

fn strip_key_field(val: &mut serde_json::Value) {
    if let serde_json::Value::Object(map) = val {
        let _removed = map.remove("key");
    }
}

fn json_required_str<'a>(
    val: &'a serde_json::Value,
    field: &str,
    rel_path: &str,
) -> Result<&'a str, AppError> {
    val.get(field)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            AppError::unprocessable("PARSE_FAILED", format!("missing '{field}' in '{rel_path}'"))
        })
}

fn json_opt_str(val: &serde_json::Value, field: &str) -> Option<String> {
    val.get(field)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
}

/// Computes the canonical `SHA-256` snapshot hash of a [`NodeTypeSpec`] (ignoring DB-assigned `id`).
#[must_use]
pub fn canonical_node_type_hash(nt: &NodeTypeSpec) -> String {
    let mut clean = nt.clone();
    clean.id = None;
    canonical_json_hash(&clean)
}

/// Computes the canonical `SHA-256` snapshot hash of a [`Dataset`].
#[must_use]
pub fn canonical_dataset_hash(ds: &Dataset) -> String {
    canonical_json_hash(&serde_json::json!({
        "name": ds.name,
        "display_name": ds.display_name,
        "sync_mode": ds.sync_mode,
        "schema_version": ds.schema_version,
        "row_count": ds.row_count,
    }))
}

/// Computes the canonical `SHA-256` snapshot hash of a [`MappingSpec`].
#[must_use]
pub fn canonical_mapping_hash(mp: &MappingSpec) -> String {
    canonical_json_hash(mp)
}

/// Computes the canonical `SHA-256` snapshot hash of a [`MetricDef`].
#[must_use]
pub fn canonical_metric_hash(mt: &MetricDef) -> String {
    canonical_json_hash(&serde_json::json!({
        "name": mt.name,
        "display_name": mt.display_name,
        "description": mt.description,
        "dataset_id": mt.dataset_id,
        "expression": mt.expression,
        "filter_sql": mt.filter_sql,
        "time_column": mt.time_column,
        "default_grain": mt.default_grain,
        "dimensions": mt.dimensions,
        "unit": mt.unit,
        "format": mt.format,
        "certified": mt.certified,
        "version": mt.version,
    }))
}

/// Computes the canonical `SHA-256` snapshot hash of an [`AxFlowDef`].
#[must_use]
pub fn canonical_flow_hash(fl: &AxFlowDef) -> String {
    canonical_json_hash(&serde_json::json!({
        "name": fl.name,
        "description": fl.description,
        "slug": fl.slug,
        "nodes": fl.nodes,
        "edges": fl.edges,
        "trigger_cfg": fl.trigger_cfg,
    }))
}

/// Computes the canonical `SHA-256` snapshot hash of a [`RuleDef`].
#[must_use]
pub fn canonical_rule_hash(rl: &RuleDef) -> String {
    canonical_json_hash(&serde_json::json!({
        "name": rl.name,
        "description": rl.description,
        "scope_kind": rl.scope_kind,
        "target": rl.target,
        "condition": rl.condition,
        "severity": rl.severity,
        "title_template": rl.title_template,
        "body_template": rl.body_template,
        "cooldown_minutes": rl.cooldown_minutes,
        "resolve_after_minutes": rl.resolve_after_minutes,
        "escalate_after_minutes": rl.escalate_after_minutes,
        "channel_ids": rl.channel_ids,
        "escalation_channel_ids": rl.escalation_channel_ids,
        "run_flow_id": rl.run_flow_id,
        "enabled": rl.enabled,
        "version": rl.version,
    }))
}

/// Computes the canonical `SHA-256` snapshot hash of a [`DashboardDetail`] and its ordered widgets.
#[must_use]
pub fn canonical_dashboard_hash(detail: &DashboardDetail) -> String {
    let widgets: Vec<serde_json::Value> = detail
        .widgets
        .iter()
        .map(|w| {
            serde_json::json!({
                "widget_kind": w.widget_kind,
                "title": w.title,
                "position": w.position,
                "data_source": w.data_source,
                "encoding": w.encoding,
                "options": w.options,
            })
        })
        .collect();
    canonical_json_hash(&serde_json::json!({
        "name": detail.dashboard.name,
        "slug": detail.dashboard.slug,
        "description": detail.dashboard.description,
        "theme": detail.dashboard.theme,
        "filters": detail.dashboard.filters,
        "version": detail.dashboard.version,
        "widgets": widgets,
    }))
}

/// Computes the canonical `SHA-256` snapshot hash of an [`AgentRecord`].
#[must_use]
pub fn canonical_agent_hash(ag: &AgentRecord) -> String {
    canonical_json_hash(&serde_json::json!({
        "name": ag.name,
        "description": ag.description,
        "spec": ag.spec,
        "version": ag.version,
    }))
}

/// Computes the canonical `SHA-256` snapshot hash of an [`ExportTemplateRecord`].
#[must_use]
pub fn canonical_export_template_hash(tpl: &ExportTemplateRecord) -> String {
    canonical_json_hash(&serde_json::json!({
        "name": tpl.name,
        "export_format": tpl.export_format,
        "sha256": tpl.sha256,
    }))
}

/// Computes the canonical `SHA-256` snapshot hash of a post-install [`WorkItem`].
#[must_use]
pub fn canonical_work_item_hash(wi: &WorkItem) -> String {
    canonical_json_hash(&serde_json::json!({
        "title": wi.title,
        "description": wi.description,
        "status": wi.status,
        "priority": wi.priority,
        "assignee": wi.assignee,
    }))
}

fn resolve_pack_dir(catalog_dir: &Path, pack_id: &str) -> Result<PathBuf, AppError> {
    let trimmed = pack_id.trim();
    if trimmed.is_empty()
        || trimmed.len() > 64
        || !trimmed
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
    {
        return Err(AppError::NotFound("TEMPLATE_NOT_FOUND"));
    }
    let dir = catalog_dir.join(trimmed);
    if !dir.join("manifest.json").is_file() {
        return Err(AppError::NotFound("TEMPLATE_NOT_FOUND"));
    }
    Ok(dir)
}

fn safe_pack_file(pack_dir: &Path, rel_path: &str) -> Result<PathBuf, AppError> {
    let rel = Path::new(rel_path.trim());
    if rel.as_os_str().is_empty()
        || rel.is_absolute()
        || rel.components().any(|c| {
            matches!(
                c,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            format!("invalid relative template path '{rel_path}'"),
        ));
    }
    Ok(pack_dir.join(rel))
}

fn read_manifest(pack_dir: &Path) -> Result<TemplateManifest, AppError> {
    let raw = std::fs::read_to_string(pack_dir.join("manifest.json"))
        .map_err(|err| AppError::Config(format!("read manifest.json failed: {err}")))?;
    serde_json::from_str::<TemplateManifest>(&raw).map_err(|err| {
        AppError::unprocessable("PARSE_FAILED", format!("invalid manifest.json: {err}"))
    })
}

fn read_pack_json(pack_dir: &Path, rel_path: &str) -> Result<serde_json::Value, AppError> {
    let full = safe_pack_file(pack_dir, rel_path)?;
    let raw = std::fs::read_to_string(&full).map_err(|err| {
        AppError::unprocessable("PARSE_FAILED", format!("read '{rel_path}' failed: {err}"))
    })?;
    serde_json::from_str::<serde_json::Value>(&raw).map_err(|err| {
        AppError::unprocessable("PARSE_FAILED", format!("parse '{rel_path}' failed: {err}"))
    })
}

fn sha256_bytes_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn to_pascal_case(raw: &str) -> String {
    let mut out = String::new();
    for seg in raw.split(|c: char| !c.is_ascii_alphanumeric()) {
        let mut chars = seg.chars();
        if let Some(first) = chars.next() {
            out.push(first.to_ascii_uppercase());
            for rest in chars {
                out.push(rest.to_ascii_lowercase());
            }
        }
    }
    if out.is_empty() {
        String::from("Pack")
    } else {
        out
    }
}

fn truncate_chars(s: &str, max_chars: usize) -> String {
    s.chars().take(max_chars).collect()
}

fn file_stem_key(rel_path: &str) -> String {
    Path::new(rel_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .map_or_else(|| String::from("item"), String::from)
}

fn extract_json_array(
    val: serde_json::Value,
    wrapper_key: &str,
) -> Result<Vec<serde_json::Value>, AppError> {
    match val {
        serde_json::Value::Array(arr) => Ok(arr),
        serde_json::Value::Object(mut map) => match map.remove(wrapper_key) {
            Some(serde_json::Value::Array(arr)) => Ok(arr),
            _ => Err(AppError::unprocessable(
                "PARSE_FAILED",
                format!("expected JSON array or object with '{wrapper_key}' array"),
            )),
        },
        _ => Err(AppError::unprocessable(
            "PARSE_FAILED",
            format!("expected JSON array or '{wrapper_key}' envelope"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axpoint_domain::ops::TemplateContents;
    use axpoint_domain::view::{DashboardRecord, DashboardStatus};

    fn make_temp_pack_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("axops_tpl_test_{label}_{}", Uuid::new_v4()));
        let _created = std::fs::create_dir_all(&dir);
        dir
    }

    #[test]
    fn verify_manifest_checksums_detects_tampered_and_missing_files() {
        let pack_dir = make_temp_pack_dir("sha");
        let schema_bytes = br#"{"node_types":[],"edge_types":[],"mappings":[]}"#;
        assert!(std::fs::write(pack_dir.join("schema.json"), schema_bytes).is_ok());

        let good_sha = sha256_bytes_hex(schema_bytes);
        let mut manifest = TemplateManifest {
            id: String::from("demo_pack"),
            name: String::from("演示包"),
            version: String::from("1.0.0"),
            industry: String::from("通用"),
            description: String::from("测试模板包"),
            contents: TemplateContents {
                schema: String::from("schema.json"),
                datasets: Vec::new(),
                mappings: None,
                metrics: None,
                rules: None,
                flows: Vec::new(),
                dashboards: Vec::new(),
                agents: Vec::new(),
                export_templates: Vec::new(),
            },
            sha256: BTreeMap::from([(String::from("schema.json"), good_sha)]),
        };

        assert!(verify_manifest_checksums(&pack_dir, &manifest).is_ok());

        manifest.sha256.insert(
            String::from("schema.json"),
            String::from("0000000000000000000000000000000000000000000000000000000000000000"),
        );
        let err = verify_manifest_checksums(&pack_dir, &manifest);
        assert!(matches!(
            err,
            Err(AppError::Unprocessable {
                code: "CHECKSUM_MISMATCH",
                ..
            })
        ));
        let _cleanup = std::fs::remove_dir_all(&pack_dir);
    }

    #[test]
    fn resolve_json_refs_substitutes_uuid_and_logical_names() -> Result<(), AppError> {
        let ds_uuid = Uuid::from_u128(42).to_string();
        let flow_uuid = Uuid::from_u128(99).to_string();
        let object_map = BTreeMap::from([
            (
                String::from("dataset:orders"),
                TemplateCreatedObject {
                    id: ds_uuid.clone(),
                    category: String::from("dataset"),
                    target_name: String::from("orders_supply_chain"),
                    content_hash: String::from("h1"),
                },
            ),
            (
                String::from("flow:daily_refresh"),
                TemplateCreatedObject {
                    id: flow_uuid.clone(),
                    category: String::from("flow"),
                    target_name: String::from("supply-daily-refresh"),
                    content_hash: String::from("h2"),
                },
            ),
        ]);

        let mut payload = serde_json::json!({
            "dataset": { "$ref": "dataset:orders" },
            "dataset_id": { "$ref": "dataset:orders" },
            "run_flow_id": { "$ref": "flow:daily_refresh" }
        });
        resolve_json_refs(&mut payload, None, &object_map)?;
        assert_eq!(
            payload.get("dataset").and_then(serde_json::Value::as_str),
            Some("orders_supply_chain")
        );
        assert_eq!(
            payload
                .get("dataset_id")
                .and_then(serde_json::Value::as_str),
            Some(ds_uuid.as_str())
        );
        assert_eq!(
            payload
                .get("run_flow_id")
                .and_then(serde_json::Value::as_str),
            Some(flow_uuid.as_str())
        );

        let mut bad = serde_json::json!({ "dataset_id": { "$ref": "dataset:missing" } });
        assert!(matches!(
            resolve_json_refs(&mut bad, None, &object_map),
            Err(AppError::Unprocessable {
                code: "UNRESOLVED_TEMPLATE_REF",
                ..
            })
        ));
        Ok(())
    }

    #[test]
    fn conflict_planning_supports_skip_and_category_aware_rename() -> Result<(), AppError> {
        assert_eq!(
            compute_renamed_target("node_type", "Factory", "supply_chain"),
            "FactorySupplyChain"
        );
        assert_eq!(
            compute_renamed_target("dashboard", "supply-overview", "supply_chain"),
            "supply-overview-supply-chain"
        );
        assert_eq!(
            compute_renamed_target("dataset", "orders", "supply_chain"),
            "orders_supply_chain"
        );

        if let Some(packs_dir) = resolve_templates_dir() {
            let manifests = list_templates_in_dir(&packs_dir)?;
            for m in &manifests {
                let pdir = resolve_pack_dir(&packs_dir, &m.id)?;
                verify_manifest_checksums(&pdir, m)?;
            }
            if let Some(sc) = manifests.iter().find(|m| m.id == "supply_chain") {
                let pdir = resolve_pack_dir(&packs_dir, &sc.id)?;
                let mut existing = ExistingTenantNames::default();
                existing.node_types.insert(String::from("Factory"));
                existing.dashboards.insert(String::from("supply-overview"));

                let rename_plan = build_install_plan_from_dir(
                    &pdir,
                    sc,
                    TemplateConflictStrategy::Rename,
                    &existing,
                )?;
                assert_eq!(rename_plan.conflicts.len(), 2);
                assert!(rename_plan.objects.iter().any(|o| {
                    o.category == "node_type"
                        && o.source_name == "Factory"
                        && o.target_name == "FactorySupplyChain"
                        && o.action == "RENAME"
                }));
                assert!(rename_plan.objects.iter().any(|o| {
                    o.category == "dashboard"
                        && o.source_name == "supply-overview"
                        && o.target_name == "supply-overview-supply-chain"
                        && o.action == "RENAME"
                }));

                let skip_plan = build_install_plan_from_dir(
                    &pdir,
                    sc,
                    TemplateConflictStrategy::Skip,
                    &existing,
                )?;
                assert!(skip_plan.objects.iter().any(|o| {
                    o.category == "dashboard"
                        && o.source_name == "supply-overview"
                        && o.action == "SKIP"
                }));
            }
        }
        Ok(())
    }

    #[test]
    fn canonical_dashboard_hash_changes_when_user_edits_metadata() {
        let now = Utc::now();
        let mut detail = DashboardDetail {
            dashboard: DashboardRecord {
                id: Uuid::from_u128(1),
                tenant_id: String::from("default"),
                name: String::from("原始驾驶舱"),
                slug: String::from("supply-overview"),
                description: String::from("desc"),
                filters: Vec::new(),
                theme: DashboardTheme::Light,
                canvas: CanvasConfig::default(),
                status: DashboardStatus::Published,
                version: 1,
                created_by: None,
                created_at: now,
                updated_at: now,
            },
            widgets: Vec::new(),
        };
        let initial_hash = canonical_dashboard_hash(&detail);
        detail.dashboard.updated_at = now + chrono::Duration::seconds(60);
        assert_eq!(canonical_dashboard_hash(&detail), initial_hash);

        detail.dashboard.name = String::from("用户自定义驾驶舱标题");
        assert_ne!(canonical_dashboard_hash(&detail), initial_hash);
    }
}
