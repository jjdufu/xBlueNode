//! PostgreSQL (`PgStore`) implementations of [`ConnectionRepository`] and [`DatasetRepository`] for Step 02 (`AxHub`).

use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;

use axpoint_domain::AppError;
use axpoint_domain::axhub::{
    CatalogSearchHit, ColumnMeta, ColumnProfile, Connection, ConnectionStatus,
    CreateDatasetRequest, CreateGlossaryTermRequest, CreateQualityRuleRequest,
    CreateWebhookRequest, Dataset, DatasetListQuery, DatasetStatus, GlossaryTerm, LineageEdge,
    NewConnectionInput, NewUploadInput, PaginatedList, QualityOnFail, QualityReport, QualityRule,
    ResolvedWebhook, ScheduledDataset, SchemaChange, SchemaPolicy, SealedCredential,
    SidecarSyncResult, SyncDatasetResponse, SyncJob, UpdateColumnMetaRequest,
    UpdateConnectionInput, UpdateDatasetRequest, UploadRecord, UploadStatus, WebhookAuthScheme,
    WebhookBufferItem, WebhookConfig,
};
use axpoint_domain::enums::{
    AuditAction, ConnectorType, DataType, JobStatus, OutboxTopic, PiiCategory, QualityRuleKind,
    SourceKind, SyncMode,
};
use axpoint_domain::platform::NewOutboxEvent;
use axpoint_domain::ports::{ConnectionRepository, DatasetRepository};
use chrono::{DateTime, Utc};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

use crate::PgStore;
use crate::outbox::insert_outbox_tx;
use crate::tenant::begin_tenant_tx;

const INTERNAL_SECRET_FIELDS_KEY: &str = "__secret_fields";

fn is_unique_violation(err: &sqlx::Error) -> bool {
    err.as_database_error()
        .and_then(sqlx::error::DatabaseError::code)
        .is_some_and(|code| code == "23505")
}

fn is_valid_dataset_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    let Some(&first) = bytes.first() else {
        return false;
    };
    if bytes.len() > 63 || !first.is_ascii_lowercase() {
        return false;
    }
    bytes
        .iter()
        .all(|&b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

fn is_valid_webhook_slug(slug: &str) -> bool {
    let bytes = slug.as_bytes();
    if !(3..=64).contains(&bytes.len()) {
        return false;
    }
    bytes
        .iter()
        .all(|&b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn parse_connection_status(raw: &str) -> Result<ConnectionStatus, AppError> {
    match raw {
        "UNTESTED" => Ok(ConnectionStatus::Untested),
        "OK" => Ok(ConnectionStatus::Ok),
        "FAILED" => Ok(ConnectionStatus::Failed),
        _ => Err(AppError::bad(
            "INVALID_ENUM",
            format!("unknown ConnectionStatus: {raw}"),
        )),
    }
}

fn parse_schema_policy(raw: &str) -> Result<SchemaPolicy, AppError> {
    match raw {
        "STRICT" => Ok(SchemaPolicy::Strict),
        "ADDITIVE" => Ok(SchemaPolicy::Additive),
        "EVOLVE" => Ok(SchemaPolicy::Evolve),
        _ => Err(AppError::bad(
            "INVALID_ENUM",
            format!("unknown SchemaPolicy: {raw}"),
        )),
    }
}

fn parse_dataset_status(raw: &str) -> Result<DatasetStatus, AppError> {
    match raw {
        "DRAFT" => Ok(DatasetStatus::Draft),
        "READY" => Ok(DatasetStatus::Ready),
        "SYNCING" => Ok(DatasetStatus::Syncing),
        "FAILED" => Ok(DatasetStatus::Failed),
        "DISABLED" => Ok(DatasetStatus::Disabled),
        _ => Err(AppError::bad(
            "INVALID_ENUM",
            format!("unknown DatasetStatus: {raw}"),
        )),
    }
}

fn parse_upload_status(raw: &str) -> Result<UploadStatus, AppError> {
    match raw {
        "PENDING" => Ok(UploadStatus::Pending),
        "PARSED" => Ok(UploadStatus::Parsed),
        "COMMITTED" => Ok(UploadStatus::Committed),
        "FAILED" => Ok(UploadStatus::Failed),
        _ => Err(AppError::bad(
            "INVALID_ENUM",
            format!("unknown UploadStatus: {raw}"),
        )),
    }
}

fn parse_webhook_auth_scheme(raw: &str) -> Result<WebhookAuthScheme, AppError> {
    match raw {
        "HMAC_SHA256" => Ok(WebhookAuthScheme::HmacSha256),
        "BEARER" => Ok(WebhookAuthScheme::Bearer),
        _ => Err(AppError::bad(
            "INVALID_ENUM",
            format!("unknown WebhookAuthScheme: {raw}"),
        )),
    }
}

fn parse_quality_on_fail(raw: &str) -> Result<QualityOnFail, AppError> {
    match raw {
        "WARN" => Ok(QualityOnFail::Warn),
        "BLOCK" => Ok(QualityOnFail::Block),
        _ => Err(AppError::bad(
            "INVALID_ENUM",
            format!("unknown QualityOnFail: {raw}"),
        )),
    }
}

fn extract_config_and_secret_fields(
    raw: serde_json::Value,
) -> (serde_json::Map<String, serde_json::Value>, Vec<String>) {
    match raw {
        serde_json::Value::Object(mut map) => {
            let secret_fields = match map.remove(INTERNAL_SECRET_FIELDS_KEY) {
                Some(serde_json::Value::Array(items)) => items
                    .into_iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect(),
                _ => Vec::new(),
            };
            (map, secret_fields)
        }
        _ => (serde_json::Map::new(), Vec::new()),
    }
}

fn decode_connection_with_credential(
    row: &sqlx::postgres::PgRow,
) -> Result<(Connection, Option<SealedCredential>), AppError> {
    let connector_type = ConnectorType::from_str(&row.get::<String, _>("connector_type"))?;
    let status = parse_connection_status(&row.get::<String, _>("status"))?;

    let raw_cfg: serde_json::Value = row.get("config");
    let (config, secret_fields_set) = extract_config_and_secret_fields(raw_cfg);

    let cred_id: Option<Uuid> = row.get("credential_id");
    let sealed = match cred_id {
        Some(cid) => {
            let kv: Option<i32> = row.try_get("cred_key_version").ok().flatten();
            let nonce: Option<Vec<u8>> = row.try_get("cred_nonce").ok().flatten();
            let ciphertext: Option<Vec<u8>> = row.try_get("cred_ciphertext").ok().flatten();
            match (kv, nonce, ciphertext) {
                (Some(key_version), Some(n), Some(ct)) => Some(SealedCredential {
                    id: cid,
                    key_version,
                    nonce: n,
                    ciphertext: ct,
                }),
                _ => None,
            }
        }
        None => None,
    };

    let conn = Connection {
        id: row.get("id"),
        name: row.get("name"),
        connector_type,
        config,
        secret_fields_set,
        allow_private_network: row.get("allow_private_network"),
        status,
        last_test_message: row.get("last_test_message"),
        last_tested_at: row.get("last_tested_at"),
        created_by: row.get("created_by"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    };

    Ok((conn, sealed))
}

fn decode_sealed_credential(row: &sqlx::postgres::PgRow) -> SealedCredential {
    SealedCredential {
        id: row.get("id"),
        key_version: row.get("key_version"),
        nonce: row.get("nonce"),
        ciphertext: row.get("ciphertext"),
    }
}

fn decode_dataset(row: &sqlx::postgres::PgRow) -> Result<Dataset, AppError> {
    let source_kind = SourceKind::from_str(&row.get::<String, _>("source_kind"))?;
    let sync_mode = SyncMode::from_str(&row.get::<String, _>("sync_mode"))?;
    let schema_policy = parse_schema_policy(&row.get::<String, _>("schema_policy"))?;
    let status = parse_dataset_status(&row.get::<String, _>("status"))?;
    let schema_val: serde_json::Value = row.get("schema_json");
    let schema_json: Vec<ColumnProfile> = serde_json::from_value(schema_val).unwrap_or_default();

    Ok(Dataset {
        id: row.get("id"),
        connection_id: row.get("connection_id"),
        name: row.get("name"),
        display_name: row.get("display_name"),
        description: row.get("description"),
        source_kind,
        source_ref: row.get("source_ref"),
        sync_mode,
        primary_key: row.get("primary_key"),
        cursor_column: row.get("cursor_column"),
        cursor_value: row.get("cursor_value"),
        delete_column: row.get("delete_column"),
        schema_policy,
        lake_schema: row.get("lake_schema"),
        lake_table: row.get("lake_table"),
        schema_json,
        schema_version: row.get("schema_version"),
        row_count: row.get("row_count"),
        last_snapshot_id: row.get("last_snapshot_id"),
        last_synced_at: row.get("last_synced_at"),
        schedule_cron: row.get("schedule_cron"),
        tags: row.get("tags"),
        owner_user_id: row.get("owner_user_id"),
        status,
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

fn decode_sync_job(row: &sqlx::postgres::PgRow) -> Result<SyncJob, AppError> {
    let sync_mode = SyncMode::from_str(&row.get::<String, _>("sync_mode"))?;
    let status = JobStatus::from_str(&row.get::<String, _>("status"))?;
    let sc_val: serde_json::Value = row.get("schema_change");
    let qr_val: serde_json::Value = row.get("quality_result");
    let schema_change: SchemaChange = serde_json::from_value(sc_val).unwrap_or_default();
    let quality_result: QualityReport = serde_json::from_value(qr_val).unwrap_or_default();

    Ok(SyncJob {
        id: row.get("id"),
        dataset_id: row.get("dataset_id"),
        job_id: row.get("job_id"),
        batch_id: row.get("batch_id"),
        sync_mode,
        status,
        cursor_from: row.get("cursor_from"),
        cursor_to: row.get("cursor_to"),
        rows_read: row.get("rows_read"),
        rows_written: row.get("rows_written"),
        rows_deleted: row.get("rows_deleted"),
        snapshot_id: row.get("snapshot_id"),
        schema_change,
        quality_result,
        error_code: row.get("error_code"),
        error_message: row.get("error_message"),
        started_at: row.get("started_at"),
        finished_at: row.get("finished_at"),
        created_at: row.get("created_at"),
    })
}

fn decode_column_meta(row: &sqlx::postgres::PgRow) -> Result<ColumnMeta, AppError> {
    let data_type = DataType::from_str(&row.get::<String, _>("data_type"))?;
    let pii_category = PiiCategory::from_str(&row.get::<String, _>("pii_category"))?;

    Ok(ColumnMeta {
        dataset_id: row.get("dataset_id"),
        column_name: row.get("column_name"),
        data_type,
        pii_category,
        pii_confirmed: row.get("pii_confirmed"),
        description: row.get("description"),
        business_term: row.get("business_term"),
        unit: row.get("unit"),
        ordinal: row.get("ordinal"),
    })
}

fn decode_quality_rule(row: &sqlx::postgres::PgRow) -> Result<QualityRule, AppError> {
    let rule_kind = QualityRuleKind::from_str(&row.get::<String, _>("rule_kind"))?;
    let on_fail = parse_quality_on_fail(&row.get::<String, _>("on_fail"))?;

    Ok(QualityRule {
        id: row.get("id"),
        dataset_id: row.get("dataset_id"),
        rule_kind,
        column_name: row.get("column_name"),
        params: row.get("params"),
        on_fail,
        created_at: row.get("created_at"),
    })
}

fn decode_upload_record(row: &sqlx::postgres::PgRow) -> Result<UploadRecord, AppError> {
    let status = parse_upload_status(&row.get::<String, _>("status"))?;

    Ok(UploadRecord {
        id: row.get("id"),
        filename: row.get("filename"),
        sha256: row.get("sha256"),
        size_bytes: row.get("size_bytes"),
        parse_id: row.get("parse_id"),
        doc_id: row.get("doc_id"),
        dataset_ids: row.get("dataset_ids"),
        status,
        error_code: row.get("error_code"),
        created_by: row.get("created_by"),
        created_at: row.get("created_at"),
    })
}

fn decode_webhook_config(row: &sqlx::postgres::PgRow) -> Result<WebhookConfig, AppError> {
    let auth_scheme = parse_webhook_auth_scheme(&row.get::<String, _>("auth_scheme"))?;

    Ok(WebhookConfig {
        id: row.get("id"),
        dataset_id: row.get("dataset_id"),
        slug: row.get("slug"),
        auth_scheme,
        record_path: row.get("record_path"),
        max_body_bytes: row.get("max_body_bytes"),
        enabled: row.get("enabled"),
        created_at: row.get("created_at"),
    })
}

fn parse_ts_uuid_cursor(raw: &str) -> Result<(DateTime<Utc>, Uuid), AppError> {
    let (ts_part, id_part) = raw
        .split_once('|')
        .ok_or_else(|| AppError::bad("INVALID_ARGUMENT", "invalid pagination cursor format"))?;
    let ts = DateTime::parse_from_rfc3339(ts_part)
        .map_err(|err| AppError::bad("INVALID_ARGUMENT", format!("invalid cursor ts: {err}")))?
        .with_timezone(&Utc);
    let id = Uuid::parse_str(id_part)
        .map_err(|err| AppError::bad("INVALID_ARGUMENT", format!("invalid cursor id: {err}")))?;
    Ok((ts, id))
}

async fn upsert_credential_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    cred: &SealedCredential,
) -> Result<(), AppError> {
    sqlx::query(
        "INSERT INTO ds_credentials (id, tenant_id, key_version, nonce, ciphertext)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (id) DO UPDATE SET
            key_version = EXCLUDED.key_version,
            nonce = EXCLUDED.nonce,
            ciphertext = EXCLUDED.ciphertext,
            rotated_at = NOW()",
    )
    .bind(cred.id)
    .bind(tenant_id)
    .bind(cred.key_version)
    .bind(&cred.nonce)
    .bind(&cred.ciphertext)
    .execute(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("upsert ds_credentials failed: {err}")))?;
    Ok(())
}

async fn delete_orphan_credential_tx(
    tx: &mut Transaction<'_, Postgres>,
    cred_id: Uuid,
) -> Result<(), AppError> {
    sqlx::query(
        "DELETE FROM ds_credentials c
          WHERE c.id = $1
            AND NOT EXISTS (SELECT 1 FROM ds_connections conn WHERE conn.credential_id = c.id)
            AND NOT EXISTS (SELECT 1 FROM ds_webhooks wh WHERE wh.credential_id = c.id)",
    )
    .bind(cred_id)
    .execute(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("delete orphan ds_credentials failed: {err}")))?;
    Ok(())
}

async fn insert_lineage_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    src_kind: &str,
    src_id: &str,
    dst_kind: &str,
    dst_id: &str,
    via: &str,
) -> Result<(), AppError> {
    sqlx::query(
        "INSERT INTO ds_lineage (tenant_id, src_kind, src_id, dst_kind, dst_id, via)
         VALUES ($1, $2, $3, $4, $5, $6)
         ON CONFLICT (tenant_id, src_kind, src_id, dst_kind, dst_id)
         DO UPDATE SET via = EXCLUDED.via",
    )
    .bind(tenant_id)
    .bind(src_kind)
    .bind(src_id)
    .bind(dst_kind)
    .bind(dst_id)
    .bind(via)
    .execute(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("insert ds_lineage failed: {err}")))?;
    Ok(())
}

async fn insert_system_audit_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    action: AuditAction,
    resource_ref: &str,
    detail: serde_json::Value,
) -> Result<(), AppError> {
    sqlx::query(
        "INSERT INTO sec_audit_logs (
             tenant_id, user_id, username, action, resource_ref, status_code, detail_json
         )
         VALUES ($1, NULL, 'system', $2, $3, 200, $4)",
    )
    .bind(tenant_id)
    .bind(action.as_str())
    .bind(resource_ref)
    .bind(detail)
    .execute(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("insert sec_audit_logs failed: {err}")))?;
    Ok(())
}

