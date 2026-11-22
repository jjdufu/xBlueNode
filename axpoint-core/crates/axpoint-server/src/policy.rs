//! Central `AxGuard` `Cedar` `ABAC` authorization and [`PolicyContext`] compiler shared across
//! `axpoint-server` HTTP routes, background workers, and `xBlueView` execution engines (`11-STEP13` §13.6–§13.7).

use std::collections::{BTreeMap, BTreeSet};

use axpoint_domain::AppError;
use axpoint_domain::axhub::{Dataset, DatasetListQuery, DatasetStatus};
use axpoint_domain::enums::{MaskRule, Permission, PiiCategory, Sensitivity};
use axpoint_domain::graph::NodeSearchHit;
use axpoint_domain::guard::{
    CedarPolicyRecord, ColumnPolicyRecord, DepartmentRecord, GrantRecord, GrantResourceKind,
    GrantStatus, GranteeKind, GuardPrincipalProfile, NodeTypePolicySpec, PolicyContext,
    PolicyTargetKind, ResourceRef, RowPolicyDefaultRecord, RowPolicyRecord, UserAccountStatus,
};
use axpoint_domain::ports::{DatasetRepository, GuardRepository, StudioRepository};
use axpoint_domain::studio::{DraftStatus, NodeTypeSpec};
use axpoint_domain::topology::BlueNode;
use axpoint_security::AuthPrincipal;
use axpoint_security::cedar_authz::{CedarAuthorizer, CedarContext, dept_lineage};
use axpoint_security::masking::mask_json_properties;
use axpoint_security::row_policy::{
    DatasetPolicyInput, NodeTypePolicyInput, resolve_dataset_policy, resolve_node_type_policy,
};
use chrono::{DateTime, Timelike as _, Utc};
use uuid::Uuid;

/// Bundled tenant governance metadata loaded once when compiling a [`PolicyContext`].
struct TenantGovernanceSnapshot {
    departments: Vec<DepartmentRecord>,
    cedar_policies: Vec<CedarPolicyRecord>,
    row_policies: Vec<RowPolicyRecord>,
    row_defaults: Vec<RowPolicyDefaultRecord>,
    column_policies: Vec<ColumnPolicyRecord>,
    active_grants: Vec<GrantRecord>,
}

/// Returns the current UTC hour (`0..=23`) for `CedarContext.hour`.
#[must_use]
pub fn current_utc_hour() -> u8 {
    u8::try_from(Utc::now().hour()).unwrap_or(0)
}

fn normalize_ip(client_ip: &str) -> String {
    let trimmed = client_ip.trim();
    if trimmed.is_empty() {
        String::from("127.0.0.1")
    } else {
        String::from(trimmed)
    }
}

/// Resolves an [`AuthPrincipal`] into an active [`GuardPrincipalProfile`], intersecting DB permissions
/// with token-scoped `principal.permissions` (for scoped `axp_` tokens and masked export/share principals).
///
/// # Errors
/// Returns `AppError::Unauthorized("TOKEN_INVALID")` if the user does not exist or is `DISABLED`,
/// or [`AppError::Database`] if querying [`GuardRepository`] fails.
pub async fn load_effective_guard_profile<S>(
    store: &S,
    principal: &AuthPrincipal,
) -> Result<GuardPrincipalProfile, AppError>
where
    S: GuardRepository + ?Sized,
{
    if principal.user_id.is_nil() {
        let mut base = store
            .load_guard_principal(&principal.tenant_id, Uuid::nil())
            .await?
            .unwrap_or_else(|| GuardPrincipalProfile {
                user_id: Uuid::nil(),
                tenant_id: principal.tenant_id.clone(),
                username: principal.username.clone(),
                display_name: principal.username.clone(),
                email: String::new(),
                dept_id: principal.dept_id.clone(),
                clearance: Sensitivity::Secret,
                status: UserAccountStatus::Active,
                tokens_valid_after: DateTime::UNIX_EPOCH,
                roles: principal.roles.clone(),
                permissions: principal.permissions.clone(),
                attributes: BTreeMap::new(),
            });
        base.clearance = Sensitivity::Secret;
        base.status = UserAccountStatus::Active;
        base.permissions.clone_from(&principal.permissions);
        if base.roles.is_empty() {
            base.roles.clone_from(&principal.roles);
        }
        return Ok(base);
    }

    let mut profile = store
        .load_guard_principal(&principal.tenant_id, principal.user_id)
        .await?
        .ok_or(AppError::Unauthorized("TOKEN_INVALID"))?;

    if profile.status != UserAccountStatus::Active {
        return Err(AppError::Unauthorized("TOKEN_INVALID"));
    }

    profile.permissions = profile
        .permissions
        .intersection(&principal.permissions)
        .copied()
        .collect();
    Ok(profile)
}

