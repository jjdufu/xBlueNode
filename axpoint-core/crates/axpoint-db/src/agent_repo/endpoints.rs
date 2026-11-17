//! Persistence helpers for `ax_published_endpoints` and `ax_mcp_clients`.

use axpoint_domain::AppError;
use axpoint_domain::agent::{
    EndpointListQuery, EndpointSpec, McpClientRecord, McpToolInfo, SaveEndpointInput,
    SaveMcpClientInput, UpdateEndpointRequest,
};
use axpoint_domain::axhub::SealedCredential;
use axpoint_domain::enums::{EndpointKind, Environment, OutputShape, Permission};
use chrono::{DateTime, Utc};
use sqlx::Row;
use sqlx::postgres::PgRow;
use uuid::Uuid;

use super::providers::{
    delete_orphan_credential_tx, extract_joined_credential, upsert_sealed_credential_tx,
};
use crate::PgStore;
use crate::tenant::begin_tenant_tx;

fn map_pg_constraint_err(context: &str, err: &sqlx::Error) -> AppError {
    if let Some(db_err) = err.as_database_error() {
        match db_err.code().as_deref() {
            Some("23505") => return AppError::Conflict("NAME_TAKEN"),
            Some("23514") => {
                return AppError::bad(
                    "INVALID_ARGUMENT",
                    format!("{context} violated constraint: {}", db_err.message()),
                );
            }
            _ => {}
        }
    }
    AppError::Database(format!("{context} failed: {err}"))
}

fn decode_endpoint_row(row: &PgRow) -> Result<EndpointSpec, AppError> {
    let id: Uuid = row
        .try_get("id")
        .map_err(|e| AppError::Database(format!("decode ax_published_endpoints.id: {e}")))?;
    let tenant_id: String = row
        .try_get("tenant_id")
        .map_err(|e| AppError::Database(format!("decode ax_published_endpoints.tenant_id: {e}")))?;
    let slug: String = row
        .try_get("slug")
        .map_err(|e| AppError::Database(format!("decode ax_published_endpoints.slug: {e}")))?;
    let raw_env: String = row.try_get("environment").map_err(|e| {
        AppError::Database(format!("decode ax_published_endpoints.environment: {e}"))
    })?;
    let environment: Environment = raw_env.parse()?;
    let raw_kind: String = row
        .try_get("kind")
        .map_err(|e| AppError::Database(format!("decode ax_published_endpoints.kind: {e}")))?;
    let kind: EndpointKind = raw_kind.parse()?;
    let target_id: String = row
        .try_get("target_id")
        .map_err(|e| AppError::Database(format!("decode ax_published_endpoints.target_id: {e}")))?;
    let target_version: Option<i32> = row.try_get("target_version").map_err(|e| {
        AppError::Database(format!("decode ax_published_endpoints.target_version: {e}"))
    })?;
    let target_snapshot: Option<serde_json::Value> =
        row.try_get("target_snapshot").map_err(|e| {
            AppError::Database(format!(
                "decode ax_published_endpoints.target_snapshot: {e}"
            ))
        })?;
    let title: String = row
        .try_get("title")
        .map_err(|e| AppError::Database(format!("decode ax_published_endpoints.title: {e}")))?;
    let description: String = row.try_get("description").map_err(|e| {
        AppError::Database(format!("decode ax_published_endpoints.description: {e}"))
    })?;
    let input_schema: serde_json::Value = row.try_get("input_schema").map_err(|e| {
        AppError::Database(format!("decode ax_published_endpoints.input_schema: {e}"))
    })?;
    let raw_shape: String = row.try_get("output_shape").map_err(|e| {
        AppError::Database(format!("decode ax_published_endpoints.output_shape: {e}"))
    })?;
    let output_shape: OutputShape = raw_shape.parse()?;
    let raw_scope: String = row.try_get("required_scope").map_err(|e| {
        AppError::Database(format!("decode ax_published_endpoints.required_scope: {e}"))
    })?;
    let required_scope: Permission = raw_scope.parse()?;
    let rate_limit_rpm: i32 = row.try_get("rate_limit_rpm").map_err(|e| {
        AppError::Database(format!("decode ax_published_endpoints.rate_limit_rpm: {e}"))
    })?;
    let cache_ttl_s: i32 = row.try_get("cache_ttl_s").map_err(|e| {
        AppError::Database(format!("decode ax_published_endpoints.cache_ttl_s: {e}"))
    })?;
    let expose_mcp: bool = row.try_get("expose_mcp").map_err(|e| {
        AppError::Database(format!("decode ax_published_endpoints.expose_mcp: {e}"))
    })?;
    let show_as_app: bool = row.try_get("show_as_app").map_err(|e| {
        AppError::Database(format!("decode ax_published_endpoints.show_as_app: {e}"))
    })?;
    let enabled: bool = row
        .try_get("enabled")
        .map_err(|e| AppError::Database(format!("decode ax_published_endpoints.enabled: {e}")))?;
    let published_by: Option<Uuid> = row.try_get("published_by").map_err(|e| {
        AppError::Database(format!("decode ax_published_endpoints.published_by: {e}"))
    })?;
    let published_at: DateTime<Utc> = row.try_get("published_at").map_err(|e| {
        AppError::Database(format!("decode ax_published_endpoints.published_at: {e}"))
    })?;

    Ok(EndpointSpec {
        id,
        tenant_id,
        slug,
        environment,
        kind,
        target_id,
        target_version,
        target_snapshot,
        title,
        description,
        input_schema,
        output_shape,
        required_scope,
        rate_limit_rpm,
        cache_ttl_s,
        expose_mcp,
        show_as_app,
        enabled,
        published_by,
        published_at,
    })
}

