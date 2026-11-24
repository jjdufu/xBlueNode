//! PostgreSQL 16 integration tests for Step 04 (`AxAnalytics` `AnalyticsRepository` on `PgStore`):
//! multi-tenant RLS isolation, semantic metrics & broken-column detection, saved queries,
//! query history truncation & cursor pagination, ask turns & Chinese few-shot recall,
//! hybrid schema-linking candidate recall (`DS_COLUMN` / `AN_METRIC` / `xb_nodes`),
//! transform draft state machine, and `DERIVED_VIEW` topological sort + cycle detection.

use std::collections::BTreeMap;

use axpoint_db::PgStore;
use axpoint_domain::AppError;
use axpoint_domain::analytics::{
    AskCitation, AskFeedbackKind, AskIntent, CreateMetricRequest, CreateSavedQueryRequest,
    LinkedObjects, MetricDimension, NewAskTurnInput, NewQueryHistoryEntry, NewTransformDraftInput,
    QueryColumn, QueryHistoryKind, QueryHistoryListQuery, QueryHistoryStatus, QueryParamSpec,
    SavedQueryKind, TransformDraftStatus, TransformPreview, UpdateMetricRequest,
    UpdateSavedQueryRequest, ds_column_embedding_id,
};
use axpoint_domain::axhub::{
    ColumnProfile, CreateDatasetRequest, CreateGlossaryTermRequest, Dataset, QualityReport,
    SchemaChange, SchemaPolicy, SidecarSyncResult, UpdateColumnMetaRequest,
};
use axpoint_domain::enums::{DataType, PiiCategory, RollupGrain, SourceKind, SyncMode};
use axpoint_domain::ports::{AnalyticsRepository, DatasetRepository, GraphStorePort};
use axpoint_domain::topology::{BlueNodeTypeInput, SaveXBlueSchemaRequest, UpsertBlueNodeRequest};
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};
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
        .max_connections(8)
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
        "DELETE FROM an_transform_drafts WHERE tenant_id = ANY($1)",
        "DELETE FROM an_ask_turns WHERE tenant_id = ANY($1)",
        "DELETE FROM an_query_history WHERE tenant_id = ANY($1)",
        "DELETE FROM an_saved_queries WHERE tenant_id = ANY($1)",
        "DELETE FROM an_metrics WHERE tenant_id = ANY($1)",
        "DELETE FROM ds_sync_jobs WHERE tenant_id = ANY($1)",
        "DELETE FROM ds_column_meta WHERE tenant_id = ANY($1)",
        "DELETE FROM ds_datasets WHERE tenant_id = ANY($1)",
        "DELETE FROM ds_glossary_terms WHERE tenant_id = ANY($1)",
        "DELETE FROM ds_lineage WHERE tenant_id = ANY($1)",
        "DELETE FROM xb_links WHERE tenant_id = ANY($1)",
        "DELETE FROM xb_nodes WHERE tenant_id = ANY($1)",
        "DELETE FROM xb_node_types WHERE tenant_id = ANY($1)",
        "DELETE FROM hg_embeddings WHERE tenant_id = ANY($1)",
        "DELETE FROM ax_jobs WHERE tenant_id = ANY($1)",
        "DELETE FROM ax_outbox WHERE tenant_id = ANY($1)",
        "DELETE FROM sec_audit_chain_heads WHERE tenant_id = ANY($1)",
        "DELETE FROM sec_audit_logs WHERE tenant_id = ANY($1)",
    ] {
        sqlx::query(sql).bind(tenants).execute(owner_pool).await?;
    }
    Ok(())
}

fn col_prof(
    name: &str,
    ordinal: i32,
    data_type: DataType,
    samples: &[&str],
    comment: &str,
) -> ColumnProfile {
    ColumnProfile {
        name: String::from(name),
        ordinal,
        data_type,
        nullable: false,
        null_count: 0,
        null_ratio: 0.0,
        distinct_estimate: 10,
        sample_values: samples.iter().map(|&s| String::from(s)).collect(),
        pii_category: PiiCategory::None,
        pii_hit_ratio: 0.0,
        source_comment: String::from(comment),
    }
}

fn demo_orders_columns() -> Vec<ColumnProfile> {
    vec![
        col_prof("order_id", 1, DataType::Int64, &["1", "2", "3"], "订单编号"),
        col_prof(
            "amount",
            2,
            DataType::Decimal,
            &["19.75", "39.50", "59.25", "79.00"],
            "订单金额",
        ),
        col_prof(
            "status",
            3,
            DataType::String,
            &["PAID", "SHIPPED", "OPEN"],
            "支付状态",
        ),
        col_prof("region", 4, DataType::String, &["华东", "华南"], "履约大区"),
        col_prof(
            "updated_at",
            5,
            DataType::Timestamp,
            &["2026-01-01T00:01:00Z"],
            "更新时间",
        ),
    ]
}

