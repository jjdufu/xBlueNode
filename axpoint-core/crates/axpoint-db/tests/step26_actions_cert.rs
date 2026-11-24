//! Real PostgreSQL RLS integration tests for Step 26 `ActionSpec` execution, 4-path edge dynamics,
//! 3-track metric explainability, and L4 `Trial Tick` runtime readiness certification.

use std::collections::BTreeMap;

use axpoint_db::{PgStore, begin_tenant_tx};
use axpoint_domain::AppError;
use axpoint_domain::enums::{
    ActionSideEffectTier, AggFn, Cardinality, DataType, DesignMaturity, PiiCategory,
    PropagationPathKind, RuntimeCertLevel,
};
use axpoint_domain::graph::{LinkMetricPointInput, UpsertLinkRequest, WriteLinkMetricsRequest};
use axpoint_domain::ports::{
    GraphStorePort as _, MetricStorePort as _, StudioExecutionPort as _, StudioRepository as _,
};
use axpoint_domain::studio::{
    ActionParamSpec, ActionSpec, AggRuleSpec, CertifySubgraphRequest, CreateSubgraphRequest,
    DerivedRuleSpec, EdgeDerivedRuleSpec, EdgeTypeSpec, InvokeObjectActionRequest, LinkDirection,
    MutationStepSpec, NodeTypeSpec, PropertyKind, PropertySpec, SaveSchemaDirectRequest, SchemaDoc,
};
use axpoint_domain::topology::UpsertBlueNodeRequest;
use chrono::Utc;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const DEFAULT_APP_URL: &str = "postgres://axpoint_app:axpoint_app_secret@127.0.0.1:5432/xbluenode";
const DEFAULT_OWNER_URL: &str = "postgres://axpoint:axpoint_secret@127.0.0.1:5432/xbluenode";

