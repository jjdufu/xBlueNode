//! PostgreSQL integration tests for Step 02 (`AxHub` repositories) and Step 03 (`AxGraph` nodes,
//! time-series rollups, entity linking, graph traversal, and `petgraph` algorithms).

use std::collections::BTreeMap;

use axpoint_db::PgStore;
use axpoint_domain::AppError;
use axpoint_domain::axhub::{
    ColumnProfile, CreateDatasetRequest, CreateGlossaryTermRequest, CreateWebhookRequest,
    NewConnectionInput, QualityReport, SchemaChange, SchemaPolicy, SealedCredential,
    SidecarSyncResult, UpdateColumnMetaRequest, WebhookAuthScheme,
};
use axpoint_domain::enums::{
    AggFn, ConnectorType, DataType, GraphAlgo, MetricSourceTag, PiiCategory, SourceKind, SyncMode,
};
use axpoint_domain::graph::{
    AlgoResultsQuery, AlgoRunRequest, BulkLinkInput, BulkNodeInput, BulkNodesRequest, GeoPoint,
    LinkEndpointRef, MetricPointInput, MetricQueryGrain, MetricsMerge, NeighborQuery,
    NodeListQuery, NodeMetricSeriesQuery, NodeRef, NodeSearchQuery, PatchNodeRequest, PathQuery,
    ResolveEntitiesRequest, TraversalDirection, UpsertLinkRequest, WindowGroupBy, WindowKind,
    WindowQuery, WindowSpec, WriteMetricsRequest,
};
use axpoint_domain::ports::{
    ConnectionRepository, DatasetRepository, GraphQueryPort, GraphStorePort, MetricStorePort,
};
use axpoint_domain::topology::{
    BlueEdgeTypeInput, BlueNodeTypeInput, SaveXBlueSchemaRequest, UpsertBlueNodeRequest,
};
use chrono::{Datelike, Duration, TimeZone, Utc};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

type TestResult = Result<(), Box<dyn std::error::Error>>;

async fn test_pools() -> Result<Option<(PgPool, PgPool)>, Box<dyn std::error::Error>> {
    let require_db = std::env::var("AXPOINT_REQUIRE_DB_TESTS")
        .is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"));

    let app_url = match std::env::var("DATABASE_URL") {
        Ok(url) if !url.trim().is_empty() => url,
        _ if require_db => {
            return Err("DATABASE_URL must be set when AXPOINT_REQUIRE_DB_TESTS=1".into());
        }
        _ => return Ok(None),
    };

    let owner_url = match std::env::var("MIGRATION_DATABASE_URL") {
        Ok(url) if !url.trim().is_empty() => url,
        _ => app_url.replace("axpoint_app:axpoint_app_secret", "axpoint:axpoint_secret"),
    };

    let app_pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(&app_url)
        .await?;
    let owner_pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&owner_url)
        .await?;

    Ok(Some((app_pool, owner_pool)))
}

async fn ensure_tenant(owner_pool: &PgPool, tenant_id: &str) -> TestResult {
    sqlx::query(
        "INSERT INTO sec_tenants (tenant_id, display_name, timezone)
         VALUES ($1, $2, 'Asia/Shanghai')
         ON CONFLICT (tenant_id) DO NOTHING",
    )
    .bind(tenant_id)
    .bind(tenant_id)
    .execute(owner_pool)
    .await?;
    Ok(())
}

fn unique_tenant(prefix: &str) -> String {
    let hex_id = Uuid::new_v4().simple().to_string();
    let short: String = hex_id.chars().take(10).collect();
    format!("{prefix}_{short}")
}