/// Checks whether any active, non-expired `sec_grants` row grants `action` on `(kind, primary_ref | secondary_ref)`
/// to `profile` (directly, via `dept_lineage`, or via `profile.roles`).
#[must_use]
pub fn has_matching_grant(
    active_grants: &[GrantRecord],
    kind: GrantResourceKind,
    primary_ref: &str,
    secondary_ref: &str,
    action: Permission,
    profile: &GuardPrincipalProfile,
    lineage: &[String],
) -> bool {
    let now = Utc::now();
    let uid_str = profile.user_id.to_string();
    for grant in active_grants {
        if grant.status != GrantStatus::Active || grant.expires_at <= now {
            continue;
        }
        if grant.resource_kind != kind || !grant.actions.contains(&action) {
            continue;
        }
        let ref_matches = grant.resource_ref.eq_ignore_ascii_case(primary_ref)
            || grant.resource_ref.eq_ignore_ascii_case(secondary_ref);
        if !ref_matches {
            continue;
        }
        let subject_matches = match grant.grantee_kind {
            GranteeKind::User => grant.grantee_ref.eq_ignore_ascii_case(&uid_str),
            GranteeKind::Dept => lineage
                .iter()
                .any(|d| d.eq_ignore_ascii_case(&grant.grantee_ref)),
            GranteeKind::Role => profile
                .roles
                .iter()
                .any(|r| r.eq_ignore_ascii_case(&grant.grantee_ref)),
        };
        if subject_matches {
            return true;
        }
    }
    false
}

/// Enforces `Cedar` `ABAC` authorization on a governed resource resolved from `store`, synthesizing
/// a default `INTERNAL` / `"default"` resource descriptor when unregistered (`11-STEP13` §13.6.1).
///
/// # Errors
/// Returns `401 TOKEN_INVALID`, `403 POLICY_DENIED`, or database errors.
pub async fn authorize_resource_for_store<S>(
    store: &S,
    cedar: &CedarAuthorizer,
    principal: &AuthPrincipal,
    action: Permission,
    kind: GrantResourceKind,
    resource_ref: &str,
    client_ip: &str,
) -> Result<ResourceRef, AppError>
where
    S: GuardRepository + ?Sized,
{
    let profile = load_effective_guard_profile(store, principal).await?;
    let resource = store
        .resolve_resource_ref(&principal.tenant_id, kind, resource_ref)
        .await?
        .unwrap_or_else(|| ResourceRef {
            kind,
            id: String::from(resource_ref),
            owner_dept: String::from("default"),
            sensitivity: Sensitivity::Internal,
        });
    authorize_explicit_resource_for_profile(
        store,
        cedar,
        &profile,
        action,
        &resource,
        resource_ref,
        client_ip,
    )
    .await?;
    Ok(resource)
}

/// Enforces `Cedar` `ABAC` authorization against an explicit [`ResourceRef`] for `principal`.
///
/// # Errors
/// Returns `401 TOKEN_INVALID`, `403 POLICY_DENIED`, or database errors.
pub async fn authorize_explicit_resource_for_store<S>(
    store: &S,
    cedar: &CedarAuthorizer,
    principal: &AuthPrincipal,
    action: Permission,
    resource: &ResourceRef,
    client_ip: &str,
) -> Result<(), AppError>
where
    S: GuardRepository + ?Sized,
{
    let profile = load_effective_guard_profile(store, principal).await?;
    authorize_explicit_resource_for_profile(
        store,
        cedar,
        &profile,
        action,
        resource,
        &resource.id,
        client_ip,
    )
    .await
}

