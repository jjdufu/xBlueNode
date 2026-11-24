//! Join-free `AxQL` ontology compiler (`OBJECT(...)`, `MATCH (...)`, `SUM_BY_ENTITY`,
//! `MATCH_KNOWLEDGE`) and guarded `DuckDB` execution (`05-STEP27` §27.2).

pub mod fanout_guard;
mod field_rewriter;
pub mod match_plan;
mod match_syntax;
pub mod object_plan;
pub mod parser;
pub mod rewriter;

use std::collections::BTreeMap;

use axpoint_domain::AppError;
use axpoint_domain::analytics::{AxqlCompilePlan, AxqlQueryRequest, AxqlQueryResponse};
use axpoint_domain::guard::PolicyContext;
use axpoint_domain::studio::SchemaDoc;
use uuid::Uuid;

use crate::axql::parser::bad_axql;
pub use crate::axql::parser::{
    AxqlFromSource, DEFAULT_MATCH_KNOWLEDGE_TOP_K, KnowledgeCallSpec, KnowledgeQueryArg,
    MAX_MATCH_KNOWLEDGE_TOP_K, MatchHopPat, MatchNodePat, MatchPathSpec, ParsedAxql, parse_axql,
};
use crate::axql::rewriter::rewrite_parsed_axql;
use crate::result::{ResultSerializeOptions, batches_to_query_result};
use crate::session::TenantDb;
use crate::sql_guard::{GuardOptions, GuardedSql, TenantCatalog, guard_with_ceiling};

/// Default `max_rows` cap for `AxQL` queries (`05-STEP27` §27.2.4).
pub const AXQL_DEFAULT_MAX_ROWS: u32 = 1_000;

/// Maximum `max_rows` cap for `AxQL` queries (`05-STEP27` §27.2.4).
pub const AXQL_MAX_ROWS_CAP: u32 = 50_000;

/// Default `timeout_ms` for `AxQL` execution (`05-STEP27` §27.2.4).
pub const AXQL_DEFAULT_TIMEOUT_MS: u64 = 15_000;

/// Minimum `timeout_ms` for `AxQL` execution.
pub const AXQL_MIN_TIMEOUT_MS: u64 = 100;

/// Maximum `timeout_ms` for `AxQL` execution (`05-STEP27` §27.2.4).
pub const AXQL_MAX_TIMEOUT_MS: u64 = 60_000;

/// Resolved `MATCH_KNOWLEDGE` recall request ready for 5-way hypergraph RRF execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedKnowledgeCall {
    /// Zero-based call index matching `KnowledgeCallSpec.call_id`.
    pub call_id: usize,
    /// Resolved question text (from literal or `$param`).
    pub query_text: String,
    /// Candidate limit (`1..=100`).
    pub top_k: usize,
}

/// Compiled `AxQL` artifact containing both [`AxqlCompilePlan`] and [`GuardedSql`].
#[derive(Debug, Clone)]
pub struct CompiledAxqlArtifact {
    /// Public compilation plan returned by `/api/analytics/axql/compile` and `/run`.
    pub plan: AxqlCompilePlan,
    /// Guarded SQL ready for `TenantDb::execute_guarded` or in-memory `DuckDB`.
    pub guarded: GuardedSql,
}

/// Extracts and resolves all `MATCH_KNOWLEDGE(...)` calls in `req.axql` using `req.params`.
///
/// # Errors
/// Returns `AppError::BadRequest` (`SQL_REJECTED` or `LIMIT_OUT_OF_RANGE`) on syntax or missing
/// parameter errors.
pub fn extract_axql_knowledge_requests(
    req: &AxqlQueryRequest,
) -> Result<(ParsedAxql, Vec<ResolvedKnowledgeCall>), AppError> {
    let parsed = parse_axql(&req.axql)?;
    let norm_params = normalize_axql_params(&req.params);
    let mut resolved = Vec::with_capacity(parsed.knowledge_calls.len());

    for call in &parsed.knowledge_calls {
        let query_text = match &call.query_arg {
            KnowledgeQueryArg::Literal(lit) => lit.clone(),
            KnowledgeQueryArg::Param(p) => {
                let Some(val) = norm_params.get(p) else {
                    return Err(bad_axql(
                        "SQL_REJECTED",
                        format!("missing parameter '${p}' for MATCH_KNOWLEDGE"),
                    ));
                };
                let Some(s) = val.as_str() else {
                    return Err(bad_axql(
                        "SQL_REJECTED",
                        format!("MATCH_KNOWLEDGE parameter '${p}' must be a string"),
                    ));
                };
                String::from(s)
            }
        };
        if query_text.trim().is_empty() {
            return Err(bad_axql(
                "SQL_REJECTED",
                "MATCH_KNOWLEDGE question string cannot be empty",
            ));
        }
        resolved.push(ResolvedKnowledgeCall {
            call_id: call.call_id,
            query_text,
            top_k: call.top_k,
        });
    }

    Ok((parsed, resolved))
}