async fn cleanup_tenants(owner_pool: &PgPool, tenants: &[String]) -> TestResult {
    for sql in [
        "DELETE FROM ds_webhook_buffer WHERE tenant_id = ANY($1)",
        "DELETE FROM ds_webhooks WHERE tenant_id = ANY($1)",
        "DELETE FROM ds_sync_jobs WHERE tenant_id = ANY($1)",
        "DELETE FROM ds_quality_rules WHERE tenant_id = ANY($1)",
        "DELETE FROM ds_column_meta WHERE tenant_id = ANY($1)",
        "DELETE FROM ds_stream_offsets WHERE tenant_id = ANY($1)",
        "DELETE FROM ds_datasets WHERE tenant_id = ANY($1)",
        "DELETE FROM ds_connections WHERE tenant_id = ANY($1)",
        "DELETE FROM ds_credentials WHERE tenant_id = ANY($1)",
        "DELETE FROM ds_uploads WHERE tenant_id = ANY($1)",
        "DELETE FROM ds_glossary_terms WHERE tenant_id = ANY($1)",
        "DELETE FROM ds_lineage WHERE tenant_id = ANY($1)",
        "DELETE FROM xb_graph_algo_results WHERE tenant_id = ANY($1)",
        "DELETE FROM xb_entity_links WHERE tenant_id = ANY($1)",
        "DELETE FROM xb_rollup_dirty WHERE tenant_id = ANY($1)",
        "DELETE FROM xb_rollup_watermarks WHERE tenant_id = ANY($1)",
        "DELETE FROM xb_metric_rollups WHERE tenant_id = ANY($1)",
        "DELETE FROM xb_metric_series WHERE tenant_id = ANY($1)",
        "DELETE FROM xb_links WHERE tenant_id = ANY($1)",
        "DELETE FROM xb_nodes WHERE tenant_id = ANY($1)",
        "DELETE FROM xb_edge_types WHERE tenant_id = ANY($1)",
        "DELETE FROM xb_node_types WHERE tenant_id = ANY($1)",
        "DELETE FROM hg_embeddings WHERE tenant_id = ANY($1)",
        "DELETE FROM hg_edges WHERE tenant_id = ANY($1)",
        "DELETE FROM hg_paras WHERE tenant_id = ANY($1)",
        "DELETE FROM hg_entities WHERE tenant_id = ANY($1)",
        "DELETE FROM hg_docs WHERE tenant_id = ANY($1)",
        "DELETE FROM ax_jobs WHERE tenant_id = ANY($1)",
        "DELETE FROM ax_outbox WHERE tenant_id = ANY($1)",
        "DELETE FROM sec_audit_logs WHERE tenant_id = ANY($1)",
    ] {
        sqlx::query(sql).bind(tenants).execute(owner_pool).await?;
    }
    Ok(())
}

async fn setup_axhub_conn_and_dataset(
    store: &PgStore,
    t1: &str,
    t2: &str,
) -> Result<(Uuid, Uuid), Box<dyn std::error::Error>> {
    let sealed = SealedCredential {
        id: Uuid::new_v4(),
        key_version: 1,
        nonce: vec![1_u8; 12],
        ciphertext: vec![9_u8; 32],
    };
    let conn = store
        .create_connection(
            t1,
            NewConnectionInput {
                created_by: None,
                name: String::from("demo_pg"),
                connector_type: ConnectorType::Postgres,
                config: serde_json::json!({"host": "127.0.0.1", "port": 5432}),
                allow_private_network: true,
                credential: Some(sealed),
            },
        )
        .await?;

    let dup_conn = store
        .create_connection(
            t1,
            NewConnectionInput {
                created_by: None,
                name: String::from("demo_pg"),
                connector_type: ConnectorType::Postgres,
                config: serde_json::json!({}),
                allow_private_network: false,
                credential: None,
            },
        )
        .await;
    assert!(matches!(dup_conn, Err(AppError::Conflict(_))));
    assert!(store.get_connection(t2, conn.id).await?.is_none());

    let bad_ds = store
        .create_dataset(
            t1,
            None,
            CreateDatasetRequest {
                connection_id: Some(conn.id),
                name: String::from("orders_bad"),
                display_name: String::from("Orders Bad"),
                description: String::new(),
                source_kind: SourceKind::SqlTable,
                source_ref: serde_json::json!({"table": "orders"}),
                sync_mode: SyncMode::Upsert,
                primary_key: Vec::new(),
                cursor_column: None,
                delete_column: None,
                schema_policy: SchemaPolicy::Additive,
                schedule_cron: None,
                tags: Vec::new(),
            },
        )
        .await;
    assert!(matches!(bad_ds, Err(AppError::BadRequest { .. })));

    let ds = store
        .create_dataset(
            t1,
            None,
            CreateDatasetRequest {
                connection_id: Some(conn.id),
                name: String::from("orders_fact"),
                display_name: String::from("销售订单"),
                description: String::from("核心履约订单明细"),
                source_kind: SourceKind::SqlTable,
                source_ref: serde_json::json!({"table": "orders"}),
                sync_mode: SyncMode::Snapshot,
                primary_key: vec![String::from("order_id")],
                cursor_column: None,
                delete_column: None,
                schema_policy: SchemaPolicy::Additive,
                schedule_cron: None,
                tags: vec![String::from("supply_chain")],
            },
        )
        .await?;

    assert!(matches!(
        store.delete_connection(t1, conn.id).await,
        Err(AppError::Conflict(_))
    ));
    Ok((conn.id, ds.id))
}

