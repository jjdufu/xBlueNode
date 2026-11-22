//! Enterprise IM outbound connector persistence, single-leader lease management,
//! event deduplication, atomic challenge binding, and `AxGuard` principal hydration (`07-STEP21`).

use std::collections::{BTreeMap, BTreeSet};

use axpoint_domain::AppError;
use axpoint_domain::agent::ConversationRecord;
use axpoint_domain::enums::{
    ConversationChannel, ConversationStatus, ImConnectorStatus, ImIdentityPolicy, ImPlatform,
    Permission, Sensitivity,
};
use axpoint_domain::im::{
    ConsumedImBindChallenge, HydratedImPrincipal, ImBindChallengePreview, ImConnectorConfig,
    ImConnectorRecord, ImUserBindingRecord, NewImBindChallengeInput, NewImConnectorInput,
    UpdateImConnectorInput, apply_im_identity_guardrails,
};
use axpoint_domain::platform::PrincipalRecord;
use axpoint_domain::ports::{ImGatewayRepository, PrincipalHydrationPort};
use chrono::{DateTime, Utc};
use sqlx::postgres::PgRow;
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

use crate::PgStore;
use crate::tenant::begin_tenant_tx;

fn map_im_db_err(context: &str, err: &sqlx::Error) -> AppError {
    if err
        .as_database_error()
        .and_then(sqlx::error::DatabaseError::code)
        .as_deref()
        == Some("23505")
    {
        return AppError::Conflict("NAME_TAKEN");
    }
    AppError::Database(format!("{context} failed: {err}"))
}

fn decode_im_connector_row(row: &PgRow) -> Result<ImConnectorRecord, AppError> {
    let id: Uuid = row
        .try_get("id")
        .map_err(|e| AppError::Database(format!("decode ax_im_connectors.id: {e}")))?;
    let tenant_id: String = row
        .try_get("tenant_id")
        .map_err(|e| AppError::Database(format!("decode ax_im_connectors.tenant_id: {e}")))?;
    let name: String = row
        .try_get("name")
        .map_err(|e| AppError::Database(format!("decode ax_im_connectors.name: {e}")))?;
    let raw_platform: String = row
        .try_get("platform")
        .map_err(|e| AppError::Database(format!("decode ax_im_connectors.platform: {e}")))?;
    let platform: ImPlatform = raw_platform.parse()?;
    let agent_id: Uuid = row
        .try_get("agent_id")
        .map_err(|e| AppError::Database(format!("decode ax_im_connectors.agent_id: {e}")))?;
    let credential_id: Uuid = row
        .try_get("credential_id")
        .map_err(|e| AppError::Database(format!("decode ax_im_connectors.credential_id: {e}")))?;
    let raw_cfg: serde_json::Value = row
        .try_get("config")
        .map_err(|e| AppError::Database(format!("decode ax_im_connectors.config: {e}")))?;
    let config: ImConnectorConfig = serde_json::from_value(raw_cfg)
        .map_err(|e| AppError::Database(format!("parse ax_im_connectors.config JSON: {e}")))?;
    let raw_policy: String = row
        .try_get("identity_policy")
        .map_err(|e| AppError::Database(format!("decode ax_im_connectors.identity_policy: {e}")))?;
    let identity_policy: ImIdentityPolicy = raw_policy.parse()?;
    let delegated_user_id: Option<Uuid> = row.try_get("delegated_user_id").map_err(|e| {
        AppError::Database(format!("decode ax_im_connectors.delegated_user_id: {e}"))
    })?;
    let enabled: bool = row
        .try_get("enabled")
        .map_err(|e| AppError::Database(format!("decode ax_im_connectors.enabled: {e}")))?;
    let leader_holder: Option<String> = row
        .try_get("leader_holder")
        .map_err(|e| AppError::Database(format!("decode ax_im_connectors.leader_holder: {e}")))?;
    let lease_until: Option<DateTime<Utc>> = row
        .try_get("lease_until")
        .map_err(|e| AppError::Database(format!("decode ax_im_connectors.lease_until: {e}")))?;
    let raw_status: String = row
        .try_get("status")
        .map_err(|e| AppError::Database(format!("decode ax_im_connectors.status: {e}")))?;
    let status: ImConnectorStatus = raw_status.parse()?;
    let last_error: String = row
        .try_get("last_error")
        .map_err(|e| AppError::Database(format!("decode ax_im_connectors.last_error: {e}")))?;
    let last_heartbeat_at: Option<DateTime<Utc>> = row.try_get("last_heartbeat_at").map_err(|e| {
        AppError::Database(format!("decode ax_im_connectors.last_heartbeat_at: {e}"))
    })?;
    let created_by: Option<Uuid> = row
        .try_get("created_by")
        .map_err(|e| AppError::Database(format!("decode ax_im_connectors.created_by: {e}")))?;
    let created_at: DateTime<Utc> = row
        .try_get("created_at")
        .map_err(|e| AppError::Database(format!("decode ax_im_connectors.created_at: {e}")))?;
    let updated_at: DateTime<Utc> = row
        .try_get("updated_at")
        .map_err(|e| AppError::Database(format!("decode ax_im_connectors.updated_at: {e}")))?;

    Ok(ImConnectorRecord {
        id,
        tenant_id,
        name,
        platform,
        agent_id,
        credential_id,
        has_secret: !credential_id.is_nil(),
        config,
        identity_policy,
        delegated_user_id,
        enabled,
        leader_holder,
        lease_until,
        status,
        last_error,
        last_heartbeat_at,
        created_by,
        created_at,
        updated_at,
    })
}