/// Compiles a pre-parsed [`ParsedAxql`] against `schema_doc`, `catalog`, and `policy`, passing
/// the expanded SQL through `sql_guard` AST verification and row/column policy injection.
///
/// # Errors
/// - Returns `AppError::Forbidden("PERMISSION_DENIED")` if `req.reveal_pii` is unauthorized.
/// - Returns `AppError::Forbidden("POLICY_DENIED")` if a referenced dataset is denied.
/// - Returns `AppError::BadRequest` (`AXQL_UNMAPPED_SYMBOL`, `JOIN_PATH_UNGROUNDED`,
///   `LIMIT_OUT_OF_RANGE`, or `SQL_REJECTED`) on compilation or guard errors.
pub fn compile_parsed_axql(
    parsed: &ParsedAxql,
    req: &AxqlQueryRequest,
    schema_doc: &SchemaDoc,
    catalog: &TenantCatalog,
    policy: Option<&PolicyContext>,
    has_pii_raw_read: bool,
    knowledge_results: &BTreeMap<usize, Vec<Uuid>>,
) -> Result<CompiledAxqlArtifact, AppError> {
    let pii_allowed = has_pii_raw_read || policy.is_some_and(|p| p.reveal_pii_authorized);
    if req.reveal_pii && !pii_allowed {
        return Err(AppError::Forbidden("PERMISSION_DENIED"));
    }
    let effective_max_rows = validate_axql_max_rows(req.max_rows)?;
    let _timeout = validate_axql_timeout_ms(req.timeout_ms)?;

    let (raw_sql, mut plan) = rewrite_parsed_axql(
        parsed,
        schema_doc,
        catalog,
        policy,
        req.reveal_pii,
        knowledge_results,
    )?;

    let guard_opts = GuardOptions {
        max_rows: effective_max_rows,
        allow_live: false,
        params: normalize_axql_params(&req.params),
        policy: policy.cloned(),
        strict_ontology_join: false,
        allowed_joins: catalog.allowed_joins.clone(),
    };
    let guarded = guard_with_ceiling(&raw_sql, catalog, &guard_opts, AXQL_MAX_ROWS_CAP)?;
    plan.compiled_sql.clone_from(&guarded.sql);

    Ok(CompiledAxqlArtifact { plan, guarded })
}

/// Convenience one-shot `AxQL` compiler when no external `MATCH_KNOWLEDGE` recall is needed
/// (or with pre-populated `knowledge_results`).
///
/// # Errors
/// Propagates errors from [`parse_axql`] and [`compile_parsed_axql`].
pub fn compile_axql_query(
    req: &AxqlQueryRequest,
    schema_doc: &SchemaDoc,
    catalog: &TenantCatalog,
    policy: Option<&PolicyContext>,
    has_pii_raw_read: bool,
    knowledge_results: &BTreeMap<usize, Vec<Uuid>>,
) -> Result<CompiledAxqlArtifact, AppError> {
    let (parsed, _) = extract_axql_knowledge_requests(req)?;
    compile_parsed_axql(
        &parsed,
        req,
        schema_doc,
        catalog,
        policy,
        has_pii_raw_read,
        knowledge_results,
    )
}

/// Executes a [`CompiledAxqlArtifact`] against an async [`TenantDb`] session.
///
/// # Errors
/// Returns `PERMISSION_DENIED`, `QUERY_TIMEOUT`, `SQL_REJECTED`, or `Database`.
pub async fn execute_compiled_axql(
    session: &TenantDb,
    artifact: CompiledAxqlArtifact,
    catalog: &TenantCatalog,
    timeout_ms: Option<u64>,
    reveal_pii: bool,
    has_pii_raw_read: bool,
) -> Result<AxqlQueryResponse, AppError> {
    let effective_timeout = validate_axql_timeout_ms(timeout_ms)?;
    let out = session
        .execute_guarded(
            &artifact.guarded,
            catalog,
            Some(effective_timeout),
            reveal_pii,
            has_pii_raw_read,
        )
        .await?;
    Ok(AxqlQueryResponse {
        plan: artifact.plan,
        result: out.result,
    })
}

