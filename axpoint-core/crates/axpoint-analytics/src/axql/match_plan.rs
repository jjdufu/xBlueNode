//! `FROM MATCH (a:TypeA)-[:REL]->(b:TypeB)...` ontology path verifier and CTE builder
//! (`05-STEP27` §27.2.2).

use std::collections::{BTreeMap, BTreeSet};

use axpoint_domain::AppError;
use axpoint_domain::guard::PolicyContext;
use axpoint_domain::studio::{
    EdgeTypeSpec, LinkDirection, MappingSpec, NodeTypeSpec, PropertySpec, SchemaDoc,
};
use sqlparser::ast::Cte;

use crate::axql::fanout_guard::parse_cte;
use crate::axql::object_plan::{
    BUILTIN_NODE_FIELDS, ax_node_property_sql, escape_ident, find_active_mapping, lookup_node_type,
    push_unique,
};
use crate::axql::parser::{MatchPathSpec, bad_axql};
use crate::sql_guard::TenantCatalog;

const BUILTIN_EDGE_FIELDS: &[&str] = &["id", "link_type", "weight", "src_node_id", "dst_node_id"];

/// Resolved field on a `MATCH` vertex or relationship variable.
#[derive(Debug, Clone)]
pub enum MatchFieldBinding {
    /// Built-in node column (`id`, `node_key`, `title`, `type_name`, `updated_at`).
    NodeBuiltin(String),
    /// Declared property on a vertex `NodeTypeSpec`.
    NodeProperty(PropertySpec),
    /// Built-in edge column (`id`, `link_type`, `weight`, `src_node_id`, `dst_node_id`).
    EdgeBuiltin(String),
    /// Declared property on an `EdgeTypeSpec`.
    EdgeProperty(PropertySpec),
}

impl MatchFieldBinding {
    /// Canonical field name exposed on the variable alias.
    #[must_use]
    pub fn canonical_name(&self) -> &str {
        match self {
            Self::NodeBuiltin(n) | Self::EdgeBuiltin(n) => n,
            Self::NodeProperty(p) | Self::EdgeProperty(p) => &p.name,
        }
    }
}

/// Validated `FROM MATCH` path schema metadata.
#[derive(Debug)]
pub struct ValidatedMatchPath {
    /// Ordered `(vertex_alias, NodeTypeSpec)` pairs.
    pub vertices: Vec<(String, NodeTypeSpec)>,
    /// Ordered `(hop_index, optional_edge_alias, direction, EdgeTypeSpec)` items.
    pub edges: Vec<(usize, Option<String>, LinkDirection, EdgeTypeSpec)>,
    /// Set of vertex aliases (eligible for `SUM_BY_ENTITY` / `MATCH_KNOWLEDGE`).
    pub vertex_aliases: BTreeSet<String>,
}

/// Output of compiling `FROM MATCH (...)` into `"__axql_base"`.
#[derive(Debug)]
pub struct MatchCtePlan {
    /// Generated `"__axql_base"` CTE.
    pub base_cte: Cte,
    /// Referenced ontology node type names.
    pub referenced_node_types: Vec<String>,
    /// Referenced ontology edge type names.
    pub referenced_edge_types: Vec<String>,
    /// Referenced dataset or `ax.*` relation names.
    pub referenced_relations: Vec<String>,
}