fn decode_im_binding_row(row: &PgRow) -> Result<ImUserBindingRecord, AppError> {
    let id: Uuid = row
        .try_get("id")
        .map_err(|e| AppError::Database(format!("decode ax_im_user_bindings.id: {e}")))?;
    let tenant_id: String = row
        .try_get("tenant_id")
        .map_err(|e| AppError::Database(format!("decode ax_im_user_bindings.tenant_id: {e}")))?;
    let raw_platform: String = row
        .try_get("platform")
        .map_err(|e| AppError::Database(format!("decode ax_im_user_bindings.platform: {e}")))?;
    let platform: ImPlatform = raw_platform.parse()?;
    let external_user_id: String = row.try_get("external_user_id").map_err(|e| {
        AppError::Database(format!("decode ax_im_user_bindings.external_user_id: {e}"))
    })?;
    let external_display_name: String = row.try_get("external_display_name").map_err(|e| {
        AppError::Database(format!(
            "decode ax_im_user_bindings.external_display_name: {e}"
        ))
    })?;
    let user_id: Uuid = row
        .try_get("user_id")
        .map_err(|e| AppError::Database(format!("decode ax_im_user_bindings.user_id: {e}")))?;
    let username: Option<String> = row
        .try_get("username")
        .map_err(|e| AppError::Database(format!("decode ax_im_user_bindings.username: {e}")))?;
    let bound_at: DateTime<Utc> = row
        .try_get("bound_at")
        .map_err(|e| AppError::Database(format!("decode ax_im_user_bindings.bound_at: {e}")))?;

    Ok(ImUserBindingRecord {
        id,
        tenant_id,
        platform,
        external_user_id,
        external_display_name,
        user_id,
        username,
        bound_at,
    })
}

