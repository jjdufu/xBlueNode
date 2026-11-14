//! `AxHub` data fabric HTTP routes (`/api/axhub/*`) for Step 02.

use std::collections::{BTreeMap, HashMap};
use std::str::FromStr;
use std::sync::{LazyLock, Mutex};
use std::time::Instant;

use axpoint_domain::AppError;
use axpoint_domain::axhub::{
    CatalogSearchHit, ColumnMeta, Connection, ConnectionPreviewRequest, ConnectionStatus,
    ConnectionTestResult, ConnectorSpecDto, CreateConnectionRequest, CreateDatasetRequest,
    CreateGlossaryTermRequest, CreateQualityRuleRequest, CreateWebhookRequest,
    CreateWebhookResponse, Dataset, DatasetListQuery, DiscoveredObject, EnqueueUploadResponse,
    GlossaryTerm, LakePreviewRequest, LakeSnapshotInfo, LineageEdge, NewConnectionInput,
    NewUploadInput, PaginatedList, PreviewResponse, QualityRule, SealedCredential,
    SidecarConnectorRequest, SidecarPreviewRequest, SyncDatasetResponse, SyncJob,
    UpdateColumnMetaRequest, UpdateConnectionInput, UpdateConnectionRequest, UpdateDatasetRequest,
    UploadRecord, WebhookAuthScheme, WebhookConfig,
};
use axpoint_domain::enums::{ConnectorType, JobKind, Permission, SyncMode};
use axpoint_domain::platform::NewJob;
use axpoint_domain::ports::{
    ConnectionRepository as _, DatasetRepository as _, JobQueuePort as _, SidecarPort as _,
};
use axpoint_security::{AuthPrincipal, CredentialKeyring};
use axum::body::Bytes;
use axum::extract::{Multipart, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use chrono::Utc;
use hmac::{Hmac, Mac};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::state::{ApiJson, ApiPath, ApiQuery, AppState, HttpError};

type HmacSha256 = Hmac<Sha256>;

const INTERNAL_SECRET_FIELDS_KEY: &str = "__secret_fields";
const WEBHOOK_TIMESTAMP_SKEW_SECS: i64 = 300;
const WEBHOOK_RATE_PER_SEC: f64 = 50.0;
const WEBHOOK_BURST_CAPACITY: f64 = 100.0;

struct TokenBucket {
    tokens: f64,
    last_refill: Instant,
}

static WEBHOOK_RATE_LIMITERS: LazyLock<Mutex<HashMap<(String, String), TokenBucket>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn check_webhook_rate_limit(tenant_id: &str, slug: &str) -> Result<(), AppError> {
    let mut guard = WEBHOOK_RATE_LIMITERS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let now = Instant::now();
    let entry = guard
        .entry((tenant_id.to_owned(), slug.to_owned()))
        .or_insert(TokenBucket {
            tokens: WEBHOOK_BURST_CAPACITY,
            last_refill: now,
        });
    let elapsed = now.duration_since(entry.last_refill).as_secs_f64();
    entry.tokens = (entry.tokens + elapsed * WEBHOOK_RATE_PER_SEC).min(WEBHOOK_BURST_CAPACITY);
    entry.last_refill = now;
    if entry.tokens < 1.0 {
        return Err(AppError::RateLimited);
    }
    entry.tokens -= 1.0;
    Ok(())
}

/// Returns the secret configuration field keys for `connector_type` (`03-STEP02` §02.2).
pub(crate) const fn secret_fields_for(connector_type: ConnectorType) -> &'static [&'static str] {
    match connector_type {
        ConnectorType::FileUpload | ConnectorType::Sqlite | ConnectorType::Hbase => &[],
        ConnectorType::S3 => &["access_key_id", "secret_access_key"],
        ConnectorType::Sftp => &["password", "private_key"],
        ConnectorType::Postgres
        | ConnectorType::Mysql
        | ConnectorType::Mssql
        | ConnectorType::Oracle
        | ConnectorType::Clickhouse
        | ConnectorType::Dameng
        | ConnectorType::Kingbase
        | ConnectorType::Opengauss
        | ConnectorType::Oceanbase
        | ConnectorType::Tidb
        | ConnectorType::Doris
        | ConnectorType::Starrocks => &["password"],
        ConnectorType::Hive => &["password", "keytab_b64"],
        ConnectorType::Mongodb
        | ConnectorType::Redis
        | ConnectorType::Cassandra
        | ConnectorType::Neo4j
        | ConnectorType::Tdengine
        | ConnectorType::Iotdb
        | ConnectorType::Mqtt => &["username", "password"],
        ConnectorType::Elasticsearch => &["api_key", "username", "password"],
        ConnectorType::Influxdb | ConnectorType::Pulsar => &["token"],
        ConnectorType::Kafka => &["sasl_username", "sasl_password", "ssl_key_pem"],
        ConnectorType::RestApi | ConnectorType::Graphql => &[
            "username",
            "password",
            "token",
            "api_key",
            "client_id",
            "client_secret",
        ],
        ConnectorType::Webhook => &["secret"],
    }
}

/// Opens a [`SealedCredential`] into a `BTreeMap<String, String>` bound to `(tenant_id, cred.id)`.
pub(crate) fn open_secret_map(
    keyring: &CredentialKeyring,
    tenant_id: &str,
    sealed: Option<&SealedCredential>,
) -> Result<BTreeMap<String, String>, AppError> {
    let Some(cred) = sealed else {
        return Ok(BTreeMap::new());
    };
    let aad = CredentialKeyring::aad_for_credential(tenant_id, cred.id);
    let plain = keyring.open_credential(&aad, cred)?;
    serde_json::from_slice(plain.as_slice())
        .map_err(|_| AppError::Forbidden("CREDENTIAL_DECRYPT_FAILED"))
}

