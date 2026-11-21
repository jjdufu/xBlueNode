//! Cedar ABAC policies, row-level policies, default-deny toggles, column masking policies,
//! and governed resource classification lookups (`11-STEP13` §13.6–§13.7).

use std::collections::HashMap;

use axpoint_domain::AppError;
use axpoint_domain::enums::{MaskRule, PiiCategory, Sensitivity};
use axpoint_domain::guard::{
    CedarPolicyRecord, ColumnPolicyRecord, GrantResourceKind, PolicyTargetKind,
    PutCedarPolicyRequest, PutColumnPolicyRequest, PutRowPolicyDefaultRequest, ResourceRef,
    RowPolicyDefaultRecord, RowPolicyRecord, SaveRowPolicyRequest, SubjectSelector,
};
use sqlx::postgres::PgRow;
use sqlx::{Postgres, Row as _, Transaction};
use uuid::Uuid;

use super::{is_unique_violation, notify_policy_changed_tx, parse_sensitivity};
use crate::PgStore;
use crate::tenant::begin_tenant_tx;

fn parse_target_kind(raw: &str) -> Result<PolicyTargetKind, AppError> {
    match raw {
        "DATASET" => Ok(PolicyTargetKind::Dataset),
        "NODE_TYPE" => Ok(PolicyTargetKind::NodeType),
        other => Err(AppError::Database(format!(
            "unexpected target_kind: {other}"
        ))),
    }
}

fn decode_cedar_policy(row: &PgRow) -> CedarPolicyRecord {
    CedarPolicyRecord {
        id: row.get("id"),
        tenant_id: row.get("tenant_id"),
        name: row.get("name"),
        description: row.get("description"),
        policy_text: row.get("policy_text"),
        enabled: row.get("enabled"),
        version: row.get("version"),
        updated_by: row.get("updated_by"),
        updated_at: row.get("updated_at"),
    }
}

fn decode_row_policy(row: &PgRow) -> Result<RowPolicyRecord, AppError> {
    let raw_kind: String = row.get("target_kind");
    let raw_applies: serde_json::Value = row.get("applies_to");
    let applies_to: SubjectSelector = serde_json::from_value(raw_applies)
        .map_err(|err| AppError::Database(format!("decode applies_to JSON: {err}")))?;
    Ok(RowPolicyRecord {
        id: row.get("id"),
        tenant_id: row.get("tenant_id"),
        name: row.get("name"),
        target_kind: parse_target_kind(&raw_kind)?,
        target_ref: row.get("target_ref"),
        applies_to,
        predicate: row.get("predicate"),
        enabled: row.get("enabled"),
        created_at: row.get("created_at"),
    })
}

fn decode_column_policy(row: &PgRow) -> Result<ColumnPolicyRecord, AppError> {
    let raw_kind: String = row.get("target_kind");
    let raw_rule: String = row.get("mask_rule");
    let mask_rule: MaskRule = raw_rule.parse()?;
    let raw_exempt: serde_json::Value = row.get("exempt");
    let exempt: SubjectSelector = serde_json::from_value(raw_exempt)
        .map_err(|err| AppError::Database(format!("decode exempt JSON: {err}")))?;
    Ok(ColumnPolicyRecord {
        id: row.get("id"),
        tenant_id: row.get("tenant_id"),
        target_kind: parse_target_kind(&raw_kind)?,
        target_ref: row.get("target_ref"),
        column_name: row.get("column_name"),
        mask_rule,
        exempt,
    })
}

pub(super) async fn list_cedar_policies_impl(
    store: &PgStore,
    tenant_id: &str,
) -> Result<Vec<CedarPolicyRecord>, AppError> {
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let rows = sqlx::query(
        "SELECT id, tenant_id, name, description, policy_text, enabled, version, updated_by, updated_at
           FROM sec_policies
          ORDER BY name ASC",
    )
    .fetch_all(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("list sec_policies failed: {err}")))?;
    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit list_cedar_policies tx: {err}")))?;

    Ok(rows.iter().map(decode_cedar_policy).collect())
}

