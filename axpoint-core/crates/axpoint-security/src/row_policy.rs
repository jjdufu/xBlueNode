//! Row-level security (`RLS`) predicate `AST` validator, dual-dialect (`DuckDB` / `PostgreSQL`)
//! compiler, state-driven node visibility (`properties.*` / `metrics.*` + TTL), and dataset /
//! node-type policy resolvers (`11-STEP13` §13.6.2, §13.6.3, `05-STEP27` §27.3).

use std::collections::{BTreeMap, BTreeSet};

use axpoint_domain::AppError;
use axpoint_domain::axhub::ColumnMeta;
use axpoint_domain::enums::{MaskRule, Permission, PiiCategory};
use axpoint_domain::guard::{
    ColumnPolicyRecord, DatasetPolicySpec, GrantRecord, GrantResourceKind, GrantStatus,
    GranteeKind, GuardPrincipalProfile, NodeTypePolicySpec, PolicyTargetKind, RowPolicyRecord,
};
use axpoint_domain::studio::PropertySpec;
use axpoint_domain::topology::PropertyDef;
use chrono::Utc;
use uuid::Uuid;

use crate::row_policy_ast::{
    PlaceholderMode, compile_expr, compile_ttl_clause, parse_predicate_expr,
};

/// Dual-dialect boolean SQL predicate compiled from a validated row policy expression (`§13.6.2`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompiledPredicate {
    /// Compiled `DuckDB` SQL predicate expression (`lake."tenant_x"."t"` or `ax.nodes`).
    pub duckdb_sql: String,
    /// Compiled `PostgreSQL` SQL predicate expression (`xb_nodes` or relational tables).
    pub pg_sql: String,
}

/// Validates a dataset row-policy `predicate` against the `§13.6.2` `EBNF` grammar and optional
/// `allowed_columns` whitelist without requiring a runtime user profile.
///
/// # Errors
/// Returns `AppError::BadRequest` (`INVALID_PREDICATE`) if `predicate` contains comments,
/// semicolons, unknown `{{...}}` placeholders, disallowed `AST` constructs, `{{user.attr.*}}` /
/// `{{user.roles}}` outside `IN`, or columns absent from `allowed_columns`.
pub fn validate_predicate_syntax(
    predicate: &str,
    allowed_columns: Option<&BTreeSet<String>>,
) -> Result<(), AppError> {
    validate_predicate_syntax_for_target(predicate, PolicyTargetKind::Dataset, allowed_columns)
}

/// Validates a row-policy `predicate` for a specific `target_kind` (`DATASET` or `NODE_TYPE`),
/// permitting `properties.*` / `metrics.*` / `node.prop.*` / `node.metric.*` state fields on
/// `PolicyTargetKind::NodeType` (`05-STEP27` §27.3).
///
/// # Errors
/// Returns `AppError::BadRequest` (`INVALID_PREDICATE`) on syntax, grammar, placeholder, or
/// column/property-whitelist violations.
pub fn validate_predicate_syntax_for_target(
    predicate: &str,
    target_kind: PolicyTargetKind,
    allowed_columns: Option<&BTreeSet<String>>,
) -> Result<(), AppError> {
    let ast = parse_predicate_expr(predicate)?;
    let _compiled = compile_expr(
        &ast,
        target_kind,
        allowed_columns,
        PlaceholderMode::SyntaxValidationOnly,
    )?;
    Ok(())
}

/// Validates optional `ttl_timestamp_field` and `ttl_max_age_hours` active-window parameters (`05-STEP27` §27.1).
///
/// # Errors
/// Returns `AppError::BadRequest` (`INVALID_PREDICATE`) if `ttl_max_age_hours` is outside `1..=87_600`,
/// paired inconsistently with `ttl_timestamp_field`, or references an invalid/unknown column.
pub fn validate_ttl_config(
    target_kind: PolicyTargetKind,
    ttl_timestamp_field: Option<&str>,
    ttl_max_age_hours: Option<i32>,
    allowed_columns: Option<&BTreeSet<String>>,
) -> Result<(), AppError> {
    let _ttl = compile_ttl_clause(
        target_kind,
        ttl_timestamp_field,
        ttl_max_age_hours,
        allowed_columns,
    )?;
    Ok(())
}

