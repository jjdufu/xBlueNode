//! Real PostgreSQL RLS integration tests for `axpoint-db::studio_repo` (`StudioRepository`).

use std::collections::BTreeMap;

use axpoint_db::PgStore;
use axpoint_db::studio_repo::NeighborBatchKey;
use axpoint_domain::AppError;
use axpoint_domain::enums::{
    AggFn, Cardinality, DataType, JobKind, PiiCategory, SubgraphMergeStrategy,
};
use axpoint_domain::ports::StudioRepository as _;
use axpoint_domain::studio::{
    AggRuleSpec, CreateDraftRequest, CreateSubgraphRequest, DerivedRuleSpec, DomainObjectListQuery,
    DraftSource, EdgeTypeSpec, LinkDirection, MappingGeoSpec, MappingLinkSpec, MappingMode,
    MappingSpec, MergePreviewRequest, MergeSubgraphRequest, NodeTypeSpec, OnSourceDelete,
    PropertyKind, PropertySpec, PublishDraftRequest, SchemaDoc,
};
use axpoint_domain::topology::UpsertBlueNodeRequest;
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
        Err(err) if require_db => return Err(Box::new(err)),
        Err(_) => return Ok(None),
    };
    let owner_pool = match PgPoolOptions::new()
        .max_connections(4)
        .connect(&owner_url)
        .await
    {
        Ok(p) => p,
        Err(err) if require_db => return Err(Box::new(err)),
        Err(_) => return Ok(None),
    };

    Ok(Some((app_pool, owner_pool)))
}

fn unique_tenant(prefix: &str) -> String {
    let hex_id = Uuid::new_v4().simple().to_string();
    let short: String = hex_id.chars().take(10).collect();
    format!("{prefix}_{short}")
}

async fn setup_tenant(owner_pool: &PgPool, tenant_id: &str) -> TestResult {
    sqlx::query(
        "INSERT INTO sec_tenants (tenant_id, display_name)
         VALUES ($1, $2)
         ON CONFLICT (tenant_id) DO NOTHING",
    )
    .bind(tenant_id)
    .bind(tenant_id)
    .execute(owner_pool)
    .await?;
    Ok(())
}

async fn cleanup_tenant(owner_pool: &PgPool, tenant_id: &str) -> TestResult {
    for stmt in [
        "DELETE FROM xb_mapping_runs WHERE tenant_id = $1",
        "DELETE FROM xb_mappings WHERE tenant_id = $1",
        "DELETE FROM xb_schema_drafts WHERE tenant_id = $1",
        "DELETE FROM xb_schema_versions WHERE tenant_id = $1",
        "DELETE FROM xb_schema_heads WHERE tenant_id = $1",
        "DELETE FROM xb_links WHERE tenant_id = $1",
        "DELETE FROM xb_metric_series WHERE tenant_id = $1",
        "DELETE FROM xb_nodes WHERE tenant_id = $1",
        "DELETE FROM xb_edge_types WHERE tenant_id = $1",
        "DELETE FROM xb_node_types WHERE tenant_id = $1",
        "DELETE FROM xb_subgraphs WHERE tenant_id = $1",
        "DELETE FROM ds_lineage WHERE tenant_id = $1",
        "DELETE FROM ds_column_meta WHERE tenant_id = $1",
        "DELETE FROM ds_datasets WHERE tenant_id = $1",
        "DELETE FROM ax_jobs WHERE tenant_id = $1",
        "DELETE FROM sec_tenants WHERE tenant_id = $1",
    ] {
        sqlx::query(stmt)
            .bind(tenant_id)
            .execute(owner_pool)
            .await?;
    }
    Ok(())
}

fn prop_attr(name: &str, dt: DataType) -> PropertySpec {
    PropertySpec {
        name: String::from(name),
        display_name: String::from(name),
        description: String::new(),
        unit: None,
        data_type: dt,
        kind: PropertyKind::Attribute,
        required: false,
        pii_category: PiiCategory::None,
        allowed_values: None,
        maturity: axpoint_domain::enums::DesignMaturity::Confirmed,
    }
}