async fn seed_ready_dataset(
    store: &PgStore,
    tenant_id: &str,
    name: &str,
    display_name: &str,
    cols: Vec<ColumnProfile>,
) -> Result<Dataset, Box<dyn std::error::Error>> {
    let ds = store
        .create_dataset(
            tenant_id,
            None,
            CreateDatasetRequest {
                connection_id: None,
                name: String::from(name),
                display_name: String::from(display_name),
                description: format!("{display_name} 明细数据"),
                source_kind: SourceKind::FileTable,
                source_ref: serde_json::json!({ "staging": name }),
                sync_mode: SyncMode::Snapshot,
                primary_key: Vec::new(),
                cursor_column: None,
                delete_column: None,
                schema_policy: SchemaPolicy::Additive,
                schedule_cron: None,
                tags: vec![String::from("orders")],
            },
        )
        .await?;

    let enq = store.enqueue_sync_job(tenant_id, ds.id).await?;
    let sj = store
        .start_sync_execution(tenant_id, ds.id, enq.job_id)
        .await?;
    let sync_res = SidecarSyncResult {
        dataset_id: ds.id.to_string(),
        rows_read: 100,
        rows_written: 100,
        rows_deleted: 0,
        total_rows: Some(100),
        cursor_to: None,
        stream_offsets: BTreeMap::new(),
        snapshot_id: 1,
        schema: cols,
        schema_change: SchemaChange::default(),
        quality: QualityReport::default(),
    };
    store
        .complete_sync_execution(tenant_id, sj.id, ds.id, &sync_res)
        .await?;

    store
        .get_dataset(tenant_id, ds.id)
        .await?
        .ok_or_else(|| "ready dataset missing after sync".into())
}

async fn verify_metrics_create_and_rls(
    store: &PgStore,
    owner_pool: &PgPool,
    t1: &str,
    t2: &str,
    ds1: &Dataset,
    ds2: &Dataset,
) -> Result<Uuid, Box<dyn std::error::Error>> {
    let req = CreateMetricRequest {
        name: String::from("paid_amount"),
        display_name: String::from("已支付金额"),
        description: String::from("已支付订单总金额"),
        dataset_id: ds1.id,
        expression: String::from("SUM(amount)"),
        filter_sql: String::from("status = 'PAID'"),
        time_column: Some(String::from("updated_at")),
        default_grain: RollupGrain::Day,
        dimensions: vec![MetricDimension {
            name: String::from("region"),
            column: String::from("region"),
            display_name: String::from("区域"),
        }],
        unit: String::from("CNY"),
        format: String::from("0,0.00"),
        synonyms: vec![String::from("成交金额")],
        certified: true,
    };

    let created = store
        .create_metric(t1, None, ds1.schema_version, req.clone())
        .await?;
    assert_eq!(created.version, 1);
    assert!(!created.broken);

    let dup = store
        .create_metric(t1, None, ds1.schema_version, req.clone())
        .await;
    assert!(matches!(dup, Err(AppError::Conflict("CONFLICT"))));

    let mut req_t2 = req;
    req_t2.dataset_id = ds2.id;
    let m_t2 = store
        .create_metric(t2, None, ds2.schema_version, req_t2)
        .await?;
    assert_eq!(m_t2.name, "paid_amount");
    assert_eq!(store.list_metrics(t1).await?.len(), 1);
    assert!(store.get_metric_by_name(t2, "paid_amount").await?.is_some());

    let lineage_cnt: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM ds_lineage
          WHERE tenant_id = $1 AND src_kind = 'DATASET' AND src_id = $2
            AND dst_kind = 'METRIC' AND dst_id = $3 AND via = 'metric_def'",
    )
    .bind(t1)
    .bind(ds1.id.to_string())
    .bind(created.id.to_string())
    .fetch_one(owner_pool)
    .await?;
    assert_eq!(lineage_cnt, 1);

    let expected_key = format!("embed-metric:{}:v1", created.id);
    let job_cnt: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM ax_jobs
          WHERE tenant_id = $1 AND kind = 'EMBED_NODES' AND idempotency_key = $2",
    )
    .bind(t1)
    .bind(&expected_key)
    .fetch_one(owner_pool)
    .await?;
    assert_eq!(job_cnt, 1);
    Ok(created.id)
}

async fn verify_metric_versioning_and_broken(
    store: &PgStore,
    owner_pool: &PgPool,
    t1: &str,
    ds1: &Dataset,
    metric_id: Uuid,
) -> TestResult {
    store
        .upsert_analytics_embeddings(
            t1,
            "mock-embed-v1",
            4,
            &[(
                String::from("AN_METRIC"),
                metric_id,
                vec![0.5, 0.5, 0.0, 0.0],
            )],
        )
        .await?;

    let meta_upd = store
        .update_metric(
            t1,
            "paid_amount",
            99,
            UpdateMetricRequest {
                synonyms: Some(vec![String::from("成交金额"), String::from("实付总额")]),
                ..UpdateMetricRequest::default()
            },
        )
        .await?
        .ok_or("metric missing on metadata update")?;
    assert_eq!(meta_upd.version, 1);
    assert_eq!(meta_upd.schema_version, ds1.schema_version);

    let emb_after_meta: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM hg_embeddings WHERE tenant_id = $1 AND node_type = 'AN_METRIC' AND node_id = $2",
    )
    .bind(t1)
    .bind(metric_id)
    .fetch_one(owner_pool)
    .await?;
    assert_eq!(emb_after_meta, 0);

    let def_upd = store
        .update_metric(
            t1,
            "paid_amount",
            2,
            UpdateMetricRequest {
                expression: Some(String::from(
                    "SUM(amount) / NULLIF(COUNT(DISTINCT order_id), 0)",
                )),
                ..UpdateMetricRequest::default()
            },
        )
        .await?
        .ok_or("metric missing on def update")?;
    assert_eq!(def_upd.version, 2);
    assert_eq!(def_upd.schema_version, 2);
    assert!(!def_upd.broken);

    let reduced_cols: Vec<ColumnProfile> = demo_orders_columns()
        .into_iter()
        .filter(|c| c.name != "amount")
        .collect();
    sqlx::query("UPDATE ds_datasets SET schema_json = $1 WHERE id = $2")
        .bind(serde_json::to_value(&reduced_cols)?)
        .bind(ds1.id)
        .execute(owner_pool)
        .await?;

    let broken_m = store
        .get_metric_by_name(t1, "paid_amount")
        .await?
        .ok_or("metric missing")?;
    assert!(broken_m.broken);
    assert!(store.list_metrics(t1).await?.iter().all(|m| m.broken));

    sqlx::query("UPDATE ds_datasets SET schema_json = $1 WHERE id = $2")
        .bind(serde_json::to_value(demo_orders_columns())?)
        .bind(ds1.id)
        .execute(owner_pool)
        .await?;
    assert!(
        !store
            .get_metric_by_name(t1, "paid_amount")
            .await?
            .ok_or("metric missing")?
            .broken
    );
    Ok(())
}

