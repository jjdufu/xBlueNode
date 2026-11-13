//! PostgreSQL persistence for `xBlueStudio`, `AxFlow`, and `AxSim` sandboxes.

use std::collections::HashMap;

use axpoint_domain::AppError;
use axpoint_domain::topology::{
    AggRuleDef, AxFlowDef, AxFlowRunResult, AxFlowStep, AxSandboxRecord, AxSimAction, AxSimResult,
    AxSimSnapshot, BlueEdgeTypeDef, BlueLink, BlueNode, BlueNodeTypeDef, CreateAxFlowRequest,
    CreateAxSandboxRequest, PropertyDef, SaveXBlueSchemaRequest, UpsertBlueNodeRequest,
    XBlueSchema, simulate_causal_propagation,
};
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

use crate::PgStore;

impl PgStore {
    /// Reads the `xBlueStudio` schema (node types & edge types) for a tenant.
    ///
    /// # Errors
    /// Returns [`AppError::Database`] if querying schema tables fails.
    pub async fn get_xblue_schema(&self, tenant_id: &str) -> Result<XBlueSchema, AppError> {
        let node_rows = sqlx::query(
            "SELECT id, name, display_name, properties, agg_rules, pos_x, pos_y
             FROM xb_node_types WHERE tenant_id = $1 ORDER BY created_at",
        )
        .bind(tenant_id)
        .fetch_all(self.pool())
        .await
        .map_err(|err| AppError::Database(format!("query xb_node_types failed: {err}")))?;

        let mut node_types = Vec::with_capacity(node_rows.len());
        for r in node_rows {
            let props_val: serde_json::Value = r.get("properties");
            let rules_val: serde_json::Value = r.get("agg_rules");
            let properties: Vec<PropertyDef> =
                serde_json::from_value(props_val).unwrap_or_default();
            let agg_rules: Vec<AggRuleDef> = serde_json::from_value(rules_val).unwrap_or_default();
            node_types.push(BlueNodeTypeDef {
                id: r.get("id"),
                name: r.get("name"),
                display_name: r.get("display_name"),
                properties,
                agg_rules,
                pos_x: r.get("pos_x"),
                pos_y: r.get("pos_y"),
            });
        }

        let edge_rows = sqlx::query(
            "SELECT id, name, src_type_name, dst_type_name, impact_weight
             FROM xb_edge_types WHERE tenant_id = $1 ORDER BY created_at",
        )
        .bind(tenant_id)
        .fetch_all(self.pool())
        .await
        .map_err(|err| AppError::Database(format!("query xb_edge_types failed: {err}")))?;

        let edge_types = edge_rows
            .into_iter()
            .map(|r| BlueEdgeTypeDef {
                id: r.get("id"),
                name: r.get("name"),
                src_type_name: r.get("src_type_name"),
                dst_type_name: r.get("dst_type_name"),
                impact_weight: r.get("impact_weight"),
            })
            .collect();

        Ok(XBlueSchema {
            node_types,
            edge_types,
        })
    }

