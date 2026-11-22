//! Pure-memory grounding guard-chain latency benchmark (`08-STEP22` §22.2.3).
//!
//! Measures 1,000 iterations of:
//! 1. SQL AST validation, tenant catalog rewrite, and `MappingLinkSpec` ontology `JOIN` path checking,
//! 2. Numeric cell grounding verification (`verify_or_fallback_summary`),
//! 3. Inline paragraph citation bounds sanitization (`verify_and_sanitize_citations`),
//! 4. Outbound `PII` span redaction and masked restoration (`EgressSession` + `redact_for_egress`).
//!
//! Asserts P95 `< 50.0ms` and emits `GROUNDED_BENCH_JSON=<json>`.

use std::collections::BTreeMap;
use std::hint::black_box;
use std::io::Write as _;
use std::time::Instant;

use axpoint_analytics::nl2sql::chart::{fallback_table_summary, verify_or_fallback_summary};
use axpoint_analytics::nl2sql::schema_link::compile_ontology_join_edges;
use axpoint_analytics::{
    AllowedDatasetJoinEdge, CatalogDatasetEntry, GuardOptions, TenantCatalog, guard,
    guard_and_rewrite,
};
use axpoint_domain::AppError;
use axpoint_domain::axhub::ColumnMeta;
use axpoint_domain::enums::{DataType, PiiCategory};
use axpoint_domain::grounding::{count_grounded_numbers, verify_and_sanitize_citations};
use axpoint_domain::studio::{
    EdgeTypeSpec, LinkDirection, MappingGeoSpec, MappingLinkSpec, MappingMode, MappingSpec,
    NodeTypeSpec, OnSourceDelete, PropertyKind, PropertySpec, SchemaDoc,
};
use axpoint_security::pii_local::{analyze_pii_local, redact_for_egress, restore_from_egress};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use uuid::Uuid;

const BENCH_ITERATIONS: usize = 1_000;
const MAX_ALLOWED_P95_MS: f64 = 50.0;

/// Machine-readable latency distribution emitted by `grounded_guard_bench` (`08-STEP22` §22.2.3).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroundedBenchSummary {
    /// Total guard-chain iterations executed (`1000`).
    pub iterations: usize,
    /// Median (`P50`) latency in milliseconds.
    pub p50_ms: f64,
    /// 95th-percentile (`P95`) latency in milliseconds (`< 50.0`).
    pub p95_ms: f64,
    /// 99th-percentile (`P99`) latency in milliseconds.
    pub p99_ms: f64,
}

struct GuardBenchFixture {
    catalog: TenantCatalog,
    strict_opts: GuardOptions,
    rows: Vec<Map<String, Value>>,
    expected_fallback: String,
    hmac_key: [u8; 32],
}

#[test]
fn grounded_guard_chain_1000_iterations_p95_under_50ms() -> Result<(), AppError> {
    let fx = build_bench_fixture();
    let mut durations_ns = Vec::with_capacity(BENCH_ITERATIONS);

    for iter_idx in 0..BENCH_ITERATIONS {
        let started = Instant::now();
        run_single_guard_iteration(black_box(&fx), black_box(iter_idx))?;
        let elapsed_ns = u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX);
        durations_ns.push(elapsed_ns);
    }

    durations_ns.sort_unstable();
    let p50_ms = percentile_ms(&durations_ns, 50);
    let p95_ms = percentile_ms(&durations_ns, 95);
    let p99_ms = percentile_ms(&durations_ns, 99);

    let summary = GroundedBenchSummary {
        iterations: BENCH_ITERATIONS,
        p50_ms,
        p95_ms,
        p99_ms,
    };
    emit_bench_summary_line(&summary)?;

    if !summary.p95_ms.is_finite() || summary.p95_ms >= MAX_ALLOWED_P95_MS {
        return Err(AppError::Config(format!(
            "grounding guard P95 latency {}ms exceeded budget {MAX_ALLOWED_P95_MS}ms",
            summary.p95_ms
        )));
    }
    Ok(())
}