pub(super) async fn put_cedar_policy_impl(
    store: &PgStore,
    tenant_id: &str,
    name: &str,
    updated_by: Option<Uuid>,
    req: PutCedarPolicyRequest,
) -> Result<CedarPolicyRecord, AppError> {
    let clean_name = name.trim();
    if clean_name.is_empty() || clean_name.len() > 128 {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "policy name must be 1..=128 characters",
        ));
    }
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let row = sqlx::query(
        "INSERT INTO sec_policies (
             tenant_id, name, description, policy_text, enabled, version, updated_by, updated_at
         )
         VALUES ($1, $2, $3, $4, $5, 1, $6, NOW())
         ON CONFLICT (tenant_id, name) DO UPDATE SET
             description = EXCLUDED.description,
             policy_text = EXCLUDED.policy_text,
             enabled = EXCLUDED.enabled,
             version = sec_policies.version + 1,
             updated_by = EXCLUDED.updated_by,
             updated_at = NOW()
         RETURNING id, tenant_id, name, description, policy_text, enabled, version, updated_by, updated_at",
    )
    .bind(tenant_id)
    .bind(clean_name)
    .bind(req.description.trim())
    .bind(&req.policy_text)
    .bind(req.enabled)
    .bind(updated_by)
    .fetch_one(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("upsert sec_policies failed: {err}")))?;

    notify_policy_changed_tx(&mut tx, tenant_id).await?;
    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit put_cedar_policy tx: {err}")))?;
    Ok(decode_cedar_policy(&row))
}

pub(super) async fn delete_cedar_policy_impl(
    store: &PgStore,
    tenant_id: &str,
    name: &str,
) -> Result<bool, AppError> {
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let res = sqlx::query("DELETE FROM sec_policies WHERE name = $1")
        .bind(name.trim())
        .execute(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("delete sec_policies failed: {err}")))?;
    let deleted = res.rows_affected() > 0;
    if deleted {
        notify_policy_changed_tx(&mut tx, tenant_id).await?;
    }
    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit delete_cedar_policy tx: {err}")))?;
    Ok(deleted)
}

pub(super) async fn list_row_policies_impl(
    store: &PgStore,
    tenant_id: &str,
) -> Result<Vec<RowPolicyRecord>, AppError> {
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let rows = sqlx::query(
        "SELECT id, tenant_id, name, target_kind, target_ref, applies_to, predicate, enabled, created_at
           FROM sec_row_policies
          ORDER BY created_at ASC, name ASC",
    )
    .fetch_all(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("list sec_row_policies failed: {err}")))?;
    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit list_row_policies tx: {err}")))?;

    let mut out = Vec::with_capacity(rows.len());
    for r in &rows {
        out.push(decode_row_policy(r)?);
    }
    Ok(out)
}

pub(super) async fn create_row_policy_impl(
    store: &PgStore,
    tenant_id: &str,
    req: SaveRowPolicyRequest,
) -> Result<RowPolicyRecord, AppError> {
    let clean_name = req.name.trim();
    if clean_name.is_empty() {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "row policy name cannot be empty",
        ));
    }
    let applies_json = serde_json::to_value(&req.applies_to)
        .map_err(|err| AppError::bad("INVALID_ARGUMENT", format!("invalid applies_to: {err}")))?;

    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let row = sqlx::query(
        "INSERT INTO sec_row_policies (
             tenant_id, name, target_kind, target_ref, applies_to, predicate, enabled
         )
         VALUES ($1, $2, $3, $4, $5, $6, $7)
         RETURNING id, tenant_id, name, target_kind, target_ref, applies_to, predicate, enabled, created_at",
    )
    .bind(tenant_id)
    .bind(clean_name)
    .bind(req.target_kind.as_str())
    .bind(req.target_ref.trim())
    .bind(applies_json)
    .bind(req.predicate.trim())
    .bind(req.enabled)
    .fetch_one(&mut *tx)
    .await
    .map_err(|err| {
        if is_unique_violation(&err) {
            AppError::Conflict("NAME_TAKEN")
        } else {
            AppError::Database(format!("insert sec_row_policies failed: {err}"))
        }
    })?;

    notify_policy_changed_tx(&mut tx, tenant_id).await?;
    let rec = decode_row_policy(&row)?;
    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit create_row_policy tx: {err}")))?;
    Ok(rec)
}

