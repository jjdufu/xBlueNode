//! Row-level security (`RLS`) predicate `AST` validator, dual-dialect (`DuckDB` / `PostgreSQL`)
//! compiler, and dataset / node-type policy resolvers (`11-STEP13` §13.6.2, §13.6.3).

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
use sqlparser::ast::{BinaryOperator, Expr, Statement, UnaryOperator, Value};
use sqlparser::dialect::GenericDialect;
use sqlparser::parser::Parser;
use uuid::Uuid;

const PLACEHOLDER_USER_ID: &str = "__ax_ph_user_id__";
const PLACEHOLDER_USER_NAME: &str = "__ax_ph_user_username__";
const PLACEHOLDER_USER_DEPT: &str = "__ax_ph_user_dept__";
const PLACEHOLDER_ATTR_PREFIX: &str = "__ax_ph_user_attr_";
const PLACEHOLDER_SUFFIX: &str = "__";

/// Dual-dialect boolean SQL predicate compiled from a validated row policy expression (`§13.6.2`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompiledPredicate {
    /// Compiled `DuckDB` SQL predicate expression (`lake."tenant_x"."t"` or `ax.nodes`).
    pub duckdb_sql: String,
    /// Compiled `PostgreSQL` SQL predicate expression (`xb_nodes` or relational tables).
    pub pg_sql: String,
}

#[derive(Clone, Copy)]
enum PlaceholderMode<'a> {
    SyntaxValidationOnly,
    BindProfile(&'a GuardPrincipalProfile),
}

enum PlaceholderKind {
    UserId,
    Username,
    Dept,
    Attr(String),
}

/// Validates a row-policy `predicate` against the `§13.6.2` `EBNF` grammar and optional
/// `allowed_columns` whitelist without requiring a runtime user profile.
///
/// # Errors
/// Returns `AppError::BadRequest` (`INVALID_PREDICATE`) if `predicate` contains comments,
/// semicolons, unknown `{{...}}` placeholders, disallowed `AST` constructs, `{{user.attr.*}}`
/// outside `IN`, or columns absent from `allowed_columns`.
pub fn validate_predicate_syntax(
    predicate: &str,
    allowed_columns: Option<&BTreeSet<String>>,
) -> Result<(), AppError> {
    let ast = parse_predicate_expr(predicate)?;
    let _compiled = compile_expr(
        &ast,
        PolicyTargetKind::Dataset,
        allowed_columns,
        PlaceholderMode::SyntaxValidationOnly,
    )?;
    Ok(())
}

/// Validates and compiles `predicate` for `target_kind` (`DATASET` or `NODE_TYPE`), substituting
/// `profile` attributes and enforcing `allowed_columns` (`§13.6.2`).
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
                let compiled = compile_predicate(
                    &rp.predicate,
                    PolicyTargetKind::Dataset,
                    allowed_opt,
                    input.profile,
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
    /// Property identifier (`xb_nodes.properties` key).
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
/// `xBlueStudio` node type (`§13.6.3`, `§13.7`).
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
                let compiled = compile_predicate(
                    &rp.predicate,
                    PolicyTargetKind::NodeType,
                    allowed_opt,
                    input.profile,
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
                let compiled = compile_predicate(
                    pred,
                    PolicyTargetKind::NodeType,
                    allowed_opt,
                    input.profile,
                )?;
                duck_branches.push(compiled.duckdb_sql);
                pg_branches.push(compiled.pg_sql);
            }
        }
        (
            combine_or_predicates(&duck_branches, input.default_deny),
            combine_or_predicates(&pg_branches, input.default_deny),
        )
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

    // Also include explicit column policies on columns not present in `ctx.columns` metadata.
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

fn invalid_pred(msg: impl Into<String>) -> AppError {
    AppError::bad("INVALID_PREDICATE", msg.into())
}

fn parse_predicate_expr(predicate: &str) -> Result<Expr, AppError> {
    let trimmed = predicate.trim();
    if trimmed.is_empty() {
        return Err(invalid_pred("row policy predicate cannot be empty"));
    }
    if trimmed.len() > 8_192 {
        return Err(invalid_pred("row policy predicate exceeds 8 KiB"));
    }
    let preprocessed = preprocess_placeholders_and_scan_hazards(trimmed)?;
    let wrapper_sql = format!("SELECT 1 WHERE {preprocessed}");
    let stmts = Parser::parse_sql(&GenericDialect {}, &wrapper_sql)
        .map_err(|err| invalid_pred(format!("predicate syntax error: {err}")))?;
    let [Statement::Query(query)] = stmts.as_slice() else {
        return Err(invalid_pred(
            "predicate must parse as a single WHERE expression",
        ));
    };
    let sqlparser::ast::SetExpr::Select(select) = query.body.as_ref() else {
        return Err(invalid_pred("invalid predicate SELECT wrapper"));
    };
    select
        .selection
        .clone()
        .ok_or_else(|| invalid_pred("missing predicate expression"))
}