/// Validates all vertices and directed hops of `path` against `schema_doc`.
///
/// # Errors
/// - Returns `AppError::BadRequest` (`AXQL_UNMAPPED_SYMBOL`) if a vertex `NodeTypeSpec` is missing
///   or an alias is duplicated.
/// - Returns `AppError::BadRequest` (`JOIN_PATH_UNGROUNDED`) if any hop lacks a matching `EdgeTypeSpec`.
pub fn validate_match_path(
    schema_doc: &SchemaDoc,
    path: &MatchPathSpec,
) -> Result<ValidatedMatchPath, AppError> {
    let mut seen_aliases = BTreeSet::new();
    let mut vertices = Vec::with_capacity(path.nodes.len());
    let mut vertex_aliases = BTreeSet::new();

    for v in &path.nodes {
        if !seen_aliases.insert(v.alias.to_ascii_lowercase()) {
            return Err(bad_axql(
                "SQL_REJECTED",
                format!("duplicate alias '{}' in FROM MATCH", v.alias),
            ));
        }
        let spec = lookup_node_type(schema_doc, &v.type_name)?.clone();
        vertex_aliases.insert(v.alias.clone());
        vertices.push((v.alias.clone(), spec));
    }

    let mut edges = Vec::with_capacity(path.hops.len());
    for (idx, hop) in path.hops.iter().enumerate() {
        if let Some(ref ea) = hop.edge_alias
            && !seen_aliases.insert(ea.to_ascii_lowercase())
        {
            return Err(bad_axql(
                "SQL_REJECTED",
                format!("duplicate edge alias '{ea}' in FROM MATCH"),
            ));
        }
        let Some((_, left_spec)) = vertices.get(idx) else {
            return Err(bad_axql("JOIN_PATH_UNGROUNDED", "missing source vertex"));
        };
        let Some((_, right_spec)) = vertices.get(idx.saturating_add(1)) else {
            return Err(bad_axql("JOIN_PATH_UNGROUNDED", "missing target vertex"));
        };
        let (expected_src, expected_dst) = match hop.direction {
            LinkDirection::Out => (&left_spec.name, &right_spec.name),
            LinkDirection::In => (&right_spec.name, &left_spec.name),
        };
        let edge_spec = schema_doc
            .edge_types
            .iter()
            .find(|et| {
                et.name.eq_ignore_ascii_case(&hop.edge_type)
                    && et.src_type_name == *expected_src
                    && et.dst_type_name == *expected_dst
            })
            .cloned()
            .ok_or_else(|| {
                bad_axql(
                    "JOIN_PATH_UNGROUNDED",
                    format!(
                        "JOIN_PATH_UNGROUNDED: edge '{}' connecting '{}' -> '{}' is not declared in SchemaDoc",
                        hop.edge_type, expected_src, expected_dst
                    ),
                )
            })?;
        edges.push((idx, hop.edge_alias.clone(), hop.direction, edge_spec));
    }

    Ok(ValidatedMatchPath {
        vertices,
        edges,
        vertex_aliases,
    })
}

/// Resolves `(alias, field)` against a validated `MATCH` path.
///
/// # Errors
/// Returns `AppError::BadRequest` (`AXQL_UNMAPPED_SYMBOL`) if `alias` or `field` is unknown.
pub fn resolve_match_qualified_field(
    validated: &ValidatedMatchPath,
    alias: &str,
    field: &str,
) -> Result<(String, MatchFieldBinding), AppError> {
    if let Some((v_alias, v_spec)) = validated
        .vertices
        .iter()
        .find(|(a, _)| a.eq_ignore_ascii_case(alias))
    {
        let lower = field.to_ascii_lowercase();
        if BUILTIN_NODE_FIELDS.contains(&lower.as_str()) {
            return Ok((v_alias.clone(), MatchFieldBinding::NodeBuiltin(lower)));
        }
        if let Some(prop) = v_spec
            .properties
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(field))
        {
            return Ok((
                v_alias.clone(),
                MatchFieldBinding::NodeProperty(prop.clone()),
            ));
        }
        return Err(bad_axql(
            "AXQL_UNMAPPED_SYMBOL",
            format!(
                "AXQL_UNMAPPED_SYMBOL: field '{field}' is not declared on '{}'",
                v_spec.name
            ),
        ));
    }

    if let Some((_, Some(e_alias), _, e_spec)) = validated
        .edges
        .iter()
        .find(|(_, ea, _, _)| ea.as_deref().is_some_and(|a| a.eq_ignore_ascii_case(alias)))
    {
        let lower = field.to_ascii_lowercase();
        if BUILTIN_EDGE_FIELDS.contains(&lower.as_str()) {
            return Ok((e_alias.clone(), MatchFieldBinding::EdgeBuiltin(lower)));
        }
        if let Some(prop) = e_spec
            .properties
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(field))
        {
            return Ok((
                e_alias.clone(),
                MatchFieldBinding::EdgeProperty(prop.clone()),
            ));
        }
        return Err(bad_axql(
            "AXQL_UNMAPPED_SYMBOL",
            format!(
                "AXQL_UNMAPPED_SYMBOL: edge property '{field}' is not declared on '{}'",
                e_spec.name
            ),
        ));
    }

    Err(bad_axql(
        "AXQL_UNMAPPED_SYMBOL",
        format!("AXQL_UNMAPPED_SYMBOL: unknown alias '{alias}' in MATCH query"),
    ))
}