/// Validates and compiles `predicate` for `target_kind` (`DATASET` or `NODE_TYPE`), substituting
/// `profile` attributes and enforcing `allowed_columns` (`§13.6.2`, `05-STEP27` §27.3).
///
/// # Errors
/// Returns `AppError::BadRequest` (`INVALID_PREDICATE`) on syntax, grammar, placeholder, or
/// column-whitelist violations.
pub fn compile_predicate(
    predicate: &str,
    target_kind: PolicyTargetKind,
    allowed_columns: Option<&BTreeSet<String>>,
    profile: &GuardPrincipalProfile,
) -> Result<CompiledPredicate, AppError> {
    let ast = parse_predicate_expr(predicate)?;
    compile_expr(
        &ast,
        target_kind,
        allowed_columns,
        PlaceholderMode::BindProfile(profile),
    )
}

/// Validates and compiles `predicate` together with optional TTL expiration (`ttl_timestamp_field`
/// and `ttl_max_age_hours`) for `target_kind` (`05-STEP27` §27.1, §27.3).
///
/// # Errors
/// Returns `AppError::BadRequest` (`INVALID_PREDICATE`) if `predicate` or the TTL configuration is invalid.
pub fn compile_predicate_with_ttl(
    predicate: &str,
    target_kind: PolicyTargetKind,
    allowed_columns: Option<&BTreeSet<String>>,
    profile: &GuardPrincipalProfile,
    ttl_timestamp_field: Option<&str>,
    ttl_max_age_hours: Option<i32>,
) -> Result<CompiledPredicate, AppError> {
    let base = compile_predicate(predicate, target_kind, allowed_columns, profile)?;
    let Some(ttl) = compile_ttl_clause(
        target_kind,
        ttl_timestamp_field,
        ttl_max_age_hours,
        allowed_columns,
    )?
    else {
        return Ok(base);
    };
    Ok(CompiledPredicate {
        duckdb_sql: format!("({} AND {})", base.duckdb_sql, ttl.duckdb_sql),
        pg_sql: format!("({} AND {})", base.pg_sql, ttl.pg_sql),
    })
}

/// Input parameters for [`resolve_dataset_policy`] (`§13.6.3`, `§13.7`).
#[derive(Debug, Clone)]
pub struct DatasetPolicyInput<'a> {
    /// Caller principal profile.
    pub profile: &'a GuardPrincipalProfile,
    /// Cycle-safe department ancestor lineage `[dept_id, parent, ...]`.
    pub dept_lineage: &'a [String],
    /// Dataset UUID.
    pub dataset_id: Uuid,
    /// Dataset SQL name.
    pub dataset_name: &'a str,
    /// Dataset column metadata slice.
    pub dataset_columns: &'a [ColumnMeta],
    /// Tenant `sec_row_policies` records.
    pub row_policies: &'a [RowPolicyRecord],
    /// Dataset `default_deny` toggle from `sec_row_policy_defaults`.
    pub default_deny: bool,
    /// Tenant `sec_column_policies` records.
    pub column_policies: &'a [ColumnPolicyRecord],
    /// Active `sec_grants` records.
    pub active_grants: &'a [GrantRecord],
    /// Whether caller explicitly requested `reveal_pii: true`.
    pub reveal_pii: bool,
}

