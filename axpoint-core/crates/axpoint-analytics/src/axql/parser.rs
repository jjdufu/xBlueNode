//! `AxQL` surface syntax preprocessor and `DuckDB` `AST` parser (`05-STEP27` §27.2).
//!
//! Desugars `FROM OBJECT(<Type>) [AS] <alias>`, `FROM MATCH (a:TypeA)-[:REL]->(b:TypeB)`,
//! `SUM_BY_ENTITY(expr BY alias)` / `AVG_BY_ENTITY(expr BY alias)`, `MATCH_KNOWLEDGE(...)`,
//! and `$param` bindings into sentinel `DuckDbDialect` `AST` nodes for deterministic validation
//! and rewriting.

use axpoint_domain::AppError;
use axpoint_domain::studio::LinkDirection;
use sqlparser::ast::{Query, SetExpr, Statement, TableFactor};
use sqlparser::dialect::DuckDbDialect;
use sqlparser::parser::Parser;

use crate::axql::match_syntax::{desugar_by_entity_fn, desugar_match_knowledge, parse_match_chain};
use crate::sql_guard::MAX_SQL_BYTES;

pub(crate) const SENTINEL_OBJECT_TABLE: &str = "__axql_src_object__";
pub(crate) const SENTINEL_MATCH_TABLE: &str = "__axql_src_match__";
pub(crate) const FN_SUM_BY_ENTITY: &str = "__axql_sum_by_entity__";
pub(crate) const FN_AVG_BY_ENTITY: &str = "__axql_avg_by_entity__";
pub(crate) const FN_MATCH_KNOWLEDGE: &str = "__axql_match_knowledge__";

/// Default `top_k` candidate limit for `MATCH_KNOWLEDGE(...)`.
pub const DEFAULT_MATCH_KNOWLEDGE_TOP_K: usize = 10;

/// Maximum `top_k` candidate limit for `MATCH_KNOWLEDGE(...)`.
pub const MAX_MATCH_KNOWLEDGE_TOP_K: usize = 100;

/// Parsed `AxQL` statement prior to ontology symbol resolution and CTE expansion.
#[derive(Debug, Clone)]
pub struct ParsedAxql {
    /// Primary ontology source (`OBJECT(...)` or `MATCH (...)`).
    pub source: AxqlFromSource,
    /// Desugared `sqlparser` `SELECT` query AST.
    pub query: Box<Query>,
    /// Extracted `MATCH_KNOWLEDGE(...)` invocations in encounter order.
    pub knowledge_calls: Vec<KnowledgeCallSpec>,
}

/// Ontology `FROM` clause source in an `AxQL` query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AxqlFromSource {
    /// `FROM OBJECT(<type_name>) [AS] <alias>` cross-tier entity projection.
    Object {
        /// Target `NodeTypeSpec.name`.
        type_name: String,
        /// Bound SQL alias in the query.
        alias: String,
    },
    /// `FROM MATCH (a:TypeA)-[:REL]->(b:TypeB)...` grounded multi-hop graph pattern.
    Match(MatchPathSpec),
}

/// Grounded multi-hop `MATCH` graph path (`nodes.len() == hops.len() + 1`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchPathSpec {
    /// Node patterns `(alias:TypeName)` along the path (`>= 2`).
    pub nodes: Vec<MatchNodePat>,
    /// Directed relationship hops connecting adjacent `nodes`.
    pub hops: Vec<MatchHopPat>,
}

/// Single `(alias:TypeName)` vertex pattern inside `FROM MATCH`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchNodePat {
    /// Bound vertex alias (e.g., `w`).
    pub alias: String,
    /// Declared `NodeTypeSpec.name` (e.g., `Step27Warehouse`).
    pub type_name: String,
}

/// Single `-[alias?:EDGE_TYPE]->` or `<-[alias?:EDGE_TYPE]-` hop inside `FROM MATCH`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchHopPat {
    /// Optional edge variable alias (`[r:FULFILLS]`).
    pub edge_alias: Option<String>,
    /// Declared `EdgeTypeSpec.name` (`FULFILLS`).
    pub edge_type: String,
    /// Edge direction (`Out` for `-[...]->`, `In` for `<-[...]-`).
    pub direction: LinkDirection,
}