async fn fetch_connection_by_id_tx(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
) -> Result<Option<sqlx::postgres::PgRow>, AppError> {
    sqlx::query(
        "SELECT conn.id, conn.name, conn.connector_type, conn.config, conn.credential_id,
                conn.allow_private_network, conn.status, conn.last_test_message,
                conn.last_tested_at, conn.created_by, conn.created_at, conn.updated_at,
                cred.key_version AS cred_key_version,
                cred.nonce AS cred_nonce,
                cred.ciphertext AS cred_ciphertext
           FROM ds_connections conn
           LEFT JOIN ds_credentials cred ON cred.id = conn.credential_id
          WHERE conn.id = $1",
    )
    .bind(id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("get ds_connections failed: {err}")))
}

impl ConnectionRepository for PgStore {
    async fn list_connections(
        &self,
        tenant_id: &str,
    ) -> Result<Vec<(Connection, Option<SealedCredential>)>, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let rows = sqlx::query(
            "SELECT conn.id, conn.name, conn.connector_type, conn.config, conn.credential_id,
                    conn.allow_private_network, conn.status, conn.last_test_message,
                    conn.last_tested_at, conn.created_by, conn.created_at, conn.updated_at,
                    cred.key_version AS cred_key_version,
                    cred.nonce AS cred_nonce,
                    cred.ciphertext AS cred_ciphertext
               FROM ds_connections conn
               LEFT JOIN ds_credentials cred ON cred.id = conn.credential_id
              ORDER BY conn.created_at DESC, conn.id DESC",
        )
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("list ds_connections failed: {err}")))?;
        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit list_connections tx failed: {err}"))
        })?;

        rows.iter().map(decode_connection_with_credential).collect()
    }

    async fn get_connection(
        &self,
        tenant_id: &str,
        id: Uuid,
    ) -> Result<Option<(Connection, Option<SealedCredential>)>, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let maybe_row = fetch_connection_by_id_tx(&mut tx, id).await?;
        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit get_connection tx failed: {err}")))?;

        maybe_row
            .as_ref()
            .map(decode_connection_with_credential)
            .transpose()
    }

    async fn create_connection(
        &self,
        tenant_id: &str,
        input: NewConnectionInput,
    ) -> Result<Connection, AppError> {
        let trimmed_name = input.name.trim();
        if trimmed_name.is_empty() {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "connection name cannot be empty",
            ));
        }

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let credential_id = if let Some(ref cred) = input.credential {
            upsert_credential_tx(&mut tx, tenant_id, cred).await?;
            Some(cred.id)
        } else {
            None
        };

        let config_json = if input.config.is_null() {
            serde_json::json!({})
        } else {
            input.config
        };

        let insert_res = sqlx::query(
            "INSERT INTO ds_connections (
                 tenant_id, name, connector_type, config, credential_id,
                 allow_private_network, created_by
             )
             VALUES ($1, $2, $3, $4, $5, $6, $7)
             RETURNING id",
        )
        .bind(tenant_id)
        .bind(trimmed_name)
        .bind(input.connector_type.as_str())
        .bind(&config_json)
        .bind(credential_id)
        .bind(input.allow_private_network)
        .bind(input.created_by)
        .fetch_one(&mut *tx)
        .await;

        let inserted = match insert_res {
            Ok(row) => row,
            Err(err) if is_unique_violation(&err) => {
                return Err(AppError::Conflict("NAME_TAKEN"));
            }
            Err(err) => {
                return Err(AppError::Database(format!(
                    "insert ds_connections failed: {err}"
                )));
            }
        };

        let conn_id: Uuid = inserted.get("id");
        let row = fetch_connection_by_id_tx(&mut tx, conn_id)
            .await?
            .ok_or_else(|| AppError::Database(String::from("inserted connection missing")))?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit create_connection tx failed: {err}"))
        })?;

        let (conn, _) = decode_connection_with_credential(&row)?;
        Ok(conn)
    }

    async fn update_connection(
        &self,
        tenant_id: &str,
        id: Uuid,
        input: UpdateConnectionInput,
    ) -> Result<Option<Connection>, AppError> {
        if let Some(ref name) = input.name
            && name.trim().is_empty()
        {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "connection name cannot be empty",
            ));
        }

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let existing =
            sqlx::query("SELECT credential_id FROM ds_connections WHERE id = $1 FOR UPDATE")
                .bind(id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(|err| {
                    AppError::Database(format!("select ds_connections for update: {err}"))
                })?;

        let Some(existing_row) = existing else {
            tx.commit().await.map_err(|err| {
                AppError::Database(format!("commit empty update_connection tx failed: {err}"))
            })?;
            return Ok(None);
        };

        let old_cred_id: Option<Uuid> = existing_row.get("credential_id");
        let new_cred_id = match input.credential {
            None => old_cred_id,
            Some(None) => None,
            Some(Some(ref cred)) => {
                upsert_credential_tx(&mut tx, tenant_id, cred).await?;
                Some(cred.id)
            }
        };

        let trimmed_name = input.name.as_deref().map(str::trim);
        let update_res = sqlx::query(
            "UPDATE ds_connections
                SET name = COALESCE($2, name),
                    config = COALESCE($3, config),
                    allow_private_network = COALESCE($4, allow_private_network),
                    credential_id = $5,
                    updated_at = NOW()
              WHERE id = $1",
        )
        .bind(id)
        .bind(trimmed_name)
        .bind(input.config)
        .bind(input.allow_private_network)
        .bind(new_cred_id)
        .execute(&mut *tx)
        .await;

        match update_res {
            Ok(_) => {}
            Err(err) if is_unique_violation(&err) => {
                return Err(AppError::Conflict("NAME_TAKEN"));
            }
            Err(err) => {
                return Err(AppError::Database(format!(
                    "update ds_connections failed: {err}"
                )));
            }
        }

        if let Some(old_id) = old_cred_id
            && Some(old_id) != new_cred_id
        {
            delete_orphan_credential_tx(&mut tx, old_id).await?;
        }

        let row = fetch_connection_by_id_tx(&mut tx, id)
            .await?
            .ok_or_else(|| AppError::Database(String::from("updated connection missing")))?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit update_connection tx failed: {err}"))
        })?;

        let (conn, _) = decode_connection_with_credential(&row)?;
        Ok(Some(conn))
    }

    async fn delete_connection(&self, tenant_id: &str, id: Uuid) -> Result<bool, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let in_use: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM ds_datasets WHERE connection_id = $1)")
                .bind(id)
                .fetch_one(&mut *tx)
                .await
                .map_err(|err| {
                    AppError::Database(format!("check ds_datasets dependency failed: {err}"))
                })?;

        if in_use {
            return Err(AppError::Conflict("CONNECTION_IN_USE"));
        }

        let deleted =
            sqlx::query("DELETE FROM ds_connections WHERE id = $1 RETURNING credential_id")
                .bind(id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(|err| {
                    AppError::Database(format!("delete ds_connections failed: {err}"))
                })?;

        let Some(row) = deleted else {
            tx.commit().await.map_err(|err| {
                AppError::Database(format!("commit noop delete_connection tx failed: {err}"))
            })?;
            return Ok(false);
        };

        let cred_id: Option<Uuid> = row.get("credential_id");
        if let Some(cid) = cred_id {
            delete_orphan_credential_tx(&mut tx, cid).await?;
        }

        sqlx::query("DELETE FROM ds_lineage WHERE src_kind = 'CONNECTION' AND src_id = $1")
            .bind(id.to_string())
            .execute(&mut *tx)
            .await
            .map_err(|err| {
                AppError::Database(format!("cleanup connection lineage failed: {err}"))
            })?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit delete_connection tx failed: {err}"))
        })?;
        Ok(true)
    }

    async fn update_test_status(
        &self,
        tenant_id: &str,
        id: Uuid,
        status: ConnectionStatus,
        message: &str,
    ) -> Result<(), AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let res = sqlx::query(
            "UPDATE ds_connections
                SET status = $2,
                    last_test_message = $3,
                    last_tested_at = NOW(),
                    updated_at = NOW()
              WHERE id = $1",
        )
        .bind(id)
        .bind(status.as_str())
        .bind(message)
        .execute(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("update_test_status failed: {err}")))?;

        if res.rows_affected() == 0 {
            return Err(AppError::NotFound("CONNECTION_NOT_FOUND"));
        }

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit update_test_status tx failed: {err}"))
        })?;
        Ok(())
    }

    async fn get_credential(
        &self,
        tenant_id: &str,
        credential_id: Uuid,
    ) -> Result<Option<SealedCredential>, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let maybe_row = sqlx::query(
            "SELECT id, key_version, nonce, ciphertext FROM ds_credentials WHERE id = $1",
        )
        .bind(credential_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("get ds_credentials failed: {err}")))?;
        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit get_credential tx failed: {err}")))?;

        Ok(maybe_row.as_ref().map(decode_sealed_credential))
    }

    async fn list_credentials(&self, tenant_id: &str) -> Result<Vec<SealedCredential>, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let rows = sqlx::query(
            "SELECT id, key_version, nonce, ciphertext FROM ds_credentials ORDER BY created_at ASC",
        )
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("list ds_credentials failed: {err}")))?;
        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit list_credentials tx failed: {err}"))
        })?;

        Ok(rows.iter().map(decode_sealed_credential).collect())
    }

    async fn update_rotated_credential(
        &self,
        tenant_id: &str,
        credential: SealedCredential,
    ) -> Result<(), AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let res = sqlx::query(
            "UPDATE ds_credentials
                SET key_version = $2,
                    nonce = $3,
                    ciphertext = $4,
                    rotated_at = NOW()
              WHERE id = $1",
        )
        .bind(credential.id)
        .bind(credential.key_version)
        .bind(&credential.nonce)
        .bind(&credential.ciphertext)
        .execute(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("update_rotated_credential failed: {err}")))?;

        if res.rows_affected() == 0 {
            return Err(AppError::NotFound("CREDENTIAL_NOT_FOUND"));
        }

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit update_rotated_credential tx failed: {err}"))
        })?;
        Ok(())
    }
}