pub(super) async fn update_row_policy_impl(
    store: &PgStore,
    tenant_id: &str,
    policy_id: Uuid,
    req: SaveRowPolicyRequest,
) -> Result<Option<RowPolicyRecord>, AppError> {
    let clean_name = req.name.trim();
    if clean_name.is_empty() {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "row policy name cannot be empty",
        ));
    }
    let applies_json = serde_json::to_value(&req.applies_to)
        .map_err(|err| AppError::bad("INVALID_ARGUMENT", format!("invalid applies_to: {err}")))?;

    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let row_opt = sqlx::query(
        "UPDATE sec_row_policies
            SET name = $2,
                target_kind = $3,
                target_ref = $4,
                applies_to = $5,
                predicate = $6,
                enabled = $7
          WHERE id = $1
      RETURNING id, tenant_id, name, target_kind, target_ref, applies_to, predicate, enabled, created_at",
    )
    .bind(policy_id)
    .bind(clean_name)
    .bind(req.target_kind.as_str())
    .bind(req.target_ref.trim())
    .bind(applies_json)
    .bind(req.predicate.trim())
    .bind(req.enabled)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|err| {
        if is_unique_violation(&err) {
            AppError::Conflict("NAME_TAKEN")
        } else {
            AppError::Database(format!("update sec_row_policies failed: {err}"))
        }
    })?;

    let Some(row) = row_opt else {
        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit missing row policy tx: {err}")))?;
        return Ok(None);
    };

    notify_policy_changed_tx(&mut tx, tenant_id).await?;
    let rec = decode_row_policy(&row)?;
    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit update_row_policy tx: {err}")))?;
    Ok(Some(rec))
}

pub(super) async fn delete_row_policy_impl(
    store: &PgStore,
    tenant_id: &str,
    policy_id: Uuid,
) -> Result<bool, AppError> {
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let res = sqlx::query("DELETE FROM sec_row_policies WHERE id = $1")
        .bind(policy_id)
        .execute(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("delete sec_row_policies failed: {err}")))?;
    let deleted = res.rows_affected() > 0;
    if deleted {
        notify_policy_changed_tx(&mut tx, tenant_id).await?;
    }
    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit delete_row_policy tx: {err}")))?;
    Ok(deleted)
}

pub(super) async fn list_row_policy_defaults_impl(
    store: &PgStore,
    tenant_id: &str,
) -> Result<Vec<RowPolicyDefaultRecord>, AppError> {
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let rows = sqlx::query(
        "SELECT tenant_id, target_kind, target_ref, default_deny
           FROM sec_row_policy_defaults
          ORDER BY target_kind ASC, target_ref ASC",
    )
    .fetch_all(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("list sec_row_policy_defaults failed: {err}")))?;
    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit list_row_policy_defaults tx: {err}")))?;

    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let raw_kind: String = r.get("target_kind");
        out.push(RowPolicyDefaultRecord {
            tenant_id: r.get("tenant_id"),
            target_kind: parse_target_kind(&raw_kind)?,
            target_ref: r.get("target_ref"),
            default_deny: r.get("default_deny"),
        });
    }
    Ok(out)
}

pub(super) async fn put_row_policy_default_impl(
    store: &PgStore,
    tenant_id: &str,
    req: PutRowPolicyDefaultRequest,
) -> Result<RowPolicyDefaultRecord, AppError> {
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let row = sqlx::query(
        "INSERT INTO sec_row_policy_defaults (tenant_id, target_kind, target_ref, default_deny)
         VALUES ($1, $2, $3, $4)
         ON CONFLICT (tenant_id, target_kind, target_ref) DO UPDATE SET
             default_deny = EXCLUDED.default_deny
         RETURNING tenant_id, target_kind, target_ref, default_deny",
    )
    .bind(tenant_id)
    .bind(req.target_kind.as_str())
    .bind(req.target_ref.trim())
    .bind(req.default_deny)
    .fetch_one(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("upsert sec_row_policy_defaults failed: {err}")))?;

    notify_policy_changed_tx(&mut tx, tenant_id).await?;
    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit put_row_policy_default tx: {err}")))?;

    let raw_kind: String = row.get("target_kind");
    Ok(RowPolicyDefaultRecord {
        tenant_id: row.get("tenant_id"),
        target_kind: parse_target_kind(&raw_kind)?,
        target_ref: row.get("target_ref"),
        default_deny: row.get("default_deny"),
    })
}

pub(super) async fn list_column_policies_impl(
    store: &PgStore,
    tenant_id: &str,
) -> Result<Vec<ColumnPolicyRecord>, AppError> {
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let rows = sqlx::query(
        "SELECT id, tenant_id, target_kind, target_ref, column_name, mask_rule, exempt
           FROM sec_column_policies
          ORDER BY target_kind ASC, target_ref ASC, column_name ASC",
    )
    .fetch_all(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("list sec_column_policies failed: {err}")))?;
    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit list_column_policies tx: {err}")))?;

    let mut out = Vec::with_capacity(rows.len());
    for r in &rows {
        out.push(decode_column_policy(r)?);
    }
    Ok(out)
}