async fn authorize_explicit_resource_for_profile<S>(
    store: &S,
    cedar: &CedarAuthorizer,
    profile: &GuardPrincipalProfile,
    action: Permission,
    resource: &ResourceRef,
    secondary_ref: &str,
    client_ip: &str,
) -> Result<(), AppError>
where
    S: GuardRepository + ?Sized,
{
    let tenant_id = &profile.tenant_id;
    let departments = store.list_departments(tenant_id).await?;
    let custom_policies = store.list_cedar_policies(tenant_id).await?;
    let active_grants = store.list_active_grants(tenant_id).await?;
    let lineage = dept_lineage(&profile.dept_id, &departments);
    let granted = has_matching_grant(
        &active_grants,
        resource.kind,
        &resource.id,
        secondary_ref,
        action,
        profile,
        &lineage,
    );
    let ctx = CedarContext {
        ip: normalize_ip(client_ip),
        hour: current_utc_hour(),
        mfa: false,
        granted,
    };
    cedar.authorize(
        profile,
        action,
        resource,
        &ctx,
        &departments,
        &custom_policies,
    )
}

/// Compiles the complete [`PolicyContext`] (`datasets`, `node_types`, `denied_datasets`, and `reveal_pii`
/// eligibility) for an authenticated [`AuthPrincipal`] (`11-STEP13` §13.6.3, §13.7).
///
/// # Errors
/// - Returns `401 TOKEN_INVALID` if the principal is missing or disabled.
/// - Returns `403 PERMISSION_DENIED` if `reveal_pii_requested` is `true` and caller is neither
///   granted `Permission::PiiRawRead` nor matched by any `sec_column_policies.exempt` selector.
/// - Returns `400 INVALID_PREDICATE` or [`AppError::Database`] on policy compilation or DB failure.
pub async fn build_policy_context_for_store<S>(
    store: &S,
    cedar: &CedarAuthorizer,
    principal: &AuthPrincipal,
    reveal_pii_requested: bool,
    client_ip: &str,
) -> Result<PolicyContext, AppError>
where
    S: GuardRepository + DatasetRepository + StudioRepository + ?Sized,
{
    build_policy_context_for_subgraph(
        store,
        cedar,
        principal,
        "main",
        reveal_pii_requested,
        client_ip,
    )
    .await
}

/// Compiles the complete [`PolicyContext`] for `principal` scoped to `subgraph_slug`, overlaying
/// active `DRAFT` and published subgraph node-type `PII` declarations onto `"main"` (`03-STEP17` §17.3.1).
///
/// # Errors
/// - Returns `401 TOKEN_INVALID` if the principal is missing or disabled.
/// - Returns `403 PERMISSION_DENIED` if `reveal_pii_requested` is `true` and caller lacks
///   `Permission::PiiRawRead` and matches no `sec_column_policies.exempt` selector.
/// - Returns `400 INVALID_PREDICATE` or [`AppError::Database`] on policy compilation or DB failure.
pub async fn build_policy_context_for_subgraph<S>(
    store: &S,
    cedar: &CedarAuthorizer,
    principal: &AuthPrincipal,
    subgraph_slug: &str,
    reveal_pii_requested: bool,
    client_ip: &str,
) -> Result<PolicyContext, AppError>
where
    S: GuardRepository + DatasetRepository + StudioRepository + ?Sized,
{
    Box::pin(async move {
        let profile = load_effective_guard_profile(store, principal).await?;
        build_policy_context_for_profile_inner(
            store,
            cedar,
            &profile,
            subgraph_slug,
            reveal_pii_requested,
            client_ip,
        )
        .await
    })
    .await
}

/// Compiles a [`PolicyContext`] directly for an already-loaded [`GuardPrincipalProfile`]
/// (used by both live query execution and `POST /api/axguard/row-policies/preview`).
///
/// # Errors
/// Propagates `403 PERMISSION_DENIED` (invalid `reveal_pii`), `400 INVALID_PREDICATE`, or DB errors.
pub async fn build_policy_context_for_profile<S>(
    store: &S,
    cedar: &CedarAuthorizer,
    profile: &GuardPrincipalProfile,
    reveal_pii_requested: bool,
    client_ip: &str,
) -> Result<PolicyContext, AppError>
where
    S: GuardRepository + DatasetRepository + StudioRepository + ?Sized,
{
    Box::pin(build_policy_context_for_profile_inner(
        store,
        cedar,
        profile,
        "main",
        reveal_pii_requested,
        client_ip,
    ))
    .await
}