async fn test_pools() -> Result<Option<(PgPool, PgPool)>, Box<dyn std::error::Error>> {
    let require_db = std::env::var("AXPOINT_REQUIRE_DB_TESTS")
        .is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"));
    let app_url = std::env::var("DATABASE_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| String::from(DEFAULT_APP_URL));
    let owner_url = std::env::var("MIGRATION_DATABASE_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| String::from(DEFAULT_OWNER_URL));

    let app_pool = match PgPoolOptions::new()
        .max_connections(6)
        .connect(&app_url)
        .await
    {
        Ok(p) => p,
        Err(e) if require_db => return Err(Box::new(e)),
        Err(_) => return Ok(None),
    };
    let owner_pool = match PgPoolOptions::new()
        .max_connections(4)
        .connect(&owner_url)
        .await
    {
        Ok(p) => p,
        Err(e) if require_db => return Err(Box::new(e)),
        Err(_) => return Ok(None),
    };
    Ok(Some((app_pool, owner_pool)))
}

fn unique_tenant(prefix: &str) -> String {
    let short: String = Uuid::new_v4()
        .simple()
        .to_string()
        .chars()
        .take(10)
        .collect();
    format!("{prefix}_{short}")
}

async fn setup_tenant(owner_pool: &PgPool, tenant_id: &str) -> TestResult {
    sqlx::query(
        "INSERT INTO sec_tenants (tenant_id, display_name)
         VALUES ($1, $2) ON CONFLICT (tenant_id) DO NOTHING",
    )
    .bind(tenant_id)
    .bind(tenant_id)
    .execute(owner_pool)
    .await?;
    Ok(())
}

async fn cleanup_tenant(owner_pool: &PgPool, tenant_id: &str) -> TestResult {
    for stmt in [
        "DELETE FROM xb_runtime_certifications WHERE tenant_id = $1",
        "DELETE FROM xb_link_metric_series WHERE tenant_id = $1",
        "DELETE FROM ax_approvals WHERE tenant_id = $1",
        "DELETE FROM ax_action_log WHERE tenant_id = $1",
        "DELETE FROM ax_outbox WHERE tenant_id = $1",
        "DELETE FROM ax_jobs WHERE tenant_id = $1",
        "DELETE FROM xb_schema_drafts WHERE tenant_id = $1",
        "DELETE FROM xb_schema_versions WHERE tenant_id = $1",
        "DELETE FROM xb_schema_heads WHERE tenant_id = $1",
        "DELETE FROM xb_links WHERE tenant_id = $1",
        "DELETE FROM xb_metric_series WHERE tenant_id = $1",
        "DELETE FROM xb_nodes WHERE tenant_id = $1",
        "DELETE FROM xb_edge_types WHERE tenant_id = $1",
        "DELETE FROM xb_node_types WHERE tenant_id = $1",
        "DELETE FROM xb_subgraphs WHERE tenant_id = $1",
        "DELETE FROM sec_tenants WHERE tenant_id = $1",
    ] {
        sqlx::query(stmt)
            .bind(tenant_id)
            .execute(owner_pool)
            .await?;
    }
    Ok(())
}

fn metric_prop(name: &str) -> PropertySpec {
    PropertySpec {
        name: String::from(name),
        display_name: String::from(name),
        description: String::new(),
        unit: Some(String::from("u")),
        data_type: DataType::Float64,
        kind: PropertyKind::Metric,
        required: false,
        pii_category: PiiCategory::None,
        allowed_values: None,
        maturity: DesignMaturity::Confirmed,
    }
}

fn step26_hub_depot_doc() -> SchemaDoc {
    SchemaDoc {
        node_types: vec![
            NodeTypeSpec {
                name: String::from("Step26Hub"),
                display_name: String::from("调拨枢纽"),
                properties: vec![metric_prop("output_rate"), metric_prop("reserve_qty")],
                actions: vec![
                    ActionSpec {
                        name: String::from("dispatch_reserve"),
                        display_name: String::from("紧急调拨"),
                        parameters: vec![ActionParamSpec {
                            name: String::from("qty"),
                            display_name: String::from("调拨量"),
                            data_type: DataType::Float64,
                            required: true,
                            default_value: None,
                            description: String::new(),
                        }],
                        precondition_expr: Some(String::from(
                            "reserve_qty >= param.qty and param.qty > 0",
                        )),
                        side_effect_tier: ActionSideEffectTier::SelfOnly,
                        mutations: vec![
                            MutationStepSpec {
                                target: String::from("self.reserve_qty"),
                                expression: String::from("reserve_qty - param.qty"),
                                filter: None,
                            },
                            MutationStepSpec {
                                target: String::from("self.output_rate"),
                                expression: String::from("output_rate + param.qty"),
                                filter: None,
                            },
                        ],
                        ..ActionSpec::default()
                    },
                    ActionSpec {
                        name: String::from("emergency_boost_lane"),
                        display_name: String::from("审批增补在途"),
                        parameters: vec![ActionParamSpec {
                            name: String::from("boost"),
                            display_name: String::from("增量"),
                            data_type: DataType::Float64,
                            required: true,
                            default_value: None,
                            description: String::new(),
                        }],
                        side_effect_tier: ActionSideEffectTier::RelatedNodes,
                        require_approval: true,
                        mutations: vec![MutationStepSpec {
                            target: String::from("edge(STEP26_LANE, OUT).in_transit_qty"),
                            expression: String::from("in_transit_qty + param.boost"),
                            filter: None,
                        }],
                        ..ActionSpec::default()
                    },
                ],
                ..NodeTypeSpec::default()
            },
            NodeTypeSpec {
                name: String::from("Step26Depot"),
                display_name: String::from("区域前置仓"),
                properties: vec![metric_prop("congestion_ratio"), metric_prop("inbound_flux")],
                agg_rules: vec![AggRuleSpec {
                    target_metric: String::from("inbound_flux"),
                    op: AggFn::Sum,
                    via_link_type: String::from("STEP26_LANE"),
                    direction: LinkDirection::In,
                    source_type_name: String::from("STEP26_LANE"),
                    source_metric: String::from("effective_flux"),
                    filter: None,
                    weight_by_link: false,
                    maturity: DesignMaturity::Confirmed,
                }],
                ..NodeTypeSpec::default()
            },
        ],
        edge_types: vec![EdgeTypeSpec {
            name: String::from("STEP26_LANE"),
            src_type_name: String::from("Step26Hub"),
            dst_type_name: String::from("Step26Depot"),
            impact_weight: 0.8,
            cardinality: Cardinality::OneToMany,
            properties: vec![metric_prop("in_transit_qty"), metric_prop("effective_flux")],
            derived_rules: vec![EdgeDerivedRuleSpec {
                target_metric: String::from("effective_flux"),
                expression: String::from(
                    "src.output_rate * (1.0 - dst.congestion_ratio) + in_transit_qty",
                ),
                path_kind: PropagationPathKind::VertexToEdge,
                enabled: true,
                maturity: DesignMaturity::Confirmed,
            }],
            ..EdgeTypeSpec::default()
        }],
        mappings: Vec::new(),
    }
}

async fn seed_hub_and_depot(
    store: &PgStore,
    tenant_a: &str,
) -> Result<(i64, Uuid, Uuid), AppError> {
    let _ = store
        .save_schema_direct(
            tenant_a,
            "main",
            None,
            SaveSchemaDirectRequest {
                doc: step26_hub_depot_doc(),
                base_version: None,
                change_summary: Some(String::from("step26 schema")),
                force: true,
                cascade: false,
            },
        )
        .await?;

    let hub = store
        .upsert_node(
            tenant_a,
            UpsertBlueNodeRequest {
                type_name: String::from("Step26Hub"),
                node_key: String::from("H-1"),
                title: String::from("华东枢纽"),
                metrics: BTreeMap::from([
                    (String::from("output_rate"), 100.0),
                    (String::from("reserve_qty"), 50.0),
                ]),
                ..UpsertBlueNodeRequest::default()
            },
        )
        .await?;
    let depot = store
        .upsert_node(
            tenant_a,
            UpsertBlueNodeRequest {
                type_name: String::from("Step26Depot"),
                node_key: String::from("D-1"),
                title: String::from("苏州仓"),
                metrics: BTreeMap::from([
                    (String::from("congestion_ratio"), 0.2),
                    (String::from("inbound_flux"), 0.0),
                ]),
                ..UpsertBlueNodeRequest::default()
            },
        )
        .await?;
    let lane = store
        .upsert_link(
            tenant_a,
            UpsertLinkRequest {
                link_type: String::from("STEP26_LANE"),
                src_node_id: hub.id,
                dst_node_id: depot.id,
                weight: Some(0.8),
                properties: serde_json::Map::new(),
                metrics: BTreeMap::new(),
                valid_from: None,
                valid_to: None,
            },
        )
        .await?;
    Ok((hub.row_version, depot.id, lane.id))
}

#[tokio::test]
async fn step26_action_cas_precondition_idempotency_and_4path() -> TestResult {
    let Some((app_pool, owner_pool)) = test_pools().await? else {
        return Ok(());
    };
    let tenant_a = unique_tenant("step26_act");
    setup_tenant(&owner_pool, &tenant_a).await?;
    let store = PgStore::new(app_pool);
    let (hub_ver, depot_id, lane_id) = seed_hub_and_depot(&store, &tenant_a).await?;

    let pre_err = store
        .invoke_object_action(
            &tenant_a,
            None,
            "Step26Hub",
            "H-1",
            "dispatch_reserve",
            InvokeObjectActionRequest {
                inputs: serde_json::json!({ "qty": 999.0 }),
                idempotency_key: String::from("idem-pre-fail"),
                expected_version: None,
                dry_run: false,
            },
        )
        .await;
    assert!(matches!(
        pre_err,
        Err(AppError::Unprocessable {
            code: "ACTION_PRECONDITION_FAILED",
            ..
        })
    ));

    let cas_err = store
        .invoke_object_action(
            &tenant_a,
            None,
            "Step26Hub",
            "H-1",
            "dispatch_reserve",
            InvokeObjectActionRequest {
                inputs: serde_json::json!({ "qty": 10.0 }),
                idempotency_key: String::from("idem-cas-fail"),
                expected_version: Some(999),
                dry_run: false,
            },
        )
        .await;
    assert!(matches!(
        cas_err,
        Err(AppError::Conflict("VERSION_CONFLICT"))
    ));

    let ok1 = store
        .invoke_object_action(
            &tenant_a,
            None,
            "Step26Hub",
            "H-1",
            "dispatch_reserve",
            InvokeObjectActionRequest {
                inputs: serde_json::json!({ "qty": 20.0 }),
                idempotency_key: String::from("idem-ok-1"),
                expected_version: Some(hub_ver),
                dry_run: false,
            },
        )
        .await?;
    assert_eq!(ok1.status, "APPLIED");
    assert_eq!(
        ok1.node
            .as_ref()
            .and_then(|n| n.metrics.get("output_rate"))
            .copied(),
        Some(120.0)
    );

    let replay = store
        .invoke_object_action(
            &tenant_a,
            None,
            "Step26Hub",
            "H-1",
            "dispatch_reserve",
            InvokeObjectActionRequest {
                inputs: serde_json::json!({ "qty": 20.0 }),
                idempotency_key: String::from("idem-ok-1"),
                expected_version: None,
                dry_run: false,
            },
        )
        .await?;
    assert!(replay.idempotency_replay);
    assert_eq!(replay.action_log_id, ok1.action_log_id);

    verify_approval_and_four_path_explain(&store, &tenant_a, lane_id, depot_id).await?;
    cleanup_tenant(&owner_pool, &tenant_a).await?;
    Ok(())
}

async fn verify_approval_and_four_path_explain(
    store: &PgStore,
    tenant_a: &str,
    lane_id: Uuid,
    depot_id: Uuid,
) -> TestResult {
    let pending = store
        .invoke_object_action(
            tenant_a,
            None,
            "Step26Hub",
            "H-1",
            "emergency_boost_lane",
            InvokeObjectActionRequest {
                inputs: serde_json::json!({ "boost": 5.0 }),
                idempotency_key: String::from("idem-approval-1"),
                expected_version: None,
                dry_run: false,
            },
        )
        .await?;
    assert_eq!(pending.status, "WAITING_APPROVAL");
    assert!(pending.approval_id.is_some());

    let ingest = store
        .write_link_metrics(
            tenant_a,
            WriteLinkMetricsRequest {
                points: vec![LinkMetricPointInput {
                    link_id: lane_id,
                    metric: String::from("in_transit_qty"),
                    ts: Utc::now(),
                    value: Some(16.0),
                    value_text: None,
                }],
                source_tag: None,
            },
        )
        .await?;
    assert_eq!(ingest.written, 1);

    let link_exp = store
        .explain_link_metric(tenant_a, lane_id, "effective_flux")
        .await?;
    assert_eq!(link_exp.value, Some(112.0));
    assert_ne!(link_exp.rule_chain.len(), 0);

    let node_exp = store
        .explain_node_metric(tenant_a, depot_id, "inbound_flux")
        .await?;
    assert_eq!(node_exp.value, Some(112.0));
    assert!(node_exp.rule_chain.len() >= 2);
    Ok(())
}

fn clean_sub_doc() -> SchemaDoc {
    SchemaDoc {
        node_types: vec![NodeTypeSpec {
            name: String::from("CleanNode"),
            display_name: String::from("认证节点"),
            description: String::from("无死指标节点"),
            properties: vec![metric_prop("base_load"), metric_prop("headroom")],
            derived_rules: vec![DerivedRuleSpec {
                target_metric: String::from("headroom"),
                expression: String::from("100 - base_load"),
                enabled: true,
                maturity: DesignMaturity::Confirmed,
            }],
            actions: vec![ActionSpec {
                name: String::from("add_load"),
                display_name: String::from("加载"),
                parameters: vec![ActionParamSpec {
                    name: String::from("delta"),
                    display_name: String::from("增量"),
                    data_type: DataType::Float64,
                    required: true,
                    default_value: None,
                    description: String::new(),
                }],
                precondition_expr: Some(String::from("headroom >= param.delta")),
                side_effect_tier: ActionSideEffectTier::SelfOnly,
                mutations: vec![MutationStepSpec {
                    target: String::from("self.base_load"),
                    expression: String::from("base_load + param.delta"),
                    filter: None,
                }],
                ..ActionSpec::default()
            }],
            ..NodeTypeSpec::default()
        }],
        edge_types: Vec::new(),
        mappings: Vec::new(),
    }
}

#[tokio::test]
async fn step26_trial_tick_savepoint_and_rls_isolation() -> TestResult {
    let Some((app_pool, owner_pool)) = test_pools().await? else {
        return Ok(());
    };
    let tenant_a = unique_tenant("step26_cert_a");
    let tenant_b = unique_tenant("step26_cert_b");
    setup_tenant(&owner_pool, &tenant_a).await?;
    setup_tenant(&owner_pool, &tenant_b).await?;
    let store = PgStore::new(app_pool.clone());

    let sub = store
        .create_subgraph(
            &tenant_a,
            None,
            CreateSubgraphRequest {
                slug: String::from("trial_s26"),
                display_name: String::from("认证沙盒"),
                description: String::new(),
                owner_dept: String::new(),
                seed_from_main: false,
                seed_node_types: None,
                is_dev_sandbox: true,
                source_template_id: None,
            },
        )
        .await?;

    let _ = store
        .save_schema_direct(
            &tenant_a,
            &sub.slug,
            None,
            SaveSchemaDirectRequest {
                doc: clean_sub_doc(),
                base_version: None,
                change_summary: None,
                force: true,
                cascade: false,
            },
        )
        .await?;

    let cert = store
        .certify_subgraph_runtime(
            &tenant_a,
            &sub.slug,
            None,
            CertifySubgraphRequest::default(),
        )
        .await?;
    assert!(cert.trial_tick_passed);
    assert_eq!(cert.cert_level, RuntimeCertLevel::L4Certified);
    assert_eq!(cert.dead_knowledge_cnt, 0);

    // Verify SAVEPOINT trial_tick rolled back all synthetic nodes
    let mut tx_a = begin_tenant_tx(&app_pool, &tenant_a).await?;
    let leftover_nodes: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM xb_nodes WHERE subgraph_slug = 'trial_s26'")
            .fetch_one(&mut *tx_a)
            .await?;
    tx_a.commit().await?;
    assert_eq!(leftover_nodes, 0);

    // Verify Tenant B sees 0 rows in xb_link_metric_series and xb_runtime_certifications
    let mut tx_b = begin_tenant_tx(&app_pool, &tenant_b).await?;
    let b_series: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM xb_link_metric_series")
        .fetch_one(&mut *tx_b)
        .await?;
    let b_certs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM xb_runtime_certifications")
        .fetch_one(&mut *tx_b)
        .await?;
    tx_b.commit().await?;
    assert_eq!(b_series, 0);
    assert_eq!(b_certs, 0);

    cleanup_tenant(&owner_pool, &tenant_a).await?;
    cleanup_tenant(&owner_pool, &tenant_b).await?;
    Ok(())
}