pub(super) async fn put_column_policy_impl(
    store: &PgStore,
    tenant_id: &str,
    req: PutColumnPolicyRequest,
) -> Result<ColumnPolicyRecord, AppError> {
    let clean_col = req.column_name.trim();
    if clean_col.is_empty() {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "column_name cannot be empty",
        ));
    }
    let exempt_json = serde_json::to_value(&req.exempt)
        .map_err(|err| AppError::bad("INVALID_ARGUMENT", format!("invalid exempt: {err}")))?;

    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let row = sqlx::query(
        "INSERT INTO sec_column_policies (
             tenant_id, target_kind, target_ref, column_name, mask_rule, exempt
         )
         VALUES ($1, $2, $3, $4, $5, $6)
         ON CONFLICT (tenant_id, target_kind, target_ref, column_name) DO UPDATE SET
             mask_rule = EXCLUDED.mask_rule,
             exempt = EXCLUDED.exempt
         RETURNING id, tenant_id, target_kind, target_ref, column_name, mask_rule, exempt",
    )
    .bind(tenant_id)
    .bind(req.target_kind.as_str())
    .bind(req.target_ref.trim())
    .bind(clean_col)
    .bind(req.mask_rule.as_str())
    .bind(exempt_json)
    .fetch_one(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("upsert sec_column_policies failed: {err}")))?;

    notify_policy_changed_tx(&mut tx, tenant_id).await?;
    let rec = decode_column_policy(&row)?;
    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit put_column_policy tx: {err}")))?;
    Ok(rec)
}

pub(super) async fn delete_column_policy_impl(
    store: &PgStore,
    tenant_id: &str,
    policy_id: Uuid,
) -> Result<bool, AppError> {
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let res = sqlx::query("DELETE FROM sec_column_policies WHERE id = $1")
        .bind(policy_id)
        .execute(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("delete sec_column_policies failed: {err}")))?;
    let deleted = res.rows_affected() > 0;
    if deleted {
        notify_policy_changed_tx(&mut tx, tenant_id).await?;
    }
    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit delete_column_policy tx: {err}")))?;
    Ok(deleted)
}

pub(crate) async fn resolve_resource_ref_tx(
    tx: &mut Transaction<'_, Postgres>,
    kind: GrantResourceKind,
    resource_ref: &str,
) -> Result<Option<ResourceRef>, AppError> {
    let trimmed = resource_ref.trim();
    let row_opt = match kind {
        GrantResourceKind::Dataset => {
            if let Ok(ds_id) = Uuid::parse_str(trimmed) {
                sqlx::query(
                    "SELECT id::text AS res_id, owner_dept, sensitivity
                       FROM ds_datasets
                      WHERE id = $1",
                )
                .bind(ds_id)
                .fetch_optional(&mut **tx)
                .await
            } else {
                sqlx::query(
                    "SELECT id::text AS res_id, owner_dept, sensitivity
                       FROM ds_datasets
                      WHERE name = $1",
                )
                .bind(trimmed)
                .fetch_optional(&mut **tx)
                .await
            }
        }
        GrantResourceKind::NodeType => {
            sqlx::query(
                "SELECT name AS res_id, owner_dept, sensitivity
                   FROM xb_node_types
                  WHERE subgraph_slug = 'main' AND name = $1",
            )
            .bind(trimmed)
            .fetch_optional(&mut **tx)
            .await
        }
        GrantResourceKind::Subgraph => {
            sqlx::query(
                "SELECT slug AS res_id, owner_dept, sensitivity
                   FROM xb_subgraphs
                  WHERE slug = $1",
            )
            .bind(trimmed)
            .fetch_optional(&mut **tx)
            .await
        }
        GrantResourceKind::Dashboard => {
            if let Ok(dash_id) = Uuid::parse_str(trimmed) {
                sqlx::query(
                    "SELECT id::text AS res_id, owner_dept, sensitivity
                       FROM xv_dashboards
                      WHERE id = $1",
                )
                .bind(dash_id)
                .fetch_optional(&mut **tx)
                .await
            } else {
                sqlx::query(
                    "SELECT id::text AS res_id, owner_dept, sensitivity
                       FROM xv_dashboards
                      WHERE slug = $1",
                )
                .bind(trimmed)
                .fetch_optional(&mut **tx)
                .await
            }
        }
        GrantResourceKind::Agent => {
            if let Ok(agent_id) = Uuid::parse_str(trimmed) {
                sqlx::query(
                    "SELECT id::text AS res_id, owner_dept, sensitivity
                       FROM ax_agents
                      WHERE id = $1",
                )
                .bind(agent_id)
                .fetch_optional(&mut **tx)
                .await
            } else {
                sqlx::query(
                    "SELECT id::text AS res_id, owner_dept, sensitivity
                       FROM ax_agents
                      WHERE name = $1",
                )
                .bind(trimmed)
                .fetch_optional(&mut **tx)
                .await
            }
        }
    }
    .map_err(|err| AppError::Database(format!("resolve_resource_ref query failed: {err}")))?;

    let Some(row) = row_opt else {
        return Ok(None);
    };
    let raw_sens: String = row.get("sensitivity");
    Ok(Some(ResourceRef {
        kind,
        id: row.get("res_id"),
        owner_dept: row.get("owner_dept"),
        sensitivity: parse_sensitivity(&raw_sens)?,
    }))
}