fn merge_node_type_into(merged: &mut BTreeMap<String, NodeTypeSpec>, incoming: &NodeTypeSpec) {
    let Some(existing) = merged.get_mut(&incoming.name) else {
        merged.insert(incoming.name.clone(), incoming.clone());
        return;
    };
    for prop in &incoming.properties {
        if let Some(ex_prop) = existing.properties.iter_mut().find(|p| p.name == prop.name) {
            if ex_prop.pii_category == PiiCategory::None && prop.pii_category != PiiCategory::None {
                ex_prop.pii_category = prop.pii_category;
            }
        } else {
            existing.properties.push(prop.clone());
        }
    }
}

async fn load_effective_node_types<S>(
    store: &S,
    tenant_id: &str,
    subgraph_slug: &str,
) -> Result<Vec<NodeTypeSpec>, AppError>
where
    S: StudioRepository + ?Sized,
{
    let main_pub = store.get_published_schema(tenant_id, "main").await?;
    let clean_slug = subgraph_slug.trim();
    if clean_slug.is_empty() || clean_slug == "main" {
        return Ok(main_pub.doc.node_types);
    }

    let mut merged: BTreeMap<String, NodeTypeSpec> = BTreeMap::new();
    let active_drafts = store
        .list_drafts(tenant_id, clean_slug, Some(DraftStatus::Draft))
        .await
        .unwrap_or_default();
    for draft in &active_drafts {
        for nt in &draft.schema_doc.node_types {
            merge_node_type_into(&mut merged, nt);
        }
    }
    if active_drafts.is_empty()
        && let Ok(all_drafts) = store.list_drafts(tenant_id, clean_slug, None).await
    {
        for draft in &all_drafts {
            for nt in &draft.schema_doc.node_types {
                merge_node_type_into(&mut merged, nt);
            }
        }
    }
    if let Ok(sub_pub) = store.get_published_schema(tenant_id, clean_slug).await {
        for nt in &sub_pub.doc.node_types {
            merge_node_type_into(&mut merged, nt);
        }
    }
    for nt in &main_pub.doc.node_types {
        merge_node_type_into(&mut merged, nt);
    }
    Ok(merged.into_values().collect())
}

async fn build_policy_context_for_profile_inner<S>(
    store: &S,
    cedar: &CedarAuthorizer,
    profile: &GuardPrincipalProfile,
    subgraph_slug: &str,
    reveal_pii_requested: bool,
    client_ip: &str,
) -> Result<PolicyContext, AppError>
where
    S: GuardRepository + DatasetRepository + StudioRepository + ?Sized,
{
    let tenant_id = &profile.tenant_id;
    let snap = load_governance_snapshot(store, tenant_id).await?;
    let all_datasets = load_all_active_datasets(store, tenant_id).await?;
    let effective_node_types = load_effective_node_types(store, tenant_id, subgraph_slug).await?;
    let lineage = dept_lineage(&profile.dept_id, &snap.departments);
    let ip = normalize_ip(client_ip);
    let hour = current_utc_hour();

    let has_global_pii_raw = profile.permissions.contains(&Permission::PiiRawRead);
    let bypass_row_policies = profile.permissions.contains(&Permission::DataAdmin)
        || profile.permissions.contains(&Permission::GuardAdmin);

    let mut datasets_map = BTreeMap::new();
    let mut denied_datasets = BTreeSet::new();
    let mut any_exempt_matched = false;

    for ds in &all_datasets {
        let outcome = compile_single_dataset_entry(
            store,
            cedar,
            profile,
            ds,
            &snap,
            (&lineage, &ip, hour, reveal_pii_requested),
        )
        .await?;
        match outcome {
            DatasetCompileOutcome::Denied => {
                denied_datasets.insert(ds.id.to_string());
                denied_datasets.insert(ds.name.to_ascii_lowercase());
            }
            DatasetCompileOutcome::Allowed { spec, exempt } => {
                if exempt {
                    any_exempt_matched = true;
                }
                datasets_map.insert(ds.id.to_string(), spec.clone());
                datasets_map.insert(ds.name.to_ascii_lowercase(), spec);
            }
        }
    }

    let mut node_types_map = BTreeMap::new();
    for nt in &effective_node_types {
        let (spec, exempt) = compile_single_node_type_entry(
            store,
            cedar,
            profile,
            nt,
            &snap,
            (&lineage, &ip, hour, reveal_pii_requested),
        )
        .await?;
        if exempt {
            any_exempt_matched = true;
        }
        node_types_map.insert(nt.name.clone(), spec);
    }

    if reveal_pii_requested && !has_global_pii_raw && !any_exempt_matched {
        return Err(AppError::Forbidden("PERMISSION_DENIED"));
    }

    Ok(PolicyContext {
        bypass_row_policies,
        reveal_pii_authorized: reveal_pii_requested && (has_global_pii_raw || any_exempt_matched),
        datasets: datasets_map,
        node_types: node_types_map,
        denied_datasets,
    })
}