/// Query string source passed into `MATCH_KNOWLEDGE(...)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KnowledgeQueryArg {
    /// Inline string literal (`'warehouse fire risk'`).
    Literal(String),
    /// Named parameter (`$question` or `:question`, stored without prefix).
    Param(String),
}

/// Parsed `MATCH_KNOWLEDGE([alias,] $question [, top_k = N])` call specification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnowledgeCallSpec {
    /// Zero-based index matching `__axql_match_knowledge__(call_id)`.
    pub call_id: usize,
    /// Optional explicit entity alias targeted by this recall predicate.
    pub target_alias: Option<String>,
    /// Literal or `$param` question source.
    pub query_arg: KnowledgeQueryArg,
    /// Recall `top_k` (`1..=100`).
    pub top_k: usize,
}

pub(crate) struct DesugarState {
    pub(crate) source: Option<AxqlFromSource>,
    pub(crate) knowledge_calls: Vec<KnowledgeCallSpec>,
}

pub(crate) fn bad_axql(code: &'static str, msg: impl Into<String>) -> AppError {
    AppError::bad(code, msg.into())
}

pub(crate) fn is_ident_start(ch: char) -> bool {
    ch.is_ascii_alphabetic() || ch == '_'
}

pub(crate) fn is_ident_continue(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_'
}

pub(crate) fn is_valid_ident(raw: &str) -> bool {
    let mut chars = raw.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    is_ident_start(first) && chars.all(is_ident_continue)
}

/// Parses an `AxQL` query string into a structured [`ParsedAxql`] representation.
///
/// # Errors
/// - Returns `AppError::PayloadTooLarge("PAYLOAD_TOO_LARGE")` if `axql.len() > 65_536`.
/// - Returns `AppError::BadRequest` with `AXQL_UNMAPPED_SYMBOL`, `JOIN_PATH_UNGROUNDED`, or
///   `SQL_REJECTED` on syntax, ungrounded join, or malformed extension errors.
pub fn parse_axql(axql: &str) -> Result<ParsedAxql, AppError> {
    if axql.len() > MAX_SQL_BYTES {
        return Err(AppError::PayloadTooLarge("PAYLOAD_TOO_LARGE"));
    }
    if axql.contains('\0') {
        return Err(bad_axql("SQL_REJECTED", "AxQL contains forbidden NUL byte"));
    }
    let trimmed = axql.trim().trim_end_matches(';').trim();
    if trimmed.is_empty() {
        return Err(bad_axql("SQL_REJECTED", "AxQL statement cannot be empty"));
    }

    let mut state = DesugarState {
        source: None,
        knowledge_calls: Vec::new(),
    };
    let desugared = desugar_axql_tokens(trimmed, &mut state)?;
    let stmts = Parser::parse_sql(&DuckDbDialect {}, &desugared)
        .map_err(|err| bad_axql("SQL_REJECTED", format!("AxQL syntax error: {err}")))?;
    let [Statement::Query(query)] = stmts.as_slice() else {
        return Err(bad_axql(
            "SQL_REJECTED",
            "AxQL only permits a single SELECT query",
        ));
    };

    let source = match state.source {
        Some(src) => {
            ensure_no_extra_sql_joins(query)?;
            src
        }
        None => infer_bare_from_source(query)?,
    };

    Ok(ParsedAxql {
        source,
        query: query.clone(),
        knowledge_calls: state.knowledge_calls,
    })
}

fn ensure_no_extra_sql_joins(query: &Query) -> Result<(), AppError> {
    let SetExpr::Select(select) = query.body.as_ref() else {
        return Err(bad_axql(
            "SQL_REJECTED",
            "AxQL top-level query body must be SELECT",
        ));
    };
    if select.from.len() != 1 {
        return Err(bad_axql(
            "JOIN_PATH_UNGROUNDED",
            "comma cross-joins are forbidden in AxQL; use FROM MATCH (a)-[:REL]->(b)",
        ));
    }
    if let Some(first) = select.from.first()
        && !first.joins.is_empty()
    {
        return Err(bad_axql(
            "JOIN_PATH_UNGROUNDED",
            "manual SQL JOIN is forbidden in AxQL; use FROM OBJECT(...) or FROM MATCH (...)",
        ));
    }
    Ok(())
}