fn preprocess_placeholders_and_scan_hazards(raw: &str) -> Result<String, AppError> {
    if raw.contains('\0') {
        return Err(invalid_pred("NUL byte in predicate is forbidden"));
    }
    let chars: Vec<char> = raw.chars().collect();
    let mut out = String::with_capacity(raw.len() + 32);
    let mut idx = 0_usize;

    while let Some(&ch) = chars.get(idx) {
        if ch == '\'' {
            out.push('\'');
            idx = idx.saturating_add(1);
            let mut closed = false;
            while let Some(&c) = chars.get(idx) {
                out.push(c);
                idx = idx.saturating_add(1);
                if c == '\'' {
                    if chars.get(idx) == Some(&'\'') {
                        out.push('\'');
                        idx = idx.saturating_add(1);
                    } else {
                        closed = true;
                        break;
                    }
                }
            }
            if !closed {
                return Err(invalid_pred("unclosed single-quoted string literal"));
            }
            continue;
        }

        if ch == ';' {
            return Err(invalid_pred("semicolons are forbidden in row predicates"));
        }
        if ch == '-' && chars.get(idx.saturating_add(1)) == Some(&'-') {
            return Err(invalid_pred(
                "SQL line comments are forbidden in row predicates",
            ));
        }
        if ch == '/' && chars.get(idx.saturating_add(1)) == Some(&'*') {
            return Err(invalid_pred(
                "SQL block comments are forbidden in row predicates",
            ));
        }

        if ch == '{' && chars.get(idx.saturating_add(1)) == Some(&'{') {
            let (token_ident, next_idx) = extract_placeholder(&chars, idx.saturating_add(2))?;
            // On `IN {{user.attr.k}}`, sqlparser expects parenthesized `IN (ident)`!
            let prev_word = previous_upper_keyword(&out);
            if prev_word == "IN" {
                out.push('(');
                out.push_str(&token_ident);
                out.push(')');
            } else {
                out.push_str(&token_ident);
            }
            idx = next_idx;
            continue;
        }
        if ch == '}' && chars.get(idx.saturating_add(1)) == Some(&'}') {
            return Err(invalid_pred("unmatched '}}' in row predicate"));
        }

        out.push(ch);
        idx = idx.saturating_add(1);
    }

    Ok(out)
}

fn previous_upper_keyword(prefix: &str) -> String {
    let trimmed = prefix.trim_end();
    let word: String = trimmed
        .chars()
        .rev()
        .take_while(char::is_ascii_alphabetic)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    word.to_ascii_uppercase()
}

fn extract_placeholder(chars: &[char], start: usize) -> Result<(String, usize), AppError> {
    let mut cursor = start;
    let mut body = String::new();
    while let Some(&c) = chars.get(cursor) {
        if c == '}' && chars.get(cursor.saturating_add(1)) == Some(&'}') {
            let token = validate_and_encode_placeholder(body.trim())?;
            return Ok((token, cursor.saturating_add(2)));
        }
        body.push(c);
        cursor = cursor.saturating_add(1);
    }
    Err(invalid_pred("unclosed '{{' placeholder in row predicate"))
}

fn validate_and_encode_placeholder(spec: &str) -> Result<String, AppError> {
    match spec {
        "user.id" => Ok(String::from(PLACEHOLDER_USER_ID)),
        "user.username" => Ok(String::from(PLACEHOLDER_USER_NAME)),
        "user.dept" | "user.dept_id" => Ok(String::from(PLACEHOLDER_USER_DEPT)),
        other => {
            let Some(attr_key) = other.strip_prefix("user.attr.") else {
                return Err(invalid_pred(format!(
                    "unknown placeholder '{{{{{other}}}}}'"
                )));
            };
            if attr_key.is_empty()
                || !attr_key
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_')
            {
                return Err(invalid_pred(format!(
                    "invalid user attribute placeholder '{{{{{other}}}}}'"
                )));
            }
            Ok(format!(
                "{PLACEHOLDER_ATTR_PREFIX}{attr_key}{PLACEHOLDER_SUFFIX}"
            ))
        }
    }
}