/// Resolves the effective [`DatasetPolicySpec`] and `(spec, any_exempt_matched)` for a dataset
/// (`§13.6.3`, `§13.7`).
///
/// # Errors
/// - Returns `AppError::Forbidden("PERMISSION_DENIED")` if `input.reveal_pii` is `true`, caller
///   lacks `Permission::PiiRawRead`, and no column `exempt` selector matched the caller.
/// - Returns `AppError::BadRequest` (`INVALID_PREDICATE`) if a matching row predicate fails to compile.
pub fn resolve_dataset_policy(
    input: &DatasetPolicyInput<'_>,
) -> Result<(DatasetPolicySpec, bool), AppError> {
    let ds_id_str = input.dataset_id.to_string();
    let allowed_cols: BTreeSet<String> = input
        .dataset_columns
        .iter()
        .map(|c| c.column_name.to_ascii_lowercase())
        .collect();
    let allowed_opt = (!allowed_cols.is_empty()).then_some(&allowed_cols);

    let bypass_rows = input.profile.permissions.contains(&Permission::DataAdmin)
        || input.profile.permissions.contains(&Permission::GuardAdmin);

    let duckdb_row_filter = if bypass_rows {
        None
    } else {
        let mut or_branches = Vec::new();
        for rp in input.row_policies {
            if !rp.enabled || rp.target_kind != PolicyTargetKind::Dataset {
                continue;
            }
            if !matches_target_ref(&rp.target_ref, &ds_id_str, input.dataset_name) {
                continue;
            }
            if rp.applies_to.matches_applies_to(
                input.profile.user_id,
                input.dept_lineage,
                &input.profile.roles,
            ) {
                let compiled = compile_predicate_with_ttl(
                    &rp.predicate,
                    PolicyTargetKind::Dataset,
                    allowed_opt,
                    input.profile,
                    rp.ttl_timestamp_field.as_deref(),
                    rp.ttl_max_age_hours,
                )?;
                or_branches.push(compiled.duckdb_sql);
            }
        }
        for grant in input.active_grants {
            if !grant_matches_subject_and_resource(
                grant,
                GrantResourceKind::Dataset,
                &ds_id_str,
                input.dataset_name,
                input.profile,
                input.dept_lineage,
            ) {
                continue;
            }
            if let Some(ref pred) = grant.row_predicate
                && !pred.trim().is_empty()
            {
                let compiled =
                    compile_predicate(pred, PolicyTargetKind::Dataset, allowed_opt, input.profile)?;
                or_branches.push(compiled.duckdb_sql);
            }
        }
        combine_or_predicates(&or_branches, input.default_deny)
    };

    let col_pairs: Vec<(&str, PiiCategory)> = input
        .dataset_columns
        .iter()
        .map(|c| (c.column_name.as_str(), c.pii_category))
        .collect();
    let (column_masks, any_exempt_matched) = resolve_column_masks_ctx(&ColumnMaskResolveCtx {
        target_kind: PolicyTargetKind::Dataset,
        primary_ref: &ds_id_str,
        secondary_ref: input.dataset_name,
        columns: &col_pairs,
        column_policies: input.column_policies,
        profile: input.profile,
        dept_lineage: input.dept_lineage,
        reveal_pii: input.reveal_pii,
    })?;

    Ok((
        DatasetPolicySpec {
            duckdb_row_filter,
            column_masks,
        },
        any_exempt_matched,
    ))
}

/// Trait abstracting property `(name, pii_category)` metadata across [`PropertySpec`],
/// [`PropertyDef`], and tuple slices for [`resolve_node_type_policy`].
pub trait NodePropertyMeta {
    /// Property identifier (`xb_nodes.properties` or `xb_nodes.metrics` key).
    fn property_name(&self) -> &str;
    /// Default PII classification on this property.
    fn property_pii(&self) -> PiiCategory;
}

impl NodePropertyMeta for PropertySpec {
    fn property_name(&self) -> &str {
        &self.name
    }
    fn property_pii(&self) -> PiiCategory {
        self.pii_category
    }
}

impl NodePropertyMeta for PropertyDef {
    fn property_name(&self) -> &str {
        &self.name
    }
    fn property_pii(&self) -> PiiCategory {
        self.pii_category
    }
}

impl NodePropertyMeta for (String, PiiCategory) {
    fn property_name(&self) -> &str {
        &self.0
    }
    fn property_pii(&self) -> PiiCategory {
        self.1
    }
}

impl NodePropertyMeta for (&str, PiiCategory) {
    fn property_name(&self) -> &str {
        self.0
    }
    fn property_pii(&self) -> PiiCategory {
        self.1
    }
}

/// Input parameters for [`resolve_node_type_policy`] (`§13.6.3`, `§13.7`).
#[derive(Debug, Clone)]
pub struct NodeTypePolicyInput<'a, P = PropertySpec> {
    /// Caller principal profile.
    pub profile: &'a GuardPrincipalProfile,
    /// Cycle-safe department ancestor lineage `[dept_id, parent, ...]`.
    pub dept_lineage: &'a [String],
    /// Business node `type_name`.
    pub type_name: &'a str,
    /// Node type property metadata slice.
    pub properties: &'a [P],
    /// Tenant `sec_row_policies` records.
    pub row_policies: &'a [RowPolicyRecord],
    /// Node-type `default_deny` toggle from `sec_row_policy_defaults`.
    pub default_deny: bool,
    /// Tenant `sec_column_policies` records.
    pub column_policies: &'a [ColumnPolicyRecord],
    /// Active `sec_grants` records.
    pub active_grants: &'a [GrantRecord],
    /// Whether caller explicitly requested `reveal_pii: true`.
    pub reveal_pii: bool,
}