async fn verify_metric_deletion_cleanup(
    store: &PgStore,
    owner_pool: &PgPool,
    t1: &str,
    ds1: &Dataset,
) -> TestResult {
    let temp_m = store
        .create_metric(
            t1,
            None,
            2,
            CreateMetricRequest {
                name: String::from("order_cnt"),
                display_name: String::from("订单数"),
                description: String::new(),
                dataset_id: ds1.id,
                expression: String::from("COUNT(order_id)"),
                filter_sql: String::new(),
                time_column: None,
                default_grain: RollupGrain::Day,
                dimensions: Vec::new(),
                unit: String::new(),
                format: String::new(),
                synonyms: Vec::new(),
                certified: false,
            },
        )
        .await?;
    store
        .upsert_analytics_embeddings(
            t1,
            "mock-embed-v1",
            4,
            &[(
                String::from("AN_METRIC"),
                temp_m.id,
                vec![0.1, 0.2, 0.3, 0.4],
            )],
        )
        .await?;
    assert!(store.delete_metric(t1, "order_cnt").await?);
    assert!(!store.delete_metric(t1, "order_cnt").await?);

    let leftover: i64 = sqlx::query_scalar(
        "SELECT (SELECT COUNT(*) FROM hg_embeddings WHERE node_type = 'AN_METRIC' AND node_id = $1)
              + (SELECT COUNT(*) FROM ds_lineage WHERE dst_kind = 'METRIC' AND dst_id = $2)",
    )
    .bind(temp_m.id)
    .bind(temp_m.id.to_string())
    .fetch_one(owner_pool)
    .await?;
    assert_eq!(leftover, 0);
    Ok(())
}

async fn verify_saved_queries(store: &PgStore, t1: &str, t2: &str) -> TestResult {
    let user_a = Uuid::new_v4();
    let user_b = Uuid::new_v4();

    let priv_sq = store
        .create_saved_query(
            t1,
            Some(user_a),
            CreateSavedQueryRequest {
                name: String::from("my_private_orders"),
                kind: SavedQueryKind::Sql,
                spec: serde_json::json!({ "sql": "SELECT * FROM demo_orders WHERE region = :r" }),
                parameters: vec![QueryParamSpec {
                    name: String::from("r"),
                    data_type: DataType::String,
                    default: Some(serde_json::json!("华东")),
                }],
                shared: false,
            },
        )
        .await?;

    let shared_sq = store
        .create_saved_query(
            t1,
            Some(user_a),
            CreateSavedQueryRequest {
                name: String::from("team_revenue"),
                kind: SavedQueryKind::Metric,
                spec: serde_json::json!({ "metric": "paid_amount" }),
                parameters: Vec::new(),
                shared: true,
            },
        )
        .await?;

    let dup_sq = store
        .create_saved_query(
            t1,
            Some(user_b),
            CreateSavedQueryRequest {
                name: String::from("team_revenue"),
                kind: SavedQueryKind::Sql,
                spec: serde_json::json!({}),
                parameters: Vec::new(),
                shared: false,
            },
        )
        .await;
    assert!(matches!(dup_sq, Err(AppError::Conflict("CONFLICT"))));

    assert_eq!(store.list_saved_queries(t1, user_a, false).await?.len(), 2);
    let visible_to_b = store.list_saved_queries(t1, user_b, false).await?;
    assert_eq!(visible_to_b.len(), 1);
    assert_eq!(visible_to_b.first().map(|q| q.id), Some(shared_sq.id));
    assert_eq!(store.list_saved_queries(t1, user_b, true).await?.len(), 2);
    assert!(store.get_saved_query(t2, shared_sq.id).await?.is_none());

    let promoted = store
        .update_saved_query(
            t1,
            priv_sq.id,
            UpdateSavedQueryRequest {
                name: Some(String::from("promoted_orders")),
                spec: None,
                parameters: None,
                shared: Some(true),
            },
        )
        .await?
        .ok_or("saved query missing on update")?;
    assert!(promoted.shared);
    assert_eq!(promoted.name, "promoted_orders");
    assert!(store.delete_saved_query(t1, priv_sq.id).await?);
    assert!(!store.delete_saved_query(t1, priv_sq.id).await?);
    Ok(())
}