fn seal_secret_map(
    keyring: &CredentialKeyring,
    tenant_id: &str,
    cred_id: Uuid,
    secrets: &BTreeMap<String, String>,
) -> Result<SealedCredential, AppError> {
    let encoded = Zeroizing::new(
        serde_json::to_vec(secrets)
            .map_err(|err| AppError::Config(format!("serialize secrets failed: {err}")))?,
    );
    let aad = CredentialKeyring::aad_for_credential(tenant_id, cred_id);
    keyring.seal_credential(cred_id, &aad, encoded.as_slice())
}

fn split_and_seal_create(
    keyring: &CredentialKeyring,
    tenant_id: &str,
    connector_type: ConnectorType,
    mut raw_config: serde_json::Map<String, serde_json::Value>,
    explicit_secrets: BTreeMap<String, String>,
) -> Result<(serde_json::Value, Option<SealedCredential>), AppError> {
    raw_config.remove(INTERNAL_SECRET_FIELDS_KEY);
    let secret_keys = secret_fields_for(connector_type);
    let mut secrets_map = BTreeMap::new();

    for &skey in secret_keys {
        if let Some(val) = raw_config.remove(skey)
            && let Some(s) = val.as_str()
            && !s.is_empty()
        {
            secrets_map.insert(String::from(skey), String::from(s));
        }
    }
    for (k, v) in explicit_secrets {
        raw_config.remove(&k);
        if !v.is_empty() {
            secrets_map.insert(k, v);
        }
    }

    let credential = if secrets_map.is_empty() {
        None
    } else {
        let keys_json: Vec<serde_json::Value> = secrets_map
            .keys()
            .map(|k| serde_json::Value::String(k.clone()))
            .collect();
        raw_config.insert(
            String::from(INTERNAL_SECRET_FIELDS_KEY),
            serde_json::Value::Array(keys_json),
        );
        Some(seal_secret_map(
            keyring,
            tenant_id,
            Uuid::new_v4(),
            &secrets_map,
        )?)
    };

    Ok((serde_json::Value::Object(raw_config), credential))
}

fn split_and_seal_update(
    keyring: &CredentialKeyring,
    tenant_id: &str,
    existing_conn: &Connection,
    existing_cred: Option<&SealedCredential>,
    req: UpdateConnectionRequest,
) -> Result<UpdateConnectionInput, AppError> {
    if req.config.is_none() && req.secrets.is_none() {
        return Ok(UpdateConnectionInput {
            name: req.name,
            config: None,
            allow_private_network: req.allow_private_network,
            credential: None,
        });
    }

    let secret_keys = secret_fields_for(existing_conn.connector_type);
    let mut plain_cfg = existing_conn.config.clone();
    plain_cfg.remove(INTERNAL_SECRET_FIELDS_KEY);
    let mut secrets_map = open_secret_map(keyring, tenant_id, existing_cred)?;

    if let Some(incoming_cfg) = req.config {
        for (k, v) in incoming_cfg {
            if k == INTERNAL_SECRET_FIELDS_KEY {
                continue;
            }
            if secret_keys.contains(&k.as_str()) {
                match v {
                    serde_json::Value::Null => {
                        secrets_map.remove(&k);
                    }
                    serde_json::Value::String(s) => {
                        if s.is_empty() {
                            secrets_map.remove(&k);
                        } else {
                            secrets_map.insert(k, s);
                        }
                    }
                    _ => {
                        return Err(AppError::bad(
                            "INVALID_ARGUMENT",
                            format!("secret field '{k}' must be a string or null"),
                        ));
                    }
                }
            } else if v.is_null() {
                plain_cfg.remove(&k);
            } else {
                plain_cfg.insert(k, v);
            }
        }
    }

    if let Some(incoming_secrets) = req.secrets {
        for (k, maybe_val) in incoming_secrets {
            plain_cfg.remove(&k);
            match maybe_val {
                Some(s) if !s.is_empty() => {
                    secrets_map.insert(k, s);
                }
                _ => {
                    secrets_map.remove(&k);
                }
            }
        }
    }

    let credential_update = if secrets_map.is_empty() {
        Some(None)
    } else {
        let keys_json: Vec<serde_json::Value> = secrets_map
            .keys()
            .map(|k| serde_json::Value::String(k.clone()))
            .collect();
        plain_cfg.insert(
            String::from(INTERNAL_SECRET_FIELDS_KEY),
            serde_json::Value::Array(keys_json),
        );
        let cred_id = existing_cred.map_or_else(Uuid::new_v4, |c| c.id);
        let sealed = seal_secret_map(keyring, tenant_id, cred_id, &secrets_map)?;
        Some(Some(sealed))
    };

    Ok(UpdateConnectionInput {
        name: req.name,
        config: Some(serde_json::Value::Object(plain_cfg)),
        allow_private_network: req.allow_private_network,
        credential: credential_update,
    })
}

/// Validates a 5-field (or 6-field) cron expression and returns the 6-field `cron::Schedule` string.
pub(crate) fn to_six_field_cron(expr: &str) -> Result<String, AppError> {
    let trimmed = expr.trim();
    let field_count = trimmed.split_whitespace().count();
    let six_field = match field_count {
        5 => format!("0 {trimmed}"),
        6 | 7 => String::from(trimmed),
        _ => {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                format!("schedule_cron '{trimmed}' must have 5 fields (min hour dom mon dow)"),
            ));
        }
    };
    cron::Schedule::from_str(&six_field).map_err(|err| {
        AppError::bad(
            "INVALID_ARGUMENT",
            format!("invalid schedule_cron '{trimmed}': {err}"),
        )
    })?;
    Ok(six_field)
}