fn run_single_guard_iteration(fx: &GuardBenchFixture, iter_idx: usize) -> Result<(), AppError> {
    // 1. SQL AST guard + tenant table rewrite + strict MappingLinkSpec ontology JOIN validation
    let rewritten = guard_and_rewrite(
        "SELECT o.order_id, w.title, o.amount \
         FROM demo_orders o \
         JOIN demo_warehouses w ON o.warehouse_code = w.warehouse_code \
         WHERE o.region = :region ORDER BY o.amount DESC",
        &fx.catalog,
        200,
    )?;
    let legal_join = guard(
        "SELECT o.order_id, w.title, s.supplier_name \
         FROM demo_orders o \
         JOIN demo_warehouses w ON o.warehouse_code = w.warehouse_code \
         JOIN demo_suppliers s ON w.supplier_id = s.supplier_id",
        &fx.catalog,
        &fx.strict_opts,
    )?;
    let ungrounded_join = guard(
        "SELECT o.order_id, c.budget \
         FROM demo_orders o \
         JOIN demo_campaigns c ON o.region = c.region",
        &fx.catalog,
        &fx.strict_opts,
    );
    if rewritten.param_order.len() != 1
        || legal_join.referenced_datasets.len() != 3
        || ungrounded_join.is_ok()
    {
        return Err(AppError::Config(format!(
            "unexpected SQL/JOIN guard state at iteration {iter_idx}"
        )));
    }

    // 2. Numeric cell-grounding verification (drifted summary rejected + grounded summary preserved)
    let drifted_out = verify_or_fallback_summary(
        "华东区域三笔订单心算合计 62,935 元，环比增长 +14.2%。",
        &fx.rows,
    );
    let grounded_out = verify_or_fallback_summary(
        "订单 O-1 金额为 1,284.50 元（40 件），订单 O-2 金额为 33,239.25 元。",
        &fx.rows,
    );
    let grounded_cnt = count_grounded_numbers(&grounded_out);
    if drifted_out != fx.expected_fallback || grounded_cnt != 3 {
        return Err(AppError::Config(format!(
            "unexpected numeric grounding state at iteration {iter_idx}"
        )));
    }

    // 3. Citation bounds verification against 2 recalled hypergraph paragraphs
    let cit_out = verify_and_sanitize_citations(
        "低于安全库存阈值 500 件自动触发补货 [文1]，须经双级签批 [Doc 2] [文8] [Doc 99]。",
        2,
    );
    if cit_out.verified_count() != 2 || cit_out.stripped_phantom_indices.len() != 2 {
        return Err(AppError::Config(format!(
            "unexpected citation guard state at iteration {iter_idx}"
        )));
    }

    // 4. Outbound PII redaction & unprivileged masked restoration
    let (outbound, session) = redact_for_egress(
        "督导手机 13812345678，身份证 11010119900307751X，邮箱 ops@xbluenode.local，请核验订单。",
    );
    let masked = restore_from_egress(&outbound, &session, false, &fx.hmac_key);
    if !analyze_pii_local(&outbound).is_empty()
        || outbound.contains("13812345678")
        || masked.contains("13812345678")
    {
        return Err(AppError::Config(format!(
            "PII leaked during bench iteration {iter_idx}"
        )));
    }

    black_box((
        rewritten,
        legal_join,
        drifted_out,
        grounded_out,
        cit_out,
        masked,
    ));
    Ok(())
}

fn percentile_ms(sorted_ns: &[u64], pct: usize) -> f64 {
    let idx = sorted_ns
        .len()
        .saturating_mul(pct)
        .saturating_sub(1)
        .checked_div(100)
        .unwrap_or(0);
    let ns = sorted_ns.get(idx).copied().unwrap_or(0);
    nanos_to_ms_rounded(ns)
}