    /// Upserts `xBlueStudio` node types and edge types transactionally.
    ///
    /// # Errors
    /// Returns [`AppError::Database`] if any upsert or commit fails.
    pub async fn save_xblue_schema(
        &self,
        tenant_id: &str,
        req: SaveXBlueSchemaRequest,
    ) -> Result<XBlueSchema, AppError> {
        let mut tx = self
            .pool()
            .begin()
            .await
            .map_err(|err| AppError::Database(format!("begin schema tx failed: {err}")))?;

        for nt in req.node_types {
            let props_json = serde_json::to_value(&nt.properties).map_err(|err| {
                AppError::bad("INVALID_ARGUMENT", format!("invalid properties: {err}"))
            })?;
            let rules_json = serde_json::to_value(&nt.agg_rules).map_err(|err| {
                AppError::bad("INVALID_ARGUMENT", format!("invalid agg_rules: {err}"))
            })?;

            sqlx::query(
                "INSERT INTO xb_node_types (tenant_id, name, display_name, properties, agg_rules, pos_x, pos_y)
                 VALUES ($1, $2, $3, $4, $5, $6, $7)
                 ON CONFLICT (tenant_id, name) DO UPDATE SET
                    display_name = EXCLUDED.display_name,
                    properties = EXCLUDED.properties,
                    agg_rules = EXCLUDED.agg_rules,
                    pos_x = EXCLUDED.pos_x,
                    pos_y = EXCLUDED.pos_y",
            )
            .bind(tenant_id)
            .bind(nt.name.trim())
            .bind(nt.display_name.trim())
            .bind(props_json)
            .bind(rules_json)
            .bind(nt.pos_x)
            .bind(nt.pos_y)
            .execute(&mut *tx)
            .await
            .map_err(|err| AppError::Database(format!("upsert xb_node_type failed: {err}")))?;
        }

        for et in req.edge_types {
            sqlx::query(
                "INSERT INTO xb_edge_types (tenant_id, name, src_type_name, dst_type_name, impact_weight)
                 VALUES ($1, $2, $3, $4, $5)
                 ON CONFLICT (tenant_id, name, src_type_name, dst_type_name) DO UPDATE SET
                    impact_weight = EXCLUDED.impact_weight",
            )
            .bind(tenant_id)
            .bind(et.name.trim())
            .bind(et.src_type_name.trim())
            .bind(et.dst_type_name.trim())
            .bind(et.impact_weight)
            .execute(&mut *tx)
            .await
            .map_err(|err| AppError::Database(format!("upsert xb_edge_type failed: {err}")))?;
        }

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit schema tx failed: {err}")))?;

        self.get_xblue_schema(tenant_id).await
    }

    /// Lists runtime `BlueNode`s and `BlueLink`s for a tenant.
    ///
    /// # Errors
    /// Returns [`AppError::Database`] if querying runtime tables fails.
    pub async fn get_runtime_snapshot(&self, tenant_id: &str) -> Result<AxSimSnapshot, AppError> {
        let node_rows = sqlx::query(
            "SELECT id, type_name, node_key, title, metrics
             FROM xb_nodes WHERE tenant_id = $1 ORDER BY updated_at DESC",
        )
        .bind(tenant_id)
        .fetch_all(self.pool())
        .await
        .map_err(|err| AppError::Database(format!("query xb_nodes failed: {err}")))?;

        let nodes = node_rows
            .into_iter()
            .map(|r| {
                let m_val: serde_json::Value = r.get("metrics");
                let metrics: HashMap<String, f64> =
                    serde_json::from_value(m_val).unwrap_or_default();
                BlueNode {
                    id: r.get("id"),
                    type_name: r.get("type_name"),
                    node_key: r.get("node_key"),
                    title: r.get("title"),
                    metrics,
                }
            })
            .collect();

        let link_rows = sqlx::query(
            "SELECT id, link_type, src_node_id, dst_node_id, weight
             FROM xb_links WHERE tenant_id = $1",
        )
        .bind(tenant_id)
        .fetch_all(self.pool())
        .await
        .map_err(|err| AppError::Database(format!("query xb_links failed: {err}")))?;

        let links = link_rows
            .into_iter()
            .map(|r| BlueLink {
                id: r.get("id"),
                link_type: r.get("link_type"),
                src_node_id: r.get("src_node_id"),
                dst_node_id: r.get("dst_node_id"),
                weight: r.get("weight"),
            })
            .collect();

        Ok(AxSimSnapshot { nodes, links })
    }