impl DatasetRepository for PgStore {
    async fn list_datasets(
        &self,
        tenant_id: &str,
        query: DatasetListQuery,
    ) -> Result<PaginatedList<Dataset>, AppError> {
        let limit = query.limit.unwrap_or(50);
        if !(1..=500).contains(&limit) {
            return Err(AppError::bad(
                "LIMIT_OUT_OF_RANGE",
                format!("limit must be in 1..=500, got {limit}"),
            ));
        }

        let limit_usize = usize::try_from(limit).unwrap_or(50);
        let fetch_limit = i64::from(limit).saturating_add(1);
        let (cursor_ts, cursor_id) = match query.cursor.as_deref().filter(|s| !s.trim().is_empty())
        {
            Some(c) => {
                let (ts, id) = parse_ts_uuid_cursor(c.trim())?;
                (Some(ts), Some(id))
            }
            None => (None, None),
        };

        let q_clean = query
            .q
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from);
        let like_pattern = q_clean.as_ref().map(|q| format!("%{q}%"));
        let tag_filter = query
            .tag
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from);
        let sk_filter = query.source_kind.map(SourceKind::as_str);

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let rows = sqlx::query(
            "SELECT id, connection_id, name, display_name, description, source_kind, source_ref,
                    sync_mode, primary_key, cursor_column, cursor_value, delete_column, schema_policy,
                    lake_schema, lake_table, schema_json, schema_version, row_count, last_snapshot_id,
                    last_synced_at, schedule_cron, tags, owner_user_id, status, created_at, updated_at
               FROM ds_datasets
              WHERE ($1::text IS NULL OR fts @@ plainto_tsquery('simple', $1)
                     OR name ILIKE $2 OR display_name ILIKE $2 OR description ILIKE $2)
                AND ($3::text IS NULL OR $3 = ANY(tags))
                AND ($4::text IS NULL OR source_kind = $4)
                AND ($5::timestamptz IS NULL OR (created_at, id) < ($5, $6::uuid))
              ORDER BY created_at DESC, id DESC
              LIMIT $7",
        )
        .bind(q_clean)
        .bind(like_pattern)
        .bind(tag_filter)
        .bind(sk_filter)
        .bind(cursor_ts)
        .bind(cursor_id)
        .bind(fetch_limit)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("list ds_datasets failed: {err}")))?;

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit list_datasets tx failed: {err}")))?;

        let mut items: Vec<Dataset> = rows.iter().map(decode_dataset).collect::<Result<_, _>>()?;
        let next_cursor = if items.len() > limit_usize {
            items.truncate(limit_usize);
            items
                .last()
                .map(|d| format!("{}|{}", d.created_at.to_rfc3339(), d.id))
        } else {
            None
        };

        Ok(PaginatedList { items, next_cursor })
    }

    async fn get_dataset(&self, tenant_id: &str, id: Uuid) -> Result<Option<Dataset>, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let maybe_row = sqlx::query(
            "SELECT id, connection_id, name, display_name, description, source_kind, source_ref,
                    sync_mode, primary_key, cursor_column, cursor_value, delete_column, schema_policy,
                    lake_schema, lake_table, schema_json, schema_version, row_count, last_snapshot_id,
                    last_synced_at, schedule_cron, tags, owner_user_id, status, created_at, updated_at
               FROM ds_datasets
              WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("get ds_datasets failed: {err}")))?;
        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit get_dataset tx failed: {err}")))?;

        maybe_row.as_ref().map(decode_dataset).transpose()
    }

    async fn get_dataset_by_name(
        &self,
        tenant_id: &str,
        name: &str,
    ) -> Result<Option<Dataset>, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let maybe_row = sqlx::query(
            "SELECT id, connection_id, name, display_name, description, source_kind, source_ref,
                    sync_mode, primary_key, cursor_column, cursor_value, delete_column, schema_policy,
                    lake_schema, lake_table, schema_json, schema_version, row_count, last_snapshot_id,
                    last_synced_at, schedule_cron, tags, owner_user_id, status, created_at, updated_at
               FROM ds_datasets
              WHERE name = $1",
        )
        .bind(name.trim())
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("get_dataset_by_name failed: {err}")))?;
        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit get_dataset_by_name tx failed: {err}"))
        })?;

        maybe_row.as_ref().map(decode_dataset).transpose()
    }

    async fn create_dataset(
        &self,
        tenant_id: &str,
        owner_user_id: Option<Uuid>,
        req: CreateDatasetRequest,
    ) -> Result<Dataset, AppError> {
        create_dataset_impl(self, tenant_id, owner_user_id, req).await
    }

    async fn update_dataset(
        &self,
        tenant_id: &str,
        id: Uuid,
        req: UpdateDatasetRequest,
    ) -> Result<Option<Dataset>, AppError> {
        update_dataset_impl(self, tenant_id, id, req).await
    }

    async fn delete_dataset(&self, tenant_id: &str, id: Uuid) -> Result<Option<Dataset>, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let maybe_row = sqlx::query(
            "DELETE FROM ds_datasets
              WHERE id = $1
              RETURNING id, connection_id, name, display_name, description, source_kind, source_ref,
                        sync_mode, primary_key, cursor_column, cursor_value, delete_column, schema_policy,
                        lake_schema, lake_table, schema_json, schema_version, row_count, last_snapshot_id,
                        last_synced_at, schedule_cron, tags, owner_user_id, status, created_at, updated_at",
        )
        .bind(id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("delete ds_datasets failed: {err}")))?;

        if maybe_row.is_some() {
            let id_str = id.to_string();
            sqlx::query(
                "DELETE FROM ds_lineage
                  WHERE (src_kind = 'DATASET' AND src_id = $1)
                     OR (dst_kind = 'DATASET' AND dst_id = $1)",
            )
            .bind(id_str)
            .execute(&mut *tx)
            .await
            .map_err(|err| AppError::Database(format!("cleanup dataset lineage failed: {err}")))?;
        }

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit delete_dataset tx failed: {err}")))?;
        maybe_row.as_ref().map(decode_dataset).transpose()
    }

    async fn enqueue_sync_job(
        &self,
        tenant_id: &str,
        dataset_id: Uuid,
    ) -> Result<SyncDatasetResponse, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM ds_datasets WHERE id = $1)")
                .bind(dataset_id)
                .fetch_one(&mut *tx)
                .await
                .map_err(|err| AppError::Database(format!("check dataset exists failed: {err}")))?;

        if !exists {
            return Err(AppError::NotFound("DATASET_NOT_FOUND"));
        }

        let ds_id_str = dataset_id.to_string();
        let payload = serde_json::json!({ "dataset_id": ds_id_str });

        let inserted = sqlx::query(
            "INSERT INTO ax_jobs (tenant_id, kind, payload, priority, max_attempts)
             VALUES ($1, 'SYNC_DATASET', $2, 100, 5)
             ON CONFLICT ((payload->>'dataset_id'))
             WHERE kind = 'SYNC_DATASET' AND status IN ('PENDING', 'RUNNING')
             DO NOTHING
             RETURNING id",
        )
        .bind(tenant_id)
        .bind(payload)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("enqueue SYNC_DATASET job failed: {err}")))?;

        let response = if let Some(row) = inserted {
            SyncDatasetResponse {
                job_id: row.get("id"),
                existing: false,
            }
        } else {
            let existing_row = sqlx::query(
                "SELECT id FROM ax_jobs
                  WHERE kind = 'SYNC_DATASET'
                    AND status IN ('PENDING', 'RUNNING')
                    AND (payload->>'dataset_id') = $1
                  ORDER BY created_at DESC
                  LIMIT 1",
            )
            .bind(&ds_id_str)
            .fetch_one(&mut *tx)
            .await
            .map_err(|err| {
                AppError::Database(format!("fetch existing SYNC_DATASET job failed: {err}"))
            })?;
            SyncDatasetResponse {
                job_id: existing_row.get("id"),
                existing: true,
            }
        };

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit enqueue_sync_job tx failed: {err}"))
        })?;
        Ok(response)
    }

    async fn start_sync_execution(
        &self,
        tenant_id: &str,
        dataset_id: Uuid,
        job_id: Uuid,
    ) -> Result<SyncJob, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let ds_row =
            sqlx::query("SELECT sync_mode, cursor_value FROM ds_datasets WHERE id = $1 FOR UPDATE")
                .bind(dataset_id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(|err| AppError::Database(format!("lock dataset for sync failed: {err}")))?
                .ok_or(AppError::NotFound("DATASET_NOT_FOUND"))?;

        let sync_mode: String = ds_row.get("sync_mode");
        let cursor_from: Option<String> = ds_row.get("cursor_value");

        sqlx::query("UPDATE ds_datasets SET status = 'SYNCING', updated_at = NOW() WHERE id = $1")
            .bind(dataset_id)
            .execute(&mut *tx)
            .await
            .map_err(|err| AppError::Database(format!("mark dataset SYNCING failed: {err}")))?;

        let existing_sj = sqlx::query(
            "SELECT id FROM ds_sync_jobs
              WHERE dataset_id = $1 AND job_id = $2
              ORDER BY created_at DESC
              LIMIT 1
              FOR UPDATE",
        )
        .bind(dataset_id)
        .bind(job_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("query existing ds_sync_jobs failed: {err}")))?;

        let sj_row = if let Some(prev) = existing_sj {
            let sj_id: Uuid = prev.get("id");
            sqlx::query(
                "UPDATE ds_sync_jobs
                    SET status = 'RUNNING',
                        sync_mode = $2,
                        cursor_from = $3,
                        error_code = NULL,
                        error_message = NULL,
                        started_at = NOW(),
                        finished_at = NULL
                  WHERE id = $1
                  RETURNING *",
            )
            .bind(sj_id)
            .bind(&sync_mode)
            .bind(&cursor_from)
            .fetch_one(&mut *tx)
            .await
            .map_err(|err| AppError::Database(format!("reuse ds_sync_jobs failed: {err}")))?
        } else {
            let batch_id = Uuid::new_v4();
            sqlx::query(
                "INSERT INTO ds_sync_jobs (
                     tenant_id, dataset_id, job_id, batch_id, sync_mode,
                     status, cursor_from, started_at
                 )
                 VALUES ($1, $2, $3, $4, $5, 'RUNNING', $6, NOW())
                 RETURNING *",
            )
            .bind(tenant_id)
            .bind(dataset_id)
            .bind(job_id)
            .bind(batch_id)
            .bind(&sync_mode)
            .bind(&cursor_from)
            .fetch_one(&mut *tx)
            .await
            .map_err(|err| AppError::Database(format!("insert ds_sync_jobs failed: {err}")))?
        };

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit start_sync_execution tx failed: {err}"))
        })?;
        decode_sync_job(&sj_row)
    }

    async fn complete_sync_execution(
        &self,
        tenant_id: &str,
        sync_job_id: Uuid,
        dataset_id: Uuid,
        result: &SidecarSyncResult,
    ) -> Result<(), AppError> {
        complete_sync_execution_impl(self, tenant_id, sync_job_id, dataset_id, result).await
    }

    async fn fail_sync_execution(
        &self,
        tenant_id: &str,
        sync_job_id: Uuid,
        dataset_id: Uuid,
        error_code: &str,
        error_message: &str,
        terminal: bool,
    ) -> Result<(), AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        sqlx::query(
            "UPDATE ds_sync_jobs
                SET status = 'FAILED',
                    error_code = $2,
                    error_message = $3,
                    finished_at = NOW()
              WHERE id = $1",
        )
        .bind(sync_job_id)
        .bind(error_code)
        .bind(error_message)
        .execute(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("mark ds_sync_jobs FAILED: {err}")))?;

        if terminal {
            sqlx::query(
                "UPDATE ds_datasets SET status = 'FAILED', updated_at = NOW() WHERE id = $1",
            )
            .bind(dataset_id)
            .execute(&mut *tx)
            .await
            .map_err(|err| AppError::Database(format!("mark dataset FAILED: {err}")))?;
        }

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit fail_sync_execution tx failed: {err}"))
        })?;
        Ok(())
    }

    async fn list_sync_jobs(
        &self,
        tenant_id: &str,
        dataset_id: Uuid,
        limit: u32,
        cursor: Option<&str>,
    ) -> Result<PaginatedList<SyncJob>, AppError> {
        if !(1..=500).contains(&limit) {
            return Err(AppError::bad(
                "LIMIT_OUT_OF_RANGE",
                format!("limit must be in 1..=500, got {limit}"),
            ));
        }

        let limit_usize = usize::try_from(limit).unwrap_or(50);
        let fetch_limit = i64::from(limit).saturating_add(1);
        let (cursor_ts, cursor_id) = match cursor.filter(|s| !s.trim().is_empty()) {
            Some(c) => {
                let (ts, id) = parse_ts_uuid_cursor(c.trim())?;
                (Some(ts), Some(id))
            }
            None => (None, None),
        };

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let rows = sqlx::query(
            "SELECT * FROM ds_sync_jobs
              WHERE dataset_id = $1
                AND ($2::timestamptz IS NULL OR (created_at, id) < ($2, $3::uuid))
              ORDER BY created_at DESC, id DESC
              LIMIT $4",
        )
        .bind(dataset_id)
        .bind(cursor_ts)
        .bind(cursor_id)
        .bind(fetch_limit)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("list ds_sync_jobs failed: {err}")))?;

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit list_sync_jobs tx failed: {err}")))?;

        let mut items: Vec<SyncJob> = rows.iter().map(decode_sync_job).collect::<Result<_, _>>()?;
        let next_cursor = if items.len() > limit_usize {
            items.truncate(limit_usize);
            items
                .last()
                .map(|j| format!("{}|{}", j.created_at.to_rfc3339(), j.id))
        } else {
            None
        };

        Ok(PaginatedList { items, next_cursor })
    }

    async fn get_stream_offsets(
        &self,
        tenant_id: &str,
        dataset_id: Uuid,
    ) -> Result<BTreeMap<String, String>, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let rows = sqlx::query(
            "SELECT partition, last_offset FROM ds_stream_offsets WHERE dataset_id = $1",
        )
        .bind(dataset_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("query ds_stream_offsets failed: {err}")))?;
        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit get_stream_offsets tx failed: {err}"))
        })?;

        let mut map = BTreeMap::new();
        for r in rows {
            map.insert(r.get("partition"), r.get("last_offset"));
        }
        Ok(map)
    }

    async fn list_columns(
        &self,
        tenant_id: &str,
        dataset_id: Uuid,
    ) -> Result<Vec<ColumnMeta>, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let rows = sqlx::query(
            "SELECT dataset_id, column_name, data_type, pii_category, pii_confirmed,
                    description, business_term, unit, ordinal
               FROM ds_column_meta
              WHERE dataset_id = $1
              ORDER BY ordinal ASC, column_name ASC",
        )
        .bind(dataset_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("list ds_column_meta failed: {err}")))?;
        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit list_columns tx failed: {err}")))?;

        rows.iter().map(decode_column_meta).collect()
    }

    async fn update_column_meta(
        &self,
        tenant_id: &str,
        dataset_id: Uuid,
        column_name: &str,
        req: UpdateColumnMetaRequest,
    ) -> Result<Option<ColumnMeta>, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let pii_str = req.pii_category.map(PiiCategory::as_str);
        let pii_confirmed = match (req.pii_confirmed, req.pii_category) {
            (Some(flag), _) => Some(flag),
            (None, Some(_)) => Some(true),
            (None, None) => None,
        };

        let maybe_row = sqlx::query(
            "UPDATE ds_column_meta
                SET description = COALESCE($3, description),
                    business_term = COALESCE($4, business_term),
                    unit = COALESCE($5, unit),
                    pii_category = COALESCE($6, pii_category),
                    pii_confirmed = COALESCE($7, pii_confirmed)
              WHERE dataset_id = $1 AND column_name = $2
              RETURNING dataset_id, column_name, data_type, pii_category, pii_confirmed,
                        description, business_term, unit, ordinal",
        )
        .bind(dataset_id)
        .bind(column_name)
        .bind(req.description)
        .bind(req.business_term)
        .bind(req.unit)
        .bind(pii_str)
        .bind(pii_confirmed)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("update ds_column_meta failed: {err}")))?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit update_column_meta tx failed: {err}"))
        })?;
        maybe_row.as_ref().map(decode_column_meta).transpose()
    }

    async fn list_quality_rules(
        &self,
        tenant_id: &str,
        dataset_id: Uuid,
    ) -> Result<Vec<QualityRule>, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let rows = sqlx::query(
            "SELECT id, dataset_id, rule_kind, column_name, params, on_fail, created_at
               FROM ds_quality_rules
              WHERE dataset_id = $1
              ORDER BY created_at ASC",
        )
        .bind(dataset_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("list ds_quality_rules failed: {err}")))?;
        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit list_quality_rules tx failed: {err}"))
        })?;

        rows.iter().map(decode_quality_rule).collect()
    }

    async fn create_quality_rule(
        &self,
        tenant_id: &str,
        dataset_id: Uuid,
        req: CreateQualityRuleRequest,
    ) -> Result<QualityRule, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM ds_datasets WHERE id = $1)")
                .bind(dataset_id)
                .fetch_one(&mut *tx)
                .await
                .map_err(|err| AppError::Database(format!("check dataset exists: {err}")))?;
        if !exists {
            return Err(AppError::NotFound("DATASET_NOT_FOUND"));
        }

        let params = if req.params.is_null() {
            serde_json::json!({})
        } else {
            req.params
        };
        let col_name = req
            .column_name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from);

        let row = sqlx::query(
            "INSERT INTO ds_quality_rules (
                 tenant_id, dataset_id, rule_kind, column_name, params, on_fail
             )
             VALUES ($1, $2, $3, $4, $5, $6)
             RETURNING id, dataset_id, rule_kind, column_name, params, on_fail, created_at",
        )
        .bind(tenant_id)
        .bind(dataset_id)
        .bind(req.rule_kind.as_str())
        .bind(col_name)
        .bind(params)
        .bind(req.on_fail.as_str())
        .fetch_one(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("insert ds_quality_rules failed: {err}")))?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit create_quality_rule tx failed: {err}"))
        })?;
        decode_quality_rule(&row)
    }

    async fn delete_quality_rule(&self, tenant_id: &str, rule_id: Uuid) -> Result<bool, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let res = sqlx::query("DELETE FROM ds_quality_rules WHERE id = $1")
            .bind(rule_id)
            .execute(&mut *tx)
            .await
            .map_err(|err| AppError::Database(format!("delete ds_quality_rules failed: {err}")))?;
        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit delete_quality_rule tx failed: {err}"))
        })?;
        Ok(res.rows_affected() > 0)
    }

    async fn find_committed_upload_by_sha(
        &self,
        tenant_id: &str,
        sha256: &str,
    ) -> Result<Option<UploadRecord>, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let maybe_row = sqlx::query(
            "SELECT id, filename, sha256, size_bytes, parse_id, doc_id, dataset_ids,
                    status, error_code, created_by, created_at
               FROM ds_uploads
              WHERE sha256 = $1 AND status = 'COMMITTED'
              ORDER BY created_at DESC
              LIMIT 1",
        )
        .bind(sha256)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("find_committed_upload_by_sha: {err}")))?;
        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit find_committed_upload_by_sha tx: {err}"))
        })?;

        maybe_row.as_ref().map(decode_upload_record).transpose()
    }

    async fn create_upload(
        &self,
        tenant_id: &str,
        input: NewUploadInput,
    ) -> Result<UploadRecord, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let row = sqlx::query(
            "INSERT INTO ds_uploads (id, tenant_id, filename, sha256, size_bytes, created_by)
             VALUES ($1, $2, $3, $4, $5, $6)
             RETURNING id, filename, sha256, size_bytes, parse_id, doc_id, dataset_ids,
                       status, error_code, created_by, created_at",
        )
        .bind(input.id)
        .bind(tenant_id)
        .bind(&input.filename)
        .bind(&input.sha256)
        .bind(input.size_bytes)
        .bind(input.created_by)
        .fetch_one(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("insert ds_uploads failed: {err}")))?;
        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit create_upload tx failed: {err}")))?;

        decode_upload_record(&row)
    }

    async fn get_upload(
        &self,
        tenant_id: &str,
        id: Uuid,
    ) -> Result<Option<UploadRecord>, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let maybe_row = sqlx::query(
            "SELECT id, filename, sha256, size_bytes, parse_id, doc_id, dataset_ids,
                    status, error_code, created_by, created_at
               FROM ds_uploads
              WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("get ds_uploads failed: {err}")))?;
        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit get_upload tx failed: {err}")))?;

        maybe_row.as_ref().map(decode_upload_record).transpose()
    }

    async fn complete_upload(
        &self,
        tenant_id: &str,
        upload_id: Uuid,
        parse_id: Uuid,
        doc_id: Option<Uuid>,
        dataset_ids: &[Uuid],
    ) -> Result<(), AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let res = sqlx::query(
            "UPDATE ds_uploads
                SET status = 'COMMITTED',
                    parse_id = $2,
                    doc_id = $3,
                    dataset_ids = $4,
                    error_code = NULL
              WHERE id = $1",
        )
        .bind(upload_id)
        .bind(parse_id)
        .bind(doc_id)
        .bind(dataset_ids)
        .execute(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("complete ds_uploads failed: {err}")))?;

        if res.rows_affected() == 0 {
            return Err(AppError::NotFound("UPLOAD_NOT_FOUND"));
        }

        let upload_str = upload_id.to_string();
        if let Some(did) = doc_id {
            sqlx::query("UPDATE hg_docs SET upload_id = $1, updated_at = NOW() WHERE id = $2")
                .bind(upload_id)
                .bind(did)
                .execute(&mut *tx)
                .await
                .map_err(|err| {
                    AppError::Database(format!("link hg_docs.upload_id failed: {err}"))
                })?;
            insert_lineage_tx(
                &mut tx,
                tenant_id,
                "UPLOAD",
                &upload_str,
                "DOC",
                &did.to_string(),
                "PARSE_FILE",
            )
            .await?;
        }

        for &ds_id in dataset_ids {
            insert_lineage_tx(
                &mut tx,
                tenant_id,
                "UPLOAD",
                &upload_str,
                "DATASET",
                &ds_id.to_string(),
                "PARSE_FILE",
            )
            .await?;
        }

        insert_system_audit_tx(
            &mut tx,
            tenant_id,
            AuditAction::FileUpload,
            &format!("upload:{upload_id}"),
            serde_json::json!({
                "parse_id": parse_id,
                "doc_id": doc_id,
                "dataset_count": dataset_ids.len(),
            }),
        )
        .await?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit complete_upload tx failed: {err}"))
        })?;
        Ok(())
    }

    async fn fail_upload(
        &self,
        tenant_id: &str,
        upload_id: Uuid,
        error_code: &str,
    ) -> Result<(), AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        sqlx::query("UPDATE ds_uploads SET status = 'FAILED', error_code = $2 WHERE id = $1")
            .bind(upload_id)
            .bind(error_code)
            .execute(&mut *tx)
            .await
            .map_err(|err| AppError::Database(format!("fail ds_uploads failed: {err}")))?;
        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit fail_upload tx failed: {err}")))?;
        Ok(())
    }

    async fn create_webhook(
        &self,
        tenant_id: &str,
        owner_user_id: Option<Uuid>,
        req: &CreateWebhookRequest,
        credential: SealedCredential,
    ) -> Result<(WebhookConfig, Dataset), AppError> {
        let ds_name = req.dataset_name.trim();
        let slug = req.slug.trim();
        if !is_valid_dataset_name(ds_name) {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                format!("dataset_name '{ds_name}' must match ^[a-z][a-z0-9_]{{0,62}}$"),
            ));
        }
        if !is_valid_webhook_slug(slug) {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                format!("webhook slug '{slug}' must match ^[a-z0-9-]{{3,64}}$"),
            ));
        }
        if req.max_body_bytes <= 0 {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "max_body_bytes must be positive",
            ));
        }

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        upsert_credential_tx(&mut tx, tenant_id, &credential).await?;

        let lake_schema = format!("tenant_{tenant_id}");
        let record_path = if req.record_path.trim().is_empty() {
            String::from("$")
        } else {
            String::from(req.record_path.trim())
        };
        let source_ref = serde_json::json!({
            "slug": slug,
            "record_path": record_path,
        });

        let ds_res = sqlx::query(
            "INSERT INTO ds_datasets (
                 tenant_id, name, display_name, description, source_kind, source_ref,
                 sync_mode, lake_schema, lake_table, schedule_cron, owner_user_id, status
             )
             VALUES ($1, $2, $3, $4, 'WEBHOOK_STREAM', $5, 'APPEND', $6, $2, '* * * * *', $7, 'READY')
             RETURNING id, connection_id, name, display_name, description, source_kind, source_ref,
                       sync_mode, primary_key, cursor_column, cursor_value, delete_column, schema_policy,
                       lake_schema, lake_table, schema_json, schema_version, row_count, last_snapshot_id,
                       last_synced_at, schedule_cron, tags, owner_user_id, status, created_at, updated_at",
        )
        .bind(tenant_id)
        .bind(ds_name)
        .bind(slug)
        .bind(format!("Inbound webhook stream ({slug})"))
        .bind(source_ref)
        .bind(&lake_schema)
        .bind(owner_user_id)
        .fetch_one(&mut *tx)
        .await;

        let ds_row = match ds_res {
            Ok(r) => r,
            Err(err) if is_unique_violation(&err) => {
                return Err(AppError::Conflict("NAME_TAKEN"));
            }
            Err(err) => {
                return Err(AppError::Database(format!(
                    "create webhook dataset failed: {err}"
                )));
            }
        };
        let dataset = decode_dataset(&ds_row)?;

        let wh_res = sqlx::query(
            "INSERT INTO ds_webhooks (
                 tenant_id, dataset_id, slug, auth_scheme, credential_id,
                 record_path, max_body_bytes, enabled
             )
             VALUES ($1, $2, $3, $4, $5, $6, $7, TRUE)
             RETURNING id, dataset_id, slug, auth_scheme, record_path, max_body_bytes, enabled, created_at",
        )
        .bind(tenant_id)
        .bind(dataset.id)
        .bind(slug)
        .bind(req.auth_scheme.as_str())
        .bind(credential.id)
        .bind(&record_path)
        .bind(req.max_body_bytes)
        .fetch_one(&mut *tx)
        .await;

        let wh_row = match wh_res {
            Ok(r) => r,
            Err(err) if is_unique_violation(&err) => {
                return Err(AppError::Conflict("NAME_TAKEN"));
            }
            Err(err) => {
                return Err(AppError::Database(format!(
                    "insert ds_webhooks failed: {err}"
                )));
            }
        };

        let webhook = decode_webhook_config(&wh_row)?;
        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit create_webhook tx failed: {err}")))?;
        Ok((webhook, dataset))
    }

    async fn list_webhooks(&self, tenant_id: &str) -> Result<Vec<WebhookConfig>, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let rows = sqlx::query(
            "SELECT id, dataset_id, slug, auth_scheme, record_path, max_body_bytes, enabled, created_at
               FROM ds_webhooks
              ORDER BY created_at DESC",
        )
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("list ds_webhooks failed: {err}")))?;
        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit list_webhooks tx failed: {err}")))?;

        rows.iter().map(decode_webhook_config).collect()
    }

    async fn delete_webhook(&self, tenant_id: &str, id: Uuid) -> Result<bool, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let deleted = sqlx::query("DELETE FROM ds_webhooks WHERE id = $1 RETURNING credential_id")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|err| AppError::Database(format!("delete ds_webhooks failed: {err}")))?;

        let Some(row) = deleted else {
            tx.commit().await.map_err(|err| {
                AppError::Database(format!("commit noop delete_webhook tx failed: {err}"))
            })?;
            return Ok(false);
        };

        let cred_id: Uuid = row.get("credential_id");
        delete_orphan_credential_tx(&mut tx, cred_id).await?;

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit delete_webhook tx failed: {err}")))?;
        Ok(true)
    }

    async fn resolve_webhook(
        &self,
        tenant_id: &str,
        slug: &str,
    ) -> Result<Option<ResolvedWebhook>, AppError> {
        let maybe_row = sqlx::query(
            "SELECT webhook_id, dataset_id, auth_scheme, credential_id, record_path, max_body_bytes
               FROM ax_resolve_webhook($1, $2)",
        )
        .bind(tenant_id)
        .bind(slug)
        .fetch_optional(self.pool())
        .await
        .map_err(|err| AppError::Database(format!("ax_resolve_webhook failed: {err}")))?;

        let Some(row) = maybe_row else {
            return Ok(None);
        };
        let auth_scheme = parse_webhook_auth_scheme(&row.get::<String, _>("auth_scheme"))?;
        Ok(Some(ResolvedWebhook {
            webhook_id: row.get("webhook_id"),
            dataset_id: row.get("dataset_id"),
            auth_scheme,
            credential_id: row.get("credential_id"),
            record_path: row.get("record_path"),
            max_body_bytes: row.get("max_body_bytes"),
        }))
    }

    async fn get_webhook_by_dataset(
        &self,
        tenant_id: &str,
        dataset_id: Uuid,
    ) -> Result<Option<WebhookConfig>, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let maybe_row = sqlx::query(
            "SELECT id, dataset_id, slug, auth_scheme, record_path, max_body_bytes, enabled, created_at
               FROM ds_webhooks
              WHERE dataset_id = $1
              ORDER BY created_at DESC
              LIMIT 1",
        )
        .bind(dataset_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("get_webhook_by_dataset failed: {err}")))?;
        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit get_webhook_by_dataset tx: {err}"))
        })?;

        maybe_row.as_ref().map(decode_webhook_config).transpose()
    }

    async fn ingest_webhook_event(
        &self,
        tenant_id: &str,
        webhook_id: Uuid,
        delivery_id: &str,
        payload: serde_json::Value,
    ) -> Result<bool, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let inserted = sqlx::query(
            "INSERT INTO ds_webhook_buffer (tenant_id, webhook_id, delivery_id, payload)
             VALUES ($1, $2, $3, $4)
             ON CONFLICT (webhook_id, delivery_id) DO NOTHING
             RETURNING id",
        )
        .bind(tenant_id)
        .bind(webhook_id)
        .bind(delivery_id)
        .bind(payload)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("insert ds_webhook_buffer failed: {err}")))?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit ingest_webhook_event tx failed: {err}"))
        })?;
        Ok(inserted.is_some())
    }

    async fn list_pending_webhook_events(
        &self,
        tenant_id: &str,
        webhook_id: Uuid,
        limit: u32,
    ) -> Result<Vec<WebhookBufferItem>, AppError> {
        let fetch_limit = i64::from(limit.clamp(1, 50_000));
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let rows = sqlx::query(
            "SELECT id, delivery_id, payload, received_at
               FROM ds_webhook_buffer
              WHERE webhook_id = $1
                AND flushed_at IS NULL
              ORDER BY id ASC
              LIMIT $2",
        )
        .bind(webhook_id)
        .bind(fetch_limit)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("list_pending_webhook_events failed: {err}")))?;
        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit list_pending_webhook_events tx: {err}"))
        })?;

        Ok(rows
            .into_iter()
            .map(|r| WebhookBufferItem {
                id: r.get("id"),
                delivery_id: r.get("delivery_id"),
                payload: r.get("payload"),
                received_at: r.get("received_at"),
            })
            .collect())
    }

    async fn mark_webhook_events_flushed(
        &self,
        tenant_id: &str,
        ids: &[i64],
    ) -> Result<(), AppError> {
        if ids.is_empty() {
            return Ok(());
        }
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        sqlx::query(
            "UPDATE ds_webhook_buffer
                SET flushed_at = NOW()
              WHERE id = ANY($1) AND flushed_at IS NULL",
        )
        .bind(ids)
        .execute(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("mark_webhook_events_flushed failed: {err}")))?;
        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit mark_webhook_events_flushed tx: {err}"))
        })?;
        Ok(())
    }

    async fn upsert_lineage(
        &self,
        tenant_id: &str,
        src_kind: &str,
        src_id: &str,
        dst_kind: &str,
        dst_id: &str,
        via: &str,
    ) -> Result<(), AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        insert_lineage_tx(&mut tx, tenant_id, src_kind, src_id, dst_kind, dst_id, via).await?;
        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit upsert_lineage tx failed: {err}")))?;
        Ok(())
    }

    async fn get_dataset_lineage(
        &self,
        tenant_id: &str,
        dataset_id: Uuid,
        depth: u32,
    ) -> Result<Vec<LineageEdge>, AppError> {
        if depth == 0 {
            return Ok(Vec::new());
        }
        let max_depth = i32::try_from(depth.min(10)).unwrap_or(3);
        let ds_id_str = dataset_id.to_string();

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let rows = sqlx::query(
            "WITH RECURSIVE upstream AS (
                 SELECT l.id, l.src_kind, l.src_id, l.dst_kind, l.dst_id, l.via, l.created_at,
                        1 AS hop, ARRAY[l.id] AS visited
                   FROM ds_lineage l
                  WHERE l.dst_kind = 'DATASET' AND l.dst_id = $1
                 UNION ALL
                 SELECT l.id, l.src_kind, l.src_id, l.dst_kind, l.dst_id, l.via, l.created_at,
                        u.hop + 1, u.visited || l.id
                   FROM ds_lineage l
                   JOIN upstream u ON l.dst_kind = u.src_kind AND l.dst_id = u.src_id
                  WHERE u.hop < $2 AND NOT (l.id = ANY(u.visited))
             ),
             downstream AS (
                 SELECT l.id, l.src_kind, l.src_id, l.dst_kind, l.dst_id, l.via, l.created_at,
                        1 AS hop, ARRAY[l.id] AS visited
                   FROM ds_lineage l
                  WHERE l.src_kind = 'DATASET' AND l.src_id = $1
                 UNION ALL
                 SELECT l.id, l.src_kind, l.src_id, l.dst_kind, l.dst_id, l.via, l.created_at,
                        d.hop + 1, d.visited || l.id
                   FROM ds_lineage l
                   JOIN downstream d ON l.src_kind = d.dst_kind AND l.src_id = d.dst_id
                  WHERE d.hop < $2 AND NOT (l.id = ANY(d.visited))
             )
             SELECT DISTINCT id, src_kind, src_id, dst_kind, dst_id, via, created_at
               FROM (
                   SELECT id, src_kind, src_id, dst_kind, dst_id, via, created_at FROM upstream
                   UNION ALL
                   SELECT id, src_kind, src_id, dst_kind, dst_id, via, created_at FROM downstream
               ) combined
              ORDER BY id ASC",
        )
        .bind(ds_id_str)
        .bind(max_depth)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("get_dataset_lineage failed: {err}")))?;

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit get_dataset_lineage tx failed: {err}"))
        })?;

        Ok(rows
            .into_iter()
            .map(|r| LineageEdge {
                id: r.get("id"),
                src_kind: r.get("src_kind"),
                src_id: r.get("src_id"),
                dst_kind: r.get("dst_kind"),
                dst_id: r.get("dst_id"),
                via: r.get("via"),
                created_at: r.get("created_at"),
            })
            .collect())
    }

    async fn search_catalog(
        &self,
        tenant_id: &str,
        query: &str,
        limit: u32,
    ) -> Result<PaginatedList<CatalogSearchHit>, AppError> {
        let fetch_limit = i64::from(limit.clamp(1, 200));
        let trimmed = query.trim();
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;

        if trimmed.is_empty() {
            return search_catalog_empty_tx(tx, fetch_limit).await;
        }

        let terms = expand_glossary_terms_tx(&mut tx, trimmed).await?;
        let patterns: Vec<String> = terms.iter().map(|t| format!("%{t}%")).collect();

        let rows = sqlx::query(
            "SELECT d.id, d.name, d.display_name, d.description, d.source_kind, d.row_count, d.tags,
                    COALESCE(
                        ARRAY(
                            SELECT cm.column_name
                              FROM ds_column_meta cm
                             WHERE cm.dataset_id = d.id
                               AND (
                                   cm.column_name ILIKE ANY($2)
                                   OR cm.description ILIKE ANY($2)
                                   OR cm.business_term ILIKE ANY($2)
                               )
                             ORDER BY cm.ordinal ASC
                        ),
                        '{}'::text[]
                    ) AS matched_columns
               FROM ds_datasets d
              WHERE EXISTS (
                        SELECT 1 FROM unnest($1::text[]) AS t
                         WHERE d.fts @@ plainto_tsquery('simple', t)
                    )
                 OR d.name ILIKE ANY($2)
                 OR d.display_name ILIKE ANY($2)
                 OR d.description ILIKE ANY($2)
                 OR EXISTS (
                        SELECT 1 FROM unnest(d.tags) AS tg
                         WHERE tg ILIKE ANY($2)
                    )
                 OR EXISTS (
                        SELECT 1 FROM ds_column_meta cm
                         WHERE cm.dataset_id = d.id
                           AND (
                               cm.column_name ILIKE ANY($2)
                               OR cm.description ILIKE ANY($2)
                               OR cm.business_term ILIKE ANY($2)
                           )
                    )
              ORDER BY d.updated_at DESC, d.name ASC
              LIMIT $3",
        )
        .bind(&terms)
        .bind(&patterns)
        .bind(fetch_limit)
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("search_catalog query failed: {err}")))?;

        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit search_catalog tx failed: {err}")))?;

        let mut items = Vec::with_capacity(rows.len());
        for r in rows {
            items.push(CatalogSearchHit {
                id: r.get("id"),
                name: r.get("name"),
                display_name: r.get("display_name"),
                description: r.get("description"),
                source_kind: SourceKind::from_str(&r.get::<String, _>("source_kind"))?,
                row_count: r.get("row_count"),
                tags: r.get("tags"),
                matched_columns: r.get("matched_columns"),
            });
        }

        Ok(PaginatedList {
            items,
            next_cursor: None,
        })
    }

    async fn list_glossary(&self, tenant_id: &str) -> Result<Vec<GlossaryTerm>, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let rows = sqlx::query(
            "SELECT id, term, definition, synonyms, created_at
               FROM ds_glossary_terms
              ORDER BY term ASC",
        )
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::Database(format!("list ds_glossary_terms failed: {err}")))?;
        tx.commit()
            .await
            .map_err(|err| AppError::Database(format!("commit list_glossary tx failed: {err}")))?;

        Ok(rows
            .into_iter()
            .map(|r| GlossaryTerm {
                id: r.get("id"),
                term: r.get("term"),
                definition: r.get("definition"),
                synonyms: r.get("synonyms"),
                created_at: r.get("created_at"),
            })
            .collect())
    }

    async fn create_glossary_term(
        &self,
        tenant_id: &str,
        req: CreateGlossaryTermRequest,
    ) -> Result<GlossaryTerm, AppError> {
        let term = req.term.trim();
        if term.is_empty() {
            return Err(AppError::bad("INVALID_ARGUMENT", "term cannot be empty"));
        }
        let synonyms: Vec<String> = req
            .synonyms
            .into_iter()
            .map(|s| String::from(s.trim()))
            .filter(|s| !s.is_empty())
            .collect();

        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let insert_res = sqlx::query(
            "INSERT INTO ds_glossary_terms (tenant_id, term, definition, synonyms)
             VALUES ($1, $2, $3, $4)
             RETURNING id, term, definition, synonyms, created_at",
        )
        .bind(tenant_id)
        .bind(term)
        .bind(req.definition.trim())
        .bind(synonyms)
        .fetch_one(&mut *tx)
        .await;

        let row = match insert_res {
            Ok(r) => r,
            Err(err) if is_unique_violation(&err) => {
                return Err(AppError::Conflict("NAME_TAKEN"));
            }
            Err(err) => {
                return Err(AppError::Database(format!(
                    "insert ds_glossary_terms failed: {err}"
                )));
            }
        };

        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit create_glossary_term tx failed: {err}"))
        })?;

        Ok(GlossaryTerm {
            id: row.get("id"),
            term: row.get("term"),
            definition: row.get("definition"),
            synonyms: row.get("synonyms"),
            created_at: row.get("created_at"),
        })
    }

    async fn delete_glossary_term(&self, tenant_id: &str, id: Uuid) -> Result<bool, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let res = sqlx::query("DELETE FROM ds_glossary_terms WHERE id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(|err| AppError::Database(format!("delete ds_glossary_terms failed: {err}")))?;
        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit delete_glossary_term tx failed: {err}"))
        })?;
        Ok(res.rows_affected() > 0)
    }

    async fn list_scheduled_datasets(&self) -> Result<Vec<ScheduledDataset>, AppError> {
        let rows = sqlx::query(
            "SELECT tenant_id, dataset_id, schedule_cron, last_synced_at
               FROM ax_scheduled_datasets()",
        )
        .fetch_all(self.pool())
        .await
        .map_err(|err| AppError::Database(format!("ax_scheduled_datasets failed: {err}")))?;

        Ok(rows
            .into_iter()
            .map(|r| ScheduledDataset {
                tenant_id: r.get("tenant_id"),
                dataset_id: r.get("dataset_id"),
                schedule_cron: r.get("schedule_cron"),
                last_synced_at: r.get("last_synced_at"),
            })
            .collect())
    }

    async fn get_tenant_timezone(&self, tenant_id: &str) -> Result<String, AppError> {
        let maybe_tz: Option<String> =
            sqlx::query_scalar("SELECT timezone FROM sec_tenants WHERE tenant_id = $1")
                .bind(tenant_id)
                .fetch_optional(self.pool())
                .await
                .map_err(|err| {
                    AppError::Database(format!("query sec_tenants.timezone failed: {err}"))
                })?;

        Ok(maybe_tz.unwrap_or_else(|| String::from("Asia/Shanghai")))
    }

    async fn get_lake_snapshot_keep_days(&self, tenant_id: &str) -> Result<i32, AppError> {
        let mut tx = begin_tenant_tx(self.pool(), tenant_id).await?;
        let maybe_days: Option<i32> = sqlx::query_scalar(
            "SELECT keep_days
               FROM ax_retention_policies
              WHERE tenant_id = $1 AND target = 'LAKE_SNAPSHOT'",
        )
        .bind(tenant_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| {
            AppError::Database(format!("query LAKE_SNAPSHOT retention failed: {err}"))
        })?;
        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit get_lake_snapshot_keep_days tx: {err}"))
        })?;

        Ok(maybe_days.unwrap_or(30))
    }
}