async fn verify_query_history(store: &PgStore, t1: &str, t2: &str) -> TestResult {
    let user_a = Uuid::new_v4();
    let user_b = Uuid::new_v4();
    let long_sql = format!("SELECT /* {} */ 1", "中".repeat(4_500));

    let h1 = store
        .record_query_history(
            t1,
            NewQueryHistoryEntry {
                user_id: Some(user_a),
                kind: QueryHistoryKind::Sql,
                sql_text: long_sql,
                status: QueryHistoryStatus::Succeeded,
                row_count: Some(1),
                duration_ms: Some(12),
                error_code: None,
            },
        )
        .await?;
    let h2 = store
        .record_query_history(
            t1,
            NewQueryHistoryEntry {
                user_id: Some(user_a),
                kind: QueryHistoryKind::Ask,
                sql_text: String::from("SELECT SUM(amount) FROM demo_orders"),
                status: QueryHistoryStatus::Succeeded,
                row_count: Some(1),
                duration_ms: Some(45),
                error_code: None,
            },
        )
        .await?;
    let h3 = store
        .record_query_history(
            t1,
            NewQueryHistoryEntry {
                user_id: Some(user_b),
                kind: QueryHistoryKind::Sql,
                sql_text: String::from("DROP TABLE x"),
                status: QueryHistoryStatus::Rejected,
                row_count: None,
                duration_ms: Some(1),
                error_code: Some(String::from("SQL_REJECTED")),
            },
        )
        .await?;
    assert!(h1 < h2 && h2 < h3);

    let t2_hist = store
        .list_query_history(t2, None, QueryHistoryListQuery::default())
        .await?;
    assert_eq!(t2_hist.items.len(), 0);

    let page1 = store
        .list_query_history(
            t1,
            None,
            QueryHistoryListQuery {
                kind: None,
                status: None,
                limit: Some(2),
                cursor: None,
            },
        )
        .await?;
    assert_eq!(page1.items.len(), 2);
    assert_eq!(page1.items.first().map(|e| e.id), Some(h3));

    let page2 = store
        .list_query_history(
            t1,
            None,
            QueryHistoryListQuery {
                kind: None,
                status: None,
                limit: Some(2),
                cursor: page1.next_cursor,
            },
        )
        .await?;
    assert_eq!(page2.items.len(), 1);
    let oldest = page2.items.first().ok_or("missing oldest history item")?;
    assert_eq!(oldest.id, h1);
    assert_eq!(oldest.sql_text.chars().count(), 4_000);
    assert!(page2.next_cursor.is_none());
    assert_eq!(
        store
            .list_query_history(t1, Some(user_a), QueryHistoryListQuery::default())
            .await?
            .items
            .len(),
        2
    );
    Ok(())
}

async fn seed_ask_turns_for_session(
    store: &PgStore,
    t1: &str,
    session_id: Uuid,
    fixed_sql: &str,
) -> Result<Uuid, Box<dyn std::error::Error>> {
    let user_id = Uuid::new_v4();
    let t_up = store
        .insert_ask_turn(
            t1,
            NewAskTurnInput {
                session_id,
                user_id: Some(user_id),
                question: String::from("已支付订单的总金额是多少"),
                intent: AskIntent::Data,
                linked_objects: LinkedObjects {
                    datasets: vec![String::from("demo_orders")],
                    columns: vec![String::from("demo_orders.amount")],
                    metrics: vec![String::from("paid_amount")],
                    nodes: Vec::new(),
                },
                sql_text: Some(String::from(
                    "SELECT SUM(amount) AS paid_amount FROM demo_orders WHERE status = 'PAID'",
                )),
                result_preview: Some(serde_json::json!({ "rows": [{"paid_amount": 65175.0}] })),
                answer: String::from("已支付订单总金额为 65,175.00 元。"),
                citations: Vec::new(),
                model_name: String::from("qwen-plus"),
                grounding_mode: axpoint_domain::enums::GroundingMode::GuardedSql,
                verified_numbers: 1,
                verified_citations: 0,
                abstained: false,
                slot_payload: None,
            },
        )
        .await?;
    store
        .update_ask_feedback(t1, t_up.id, AskFeedbackKind::Up, None)
        .await?;

    let t_down = store
        .insert_ask_turn(
            t1,
            NewAskTurnInput {
                session_id,
                user_id: Some(user_id),
                question: String::from("已支付订单的平均单价是多少"),
                intent: AskIntent::Data,
                linked_objects: LinkedObjects::default(),
                sql_text: Some(String::from("SELECT COUNT(*) FROM demo_orders")),
                result_preview: None,
                answer: String::from("错误回答"),
                citations: Vec::new(),
                model_name: String::from("qwen-plus"),
                grounding_mode: axpoint_domain::enums::GroundingMode::GuardedSql,
                verified_numbers: 0,
                verified_citations: 0,
                abstained: false,
                slot_payload: None,
            },
        )
        .await?;
    store
        .update_ask_feedback(t1, t_down.id, AskFeedbackKind::Down, None)
        .await?;

    let t_corr = store
        .insert_ask_turn(
            t1,
            NewAskTurnInput {
                session_id,
                user_id: Some(user_id),
                question: String::from("华东大区订单笔数统计"),
                intent: AskIntent::Hybrid,
                linked_objects: LinkedObjects::default(),
                sql_text: Some(String::from("SELECT COUNT(*) FROM demo_orders")),
                result_preview: None,
                answer: String::from("修正前回答"),
                citations: vec![AskCitation {
                    para_id: Uuid::new_v4(),
                    doc_id: Uuid::new_v4(),
                    doc_title: String::from("周报"),
                    snippet: String::from("华东履约正常"),
                }],
                model_name: String::from("qwen-plus"),
                grounding_mode: axpoint_domain::enums::GroundingMode::GuardedSql,
                verified_numbers: 0,
                verified_citations: 1,
                abstained: false,
                slot_payload: None,
            },
        )
        .await?;
    store
        .update_ask_feedback(t1, t_corr.id, AskFeedbackKind::Down, Some(fixed_sql))
        .await?;
    Ok(t_up.id)
}