enum DatasetCompileOutcome {
    Denied,
    Allowed {
        spec: axpoint_domain::guard::DatasetPolicySpec,
        exempt: bool,
    },
}

async fn load_governance_snapshot<S>(
    store: &S,
    tenant_id: &str,
) -> Result<TenantGovernanceSnapshot, AppError>
where
    S: GuardRepository + ?Sized,
{
    let departments = store.list_departments(tenant_id).await?;
    let cedar_policies = store.list_cedar_policies(tenant_id).await?;
    let row_policies = store.list_row_policies(tenant_id).await?;
    let row_defaults = store.list_row_policy_defaults(tenant_id).await?;
    let column_policies = store.list_column_policies(tenant_id).await?;
    let active_grants = store.list_active_grants(tenant_id).await?;
    Ok(TenantGovernanceSnapshot {
        departments,
        cedar_policies,
        row_policies,
        row_defaults,
        column_policies,
        active_grants,
    })
}

async fn load_all_active_datasets<S>(store: &S, tenant_id: &str) -> Result<Vec<Dataset>, AppError>
where
    S: DatasetRepository + ?Sized,
{
    let mut out = Vec::new();
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
        for ds in page.items {
            if ds.status != DatasetStatus::Disabled {
                out.push(ds);
            }
        }
        if page.next_cursor.is_none() {
            break;
        }
        cursor = page.next_cursor;
    }
    Ok(out)
}

fn lookup_default_deny(
    defaults: &[RowPolicyDefaultRecord],
    kind: PolicyTargetKind,
    primary_ref: &str,
    secondary_ref: &str,
) -> bool {
    defaults
        .iter()
        .find(|d| {
            d.target_kind == kind
                && (d.target_ref.eq_ignore_ascii_case(primary_ref)
                    || d.target_ref.eq_ignore_ascii_case(secondary_ref))
        })
        .is_some_and(|d| d.default_deny)
}