pub(super) async fn resolve_resource_ref_impl(
    store: &PgStore,
    tenant_id: &str,
    kind: GrantResourceKind,
    resource_ref: &str,
) -> Result<Option<ResourceRef>, AppError> {
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let res = resolve_resource_ref_tx(&mut tx, kind, resource_ref).await?;
    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit resolve_resource_ref tx: {err}")))?;
    Ok(res)
}

impl PgStore {
    /// Batch-loads `(id, name, owner_dept, sensitivity)` for all active datasets in `tenant_id` (`11-STEP13` §13.6.3).
    ///
    /// # Errors
    /// Returns [`AppError::Database`] if querying `ds_datasets` fails.
    pub async fn list_dataset_resource_refs(
        &self,
        tenant_id: &str,
    ) -> Result<Vec<(Uuid, String, String, Sensitivity)>, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let rows = sqlx::query(
            "SELECT id, name, owner_dept, sensitivity
               FROM ds_datasets
              WHERE status <> 'DISABLED'
              ORDER BY name ASC",
        )
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("list_dataset_resource_refs failed: {err}")))?;
        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit list_dataset_resource_refs tx: {err}"))
        })?;

        let mut out = Vec::with_capacity(rows.len());
        for r in rows {
            let raw_sens: String = r.get("sensitivity");
            out.push((
                r.get("id"),
                r.get("name"),
                r.get("owner_dept"),
                parse_sensitivity(&raw_sens)?,
            ));
        }
        Ok(out)
    }

    /// Batch-loads `(column_name, pii_category)` grouped by `dataset_id` for all datasets in `tenant_id`.
    ///
    /// # Errors
    /// Returns [`AppError::Database`] if querying `ds_column_meta` fails.
    pub async fn list_all_dataset_column_pii(
        &self,
        tenant_id: &str,
    ) -> Result<HashMap<Uuid, Vec<(String, PiiCategory)>>, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let rows = sqlx::query(
            "SELECT dataset_id, column_name, pii_category
               FROM ds_column_meta
              ORDER BY dataset_id, ordinal ASC",
        )
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("list_all_dataset_column_pii failed: {err}")))?;
        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit list_all_dataset_column_pii tx: {err}"))
        })?;

        let mut map: HashMap<Uuid, Vec<(String, PiiCategory)>> = HashMap::new();
        for r in rows {
            let dsid: Uuid = r.get("dataset_id");
            let col: String = r.get("column_name");
            let raw_cat: String = r.get("pii_category");
            let cat: PiiCategory = raw_cat.parse()?;
            map.entry(dsid).or_default().push((col, cat));
        }
        Ok(map)
    }

    /// Batch-loads `(name, owner_dept, sensitivity)` for all `main` subgraph node types in `tenant_id`.
    ///
    /// # Errors
    /// Returns [`AppError::Database`] if querying `xb_node_types` fails.
    pub async fn list_node_type_resource_refs(
        &self,
        tenant_id: &str,
    ) -> Result<Vec<(String, String, Sensitivity)>, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let rows = sqlx::query(
            "SELECT name, owner_dept, sensitivity
               FROM xb_node_types
              WHERE subgraph_slug = 'main'
              ORDER BY name ASC",
        )
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("list_node_type_resource_refs failed: {err}")))?;
        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit list_node_type_resource_refs tx: {err}"))
        })?;

        let mut out = Vec::with_capacity(rows.len());
        for r in rows {
            let raw_sens: String = r.get("sensitivity");
            out.push((
                r.get("name"),
                r.get("owner_dept"),
                parse_sensitivity(&raw_sens)?,
            ));
        }
        Ok(out)
    }
}
