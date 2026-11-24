//! `FROM OBJECT(<MainType>) <alias>` ontology symbol resolution, cross-tier 1:1 folding,
//! and dual `DuckLake` / `ax.nodes` CTE compilation (`05-STEP27` §27.2.1).

use std::collections::BTreeMap;

use axpoint_domain::AppError;
use axpoint_domain::enums::{AggFn, DataType, MaskRule, PiiCategory, SemanticLayerTier};
use axpoint_domain::guard::PolicyContext;
use axpoint_domain::studio::{
    LinkDirection, MappingSpec, NodeTypeSpec, PropertyKind, PropertySpec, SchemaDoc,
};
use sqlparser::ast::Cte;

use crate::axql::fanout_guard::parse_cte;
use crate::axql::parser::bad_axql;
use crate::sql_guard::TenantCatalog;

pub(crate) const BUILTIN_NODE_FIELDS: &[&str] =
    &["id", "node_key", "title", "type_name", "updated_at"];

/// Resolved field classification on an `OBJECT(<MainType>)` projection.
#[derive(Debug, Clone)]
pub enum ObjectFieldBinding {
    /// Built-in node identity / metadata column (`id`, `node_key`, `title`, `type_name`, `updated_at`).
    Builtin(String),
    /// Property declared directly on `<MainType>`.
    DirectProperty(PropertySpec),
    /// Folded 1-hop child metric from a `STATIC_FACT` <-> `EVENT_DYNAMIC`/`CUBE_ROLLUP` neighbor or `AggRuleSpec`.
    FoldedChildMetric {
        /// Output field name on `<MainType>`.
        field_name: String,
        /// Aggregation function (`SUM`, `AVG`, `COUNT`, `MIN`, `MAX`).
        agg_op: AggFn,
        /// Connecting `EdgeTypeSpec.name`.
        edge_type: String,
        /// Direction relative to `<MainType>` (`Out` = `Main -> Child`, `In` = `Child -> Main`).
        direction: LinkDirection,
        /// Neighbor `NodeTypeSpec.name`.
        child_type: String,
        /// Source metric property name on `child_type`.
        child_metric: String,
    },
}

impl ObjectFieldBinding {
    /// Canonical field name exposed on the object alias.
    #[must_use]
    pub fn canonical_name(&self) -> &str {
        match self {
            Self::Builtin(n) => n,
            Self::DirectProperty(p) => &p.name,
            Self::FoldedChildMetric { field_name, .. } => field_name,
        }
    }
}

/// Output of compiling `FROM OBJECT(<MainType>) <alias>` into CTEs.
#[derive(Debug)]
pub struct ObjectCtePlan {
    /// Generated CTEs ending with `"__axql_base"`.
    pub ctes: Vec<Cte>,
    /// Referenced ontology node type names.
    pub referenced_node_types: Vec<String>,
    /// Referenced ontology edge type names (from folded 1-hop child tiers).
    pub referenced_edge_types: Vec<String>,
    /// Referenced dataset or `ax.*` relation names.
    pub referenced_relations: Vec<String>,
    /// `true` when 1:1 pre-aggregated CTE folding was applied across tiers.
    pub folded_child_tiers: bool,
}

/// Context passed into [`build_object_ctes`].
#[derive(Debug, Clone, Copy)]
pub struct ObjectBuildCtx<'a> {
    /// Target subgraph `SchemaDoc`.
    pub schema_doc: &'a SchemaDoc,
    /// Tenant `DuckLake` catalog.
    pub catalog: &'a TenantCatalog,
    /// Optional compiled `AxGuard` policy context.
    pub policy: Option<&'a PolicyContext>,
    /// Whether unmasked PII was explicitly requested and authorized.
    pub reveal_pii: bool,
}

struct FoldChildSpec<'a> {
    fold_idx: usize,
    agg_op: AggFn,
    edge_type: &'a str,
    direction: LinkDirection,
    child_type: &'a str,
    child_metric: &'a str,
}

