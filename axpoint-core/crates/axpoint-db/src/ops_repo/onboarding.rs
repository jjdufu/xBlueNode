//! Step 19 5-step tenant onboarding progress and reverse subgraph consumption queries
//! (`OnboardingAndPackagingPort` on [`PgStore`], `05-STEP19` §19.1 & §19.3.2).

use std::collections::{BTreeMap, BTreeSet};

use axpoint_domain::AppError;
use axpoint_domain::ops::{OnboardingRawCounts, OnboardingStatusResponse, build_onboarding_status};
use axpoint_domain::studio::{NodeConsumptionCounts, SchemaDoc, SubgraphConsumptionResponse};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

use crate::PgStore;
use crate::tenant::begin_tenant_tx;

fn i64_to_u64(val: i64) -> u64 {
    u64::try_from(val.max(0)).unwrap_or(0)
}

pub(super) async fn get_onboarding_status_impl(
    store: &PgStore,
    tenant_id: &str,
) -> Result<OnboardingStatusResponse, AppError> {
    let mut tx = begin_tenant_tx(&store.pool, tenant_id).await?;
    let row = sqlx::query(
        "SELECT
             (SELECT COUNT(*) FROM ds_datasets WHERE tenant_id = $1) AS dataset_count,
             (SELECT COUNT(*) FROM hg_docs WHERE tenant_id = $1) AS doc_count,
             (SELECT COUNT(*) FROM xb_node_types WHERE tenant_id = $1) AS node_type_count,
             (SELECT COUNT(*) FROM xb_edge_types WHERE tenant_id = $1) AS edge_type_count,
             (SELECT COUNT(*) FROM xb_schema_versions WHERE tenant_id = $1) AS schema_version_count,
             (SELECT COUNT(*) FROM xb_nodes WHERE tenant_id = $1) AS node_count,
             (SELECT COUNT(*) FROM ax_flows WHERE tenant_id = $1) AS flow_count,
             (SELECT COUNT(*) FROM ax_sim_scenarios WHERE tenant_id = $1 AND status = 'SUCCEEDED') AS succeeded_scenario_count,
             (SELECT COUNT(*) FROM ax_sandboxes WHERE tenant_id = $1) AS sandbox_count,
             (SELECT COUNT(*) FROM xv_dashboards WHERE tenant_id = $1) AS dashboard_count,
             (SELECT COUNT(*) FROM ax_agents WHERE tenant_id = $1) AS agent_count,
             EXISTS (
                 SELECT 1 FROM xb_subgraphs
                  WHERE tenant_id = $1
                    AND is_dev_sandbox = TRUE
                    AND source_template_id IS NOT NULL
             ) AS has_trial_subgraph",
    )
    .bind(tenant_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("query onboarding status counts failed: {err}")))?;

    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit get_onboarding_status tx: {err}")))?;

    let counts = OnboardingRawCounts {
        dataset_count: i64_to_u64(row.get("dataset_count")),
        doc_count: i64_to_u64(row.get("doc_count")),
        node_type_count: i64_to_u64(row.get("node_type_count")),
        edge_type_count: i64_to_u64(row.get("edge_type_count")),
        schema_version_count: i64_to_u64(row.get("schema_version_count")),
        node_count: i64_to_u64(row.get("node_count")),
        flow_count: i64_to_u64(row.get("flow_count")),
        succeeded_scenario_count: i64_to_u64(row.get("succeeded_scenario_count")),
        sandbox_count: i64_to_u64(row.get("sandbox_count")),
        dashboard_count: i64_to_u64(row.get("dashboard_count")),
        agent_count: i64_to_u64(row.get("agent_count")),
        has_trial_subgraph: row.get("has_trial_subgraph"),
    };

    Ok(build_onboarding_status(&counts))
}

pub(super) async fn get_subgraph_consumption_impl(
    store: &PgStore,
    tenant_id: &str,
    subgraph_slug: &str,
) -> Result<SubgraphConsumptionResponse, AppError> {
    let clean_slug = subgraph_slug.trim();
    let mut tx = begin_tenant_tx(&store.pool, tenant_id).await?;

    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM xb_subgraphs WHERE tenant_id = $1 AND slug = $2)",
    )
    .bind(tenant_id)
    .bind(clean_slug)
    .fetch_one(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("check subgraph exists failed: {err}")))?;

    if !exists {
        return Err(AppError::NotFound("SUBGRAPH_NOT_FOUND"));
    }

    let node_types = collect_subgraph_node_type_names_tx(&mut tx, tenant_id, clean_slug).await?;
    let mut by_node_type = BTreeMap::new();
    for node_type in node_types {
        let counts = query_single_node_consumption_tx(&mut tx, tenant_id, &node_type).await?;
        by_node_type.insert(node_type, counts);
    }

    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit get_subgraph_consumption tx: {err}")))?;

    Ok(SubgraphConsumptionResponse {
        subgraph_slug: String::from(clean_slug),
        by_node_type,
    })
}