async fn compile_single_dataset_entry<S>(
    store: &S,
    cedar: &CedarAuthorizer,
    profile: &GuardPrincipalProfile,
    ds: &Dataset,
    snap: &TenantGovernanceSnapshot,
    env: (&[String], &str, u8, bool),
) -> Result<DatasetCompileOutcome, AppError>
where
    S: GuardRepository + DatasetRepository + ?Sized,
{
    let (lineage, ip, hour, reveal_pii_requested) = env;
    let ds_id_str = ds.id.to_string();
    let res_ref = store
        .resolve_resource_ref(&profile.tenant_id, GrantResourceKind::Dataset, &ds_id_str)
        .await?
        .unwrap_or_else(|| ResourceRef {
            kind: GrantResourceKind::Dataset,
            id: ds_id_str.clone(),
            owner_dept: String::from("default"),
            sensitivity: Sensitivity::Internal,
        });

    let granted = has_matching_grant(
        &snap.active_grants,
        GrantResourceKind::Dataset,
        &ds_id_str,
        &ds.name,
        Permission::DataRead,
        profile,
        lineage,
    );
    let eval = cedar.evaluate(
        profile,
        Permission::DataRead,
        &res_ref,
        &CedarContext {
            ip: String::from(ip),
            hour,
            mfa: false,
            granted,
        },
        &snap.departments,
        &snap.cedar_policies,
    );
    if !eval.allowed {
        return Ok(DatasetCompileOutcome::Denied);
    }

    let columns = store.list_columns(&profile.tenant_id, ds.id).await?;
    let default_deny = lookup_default_deny(
        &snap.row_defaults,
        PolicyTargetKind::Dataset,
        &ds_id_str,
        &ds.name,
    );
    let has_global_pii_raw = profile.permissions.contains(&Permission::PiiRawRead);

    let build_input = |reveal: bool| DatasetPolicyInput {
        profile,
        dept_lineage: lineage,
        dataset_id: ds.id,
        dataset_name: &ds.name,
        dataset_columns: &columns,
        row_policies: &snap.row_policies,
        default_deny,
        column_policies: &snap.column_policies,
        active_grants: &snap.active_grants,
        reveal_pii: reveal,
    };

    let (spec, exempt) = if reveal_pii_requested && !has_global_pii_raw {
        match resolve_dataset_policy(&build_input(true)) {
            Ok(pair) => pair,
            Err(AppError::Forbidden("PERMISSION_DENIED")) => {
                resolve_dataset_policy(&build_input(false))?
            }
            Err(other) => return Err(other),
        }
    } else {
        resolve_dataset_policy(&build_input(reveal_pii_requested))?
    };

    Ok(DatasetCompileOutcome::Allowed { spec, exempt })
}

async fn compile_single_node_type_entry<S>(
    store: &S,
    cedar: &CedarAuthorizer,
    profile: &GuardPrincipalProfile,
    nt: &NodeTypeSpec,
    snap: &TenantGovernanceSnapshot,
    env: (&[String], &str, u8, bool),
) -> Result<(NodeTypePolicySpec, bool), AppError>
where
    S: GuardRepository + ?Sized,
{
    let (lineage, ip, hour, reveal_pii_requested) = env;
    let res_ref = store
        .resolve_resource_ref(&profile.tenant_id, GrantResourceKind::NodeType, &nt.name)
        .await?
        .unwrap_or_else(|| ResourceRef {
            kind: GrantResourceKind::NodeType,
            id: nt.name.clone(),
            owner_dept: String::from("default"),
            sensitivity: Sensitivity::Internal,
        });

    let granted = has_matching_grant(
        &snap.active_grants,
        GrantResourceKind::NodeType,
        &nt.name,
        &nt.name,
        Permission::DataRead,
        profile,
        lineage,
    );
    let eval = cedar.evaluate(
        profile,
        Permission::DataRead,
        &res_ref,
        &CedarContext {
            ip: String::from(ip),
            hour,
            mfa: false,
            granted,
        },
        &snap.departments,
        &snap.cedar_policies,
    );
    if !eval.allowed {
        return Ok((
            NodeTypePolicySpec {
                duckdb_row_filter: Some(String::from("FALSE")),
                pg_row_filter: Some(String::from("FALSE")),
                property_masks: BTreeMap::new(),
            },
            false,
        ));
    }

    let default_deny = lookup_default_deny(
        &snap.row_defaults,
        PolicyTargetKind::NodeType,
        &nt.name,
        &nt.name,
    );
    let has_global_pii_raw = profile.permissions.contains(&Permission::PiiRawRead);

    let build_input = |reveal: bool| NodeTypePolicyInput {
        profile,
        dept_lineage: lineage,
        type_name: &nt.name,
        properties: &nt.properties,
        row_policies: &snap.row_policies,
        default_deny,
        column_policies: &snap.column_policies,
        active_grants: &snap.active_grants,
        reveal_pii: reveal,
    };

    if reveal_pii_requested && !has_global_pii_raw {
        match resolve_node_type_policy(&build_input(true)) {
            Ok(pair) => Ok(pair),
            Err(AppError::Forbidden("PERMISSION_DENIED")) => {
                resolve_node_type_policy(&build_input(false))
            }
            Err(other) => Err(other),
        }
    } else {
        resolve_node_type_policy(&build_input(reveal_pii_requested))
    }
}