/// Finds a `NodeTypeSpec` in `schema_doc` by name, returning `400 AXQL_UNMAPPED_SYMBOL` when absent.
///
/// # Errors
/// Returns `AppError::BadRequest` (`AXQL_UNMAPPED_SYMBOL`) if `type_name` is not in `schema_doc`.
pub fn lookup_node_type<'a>(
    schema_doc: &'a SchemaDoc,
    type_name: &str,
) -> Result<&'a NodeTypeSpec, AppError> {
    schema_doc
        .node_types
        .iter()
        .find(|nt| nt.name == type_name)
        .or_else(|| {
            schema_doc
                .node_types
                .iter()
                .find(|nt| nt.name.eq_ignore_ascii_case(type_name))
        })
        .ok_or_else(|| {
            bad_axql(
                "AXQL_UNMAPPED_SYMBOL",
                format!("AXQL_UNMAPPED_SYMBOL: unknown ontology node type '{type_name}'"),
            )
        })
}

/// Resolves a field referenced on `OBJECT(<MainType>)` against built-in fields, declared
/// `NodeTypeSpec.properties`, `AggRuleSpec`s, and 1-hop `EVENT_DYNAMIC`/`CUBE_ROLLUP` neighbors.
///
/// # Errors
/// Returns `AppError::BadRequest` (`AXQL_UNMAPPED_SYMBOL`) if `field` is undeclared or ambiguous.
pub fn resolve_object_field(
    schema_doc: &SchemaDoc,
    main_spec: &NodeTypeSpec,
    field: &str,
) -> Result<ObjectFieldBinding, AppError> {
    let lower = field.to_ascii_lowercase();
    if BUILTIN_NODE_FIELDS.contains(&lower.as_str()) {
        return Ok(ObjectFieldBinding::Builtin(lower));
    }
    if let Some(prop) = main_spec
        .properties
        .iter()
        .find(|p| p.name.eq_ignore_ascii_case(field))
    {
        return Ok(ObjectFieldBinding::DirectProperty(prop.clone()));
    }
    if let Some(rule) = main_spec
        .agg_rules
        .iter()
        .find(|r| r.target_metric.eq_ignore_ascii_case(field) && !r.via_link_type.is_empty())
    {
        return Ok(ObjectFieldBinding::FoldedChildMetric {
            field_name: rule.target_metric.clone(),
            agg_op: rule.op,
            edge_type: rule.via_link_type.clone(),
            direction: rule.direction,
            child_type: rule.source_type_name.clone(),
            child_metric: rule.source_metric.clone(),
        });
    }

    let candidates = find_foldable_1hop_metrics(schema_doc, main_spec, field);
    if let [single] = candidates.as_slice() {
        return Ok(single.clone());
    }
    if candidates.len() > 1 {
        return Err(bad_axql(
            "AXQL_UNMAPPED_SYMBOL",
            format!(
                "AXQL_UNMAPPED_SYMBOL: ambiguous cross-tier metric '{field}' on '{}'; use FROM MATCH to disambiguate",
                main_spec.name
            ),
        ));
    }
    Err(bad_axql(
        "AXQL_UNMAPPED_SYMBOL",
        format!(
            "AXQL_UNMAPPED_SYMBOL: property or metric '{field}' is not declared on '{}'",
            main_spec.name
        ),
    ))
}

fn find_foldable_1hop_metrics(
    schema_doc: &SchemaDoc,
    main_spec: &NodeTypeSpec,
    field: &str,
) -> Vec<ObjectFieldBinding> {
    let mut matches = Vec::new();
    for edge in &schema_doc.edge_types {
        let (direction, neighbor_name) = if edge.src_type_name == main_spec.name {
            (LinkDirection::Out, edge.dst_type_name.as_str())
        } else if edge.dst_type_name == main_spec.name {
            (LinkDirection::In, edge.src_type_name.as_str())
        } else {
            continue;
        };
        let Some(neighbor) = schema_doc
            .node_types
            .iter()
            .find(|nt| nt.name == neighbor_name)
        else {
            continue;
        };
        let is_foldable_tier = matches!(
            neighbor.semantic_layer_tier,
            Some(SemanticLayerTier::EventDynamic | SemanticLayerTier::CubeRollup)
        );
        if !is_foldable_tier {
            continue;
        }
        if let Some(prop) = neighbor
            .properties
            .iter()
            .find(|p| p.kind == PropertyKind::Metric && p.name.eq_ignore_ascii_case(field))
        {
            matches.push(ObjectFieldBinding::FoldedChildMetric {
                field_name: prop.name.clone(),
                agg_op: AggFn::Sum,
                edge_type: edge.name.clone(),
                direction,
                child_type: neighbor.name.clone(),
                child_metric: prop.name.clone(),
            });
        }
    }
    matches
}