fn decode_im_conversation_row(row: &PgRow) -> Result<ConversationRecord, AppError> {
    let id: Uuid = row
        .try_get("id")
        .map_err(|e| AppError::Database(format!("decode ax_conversations.id: {e}")))?;
    let tenant_id: String = row
        .try_get("tenant_id")
        .map_err(|e| AppError::Database(format!("decode ax_conversations.tenant_id: {e}")))?;
    let agent_id: Uuid = row
        .try_get("agent_id")
        .map_err(|e| AppError::Database(format!("decode ax_conversations.agent_id: {e}")))?;
    let agent_version: i32 = row
        .try_get("agent_version")
        .map_err(|e| AppError::Database(format!("decode ax_conversations.agent_version: {e}")))?;
    let user_id: Option<Uuid> = row
        .try_get("user_id")
        .map_err(|e| AppError::Database(format!("decode ax_conversations.user_id: {e}")))?;
    let raw_chan: String = row
        .try_get("channel")
        .map_err(|e| AppError::Database(format!("decode ax_conversations.channel: {e}")))?;
    let channel: ConversationChannel = raw_chan.parse()?;
    let embed_id: Option<Uuid> = row
        .try_get("embed_id")
        .map_err(|e| AppError::Database(format!("decode ax_conversations.embed_id: {e}")))?;
    let im_connector_id: Option<Uuid> = row
        .try_get("im_connector_id")
        .map_err(|e| AppError::Database(format!("decode ax_conversations.im_connector_id: {e}")))?;
    let external_chat_id: Option<String> = row.try_get("external_chat_id").map_err(|e| {
        AppError::Database(format!("decode ax_conversations.external_chat_id: {e}"))
    })?;
    let title: String = row
        .try_get("title")
        .map_err(|e| AppError::Database(format!("decode ax_conversations.title: {e}")))?;
    let raw_status: String = row
        .try_get("status")
        .map_err(|e| AppError::Database(format!("decode ax_conversations.status: {e}")))?;
    let status: ConversationStatus = raw_status.parse()?;
    let created_at: DateTime<Utc> = row
        .try_get("created_at")
        .map_err(|e| AppError::Database(format!("decode ax_conversations.created_at: {e}")))?;
    let updated_at: DateTime<Utc> = row
        .try_get("updated_at")
        .map_err(|e| AppError::Database(format!("decode ax_conversations.updated_at: {e}")))?;

    Ok(ConversationRecord {
        id,
        tenant_id,
        agent_id,
        agent_version,
        user_id,
        channel,
        embed_id,
        im_connector_id,
        external_chat_id,
        title,
        status,
        created_at,
        updated_at,
    })
}

async fn insert_im_credential_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    credential_id: Uuid,
    sealed: &axpoint_domain::axhub::SealedCredential,
) -> Result<(), AppError> {
    sqlx::query(
        "INSERT INTO ds_credentials (id, tenant_id, key_version, nonce, ciphertext)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(credential_id)
    .bind(tenant_id)
    .bind(sealed.key_version)
    .bind(&sealed.nonce)
    .bind(&sealed.ciphertext)
    .execute(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("insert ds_credentials failed: {err}")))?;
    Ok(())
}

async fn load_roles_and_permissions_tx(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
) -> Result<(Vec<String>, BTreeSet<Permission>), AppError> {
    let role_rows = sqlx::query(
        "SELECT r.name, r.permissions
           FROM sec_user_roles ur
           JOIN sec_roles r ON r.id = ur.role_id
          WHERE ur.user_id = $1
          ORDER BY r.name",
    )
    .bind(user_id)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("load sec_user_roles failed: {err}")))?;

    let mut roles = Vec::with_capacity(role_rows.len());
    let mut permissions = BTreeSet::new();
    for row in role_rows {
        let rname: String = row
            .try_get("name")
            .map_err(|err| AppError::Database(format!("decode sec_roles.name: {err}")))?;
        let raw_perms: Vec<String> = row
            .try_get("permissions")
            .map_err(|err| AppError::Database(format!("decode sec_roles.permissions: {err}")))?;
        roles.push(rname);
        for raw in raw_perms {
            let perm: Permission = raw.parse()?;
            permissions.insert(perm);
        }
    }
    Ok((roles, permissions))
}

async fn load_user_attributes_tx(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
) -> Result<BTreeMap<String, Vec<String>>, AppError> {
    let attr_rows = sqlx::query(
        "SELECT attr_key, attr_values
           FROM sec_user_attributes
          WHERE user_id = $1
          ORDER BY attr_key",
    )
    .bind(user_id)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("load sec_user_attributes failed: {err}")))?;

    let mut attributes = BTreeMap::new();
    for row in attr_rows {
        let key: String = row.try_get("attr_key").map_err(|err| {
            AppError::Database(format!("decode sec_user_attributes.attr_key: {err}"))
        })?;
        let vals: Vec<String> = row.try_get("attr_values").map_err(|err| {
            AppError::Database(format!("decode sec_user_attributes.attr_values: {err}"))
        })?;
        attributes.insert(key, vals);
    }
    Ok(attributes)
}