/// Resolves an unqualified `field` when uniquely declared on exactly one vertex in `validated`.
///
/// # Errors
/// Returns `AppError::BadRequest` (`AXQL_UNMAPPED_SYMBOL`) if `field` is unknown or ambiguous.
pub fn resolve_match_unqualified_field(
    validated: &ValidatedMatchPath,
    field: &str,
) -> Result<(String, MatchFieldBinding), AppError> {
    let mut hits = Vec::new();
    for (v_alias, v_spec) in &validated.vertices {
        if let Some(prop) = v_spec
            .properties
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(field))
        {
            hits.push((
                v_alias.clone(),
                MatchFieldBinding::NodeProperty(prop.clone()),
            ));
        }
    }
    if let [single] = hits.as_slice() {
        return Ok(single.clone());
    }
    Err(bad_axql(
        "AXQL_UNMAPPED_SYMBOL",
        format!(
            "AXQL_UNMAPPED_SYMBOL: unqualified or ambiguous symbol '{field}' in MATCH query; qualify as <alias>.{field}"
        ),
    ))
}

/// Compiles `validated` and its referenced `field_bindings` into `"__axql_base"`.
///
/// # Errors
/// Returns `AppError::BadRequest` if CTE construction fails.
pub fn build_match_base_cte(
    schema_doc: &SchemaDoc,
    catalog: &TenantCatalog,
    validated: &ValidatedMatchPath,
    field_bindings: &BTreeMap<(String, String), MatchFieldBinding>,
    policy: Option<&PolicyContext>,
    reveal_pii: bool,
    has_knowledge_recall: bool,
) -> Result<MatchCtePlan, AppError> {
    if !has_knowledge_recall
        && let Some(mapped) =
            try_build_mapped_match_cte(schema_doc, catalog, validated, field_bindings)?
    {
        return Ok(mapped);
    }
    build_ax_nodes_match_cte(validated, field_bindings, policy, reveal_pii)
}