fn decode_mcp_client_row(row: &PgRow) -> Result<McpClientRecord, AppError> {
    let id: Uuid = row
        .try_get("id")
        .map_err(|e| AppError::Database(format!("decode ax_mcp_clients.id: {e}")))?;
    let tenant_id: String = row
        .try_get("tenant_id")
        .map_err(|e| AppError::Database(format!("decode ax_mcp_clients.tenant_id: {e}")))?;
    let name: String = row
        .try_get("name")
        .map_err(|e| AppError::Database(format!("decode ax_mcp_clients.name: {e}")))?;
    let url: String = row
        .try_get("url")
        .map_err(|e| AppError::Database(format!("decode ax_mcp_clients.url: {e}")))?;
    let credential_id: Option<Uuid> = row
        .try_get("credential_id")
        .map_err(|e| AppError::Database(format!("decode ax_mcp_clients.credential_id: {e}")))?;
    let allowed_tools: Vec<String> = row
        .try_get("allowed_tools")
        .map_err(|e| AppError::Database(format!("decode ax_mcp_clients.allowed_tools: {e}")))?;
    let raw_cache: serde_json::Value = row
        .try_get("tools_cache")
        .map_err(|e| AppError::Database(format!("decode ax_mcp_clients.tools_cache: {e}")))?;
    let tools_cache: Vec<McpToolInfo> = serde_json::from_value(raw_cache)
        .map_err(|e| AppError::Database(format!("parse ax_mcp_clients.tools_cache JSON: {e}")))?;
    let allow_private_network: bool = row.try_get("allow_private_network").map_err(|e| {
        AppError::Database(format!("decode ax_mcp_clients.allow_private_network: {e}"))
    })?;
    let enabled: bool = row
        .try_get("enabled")
        .map_err(|e| AppError::Database(format!("decode ax_mcp_clients.enabled: {e}")))?;
    let last_synced_at: Option<DateTime<Utc>> = row
        .try_get("last_synced_at")
        .map_err(|e| AppError::Database(format!("decode ax_mcp_clients.last_synced_at: {e}")))?;
    let created_at: DateTime<Utc> = row
        .try_get("created_at")
        .map_err(|e| AppError::Database(format!("decode ax_mcp_clients.created_at: {e}")))?;

    Ok(McpClientRecord {
        id,
        tenant_id,
        name,
        url,
        has_headers: credential_id.is_some(),
        credential_id,
        allowed_tools,
        tools_cache,
        allow_private_network,
        enabled,
        last_synced_at,
        created_at,
    })
}