/// Normalizes a filename stem or prefix into `^[a-z][a-z0-9_]{0,30}$`.
pub(crate) fn sanitize_dataset_prefix(raw: &str) -> String {
    let stem = raw
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(raw)
        .split('.')
        .next()
        .unwrap_or("upload");
    let mut out = String::new();
    let mut prev_underscore = false;
    for ch in stem.chars() {
        let lower = ch.to_ascii_lowercase();
        if lower.is_ascii_lowercase() || lower.is_ascii_digit() {
            out.push(lower);
            prev_underscore = false;
        } else if !prev_underscore && !out.is_empty() {
            out.push('_');
            prev_underscore = true;
        }
        if out.len() >= 28 {
            break;
        }
    }
    let trimmed = out.trim_matches('_');
    if trimmed.is_empty() {
        String::from("ds")
    } else if trimmed
        .as_bytes()
        .first()
        .is_some_and(u8::is_ascii_lowercase)
    {
        String::from(trimmed)
    } else {
        format!("t_{trimmed}")
    }
}

fn generate_webhook_secret_hex() -> String {
    let mut bytes = [0_u8; 32];
    let left = Uuid::new_v4();
    let right = Uuid::new_v4();
    if let Some(first_half) = bytes.get_mut(..16) {
        first_half.copy_from_slice(left.as_bytes());
    }
    if let Some(second_half) = bytes.get_mut(16..) {
        second_half.copy_from_slice(right.as_bytes());
    }
    hex::encode(bytes)
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct SyncJobsQuery {
    limit: Option<u32>,
    cursor: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct DatasetPreviewQuery {
    limit: Option<u32>,
    snapshot_id: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct LineageQuery {
    depth: Option<u32>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct CatalogSearchQuery {
    q: Option<String>,
    limit: Option<u32>,
}

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/axhub/connector-types", get(list_connector_types_handler))
        .route(
            "/axhub/connections",
            get(list_connections_handler).post(create_connection_handler),
        )
        .route(
            "/axhub/connections/{id}",
            get(get_connection_handler)
                .patch(update_connection_handler)
                .delete(delete_connection_handler),
        )
        .route(
            "/axhub/connections/{id}/test",
            post(test_connection_handler),
        )
        .route(
            "/axhub/connections/{id}/discover",
            get(discover_connection_handler),
        )
        .route(
            "/axhub/connections/{id}/preview",
            post(preview_connection_handler),
        )
        .route(
            "/axhub/credentials/rotate",
            post(rotate_credentials_handler),
        )
        .route(
            "/axhub/datasets",
            get(list_datasets_handler).post(create_dataset_handler),
        )
        .route(
            "/axhub/datasets/{id}",
            get(get_dataset_handler)
                .patch(update_dataset_handler)
                .delete(delete_dataset_handler),
        )
        .route("/axhub/datasets/{id}/sync", post(sync_dataset_handler))
        .route(
            "/axhub/datasets/{id}/sync-jobs",
            get(list_sync_jobs_handler),
        )
        .route("/axhub/datasets/{id}/preview", get(preview_dataset_handler))
        .route(
            "/axhub/datasets/{id}/snapshots",
            get(list_snapshots_handler),
        )
        .route("/axhub/datasets/{id}/columns", get(list_columns_handler))
        .route(
            "/axhub/datasets/{id}/columns/{column}",
            put(update_column_meta_handler),
        )
        .route(
            "/axhub/datasets/{id}/quality-rules",
            get(list_quality_rules_handler).post(create_quality_rule_handler),
        )
        .route(
            "/axhub/quality-rules/{rule_id}",
            delete(delete_quality_rule_handler),
        )
        .route("/axhub/datasets/{id}/lineage", get(get_lineage_handler))
        .route("/axhub/uploads", post(upload_file_handler))
        .route("/axhub/uploads/{id}", get(get_upload_handler))
        .route(
            "/axhub/webhooks",
            get(list_webhooks_handler).post(create_webhook_handler),
        )
        .route("/axhub/webhooks/{id}", delete(delete_webhook_handler))
        .route(
            "/axhub/webhooks/{tenant_id}/{slug}/ingest",
            post(ingest_webhook_handler),
        )
        .route("/axhub/catalog/search", get(search_catalog_handler))
        .route(
            "/axhub/glossary",
            get(list_glossary_handler).post(create_glossary_handler),
        )
        .route("/axhub/glossary/{id}", delete(delete_glossary_handler))
}

async fn list_connector_types_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
) -> Result<Json<Vec<ConnectorSpecDto>>, HttpError> {
    principal.require(Permission::DataRead)?;
    let specs = state
        .sidecar
        .list_connector_types(&principal.tenant_id)
        .await?;
    Ok(Json(specs))
}

async fn list_connections_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
) -> Result<Json<PaginatedList<Connection>>, HttpError> {
    principal.require(Permission::DataRead)?;
    let pairs = state.store.list_connections(&principal.tenant_id).await?;
    let items = pairs.into_iter().map(|(c, _)| c).collect();
    Ok(Json(PaginatedList {
        items,
        next_cursor: None,
    }))
}

async fn create_connection_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiJson(req): ApiJson<CreateConnectionRequest>,
) -> Result<Json<Connection>, HttpError> {
    principal.require(Permission::DataAdmin)?;
    if req.name.trim().is_empty() {
        return Err(HttpError(AppError::bad(
            "INVALID_ARGUMENT",
            "connection name cannot be empty",
        )));
    }
    let (plain_config, credential) = split_and_seal_create(
        &state.keyring,
        &principal.tenant_id,
        req.connector_type,
        req.config,
        req.secrets,
    )?;
    let created = state
        .store
        .create_connection(
            &principal.tenant_id,
            NewConnectionInput {
                created_by: Some(principal.user_id),
                name: req.name.trim().to_owned(),
                connector_type: req.connector_type,
                config: plain_config,
                allow_private_network: req.allow_private_network,
                credential,
            },
        )
        .await?;
    let _ = state
        .analytics_pool
        .invalidate_tenant(&principal.tenant_id)
        .await;
    Ok(Json(created))
}