async fn verify_ask_turns_and_few_shot(store: &PgStore, t1: &str, t2: &str) -> TestResult {
    let session_id = Uuid::new_v4();
    let fixed_sql = "SELECT COUNT(*) FROM demo_orders WHERE region = '华东'";
    let first_turn_id = seed_ask_turns_for_session(store, t1, session_id, fixed_sql).await?;

    let turns = store.list_session_turns(t1, session_id).await?;
    assert_eq!(turns.len(), 3);
    assert_eq!(turns.first().map(|x| x.id), Some(first_turn_id));
    assert!(store.get_ask_turn(t2, first_turn_id).await?.is_none());
    assert_eq!(store.list_session_turns(t2, session_id).await?.len(), 0);

    let fs_paid = store
        .search_few_shot_turns(t1, "已支付订单金额合计是多少", 5)
        .await?;
    assert_eq!(
        fs_paid.first().map(|f| f.question.as_str()),
        Some("已支付订单的总金额是多少")
    );
    assert!(
        fs_paid
            .iter()
            .all(|f| f.question != "已支付订单的平均单价是多少")
    );

    let fs_east = store
        .search_few_shot_turns(t1, "华东订单笔数有多少", 5)
        .await?;
    assert!(fs_east.iter().any(|f| f.sql_text == fixed_sql));
    assert_eq!(
        store
            .search_few_shot_turns(t2, "已支付订单金额合计是多少", 5)
            .await?
            .len(),
        0
    );
    Ok(())
}

async fn seed_glossary_node_and_embeddings(store: &PgStore, t1: &str, ds1: &Dataset) -> TestResult {
    store
        .update_column_meta(
            t1,
            ds1.id,
            "amount",
            UpdateColumnMetaRequest {
                pii_category: None,
                pii_confirmed: None,
                description: Some(String::from("订单交易金额")),
                business_term: Some(String::from("交易金额")),
                unit: Some(String::from("CNY")),
            },
        )
        .await?;
    store
        .create_glossary_term(
            t1,
            CreateGlossaryTermRequest {
                term: String::from("交易金额"),
                definition: String::from("订单实付总金额"),
                synonyms: vec![String::from("总金额"), String::from("GMV")],
            },
        )
        .await?;

    store
        .save_xblue_schema(
            t1,
            SaveXBlueSchemaRequest {
                node_types: vec![BlueNodeTypeInput {
                    name: String::from("Warehouse"),
                    display_name: String::from("仓库"),
                    properties: Vec::new(),
                    agg_rules: Vec::new(),
                    pos_x: 100.0,
                    pos_y: 100.0,
                }],
                edge_types: Vec::new(),
            },
        )
        .await?;
    store
        .upsert_node(
            t1,
            UpsertBlueNodeRequest {
                type_name: String::from("Warehouse"),
                node_key: String::from("W-1"),
                title: String::from("上海华东仓有限公司"),
                ..UpsertBlueNodeRequest::default()
            },
        )
        .await?;

    let unembedded = store
        .list_unembedded_analytics_items(t1, "embed-step04-test", 100)
        .await?;
    let amount_emb_id = ds_column_embedding_id(ds1.id, "amount");
    assert!(
        unembedded
            .iter()
            .any(|(nt, id, _)| nt == "DS_COLUMN" && *id == amount_emb_id)
    );
    assert!(unembedded.iter().any(|(nt, _, _)| nt == "AN_METRIC"));

    let to_upsert: Vec<(String, Uuid, Vec<f32>)> = unembedded
        .iter()
        .map(|(nt, id, _)| {
            let v = if *id == amount_emb_id || nt == "AN_METRIC" {
                vec![1.0_f32, 0.0, 0.0, 0.0]
            } else {
                vec![0.0_f32, 1.0, 0.0, 0.0]
            };
            (nt.clone(), *id, v)
        })
        .collect();
    let upserted_cnt = store
        .upsert_analytics_embeddings(t1, "embed-step04-test", 4, &to_upsert)
        .await?;
    assert_eq!(upserted_cnt, to_upsert.len());
    assert_eq!(
        store
            .list_unembedded_analytics_items(t1, "embed-step04-test", 100)
            .await?
            .len(),
        0
    );
    Ok(())
}