/// Applies compiled `PolicyContext` property masking rules in-place to a [`BlueNode`].
pub fn apply_node_property_masks(
    node: &mut BlueNode,
    policy: &PolicyContext,
    tenant_hmac_key: &[u8; 32],
) {
    if let Some(spec) = policy.node_types.get(&node.type_name) {
        mask_json_properties(&mut node.properties, &spec.property_masks, tenant_hmac_key);
    }
}

/// Applies compiled `PolicyContext` property masking rules in-place to a [`NodeSearchHit`].
pub fn apply_search_hit_property_masks(
    hit: &mut NodeSearchHit,
    policy: &PolicyContext,
    tenant_hmac_key: &[u8; 32],
) {
    if let Some(spec) = policy.node_types.get(&hit.type_name) {
        mask_json_properties(&mut hit.properties, &spec.property_masks, tenant_hmac_key);
    }
}

/// Resolves the effective property masking map for `type_name` from `policy`, falling back to
/// `NodeTypeSpec` default `pii_category` masks when `policy` has no entry for `type_name`.
#[must_use]
pub fn effective_property_masks(
    type_name: &str,
    nt: Option<&NodeTypeSpec>,
    policy: Option<&PolicyContext>,
    has_pii_raw: bool,
) -> BTreeMap<String, MaskRule> {
    if let Some(pol) = policy
        && let Some(spec) = pol.node_types.get(type_name)
    {
        return spec.property_masks.clone();
    }
    if has_pii_raw {
        return BTreeMap::new();
    }
    let mut fallback = BTreeMap::new();
    if let Some(nt_spec) = nt {
        for prop in &nt_spec.properties {
            let rule = MaskRule::from_pii_category(prop.pii_category);
            if rule != MaskRule::None {
                fallback.insert(prop.name.clone(), rule);
            }
        }
    }
    fallback
}

#[cfg(test)]
mod tests {
    use super::*;
    use axpoint_domain::enums::DataType;
    use axpoint_domain::studio::{PropertyKind, PropertySpec};

    fn make_prop(name: &str, pii_category: PiiCategory) -> PropertySpec {
        PropertySpec {
            name: String::from(name),
            display_name: String::from(name),
            description: String::new(),
            unit: None,
            data_type: DataType::String,
            kind: PropertyKind::Attribute,
            required: false,
            allowed_values: None,
            pii_category,
        }
    }

    #[test]
    fn merge_node_type_into_unions_sandbox_draft_and_main_pii_properties() {
        let draft_order = NodeTypeSpec {
            id: None,
            name: String::from("Order"),
            display_name: String::from("订单"),
            description: String::new(),
            icon: String::new(),
            color: String::new(),
            title_template: String::new(),
            properties: vec![
                make_prop("customer_phone", PiiCategory::Phone),
                make_prop("contact_email", PiiCategory::None),
            ],
            agg_rules: Vec::new(),
            derived_rules: Vec::new(),
            sync_to_knowledge: false,
            pos_x: 0.0,
            pos_y: 0.0,
        };
        let mut main_order = draft_order.clone();
        main_order.properties = vec![
            make_prop("contact_email", PiiCategory::Email),
            make_prop("id_card", PiiCategory::IdCard),
        ];

        let mut merged = BTreeMap::new();
        merge_node_type_into(&mut merged, &draft_order);
        merge_node_type_into(&mut merged, &main_order);

        let effective = merged.get("Order");
        let masks = effective_property_masks("Order", effective, None, false);
        assert_eq!(
            masks.get("customer_phone"),
            Some(&MaskRule::from_pii_category(PiiCategory::Phone))
        );
        assert_eq!(
            masks.get("contact_email"),
            Some(&MaskRule::from_pii_category(PiiCategory::Email))
        );
        assert_eq!(
            masks.get("id_card"),
            Some(&MaskRule::from_pii_category(PiiCategory::IdCard))
        );
    }
}