async fn collect_subgraph_node_type_names_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    subgraph_slug: &str,
) -> Result<BTreeSet<String>, AppError> {
    let mut names = BTreeSet::new();

    let pub_rows = sqlx::query(
        "SELECT name FROM xb_node_types
          WHERE tenant_id = $1 AND subgraph_slug = $2
          ORDER BY name ASC",
    )
    .bind(tenant_id)
    .bind(subgraph_slug)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("load subgraph xb_node_types failed: {err}")))?;

    for row in pub_rows {
        let name: String = row.get("name");
        if !name.trim().is_empty() {
            names.insert(name);
        }
    }

    let draft_rows = sqlx::query(
        "SELECT schema_doc FROM xb_schema_drafts
          WHERE tenant_id = $1 AND subgraph_slug = $2 AND status = 'DRAFT'
          ORDER BY updated_at DESC",
    )
    .bind(tenant_id)
    .bind(subgraph_slug)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("load subgraph xb_schema_drafts failed: {err}")))?;

    for row in draft_rows {
        if let Ok(doc) = serde_json::from_value::<SchemaDoc>(row.get("schema_doc")) {
            for nt in doc.node_types {
                if !nt.name.trim().is_empty() {
                    names.insert(nt.name);
                }
            }
        }
    }

    Ok(names)
}

async fn query_single_node_consumption_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    node_type: &str,
) -> Result<NodeConsumptionCounts, AppError> {
    let ep_rows = sqlx::query(
        "SELECT id, slug
           FROM ax_published_endpoints
          WHERE tenant_id = $1
            AND kind = 'OBJECT_TYPE'
            AND enabled = TRUE
            AND target_id = $2
          ORDER BY slug ASC, environment ASC",
    )
    .bind(tenant_id)
    .bind(node_type)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("query endpoint consumption failed: {err}")))?;

    let endpoint_count = ep_rows.len();
    let mut ep_id_strs = Vec::with_capacity(ep_rows.len());
    let mut slug_set = BTreeSet::new();
    for row in ep_rows {
        let id: Uuid = row.get("id");
        let slug: String = row.get("slug");
        ep_id_strs.push(id.to_string());
        slug_set.insert(slug);
    }
    let endpoint_slugs: Vec<String> = slug_set.into_iter().collect();

    let agent_ids = query_agent_ids_for_endpoints_tx(tx, tenant_id, &ep_id_strs).await?;
    let dashboard_slugs = query_dashboard_slugs_for_node_tx(tx, tenant_id, node_type).await?;
    let rule_count = query_rule_count_for_node_tx(tx, tenant_id, node_type).await?;

    Ok(NodeConsumptionCounts {
        endpoint_count,
        agent_count: agent_ids.len(),
        dashboard_count: dashboard_slugs.len(),
        rule_count,
        endpoint_slugs,
        agent_ids,
        dashboard_slugs,
    })
}

async fn query_agent_ids_for_endpoints_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    ep_id_strs: &[String],
) -> Result<Vec<Uuid>, AppError> {
    if ep_id_strs.is_empty() {
        return Ok(Vec::new());
    }
    let rows = sqlx::query(
        "SELECT DISTINCT a.id
           FROM ax_agents a
          WHERE a.tenant_id = $1
            AND jsonb_typeof(a.spec->'tools') = 'array'
            AND EXISTS (
                SELECT 1
                  FROM jsonb_array_elements(a.spec->'tools') AS t
                 WHERE t->>'kind' = 'ENDPOINT'
                   AND (t->>'endpoint_id') = ANY($2)
            )
          ORDER BY a.id ASC",
    )
    .bind(tenant_id)
    .bind(ep_id_strs)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("query agent consumption failed: {err}")))?;

    Ok(rows.into_iter().map(|r| r.get("id")).collect())
}

async fn query_dashboard_slugs_for_node_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    node_type: &str,
) -> Result<Vec<String>, AppError> {
    let rows = sqlx::query(
        "SELECT DISTINCT d.slug
           FROM xv_dashboards d
           JOIN xv_widgets w
             ON w.dashboard_id = d.id AND w.tenant_id = d.tenant_id
          WHERE d.tenant_id = $1
            AND (
                (jsonb_typeof(w.deps->'type_names') = 'array' AND (w.deps->'type_names') ? $2)
                OR w.data_source->>'type_name' = $2
                OR (jsonb_typeof(w.data_source->'type_names') = 'array' AND (w.data_source->'type_names') ? $2)
            )
          ORDER BY d.slug ASC",
    )
    .bind(tenant_id)
    .bind(node_type)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("query dashboard consumption failed: {err}")))?;

    Ok(rows.into_iter().map(|r| r.get("slug")).collect())
}

async fn query_rule_count_for_node_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    node_type: &str,
) -> Result<usize, AppError> {
    let cnt: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)
           FROM ax_rules
          WHERE tenant_id = $1
            AND scope_kind = 'METRIC'
            AND enabled = TRUE
            AND target->>'type_name' = $2",
    )
    .bind(tenant_id)
    .bind(node_type)
    .fetch_one(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("query rule consumption failed: {err}")))?;

    Ok(usize::try_from(cnt.max(0)).unwrap_or(0))
}