impl ImGatewayRepository for PgStore {
    async fn list_im_connectors(&self, tenant_id: &str) -> Result<Vec<ImConnectorRecord>, AppError> {
        let mut tx = begin_tenant_tx(&self.pool, tenant_id).await?;
        let rows = sqlx::query("SELECT * FROM ax_im_connectors ORDER BY created_at DESC, name ASC")
            .fetch_all(&mut *tx)
            .await
            .map_err(|err| AppError::Database(format!("list ax_im_connectors failed: {err}")))?;

        let mut items = Vec::with_capacity(rows.len());
        for row in &rows {
            items.push(decode_im_connector_row(row)?);
        }
        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit list_im_connectors tx: {err}")))?;
        Ok(items)
    }

    async fn get_im_connector(
        &self,
        tenant_id: &str,
        connector_id: Uuid,
    ) -> Result<Option<ImConnectorRecord>, AppError> {
        let mut tx = begin_tenant_tx(&self.pool, tenant_id).await?;
        let row_opt = sqlx::query("SELECT * FROM ax_im_connectors WHERE id = $1")
            .bind(connector_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|err| AppError::Database(format!("get ax_im_connectors failed: {err}")))?;

        let rec = match row_opt {
            Some(ref r) => Some(decode_im_connector_row(r)?),
            None => None,
        };
        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit get_im_connector tx: {err}")))?;
        Ok(rec)
    }