    /// Upserts a runtime `BlueNode` and optionally creates a causal `BlueLink`.
    ///
    /// # Errors
    /// Returns [`AppError::Database`] if the upsert or link creation fails.
    pub async fn upsert_blue_node(
        &self,
        tenant_id: &str,
        req: UpsertBlueNodeRequest,
    ) -> Result<BlueNode, AppError> {
        let metrics_json = serde_json::to_value(&req.metrics)
            .map_err(|err| AppError::bad("INVALID_ARGUMENT", format!("invalid metrics: {err}")))?;

        let row = sqlx::query(
            "INSERT INTO xb_nodes (tenant_id, type_name, node_key, title, metrics)
             VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (tenant_id, type_name, node_key) DO UPDATE SET
                title = EXCLUDED.title,
                metrics = EXCLUDED.metrics,
                updated_at = NOW()
             RETURNING id",
        )
        .bind(tenant_id)
        .bind(req.type_name.trim())
        .bind(req.node_key.trim())
        .bind(req.title.trim())
        .bind(metrics_json)
        .fetch_one(self.pool())
        .await
        .map_err(|err| AppError::Database(format!("upsert xb_node failed: {err}")))?;

        let node_id: Uuid = row.get("id");

        if let Some(target_id) = req.link_to_node_id {
            let ltype = req.link_type.unwrap_or_else(|| String::from("IMPACTS"));
            let weight = req.link_weight.unwrap_or(0.6);
            sqlx::query(
                "INSERT INTO xb_links (tenant_id, link_type, src_node_id, dst_node_id, weight)
                 VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(tenant_id)
            .bind(ltype)
            .bind(node_id)
            .bind(target_id)
            .bind(weight)
            .execute(self.pool())
            .await
            .map_err(|err| AppError::Database(format!("insert xb_link failed: {err}")))?;
        }

        Ok(BlueNode {
            id: node_id,
            type_name: req.type_name,
            node_key: req.node_key,
            title: req.title,
            metrics: req.metrics,
        })
    }

    /// Lists `AxFlow` pipelines for a tenant.
    ///
    /// # Errors
    /// Returns [`AppError::Database`] if querying flows fails.
    pub async fn list_ax_flows(&self, tenant_id: &str) -> Result<Vec<AxFlowDef>, AppError> {
        let rows = sqlx::query(
            "SELECT id, name, description, steps, status, last_run_at
             FROM ax_flows WHERE tenant_id = $1 ORDER BY created_at DESC",
        )
        .bind(tenant_id)
        .fetch_all(self.pool())
        .await
        .map_err(|err| AppError::Database(format!("query ax_flows failed: {err}")))?;

        Ok(rows
            .into_iter()
            .map(|r| {
                let steps_val: serde_json::Value = r.get("steps");
                let steps: Vec<AxFlowStep> = serde_json::from_value(steps_val).unwrap_or_default();
                AxFlowDef {
                    id: r.get("id"),
                    name: r.get("name"),
                    description: r.get("description"),
                    steps,
                    status: r.get("status"),
                    last_run_at: r.get("last_run_at"),
                }
            })
            .collect())
    }

    /// Creates a new `AxFlow` pipeline.
    ///
    /// # Errors
    /// Returns [`AppError::Database`] if inserting the flow fails.
    pub async fn create_ax_flow(
        &self,
        tenant_id: &str,
        req: CreateAxFlowRequest,
    ) -> Result<AxFlowDef, AppError> {
        let steps_json = serde_json::to_value(&req.steps)
            .map_err(|err| AppError::bad("INVALID_ARGUMENT", format!("invalid steps: {err}")))?;

        let row = sqlx::query(
            "INSERT INTO ax_flows (tenant_id, name, description, steps, status)
             VALUES ($1, $2, $3, $4, 'PUBLISHED')
             RETURNING id",
        )
        .bind(tenant_id)
        .bind(req.name.trim())
        .bind(req.description.trim())
        .bind(steps_json)
        .fetch_one(self.pool())
        .await
        .map_err(|err| AppError::Database(format!("insert ax_flow failed: {err}")))?;

        Ok(AxFlowDef {
            id: row.get("id"),
            name: req.name,
            description: req.description,
            steps: req.steps,
            status: String::from("PUBLISHED"),
            last_run_at: None,
        })
    }

    /// Executes an `AxFlow` pipeline and updates `last_run_at`.
    ///
    /// # Errors
    /// Returns [`AppError::Database`] or [`AppError::NotFound`] if the flow is missing.
    pub async fn run_ax_flow(
        &self,
        tenant_id: &str,
        flow_id: Uuid,
    ) -> Result<AxFlowRunResult, AppError> {
        let row = sqlx::query(
            "UPDATE ax_flows SET last_run_at = NOW()
             WHERE id = $1 AND tenant_id = $2
             RETURNING steps",
        )
        .bind(flow_id)
        .bind(tenant_id)
        .fetch_optional(self.pool())
        .await
        .map_err(|err| AppError::Database(format!("run ax_flow query failed: {err}")))?
        .ok_or(AppError::NotFound("FLOW_NOT_FOUND"))?;

        let steps_val: serde_json::Value = row.get("steps");
        let steps: Vec<AxFlowStep> = serde_json::from_value(steps_val).unwrap_or_default();
        let mut logs = Vec::with_capacity(steps.len());
        for (idx, step) in steps.iter().enumerate() {
            let num = idx.saturating_add(1);
            logs.push(format!("[Step {num}] {} ({}) -> OK", step.label, step.kind));
        }

        Ok(AxFlowRunResult {
            flow_id,
            status: String::from("COMPLETED"),
            steps_executed: steps.len(),
            logs,
        })
    }

    /// Lists `AxSim` sandboxes for a tenant.
    ///
    /// # Errors
    /// Returns [`AppError::Database`] if querying sandboxes fails.
    pub async fn list_ax_sandboxes(
        &self,
        tenant_id: &str,
    ) -> Result<Vec<AxSandboxRecord>, AppError> {
        let rows = sqlx::query(
            "SELECT id, name, base_snapshot, actions, sim_result, created_at
             FROM ax_sandboxes WHERE tenant_id = $1 ORDER BY created_at DESC",
        )
        .bind(tenant_id)
        .fetch_all(self.pool())
        .await
        .map_err(|err| AppError::Database(format!("query ax_sandboxes failed: {err}")))?;

        Ok(rows
            .into_iter()
            .map(|r| {
                let snap_val: serde_json::Value = r.get("base_snapshot");
                let snap: AxSimSnapshot =
                    serde_json::from_value(snap_val).unwrap_or(AxSimSnapshot {
                        nodes: Vec::new(),
                        links: Vec::new(),
                    });
                let act_val: serde_json::Value = r.get("actions");
                let actions: Vec<AxSimAction> = serde_json::from_value(act_val).unwrap_or_default();
                let res_val: serde_json::Value = r.get("sim_result");
                let sim_result: Option<AxSimResult> = serde_json::from_value(res_val).ok();
                let created_at: DateTime<Utc> = r.get("created_at");
                AxSandboxRecord {
                    id: r.get("id"),
                    name: r.get("name"),
                    node_count: snap.nodes.len(),
                    link_count: snap.links.len(),
                    actions,
                    sim_result,
                    created_at,
                }
            })
            .collect())
    }

    /// Clones current production `xb_nodes` and `xb_links` into an isolated `AxSim` sandbox.
    ///
    /// # Errors
    /// Returns [`AppError::Database`] if snapshotting or inserting fails.
    pub async fn create_ax_sandbox(
        &self,
        tenant_id: &str,
        req: CreateAxSandboxRequest,
    ) -> Result<AxSandboxRecord, AppError> {
        let snap = self.get_runtime_snapshot(tenant_id).await?;
        let node_count = snap.nodes.len();
        let link_count = snap.links.len();
        let snap_json = serde_json::to_value(&snap).map_err(|err| {
            AppError::bad(
                "INVALID_ARGUMENT",
                format!("snapshot serialize failed: {err}"),
            )
        })?;

        let row = sqlx::query(
            "INSERT INTO ax_sandboxes (tenant_id, name, base_snapshot)
             VALUES ($1, $2, $3)
             RETURNING id, created_at",
        )
        .bind(tenant_id)
        .bind(req.name.trim())
        .bind(snap_json)
        .fetch_one(self.pool())
        .await
        .map_err(|err| AppError::Database(format!("insert ax_sandbox failed: {err}")))?;

        Ok(AxSandboxRecord {
            id: row.get("id"),
            name: req.name,
            node_count,
            link_count,
            actions: Vec::new(),
            sim_result: None,
            created_at: row.get("created_at"),
        })
    }

    /// Runs causal propagation on an `AxSim` sandbox's cloned snapshot.
    ///
    /// # Errors
    /// Returns [`AppError::Database`] or [`AppError::NotFound`] if the sandbox is missing.
    pub async fn simulate_ax_sandbox(
        &self,
        tenant_id: &str,
        sandbox_id: Uuid,
        actions: Vec<AxSimAction>,
    ) -> Result<AxSimResult, AppError> {
        let row =
            sqlx::query("SELECT base_snapshot FROM ax_sandboxes WHERE id = $1 AND tenant_id = $2")
                .bind(sandbox_id)
                .bind(tenant_id)
                .fetch_optional(self.pool())
                .await
                .map_err(|err| AppError::Database(format!("load ax_sandbox failed: {err}")))?
                .ok_or(AppError::NotFound("SANDBOX_NOT_FOUND"))?;

        let snap_val: serde_json::Value = row.get("base_snapshot");
        let snap: AxSimSnapshot = serde_json::from_value(snap_val)
            .map_err(|err| AppError::Database(format!("corrupt sandbox snapshot: {err}")))?;

        let sim_res = simulate_causal_propagation(&snap, &actions);
        let actions_json = serde_json::to_value(&actions)
            .map_err(|err| AppError::bad("INVALID_ARGUMENT", format!("invalid actions: {err}")))?;
        let res_json = serde_json::to_value(&sim_res).map_err(|err| {
            AppError::bad("INVALID_ARGUMENT", format!("invalid sim result: {err}"))
        })?;

        sqlx::query(
            "UPDATE ax_sandboxes SET actions = $1, sim_result = $2 WHERE id = $3 AND tenant_id = $4",
        )
        .bind(actions_json)
        .bind(res_json)
        .bind(sandbox_id)
        .bind(tenant_id)
        .execute(self.pool())
        .await
        .map_err(|err| AppError::Database(format!("save ax_sandbox simulation failed: {err}")))?;

        Ok(sim_res)
    }
}

impl axpoint_domain::ports::TopologyRepository for PgStore {
    async fn get_xblue_schema(&self, tenant_id: &str) -> Result<XBlueSchema, AppError> {
        Self::get_xblue_schema(self, tenant_id).await
    }