fn prop_metric(name: &str) -> PropertySpec {
    PropertySpec {
        name: String::from(name),
        display_name: String::from(name),
        description: String::new(),
        unit: None,
        data_type: DataType::Float64,
        kind: PropertyKind::Metric,
        required: false,
        pii_category: PiiCategory::None,
        allowed_values: None,
        maturity: axpoint_domain::enums::DesignMaturity::Confirmed,
    }
}

fn base_supply_doc(sync_wh_knowledge: bool) -> SchemaDoc {
    SchemaDoc {
        node_types: vec![
            NodeTypeSpec {
                id: None,
                name: String::from("Warehouse"),
                display_name: String::from("仓库"),
                description: String::from("履约仓库"),
                icon: String::from("warehouse"),
                color: String::from("#2563eb"),
                title_template: String::from("{node_key}"),
                properties: vec![
                    prop_attr("region", DataType::String),
                    prop_metric("inventory"),
                    prop_metric("total_demand"),
                    prop_metric("fill_rate"),
                ],
                agg_rules: vec![AggRuleSpec {
                    target_metric: String::from("total_demand"),
                    op: AggFn::Sum,
                    via_link_type: String::from("FULFILLS"),
                    direction: LinkDirection::Out,
                    source_type_name: String::from("Order"),
                    source_metric: String::from("demand"),
                    filter: None,
                    weight_by_link: false,
                    maturity: axpoint_domain::enums::DesignMaturity::Confirmed,
                }],
                derived_rules: vec![DerivedRuleSpec {
                    target_metric: String::from("fill_rate"),
                    expression: String::from(
                        "if(total_demand > 0, inventory / total_demand, null)",
                    ),
                    enabled: true,
                    maturity: axpoint_domain::enums::DesignMaturity::Confirmed,
                }],
                sync_to_knowledge: sync_wh_knowledge,
                pos_x: 240.0,
                pos_y: 120.0,
                maturity: axpoint_domain::enums::DesignMaturity::Confirmed,
                semantic_layer_tier: None,
                domain_group: None,
                actions: Vec::new(),
            },
            NodeTypeSpec {
                id: None,
                name: String::from("Order"),
                display_name: String::from("订单"),
                description: String::from("客户订单"),
                icon: String::from("shopping-cart"),
                color: String::from("#16a34a"),
                title_template: String::from("{node_key}"),
                properties: vec![prop_attr("status", DataType::String), prop_metric("demand")],
                agg_rules: Vec::new(),
                derived_rules: Vec::new(),
                sync_to_knowledge: false,
                pos_x: 520.0,
                pos_y: 120.0,
                maturity: axpoint_domain::enums::DesignMaturity::Confirmed,
                semantic_layer_tier: None,
                domain_group: None,
                actions: Vec::new(),
            },
        ],
        edge_types: vec![EdgeTypeSpec {
            id: None,
            name: String::from("FULFILLS"),
            src_type_name: String::from("Warehouse"),
            dst_type_name: String::from("Order"),
            impact_weight: 0.8,
            cardinality: Cardinality::OneToMany,
            description: String::new(),
            properties: Vec::new(),
            propagation: serde_json::json!({}),
            maturity: axpoint_domain::enums::DesignMaturity::Confirmed,
            derived_rules: Vec::new(),
            agg_rules: Vec::new(),
        }],
        mappings: Vec::new(),
    }
}