    async fn create_im_connector(
        &self,
        tenant_id: &str,
        input: NewImConnectorInput,
    ) -> Result<ImConnectorRecord, AppError> {
        let config_json = serde_json::to_value(&input.config)
            .map_err(|err| AppError::Config(format!("serialize ImConnectorConfig failed: {err}")))?;
        let mut tx = begin_tenant_tx(&self.pool, tenant_id).await?;
        insert_im_credential_tx(&mut tx, tenant_id, input.credential_id, &input.credential).await?;

        let row = sqlx::query(
            "INSERT INTO ax_im_connectors (
                tenant_id, name, platform, agent_id, credential_id, config,
                identity_policy, delegated_user_id, enabled, status, created_by
             )
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, 'DISCONNECTED', $10)
             RETURNING *",
        )
        .bind(tenant_id)
        .bind(&input.name)
        .bind(input.platform.as_str())
        .bind(input.agent_id)
        .bind(input.credential_id)
        .bind(config_json)
        .bind(input.identity_policy.as_str())
        .bind(input.delegated_user_id)
        .bind(input.enabled)
        .bind(input.created_by)
        .fetch_one(&mut *tx)
        .await
        .map_err(|err| map_im_db_err("insert ax_im_connectors", &err))?;

        let rec = decode_im_connector_row(&row)?;
        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit create_im_connector tx: {err}")))?;
        Ok(rec)
    }

    async fn update_im_connector(
        &self,
        tenant_id: &str,
        connector_id: Uuid,
        input: UpdateImConnectorInput,
    ) -> Result<Option<ImConnectorRecord>, AppError> {
        let config_json = serde_json::to_value(&input.config)
            .map_err(|err| AppError::Config(format!("serialize ImConnectorConfig failed: {err}")))?;
        let mut tx = begin_tenant_tx(&self.pool, tenant_id).await?;

        let locked_opt =
            sqlx::query("SELECT credential_id FROM ax_im_connectors WHERE id = $1 FOR UPDATE")
                .bind(connector_id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(|err| {
                    AppError::Database(format!("lock ax_im_connectors FOR UPDATE failed: {err}"))
                })?;

        let Some(locked_row) = locked_opt else {
            tx.commit()
                .await
                .map_err(|err| AppError::Database(format!("commit missing connector tx: {err}")))?;
            return Ok(None);
        };
        let old_cred_id: Uuid = locked_row
            .try_get("credential_id")
            .map_err(|err| AppError::Database(format!("decode credential_id: {err}")))?;

        let effective_cred_id = if let Some((new_cred_id, ref sealed)) = input.rotated_credential {
            insert_im_credential_tx(&mut tx, tenant_id, new_cred_id, sealed).await?;
            new_cred_id
        } else {
            old_cred_id
        };

        let row = sqlx::query(
            "UPDATE ax_im_connectors
                SET name              = $2,
                    agent_id          = $3,
                    credential_id     = $4,
                    config            = $5,
                    identity_policy   = $6,
                    delegated_user_id = $7,
                    enabled           = $8,
                    leader_holder     = CASE WHEN $8 THEN leader_holder ELSE NULL END,
                    lease_until       = CASE WHEN $8 THEN lease_until ELSE NULL END,
                    status            = CASE WHEN $8 THEN status ELSE 'DISCONNECTED' END,
                    last_error        = CASE WHEN $8 THEN last_error ELSE '' END,
                    updated_at        = NOW()
              WHERE id = $1
             RETURNING *",
        )
        .bind(connector_id)
        .bind(&input.name)
        .bind(input.agent_id)
        .bind(effective_cred_id)
        .bind(config_json)
        .bind(input.identity_policy.as_str())
        .bind(input.delegated_user_id)
        .bind(input.enabled)
        .fetch_one(&mut *tx)
        .await
        .map_err(|err| map_im_db_err("update ax_im_connectors", &err))?;

        if effective_cred_id != old_cred_id {
            sqlx::query("DELETE FROM ds_credentials WHERE id = $1")
                .bind(old_cred_id)
                .execute(&mut *tx)
                .await
                .map_err(|err| {
                    AppError::Database(format!("delete rotated ds_credentials failed: {err}"))
                })?;
        }

        let rec = decode_im_connector_row(&row)?;
        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit update_im_connector tx: {err}")))?;
        Ok(Some(rec))
    }

    async fn delete_im_connector(
        &self,
        tenant_id: &str,
        connector_id: Uuid,
    ) -> Result<bool, AppError> {
        let mut tx = begin_tenant_tx(&self.pool, tenant_id).await?;
        let deleted_opt =
            sqlx::query("DELETE FROM ax_im_connectors WHERE id = $1 RETURNING credential_id")
                .bind(connector_id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(|err| {
                    AppError::Database(format!("delete ax_im_connectors failed: {err}"))
                })?;

        let Some(deleted_row) = deleted_opt else {
            tx.commit().await.map_err(|err| {
                AppError::Database(format!("commit delete_im_connector tx: {err}"))
            })?;
            return Ok(false);
        };
        let cred_id: Uuid = deleted_row
            .try_get("credential_id")
            .map_err(|err| AppError::Database(format!("decode credential_id: {err}")))?;

        sqlx::query("DELETE FROM ds_credentials WHERE id = $1")
            .bind(cred_id)
            .execute(&mut *tx)
            .await
            .map_err(|err| AppError::Database(format!("delete ds_credentials failed: {err}")))?;

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit delete_im_connector tx: {err}")))?;
        Ok(true)
    }

    async fn claim_im_connectors(
        &self,
        holder_id: &str,
        limit: i32,
        lease_seconds: i32,
    ) -> Result<Vec<ImConnectorRecord>, AppError> {
        let rows = sqlx::query("SELECT * FROM ax_claim_im_connectors($1, $2, $3)")
            .bind(holder_id)
            .bind(limit)
            .bind(lease_seconds)
            .fetch_all(&self.pool)
            .await
            .map_err(|err| AppError::Database(format!("ax_claim_im_connectors failed: {err}")))?;

        let mut out = Vec::with_capacity(rows.len());
        for row in &rows {
            out.push(decode_im_connector_row(row)?);
        }
        Ok(out)
    }

    async fn release_im_connectors(&self, holder_id: &str) -> Result<i32, AppError> {
        let row = sqlx::query("SELECT ax_release_im_connectors($1) AS released")
            .bind(holder_id)
            .fetch_one(&self.pool)
            .await
            .map_err(|err| AppError::Database(format!("ax_release_im_connectors failed: {err}")))?;
        row.try_get("released")
            .map_err(|err| AppError::Database(format!("decode released count: {err}")))
    }

    async fn update_im_connector_runtime_state(
        &self,
        tenant_id: &str,
        connector_id: Uuid,
        holder_id: &str,
        status: ImConnectorStatus,
        last_error: &str,
    ) -> Result<bool, AppError> {
        let mut tx = begin_tenant_tx(&self.pool, tenant_id).await?;
        let res = sqlx::query(
            "UPDATE ax_im_connectors
                SET status            = $3,
                    last_error        = $4,
                    last_heartbeat_at = NOW(),
                    updated_at        = NOW()
              WHERE id = $1
                AND leader_holder = $2",
        )
        .bind(connector_id)
        .bind(holder_id)
        .bind(status.as_str())
        .bind(last_error)
        .execute(&mut *tx)
        .await
        .map_err(|err| {
            AppError::Database(format!("update ax_im_connectors runtime state: {err}"))
        })?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit update_im_connector_runtime_state: {err}"))
        })?;
        Ok(res.rows_affected() > 0)
    }

    async fn record_im_event_dedup(
        &self,
        tenant_id: &str,
        connector_id: Uuid,
        event_id: &str,
    ) -> Result<bool, AppError> {
        let mut tx = begin_tenant_tx(&self.pool, tenant_id).await?;
        let res = sqlx::query(
            "INSERT INTO ax_im_event_dedup (tenant_id, connector_id, event_id)
             VALUES ($1, $2, $3)
             ON CONFLICT (connector_id, event_id) DO NOTHING",
        )
        .bind(tenant_id)
        .bind(connector_id)
        .bind(event_id)
        .execute(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("insert ax_im_event_dedup failed: {err}")))?;

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit record_im_event_dedup tx: {err}")))?;
        Ok(res.rows_affected() > 0)
    }

    async fn get_im_user_binding(
        &self,
        tenant_id: &str,
        platform: ImPlatform,
        external_user_id: &str,
    ) -> Result<Option<ImUserBindingRecord>, AppError> {
        let mut tx = begin_tenant_tx(&self.pool, tenant_id).await?;
        let row_opt = sqlx::query(
            "SELECT b.id, b.tenant_id, b.platform, b.external_user_id,
                    b.external_display_name, b.user_id, u.username, b.bound_at
               FROM ax_im_user_bindings b
               LEFT JOIN sec_users u ON u.id = b.user_id AND u.tenant_id = b.tenant_id
              WHERE b.platform = $1
                AND b.external_user_id = $2",
        )
        .bind(platform.as_str())
        .bind(external_user_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("get ax_im_user_bindings failed: {err}")))?;

        let rec = match row_opt {
            Some(ref r) => Some(decode_im_binding_row(r)?),
            None => None,
        };
        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit get_im_user_binding tx: {err}")))?;
        Ok(rec)
    }

    async fn list_im_user_bindings(
        &self,
        tenant_id: &str,
        only_user_id: Option<Uuid>,
    ) -> Result<Vec<ImUserBindingRecord>, AppError> {
        let mut tx = begin_tenant_tx(&self.pool, tenant_id).await?;
        let rows = sqlx::query(
            "SELECT b.id, b.tenant_id, b.platform, b.external_user_id,
                    b.external_display_name, b.user_id, u.username, b.bound_at
               FROM ax_im_user_bindings b
               LEFT JOIN sec_users u ON u.id = b.user_id AND u.tenant_id = b.tenant_id
              WHERE ($1::uuid IS NULL OR b.user_id = $1)
              ORDER BY b.bound_at DESC, b.id DESC",
        )
        .bind(only_user_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("list ax_im_user_bindings failed: {err}")))?;

        let mut items = Vec::with_capacity(rows.len());
        for row in &rows {
            items.push(decode_im_binding_row(row)?);
        }
        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit list_im_user_bindings tx: {err}")))?;
        Ok(items)
    }

    async fn delete_im_user_binding(
        &self,
        tenant_id: &str,
        binding_id: Uuid,
        only_user_id: Option<Uuid>,
    ) -> Result<Option<ImUserBindingRecord>, AppError> {
        let mut tx = begin_tenant_tx(&self.pool, tenant_id).await?;
        let row_opt = sqlx::query(
            "DELETE FROM ax_im_user_bindings
              WHERE id = $1
                AND ($2::uuid IS NULL OR user_id = $2)
             RETURNING id, tenant_id, platform, external_user_id,
                       external_display_name, user_id, NULL::text AS username, bound_at",
        )
        .bind(binding_id)
        .bind(only_user_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("delete ax_im_user_bindings failed: {err}")))?;

        let rec = match row_opt {
            Some(ref r) => Some(decode_im_binding_row(r)?),
            None => None,
        };
        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit delete_im_user_binding tx: {err}"))
        })?;
        Ok(rec)
    }

    async fn create_im_bind_challenge(
        &self,
        tenant_id: &str,
        input: NewImBindChallengeInput,
    ) -> Result<(), AppError> {
        let mut tx = begin_tenant_tx(&self.pool, tenant_id).await?;
        sqlx::query(
            "INSERT INTO ax_im_bind_challenges (
                token_hash, tenant_id, connector_id, platform,
                external_user_id, external_display_name, expires_at
             )
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(&input.token_hash)
        .bind(tenant_id)
        .bind(input.connector_id)
        .bind(input.platform.as_str())
        .bind(&input.external_user_id)
        .bind(&input.external_display_name)
        .bind(input.expires_at)
        .execute(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("insert ax_im_bind_challenges failed: {err}")))?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit create_im_bind_challenge tx: {err}"))
        })?;
        Ok(())
    }

    async fn preview_im_bind_challenge(
        &self,
        tenant_id: &str,
        token_hash: &str,
    ) -> Result<Option<ImBindChallengePreview>, AppError> {
        let mut tx = begin_tenant_tx(&self.pool, tenant_id).await?;
        let row_opt = sqlx::query(
            "SELECT ch.connector_id,
                    c.name AS connector_name,
                    ch.platform,
                    ch.external_user_id,
                    ch.external_display_name,
                    ch.expires_at
               FROM ax_im_bind_challenges ch
               JOIN ax_im_connectors c
                 ON c.id = ch.connector_id AND c.tenant_id = ch.tenant_id
              WHERE ch.token_hash = $1
                AND ch.consumed_at IS NULL
                AND ch.expires_at > NOW()",
        )
        .bind(token_hash)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("preview ax_im_bind_challenges: {err}")))?;

        let preview = match row_opt {
            Some(row) => {
                let connector_id: Uuid = row
                    .try_get("connector_id")
                    .map_err(|e| AppError::Database(format!("decode connector_id: {e}")))?;
                let connector_name: String = row
                    .try_get("connector_name")
                    .map_err(|e| AppError::Database(format!("decode connector_name: {e}")))?;
                let raw_platform: String = row
                    .try_get("platform")
                    .map_err(|e| AppError::Database(format!("decode platform: {e}")))?;
                let platform: ImPlatform = raw_platform.parse()?;
                let external_user_id: String = row
                    .try_get("external_user_id")
                    .map_err(|e| AppError::Database(format!("decode external_user_id: {e}")))?;
                let external_display_name: String =
                    row.try_get("external_display_name").map_err(|e| {
                        AppError::Database(format!("decode external_display_name: {e}"))
                    })?;
                let expires_at: DateTime<Utc> = row
                    .try_get("expires_at")
                    .map_err(|e| AppError::Database(format!("decode expires_at: {e}")))?;
                Some(ImBindChallengePreview {
                    connector_id,
                    connector_name,
                    platform,
                    external_user_id,
                    external_display_name,
                    expires_at,
                })
            }
            None => None,
        };
        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit preview_im_bind_challenge tx: {err}"))
        })?;
        Ok(preview)
    }

    async fn consume_im_bind_challenge(
        &self,
        tenant_id: &str,
        token_hash: &str,
        user_id: Uuid,
    ) -> Result<Option<ConsumedImBindChallenge>, AppError> {
        let mut tx = begin_tenant_tx(&self.pool, tenant_id).await?;
        let row_opt = sqlx::query(
            "SELECT connector_id, platform, external_user_id, external_display_name
               FROM ax_consume_im_bind_challenge($1, $2, $3)",
        )
        .bind(token_hash)
        .bind(tenant_id)
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("ax_consume_im_bind_challenge failed: {err}")))?;

        let consumed = match row_opt {
            Some(row) => {
                let connector_id: Uuid = row
                    .try_get("connector_id")
                    .map_err(|e| AppError::Database(format!("decode connector_id: {e}")))?;
                let raw_platform: String = row
                    .try_get("platform")
                    .map_err(|e| AppError::Database(format!("decode platform: {e}")))?;
                let platform: ImPlatform = raw_platform.parse()?;
                let external_user_id: String = row
                    .try_get("external_user_id")
                    .map_err(|e| AppError::Database(format!("decode external_user_id: {e}")))?;
                let external_display_name: String =
                    row.try_get("external_display_name").map_err(|e| {
                        AppError::Database(format!("decode external_display_name: {e}"))
                    })?;
                Some(ConsumedImBindChallenge {
                    connector_id,
                    platform,
                    external_user_id,
                    external_display_name,
                })
            }
            None => None,
        };
        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit consume_im_bind_challenge tx: {err}"))
        })?;
        Ok(consumed)
    }

    async fn find_active_im_conversation(
        &self,
        tenant_id: &str,
        connector_id: Uuid,
        external_chat_id: &str,
        user_id: Option<Uuid>,
    ) -> Result<Option<ConversationRecord>, AppError> {
        let mut tx = begin_tenant_tx(&self.pool, tenant_id).await?;
        let row_opt = sqlx::query(
            "SELECT *
               FROM ax_conversations
              WHERE channel = 'IM'
                AND im_connector_id = $1
                AND external_chat_id = $2
                AND status = 'ACTIVE'
                AND ($3::uuid IS NULL OR user_id = $3)
              ORDER BY updated_at DESC, id DESC
              LIMIT 1",
        )
        .bind(connector_id)
        .bind(external_chat_id)
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("find active IM conversation failed: {err}")))?;

        let rec = match row_opt {
            Some(ref r) => Some(decode_im_conversation_row(r)?),
            None => None,
        };
        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit find_active_im_conversation tx: {err}"))
        })?;
        Ok(rec)
    }
}