fn nanos_to_ms_rounded(ns: u64) -> f64 {
    let secs = u32::try_from(ns / 1_000_000_000).unwrap_or(u32::MAX);
    let sub_ns = u32::try_from(ns % 1_000_000_000).unwrap_or(0);
    let raw_ms = f64::from(secs) * 1_000.0 + f64::from(sub_ns) / 1_000_000.0;
    (raw_ms * 1_000.0).round() / 1_000.0
}

fn emit_bench_summary_line(summary: &GroundedBenchSummary) -> Result<(), AppError> {
    let json = serde_json::to_string(summary)
        .map_err(|e| AppError::Config(format!("serialize GroundedBenchSummary: {e}")))?;
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "GROUNDED_BENCH_JSON={json}")
        .map_err(|e| AppError::Config(format!("stdout write: {e}")))?;
    stdout
        .flush()
        .map_err(|e| AppError::Config(format!("stdout flush: {e}")))?;
    Ok(())
}

fn build_bench_fixture() -> GuardBenchFixture {
    let orders_id = Uuid::from_u128(0xB101);
    let wh_id = Uuid::from_u128(0xB102);
    let sup_id = Uuid::from_u128(0xB103);
    let camp_id = Uuid::from_u128(0xB104);

    let schema_doc = build_bench_schema_doc();
    let allowed_joins: Vec<AllowedDatasetJoinEdge> =
        compile_ontology_join_edges(std::slice::from_ref(&schema_doc));

    let catalog = TenantCatalog::new("bench")
        .with_dataset(bench_ds(
            orders_id,
            "demo_orders",
            &[
                ("order_id", DataType::String),
                ("warehouse_code", DataType::String),
                ("region", DataType::String),
                ("amount", DataType::Float64),
                ("qty", DataType::Int64),
            ],
        ))
        .with_dataset(bench_ds(
            wh_id,
            "demo_warehouses",
            &[
                ("warehouse_code", DataType::String),
                ("supplier_id", DataType::String),
                ("region", DataType::String),
                ("title", DataType::String),
            ],
        ))
        .with_dataset(bench_ds(
            sup_id,
            "demo_suppliers",
            &[
                ("supplier_id", DataType::String),
                ("supplier_name", DataType::String),
            ],
        ))
        .with_dataset(bench_ds(
            camp_id,
            "demo_campaigns",
            &[
                ("campaign_id", DataType::String),
                ("region", DataType::String),
                ("budget", DataType::Float64),
            ],
        ))
        .with_allowed_joins(allowed_joins.clone())
        .with_published_schemas(vec![schema_doc]);

    let strict_opts = GuardOptions::default().with_ontology_joins(true, allowed_joins);
    let rows = build_bench_rows();
    let expected_fallback = fallback_table_summary(rows.len());

    GuardBenchFixture {
        catalog,
        strict_opts,
        rows,
        expected_fallback,
        hmac_key: [0x7B_u8; 32],
    }
}

fn bench_ds(id: Uuid, name: &str, cols: &[(&str, DataType)]) -> CatalogDatasetEntry {
    let mut map = BTreeMap::new();
    for (idx, &(col_name, dt)) in cols.iter().enumerate() {
        let ordinal = i32::try_from(idx.saturating_add(1)).unwrap_or(1);
        map.insert(
            col_name.to_ascii_lowercase(),
            ColumnMeta {
                dataset_id: id,
                column_name: String::from(col_name),
                ordinal,
                data_type: dt,
                pii_category: PiiCategory::None,
                pii_confirmed: false,
                description: String::from(col_name),
                business_term: String::from(col_name),
                unit: String::new(),
            },
        );
    }
    CatalogDatasetEntry {
        id,
        name: String::from(name),
        display_name: String::from(name),
        lake_table: String::from(name),
        schema_version: 1,
        row_count: 3,
        columns: map,
    }
}