async fn upsert_column_profiles_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    dataset_id: Uuid,
    schema: &[ColumnProfile],
) -> Result<(), AppError> {
    for (idx, col) in schema.iter().enumerate() {
        let ordinal = if col.ordinal > 0 {
            col.ordinal
        } else {
            i32::try_from(idx).unwrap_or(i32::MAX)
        };
        sqlx::query(
            "INSERT INTO ds_column_meta (
                 tenant_id, dataset_id, column_name, data_type, pii_category,
                 pii_confirmed, description, business_term, unit, ordinal
             )
             VALUES ($1, $2, $3, $4, $5, FALSE, $6, '', '', $7)
             ON CONFLICT (dataset_id, column_name) DO UPDATE SET
                 data_type = EXCLUDED.data_type,
                 ordinal = EXCLUDED.ordinal,
                 pii_category = CASE
                     WHEN ds_column_meta.pii_confirmed THEN ds_column_meta.pii_category
                     ELSE EXCLUDED.pii_category
                 END,
                 description = CASE
                     WHEN ds_column_meta.description <> '' THEN ds_column_meta.description
                     ELSE EXCLUDED.description
                 END",
        )
        .bind(tenant_id)
        .bind(dataset_id)
        .bind(&col.name)
        .bind(col.data_type.as_str())
        .bind(col.pii_category.as_str())
        .bind(&col.source_comment)
        .bind(ordinal)
        .execute(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("upsert ds_column_meta failed: {err}")))?;
    }
    Ok(())
}