async fn verify_schema_linking_recall(
    store: &PgStore,
    t1: &str,
    t2: &str,
    ds1: &Dataset,
    other_ds: &Dataset,
) -> TestResult {
    seed_glossary_node_and_embeddings(store, t1, ds1).await?;

    let bundle = store
        .recall_schema_candidates(
            t1,
            "上海华东仓已支付订单的总金额是多少",
            Some(&[1.0_f32, 0.0, 0.0, 0.0]),
            &[],
        )
        .await?;

    let amount_hit = bundle
        .columns
        .iter()
        .find(|c| c.dataset_id == ds1.id && c.column_name == "amount")
        .ok_or("demo_orders.amount missing from recall")?;
    let status_hit = bundle
        .columns
        .iter()
        .find(|c| c.dataset_id == ds1.id && c.column_name == "status")
        .ok_or("demo_orders.status missing from recall")?;
    assert!(amount_hit.fts_score >= 0.75);
    assert!(amount_hit.vec_score >= 0.99);
    assert!(amount_hit.sample_values.len() <= 3);
    assert!(status_hit.fts_score >= 0.70);

    let paid_metric = bundle
        .metrics
        .iter()
        .find(|m| m.metric.name == "paid_amount")
        .ok_or("paid_amount metric missing from recall")?;
    assert!(paid_metric.fts_score >= 0.85);
    assert!(paid_metric.vec_score >= 0.99);

    assert!(
        bundle
            .matched_nodes
            .iter()
            .any(|(ty, key, _)| ty == "Warehouse" && key == "W-1")
    );

    let scoped = store
        .recall_schema_candidates(t1, "已支付订单的总金额是多少", None, &[other_ds.id])
        .await?;
    assert!(scoped.columns.iter().all(|c| c.dataset_id == other_ds.id));
    assert_eq!(scoped.metrics.len(), 0);

    let t2_bundle = store
        .recall_schema_candidates(t2, "上海华东仓已支付订单的总金额是多少", None, &[])
        .await?;
    assert_eq!(t2_bundle.matched_nodes.len(), 0);
    Ok(())
}

async fn create_derived_ds(
    store: &PgStore,
    tenant_id: &str,
    name: &str,
    depends_on: &[Uuid],
) -> Result<Dataset, Box<dyn std::error::Error>> {
    let dep_strs: Vec<String> = depends_on.iter().map(Uuid::to_string).collect();
    let ds = store
        .create_dataset(
            tenant_id,
            None,
            CreateDatasetRequest {
                connection_id: None,
                name: String::from(name),
                display_name: String::from(name),
                description: String::new(),
                source_kind: SourceKind::DerivedView,
                source_ref: serde_json::json!({
                    "sql": format!("SELECT * FROM {name}"),
                    "depends_on": dep_strs,
                }),
                sync_mode: SyncMode::Snapshot,
                primary_key: Vec::new(),
                cursor_column: None,
                delete_column: None,
                schema_policy: SchemaPolicy::Additive,
                schedule_cron: None,
                tags: Vec::new(),
            },
        )
        .await?;
    Ok(ds)
}

async fn verify_transform_draft_lifecycle(
    store: &PgStore,
    t1: &str,
    t2: &str,
    ds1: &Dataset,
) -> Result<Dataset, Box<dyn std::error::Error>> {
    let draft = store
        .create_transform_draft(
            t1,
            NewTransformDraftInput {
                user_id: None,
                instruction: String::from("按区域汇总已支付订单金额"),
                source_dataset_ids: vec![ds1.id],
                sql_text: String::from(
                    "SELECT region, SUM(amount) AS total FROM demo_orders GROUP BY region",
                ),
                preview: TransformPreview {
                    columns: vec![QueryColumn {
                        name: String::from("region"),
                        data_type: DataType::String,
                        pii_category: PiiCategory::None,
                        masked: false,
                    }],
                    rows: Vec::new(),
                    row_count_estimate: 2,
                    added_columns: vec![String::from("total")],
                    removed_columns: vec![String::from("status")],
                },
                target_dataset_name: String::from("orders_by_region"),
            },
        )
        .await?;
    assert_eq!(draft.status, TransformDraftStatus::Draft);
    assert!(store.get_transform_draft(t2, draft.id).await?.is_none());

    let edited = store
        .update_transform_draft(
            t1,
            draft.id,
            "SELECT region, SUM(amount) AS total FROM demo_orders WHERE status = 'PAID' GROUP BY region",
            &draft.preview,
        )
        .await?
        .ok_or("draft missing on update")?;
    assert!(edited.sql_text.contains("PAID"));

    let d_clean = create_derived_ds(store, t1, "orders_clean", &[ds1.id]).await?;
    let applied = store
        .mark_transform_applied(t1, draft.id, d_clean.id)
        .await?
        .ok_or("draft missing on apply")?;
    assert_eq!(applied.status, TransformDraftStatus::Applied);
    assert_eq!(applied.dataset_id, Some(d_clean.id));

    assert!(matches!(
        store.mark_transform_applied(t1, draft.id, d_clean.id).await,
        Err(AppError::Conflict("CONFLICT"))
    ));
    assert!(matches!(
        store
            .update_transform_draft(t1, draft.id, "SELECT 1", &draft.preview)
            .await,
        Err(AppError::Conflict("CONFLICT"))
    ));
    assert!(matches!(
        store.mark_transform_discarded(t1, draft.id).await,
        Err(AppError::Conflict("CONFLICT"))
    ));

    let draft2 = store
        .create_transform_draft(
            t1,
            NewTransformDraftInput {
                user_id: None,
                instruction: String::from("discard me"),
                source_dataset_ids: vec![ds1.id],
                sql_text: String::from("SELECT 1"),
                preview: TransformPreview::default(),
                target_dataset_name: String::from("discarded_view"),
            },
        )
        .await?;
    let discarded = store
        .mark_transform_discarded(t1, draft2.id)
        .await?
        .ok_or("draft2 missing")?;
    assert_eq!(discarded.status, TransformDraftStatus::Discarded);
    assert!(matches!(
        store
            .mark_transform_applied(t1, draft2.id, d_clean.id)
            .await,
        Err(AppError::Conflict("CONFLICT"))
    ));
    Ok(d_clean)
}