fn build_bench_rows() -> Vec<Map<String, Value>> {
    let mk_row = |oid: &str, amt: f64, qty: i64| {
        let mut m = Map::new();
        m.insert(String::from("order_id"), json!(oid));
        m.insert(String::from("amount"), json!(amt));
        m.insert(String::from("qty"), json!(qty));
        m
    };
    vec![
        mk_row("O-1", 1_284.50, 40),
        mk_row("O-2", 33_239.25, 65),
        mk_row("O-5", 28_410.80, 50),
    ]
}

fn build_bench_schema_doc() -> SchemaDoc {
    let str_prop = |name: &str| PropertySpec {
        name: String::from(name),
        display_name: String::from(name),
        data_type: DataType::String,
        kind: PropertyKind::Attribute,
        ..PropertySpec::default()
    };
    SchemaDoc {
        node_types: vec![
            NodeTypeSpec {
                name: String::from("Order"),
                display_name: String::from("Order"),
                properties: vec![str_prop("region")],
                ..NodeTypeSpec::default()
            },
            NodeTypeSpec {
                name: String::from("Warehouse"),
                display_name: String::from("Warehouse"),
                properties: vec![str_prop("title")],
                ..NodeTypeSpec::default()
            },
            NodeTypeSpec {
                name: String::from("Supplier"),
                display_name: String::from("Supplier"),
                properties: vec![str_prop("supplier_name")],
                ..NodeTypeSpec::default()
            },
        ],
        edge_types: vec![
            EdgeTypeSpec {
                name: String::from("FULFILLS"),
                src_type_name: String::from("Warehouse"),
                dst_type_name: String::from("Order"),
                ..EdgeTypeSpec::default()
            },
            EdgeTypeSpec {
                name: String::from("SUPPLIES"),
                src_type_name: String::from("Supplier"),
                dst_type_name: String::from("Warehouse"),
                ..EdgeTypeSpec::default()
            },
        ],
        mappings: build_bench_mappings(),
    }
}

fn build_bench_mappings() -> Vec<MappingSpec> {
    vec![
        MappingSpec {
            name: String::from("orders_map"),
            dataset: String::from("demo_orders"),
            node_type: String::from("Order"),
            key_column: String::from("order_id"),
            title: String::from("{order_id}"),
            filter: None,
            attributes: BTreeMap::new(),
            metrics: BTreeMap::new(),
            metric_time_column: None,
            geo: MappingGeoSpec::default(),
            links: vec![MappingLinkSpec {
                link_type: String::from("FULFILLS"),
                direction: LinkDirection::In,
                target_type: String::from("Warehouse"),
                target_key_column: String::from("warehouse_code"),
                weight_column: None,
                create_missing_target: false,
            }],
            on_source_delete: OnSourceDelete::Keep,
            mode: MappingMode::Full,
        },
        MappingSpec {
            name: String::from("wh_map"),
            dataset: String::from("demo_warehouses"),
            node_type: String::from("Warehouse"),
            key_column: String::from("warehouse_code"),
            title: String::from("{title}"),
            filter: None,
            attributes: BTreeMap::new(),
            metrics: BTreeMap::new(),
            metric_time_column: None,
            geo: MappingGeoSpec::default(),
            links: vec![MappingLinkSpec {
                link_type: String::from("SUPPLIES"),
                direction: LinkDirection::In,
                target_type: String::from("Supplier"),
                target_key_column: String::from("supplier_id"),
                weight_column: None,
                create_missing_target: false,
            }],
            on_source_delete: OnSourceDelete::Keep,
            mode: MappingMode::Full,
        },
        MappingSpec {
            name: String::from("sup_map"),
            dataset: String::from("demo_suppliers"),
            node_type: String::from("Supplier"),
            key_column: String::from("supplier_id"),
            title: String::from("{supplier_name}"),
            filter: None,
            attributes: BTreeMap::new(),
            metrics: BTreeMap::new(),
            metric_time_column: None,
            geo: MappingGeoSpec::default(),
            links: Vec::new(),
            on_source_delete: OnSourceDelete::Keep,
            mode: MappingMode::Full,
        },
    ]
}