async fn get_connection_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<Connection>, HttpError> {
    principal.require(Permission::DataRead)?;
    let (conn, _) = state
        .store
        .get_connection(&principal.tenant_id, id)
        .await?
        .ok_or(AppError::NotFound("CONNECTION_NOT_FOUND"))?;
    Ok(Json(conn))
}

async fn update_connection_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(req): ApiJson<UpdateConnectionRequest>,
) -> Result<Json<Connection>, HttpError> {
    principal.require(Permission::DataAdmin)?;
    let (existing_conn, existing_cred) = state
        .store
        .get_connection(&principal.tenant_id, id)
        .await?
        .ok_or(AppError::NotFound("CONNECTION_NOT_FOUND"))?;
    let input = split_and_seal_update(
        &state.keyring,
        &principal.tenant_id,
        &existing_conn,
        existing_cred.as_ref(),
        req,
    )?;
    let updated = state
        .store
        .update_connection(&principal.tenant_id, id, input)
        .await?
        .ok_or(AppError::NotFound("CONNECTION_NOT_FOUND"))?;
    let _ = state
        .analytics_pool
        .invalidate_tenant(&principal.tenant_id)
        .await;
    Ok(Json(updated))
}

async fn delete_connection_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<serde_json::Value>, HttpError> {
    principal.require(Permission::DataAdmin)?;
    let deleted = state
        .store
        .delete_connection(&principal.tenant_id, id)
        .await?;
    if !deleted {
        return Err(HttpError(AppError::NotFound("CONNECTION_NOT_FOUND")));
    }
    let _ = state
        .analytics_pool
        .invalidate_tenant(&principal.tenant_id)
        .await;
    Ok(Json(serde_json::json!({ "deleted": true })))
}

async fn test_connection_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<ConnectionTestResult>, HttpError> {
    principal.require(Permission::DataAdmin)?;
    let (conn, sealed) = state
        .store
        .get_connection(&principal.tenant_id, id)
        .await?
        .ok_or(AppError::NotFound("CONNECTION_NOT_FOUND"))?;
    let secrets = open_secret_map(&state.keyring, &principal.tenant_id, sealed.as_ref())?;
    let sidecar_req = SidecarConnectorRequest {
        connector_type: conn.connector_type,
        config: conn.config,
        secrets,
        allow_private_network: conn.allow_private_network,
    };

    match state
        .sidecar
        .test_connector(&principal.tenant_id, &sidecar_req)
        .await
    {
        Ok(res) => {
            let st = if res.ok {
                ConnectionStatus::Ok
            } else {
                ConnectionStatus::Failed
            };
            state
                .store
                .update_test_status(&principal.tenant_id, id, st, &res.message)
                .await?;
            Ok(Json(res))
        }
        Err(err) => {
            let _ = state
                .store
                .update_test_status(
                    &principal.tenant_id,
                    id,
                    ConnectionStatus::Failed,
                    &err.client_message(),
                )
                .await;
            Err(HttpError(err))
        }
    }
}

async fn discover_connection_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<Vec<DiscoveredObject>>, HttpError> {
    principal.require(Permission::DataAdmin)?;
    let (conn, sealed) = state
        .store
        .get_connection(&principal.tenant_id, id)
        .await?
        .ok_or(AppError::NotFound("CONNECTION_NOT_FOUND"))?;
    let secrets = open_secret_map(&state.keyring, &principal.tenant_id, sealed.as_ref())?;
    let sidecar_req = SidecarConnectorRequest {
        connector_type: conn.connector_type,
        config: conn.config,
        secrets,
        allow_private_network: conn.allow_private_network,
    };
    let objects = state
        .sidecar
        .discover_connector(&principal.tenant_id, &sidecar_req)
        .await?;
    Ok(Json(objects))
}

async fn preview_connection_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(req): ApiJson<ConnectionPreviewRequest>,
) -> Result<Json<PreviewResponse>, HttpError> {
    principal.require(Permission::DataRead)?;
    if !(1..=1_000).contains(&req.limit) {
        return Err(HttpError(AppError::bad(
            "LIMIT_OUT_OF_RANGE",
            format!("preview limit must be in 1..=1000, got {}", req.limit),
        )));
    }
    let (conn, sealed) = state
        .store
        .get_connection(&principal.tenant_id, id)
        .await?
        .ok_or(AppError::NotFound("CONNECTION_NOT_FOUND"))?;
    let secrets = open_secret_map(&state.keyring, &principal.tenant_id, sealed.as_ref())?;
    let sidecar_req = SidecarPreviewRequest {
        connector_type: conn.connector_type,
        config: conn.config,
        secrets,
        allow_private_network: conn.allow_private_network,
        source_ref: req.source_ref,
        limit: req.limit,
    };
    let resp = state
        .sidecar
        .preview_connector(&principal.tenant_id, &sidecar_req)
        .await?;
    Ok(Json(resp))
}