async fn upsert_stream_offsets_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    dataset_id: Uuid,
    offsets: &BTreeMap<String, String>,
) -> Result<(), AppError> {
    for (partition, last_offset) in offsets {
        sqlx::query(
            "INSERT INTO ds_stream_offsets (tenant_id, dataset_id, partition, last_offset, updated_at)
             VALUES ($1, $2, $3, $4, NOW())
             ON CONFLICT (dataset_id, partition) DO UPDATE SET
                 last_offset = EXCLUDED.last_offset,
                 updated_at = NOW()",
        )
        .bind(tenant_id)
        .bind(dataset_id)
        .bind(partition)
        .bind(last_offset)
        .execute(&mut **tx)
        .await
        .map_err(|err| AppError::Database(format!("upsert ds_stream_offsets failed: {err}")))?;
    }
    Ok(())
}

async fn expand_glossary_terms_tx(
    tx: &mut Transaction<'_, Postgres>,
    raw_query: &str,
) -> Result<Vec<String>, AppError> {
    let like = format!("%{raw_query}%");
    let rows = sqlx::query(
        "SELECT term, synonyms
           FROM ds_glossary_terms
          WHERE term ILIKE $1
             OR $2 ILIKE '%' || term || '%'
             OR EXISTS (
                 SELECT 1 FROM unnest(synonyms) AS s
                  WHERE s ILIKE $1 OR $2 ILIKE '%' || s || '%'
             )",
    )
    .bind(&like)
    .bind(raw_query)
    .fetch_all(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("expand ds_glossary_terms failed: {err}")))?;

    let mut set = BTreeSet::new();
    set.insert(String::from(raw_query));
    for r in rows {
        let term: String = r.get("term");
        if !term.trim().is_empty() {
            set.insert(term);
        }
        let syns: Vec<String> = r.get("synonyms");
        for syn in syns {
            if !syn.trim().is_empty() {
                set.insert(syn);
            }
        }
    }

    Ok(set.into_iter().collect())
}