/// Resolves the effective [`NodeTypePolicySpec`] and `(spec, any_exempt_matched)` for an
/// `xBlueStudio` node type (`§13.6.3`, `§13.7`, `05-STEP27` §27.3).
///
/// # Errors
/// - Returns `AppError::Forbidden("PERMISSION_DENIED")` if `input.reveal_pii` is `true` without
///   `Permission::PiiRawRead` or matching `exempt` selector.
/// - Returns `AppError::BadRequest` (`INVALID_PREDICATE`) if a matching row predicate is invalid.
pub fn resolve_node_type_policy<P: NodePropertyMeta>(
    input: &NodeTypePolicyInput<'_, P>,
) -> Result<(NodeTypePolicySpec, bool), AppError> {
    let allowed_cols: BTreeSet<String> = input
        .properties
        .iter()
        .map(|p| p.property_name().to_ascii_lowercase())
        .collect();
    let allowed_opt = (!allowed_cols.is_empty()).then_some(&allowed_cols);

    let bypass_rows = input.profile.permissions.contains(&Permission::DataAdmin)
        || input.profile.permissions.contains(&Permission::GuardAdmin);

    let (duckdb_row_filter, pg_row_filter) = if bypass_rows {
        (None, None)
    } else {
        collect_node_type_filters(input, allowed_opt)?
    };

    let prop_pairs: Vec<(&str, PiiCategory)> = input
        .properties
        .iter()
        .map(|p| (p.property_name(), p.property_pii()))
        .collect();
    let (property_masks, any_exempt_matched) = resolve_column_masks_ctx(&ColumnMaskResolveCtx {
        target_kind: PolicyTargetKind::NodeType,
        primary_ref: input.type_name,
        secondary_ref: input.type_name,
        columns: &prop_pairs,
        column_policies: input.column_policies,
        profile: input.profile,
        dept_lineage: input.dept_lineage,
        reveal_pii: input.reveal_pii,
    })?;

    Ok((
        NodeTypePolicySpec {
            duckdb_row_filter,
            pg_row_filter,
            property_masks,
        },
        any_exempt_matched,
    ))
}

fn collect_node_type_filters<P: NodePropertyMeta>(
    input: &NodeTypePolicyInput<'_, P>,
    allowed_opt: Option<&BTreeSet<String>>,
) -> Result<(Option<String>, Option<String>), AppError> {
    let mut duck_branches = Vec::new();
    let mut pg_branches = Vec::new();
    for rp in input.row_policies {
        if !rp.enabled || rp.target_kind != PolicyTargetKind::NodeType {
            continue;
        }
        if !rp.target_ref.eq_ignore_ascii_case(input.type_name) {
            continue;
        }
        if rp.applies_to.matches_applies_to(
            input.profile.user_id,
            input.dept_lineage,
            &input.profile.roles,
        ) {
            let compiled = compile_predicate_with_ttl(
                &rp.predicate,
                PolicyTargetKind::NodeType,
                allowed_opt,
                input.profile,
                rp.ttl_timestamp_field.as_deref(),
                rp.ttl_max_age_hours,
            )?;
            duck_branches.push(compiled.duckdb_sql);
            pg_branches.push(compiled.pg_sql);
        }
    }
    for grant in input.active_grants {
        if !grant_matches_subject_and_resource(
            grant,
            GrantResourceKind::NodeType,
            input.type_name,
            input.type_name,
            input.profile,
            input.dept_lineage,
        ) {
            continue;
        }
        if let Some(ref pred) = grant.row_predicate
            && !pred.trim().is_empty()
        {
            let compiled =
                compile_predicate(pred, PolicyTargetKind::NodeType, allowed_opt, input.profile)?;
            duck_branches.push(compiled.duckdb_sql);
            pg_branches.push(compiled.pg_sql);
        }
    }
    Ok((
        combine_or_predicates(&duck_branches, input.default_deny),
        combine_or_predicates(&pg_branches, input.default_deny),
    ))
}

struct ColumnMaskResolveCtx<'a> {
    target_kind: PolicyTargetKind,
    primary_ref: &'a str,
    secondary_ref: &'a str,
    columns: &'a [(&'a str, PiiCategory)],
    column_policies: &'a [ColumnPolicyRecord],
    profile: &'a GuardPrincipalProfile,
    dept_lineage: &'a [String],
    reveal_pii: bool,
}