async fn verify_axhub_sync_and_catalog(store: &PgStore, t1: &str, ds_id: Uuid) -> TestResult {
    let (enq_a, enq_b) = tokio::join!(
        store.enqueue_sync_job(t1, ds_id),
        store.enqueue_sync_job(t1, ds_id),
    );
    let r_a = enq_a?;
    let r_b = enq_b?;
    assert_eq!(r_a.job_id, r_b.job_id);
    assert!(r_a.existing ^ r_b.existing);

    let sj1 = store.start_sync_execution(t1, ds_id, r_a.job_id).await?;
    let sj2 = store.start_sync_execution(t1, ds_id, r_a.job_id).await?;
    assert_eq!(sj1.batch_id, sj2.batch_id);

    let sync_res = SidecarSyncResult {
        dataset_id: ds_id.to_string(),
        rows_read: 100,
        rows_written: 100,
        rows_deleted: 0,
        total_rows: None,
        cursor_to: Some(String::from("2026-01-01T00:00:00Z")),
        stream_offsets: BTreeMap::new(),
        snapshot_id: 1,
        schema: vec![ColumnProfile {
            name: String::from("customer_phone"),
            ordinal: 1,
            data_type: DataType::String,
            nullable: true,
            null_count: 0,
            null_ratio: 0.0,
            distinct_estimate: 90,
            sample_values: Vec::new(),
            pii_category: PiiCategory::Phone,
            pii_hit_ratio: 0.95,
            source_comment: String::from("联系电话"),
        }],
        schema_change: SchemaChange::default(),
        quality: QualityReport::default(),
    };
    store
        .complete_sync_execution(t1, sj1.id, ds_id, &sync_res)
        .await?;

    store
        .update_column_meta(
            t1,
            ds_id,
            "customer_phone",
            UpdateColumnMetaRequest {
                description: Some(String::from("人工确认字段说明")),
                business_term: Some(String::from("客户手机号")),
                unit: None,
                pii_category: Some(PiiCategory::None),
                pii_confirmed: Some(true),
            },
        )
        .await?;

    store
        .complete_sync_execution(t1, sj1.id, ds_id, &sync_res)
        .await?;
    let cols = store.list_columns(t1, ds_id).await?;
    let phone_col = cols.first().ok_or("missing customer_phone column meta")?;
    assert_eq!(phone_col.pii_category, PiiCategory::None);
    assert!(phone_col.pii_confirmed);
    assert_eq!(phone_col.description, "人工确认字段说明");

    store
        .create_glossary_term(
            t1,
            CreateGlossaryTermRequest {
                term: String::from("订单"),
                definition: String::from("客户履约单据"),
                synonyms: vec![String::from("SO单"), String::from("履约单")],
            },
        )
        .await?;
    let cat_hits = store.search_catalog(t1, "SO单", 20).await?;
    assert!(cat_hits.items.iter().any(|h| h.id == ds_id));
    assert_eq!(store.get_dataset_lineage(t1, ds_id, 3).await?.len(), 1);
    Ok(())
}

async fn verify_axhub_webhooks(store: &PgStore, t1: &str) -> TestResult {
    let wh_cred = SealedCredential {
        id: Uuid::new_v4(),
        key_version: 1,
        nonce: vec![2_u8; 12],
        ciphertext: vec![8_u8; 32],
    };
    let wh_req = CreateWebhookRequest {
        dataset_name: String::from("wh_events"),
        slug: String::from("demo-events"),
        auth_scheme: WebhookAuthScheme::Bearer,
        record_path: String::from("$"),
        max_body_bytes: 65_536,
    };
    let (wh, _wh_ds) = store.create_webhook(t1, None, &wh_req, wh_cred).await?;

    let resolved = store
        .resolve_webhook(t1, "demo-events")
        .await?
        .ok_or("webhook not resolved")?;
    assert_eq!(resolved.webhook_id, wh.id);

    let first_ing = store
        .ingest_webhook_event(t1, wh.id, "deliv-1", serde_json::json!({"k": 1}))
        .await?;
    let dup_ing = store
        .ingest_webhook_event(t1, wh.id, "deliv-1", serde_json::json!({"k": 1}))
        .await?;
    assert!(first_ing);
    assert!(!dup_ing);
    Ok(())
}