async fn search_catalog_empty_tx(
    mut tx: Transaction<'_, Postgres>,
    fetch_limit: i64,
) -> Result<PaginatedList<CatalogSearchHit>, AppError> {
    let rows = sqlx::query(
        "SELECT id, name, display_name, description, source_kind, row_count, tags
           FROM ds_datasets
          ORDER BY updated_at DESC
          LIMIT $1",
    )
    .bind(fetch_limit)
    .fetch_all(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("catalog empty search failed: {err}")))?;
    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit empty catalog search tx: {err}")))?;

    let mut items = Vec::with_capacity(rows.len());
    for r in rows {
        items.push(CatalogSearchHit {
            id: r.get("id"),
            name: r.get("name"),
            display_name: r.get("display_name"),
            description: r.get("description"),
            source_kind: SourceKind::from_str(&r.get::<String, _>("source_kind"))?,
            row_count: r.get("row_count"),
            tags: r.get("tags"),
            matched_columns: Vec::new(),
        });
    }
    Ok(PaginatedList {
        items,
        next_cursor: None,
    })
}

async fn create_dataset_impl(
    store: &PgStore,
    tenant_id: &str,
    owner_user_id: Option<Uuid>,
    req: CreateDatasetRequest,
) -> Result<Dataset, AppError> {
    let name = req.name.trim();
    if !is_valid_dataset_name(name) {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            format!("dataset name '{name}' must match ^[a-z][a-z0-9_]{{0,62}}$"),
        ));
    }
    if req.sync_mode == SyncMode::Upsert && req.primary_key.is_empty() {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "primary_key must be non-empty when sync_mode is UPSERT",
        ));
    }

    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    if let Some(conn_id) = req.connection_id {
        let exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM ds_connections WHERE id = $1)")
                .bind(conn_id)
                .fetch_one(&mut *tx)
                .await
                .map_err(|err| AppError::Database(format!("check ds_connections failed: {err}")))?;
        if !exists {
            return Err(AppError::NotFound("CONNECTION_NOT_FOUND"));
        }
    }

    let dataset = insert_new_dataset_row_tx(&mut tx, tenant_id, owner_user_id, name, &req).await?;
    if let Some(conn_id) = dataset.connection_id {
        insert_lineage_tx(
            &mut tx,
            tenant_id,
            "CONNECTION",
            &conn_id.to_string(),
            "DATASET",
            &dataset.id.to_string(),
            dataset.sync_mode.as_str(),
        )
        .await?;
    }

    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit create_dataset tx failed: {err}")))?;
    Ok(dataset)
}