async fn verify_derived_dag_and_cycles(
    store: &PgStore,
    owner_pool: &PgPool,
    t1: &str,
    ds1: &Dataset,
    other_ds: &Dataset,
    d_clean: &Dataset,
) -> TestResult {
    let d_daily = create_derived_ds(store, t1, "orders_daily", &[ds1.id]).await?;
    let d_mart = create_derived_ds(store, t1, "orders_mart", &[d_clean.id, d_daily.id]).await?;
    let d_exec = create_derived_ds(store, t1, "orders_exec", &[d_mart.id]).await?;
    let d_unrel = create_derived_ds(store, t1, "other_view", &[other_ds.id]).await?;

    let topo = store.list_dependent_derived_datasets(t1, ds1.id).await?;
    let topo_ids: Vec<Uuid> = topo.iter().map(|d| d.id).collect();
    assert_eq!(topo_ids.len(), 4);
    assert!(!topo_ids.contains(&d_unrel.id));

    let pos_clean = topo_ids
        .iter()
        .position(|&x| x == d_clean.id)
        .ok_or("missing d_clean")?;
    let pos_daily = topo_ids
        .iter()
        .position(|&x| x == d_daily.id)
        .ok_or("missing d_daily")?;
    let pos_mart = topo_ids
        .iter()
        .position(|&x| x == d_mart.id)
        .ok_or("missing d_mart")?;
    let pos_exec = topo_ids
        .iter()
        .position(|&x| x == d_exec.id)
        .ok_or("missing d_exec")?;
    assert!(pos_clean < pos_mart && pos_daily < pos_mart && pos_mart < pos_exec);

    sqlx::query("UPDATE ds_datasets SET source_ref = $1 WHERE id = $2")
        .bind(serde_json::json!({
            "sql": "SELECT 1",
            "depends_on": [ds1.id.to_string(), d_exec.id.to_string()],
        }))
        .bind(d_clean.id)
        .execute(owner_pool)
        .await?;

    let cyc_res = store.list_dependent_derived_datasets(t1, ds1.id).await;
    assert!(matches!(
        cyc_res,
        Err(AppError::Conflict("DEPENDENCY_CYCLE"))
    ));
    Ok(())
}

async fn assert_sql_level_rls_isolation(
    store: &PgStore,
    from_tenant: &str,
    target_tenant: &str,
) -> TestResult {
    let mut rls_tx = axpoint_db::tenant::begin_tenant_tx(store.pool(), from_tenant).await?;
    let cross_row = sqlx::query(
        "SELECT
            (SELECT COUNT(*) FROM an_metrics WHERE tenant_id = $1) AS m_cnt,
            (SELECT COUNT(*) FROM an_saved_queries WHERE tenant_id = $1) AS sq_cnt,
            (SELECT COUNT(*) FROM an_query_history WHERE tenant_id = $1) AS qh_cnt,
            (SELECT COUNT(*) FROM an_ask_turns WHERE tenant_id = $1) AS at_cnt,
            (SELECT COUNT(*) FROM an_transform_drafts WHERE tenant_id = $1) AS td_cnt",
    )
    .bind(target_tenant)
    .fetch_one(&mut *rls_tx)
    .await?;
    rls_tx.commit().await?;

    assert_eq!(cross_row.get::<i64, _>("m_cnt"), 0);
    assert_eq!(cross_row.get::<i64, _>("sq_cnt"), 0);
    assert_eq!(cross_row.get::<i64, _>("qh_cnt"), 0);
    assert_eq!(cross_row.get::<i64, _>("at_cnt"), 0);
    assert_eq!(cross_row.get::<i64, _>("td_cnt"), 0);
    Ok(())
}