pub(super) async fn list_endpoints_impl(
    store: &PgStore,
    tenant_id: &str,
    query: EndpointListQuery,
) -> Result<Vec<EndpointSpec>, AppError> {
    let mut tx = begin_tenant_tx(&store.pool, tenant_id).await?;
    let env_str = query.environment.map(|e| String::from(e.as_str()));
    let kind_str = query.kind.map(|k| String::from(k.as_str()));

    let rows = sqlx::query(
        "SELECT * FROM ax_published_endpoints
          WHERE ($1::text IS NULL OR environment = $1)
            AND ($2::text IS NULL OR kind = $2)
            AND ($3::bool IS NULL OR show_as_app = $3)
          ORDER BY published_at DESC, slug ASC",
    )
    .bind(env_str)
    .bind(kind_str)
    .bind(query.show_as_app)
    .fetch_all(&mut *tx)
    .await
    .map_err(|e| AppError::Database(format!("list ax_published_endpoints failed: {e}")))?;

    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(decode_endpoint_row(row)?);
    }
    tx.commit()
        .await
        .map_err(|e| AppError::Database(format!("commit list_endpoints tx: {e}")))?;
    Ok(out)
}

pub(super) async fn get_endpoint_impl(
    store: &PgStore,
    tenant_id: &str,
    endpoint_id: Uuid,
) -> Result<Option<EndpointSpec>, AppError> {
    let mut tx = begin_tenant_tx(&store.pool, tenant_id).await?;
    let row_opt = sqlx::query("SELECT * FROM ax_published_endpoints WHERE id = $1")
        .bind(endpoint_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|e| AppError::Database(format!("get ax_published_endpoints failed: {e}")))?;

    let spec = match row_opt {
        Some(ref r) => Some(decode_endpoint_row(r)?),
        None => None,
    };
    tx.commit()
        .await
        .map_err(|e| AppError::Database(format!("commit get_endpoint tx: {e}")))?;
    Ok(spec)
}

pub(super) async fn get_endpoint_by_slug_impl(
    store: &PgStore,
    tenant_id: &str,
    environment: Environment,
    slug: &str,
) -> Result<Option<EndpointSpec>, AppError> {
    let mut tx = begin_tenant_tx(&store.pool, tenant_id).await?;
    let row_opt =
        sqlx::query("SELECT * FROM ax_published_endpoints WHERE environment = $1 AND slug = $2")
            .bind(environment.as_str())
            .bind(slug)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|e| {
                AppError::Database(format!("get ax_published_endpoints by slug failed: {e}"))
            })?;

    let spec = match row_opt {
        Some(ref r) => Some(decode_endpoint_row(r)?),
        None => None,
    };
    tx.commit()
        .await
        .map_err(|e| AppError::Database(format!("commit get_endpoint_by_slug tx: {e}")))?;
    Ok(spec)
}

pub(super) async fn create_endpoint_impl(
    store: &PgStore,
    tenant_id: &str,
    input: SaveEndpointInput,
) -> Result<EndpointSpec, AppError> {
    let mut tx = begin_tenant_tx(&store.pool, tenant_id).await?;
    let row = sqlx::query(
        "INSERT INTO ax_published_endpoints (
            tenant_id, slug, environment, kind, target_id, target_version, target_snapshot,
            title, description, input_schema, output_shape, required_scope,
            rate_limit_rpm, cache_ttl_s, expose_mcp, show_as_app, enabled, published_by, published_at
         )
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, NOW())
         RETURNING *",
    )
    .bind(tenant_id)
    .bind(&input.slug)
    .bind(input.environment.as_str())
    .bind(input.kind.as_str())
    .bind(&input.target_id)
    .bind(input.target_version)
    .bind(&input.target_snapshot)
    .bind(&input.title)
    .bind(&input.description)
    .bind(&input.input_schema)
    .bind(input.output_shape.as_str())
    .bind(input.required_scope.as_str())
    .bind(input.rate_limit_rpm)
    .bind(input.cache_ttl_s)
    .bind(input.expose_mcp)
    .bind(input.show_as_app)
    .bind(input.enabled)
    .bind(input.published_by)
    .fetch_one(&mut *tx)
    .await
    .map_err(|err| map_pg_constraint_err("insert ax_published_endpoints", &err))?;

    let spec = decode_endpoint_row(&row)?;
    tx.commit()
        .await
        .map_err(|e| AppError::Database(format!("commit create_endpoint tx: {e}")))?;
    Ok(spec)
}