async fn seed_ready_orders_dataset(owner_pool: &PgPool, tenant: &str) -> Result<Uuid, sqlx::Error> {
    sqlx::query_scalar(
        "INSERT INTO ds_datasets (
             tenant_id, name, display_name, source_kind, sync_mode, primary_key,
             lake_schema, lake_table, schema_json, schema_version, row_count, status
         )
         VALUES ($1, 'demo_orders', '订单表', 'SQL_TABLE', 'UPSERT', ARRAY['order_id'],
                 $2, 'demo_orders',
                 '[{\"name\":\"order_id\",\"ordinal\":1,\"data_type\":\"INT64\",\"nullable\":false},
                   {\"name\":\"status\",\"ordinal\":2,\"data_type\":\"STRING\",\"nullable\":false},
                   {\"name\":\"amount\",\"ordinal\":3,\"data_type\":\"DECIMAL\",\"nullable\":false},
                   {\"name\":\"warehouse_code\",\"ordinal\":4,\"data_type\":\"STRING\",\"nullable\":false}]'::jsonb,
                 1, 50, 'READY')
         RETURNING id",
    )
    .bind(tenant)
    .bind(format!("tenant_{tenant}"))
    .fetch_one(owner_pool)
    .await
}

fn shipment_subgraph_doc() -> SchemaDoc {
    SchemaDoc {
        node_types: vec![NodeTypeSpec {
            id: None,
            name: String::from("Shipment"),
            display_name: String::from("运单"),
            description: String::new(),
            icon: String::from("truck"),
            color: String::from("#0ea5e9"),
            title_template: String::from("{node_key}"),
            properties: vec![prop_attr("carrier", DataType::String)],
            agg_rules: Vec::new(),
            derived_rules: Vec::new(),
            sync_to_knowledge: true,
            pos_x: 400.0,
            pos_y: 160.0,
            maturity: axpoint_domain::enums::DesignMaturity::Confirmed,
            semantic_layer_tier: None,
            domain_group: None,
            actions: Vec::new(),
        }],
        edge_types: vec![EdgeTypeSpec {
            id: None,
            name: String::from("SHIPS_ORDER"),
            src_type_name: String::from("Shipment"),
            dst_type_name: String::from("Order"),
            impact_weight: 0.7,
            cardinality: Cardinality::ManyToOne,
            description: String::new(),
            properties: Vec::new(),
            propagation: serde_json::json!({}),
            maturity: axpoint_domain::enums::DesignMaturity::Confirmed,
            derived_rules: Vec::new(),
            agg_rules: Vec::new(),
        }],
        mappings: Vec::new(),
    }
}

fn new_draft_req(title: &str, doc: SchemaDoc) -> CreateDraftRequest {
    CreateDraftRequest {
        title: String::from(title),
        base_version: None,
        schema_doc: doc,
        source: Some(DraftSource::Manual),
        ai_context: None,
        subgraph_slug: None,
    }
}

fn pub_req(summary: &str, force: bool, cascade: bool) -> PublishDraftRequest {
    PublishDraftRequest {
        change_summary: String::from(summary),
        force,
        cascade,
    }
}

#[tokio::test]
async fn studio_repo_optimistic_concurrency_versions_diff_and_rls() -> TestResult {
    let Some((app_pool, owner_pool)) = test_pools().await? else {
        return Ok(());
    };
    let tenant_a = unique_tenant("t_studio_a");
    let tenant_b = unique_tenant("t_studio_b");
    setup_tenant(&owner_pool, &tenant_a).await?;
    setup_tenant(&owner_pool, &tenant_b).await?;

    let store = PgStore::new(app_pool);
    let d1 = store
        .create_draft(
            &tenant_a,
            "main",
            None,
            new_draft_req("D1", base_supply_doc(false)),
        )
        .await?;
    let d2 = store
        .create_draft(
            &tenant_a,
            "main",
            None,
            new_draft_req("D2", base_supply_doc(false)),
        )
        .await?;

    assert!(store.get_draft(&tenant_b, d1.id).await?.is_none());

    let pub1 = store
        .publish_draft(&tenant_a, d1.id, None, pub_req("v1 init", false, false))
        .await?;
    assert_eq!(pub1.version, 1);
    assert_eq!(pub1.aggregate_jobs.len(), 1);

    let conflict = store
        .publish_draft(&tenant_a, d2.id, None, pub_req("stale", false, false))
        .await;
    assert!(matches!(
        conflict,
        Err(AppError::Conflict("VERSION_CONFLICT"))
    ));
    let _ = store.discard_draft(&tenant_a, d2.id).await?;

    let mut doc_v2 = pub1.schema_doc;
    if let Some(wh) = doc_v2.node_types.first_mut() {
        wh.display_name = String::from("核心履约仓");
    }
    let d3 = store
        .create_draft(&tenant_a, "main", None, new_draft_req("D3", doc_v2))
        .await?;
    let pub2 = store
        .publish_draft(&tenant_a, d3.id, None, pub_req("v2 rename", false, false))
        .await?;
    assert_eq!(pub2.version, 2);

    let vdiff = store.diff_schema_versions(&tenant_a, "main", 1, 2).await?;
    assert_eq!(vdiff.node_types.changed.len(), 1);

    let restored = store
        .restore_schema_version(&tenant_a, "main", 1, None)
        .await?;
    assert_eq!(restored.source, DraftSource::Restore);
    assert_eq!(restored.base_version, 2);

    cleanup_tenant(&owner_pool, &tenant_a).await?;
    cleanup_tenant(&owner_pool, &tenant_b).await?;
    Ok(())
}

#[tokio::test]
async fn studio_repo_main_publish_enqueues_mapping_agg_and_knowledge_jobs() -> TestResult {
    let Some((app_pool, owner_pool)) = test_pools().await? else {
        return Ok(());
    };
    let tenant = unique_tenant("t_studio_jobs");
    setup_tenant(&owner_pool, &tenant).await?;
    let ds_id = seed_ready_orders_dataset(&owner_pool, &tenant).await?;

    let store = PgStore::new(app_pool);
    let mut main_doc = base_supply_doc(true);
    main_doc.mappings.push(MappingSpec {
        name: String::from("orders_map"),
        dataset: String::from("demo_orders"),
        node_type: String::from("Order"),
        key_column: String::from("order_id"),
        title: String::from("订单 {order_id}"),
        filter: None,
        attributes: BTreeMap::from([(String::from("status"), String::from("status"))]),
        metrics: BTreeMap::from([(String::from("demand"), String::from("amount"))]),
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
        mode: MappingMode::Incremental,
    });

    let md = store
        .create_draft(&tenant, "main", None, new_draft_req("main v1", main_doc))
        .await?;
    let mpub = store
        .publish_draft(&tenant, md.id, None, pub_req("init main", false, false))
        .await?;
    assert_eq!(mpub.mapping_jobs.len(), 1);
    assert_eq!(mpub.aggregate_jobs.len(), 1);

    let ks_cnt: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM ax_jobs WHERE tenant_id = $1 AND kind = $2")
            .bind(&tenant)
            .bind(JobKind::KnowledgeSync.as_str())
            .fetch_one(&owner_pool)
            .await?;
    assert_eq!(ks_cnt, 1);

    let mappings = store.list_mappings(&tenant, "main").await?;
    assert_eq!(mappings.len(), 1);
    assert_eq!(mappings.first().map(|m| m.dataset_id), Some(ds_id));

    cleanup_tenant(&owner_pool, &tenant).await?;
    Ok(())
}

#[tokio::test]
async fn studio_repo_subgraph_isolation_and_merge_into_main() -> TestResult {
    let Some((app_pool, owner_pool)) = test_pools().await? else {
        return Ok(());
    };
    let tenant = unique_tenant("t_studio_sub");
    setup_tenant(&owner_pool, &tenant).await?;

    let store = PgStore::new(app_pool);
    let md = store
        .create_draft(
            &tenant,
            "main",
            None,
            new_draft_req("main v1", base_supply_doc(false)),
        )
        .await?;
    let _ = store
        .publish_draft(&tenant, md.id, None, pub_req("init main", false, false))
        .await?;

    let sub_info = store
        .create_subgraph(
            &tenant,
            None,
            CreateSubgraphRequest {
                slug: String::from("logistics"),
                display_name: String::from("物流子图"),
                description: String::new(),
                owner_dept: String::from("logistics"),
                seed_from_main: false,
                seed_node_types: None,
                is_dev_sandbox: false,
                source_template_id: None,
            },
        )
        .await?;
    assert!(!sub_info.is_main);

    let sd = store
        .create_draft(
            &tenant,
            "logistics",
            None,
            new_draft_req("ship v1", shipment_subgraph_doc()),
        )
        .await?;
    let spub = store
        .publish_draft(&tenant, sd.id, None, pub_req("sub v1", false, false))
        .await?;
    assert_eq!(spub.mapping_jobs.len(), 0);
    assert_eq!(spub.aggregate_jobs.len(), 0);

    let main_before = store.get_published_schema(&tenant, "main").await?;
    assert!(
        !main_before
            .doc
            .node_types
            .iter()
            .any(|n| n.name == "Shipment")
    );

    let preview = store
        .preview_subgraph_merge(&tenant, "logistics", MergePreviewRequest::default())
        .await?;
    assert!(preview.added_node_types.contains(&String::from("Shipment")));

    let merged_pub = store
        .merge_subgraph_into_main(
            &tenant,
            "logistics",
            None,
            MergeSubgraphRequest {
                base_main_version: Some(1),
                default_strategy: SubgraphMergeStrategy::ExtendProps,
                resolutions: Vec::new(),
                change_summary: String::from("merge logistics"),
                force: false,
            },
        )
        .await?;
    assert_eq!(merged_pub.version, 2);
    assert!(
        merged_pub
            .schema_doc
            .node_types
            .iter()
            .any(|n| n.name == "Shipment")
    );

    cleanup_tenant(&owner_pool, &tenant).await?;
    Ok(())
}

#[tokio::test]
async fn studio_repo_breaking_change_and_batched_domain_neighbors() -> TestResult {
    let Some((app_pool, owner_pool)) = test_pools().await? else {
        return Ok(());
    };
    let tenant = unique_tenant("t_studio_obj");
    setup_tenant(&owner_pool, &tenant).await?;

    let store = PgStore::new(app_pool);
    let d0 = store
        .create_draft(
            &tenant,
            "main",
            None,
            new_draft_req("init", base_supply_doc(false)),
        )
        .await?;
    let _ = store
        .publish_draft(&tenant, d0.id, None, pub_req("v1", false, false))
        .await?;

    let o1 = store
        .upsert_blue_node(
            &tenant,
            UpsertBlueNodeRequest {
                type_name: String::from("Order"),
                node_key: String::from("O-1"),
                title: String::from("订单 O-1"),
                metrics: BTreeMap::from([(String::from("demand"), 120.0)]),
                ..UpsertBlueNodeRequest::default()
            },
        )
        .await?;
    let w1 = store
        .upsert_blue_node(
            &tenant,
            UpsertBlueNodeRequest {
                type_name: String::from("Warehouse"),
                node_key: String::from("W-1"),
                title: String::from("华东仓"),
                metrics: BTreeMap::from([(String::from("inventory"), 500.0)]),
                link_to_node_id: Some(o1.id),
                link_type: Some(String::from("FULFILLS")),
                ..UpsertBlueNodeRequest::default()
            },
        )
        .await?;

    let (wh_list, next_cur) = store
        .query_domain_objects_list(&tenant, "Warehouse", &DomainObjectListQuery::default())
        .await?;
    assert_eq!(wh_list.len(), 1);
    assert!(next_cur.is_none());

    let key = NeighborBatchKey {
        parent_id: w1.id,
        link_type: String::from("FULFILLS"),
        direction: LinkDirection::Out,
        target_type: String::from("Order"),
        limit: 10,
    };
    let batched = store
        .batch_load_neighbors(&tenant, std::slice::from_ref(&key))
        .await?;
    assert_eq!(batched.get(&key).map(Vec::len), Some(1));

    let mut break_doc = base_supply_doc(false);
    break_doc.node_types.retain(|n| n.name != "Order");
    break_doc.edge_types.clear();
    if let Some(wh) = break_doc.node_types.first_mut() {
        wh.agg_rules.clear();
    }
    let db = store
        .create_draft(&tenant, "main", None, new_draft_req("break", break_doc))
        .await?;
    let blocked = store
        .publish_draft(&tenant, db.id, None, pub_req("blocked", false, false))
        .await;
    assert!(matches!(
        blocked,
        Err(AppError::Conflict("SCHEMA_BREAKING_CHANGE"))
    ));

    let forced = store
        .publish_draft(&tenant, db.id, None, pub_req("forced", true, true))
        .await?;
    assert_eq!(forced.version, 2);
    assert!(
        store
            .get_domain_object_by_key(&tenant, "Order", "O-1")
            .await?
            .is_none()
    );

    cleanup_tenant(&owner_pool, &tenant).await?;
    Ok(())
}