async fn rotate_credentials_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
) -> Result<(StatusCode, Json<serde_json::Value>), HttpError> {
    principal.require(Permission::DataAdmin)?;
    let job_id = state
        .store
        .enqueue(
            &principal.tenant_id,
            NewJob::new(
                JobKind::CredentialRotate,
                serde_json::json!({
                    "target_key_version": state.keyring.active_version_i32(),
                }),
            ),
        )
        .await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(serde_json::json!({ "job_id": job_id })),
    ))
}

async fn list_datasets_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiQuery(query): ApiQuery<DatasetListQuery>,
) -> Result<Json<PaginatedList<Dataset>>, HttpError> {
    principal.require(Permission::DataRead)?;
    let list = state
        .store
        .list_datasets(&principal.tenant_id, query)
        .await?;
    Ok(Json(list))
}

async fn create_dataset_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiJson(req): ApiJson<CreateDatasetRequest>,
) -> Result<Json<Dataset>, HttpError> {
    principal.require(Permission::DataSync)?;
    if req.sync_mode == SyncMode::Upsert && req.primary_key.is_empty() {
        return Err(HttpError(AppError::bad(
            "INVALID_ARGUMENT",
            "UPSERT sync_mode requires non-empty primary_key",
        )));
    }
    if let Some(ref cron_expr) = req.schedule_cron {
        to_six_field_cron(cron_expr)?;
    }
    let conn_id = req.connection_id;
    let ds = state
        .store
        .create_dataset(&principal.tenant_id, Some(principal.user_id), req)
        .await?;
    if let Some(cid) = conn_id {
        let _ = state
            .store
            .upsert_lineage(
                &principal.tenant_id,
                "CONNECTION",
                &cid.to_string(),
                "DATASET",
                &ds.id.to_string(),
                "SYNC",
            )
            .await;
    }
    Ok(Json(ds))
}

async fn get_dataset_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<Dataset>, HttpError> {
    principal.require(Permission::DataRead)?;
    let ds = state
        .store
        .get_dataset(&principal.tenant_id, id)
        .await?
        .ok_or(AppError::NotFound("DATASET_NOT_FOUND"))?;
    Ok(Json(ds))
}

async fn update_dataset_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(req): ApiJson<UpdateDatasetRequest>,
) -> Result<Json<Dataset>, HttpError> {
    principal.require(Permission::DataSync)?;
    if let Some(Some(ref cron_expr)) = req.schedule_cron {
        to_six_field_cron(cron_expr)?;
    }
    let ds = state
        .store
        .update_dataset(&principal.tenant_id, id, req)
        .await?
        .ok_or(AppError::NotFound("DATASET_NOT_FOUND"))?;
    Ok(Json(ds))
}

async fn delete_dataset_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<serde_json::Value>, HttpError> {
    principal.require(Permission::DataAdmin)?;
    let deleted = state
        .store
        .delete_dataset(&principal.tenant_id, id)
        .await?
        .ok_or(AppError::NotFound("DATASET_NOT_FOUND"))?;
    let _ = state
        .sidecar
        .drop_lake_table(&principal.tenant_id, &deleted.lake_table)
        .await;
    crate::routes::analytics::refresh_cached_tenant_catalog(&state, &principal.tenant_id).await;
    Ok(Json(serde_json::json!({ "deleted": true })))
}

async fn sync_dataset_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<(StatusCode, Json<SyncDatasetResponse>), HttpError> {
    principal.require(Permission::DataSync)?;
    let res = state
        .store
        .enqueue_sync_job(&principal.tenant_id, id)
        .await?;
    let status = if res.existing {
        StatusCode::OK
    } else {
        StatusCode::ACCEPTED
    };
    Ok((status, Json(res)))
}

async fn list_sync_jobs_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
    ApiQuery(query): ApiQuery<SyncJobsQuery>,
) -> Result<Json<PaginatedList<SyncJob>>, HttpError> {
    principal.require(Permission::DataRead)?;
    let limit = query.limit.unwrap_or(20);
    if !(1..=200).contains(&limit) {
        return Err(HttpError(AppError::bad(
            "LIMIT_OUT_OF_RANGE",
            format!("limit must be in 1..=200, got {limit}"),
        )));
    }
    let list = state
        .store
        .list_sync_jobs(&principal.tenant_id, id, limit, query.cursor.as_deref())
        .await?;
    Ok(Json(list))
}

async fn preview_dataset_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
    ApiQuery(query): ApiQuery<DatasetPreviewQuery>,
) -> Result<Json<PreviewResponse>, HttpError> {
    principal.require(Permission::DataRead)?;
    let limit = query.limit.unwrap_or(100);
    if !(1..=1_000).contains(&limit) {
        return Err(HttpError(AppError::bad(
            "LIMIT_OUT_OF_RANGE",
            format!("limit must be in 1..=1000, got {limit}"),
        )));
    }
    let ds = state
        .store
        .get_dataset(&principal.tenant_id, id)
        .await?
        .ok_or(AppError::NotFound("DATASET_NOT_FOUND"))?;
    if ds.last_snapshot_id.is_none() && ds.row_count == 0 {
        return Ok(Json(PreviewResponse {
            columns: ds.schema_json,
            rows: Vec::new(),
        }));
    }
    let resp = state
        .sidecar
        .preview_lake(
            &principal.tenant_id,
            &LakePreviewRequest {
                tenant_id: principal.tenant_id.clone(),
                lake_table: ds.lake_table,
                limit,
                snapshot_id: query.snapshot_id,
            },
        )
        .await?;
    Ok(Json(resp))
}

async fn list_snapshots_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<Vec<LakeSnapshotInfo>>, HttpError> {
    principal.require(Permission::DataRead)?;
    let ds = state
        .store
        .get_dataset(&principal.tenant_id, id)
        .await?
        .ok_or(AppError::NotFound("DATASET_NOT_FOUND"))?;
    let snaps = state
        .sidecar
        .list_snapshots(&principal.tenant_id, &ds.lake_table)
        .await?;
    Ok(Json(snaps))
}