#[tokio::test]
async fn axhub_connections_datasets_single_flight_webhooks_and_rls() -> TestResult {
    let Some((app_pool, owner_pool)) = test_pools().await? else {
        return Ok(());
    };

    let t1 = unique_tenant("t_hub1");
    let t2 = unique_tenant("t_hub2");
    ensure_tenant(&owner_pool, &t1).await?;
    ensure_tenant(&owner_pool, &t2).await?;

    let store = PgStore::new(app_pool);
    let (_conn_id, ds_id) = setup_axhub_conn_and_dataset(&store, &t1, &t2).await?;
    verify_axhub_sync_and_catalog(&store, &t1, ds_id).await?;
    verify_axhub_webhooks(&store, &t1).await?;

    cleanup_tenants(&owner_pool, &[t1, t2]).await?;
    Ok(())
}

async fn setup_axgraph_nodes_and_bulk(
    store: &PgStore,
    tenant: &str,
) -> Result<Uuid, Box<dyn std::error::Error>> {
    let bad_node = store
        .upsert_node(
            tenant,
            UpsertBlueNodeRequest {
                type_name: String::from("UndefinedType"),
                node_key: String::from("X-1"),
                title: String::from("Invalid"),
                ..UpsertBlueNodeRequest::default()
            },
        )
        .await;
    assert!(matches!(
        bad_node,
        Err(AppError::BadRequest {
            code: "SCHEMA_INVALID",
            ..
        })
    ));

    store
        .save_xblue_schema(
            tenant,
            SaveXBlueSchemaRequest {
                node_types: vec![
                    BlueNodeTypeInput {
                        name: String::from("Factory"),
                        display_name: String::from("工厂"),
                        properties: Vec::new(),
                        agg_rules: Vec::new(),
                        pos_x: 0.0,
                        pos_y: 0.0,
                    },
                    BlueNodeTypeInput {
                        name: String::from("Warehouse"),
                        display_name: String::from("仓库"),
                        properties: Vec::new(),
                        agg_rules: Vec::new(),
                        pos_x: 100.0,
                        pos_y: 0.0,
                    },
                ],
                edge_types: vec![BlueEdgeTypeInput {
                    name: String::from("SUPPLIES"),
                    src_type_name: String::from("Factory"),
                    dst_type_name: String::from("Warehouse"),
                    impact_weight: 0.8,
                }],
            },
        )
        .await?;

    let w1 = store
        .upsert_node(
            tenant,
            UpsertBlueNodeRequest {
                type_name: String::from("Warehouse"),
                node_key: String::from("W-1"),
                title: String::from("华东仓"),
                metrics: BTreeMap::from([(String::from("inventory"), 500.0)]),
                ..UpsertBlueNodeRequest::default()
            },
        )
        .await?;

    let w1_merged = store
        .upsert_node(
            tenant,
            UpsertBlueNodeRequest {
                type_name: String::from("Warehouse"),
                node_key: String::from("W-1"),
                title: String::from("华东仓"),
                metrics: BTreeMap::from([(String::from("capacity"), 900.0)]),
                metrics_merge: MetricsMerge::Merge,
                ..UpsertBlueNodeRequest::default()
            },
        )
        .await?;
    assert_eq!(w1_merged.metrics.get("inventory"), Some(&500.0));
    assert_eq!(w1_merged.metrics.get("capacity"), Some(&900.0));

    let stale_patch = store
        .patch_node(
            tenant,
            w1.id,
            PatchNodeRequest {
                title: Some(String::from("Stale")),
                if_row_version: Some(1),
                ..PatchNodeRequest::default()
            },
        )
        .await;
    assert!(matches!(
        stale_patch,
        Err(AppError::Conflict("VERSION_CONFLICT"))
    ));

    verify_bulk_bbox_and_search(store, tenant, w1.id).await?;
    Ok(w1.id)
}