/// Builds the CTE chain (`__axql_fold_*` + `__axql_base`) for `FROM OBJECT(<MainType>) <alias>`.
///
/// # Errors
/// Returns `AppError::BadRequest` if any mapped column or generated CTE is invalid.
pub fn build_object_ctes(
    ctx: ObjectBuildCtx<'_>,
    main_spec: &NodeTypeSpec,
    alias: &str,
    bindings: &BTreeMap<String, ObjectFieldBinding>,
) -> Result<ObjectCtePlan, AppError> {
    if let Some(mapped_plan) = try_build_mapped_dataset_object_ctes(
        ctx.schema_doc,
        ctx.catalog,
        main_spec,
        alias,
        bindings,
    )? {
        return Ok(mapped_plan);
    }
    build_ax_nodes_object_ctes(main_spec, alias, bindings, ctx.policy, ctx.reveal_pii)
}

fn try_build_mapped_dataset_object_ctes(
    schema_doc: &SchemaDoc,
    catalog: &TenantCatalog,
    main_spec: &NodeTypeSpec,
    alias: &str,
    bindings: &BTreeMap<String, ObjectFieldBinding>,
) -> Result<Option<ObjectCtePlan>, AppError> {
    let Some(main_map) = find_active_mapping(schema_doc, catalog, &main_spec.name) else {
        return Ok(None);
    };
    let mut ctes = Vec::new();
    let mut ref_nodes = vec![main_spec.name.clone()];
    let mut ref_edges = Vec::new();
    let mut ref_rels = vec![main_map.dataset.clone()];
    let mut join_clauses = Vec::new();
    let mut proj_cols = vec![format!(
        "CAST(\"__m\".\"{}\" AS VARCHAR) AS \"{alias}____entity_id\"",
        escape_ident(&main_map.key_column)
    )];
    let mut fold_idx = 0_usize;

    for (field_key, binding) in bindings {
        match binding {
            ObjectFieldBinding::Builtin(b) => {
                let expr = mapped_builtin_expr(b, main_map, &main_spec.name);
                proj_cols.push(format!("{expr} AS \"{alias}__{field_key}\""));
            }
            ObjectFieldBinding::DirectProperty(prop) => {
                let Some(col_name) = main_map
                    .attributes
                    .get(&prop.name)
                    .or_else(|| main_map.metrics.get(&prop.name))
                else {
                    return Ok(None);
                };
                proj_cols.push(format!(
                    "\"__m\".\"{}\" AS \"{alias}__{field_key}\"",
                    escape_ident(col_name)
                ));
            }
            ObjectFieldBinding::FoldedChildMetric {
                agg_op,
                edge_type,
                direction,
                child_type,
                child_metric,
                ..
            } => {
                let spec = FoldChildSpec {
                    fold_idx,
                    agg_op: *agg_op,
                    edge_type,
                    direction: *direction,
                    child_type,
                    child_metric,
                };
                let Some((fold_cte, join_sql, child_ds)) =
                    compile_mapped_fold_join(schema_doc, catalog, main_map, &spec)?
                else {
                    return Ok(None);
                };
                let fold_alias = format!("__fold_{fold_idx}");
                ctes.push(fold_cte);
                join_clauses.push(join_sql);
                proj_cols.push(format!(
                    "\"{fold_alias}\".\"__val\" AS \"{alias}__{field_key}\""
                ));
                push_unique(&mut ref_nodes, child_type);
                push_unique(&mut ref_edges, edge_type);
                push_unique(&mut ref_rels, &child_ds);
                fold_idx = fold_idx.saturating_add(1);
            }
        }
    }

    let filter_clause = main_map
        .filter
        .as_deref()
        .map(str::trim)
        .filter(|f| !f.is_empty())
        .map_or_else(String::new, |f| format!(" WHERE ({f})"));
    let base_sql = format!(
        "SELECT {} FROM \"{}\" AS \"__m\"{}{}",
        proj_cols.join(", "),
        escape_ident(&main_map.dataset),
        join_clauses.join(""),
        filter_clause
    );
    ctes.push(parse_cte("__axql_base", &base_sql)?);

    Ok(Some(ObjectCtePlan {
        ctes,
        referenced_node_types: ref_nodes,
        referenced_edge_types: ref_edges,
        referenced_relations: ref_rels,
        folded_child_tiers: fold_idx > 0,
    }))
}