/// Synchronous helper executing an `AxQL` query against a direct `duckdb::Connection`
/// (used in unit tests).
///
/// # Errors
/// Returns any compilation, `DuckDB` execution, or Arrow serialization error.
pub fn execute_axql_conn(
    conn: &duckdb::Connection,
    req: &AxqlQueryRequest,
    schema_doc: &SchemaDoc,
    catalog: &TenantCatalog,
    policy: Option<&PolicyContext>,
    has_pii_raw_read: bool,
    knowledge_results: &BTreeMap<usize, Vec<Uuid>>,
) -> Result<AxqlQueryResponse, AppError> {
    let pii_allowed = has_pii_raw_read || policy.is_some_and(|p| p.reveal_pii_authorized);
    let artifact = compile_axql_query(
        req,
        schema_doc,
        catalog,
        policy,
        pii_allowed,
        knowledge_results,
    )?;
    let mut stmt = conn
        .prepare(&artifact.guarded.sql)
        .map_err(|e| AppError::Database(format!("duckdb prepare failed: {e}")))?;
    let arrow_stream = stmt
        .query_arrow([])
        .map_err(|e| AppError::Database(format!("duckdb query_arrow failed: {e}")))?;
    let schema = arrow_stream.get_schema();
    let batches: Vec<_> = arrow_stream.collect();
    let serialized = batches_to_query_result(
        &schema,
        &batches,
        ResultSerializeOptions {
            effective_max_rows: artifact.guarded.effective_max_rows,
            sql: artifact.guarded.sql.clone(),
            duration_ms: 0,
            reveal_pii: req.reveal_pii,
            has_pii_raw_read: pii_allowed,
            catalog,
            guarded: &artifact.guarded,
        },
    )?;
    Ok(AxqlQueryResponse {
        plan: artifact.plan,
        result: serialized.result,
    })
}

/// Validates `AxQL` `max_rows` (`1..=50_000`, default `1_000`).
///
/// # Errors
/// Returns `AppError::BadRequest` (`LIMIT_OUT_OF_RANGE`) if outside `1..=50_000`.
pub fn validate_axql_max_rows(max_rows: Option<u32>) -> Result<u32, AppError> {
    let rows = max_rows.unwrap_or(AXQL_DEFAULT_MAX_ROWS);
    if !(1..=AXQL_MAX_ROWS_CAP).contains(&rows) {
        return Err(bad_axql(
            "LIMIT_OUT_OF_RANGE",
            format!("max_rows must be in 1..={AXQL_MAX_ROWS_CAP}, got {rows}"),
        ));
    }
    Ok(rows)
}

/// Validates `AxQL` `timeout_ms` (`100..=60_000`, default `15_000`).
///
/// # Errors
/// Returns `AppError::BadRequest` (`LIMIT_OUT_OF_RANGE`) if outside `100..=60_000`.
pub fn validate_axql_timeout_ms(timeout_ms: Option<u64>) -> Result<u64, AppError> {
    let ms = timeout_ms.unwrap_or(AXQL_DEFAULT_TIMEOUT_MS);
    if !(AXQL_MIN_TIMEOUT_MS..=AXQL_MAX_TIMEOUT_MS).contains(&ms) {
        return Err(bad_axql(
            "LIMIT_OUT_OF_RANGE",
            format!(
                "timeout_ms must be in {AXQL_MIN_TIMEOUT_MS}..={AXQL_MAX_TIMEOUT_MS}, got {ms}"
            ),
        ));
    }
    Ok(ms)
}

fn normalize_axql_params(
    params: &BTreeMap<String, serde_json::Value>,
) -> BTreeMap<String, serde_json::Value> {
    let mut out = BTreeMap::new();
    for (k, v) in params {
        let clean = k
            .trim()
            .strip_prefix('$')
            .or_else(|| k.trim().strip_prefix(':'))
            .unwrap_or(k.trim());
        out.insert(String::from(clean), v.clone());
    }
    out
}