fn infer_bare_from_source(query: &Query) -> Result<AxqlFromSource, AppError> {
    let SetExpr::Select(select) = query.body.as_ref() else {
        return Err(bad_axql(
            "SQL_REJECTED",
            "AxQL top-level query body must be SELECT",
        ));
    };
    if select.from.len() != 1 {
        return Err(bad_axql(
            "JOIN_PATH_UNGROUNDED",
            "AxQL query must specify FROM OBJECT(<Type>) or FROM MATCH (...)",
        ));
    }
    let Some(twj) = select.from.first() else {
        return Err(bad_axql("SQL_REJECTED", "missing FROM clause in AxQL"));
    };
    if !twj.joins.is_empty() {
        return Err(bad_axql(
            "JOIN_PATH_UNGROUNDED",
            "ungrounded SQL JOIN in AxQL; use FROM MATCH (a:TypeA)-[:EDGE]->(b:TypeB)",
        ));
    }
    let TableFactor::Table { name, alias, .. } = &twj.relation else {
        return Err(bad_axql(
            "SQL_REJECTED",
            "AxQL FROM clause must use OBJECT(<Type>) or MATCH (...)",
        ));
    };
    if name.0.len() != 1 {
        return Err(bad_axql(
            "AXQL_UNMAPPED_SYMBOL",
            format!("AXQL_UNMAPPED_SYMBOL: qualified relation '{name}' is forbidden in AxQL"),
        ));
    }
    let type_name = name
        .0
        .first()
        .and_then(|p| p.as_ident())
        .map_or_default(|i| i.value.clone());
    if !is_valid_ident(&type_name) {
        return Err(bad_axql(
            "AXQL_UNMAPPED_SYMBOL",
            format!("AXQL_UNMAPPED_SYMBOL: invalid object type identifier '{type_name}'"),
        ));
    }
    let eff_alias = alias
        .as_ref()
        .map_or_else(|| type_name.to_ascii_lowercase(), |a| a.name.value.clone());
    if !is_valid_ident(&eff_alias) {
        return Err(bad_axql(
            "AXQL_UNMAPPED_SYMBOL",
            format!("AXQL_UNMAPPED_SYMBOL: invalid object alias '{eff_alias}'"),
        ));
    }
    Ok(AxqlFromSource::Object {
        type_name,
        alias: eff_alias,
    })
}

fn copy_quoted(
    chars: &[char],
    start: usize,
    quote: char,
    out: &mut String,
) -> Result<usize, AppError> {
    out.push(quote);
    let mut i = start.saturating_add(1);
    while let Some(&c) = chars.get(i) {
        out.push(c);
        i = i.saturating_add(1);
        if c == quote {
            if chars.get(i) == Some(&quote) {
                out.push(quote);
                i = i.saturating_add(1);
            } else {
                return Ok(i);
            }
        }
    }
    Err(bad_axql("SQL_REJECTED", "unclosed quoted literal in AxQL"))
}

pub(crate) fn skip_ws(chars: &[char], mut idx: usize) -> usize {
    while let Some(&c) = chars.get(idx) {
        if c.is_whitespace() {
            idx = idx.saturating_add(1);
        } else {
            break;
        }
    }
    idx
}

fn read_word(chars: &[char], start: usize) -> (String, usize) {
    let mut i = start;
    let mut out = String::new();
    while let Some(&c) = chars.get(i) {
        if is_ident_continue(c) {
            out.push(c);
            i = i.saturating_add(1);
        } else {
            break;
        }
    }
    (out, i)
}