fn compile_mapped_fold_join(
    schema_doc: &SchemaDoc,
    catalog: &TenantCatalog,
    main_map: &MappingSpec,
    spec: &FoldChildSpec<'_>,
) -> Result<Option<(Cte, String, String)>, AppError> {
    let Some(child_map) = find_active_mapping(schema_doc, catalog, spec.child_type) else {
        return Ok(None);
    };
    let metric_col = if spec.agg_op == AggFn::Count && spec.child_metric.is_empty() {
        &child_map.key_column
    } else if let Some(c) = child_map
        .metrics
        .get(spec.child_metric)
        .or_else(|| child_map.attributes.get(spec.child_metric))
    {
        c
    } else {
        return Ok(None);
    };

    let (main_join_col, child_group_col) = if let Some(link) = child_map
        .links
        .iter()
        .find(|l| l.link_type == spec.edge_type && l.target_type == main_map.node_type)
    {
        (
            main_map.key_column.as_str(),
            link.target_key_column.as_str(),
        )
    } else if let Some(link) = main_map
        .links
        .iter()
        .find(|l| l.link_type == spec.edge_type && l.target_type == child_map.node_type)
    {
        (
            link.target_key_column.as_str(),
            child_map.key_column.as_str(),
        )
    } else {
        return Ok(None);
    };

    let cte_name = format!("__axql_fold_{}", spec.fold_idx);
    let fold_alias = format!("__fold_{}", spec.fold_idx);
    let agg_expr = render_agg_sql(
        spec.agg_op,
        &format!("\"__c\".\"{}\"", escape_ident(metric_col)),
    );
    let child_filter = child_map
        .filter
        .as_deref()
        .map(str::trim)
        .filter(|f| !f.is_empty())
        .map_or_else(String::new, |f| format!(" WHERE ({f})"));
    let fold_sql = format!(
        "SELECT \"__c\".\"{cg}\" AS \"__join_key\", {agg_expr} AS \"__val\" FROM \"{ds}\" AS \"__c\"{child_filter} GROUP BY \"__c\".\"{cg}\"",
        cg = escape_ident(child_group_col),
        ds = escape_ident(&child_map.dataset),
    );
    let cte = parse_cte(&cte_name, &fold_sql)?;
    let join_sql = format!(
        " LEFT JOIN \"{cte_name}\" AS \"{fold_alias}\" ON \"__m\".\"{}\" = \"{fold_alias}\".\"__join_key\"",
        escape_ident(main_join_col)
    );
    Ok(Some((cte, join_sql, child_map.dataset.clone())))
}

fn build_ax_nodes_object_ctes(
    main_spec: &NodeTypeSpec,
    alias: &str,
    bindings: &BTreeMap<String, ObjectFieldBinding>,
    policy: Option<&PolicyContext>,
    reveal_pii: bool,
) -> Result<ObjectCtePlan, AppError> {
    let mut ctes = Vec::new();
    let mut ref_nodes = vec![main_spec.name.clone()];
    let mut ref_edges = Vec::new();
    let mut ref_rels = vec![String::from("ax.nodes")];
    let mut join_clauses = Vec::new();
    let mut proj_cols = vec![format!("\"__n\".\"id\" AS \"{alias}____entity_id\"")];
    let mut fold_idx = 0_usize;

    for (field_key, binding) in bindings {
        match binding {
            ObjectFieldBinding::Builtin(b) => {
                proj_cols.push(format!("\"__n\".\"{b}\" AS \"{alias}__{field_key}\""));
            }
            ObjectFieldBinding::DirectProperty(prop) => {
                let expr = ax_node_property_sql("__n", &main_spec.name, prop, policy, reveal_pii);
                proj_cols.push(format!("{expr} AS \"{alias}__{field_key}\""));
            }
            ObjectFieldBinding::FoldedChildMetric {
                agg_op,
                edge_type,
                direction,
                child_type,
                child_metric,
                ..
            } => {
                let spec = FoldChildSpec {
                    fold_idx,
                    agg_op: *agg_op,
                    edge_type,
                    direction: *direction,
                    child_type,
                    child_metric,
                };
                let (fold_cte, join_sql) = build_ax_nodes_fold_cte(&spec)?;
                let fold_alias = format!("__fold_{fold_idx}");
                ctes.push(fold_cte);
                join_clauses.push(join_sql);
                proj_cols.push(format!(
                    "\"{fold_alias}\".\"__val\" AS \"{alias}__{field_key}\""
                ));
                push_unique(&mut ref_nodes, child_type);
                push_unique(&mut ref_edges, edge_type);
                push_unique(&mut ref_rels, "ax.links");
                fold_idx = fold_idx.saturating_add(1);
            }
        }
    }

    let escaped_type = main_spec.name.replace('\'', "''");
    let base_sql = format!(
        "SELECT {} FROM ax.nodes AS \"__n\"{} WHERE \"__n\".\"type_name\" = '{escaped_type}'",
        proj_cols.join(", "),
        join_clauses.join("")
    );
    ctes.push(parse_cte("__axql_base", &base_sql)?);

    Ok(ObjectCtePlan {
        ctes,
        referenced_node_types: ref_nodes,
        referenced_edge_types: ref_edges,
        referenced_relations: ref_rels,
        folded_child_tiers: fold_idx > 0,
    })
}