pub(super) async fn upsert_endpoint_by_slug_impl(
    store: &PgStore,
    tenant_id: &str,
    input: SaveEndpointInput,
) -> Result<EndpointSpec, AppError> {
    let mut tx = begin_tenant_tx(&store.pool, tenant_id).await?;
    let row = sqlx::query(
        "INSERT INTO ax_published_endpoints (
            tenant_id, slug, environment, kind, target_id, target_version, target_snapshot,
            title, description, input_schema, output_shape, required_scope,
            rate_limit_rpm, cache_ttl_s, expose_mcp, show_as_app, enabled, published_by, published_at
         )
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, NOW())
         ON CONFLICT (tenant_id, environment, slug) DO UPDATE SET
            kind            = EXCLUDED.kind,
            target_id       = EXCLUDED.target_id,
            target_version  = EXCLUDED.target_version,
            target_snapshot = EXCLUDED.target_snapshot,
            title           = EXCLUDED.title,
            description     = EXCLUDED.description,
            input_schema    = EXCLUDED.input_schema,
            output_shape    = EXCLUDED.output_shape,
            required_scope  = EXCLUDED.required_scope,
            rate_limit_rpm  = EXCLUDED.rate_limit_rpm,
            cache_ttl_s     = EXCLUDED.cache_ttl_s,
            expose_mcp      = EXCLUDED.expose_mcp,
            show_as_app     = EXCLUDED.show_as_app,
            enabled         = EXCLUDED.enabled,
            published_by    = EXCLUDED.published_by,
            published_at    = NOW()
         RETURNING *",
    )
    .bind(tenant_id)
    .bind(&input.slug)
    .bind(input.environment.as_str())
    .bind(input.kind.as_str())
    .bind(&input.target_id)
    .bind(input.target_version)
    .bind(&input.target_snapshot)
    .bind(&input.title)
    .bind(&input.description)
    .bind(&input.input_schema)
    .bind(input.output_shape.as_str())
    .bind(input.required_scope.as_str())
    .bind(input.rate_limit_rpm)
    .bind(input.cache_ttl_s)
    .bind(input.expose_mcp)
    .bind(input.show_as_app)
    .bind(input.enabled)
    .bind(input.published_by)
    .fetch_one(&mut *tx)
    .await
    .map_err(|err| map_pg_constraint_err("upsert ax_published_endpoints", &err))?;

    let spec = decode_endpoint_row(&row)?;
    tx.commit()
        .await
        .map_err(|e| AppError::Database(format!("commit upsert_endpoint_by_slug tx: {e}")))?;
    Ok(spec)
}