async fn insert_new_dataset_row_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    owner_user_id: Option<Uuid>,
    name: &str,
    req: &CreateDatasetRequest,
) -> Result<Dataset, AppError> {
    let display_name = if req.display_name.trim().is_empty() {
        String::from(name)
    } else {
        String::from(req.display_name.trim())
    };
    let source_ref = if req.source_ref.is_null() {
        serde_json::json!({})
    } else {
        req.source_ref.clone()
    };
    let lake_schema = format!("tenant_{tenant_id}");
    let schedule_cron = req
        .schedule_cron
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from);

    let insert_res = sqlx::query(
        "INSERT INTO ds_datasets (
             tenant_id, connection_id, name, display_name, description, source_kind,
             source_ref, sync_mode, primary_key, cursor_column, delete_column,
             schema_policy, lake_schema, lake_table, schedule_cron, tags, owner_user_id
         )
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17)
         RETURNING id, connection_id, name, display_name, description, source_kind, source_ref,
                   sync_mode, primary_key, cursor_column, cursor_value, delete_column, schema_policy,
                   lake_schema, lake_table, schema_json, schema_version, row_count, last_snapshot_id,
                   last_synced_at, schedule_cron, tags, owner_user_id, status, created_at, updated_at",
    )
    .bind(tenant_id)
    .bind(req.connection_id)
    .bind(name)
    .bind(display_name)
    .bind(req.description.trim())
    .bind(req.source_kind.as_str())
    .bind(source_ref)
    .bind(req.sync_mode.as_str())
    .bind(&req.primary_key)
    .bind(req.cursor_column.as_deref().map(str::trim).filter(|s| !s.is_empty()))
    .bind(req.delete_column.as_deref().map(str::trim).filter(|s| !s.is_empty()))
    .bind(req.schema_policy.as_str())
    .bind(&lake_schema)
    .bind(name)
    .bind(schedule_cron)
    .bind(&req.tags)
    .bind(owner_user_id)
    .fetch_one(&mut **tx)
    .await;

    let row = match insert_res {
        Ok(r) => r,
        Err(err) if is_unique_violation(&err) => return Err(AppError::Conflict("NAME_TAKEN")),
        Err(err) => {
            return Err(AppError::Database(format!(
                "insert ds_datasets failed: {err}"
            )));
        }
    };
    decode_dataset(&row)
}