fn resolve_column_masks_ctx(
    ctx: &ColumnMaskResolveCtx<'_>,
) -> Result<(BTreeMap<String, MaskRule>, bool), AppError> {
    let has_global_pii_raw = ctx.profile.permissions.contains(&Permission::PiiRawRead);
    let mut explicit_by_col: BTreeMap<String, &ColumnPolicyRecord> = BTreeMap::new();
    for cp in ctx.column_policies {
        if cp.target_kind == ctx.target_kind
            && matches_target_ref(&cp.target_ref, ctx.primary_ref, ctx.secondary_ref)
        {
            explicit_by_col.insert(cp.column_name.to_ascii_lowercase(), cp);
        }
    }

    let mut masks = BTreeMap::new();
    let mut any_exempt_matched = false;

    for &(col_name, pii_cat) in ctx.columns {
        let lower_col = col_name.to_ascii_lowercase();
        let explicit = explicit_by_col.remove(&lower_col);
        let rule = explicit.map_or_else(|| MaskRule::from_pii_category(pii_cat), |cp| cp.mask_rule);
        if rule == MaskRule::None {
            continue;
        }
        let is_col_exempt = explicit.is_some_and(|cp| {
            cp.exempt
                .matches_explicit(ctx.profile.user_id, ctx.dept_lineage, &ctx.profile.roles)
        });
        if is_col_exempt {
            any_exempt_matched = true;
        }
        if ctx.reveal_pii && (has_global_pii_raw || is_col_exempt) {
            continue;
        }
        let key = if ctx.target_kind == PolicyTargetKind::Dataset {
            lower_col
        } else {
            String::from(col_name)
        };
        masks.insert(key, rule);
    }

    for (lower_col, cp) in explicit_by_col {
        if cp.mask_rule == MaskRule::None {
            continue;
        }
        let is_col_exempt =
            cp.exempt
                .matches_explicit(ctx.profile.user_id, ctx.dept_lineage, &ctx.profile.roles);
        if is_col_exempt {
            any_exempt_matched = true;
        }
        if ctx.reveal_pii && (has_global_pii_raw || is_col_exempt) {
            continue;
        }
        let key = if ctx.target_kind == PolicyTargetKind::Dataset {
            lower_col
        } else {
            cp.column_name.clone()
        };
        masks.insert(key, cp.mask_rule);
    }

    if ctx.reveal_pii && !has_global_pii_raw && !any_exempt_matched {
        return Err(AppError::Forbidden("PERMISSION_DENIED"));
    }

    Ok((masks, any_exempt_matched))
}

fn matches_target_ref(policy_ref: &str, primary_ref: &str, secondary_ref: &str) -> bool {
    policy_ref.eq_ignore_ascii_case(primary_ref) || policy_ref.eq_ignore_ascii_case(secondary_ref)
}

fn grant_matches_subject_and_resource(
    grant: &GrantRecord,
    expected_kind: GrantResourceKind,
    primary_ref: &str,
    secondary_ref: &str,
    profile: &GuardPrincipalProfile,
    dept_lineage: &[String],
) -> bool {
    if grant.status != GrantStatus::Active || grant.expires_at <= Utc::now() {
        return false;
    }
    if grant.resource_kind != expected_kind
        || !matches_target_ref(&grant.resource_ref, primary_ref, secondary_ref)
    {
        return false;
    }
    match grant.grantee_kind {
        GranteeKind::User => grant
            .grantee_ref
            .eq_ignore_ascii_case(&profile.user_id.to_string()),
        GranteeKind::Dept => dept_lineage
            .iter()
            .any(|d| d.eq_ignore_ascii_case(&grant.grantee_ref)),
        GranteeKind::Role => profile
            .roles
            .iter()
            .any(|r| r.eq_ignore_ascii_case(&grant.grantee_ref)),
    }
}

fn combine_or_predicates(branches: &[String], default_deny: bool) -> Option<String> {
    if branches.is_empty() {
        return default_deny.then(|| String::from("FALSE"));
    }
    if let [single] = branches {
        return Some(single.clone());
    }
    let wrapped: Vec<String> = branches.iter().map(|b| format!("({b})")).collect();
    Some(format!("({})", wrapped.join(" OR ")))
}