pub(super) async fn update_endpoint_impl(
    store: &PgStore,
    tenant_id: &str,
    endpoint_id: Uuid,
    req: UpdateEndpointRequest,
) -> Result<Option<EndpointSpec>, AppError> {
    let mut tx = begin_tenant_tx(&store.pool, tenant_id).await?;
    let scope_str = req.required_scope.map(|p| String::from(p.as_str()));
    let row_opt = sqlx::query(
        "UPDATE ax_published_endpoints
            SET title          = COALESCE($2, title),
                description    = COALESCE($3, description),
                input_schema   = COALESCE($4, input_schema),
                required_scope = COALESCE($5, required_scope),
                rate_limit_rpm = COALESCE($6, rate_limit_rpm),
                cache_ttl_s    = COALESCE($7, cache_ttl_s),
                expose_mcp     = COALESCE($8, expose_mcp),
                show_as_app    = COALESCE($9, show_as_app),
                enabled        = COALESCE($10, enabled)
          WHERE id = $1
         RETURNING *",
    )
    .bind(endpoint_id)
    .bind(req.title)
    .bind(req.description)
    .bind(req.input_schema)
    .bind(scope_str)
    .bind(req.rate_limit_rpm)
    .bind(req.cache_ttl_s)
    .bind(req.expose_mcp)
    .bind(req.show_as_app)
    .bind(req.enabled)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|err| map_pg_constraint_err("update ax_published_endpoints", &err))?;

    let spec = match row_opt {
        Some(ref r) => Some(decode_endpoint_row(r)?),
        None => None,
    };
    tx.commit()
        .await
        .map_err(|e| AppError::Database(format!("commit update_endpoint tx: {e}")))?;
    Ok(spec)
}

pub(super) async fn delete_endpoint_impl(
    store: &PgStore,
    tenant_id: &str,
    endpoint_id: Uuid,
) -> Result<bool, AppError> {
    let mut tx = begin_tenant_tx(&store.pool, tenant_id).await?;
    let res = sqlx::query("DELETE FROM ax_published_endpoints WHERE id = $1")
        .bind(endpoint_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| AppError::Database(format!("delete ax_published_endpoints failed: {e}")))?;
    tx.commit()
        .await
        .map_err(|e| AppError::Database(format!("commit delete_endpoint tx: {e}")))?;
    Ok(res.rows_affected() > 0)
}

pub(super) async fn list_mcp_clients_impl(
    store: &PgStore,
    tenant_id: &str,
) -> Result<Vec<McpClientRecord>, AppError> {
    let mut tx = begin_tenant_tx(&store.pool, tenant_id).await?;
    let rows = sqlx::query("SELECT * FROM ax_mcp_clients ORDER BY name ASC")
        .fetch_all(&mut *tx)
        .await
        .map_err(|e| AppError::Database(format!("list ax_mcp_clients failed: {e}")))?;

    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(decode_mcp_client_row(row)?);
    }
    tx.commit()
        .await
        .map_err(|e| AppError::Database(format!("commit list_mcp_clients tx: {e}")))?;
    Ok(out)
}

pub(super) async fn get_mcp_client_impl(
    store: &PgStore,
    tenant_id: &str,
    client_id: Uuid,
) -> Result<Option<(McpClientRecord, Option<SealedCredential>)>, AppError> {
    let mut tx = begin_tenant_tx(&store.pool, tenant_id).await?;
    let row_opt = sqlx::query(
        "SELECT m.*,
                c.key_version AS cred_key_version,
                c.nonce       AS cred_nonce,
                c.ciphertext  AS cred_ciphertext
           FROM ax_mcp_clients m
           LEFT JOIN ds_credentials c
                  ON c.id = m.credential_id AND c.tenant_id = m.tenant_id
          WHERE m.id = $1",
    )
    .bind(client_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|e| AppError::Database(format!("get ax_mcp_clients failed: {e}")))?;

    tx.commit()
        .await
        .map_err(|e| AppError::Database(format!("commit get_mcp_client tx: {e}")))?;

    let Some(row) = row_opt else {
        return Ok(None);
    };
    let rec = decode_mcp_client_row(&row)?;
    let sealed = extract_joined_credential(&row, rec.credential_id);
    Ok(Some((rec, sealed)))
}

pub(super) async fn get_mcp_client_by_name_impl(
    store: &PgStore,
    tenant_id: &str,
    name: &str,
) -> Result<Option<(McpClientRecord, Option<SealedCredential>)>, AppError> {
    let mut tx = begin_tenant_tx(&store.pool, tenant_id).await?;
    let row_opt = sqlx::query(
        "SELECT m.*,
                c.key_version AS cred_key_version,
                c.nonce       AS cred_nonce,
                c.ciphertext  AS cred_ciphertext
           FROM ax_mcp_clients m
           LEFT JOIN ds_credentials c
                  ON c.id = m.credential_id AND c.tenant_id = m.tenant_id
          WHERE m.name = $1",
    )
    .bind(name)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|e| AppError::Database(format!("get ax_mcp_clients by name failed: {e}")))?;

    tx.commit()
        .await
        .map_err(|e| AppError::Database(format!("commit get_mcp_client_by_name tx: {e}")))?;

    let Some(row) = row_opt else {
        return Ok(None);
    };
    let rec = decode_mcp_client_row(&row)?;
    let sealed = extract_joined_credential(&row, rec.credential_id);
    Ok(Some((rec, sealed)))
}