fn decode_placeholder(ident: &str) -> Option<PlaceholderKind> {
    if ident == PLACEHOLDER_USER_ID {
        return Some(PlaceholderKind::UserId);
    }
    if ident == PLACEHOLDER_USER_NAME {
        return Some(PlaceholderKind::Username);
    }
    if ident == PLACEHOLDER_USER_DEPT {
        return Some(PlaceholderKind::Dept);
    }
    let rest = ident.strip_prefix(PLACEHOLDER_ATTR_PREFIX)?;
    let key = rest.strip_suffix(PLACEHOLDER_SUFFIX)?;
    if key.is_empty() {
        return None;
    }
    Some(PlaceholderKind::Attr(String::from(key)))
}

fn compile_expr(
    expr: &Expr,
    target_kind: PolicyTargetKind,
    allowed_columns: Option<&BTreeSet<String>>,
    mode: PlaceholderMode<'_>,
) -> Result<CompiledPredicate, AppError> {
    match expr {
        Expr::Nested(inner) => {
            let inner_compiled = compile_expr(inner, target_kind, allowed_columns, mode)?;
            Ok(CompiledPredicate {
                duckdb_sql: wrap_if_unparenthesized(inner_compiled.duckdb_sql),
                pg_sql: wrap_if_unparenthesized(inner_compiled.pg_sql),
            })
        }
        Expr::UnaryOp {
            op: UnaryOperator::Not,
            expr: inner,
        } => {
            let sub = compile_expr(inner, target_kind, allowed_columns, mode)?;
            Ok(CompiledPredicate {
                duckdb_sql: format!("NOT {}", sub.duckdb_sql),
                pg_sql: format!("NOT {}", sub.pg_sql),
            })
        }
        Expr::BinaryOp { left, op, right } => match op {
            BinaryOperator::And | BinaryOperator::Or => {
                let l = compile_expr(left, target_kind, allowed_columns, mode)?;
                let r = compile_expr(right, target_kind, allowed_columns, mode)?;
                let kw = if *op == BinaryOperator::And {
                    "AND"
                } else {
                    "OR"
                };
                Ok(CompiledPredicate {
                    duckdb_sql: format!("({} {kw} {})", l.duckdb_sql, r.duckdb_sql),
                    pg_sql: format!("({} {kw} {})", l.pg_sql, r.pg_sql),
                })
            }
            BinaryOperator::Eq
            | BinaryOperator::NotEq
            | BinaryOperator::Lt
            | BinaryOperator::LtEq
            | BinaryOperator::Gt
            | BinaryOperator::GtEq => {
                let l = compile_scalar_operand(left, target_kind, allowed_columns, mode)?;
                let r = compile_scalar_operand(right, target_kind, allowed_columns, mode)?;
                let op_str = match op {
                    BinaryOperator::Eq => "=",
                    BinaryOperator::NotEq => "<>",
                    BinaryOperator::Lt => "<",
                    BinaryOperator::LtEq => "<=",
                    BinaryOperator::Gt => ">",
                    _ => ">=",
                };
                Ok(CompiledPredicate {
                    duckdb_sql: format!("{} {op_str} {}", l.duckdb_sql, r.duckdb_sql),
                    pg_sql: format!("{} {op_str} {}", l.pg_sql, r.pg_sql),
                })
            }
            _ => Err(invalid_pred(format!(
                "operator '{op}' is forbidden in row predicates"
            ))),
        },
        Expr::IsNull(inner) => {
            let opd = compile_scalar_operand(inner, target_kind, allowed_columns, mode)?;
            Ok(CompiledPredicate {
                duckdb_sql: format!("{} IS NULL", opd.duckdb_sql),
                pg_sql: format!("{} IS NULL", opd.pg_sql),
            })
        }
        Expr::IsNotNull(inner) => {
            let opd = compile_scalar_operand(inner, target_kind, allowed_columns, mode)?;
            Ok(CompiledPredicate {
                duckdb_sql: format!("{} IS NOT NULL", opd.duckdb_sql),
                pg_sql: format!("{} IS NOT NULL", opd.pg_sql),
            })
        }
        Expr::Like {
            negated,
            any: false,
            expr: target,
            pattern,
            escape_char: None,
        } => {
            let lhs = compile_scalar_operand(target, target_kind, allowed_columns, mode)?;
            let rhs = compile_string_literal_only(pattern)?;
            let kw = if *negated { "NOT LIKE" } else { "LIKE" };
            Ok(CompiledPredicate {
                duckdb_sql: format!("{} {kw} {rhs}", lhs.duckdb_sql),
                pg_sql: format!("{} {kw} {rhs}", lhs.pg_sql),
            })
        }
        Expr::In {
            expr: lhs_expr,
            target: sqlparser::ast::InTarget::List(list),
            negated,
            ..
        } => compile_in_list(lhs_expr, list, *negated, target_kind, allowed_columns, mode),
        _ => Err(invalid_pred(
            "unsupported SQL construct in row predicate (functions, subqueries, and CASE are forbidden)",
        )),
    }
}