fn build_ax_nodes_fold_cte(spec: &FoldChildSpec<'_>) -> Result<(Cte, String), AppError> {
    let cte_name = format!("__axql_fold_{}", spec.fold_idx);
    let fold_alias = format!("__fold_{}", spec.fold_idx);
    let (parent_fk, child_fk) = match spec.direction {
        LinkDirection::Out => ("src_node_id", "dst_node_id"),
        LinkDirection::In => ("dst_node_id", "src_node_id"),
    };
    let val_expr = if spec.agg_op == AggFn::Count && spec.child_metric.is_empty() {
        String::from("\"__c\".\"id\"")
    } else {
        let escaped_m = spec.child_metric.replace('\'', "''");
        format!("TRY_CAST(json_extract_string(\"__c\".\"metrics\", '$.{escaped_m}') AS DOUBLE)")
    };
    let agg_sql = render_agg_sql(spec.agg_op, &val_expr);
    let escaped_edge = spec.edge_type.replace('\'', "''");
    let escaped_child = spec.child_type.replace('\'', "''");
    let body = format!(
        "SELECT \"__l\".\"{parent_fk}\" AS \"__join_key\", {agg_sql} AS \"__val\" \
         FROM ax.links AS \"__l\" \
         INNER JOIN ax.nodes AS \"__c\" ON \"__c\".\"id\" = \"__l\".\"{child_fk}\" \
         WHERE \"__l\".\"link_type\" = '{escaped_edge}' AND \"__c\".\"type_name\" = '{escaped_child}' \
         GROUP BY \"__l\".\"{parent_fk}\""
    );
    let cte = parse_cte(&cte_name, &body)?;
    let join_sql = format!(
        " LEFT JOIN \"{cte_name}\" AS \"{fold_alias}\" ON \"__n\".\"id\" = \"{fold_alias}\".\"__join_key\""
    );
    Ok((cte, join_sql))
}

pub(crate) fn find_active_mapping<'a>(
    schema_doc: &'a SchemaDoc,
    catalog: &TenantCatalog,
    type_name: &str,
) -> Option<&'a MappingSpec> {
    schema_doc
        .mappings
        .iter()
        .find(|m| m.node_type == type_name && catalog.dataset(&m.dataset).is_some())
}

pub(crate) fn ax_node_property_sql(
    table_alias: &str,
    type_name: &str,
    prop: &PropertySpec,
    policy: Option<&PolicyContext>,
    reveal_pii: bool,
) -> String {
    let escaped_p = prop.name.replace('\'', "''");
    if prop.kind == PropertyKind::Metric {
        return format!(
            "TRY_CAST(json_extract_string(\"{table_alias}\".\"metrics\", '$.{escaped_p}') AS DOUBLE)"
        );
    }
    let raw_str = format!("json_extract_string(\"{table_alias}\".\"properties\", '$.{escaped_p}')");
    let mask_rule = resolve_effective_property_mask(type_name, prop, policy, reveal_pii);
    if let Some(udf) = mask_rule_udf(mask_rule) {
        return format!("{udf}({raw_str})");
    }
    match prop.data_type {
        DataType::Int64 => format!("TRY_CAST({raw_str} AS BIGINT)"),
        DataType::Float64 | DataType::Decimal => format!("TRY_CAST({raw_str} AS DOUBLE)"),
        DataType::Boolean => format!("TRY_CAST({raw_str} AS BOOLEAN)"),
        DataType::Date => format!("TRY_CAST({raw_str} AS DATE)"),
        DataType::Timestamp => format!("TRY_CAST({raw_str} AS TIMESTAMPTZ)"),
        _ => raw_str,
    }
}