async fn verify_bulk_bbox_and_search(store: &PgStore, tenant: &str, w1_id: Uuid) -> TestResult {
    let bulk_res = store
        .bulk_upsert(
            tenant,
            BulkNodesRequest {
                nodes: vec![BulkNodeInput {
                    type_name: String::from("Factory"),
                    node_key: String::from("F-1"),
                    title: String::from("苏州一厂"),
                    properties: serde_json::Map::new(),
                    metrics: BTreeMap::new(),
                    metrics_merge: MetricsMerge::Merge,
                    geo: Some(GeoPoint {
                        lng: 120.62,
                        lat: 31.30,
                    }),
                    source_dataset_id: None,
                    source_row_key: None,
                }],
                links: vec![
                    BulkLinkInput {
                        link_type: String::from("SUPPLIES"),
                        src: LinkEndpointRef::Key(NodeRef {
                            type_name: String::from("Factory"),
                            node_key: String::from("F-1"),
                        }),
                        dst: LinkEndpointRef::Id(w1_id),
                        weight: None,
                        properties: serde_json::Map::new(),
                        valid_from: None,
                        valid_to: None,
                    },
                    BulkLinkInput {
                        link_type: String::from("SUPPLIES"),
                        src: LinkEndpointRef::Key(NodeRef {
                            type_name: String::from("Factory"),
                            node_key: String::from("MISSING"),
                        }),
                        dst: LinkEndpointRef::Id(w1_id),
                        weight: None,
                        properties: serde_json::Map::new(),
                        valid_from: None,
                        valid_to: None,
                    },
                ],
            },
        )
        .await?;
    assert_eq!(bulk_res.upserted_nodes, 1);
    assert_eq!(bulk_res.upserted_links, 1);
    assert_eq!(bulk_res.unresolved.len(), 1);

    let bbox_res = store
        .list_nodes(
            tenant,
            NodeListQuery {
                type_name: Some(String::from("Factory")),
                q: None,
                bbox: Some(String::from("120,30,122,32")),
                limit: None,
                cursor: None,
            },
        )
        .await?;
    assert_eq!(bbox_res.items.len(), 1);

    let search_res = store
        .search_nodes(
            tenant,
            NodeSearchQuery {
                q: String::from("华东"),
                type_name: None,
                k: 10,
            },
            None,
        )
        .await?;
    assert_eq!(
        search_res.items.first().map(|h| h.node_key.as_str()),
        Some("W-1")
    );
    Ok(())
}

async fn verify_metrics_rollup_and_windows(
    store: &PgStore,
    tenant: &str,
    w1_id: Uuid,
) -> TestResult {
    let now_sh = Utc::now() + Duration::hours(8);
    let day1_sh = Utc
        .with_ymd_and_hms(now_sh.year(), now_sh.month(), now_sh.day(), 0, 0, 0)
        .single()
        .ok_or("bad ts")?
        - Duration::days(10)
        - Duration::hours(8);
    let day2_sh = day1_sh + Duration::hours(24);

    let mut pts = Vec::with_capacity(24);
    for hr in 0..24_i32 {
        pts.push(MetricPointInput {
            node_id: Some(w1_id),
            node: None,
            metric: String::from("inventory"),
            ts: day1_sh + Duration::hours(i64::from(hr)),
            value: Some(f64::from(100 + hr)),
            value_text: None,
        });
    }
    store
        .write_metrics(
            tenant,
            WriteMetricsRequest {
                source_tag: MetricSourceTag::Sync,
                batch_id: None,
                points: pts,
            },
        )
        .await?;

    store.run_rollup_for_tenant(tenant).await?;

    let day_series = store
        .query_node_metrics(
            tenant,
            w1_id,
            "inventory",
            NodeMetricSeriesQuery {
                from: day1_sh,
                to: day2_sh,
                grain: Some(MetricQueryGrain::Day),
                agg: AggFn::Sum,
            },
        )
        .await?;
    assert_eq!(day_series.len(), 1);
    assert_eq!(day_series.first().and_then(|p| p.value), Some(2676.0));

    let win_res = store
        .query_window(
            tenant,
            WindowQuery {
                node_ids: Some(vec![w1_id]),
                type_name: None,
                metric: String::from("inventory"),
                from: day1_sh,
                to: day2_sh,
                window: WindowSpec {
                    kind: WindowKind::Sliding,
                    size: String::from("3h"),
                    step: Some(String::from("1h")),
                },
                agg: AggFn::Avg,
                group_by: WindowGroupBy::Node,
            },
        )
        .await?;
    assert_eq!(win_res.items.len(), 24);
    verify_late_arrival_recompute(store, tenant, w1_id, day1_sh, day2_sh).await
}