fn try_build_mapped_match_cte(
    schema_doc: &SchemaDoc,
    catalog: &TenantCatalog,
    validated: &ValidatedMatchPath,
    field_bindings: &BTreeMap<(String, String), MatchFieldBinding>,
) -> Result<Option<MatchCtePlan>, AppError> {
    if validated.edges.iter().any(|(_, ea, _, _)| ea.is_some()) {
        return Ok(None);
    }
    let mut v_maps: Vec<&MappingSpec> = Vec::with_capacity(validated.vertices.len());
    for (_, v_spec) in &validated.vertices {
        let Some(m) = find_active_mapping(schema_doc, catalog, &v_spec.name) else {
            return Ok(None);
        };
        v_maps.push(m);
    }

    let mut joins = Vec::with_capacity(validated.edges.len());
    for (idx, _, dir, edge_spec) in &validated.edges {
        let (Some(&left_m), Some(&right_m)) = (v_maps.get(*idx), v_maps.get(idx.saturating_add(1)))
        else {
            return Ok(None);
        };
        let Some((left_col, right_col)) =
            resolve_mapped_hop_cols(left_m, right_m, *dir, &edge_spec.name)
        else {
            return Ok(None);
        };
        let r_idx = idx.saturating_add(1);
        joins.push(format!(
            " INNER JOIN \"{}\" AS \"__v{r_idx}\" ON \"__v{idx}\".\"{}\" = \"__v{r_idx}\".\"{}\"",
            escape_ident(&right_m.dataset),
            escape_ident(left_col),
            escape_ident(right_col)
        ));
    }

    let mut select_cols = Vec::new();
    let mut ref_nodes = Vec::new();
    let mut ref_edges = Vec::new();
    let mut ref_rels = Vec::new();
    let mut filters = Vec::new();

    for (idx, (v_alias, v_spec)) in validated.vertices.iter().enumerate() {
        let Some(&m) = v_maps.get(idx) else {
            return Ok(None);
        };
        push_unique(&mut ref_nodes, &v_spec.name);
        push_unique(&mut ref_rels, &m.dataset);
        select_cols.push(format!(
            "CAST(\"__v{idx}\".\"{}\" AS VARCHAR) AS \"{v_alias}____entity_id\"",
            escape_ident(&m.key_column)
        ));
        if let Some(f) = m.filter.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            filters.push(format!("({f})"));
        }
    }
    for (_, _, _, es) in &validated.edges {
        push_unique(&mut ref_edges, &es.name);
    }
    if !append_mapped_match_projections(validated, &v_maps, field_bindings, &mut select_cols) {
        return Ok(None);
    }

    let Some(&first_m) = v_maps.first() else {
        return Ok(None);
    };
    let where_sql = if filters.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", filters.join(" AND "))
    };
    let sql = format!(
        "SELECT {} FROM \"{}\" AS \"__v0\"{}{}",
        select_cols.join(", "),
        escape_ident(&first_m.dataset),
        joins.join(""),
        where_sql
    );
    Ok(Some(MatchCtePlan {
        base_cte: parse_cte("__axql_base", &sql)?,
        referenced_node_types: ref_nodes,
        referenced_edge_types: ref_edges,
        referenced_relations: ref_rels,
    }))
}

fn append_mapped_match_projections(
    validated: &ValidatedMatchPath,
    v_maps: &[&MappingSpec],
    field_bindings: &BTreeMap<(String, String), MatchFieldBinding>,
    select_cols: &mut Vec<String>,
) -> bool {
    for ((alias, field_key), binding) in field_bindings {
        let Some(v_idx) = validated.vertices.iter().position(|(a, _)| a == alias) else {
            return false;
        };
        let (Some(&m), Some((_, v_spec))) = (v_maps.get(v_idx), validated.vertices.get(v_idx))
        else {
            return false;
        };
        let sql_expr = match binding {
            MatchFieldBinding::NodeBuiltin(b) => match b.as_str() {
                "id" | "node_key" => {
                    format!(
                        "CAST(\"__v{v_idx}\".\"{}\" AS VARCHAR)",
                        escape_ident(&m.key_column)
                    )
                }
                "type_name" => format!("'{}'", v_spec.name.replace('\'', "''")),
                _ => format!(
                    "CAST(\"__v{v_idx}\".\"{}\" AS VARCHAR)",
                    escape_ident(&m.key_column)
                ),
            },
            MatchFieldBinding::NodeProperty(prop_spec) => {
                let Some(col) = m
                    .attributes
                    .get(&prop_spec.name)
                    .or_else(|| m.metrics.get(&prop_spec.name))
                else {
                    return false;
                };
                format!("\"__v{v_idx}\".\"{}\"", escape_ident(col))
            }
            _ => return false,
        };
        select_cols.push(format!("{sql_expr} AS \"{alias}__{field_key}\""));
    }
    true
}

fn resolve_mapped_hop_cols<'a>(
    left_m: &'a MappingSpec,
    right_m: &'a MappingSpec,
    dir: LinkDirection,
    edge_name: &str,
) -> Option<(&'a str, &'a str)> {
    if let Some(link) = left_m.links.iter().find(|l| {
        l.link_type == edge_name && l.target_type == right_m.node_type && l.direction == dir
    }) {
        return Some((link.target_key_column.as_str(), right_m.key_column.as_str()));
    }
    let inv_dir = match dir {
        LinkDirection::Out => LinkDirection::In,
        LinkDirection::In => LinkDirection::Out,
    };
    if let Some(link) = right_m.links.iter().find(|l| {
        l.link_type == edge_name && l.target_type == left_m.node_type && l.direction == inv_dir
    }) {
        return Some((left_m.key_column.as_str(), link.target_key_column.as_str()));
    }
    None
}