#[tokio::test]
async fn step04_analytics_repository_end_to_end() -> TestResult {
    let Some((app_pool, owner_pool)) = test_pools().await? else {
        return Ok(());
    };

    let t1 = unique_tenant("t_an4_a");
    let t2 = unique_tenant("t_an4_b");
    let tenants = vec![t1.clone(), t2.clone()];
    ensure_tenant(&owner_pool, &t1).await?;
    ensure_tenant(&owner_pool, &t2).await?;

    let store = PgStore::new(app_pool);
    let ds1 = seed_ready_dataset(
        &store,
        &t1,
        "demo_orders",
        "演示订单表",
        demo_orders_columns(),
    )
    .await?;
    let other_ds = seed_ready_dataset(
        &store,
        &t1,
        "iot_telemetry",
        "传感器遥测",
        vec![col_prof(
            "temperature",
            1,
            DataType::Float64,
            &["23.5"],
            "温度",
        )],
    )
    .await?;
    let ds2 = seed_ready_dataset(
        &store,
        &t2,
        "demo_orders",
        "租户二订单表",
        demo_orders_columns(),
    )
    .await?;

    let metric_id =
        verify_metrics_create_and_rls(&store, &owner_pool, &t1, &t2, &ds1, &ds2).await?;
    verify_metric_versioning_and_broken(&store, &owner_pool, &t1, &ds1, metric_id).await?;
    verify_metric_deletion_cleanup(&store, &owner_pool, &t1, &ds1).await?;
    verify_saved_queries(&store, &t1, &t2).await?;
    verify_query_history(&store, &t1, &t2).await?;
    verify_ask_turns_and_few_shot(&store, &t1, &t2).await?;
    verify_schema_linking_recall(&store, &t1, &t2, &ds1, &other_ds).await?;
    let d_clean = verify_transform_draft_lifecycle(&store, &t1, &t2, &ds1).await?;
    verify_derived_dag_and_cycles(&store, &owner_pool, &t1, &ds1, &other_ds, &d_clean).await?;
    assert_sql_level_rls_isolation(&store, &t2, &t1).await?;

    cleanup_tenants(&owner_pool, &tenants).await?;
    Ok(())
}

async fn verify_quoted_and_broken_metric_variants(
    store: &PgStore,
    t1: &str,
    ds_id: Uuid,
) -> TestResult {
    let ok_quoted = store
        .create_metric(
            t1,
            None,
            1,
            CreateMetricRequest {
                name: String::from("quoted_paid"),
                display_name: String::from("引号指标"),
                description: String::new(),
                dataset_id: ds_id,
                expression: String::from("SUM(\"amount\")"),
                filter_sql: String::from("status = 'MISSING_COL_NAME_IN_LITERAL'"),
                time_column: Some(String::from("updated_at")),
                default_grain: RollupGrain::Day,
                dimensions: Vec::new(),
                unit: String::new(),
                format: String::new(),
                synonyms: Vec::new(),
                certified: false,
            },
        )
        .await?;
    assert!(!ok_quoted.broken);

    let bad_dim = store
        .create_metric(
            t1,
            None,
            1,
            CreateMetricRequest {
                name: String::from("bad_dim_metric"),
                display_name: String::from("坏维度"),
                description: String::new(),
                dataset_id: ds_id,
                expression: String::from("SUM(amount)"),
                filter_sql: String::new(),
                time_column: None,
                default_grain: RollupGrain::Day,
                dimensions: vec![MetricDimension {
                    name: String::from("ghost"),
                    column: String::from("non_existent_col"),
                    display_name: String::from("幽灵列"),
                }],
                unit: String::new(),
                format: String::new(),
                synonyms: Vec::new(),
                certified: false,
            },
        )
        .await?;
    assert!(bad_dim.broken);
    Ok(())
}

#[tokio::test]
async fn step04_analytics_edge_cases_schema_fallback_and_broken_variants() -> TestResult {
    let Some((app_pool, owner_pool)) = test_pools().await? else {
        return Ok(());
    };

    let t1 = unique_tenant("t_an4_edge");
    let tenants = vec![t1.clone()];
    ensure_tenant(&owner_pool, &t1).await?;
    let store = PgStore::new(app_pool);

    let ds = seed_ready_dataset(
        &store,
        &t1,
        "fallback_orders",
        "回退订单表",
        demo_orders_columns(),
    )
    .await?;

    sqlx::query("DELETE FROM ds_column_meta WHERE dataset_id = $1")
        .bind(ds.id)
        .execute(&owner_pool)
        .await?;

    let unemb = store
        .list_unembedded_analytics_items(&t1, "fallback-model", 20)
        .await?;
    assert_eq!(unemb.len(), 5);

    let recall = store
        .recall_schema_candidates(&t1, "已支付订单总金额", None, &[ds.id])
        .await?;
    assert_eq!(recall.columns.len(), 5);
    assert!(
        recall
            .columns
            .iter()
            .any(|c| c.column_name == "amount" && c.fts_score >= 0.70)
    );

    verify_quoted_and_broken_metric_variants(&store, &t1, ds.id).await?;

    let d_self = create_derived_ds(&store, &t1, "self_loop_view", &[ds.id]).await?;
    sqlx::query("UPDATE ds_datasets SET source_ref = $1 WHERE id = $2")
        .bind(serde_json::json!({
            "sql": "SELECT 1",
            "depends_on": [ds.id.to_string(), d_self.id.to_string()],
        }))
        .bind(d_self.id)
        .execute(&owner_pool)
        .await?;
    assert!(matches!(
        store.list_dependent_derived_datasets(&t1, ds.id).await,
        Err(AppError::Conflict("DEPENDENCY_CYCLE"))
    ));

    cleanup_tenants(&owner_pool, &tenants).await?;
    Ok(())
}