async fn verify_late_arrival_recompute(
    store: &PgStore,
    tenant: &str,
    w1_id: Uuid,
    day1_sh: chrono::DateTime<Utc>,
    day2_sh: chrono::DateTime<Utc>,
) -> TestResult {
    let late_wr = store
        .write_metrics(
            tenant,
            WriteMetricsRequest {
                source_tag: MetricSourceTag::Manual,
                batch_id: None,
                points: vec![MetricPointInput {
                    node_id: Some(w1_id),
                    node: None,
                    metric: String::from("inventory"),
                    ts: day1_sh + Duration::minutes(30),
                    value: Some(100.0),
                    value_text: None,
                }],
            },
        )
        .await?;
    assert_eq!(late_wr.dirty_minutes, 1);

    store.run_rollup_for_tenant(tenant).await?;

    let day_after_late = store
        .query_node_metrics(
            tenant,
            w1_id,
            "inventory",
            NodeMetricSeriesQuery {
                from: day1_sh,
                to: day2_sh,
                grain: Some(MetricQueryGrain::Day),
                agg: AggFn::Sum,
            },
        )
        .await?;
    assert_eq!(day_after_late.first().and_then(|p| p.value), Some(2776.0));
    Ok(())
}

#[tokio::test]
async fn axgraph_nodes_metrics_rollup_late_arrival_and_windows() -> TestResult {
    let Some((app_pool, owner_pool)) = test_pools().await? else {
        return Ok(());
    };

    let tenant = unique_tenant("t_graph");
    ensure_tenant(&owner_pool, &tenant).await?;
    let store = PgStore::new(app_pool);
    store.ensure_metric_partitions(12, 2).await?;

    let w1_id = setup_axgraph_nodes_and_bulk(&store, &tenant).await?;
    verify_metrics_rollup_and_windows(&store, &tenant, w1_id).await?;

    cleanup_tenants(&owner_pool, &[tenant]).await?;
    Ok(())
}

fn simple_node_type(name: &str, display_name: &str) -> BlueNodeTypeInput {
    BlueNodeTypeInput {
        name: String::from(name),
        display_name: String::from(display_name),
        properties: Vec::new(),
        agg_rules: Vec::new(),
        pos_x: 0.0,
        pos_y: 0.0,
    }
}

async fn setup_supply_chain_trio(
    store: &PgStore,
    tenant: &str,
) -> Result<(Uuid, Uuid, Uuid), Box<dyn std::error::Error>> {
    store
        .save_xblue_schema(
            tenant,
            SaveXBlueSchemaRequest {
                node_types: vec![
                    simple_node_type("Factory", "工厂"),
                    simple_node_type("Warehouse", "仓库"),
                    simple_node_type("Order", "订单"),
                ],
                edge_types: vec![
                    BlueEdgeTypeInput {
                        name: String::from("SUPPLIES"),
                        src_type_name: String::from("Factory"),
                        dst_type_name: String::from("Warehouse"),
                        impact_weight: 0.9,
                    },
                    BlueEdgeTypeInput {
                        name: String::from("FULFILLS"),
                        src_type_name: String::from("Warehouse"),
                        dst_type_name: String::from("Order"),
                        impact_weight: 0.8,
                    },
                ],
            },
        )
        .await?;

    let f1 = store
        .upsert_node(
            tenant,
            UpsertBlueNodeRequest {
                type_name: String::from("Factory"),
                node_key: String::from("F-1"),
                title: String::from("晶圆一厂"),
                ..UpsertBlueNodeRequest::default()
            },
        )
        .await?;
    let w1 = store
        .upsert_node(
            tenant,
            UpsertBlueNodeRequest {
                type_name: String::from("Warehouse"),
                node_key: String::from("W-1"),
                title: String::from("华东仓"),
                ..UpsertBlueNodeRequest::default()
            },
        )
        .await?;
    let o1 = store
        .upsert_node(
            tenant,
            UpsertBlueNodeRequest {
                type_name: String::from("Order"),
                node_key: String::from("O-1"),
                title: String::from("订单 O-1"),
                ..UpsertBlueNodeRequest::default()
            },
        )
        .await?;

    for (lt, u, v, w) in [
        ("SUPPLIES", f1.id, w1.id, 0.9),
        ("FULFILLS", w1.id, o1.id, 0.8),
    ] {
        store
            .upsert_link(
                tenant,
                UpsertLinkRequest {
                    link_type: String::from(lt),
                    src_node_id: u,
                    dst_node_id: v,
                    weight: Some(w),
                    properties: serde_json::Map::new(),
                    valid_from: None,
                    valid_to: None,
                },
            )
            .await?;
    }
    Ok((f1.id, w1.id, o1.id))
}