pub(crate) fn extract_balanced_parens(
    chars: &[char],
    open_idx: usize,
) -> Result<(String, usize), AppError> {
    if chars.get(open_idx) != Some(&'(') {
        return Err(bad_axql("SQL_REJECTED", "expected '('"));
    }
    let mut depth = 1_usize;
    let mut i = open_idx.saturating_add(1);
    let mut body = String::new();
    while let Some(&c) = chars.get(i) {
        if c == '\'' || c == '"' {
            let next_i = copy_quoted(chars, i, c, &mut body)?;
            i = next_i;
            continue;
        }
        if c == '(' {
            depth = depth.saturating_add(1);
            body.push(c);
        } else if c == ')' {
            depth = depth.saturating_sub(1);
            if depth == 0 {
                return Ok((body, i.saturating_add(1)));
            }
            body.push(c);
        } else {
            body.push(c);
        }
        i = i.saturating_add(1);
    }
    Err(bad_axql("SQL_REJECTED", "unclosed '(' in AxQL construct"))
}

pub(crate) fn desugar_axql_tokens(raw: &str, state: &mut DesugarState) -> Result<String, AppError> {
    let chars: Vec<char> = raw.chars().collect();
    let mut out = String::with_capacity(raw.len().saturating_add(64));
    let mut idx = 0_usize;

    while let Some(&ch) = chars.get(idx) {
        if ch == '\'' || ch == '"' {
            idx = copy_quoted(&chars, idx, ch, &mut out)?;
            continue;
        }
        if ch == ';' {
            return Err(bad_axql(
                "SQL_REJECTED",
                "multiple statements or semicolons are forbidden in AxQL",
            ));
        }
        if (ch == '-' && chars.get(idx.saturating_add(1)) == Some(&'-'))
            || (ch == '/' && chars.get(idx.saturating_add(1)) == Some(&'*'))
        {
            return Err(bad_axql(
                "SQL_REJECTED",
                "SQL comments are forbidden in AxQL",
            ));
        }
        if ch == '$' {
            let (param_name, next_idx) = read_word(&chars, idx.saturating_add(1));
            if !is_valid_ident(&param_name) {
                return Err(bad_axql(
                    "SQL_REJECTED",
                    "invalid $parameter identifier in AxQL",
                ));
            }
            out.push(':');
            out.push_str(&param_name);
            idx = next_idx;
            continue;
        }
        if is_ident_start(ch) {
            let prev_is_dot = out.trim_end().ends_with('.');
            let (word, after_word) = read_word(&chars, idx);
            let upper = word.to_ascii_uppercase();
            if !prev_is_dot
                && let Some(next_idx) =
                    try_desugar_keyword(&chars, &upper, after_word, state, &mut out)?
            {
                idx = next_idx;
                continue;
            }
            out.push_str(&word);
            idx = after_word;
            continue;
        }
        out.push(ch);
        idx = idx.saturating_add(1);
    }

    Ok(out)
}

fn try_desugar_keyword(
    chars: &[char],
    upper: &str,
    after_word: usize,
    state: &mut DesugarState,
    out: &mut String,
) -> Result<Option<usize>, AppError> {
    match upper {
        "FROM" => try_desugar_from_clause(chars, after_word, state, out),
        "SUM_BY_ENTITY" => {
            let next = desugar_by_entity_fn(chars, after_word, FN_SUM_BY_ENTITY, out)?;
            Ok(Some(next))
        }
        "AVG_BY_ENTITY" => {
            let next = desugar_by_entity_fn(chars, after_word, FN_AVG_BY_ENTITY, out)?;
            Ok(Some(next))
        }
        "MATCH_KNOWLEDGE" => {
            let next = desugar_match_knowledge(chars, after_word, state, out)?;
            Ok(Some(next))
        }
        _ => Ok(None),
    }
}