fn compile_in_list(
    lhs_expr: &Expr,
    list: &[Expr],
    negated: bool,
    target_kind: PolicyTargetKind,
    allowed_columns: Option<&BTreeSet<String>>,
    mode: PlaceholderMode<'_>,
) -> Result<CompiledPredicate, AppError> {
    let lhs = compile_scalar_operand(lhs_expr, target_kind, allowed_columns, mode)?;
    if list.is_empty() {
        let empty_bool_sql = if negated { "TRUE" } else { "FALSE" };
        return Ok(CompiledPredicate {
            duckdb_sql: String::from(empty_bool_sql),
            pg_sql: String::from(empty_bool_sql),
        });
    }

    if let [Expr::Identifier(single_ident)] = list
        && let Some(PlaceholderKind::Attr(attr_key)) = decode_placeholder(&single_ident.value)
    {
        return Ok(compile_in_attr_placeholder(&lhs, &attr_key, negated, mode));
    }

    let mut duck_items = Vec::with_capacity(list.len());
    let mut pg_items = Vec::with_capacity(list.len());
    for item in list {
        let compiled = compile_scalar_operand(item, target_kind, allowed_columns, mode)?;
        duck_items.push(compiled.duckdb_sql);
        pg_items.push(compiled.pg_sql);
    }
    let kw = if negated { "NOT IN" } else { "IN" };
    Ok(CompiledPredicate {
        duckdb_sql: format!("{} {kw} ({})", lhs.duckdb_sql, duck_items.join(", ")),
        pg_sql: format!("{} {kw} ({})", lhs.pg_sql, pg_items.join(", ")),
    })
}

fn compile_in_attr_placeholder(
    lhs: &CompiledPredicate,
    attr_key: &str,
    negated: bool,
    mode: PlaceholderMode<'_>,
) -> CompiledPredicate {
    let kw = if negated { "NOT IN" } else { "IN" };
    match mode {
        PlaceholderMode::SyntaxValidationOnly => CompiledPredicate {
            duckdb_sql: format!("{} {kw} ('__validation__')", lhs.duckdb_sql),
            pg_sql: format!("{} {kw} ('__validation__')", lhs.pg_sql),
        },
        PlaceholderMode::BindProfile(profile) => {
            let vals = profile
                .attributes
                .get(attr_key)
                .map_or(&[][..], Vec::as_slice);
            if vals.is_empty() {
                let constant = if negated { "TRUE" } else { "FALSE" };
                return CompiledPredicate {
                    duckdb_sql: String::from(constant),
                    pg_sql: String::from(constant),
                };
            }
            let quoted: Vec<String> = vals.iter().map(|v| quote_sql_string(v)).collect();
            let joined = quoted.join(", ");
            CompiledPredicate {
                duckdb_sql: format!("{} {kw} ({joined})", lhs.duckdb_sql),
                pg_sql: format!("{} {kw} ({joined})", lhs.pg_sql),
            }
        }
    }
}

fn compile_scalar_operand(
    expr: &Expr,
    target_kind: PolicyTargetKind,
    allowed_columns: Option<&BTreeSet<String>>,
    mode: PlaceholderMode<'_>,
) -> Result<CompiledPredicate, AppError> {
    match expr {
        Expr::Identifier(ident) => {
            if let Some(ph) = decode_placeholder(&ident.value) {
                return compile_scalar_placeholder(&ph, mode);
            }
            compile_column_ident(&ident.value, target_kind, allowed_columns)
        }
        Expr::Value(val_span) => compile_literal_value(&val_span.value),
        Expr::UnaryOp {
            op: UnaryOperator::Minus,
            expr: inner,
        } => {
            if let Expr::Value(val_span) = inner.as_ref()
                && let Value::Number(num_str, false) = &val_span.value
            {
                let lit = format!("-{num_str}");
                return Ok(CompiledPredicate {
                    duckdb_sql: lit.clone(),
                    pg_sql: lit,
                });
            }
            Err(invalid_pred(
                "unary minus is only permitted on numeric literals",
            ))
        }
        _ => Err(invalid_pred(
            "invalid operand in row predicate (table-qualified columns, functions, and expressions are forbidden)",
        )),
    }
}