fn non_empty_trimmed(val: Option<String>) -> Option<String> {
    val.and_then(|s| {
        let t = s.trim();
        (!t.is_empty()).then(|| String::from(t))
    })
}

async fn update_dataset_impl(
    store: &PgStore,
    tenant_id: &str,
    id: Uuid,
    req: UpdateDatasetRequest,
) -> Result<Option<Dataset>, AppError> {
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    let Some(existing_row) = sqlx::query(
        "SELECT id, connection_id, name, display_name, description, source_kind, source_ref,
                sync_mode, primary_key, cursor_column, cursor_value, delete_column, schema_policy,
                lake_schema, lake_table, schema_json, schema_version, row_count, last_snapshot_id,
                last_synced_at, schedule_cron, tags, owner_user_id, status, created_at, updated_at
           FROM ds_datasets
          WHERE id = $1
          FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("load ds_datasets for update: {err}")))?
    else {
        tx.commit().await.map_err(|err| {
            AppError::Database(format!("commit empty update_dataset tx failed: {err}"))
        })?;
        return Ok(None);
    };

    let current = decode_dataset(&existing_row)?;
    let sync_mode = req.sync_mode.unwrap_or(current.sync_mode);
    let primary_key = req.primary_key.unwrap_or(current.primary_key);
    if sync_mode == SyncMode::Upsert && primary_key.is_empty() {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "primary_key must be non-empty when sync_mode is UPSERT",
        ));
    }

    let display_name = req
        .display_name
        .as_deref()
        .map(|s| String::from(s.trim()))
        .unwrap_or(current.display_name);
    let description = req
        .description
        .as_deref()
        .map(|s| String::from(s.trim()))
        .unwrap_or(current.description);
    let cursor_column = req
        .cursor_column
        .map_or(current.cursor_column, non_empty_trimmed);
    let delete_column = req
        .delete_column
        .map_or(current.delete_column, non_empty_trimmed);
    let schedule_cron = req
        .schedule_cron
        .map_or(current.schedule_cron, non_empty_trimmed);
    let schema_policy = req.schema_policy.unwrap_or(current.schema_policy);
    let tags = req.tags.unwrap_or(current.tags);
    let status = req.status.unwrap_or(current.status);

    let updated_row = sqlx::query(
        "UPDATE ds_datasets
            SET display_name = $2, description = $3, sync_mode = $4, primary_key = $5,
                cursor_column = $6, delete_column = $7, schema_policy = $8, schedule_cron = $9,
                tags = $10, status = $11, updated_at = NOW()
          WHERE id = $1
          RETURNING id, connection_id, name, display_name, description, source_kind, source_ref,
                    sync_mode, primary_key, cursor_column, cursor_value, delete_column, schema_policy,
                    lake_schema, lake_table, schema_json, schema_version, row_count, last_snapshot_id,
                    last_synced_at, schedule_cron, tags, owner_user_id, status, created_at, updated_at",
    )
    .bind(id)
    .bind(display_name)
    .bind(description)
    .bind(sync_mode.as_str())
    .bind(primary_key)
    .bind(cursor_column)
    .bind(delete_column)
    .bind(schema_policy.as_str())
    .bind(schedule_cron)
    .bind(tags)
    .bind(status.as_str())
    .fetch_one(&mut *tx)
    .await
    .map_err(|err| AppError::Database(format!("update ds_datasets failed: {err}")))?;

    tx.commit()
        .await
        .map_err(|err| AppError::Database(format!("commit update_dataset tx failed: {err}")))?;
    Ok(Some(decode_dataset(&updated_row)?))
}

async fn complete_sync_execution_impl(
    store: &PgStore,
    tenant_id: &str,
    sync_job_id: Uuid,
    dataset_id: Uuid,
    result: &SidecarSyncResult,
) -> Result<(), AppError> {
    let mut tx = begin_tenant_tx(store.pool(), tenant_id).await?;
    apply_sync_job_and_dataset_stats_tx(&mut tx, sync_job_id, dataset_id, result).await?;
    upsert_column_profiles_tx(&mut tx, tenant_id, dataset_id, &result.schema).await?;
    upsert_stream_offsets_tx(&mut tx, tenant_id, dataset_id, &result.stream_offsets).await?;

    insert_outbox_tx(
        &mut tx,
        tenant_id,
        &NewOutboxEvent {
            topic: OutboxTopic::DatasetSynced,
            payload: serde_json::json!({
                "dataset_id": dataset_id,
                "snapshot_id": result.snapshot_id,
                "rows_written": result.rows_written,
                "quality": result.quality,
            }),
        },
    )
    .await?;

    insert_system_audit_tx(
        &mut tx,
        tenant_id,
        AuditAction::DataSync,
        &format!("dataset:{dataset_id}"),
        serde_json::json!({
            "sync_job_id": sync_job_id,
            "snapshot_id": result.snapshot_id,
            "rows_written": result.rows_written,
        }),
    )
    .await?;

    tx.commit().await.map_err(|err| {
        AppError::Database(format!("commit complete_sync_execution tx failed: {err}"))
    })?;
    Ok(())
}

async fn apply_sync_job_and_dataset_stats_tx(
    tx: &mut Transaction<'_, Postgres>,
    sync_job_id: Uuid,
    dataset_id: Uuid,
    result: &SidecarSyncResult,
) -> Result<(), AppError> {
    let ds_row = sqlx::query(
        "SELECT sync_mode, row_count, cursor_value, schema_version, schema_json
           FROM ds_datasets
          WHERE id = $1
          FOR UPDATE",
    )
    .bind(dataset_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("lock dataset for complete_sync: {err}")))?
    .ok_or(AppError::NotFound("DATASET_NOT_FOUND"))?;

    let sync_mode = SyncMode::from_str(&ds_row.get::<String, _>("sync_mode"))?;
    let prev_row_count: i64 = ds_row.get("row_count");
    let prev_cursor: Option<String> = ds_row.get("cursor_value");
    let prev_schema_ver: i32 = ds_row.get("schema_version");
    let prev_schema_json: serde_json::Value = ds_row.get("schema_json");

    let new_row_count = result.total_rows.map_or_else(
        || match sync_mode {
            SyncMode::Snapshot => result.rows_written.max(0),
            SyncMode::Append | SyncMode::Upsert => prev_row_count
                .saturating_add(result.rows_written)
                .saturating_sub(result.rows_deleted)
                .max(0),
        },
        |n| n.max(0),
    );
    let new_cursor = result.cursor_to.clone().or(prev_cursor);
    let schema_changed =
        (prev_schema_ver == 0 && !result.schema.is_empty()) || result.schema_change.has_changes();
    let new_schema_ver = if schema_changed {
        prev_schema_ver.saturating_add(1)
    } else {
        prev_schema_ver
    };
    let new_schema_json = if result.schema.is_empty() {
        prev_schema_json
    } else {
        serde_json::to_value(&result.schema)
            .map_err(|err| AppError::bad("INVALID_ARGUMENT", format!("serialize schema: {err}")))?
    };
    let sc_json = serde_json::to_value(&result.schema_change).map_err(|err| {
        AppError::bad(
            "INVALID_ARGUMENT",
            format!("serialize schema_change: {err}"),
        )
    })?;
    let qr_json = serde_json::to_value(&result.quality)
        .map_err(|err| AppError::bad("INVALID_ARGUMENT", format!("serialize quality: {err}")))?;

    sqlx::query(
        "UPDATE ds_sync_jobs
            SET status = 'SUCCEEDED', cursor_to = $2, rows_read = $3, rows_written = $4,
                rows_deleted = $5, snapshot_id = $6, schema_change = $7, quality_result = $8,
                finished_at = NOW()
          WHERE id = $1",
    )
    .bind(sync_job_id)
    .bind(&result.cursor_to)
    .bind(result.rows_read)
    .bind(result.rows_written)
    .bind(result.rows_deleted)
    .bind(result.snapshot_id)
    .bind(sc_json)
    .bind(qr_json)
    .execute(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("complete ds_sync_jobs failed: {err}")))?;

    sqlx::query(
        "UPDATE ds_datasets
            SET cursor_value = $2, schema_json = $3, schema_version = $4, row_count = $5,
                last_snapshot_id = $6, last_synced_at = NOW(), status = 'READY', updated_at = NOW()
          WHERE id = $1",
    )
    .bind(dataset_id)
    .bind(new_cursor)
    .bind(new_schema_json)
    .bind(new_schema_ver)
    .bind(new_row_count)
    .bind(result.snapshot_id)
    .execute(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("update dataset post-sync failed: {err}")))?;
    Ok(())
}