impl PrincipalHydrationPort for PgStore {
    async fn hydrate_im_principal(
        &self,
        tenant_id: &str,
        user_id: Uuid,
        policy: ImIdentityPolicy,
    ) -> Result<Option<HydratedImPrincipal>, AppError> {
        let mut tx = begin_tenant_tx(&self.pool, tenant_id).await?;
        let user_opt = sqlx::query(
            "SELECT id, tenant_id, username, display_name, dept_id, clearance, tokens_valid_after
               FROM sec_users
              WHERE id = $1
                AND status = 'ACTIVE'
                AND (locked_until IS NULL OR locked_until <= NOW())",
        )
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("load sec_users for IM failed: {err}")))?;

        let Some(user_row) = user_opt else {
            tx.commit().await.map_err(|err| {
                AppError::Database(format!("commit hydrate_im_principal empty tx: {err}"))
            })?;
            return Ok(None);
        };

        let (roles, permissions) = load_roles_and_permissions_tx(&mut tx, user_id).await?;
        let attributes = load_user_attributes_tx(&mut tx, user_id).await?;

        let resolved_tenant: String = user_row
            .try_get("tenant_id")
            .map_err(|err| AppError::Database(format!("decode sec_users.tenant_id: {err}")))?;
        let username: String = user_row
            .try_get("username")
            .map_err(|err| AppError::Database(format!("decode sec_users.username: {err}")))?;
        let display_name: String = user_row
            .try_get("display_name")
            .map_err(|err| AppError::Database(format!("decode sec_users.display_name: {err}")))?;
        let dept_id: String = user_row
            .try_get("dept_id")
            .map_err(|err| AppError::Database(format!("decode sec_users.dept_id: {err}")))?;
        let raw_clearance: String = user_row
            .try_get("clearance")
            .map_err(|err| AppError::Database(format!("decode sec_users.clearance: {err}")))?;
        let clearance: Sensitivity = raw_clearance.parse()?;
        let tokens_valid_after: DateTime<Utc> =
            user_row.try_get("tokens_valid_after").map_err(|err| {
                AppError::Database(format!("decode sec_users.tokens_valid_after: {err}"))
            })?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit hydrate_im_principal tx: {err}"))
        })?;

        let mut principal = PrincipalRecord {
            user_id,
            tenant_id: resolved_tenant,
            username,
            display_name,
            dept_id,
            roles,
            permissions,
            clearance,
            tokens_valid_after,
        };
        apply_im_identity_guardrails(&mut principal, policy);

        Ok(Some(HydratedImPrincipal {
            principal,
            attributes,
        }))
    }
}