pub(super) async fn create_mcp_client_impl(
    store: &PgStore,
    tenant_id: &str,
    input: SaveMcpClientInput,
) -> Result<McpClientRecord, AppError> {
    let mut tx = begin_tenant_tx(&store.pool, tenant_id).await?;
    let credential_id = if let Some(ref cred) = input.credential {
        upsert_sealed_credential_tx(&mut tx, tenant_id, cred).await?;
        Some(cred.id)
    } else {
        None
    };

    let row = sqlx::query(
        "INSERT INTO ax_mcp_clients
            (tenant_id, name, url, credential_id, allowed_tools, allow_private_network, enabled)
         VALUES ($1, $2, $3, $4, $5, $6, $7)
         RETURNING *",
    )
    .bind(tenant_id)
    .bind(&input.name)
    .bind(&input.url)
    .bind(credential_id)
    .bind(&input.allowed_tools)
    .bind(input.allow_private_network)
    .bind(input.enabled)
    .fetch_one(&mut *tx)
    .await
    .map_err(|err| map_pg_constraint_err("insert ax_mcp_clients", &err))?;

    let rec = decode_mcp_client_row(&row)?;
    tx.commit()
        .await
        .map_err(|e| AppError::Database(format!("commit create_mcp_client tx: {e}")))?;
    Ok(rec)
}

pub(super) async fn update_mcp_client_tools_impl(
    store: &PgStore,
    tenant_id: &str,
    client_id: Uuid,
    tools: &[McpToolInfo],
) -> Result<Option<McpClientRecord>, AppError> {
    let mut tx = begin_tenant_tx(&store.pool, tenant_id).await?;
    let cache_json = serde_json::to_value(tools)
        .map_err(|e| AppError::Config(format!("serialize MCP tools_cache failed: {e}")))?;
    let row_opt = sqlx::query(
        "UPDATE ax_mcp_clients
            SET tools_cache    = $2,
                last_synced_at = NOW()
          WHERE id = $1
         RETURNING *",
    )
    .bind(client_id)
    .bind(cache_json)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|e| AppError::Database(format!("update ax_mcp_clients tools_cache failed: {e}")))?;

    let rec = match row_opt {
        Some(ref r) => Some(decode_mcp_client_row(r)?),
        None => None,
    };
    tx.commit()
        .await
        .map_err(|e| AppError::Database(format!("commit update_mcp_client_tools tx: {e}")))?;
    Ok(rec)
}

pub(super) async fn delete_mcp_client_impl(
    store: &PgStore,
    tenant_id: &str,
    client_id: Uuid,
) -> Result<bool, AppError> {
    let mut tx = begin_tenant_tx(&store.pool, tenant_id).await?;
    let row_opt = sqlx::query("DELETE FROM ax_mcp_clients WHERE id = $1 RETURNING credential_id")
        .bind(client_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|e| AppError::Database(format!("delete ax_mcp_clients failed: {e}")))?;

    let Some(row) = row_opt else {
        tx.commit()
            .await
            .map_err(|e| AppError::Database(format!("commit delete_mcp_client tx: {e}")))?;
        return Ok(false);
    };
    let old_cred: Option<Uuid> = row.try_get("credential_id").ok().flatten();
    if let Some(cid) = old_cred {
        delete_orphan_credential_tx(&mut tx, cid).await?;
    }
    tx.commit()
        .await
        .map_err(|e| AppError::Database(format!("commit delete_mcp_client tx: {e}")))?;
    Ok(true)
}