fn try_desugar_from_clause(
    chars: &[char],
    after_from: usize,
    state: &mut DesugarState,
    out: &mut String,
) -> Result<Option<usize>, AppError> {
    let kw_pos = skip_ws(chars, after_from);
    let (next_word, after_next) = read_word(chars, kw_pos);
    let next_upper = next_word.to_ascii_uppercase();

    if next_upper == "OBJECT" {
        let open_pos = skip_ws(chars, after_next);
        if chars.get(open_pos) != Some(&'(') {
            return Ok(None);
        }
        if state.source.is_some() {
            return Err(bad_axql(
                "SQL_REJECTED",
                "only one FROM OBJECT(...) or FROM MATCH (...) source is allowed",
            ));
        }
        let (type_raw, after_paren) = extract_balanced_parens(chars, open_pos)?;
        let type_name = type_raw.trim().trim_matches('"').trim();
        if !is_valid_ident(type_name) {
            return Err(bad_axql(
                "AXQL_UNMAPPED_SYMBOL",
                format!("AXQL_UNMAPPED_SYMBOL: invalid object type identifier '{type_name}'"),
            ));
        }
        let (alias, end_idx) = parse_optional_alias(chars, after_paren, type_name)?;
        state.source = Some(AxqlFromSource::Object {
            type_name: String::from(type_name),
            alias: alias.clone(),
        });
        out.push_str("FROM \"");
        out.push_str(SENTINEL_OBJECT_TABLE);
        out.push_str("\" AS \"");
        out.push_str(&alias);
        out.push('"');
        return Ok(Some(end_idx));
    }

    if next_upper == "MATCH" {
        let pat_start = skip_ws(chars, after_next);
        if chars.get(pat_start) != Some(&'(') {
            return Err(bad_axql(
                "JOIN_PATH_UNGROUNDED",
                "FROM MATCH must start with a vertex pattern '(alias:Type)'",
            ));
        }
        if state.source.is_some() {
            return Err(bad_axql(
                "SQL_REJECTED",
                "only one FROM OBJECT(...) or FROM MATCH (...) source is allowed",
            ));
        }
        let (match_spec, end_idx) = parse_match_chain(chars, pat_start)?;
        state.source = Some(AxqlFromSource::Match(match_spec));
        out.push_str("FROM \"");
        out.push_str(SENTINEL_MATCH_TABLE);
        out.push_str("\" AS \"__axql_m\"");
        return Ok(Some(end_idx));
    }

    Ok(None)
}

fn parse_optional_alias(
    chars: &[char],
    pos: usize,
    default_type: &str,
) -> Result<(String, usize), AppError> {
    let ws_pos = skip_ws(chars, pos);
    if chars.get(ws_pos) == Some(&'"') {
        return parse_double_quoted_ident_alias(chars, ws_pos);
    }
    let (w1, after_w1) = read_word(chars, ws_pos);
    if w1.is_empty() {
        return Ok((default_type.to_ascii_lowercase(), pos));
    }
    let u1 = w1.to_ascii_uppercase();
    if matches!(
        u1.as_str(),
        "WHERE" | "GROUP" | "HAVING" | "ORDER" | "LIMIT" | "OFFSET" | "JOIN" | "LEFT" | "INNER"
    ) {
        return Ok((default_type.to_ascii_lowercase(), pos));
    }
    if u1 == "AS" {
        let alias_pos = skip_ws(chars, after_w1);
        if chars.get(alias_pos) == Some(&'"') {
            return parse_double_quoted_ident_alias(chars, alias_pos);
        }
        let (alias_word, after_alias) = read_word(chars, alias_pos);
        if !is_valid_ident(&alias_word) {
            return Err(bad_axql(
                "AXQL_UNMAPPED_SYMBOL",
                format!("AXQL_UNMAPPED_SYMBOL: invalid object alias '{alias_word}'"),
            ));
        }
        return Ok((alias_word, after_alias));
    }
    if !is_valid_ident(&w1) {
        return Err(bad_axql(
            "AXQL_UNMAPPED_SYMBOL",
            format!("AXQL_UNMAPPED_SYMBOL: invalid object alias '{w1}'"),
        ));
    }
    Ok((w1, after_w1))
}

fn parse_double_quoted_ident_alias(
    chars: &[char],
    quote_pos: usize,
) -> Result<(String, usize), AppError> {
    let mut raw_quoted = String::new();
    let end_idx = copy_quoted(chars, quote_pos, '"', &mut raw_quoted)?;
    let inner = raw_quoted
        .get(1..raw_quoted.len().saturating_sub(1))
        .unwrap_or("")
        .replace("\"\"", "\"");
    if !is_valid_ident(&inner) {
        return Err(bad_axql(
            "AXQL_UNMAPPED_SYMBOL",
            format!("AXQL_UNMAPPED_SYMBOL: invalid quoted alias '{inner}'"),
        ));
    }
    Ok((inner, end_idx))
}