fn compile_scalar_placeholder(
    ph: &PlaceholderKind,
    mode: PlaceholderMode<'_>,
) -> Result<CompiledPredicate, AppError> {
    match ph {
        PlaceholderKind::Attr(key) => Err(invalid_pred(format!(
            "list placeholder '{{{{user.attr.{key}}}}}' is only allowed on the right-hand side of IN"
        ))),
        PlaceholderKind::UserId => {
            let raw = match mode {
                PlaceholderMode::SyntaxValidationOnly => {
                    String::from("00000000-0000-0000-0000-000000000000")
                }
                PlaceholderMode::BindProfile(p) => p.user_id.to_string(),
            };
            let lit = quote_sql_string(&raw);
            Ok(CompiledPredicate {
                duckdb_sql: lit.clone(),
                pg_sql: lit,
            })
        }
        PlaceholderKind::Username => {
            let raw = match mode {
                PlaceholderMode::SyntaxValidationOnly => "__user__",
                PlaceholderMode::BindProfile(p) => p.username.as_str(),
            };
            let lit = quote_sql_string(raw);
            Ok(CompiledPredicate {
                duckdb_sql: lit.clone(),
                pg_sql: lit,
            })
        }
        PlaceholderKind::Dept => {
            let raw = match mode {
                PlaceholderMode::SyntaxValidationOnly => "default",
                PlaceholderMode::BindProfile(p) => p.dept_id.as_str(),
            };
            let lit = quote_sql_string(raw);
            Ok(CompiledPredicate {
                duckdb_sql: lit.clone(),
                pg_sql: lit,
            })
        }
    }
}

fn compile_column_ident(
    col_raw: &str,
    target_kind: PolicyTargetKind,
    allowed_columns: Option<&BTreeSet<String>>,
) -> Result<CompiledPredicate, AppError> {
    let trimmed = col_raw.trim();
    if trimmed.is_empty()
        || !trimmed
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return Err(invalid_pred(format!(
            "invalid column identifier '{trimmed}'"
        )));
    }

    let lower = trimmed.to_ascii_lowercase();
    let is_core_node_col = target_kind == PolicyTargetKind::NodeType
        && matches!(lower.as_str(), "node_key" | "title" | "type_name");

    if let Some(whitelist) = allowed_columns
        && !is_core_node_col
        && !whitelist.iter().any(|c| c.eq_ignore_ascii_case(trimmed))
    {
        return Err(invalid_pred(format!(
            "column '{trimmed}' does not exist on target resource"
        )));
    }

    match target_kind {
        PolicyTargetKind::Dataset => {
            let quoted = format!("\"{}\"", trimmed.replace('"', "\"\""));
            Ok(CompiledPredicate {
                duckdb_sql: quoted.clone(),
                pg_sql: quoted,
            })
        }
        PolicyTargetKind::NodeType => {
            if is_core_node_col {
                Ok(CompiledPredicate {
                    duckdb_sql: format!("\"{lower}\""),
                    pg_sql: lower,
                })
            } else {
                let escaped_key = trimmed.replace('\'', "''");
                Ok(CompiledPredicate {
                    duckdb_sql: format!("json_extract_string(properties, '$.{escaped_key}')"),
                    pg_sql: format!("(properties->>'{escaped_key}')"),
                })
            }
        }
    }
}

fn compile_literal_value(val: &Value) -> Result<CompiledPredicate, AppError> {
    let lit = match val {
        Value::SingleQuotedString(s) => quote_sql_string(s),
        Value::Number(n, false) => {
            if !n.chars().all(|c| c.is_ascii_digit() || c == '.') {
                return Err(invalid_pred(format!("invalid numeric literal '{n}'")));
            }
            n.clone()
        }
        Value::Boolean(b) => {
            if *b {
                String::from("TRUE")
            } else {
                String::from("FALSE")
            }
        }
        _ => {
            return Err(invalid_pred(
                "only single-quoted strings, numbers, and booleans are allowed as literals",
            ));
        }
    };
    Ok(CompiledPredicate {
        duckdb_sql: lit.clone(),
        pg_sql: lit,
    })
}

fn compile_string_literal_only(expr: &Expr) -> Result<String, AppError> {
    if let Expr::Value(val_span) = expr
        && let Value::SingleQuotedString(s) = &val_span.value
    {
        return Ok(quote_sql_string(s));
    }
    Err(invalid_pred("LIKE pattern must be a string literal"))
}

fn quote_sql_string(raw: &str) -> String {
    format!("'{}'", raw.replace('\'', "''"))
}

fn wrap_if_unparenthesized(s: String) -> String {
    if s.starts_with('(') && s.ends_with(')') {
        s
    } else {
        format!("({s})")
    }
}