async fn list_columns_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<Vec<ColumnMeta>>, HttpError> {
    principal.require(Permission::DataRead)?;
    let cols = state.store.list_columns(&principal.tenant_id, id).await?;
    Ok(Json(cols))
}

async fn update_column_meta_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath((id, column)): ApiPath<(Uuid, String)>,
    ApiJson(req): ApiJson<UpdateColumnMetaRequest>,
) -> Result<Json<ColumnMeta>, HttpError> {
    principal.require(Permission::DataAdmin)?;
    let updated = state
        .store
        .update_column_meta(&principal.tenant_id, id, &column, req)
        .await?
        .ok_or(AppError::NotFound("COLUMN_NOT_FOUND"))?;
    crate::routes::analytics::refresh_cached_tenant_catalog(&state, &principal.tenant_id).await;
    let _ = axpoint_domain::ports::JobQueuePort::enqueue(
        &state.store,
        &principal.tenant_id,
        axpoint_domain::platform::NewJob::new(
            axpoint_domain::enums::JobKind::EmbedNodes,
            serde_json::json!({}),
        ),
    )
    .await;
    Ok(Json(updated))
}

async fn list_quality_rules_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<Vec<QualityRule>>, HttpError> {
    principal.require(Permission::DataRead)?;
    let rules = state
        .store
        .list_quality_rules(&principal.tenant_id, id)
        .await?;
    Ok(Json(rules))
}

async fn create_quality_rule_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(req): ApiJson<CreateQualityRuleRequest>,
) -> Result<Json<QualityRule>, HttpError> {
    principal.require(Permission::DataAdmin)?;
    let rule = state
        .store
        .create_quality_rule(&principal.tenant_id, id, req)
        .await?;
    Ok(Json(rule))
}

async fn delete_quality_rule_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(rule_id): ApiPath<Uuid>,
) -> Result<Json<serde_json::Value>, HttpError> {
    principal.require(Permission::DataAdmin)?;
    let deleted = state
        .store
        .delete_quality_rule(&principal.tenant_id, rule_id)
        .await?;
    if !deleted {
        return Err(HttpError(AppError::NotFound("QUALITY_RULE_NOT_FOUND")));
    }
    Ok(Json(serde_json::json!({ "deleted": true })))
}

async fn get_lineage_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
    ApiQuery(query): ApiQuery<LineageQuery>,
) -> Result<Json<serde_json::Value>, HttpError> {
    principal.require(Permission::DataRead)?;
    let depth = query.depth.unwrap_or(3).clamp(1, 10);
    let edges: Vec<LineageEdge> = state
        .store
        .get_dataset_lineage(&principal.tenant_id, id, depth)
        .await?;
    Ok(Json(serde_json::json!({ "edges": edges })))
}

fn parse_bool_field(raw: &str, default_val: bool) -> bool {
    match raw.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" => true,
        "false" | "0" | "no" => false,
        _ => default_val,
    }
}

struct ParsedMultipartUpload {
    file_name: String,
    raw_bytes: Vec<u8>,
    to_knowledge: bool,
    to_datasets: bool,
    dataset_prefix: Option<String>,
    options: BTreeMap<String, String>,
}

async fn read_upload_multipart(
    mut multipart: Multipart,
) -> Result<ParsedMultipartUpload, HttpError> {
    let mut file_name = String::from("upload.bin");
    let mut file_bytes: Option<Vec<u8>> = None;
    let mut to_knowledge = true;
    let mut to_datasets = true;
    let mut dataset_prefix: Option<String> = None;
    let mut options: BTreeMap<String, String> = BTreeMap::new();

    while let Some(field) = multipart.next_field().await.map_err(|err| {
        HttpError(AppError::bad(
            "INVALID_ARGUMENT",
            format!("multipart error: {err}"),
        ))
    })? {
        let name = field.name().unwrap_or_default().to_owned();
        if name == "tenant_id" {
            return Err(HttpError(AppError::bad(
                "INVALID_ARGUMENT",
                "tenant_id parameter is forbidden",
            )));
        }
        if name == "file" {
            if let Some(orig) = field.file_name() {
                let clean_name = orig.rsplit(['/', '\\']).next().unwrap_or(orig).trim();
                if !clean_name.is_empty() {
                    file_name = clean_name.to_owned();
                }
            }
            let bytes = field.bytes().await.map_err(|err| {
                HttpError(AppError::bad(
                    "INVALID_ARGUMENT",
                    format!("read file failed: {err}"),
                ))
            })?;
            file_bytes = Some(bytes.to_vec());
            continue;
        }
        let text_val = field.text().await.map_err(|err| {
            HttpError(AppError::bad(
                "INVALID_ARGUMENT",
                format!("invalid field '{name}': {err}"),
            ))
        })?;
        match name.as_str() {
            "to_knowledge" => to_knowledge = parse_bool_field(&text_val, true),
            "to_datasets" => to_datasets = parse_bool_field(&text_val, true),
            "dataset_prefix" => {
                let trimmed = text_val.trim();
                if !trimmed.is_empty() {
                    dataset_prefix = Some(sanitize_dataset_prefix(trimmed));
                }
            }
            "log_format" | "json_explode" | "ocr" | "record_tag" if !text_val.trim().is_empty() => {
                options.insert(name, text_val.trim().to_owned());
            }
            _ => {}
        }
    }

    let raw_bytes = file_bytes
        .ok_or_else(|| HttpError(AppError::bad("INVALID_ARGUMENT", "missing 'file' field")))?;
    if raw_bytes.is_empty() {
        return Err(HttpError(AppError::bad(
            "INVALID_ARGUMENT",
            "uploaded file cannot be empty",
        )));
    }

    Ok(ParsedMultipartUpload {
        file_name,
        raw_bytes,
        to_knowledge,
        to_datasets,
        dataset_prefix,
        options,
    })
}