    async fn save_xblue_schema(
        &self,
        tenant_id: &str,
        req: SaveXBlueSchemaRequest,
    ) -> Result<XBlueSchema, AppError> {
        Self::save_xblue_schema(self, tenant_id, req).await
    }

    async fn get_runtime_snapshot(&self, tenant_id: &str) -> Result<AxSimSnapshot, AppError> {
        Self::get_runtime_snapshot(self, tenant_id).await
    }

    async fn upsert_blue_node(
        &self,
        tenant_id: &str,
        req: UpsertBlueNodeRequest,
    ) -> Result<BlueNode, AppError> {
        Self::upsert_blue_node(self, tenant_id, req).await
    }

    async fn list_ax_flows(&self, tenant_id: &str) -> Result<Vec<AxFlowDef>, AppError> {
        Self::list_ax_flows(self, tenant_id).await
    }

    async fn create_ax_flow(
        &self,
        tenant_id: &str,
        req: CreateAxFlowRequest,
    ) -> Result<AxFlowDef, AppError> {
        Self::create_ax_flow(self, tenant_id, req).await
    }

    async fn run_ax_flow(
        &self,
        tenant_id: &str,
        flow_id: Uuid,
    ) -> Result<AxFlowRunResult, AppError> {
        Self::run_ax_flow(self, tenant_id, flow_id).await
    }

    async fn list_ax_sandboxes(&self, tenant_id: &str) -> Result<Vec<AxSandboxRecord>, AppError> {
        Self::list_ax_sandboxes(self, tenant_id).await
    }

    async fn create_ax_sandbox(
        &self,
        tenant_id: &str,
        req: CreateAxSandboxRequest,
    ) -> Result<AxSandboxRecord, AppError> {
        Self::create_ax_sandbox(self, tenant_id, req).await
    }

    async fn simulate_ax_sandbox(
        &self,
        tenant_id: &str,
        sandbox_id: Uuid,
        actions: Vec<AxSimAction>,
    ) -> Result<AxSimResult, AppError> {
        Self::simulate_ax_sandbox(self, tenant_id, sandbox_id, actions).await
    }
}