async fn verify_entity_links_and_reject(
    app_pool: &PgPool,
    store: &PgStore,
    tenant: &str,
    w1_id: Uuid,
) -> TestResult {
    let mut tx = axpoint_db::tenant::begin_tenant_tx(app_pool, tenant).await?;
    let doc_id = Uuid::new_v4();
    let para_id = Uuid::new_v4();
    let ent_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO hg_docs (id, tenant_id, title, source_name, summary)
         VALUES ($1, $2, '仓储制度', 'warehouse.md', '华东仓有限公司负责履约订单 O-1。')",
    )
    .bind(doc_id)
    .bind(tenant)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO hg_paras (id, tenant_id, doc_id, seq_no, content)
         VALUES ($1, $2, $3, 0, '华东仓有限公司负责履约订单 O-1。')",
    )
    .bind(para_id)
    .bind(tenant)
    .bind(doc_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO hg_entities (id, tenant_id, name, name_norm, entity_type, occur_count, confidence)
         VALUES ($1, $2, '华东仓有限公司', '华东仓', 'ORG', 1, 0.95)",
    )
    .bind(ent_id)
    .bind(tenant)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO hg_edges (tenant_id, rel_type, src_type, src_id, dst_type, dst_id, weight)
         VALUES ($1, 'MENTIONS', 'PARA', $2, 'ENTITY', $3, 1.0)",
    )
    .bind(tenant)
    .bind(para_id)
    .bind(ent_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    let resolved = store
        .resolve_entity_links(tenant, ResolveEntitiesRequest { node_id: None })
        .await?;
    assert_eq!(resolved.auto_confirmed, 1);
    assert_eq!(
        store.list_node_documents(tenant, w1_id).await?.items.len(),
        1
    );

    store.reject_entity_link(tenant, ent_id, w1_id).await?;
    store
        .resolve_entity_links(tenant, ResolveEntitiesRequest { node_id: None })
        .await?;
    assert!(
        store
            .list_node_documents(tenant, w1_id)
            .await?
            .items
            .is_empty()
    );
    Ok(())
}

#[tokio::test]
async fn axgraph_entity_link_and_graph_query_traversal_and_algorithms() -> TestResult {
    let Some((app_pool, owner_pool)) = test_pools().await? else {
        return Ok(());
    };

    let tenant = unique_tenant("t_algo");
    ensure_tenant(&owner_pool, &tenant).await?;
    let store = PgStore::new(app_pool.clone());
    store.ensure_metric_partitions(1, 1).await?;

    let (f1_id, w1_id, o1_id) = setup_supply_chain_trio(&store, &tenant).await?;
    verify_entity_links_and_reject(&app_pool, &store, &tenant, w1_id).await?;

    let nb = store
        .query_neighbors(
            &tenant,
            NeighborQuery {
                node_id: f1_id,
                depth: 2,
                direction: TraversalDirection::Out,
                link_types: None,
                limit: 100,
            },
        )
        .await?;
    assert_eq!(nb.nodes.len(), 3);

    let path_unw = store
        .query_shortest_path(
            &tenant,
            PathQuery {
                from: f1_id,
                to: o1_id,
                max_depth: 4,
                weighted: false,
            },
        )
        .await?;
    assert_eq!(path_unw.path, vec![f1_id, w1_id, o1_id]);

    let path_w = store
        .query_shortest_path(
            &tenant,
            PathQuery {
                from: f1_id,
                to: o1_id,
                max_depth: 4,
                weighted: true,
            },
        )
        .await?;
    assert_eq!(path_w.path, vec![f1_id, w1_id, o1_id]);

    let pr = store
        .run_algorithm(&tenant, GraphAlgo::Pagerank, AlgoRunRequest::default())
        .await?;
    assert_eq!(pr.node_count, 3);
    assert_eq!(pr.top.len(), 3);

    let pr_page = store
        .list_algorithm_results(&tenant, GraphAlgo::Pagerank, AlgoResultsQuery::default())
        .await?;
    assert_eq!(pr_page.items.len(), 3);

    cleanup_tenants(&owner_pool, &[tenant]).await?;
    Ok(())
}