fn build_ax_nodes_match_cte(
    validated: &ValidatedMatchPath,
    field_bindings: &BTreeMap<(String, String), MatchFieldBinding>,
    policy: Option<&PolicyContext>,
    reveal_pii: bool,
) -> Result<MatchCtePlan, AppError> {
    let mut select_cols = Vec::new();
    let mut joins = Vec::new();
    let mut wheres = Vec::new();
    let mut ref_nodes = Vec::new();
    let mut ref_edges = Vec::new();

    for (idx, (v_alias, v_spec)) in validated.vertices.iter().enumerate() {
        push_unique(&mut ref_nodes, &v_spec.name);
        select_cols.push(format!("\"__v{idx}\".\"id\" AS \"{v_alias}____entity_id\""));
        wheres.push(format!(
            "\"__v{idx}\".\"type_name\" = '{}'",
            v_spec.name.replace('\'', "''")
        ));
    }

    for (idx, _, dir, edge_spec) in &validated.edges {
        push_unique(&mut ref_edges, &edge_spec.name);
        let r_idx = idx.saturating_add(1);
        let (left_fk, right_fk) = match dir {
            LinkDirection::Out => ("src_node_id", "dst_node_id"),
            LinkDirection::In => ("dst_node_id", "src_node_id"),
        };
        joins.push(format!(
            " INNER JOIN ax.links AS \"__e{idx}\" ON \"__e{idx}\".\"{left_fk}\" = \"__v{idx}\".\"id\" \
             INNER JOIN ax.nodes AS \"__v{r_idx}\" ON \"__v{r_idx}\".\"id\" = \"__e{idx}\".\"{right_fk}\""
        ));
        wheres.push(format!(
            "\"__e{idx}\".\"link_type\" = '{}'",
            edge_spec.name.replace('\'', "''")
        ));
    }

    for ((alias, field_key), binding) in field_bindings {
        if let Some((v_idx, (_, v_spec))) = validated
            .vertices
            .iter()
            .enumerate()
            .find(|(_, (a, _))| a == alias)
        {
            let tbl = format!("__v{v_idx}");
            let expr = match binding {
                MatchFieldBinding::NodeBuiltin(b) => format!("\"{tbl}\".\"{b}\""),
                MatchFieldBinding::NodeProperty(prop_spec) => {
                    ax_node_property_sql(&tbl, &v_spec.name, prop_spec, policy, reveal_pii)
                }
                _ => continue,
            };
            select_cols.push(format!("{expr} AS \"{alias}__{field_key}\""));
        } else if let Some((e_idx, _, _, _)) = validated
            .edges
            .iter()
            .find(|(_, ea, _, _)| ea.as_deref() == Some(alias.as_str()))
        {
            let tbl = format!("__e{e_idx}");
            let expr = match binding {
                MatchFieldBinding::EdgeBuiltin(b) => format!("\"{tbl}\".\"{b}\""),
                MatchFieldBinding::EdgeProperty(prop_spec) => {
                    let escaped = prop_spec.name.replace('\'', "''");
                    format!("json_extract_string(\"{tbl}\".\"properties\", '$.{escaped}')")
                }
                _ => continue,
            };
            select_cols.push(format!("{expr} AS \"{alias}__{field_key}\""));
        }
    }

    let sql = format!(
        "SELECT {} FROM ax.nodes AS \"__v0\"{} WHERE {}",
        select_cols.join(", "),
        joins.join(""),
        wheres.join(" AND ")
    );
    Ok(MatchCtePlan {
        base_cte: parse_cte("__axql_base", &sql)?,
        referenced_node_types: ref_nodes,
        referenced_edge_types: ref_edges,
        referenced_relations: vec![String::from("ax.nodes"), String::from("ax.links")],
    })
}