async fn upload_file_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    multipart: Multipart,
) -> Result<(StatusCode, Json<EnqueueUploadResponse>), HttpError> {
    principal.require(Permission::DataSync)?;
    let ParsedMultipartUpload {
        file_name,
        raw_bytes,
        to_knowledge,
        to_datasets,
        dataset_prefix,
        options,
    } = read_upload_multipart(multipart).await?;

    let sha256 = hex::encode(Sha256::digest(&raw_bytes));
    if let Some(existing) = state
        .store
        .find_committed_upload_by_sha(&principal.tenant_id, &sha256)
        .await?
    {
        return Ok((
            StatusCode::OK,
            Json(EnqueueUploadResponse {
                upload_id: existing.id,
                job_id: None,
                deduplicated: true,
            }),
        ));
    }

    let upload_id = Uuid::new_v4();
    let upload_dir = state
        .data_root
        .join("tenants")
        .join(&principal.tenant_id)
        .join("uploads")
        .join(upload_id.to_string());
    tokio::fs::create_dir_all(&upload_dir)
        .await
        .map_err(|err| {
            HttpError(AppError::Config(format!(
                "failed to create upload directory: {err}"
            )))
        })?;
    let blob_path = upload_dir.join(&sha256);
    tokio::fs::write(&blob_path, &raw_bytes)
        .await
        .map_err(|err| HttpError(AppError::Config(format!("failed to persist upload: {err}"))))?;

    let size_bytes = i64::try_from(raw_bytes.len()).unwrap_or(i64::MAX);
    state
        .store
        .create_upload(
            &principal.tenant_id,
            NewUploadInput {
                id: upload_id,
                created_by: Some(principal.user_id),
                filename: file_name.clone(),
                sha256: sha256.clone(),
                size_bytes,
            },
        )
        .await?;

    let prefix = dataset_prefix.unwrap_or_else(|| sanitize_dataset_prefix(&file_name));
    let job_id = state
        .store
        .enqueue(
            &principal.tenant_id,
            NewJob::new(
                JobKind::ParseFile,
                serde_json::json!({
                    "upload_id": upload_id,
                    "filename": file_name,
                    "sha256": sha256,
                    "to_knowledge": to_knowledge,
                    "to_datasets": to_datasets,
                    "dataset_prefix": prefix,
                    "options": options,
                    "owner_user_id": principal.user_id,
                }),
            )
            .with_idempotency_key(format!("upload:{upload_id}")),
        )
        .await?;

    Ok((
        StatusCode::ACCEPTED,
        Json(EnqueueUploadResponse {
            upload_id,
            job_id: Some(job_id),
            deduplicated: false,
        }),
    ))
}

async fn get_upload_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<UploadRecord>, HttpError> {
    principal.require(Permission::DataRead)?;
    let rec = state
        .store
        .get_upload(&principal.tenant_id, id)
        .await?
        .ok_or(AppError::NotFound("UPLOAD_NOT_FOUND"))?;
    Ok(Json(rec))
}

async fn list_webhooks_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
) -> Result<Json<PaginatedList<WebhookConfig>>, HttpError> {
    principal.require(Permission::DataAdmin)?;
    let items = state.store.list_webhooks(&principal.tenant_id).await?;
    Ok(Json(PaginatedList {
        items,
        next_cursor: None,
    }))
}

async fn create_webhook_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiJson(req): ApiJson<CreateWebhookRequest>,
) -> Result<Json<CreateWebhookResponse>, HttpError> {
    principal.require(Permission::DataAdmin)?;
    let secret_hex = generate_webhook_secret_hex();
    let mut secret_map = BTreeMap::new();
    secret_map.insert(String::from("secret"), secret_hex.clone());
    let cred_id = Uuid::new_v4();
    let credential = seal_secret_map(&state.keyring, &principal.tenant_id, cred_id, &secret_map)?;

    let (webhook, ds) = state
        .store
        .create_webhook(
            &principal.tenant_id,
            Some(principal.user_id),
            &req,
            credential,
        )
        .await?;
    let url = format!(
        "/api/axhub/webhooks/{}/{}/ingest",
        principal.tenant_id, webhook.slug
    );

    Ok(Json(CreateWebhookResponse {
        webhook,
        dataset_id: ds.id,
        url,
        secret: secret_hex,
    }))
}

async fn delete_webhook_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<serde_json::Value>, HttpError> {
    principal.require(Permission::DataAdmin)?;
    let deleted = state.store.delete_webhook(&principal.tenant_id, id).await?;
    if !deleted {
        return Err(HttpError(AppError::NotFound("WEBHOOK_NOT_FOUND")));
    }
    Ok(Json(serde_json::json!({ "deleted": true })))
}