fn resolve_effective_property_mask(
    type_name: &str,
    prop: &PropertySpec,
    policy: Option<&PolicyContext>,
    reveal_pii: bool,
) -> MaskRule {
    if reveal_pii {
        return MaskRule::None;
    }
    if let Some(pol) = policy
        && let Some(nt_pol) = pol.node_types.get(type_name).or_else(|| {
            pol.node_types
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(type_name))
                .map(|(_, v)| v)
        })
        && let Some(&rule) = nt_pol.property_masks.get(&prop.name).or_else(|| {
            nt_pol
                .property_masks
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(&prop.name))
                .map(|(_, v)| v)
        })
    {
        return rule;
    }
    default_mask_for_pii(prop.pii_category)
}

const fn default_mask_for_pii(cat: PiiCategory) -> MaskRule {
    match cat {
        PiiCategory::None => MaskRule::None,
        PiiCategory::Phone => MaskRule::MaskPhone,
        PiiCategory::IdCard => MaskRule::MaskIdCard,
        PiiCategory::Email => MaskRule::MaskEmail,
        PiiCategory::BankCard => MaskRule::MaskBankCard,
        PiiCategory::Name => MaskRule::MaskName,
        PiiCategory::Address => MaskRule::MaskAddress,
        PiiCategory::Secret => MaskRule::Redact,
    }
}

const fn mask_rule_udf(rule: MaskRule) -> Option<&'static str> {
    match rule {
        MaskRule::None => None,
        MaskRule::MaskPhone => Some("ax_mask_phone"),
        MaskRule::MaskIdCard => Some("ax_mask_id_card"),
        MaskRule::MaskEmail => Some("ax_mask_email"),
        MaskRule::MaskBankCard => Some("ax_mask_bank_card"),
        MaskRule::MaskName => Some("ax_mask_name"),
        MaskRule::MaskAddress => Some("ax_mask_address"),
        MaskRule::HmacHash => Some("ax_mask_hmac_hash"),
        MaskRule::Redact => Some("ax_mask_redact"),
    }
}

fn render_agg_sql(op: AggFn, expr: &str) -> String {
    match op {
        AggFn::Sum => format!("SUM({expr})"),
        AggFn::Avg => format!("AVG({expr})"),
        AggFn::Count => format!("COUNT({expr})"),
        AggFn::CountDistinct => format!("COUNT(DISTINCT {expr})"),
        AggFn::Min => format!("MIN({expr})"),
        AggFn::Max => format!("MAX({expr})"),
        AggFn::Median => format!("MEDIAN({expr})"),
        AggFn::Last => format!("ANY_VALUE({expr})"),
    }
}

fn mapped_builtin_expr(builtin: &str, map: &MappingSpec, type_name: &str) -> String {
    let key_col = format!("\"__m\".\"{}\"", escape_ident(&map.key_column));
    match builtin {
        "id" | "node_key" => format!("CAST({key_col} AS VARCHAR)"),
        "title" => {
            if let Some(col) = map
                .title
                .strip_prefix('{')
                .and_then(|s| s.strip_suffix('}'))
                && !col.contains('{')
            {
                format!("CAST(\"__m\".\"{}\" AS VARCHAR)", escape_ident(col))
            } else {
                format!("CAST({key_col} AS VARCHAR)")
            }
        }
        "type_name" => format!("'{}'", type_name.replace('\'', "''")),
        _ => String::from("CURRENT_TIMESTAMP"),
    }
}

pub(crate) fn escape_ident(ident: &str) -> String {
    ident.replace('"', "\"\"")
}

pub(crate) fn push_unique(vec: &mut Vec<String>, val: &str) {
    if !vec.iter().any(|existing| existing == val) {
        vec.push(String::from(val));
    }
}