fn verify_webhook_auth(
    scheme: WebhookAuthScheme,
    headers: &HeaderMap,
    secret: &str,
    body: &[u8],
) -> Result<(), AppError> {
    match scheme {
        WebhookAuthScheme::HmacSha256 => {
            let ts_str = headers
                .get("x-axpoint-timestamp")
                .and_then(|v| v.to_str().ok())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or(AppError::Unauthorized("SIGNATURE_INVALID"))?;
            let ts: i64 = ts_str
                .parse()
                .map_err(|_| AppError::Unauthorized("SIGNATURE_INVALID"))?;
            let now_ts = Utc::now().timestamp();
            if (now_ts - ts).abs() > WEBHOOK_TIMESTAMP_SKEW_SECS {
                return Err(AppError::Unauthorized("SIGNATURE_INVALID"));
            }

            let sig_header = headers
                .get("x-axpoint-signature")
                .and_then(|v| v.to_str().ok())
                .map(str::trim)
                .ok_or(AppError::Unauthorized("SIGNATURE_INVALID"))?;
            let hex_sig = sig_header
                .strip_prefix("sha256=")
                .unwrap_or(sig_header)
                .trim();
            let provided =
                hex::decode(hex_sig).map_err(|_| AppError::Unauthorized("SIGNATURE_INVALID"))?;

            let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
                .map_err(|_| AppError::Unauthorized("SIGNATURE_INVALID"))?;
            mac.update(ts_str.as_bytes());
            mac.update(b".");
            mac.update(body);
            let expected = mac.finalize().into_bytes();

            if !bool::from(expected.as_slice().ct_eq(provided.as_slice())) {
                return Err(AppError::Unauthorized("SIGNATURE_INVALID"));
            }
            Ok(())
        }
        WebhookAuthScheme::Bearer => {
            let auth_header = headers
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .map(str::trim)
                .ok_or(AppError::Unauthorized("SIGNATURE_INVALID"))?;
            let token = auth_header
                .strip_prefix("Bearer ")
                .or_else(|| auth_header.strip_prefix("bearer "))
                .map(str::trim)
                .ok_or(AppError::Unauthorized("SIGNATURE_INVALID"))?;

            if !bool::from(token.as_bytes().ct_eq(secret.as_bytes())) {
                return Err(AppError::Unauthorized("SIGNATURE_INVALID"));
            }
            Ok(())
        }
    }
}

async fn ingest_webhook_handler(
    State(state): State<AppState>,
    ApiPath((tenant_id, slug)): ApiPath<(String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, HttpError> {
    check_webhook_rate_limit(&tenant_id, &slug)?;

    let resolved = state
        .store
        .resolve_webhook(&tenant_id, &slug)
        .await?
        .ok_or(AppError::NotFound("WEBHOOK_NOT_FOUND"))?;

    let max_bytes = usize::try_from(resolved.max_body_bytes.max(1)).unwrap_or(1_048_576);
    if body.len() > max_bytes {
        return Err(HttpError(AppError::PayloadTooLarge("PAYLOAD_TOO_LARGE")));
    }

    let sealed = state
        .store
        .get_credential(&tenant_id, resolved.credential_id)
        .await?
        .ok_or(AppError::Unauthorized("SIGNATURE_INVALID"))?;
    let secret_map = open_secret_map(&state.keyring, &tenant_id, Some(&sealed))?;
    let secret = secret_map
        .get("secret")
        .ok_or(AppError::Unauthorized("SIGNATURE_INVALID"))?;

    verify_webhook_auth(resolved.auth_scheme, &headers, secret, &body)?;

    let payload: serde_json::Value = serde_json::from_slice(&body).map_err(|err| {
        HttpError(AppError::bad(
            "INVALID_ARGUMENT",
            format!("webhook body must be valid JSON: {err}"),
        ))
    })?;

    let delivery_id = headers
        .get("x-axpoint-delivery")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map_or_else(|| hex::encode(Sha256::digest(&body)), String::from);

    let inserted = state
        .store
        .ingest_webhook_event(&tenant_id, resolved.webhook_id, &delivery_id, payload)
        .await?;

    if inserted {
        Ok((
            StatusCode::ACCEPTED,
            Json(serde_json::json!({ "accepted": true })),
        )
            .into_response())
    } else {
        Ok((
            StatusCode::OK,
            Json(serde_json::json!({ "accepted": true, "duplicate": true })),
        )
            .into_response())
    }
}

async fn search_catalog_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiQuery(query): ApiQuery<CatalogSearchQuery>,
) -> Result<Json<PaginatedList<CatalogSearchHit>>, HttpError> {
    principal.require(Permission::DataRead)?;
    let limit = query.limit.unwrap_or(50);
    if !(1..=200).contains(&limit) {
        return Err(HttpError(AppError::bad(
            "LIMIT_OUT_OF_RANGE",
            format!("limit must be in 1..=200, got {limit}"),
        )));
    }
    let q = query.q.unwrap_or_default();
    let res = state
        .store
        .search_catalog(&principal.tenant_id, &q, limit)
        .await?;
    Ok(Json(res))
}

async fn list_glossary_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
) -> Result<Json<PaginatedList<GlossaryTerm>>, HttpError> {
    principal.require(Permission::DataRead)?;
    let items = state.store.list_glossary(&principal.tenant_id).await?;
    Ok(Json(PaginatedList {
        items,
        next_cursor: None,
    }))
}

async fn create_glossary_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiJson(req): ApiJson<CreateGlossaryTermRequest>,
) -> Result<Json<GlossaryTerm>, HttpError> {
    principal.require(Permission::DataAdmin)?;
    let term = state
        .store
        .create_glossary_term(&principal.tenant_id, req)
        .await?;
    Ok(Json(term))
}

async fn delete_glossary_handler(
    State(state): State<AppState>,
    principal: AuthPrincipal,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<serde_json::Value>, HttpError> {
    principal.require(Permission::DataAdmin)?;
    let deleted = state
        .store
        .delete_glossary_term(&principal.tenant_id, id)
        .await?;
    if !deleted {
        return Err(HttpError(AppError::NotFound("GLOSSARY_TERM_NOT_FOUND")));
    }
    Ok(Json(serde_json::json!({ "deleted": true })))
}
