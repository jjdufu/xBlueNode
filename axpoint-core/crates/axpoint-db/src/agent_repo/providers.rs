//! Persistence helpers for `ax_llm_providers`, `ds_credentials` binding, and `ax_llm_usage`.

use axpoint_domain::AppError;
use axpoint_domain::agent::{
    CredentialMutation, LlmProviderFlags, LlmProviderRecord, LlmUsageDelta, LlmUsageQuery,
    LlmUsageRecord, SaveLlmProviderInput,
};
use axpoint_domain::axhub::SealedCredential;
use axpoint_domain::enums::Purpose;
use chrono::{DateTime, NaiveDate, Utc};
use sqlx::postgres::PgRow;
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

use crate::PgStore;
use crate::tenant::begin_tenant_tx;

fn is_unique_violation(err: &sqlx::Error) -> bool {
    err.as_database_error()
        .and_then(sqlx::error::DatabaseError::code)
        .as_deref()
        == Some("23505")
}

fn parse_purposes(raw: Vec<String>) -> Result<Vec<Purpose>, AppError> {
    let mut out = Vec::with_capacity(raw.len());
    for item in raw {
        let p: Purpose = item.parse()?;
        out.push(p);
    }
    Ok(out)
}

pub(super) async fn upsert_sealed_credential_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: &str,
    cred: &SealedCredential,
) -> Result<(), AppError> {
    sqlx::query(
        "INSERT INTO ds_credentials (id, tenant_id, key_version, nonce, ciphertext)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (id) DO UPDATE SET
             key_version = EXCLUDED.key_version,
             nonce       = EXCLUDED.nonce,
             ciphertext  = EXCLUDED.ciphertext,
             rotated_at  = NOW()",
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

pub(super) async fn delete_orphan_credential_tx(
    tx: &mut Transaction<'_, Postgres>,
    cred_id: Uuid,
) -> Result<(), AppError> {
    sqlx::query(
        "DELETE FROM ds_credentials c
          WHERE c.id = $1
            AND NOT EXISTS (SELECT 1 FROM ds_connections conn WHERE conn.credential_id = c.id)
            AND NOT EXISTS (SELECT 1 FROM ds_webhooks wh WHERE wh.credential_id = c.id)
            AND NOT EXISTS (SELECT 1 FROM ax_notify_channels ch WHERE ch.credential_id = c.id)
            AND NOT EXISTS (SELECT 1 FROM ax_llm_providers p WHERE p.credential_id = c.id)
            AND NOT EXISTS (SELECT 1 FROM ax_mcp_clients m WHERE m.credential_id = c.id)",
    )
    .bind(cred_id)
    .execute(&mut **tx)
    .await
    .map_err(|err| AppError::Database(format!("delete orphan ds_credentials failed: {err}")))?;
    Ok(())
}

pub(super) fn extract_joined_credential(
    row: &PgRow,
    credential_id: Option<Uuid>,
) -> Option<SealedCredential> {
    let cid = credential_id?;
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

fn decode_provider_row(row: &PgRow) -> Result<LlmProviderRecord, AppError> {
    let id: Uuid = row
        .try_get("id")
        .map_err(|e| AppError::Database(format!("decode ax_llm_providers.id: {e}")))?;
    let tenant_id: String = row
        .try_get("tenant_id")
        .map_err(|e| AppError::Database(format!("decode ax_llm_providers.tenant_id: {e}")))?;
    let name: String = row
        .try_get("name")
        .map_err(|e| AppError::Database(format!("decode ax_llm_providers.name: {e}")))?;
    let base_url: String = row
        .try_get("base_url")
        .map_err(|e| AppError::Database(format!("decode ax_llm_providers.base_url: {e}")))?;
    let model: String = row
        .try_get("model")
        .map_err(|e| AppError::Database(format!("decode ax_llm_providers.model: {e}")))?;
    let raw_purposes: Vec<String> = row
        .try_get("purposes")
        .map_err(|e| AppError::Database(format!("decode ax_llm_providers.purposes: {e}")))?;
    let purposes = parse_purposes(raw_purposes)?;
    let credential_id: Option<Uuid> = row
        .try_get("credential_id")
        .map_err(|e| AppError::Database(format!("decode ax_llm_providers.credential_id: {e}")))?;
    let priority: i32 = row
        .try_get("priority")
        .map_err(|e| AppError::Database(format!("decode ax_llm_providers.priority: {e}")))?;
    let temperature: f32 = row
        .try_get("temperature")
        .map_err(|e| AppError::Database(format!("decode ax_llm_providers.temperature: {e}")))?;
    let max_tokens: i32 = row
        .try_get("max_tokens")
        .map_err(|e| AppError::Database(format!("decode ax_llm_providers.max_tokens: {e}")))?;
    let timeout_s: i32 = row
        .try_get("timeout_s")
        .map_err(|e| AppError::Database(format!("decode ax_llm_providers.timeout_s: {e}")))?;
    let rpm_limit: Option<i32> = row
        .try_get("rpm_limit")
        .map_err(|e| AppError::Database(format!("decode ax_llm_providers.rpm_limit: {e}")))?;
    let supports_tools: bool = row
        .try_get("supports_tools")
        .map_err(|e| AppError::Database(format!("decode ax_llm_providers.supports_tools: {e}")))?;
    let embedding_dim: Option<i32> = row
        .try_get("embedding_dim")
        .map_err(|e| AppError::Database(format!("decode ax_llm_providers.embedding_dim: {e}")))?;
    let allow_private_network: bool = row.try_get("allow_private_network").map_err(|e| {
        AppError::Database(format!(
            "decode ax_llm_providers.allow_private_network: {e}"
        ))
    })?;
    let allow_pii: bool = row
        .try_get("allow_pii")
        .map_err(|e| AppError::Database(format!("decode ax_llm_providers.allow_pii: {e}")))?;
    let enabled: bool = row
        .try_get("enabled")
        .map_err(|e| AppError::Database(format!("decode ax_llm_providers.enabled: {e}")))?;
    let created_at: DateTime<Utc> = row
        .try_get("created_at")
        .map_err(|e| AppError::Database(format!("decode ax_llm_providers.created_at: {e}")))?;

    Ok(LlmProviderRecord {
        id,
        tenant_id,
        name,
        base_url,
        model,
        purposes,
        has_api_key: credential_id.is_some(),
        credential_id,
        priority,
        temperature,
        max_tokens,
        timeout_s,
        rpm_limit,
        embedding_dim,
        flags: LlmProviderFlags {
            supports_tools,
            allow_private_network,
            allow_pii,
        },
        enabled,
        created_at,
    })
}

fn decode_usage_row(row: &PgRow) -> Result<LlmUsageRecord, AppError> {
    let provider_id: Uuid = row
        .try_get("provider_id")
        .map_err(|e| AppError::Database(format!("decode ax_llm_usage.provider_id: {e}")))?;
    let provider_name: String = row
        .try_get("provider_name")
        .map_err(|e| AppError::Database(format!("decode provider_name: {e}")))?;
    let usage_date: NaiveDate = row
        .try_get("usage_date")
        .map_err(|e| AppError::Database(format!("decode ax_llm_usage.usage_date: {e}")))?;
    let raw_purpose: String = row
        .try_get("purpose")
        .map_err(|e| AppError::Database(format!("decode ax_llm_usage.purpose: {e}")))?;
    let purpose: Purpose = raw_purpose.parse()?;
    let requests: i64 = row
        .try_get("requests")
        .map_err(|e| AppError::Database(format!("decode ax_llm_usage.requests: {e}")))?;
    let errors: i64 = row
        .try_get("errors")
        .map_err(|e| AppError::Database(format!("decode ax_llm_usage.errors: {e}")))?;
    let prompt_tokens: i64 = row
        .try_get("prompt_tokens")
        .map_err(|e| AppError::Database(format!("decode ax_llm_usage.prompt_tokens: {e}")))?;
    let completion_tokens: i64 = row
        .try_get("completion_tokens")
        .map_err(|e| AppError::Database(format!("decode ax_llm_usage.completion_tokens: {e}")))?;

    Ok(LlmUsageRecord {
        provider_id,
        provider_name,
        usage_date,
        purpose,
        requests,
        errors,
        prompt_tokens,
        completion_tokens,
    })
}

pub(super) async fn list_llm_providers_impl(
    store: &PgStore,
    tenant_id: &str,
) -> Result<Vec<LlmProviderRecord>, AppError> {
    let mut tx = begin_tenant_tx(&store.pool, tenant_id).await?;
    let rows = sqlx::query("SELECT * FROM ax_llm_providers ORDER BY priority ASC, created_at ASC")
        .fetch_all(&mut *tx)
        .await
        .map_err(|e| AppError::Database(format!("list ax_llm_providers failed: {e}")))?;

    let mut items = Vec::with_capacity(rows.len());
    for row in &rows {
        items.push(decode_provider_row(row)?);
    }
    tx.commit()
        .await
        .map_err(|e| AppError::Database(format!("commit list_llm_providers tx: {e}")))?;
    Ok(items)
}

pub(super) async fn get_llm_provider_impl(
    store: &PgStore,
    tenant_id: &str,
    provider_id: Uuid,
) -> Result<Option<(LlmProviderRecord, Option<SealedCredential>)>, AppError> {
    let mut tx = begin_tenant_tx(&store.pool, tenant_id).await?;
    let row_opt = sqlx::query(
        "SELECT p.*,
                c.key_version AS cred_key_version,
                c.nonce       AS cred_nonce,
                c.ciphertext  AS cred_ciphertext
           FROM ax_llm_providers p
           LEFT JOIN ds_credentials c
                  ON c.id = p.credential_id AND c.tenant_id = p.tenant_id
          WHERE p.id = $1",
    )
    .bind(provider_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|e| AppError::Database(format!("get ax_llm_providers failed: {e}")))?;

    tx.commit()
        .await
        .map_err(|e| AppError::Database(format!("commit get_llm_provider tx: {e}")))?;

    let Some(row) = row_opt else {
        return Ok(None);
    };
    let rec = decode_provider_row(&row)?;
    let sealed = extract_joined_credential(&row, rec.credential_id);
    Ok(Some((rec, sealed)))
}

pub(super) async fn create_llm_provider_impl(
    store: &PgStore,
    tenant_id: &str,
    input: SaveLlmProviderInput,
) -> Result<LlmProviderRecord, AppError> {
    if input.purposes.is_empty() {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "purposes cannot be empty",
        ));
    }
    let mut tx = begin_tenant_tx(&store.pool, tenant_id).await?;
    let credential_id = match &input.credential {
        CredentialMutation::Set(cred) => {
            upsert_sealed_credential_tx(&mut tx, tenant_id, cred).await?;
            Some(cred.id)
        }
        CredentialMutation::Keep | CredentialMutation::Clear => None,
    };

    let purposes_str: Vec<String> = input
        .purposes
        .iter()
        .map(|p| String::from(p.as_str()))
        .collect();

    let row = sqlx::query(
        "INSERT INTO ax_llm_providers (
            tenant_id, name, base_url, model, purposes, credential_id, priority,
            temperature, max_tokens, timeout_s, rpm_limit, supports_tools, embedding_dim,
            allow_private_network, allow_pii, enabled
         )
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)
         RETURNING *",
    )
    .bind(tenant_id)
    .bind(&input.name)
    .bind(&input.base_url)
    .bind(&input.model)
    .bind(&purposes_str)
    .bind(credential_id)
    .bind(input.priority)
    .bind(input.temperature)
    .bind(input.max_tokens)
    .bind(input.timeout_s)
    .bind(input.rpm_limit)
    .bind(input.flags.supports_tools)
    .bind(input.embedding_dim)
    .bind(input.flags.allow_private_network)
    .bind(input.flags.allow_pii)
    .bind(input.enabled)
    .fetch_one(&mut *tx)
    .await
    .map_err(|err| {
        if is_unique_violation(&err) {
            AppError::Conflict("NAME_TAKEN")
        } else {
            AppError::Database(format!("insert ax_llm_providers failed: {err}"))
        }
    })?;

    let rec = decode_provider_row(&row)?;
    tx.commit()
        .await
        .map_err(|e| AppError::Database(format!("commit create_llm_provider tx: {e}")))?;
    Ok(rec)
}

pub(super) async fn update_llm_provider_impl(
    store: &PgStore,
    tenant_id: &str,
    provider_id: Uuid,
    input: SaveLlmProviderInput,
) -> Result<Option<LlmProviderRecord>, AppError> {
    if input.purposes.is_empty() {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "purposes cannot be empty",
        ));
    }
    let mut tx = begin_tenant_tx(&store.pool, tenant_id).await?;
    let existing_opt =
        sqlx::query("SELECT credential_id FROM ax_llm_providers WHERE id = $1 FOR UPDATE")
            .bind(provider_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|e| AppError::Database(format!("lock ax_llm_providers failed: {e}")))?;

    let Some(existing) = existing_opt else {
        tx.commit()
            .await
            .map_err(|e| AppError::Database(format!("commit missing provider tx: {e}")))?;
        return Ok(None);
    };
    let old_cred_id: Option<Uuid> = existing
        .try_get("credential_id")
        .map_err(|e| AppError::Database(format!("decode credential_id: {e}")))?;

    let new_cred_id = match &input.credential {
        CredentialMutation::Keep => old_cred_id,
        CredentialMutation::Clear => None,
        CredentialMutation::Set(cred) => {
            upsert_sealed_credential_tx(&mut tx, tenant_id, cred).await?;
            Some(cred.id)
        }
    };

    let purposes_str: Vec<String> = input
        .purposes
        .iter()
        .map(|p| String::from(p.as_str()))
        .collect();

    let row = sqlx::query(
        "UPDATE ax_llm_providers
            SET name                  = $2,
                base_url              = $3,
                model                 = $4,
                purposes              = $5,
                credential_id         = $6,
                priority              = $7,
                temperature           = $8,
                max_tokens            = $9,
                timeout_s             = $10,
                rpm_limit             = $11,
                supports_tools        = $12,
                embedding_dim         = $13,
                allow_private_network = $14,
                allow_pii             = $15,
                enabled               = $16
          WHERE id = $1
         RETURNING *",
    )
    .bind(provider_id)
    .bind(&input.name)
    .bind(&input.base_url)
    .bind(&input.model)
    .bind(&purposes_str)
    .bind(new_cred_id)
    .bind(input.priority)
    .bind(input.temperature)
    .bind(input.max_tokens)
    .bind(input.timeout_s)
    .bind(input.rpm_limit)
    .bind(input.flags.supports_tools)
    .bind(input.embedding_dim)
    .bind(input.flags.allow_private_network)
    .bind(input.flags.allow_pii)
    .bind(input.enabled)
    .fetch_one(&mut *tx)
    .await
    .map_err(|err| {
        if is_unique_violation(&err) {
            AppError::Conflict("NAME_TAKEN")
        } else {
            AppError::Database(format!("update ax_llm_providers failed: {err}"))
        }
    })?;

    if let Some(old_cid) = old_cred_id
        && Some(old_cid) != new_cred_id
    {
        delete_orphan_credential_tx(&mut tx, old_cid).await?;
    }

    let rec = decode_provider_row(&row)?;
    tx.commit()
        .await
        .map_err(|e| AppError::Database(format!("commit update_llm_provider tx: {e}")))?;
    Ok(Some(rec))
}

pub(super) async fn delete_llm_provider_impl(
    store: &PgStore,
    tenant_id: &str,
    provider_id: Uuid,
) -> Result<bool, AppError> {
    let mut tx = begin_tenant_tx(&store.pool, tenant_id).await?;
    let row_opt = sqlx::query("DELETE FROM ax_llm_providers WHERE id = $1 RETURNING credential_id")
        .bind(provider_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|e| AppError::Database(format!("delete ax_llm_providers failed: {e}")))?;

    let Some(row) = row_opt else {
        tx.commit()
            .await
            .map_err(|e| AppError::Database(format!("commit delete_llm_provider tx: {e}")))?;
        return Ok(false);
    };
    let old_cred: Option<Uuid> = row.try_get("credential_id").ok().flatten();
    if let Some(cid) = old_cred {
        delete_orphan_credential_tx(&mut tx, cid).await?;
    }
    tx.commit()
        .await
        .map_err(|e| AppError::Database(format!("commit delete_llm_provider tx: {e}")))?;
    Ok(true)
}

pub(super) async fn record_llm_usage_impl(
    store: &PgStore,
    tenant_id: &str,
    delta: LlmUsageDelta,
) -> Result<(), AppError> {
    let mut tx = begin_tenant_tx(&store.pool, tenant_id).await?;
    sqlx::query(
        "INSERT INTO ax_llm_usage
            (tenant_id, provider_id, usage_date, purpose, requests, errors, prompt_tokens, completion_tokens)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
         ON CONFLICT (tenant_id, provider_id, usage_date, purpose) DO UPDATE SET
            requests          = ax_llm_usage.requests + EXCLUDED.requests,
            errors            = ax_llm_usage.errors + EXCLUDED.errors,
            prompt_tokens     = ax_llm_usage.prompt_tokens + EXCLUDED.prompt_tokens,
            completion_tokens = ax_llm_usage.completion_tokens + EXCLUDED.completion_tokens",
    )
    .bind(tenant_id)
    .bind(delta.provider_id)
    .bind(delta.usage_date)
    .bind(delta.purpose.as_str())
    .bind(delta.requests)
    .bind(delta.errors)
    .bind(delta.prompt_tokens)
    .bind(delta.completion_tokens)
    .execute(&mut *tx)
    .await
    .map_err(|e| AppError::Database(format!("upsert ax_llm_usage failed: {e}")))?;

    tx.commit()
        .await
        .map_err(|e| AppError::Database(format!("commit record_llm_usage tx: {e}")))?;
    Ok(())
}

pub(super) async fn query_llm_usage_impl(
    store: &PgStore,
    tenant_id: &str,
    query: LlmUsageQuery,
) -> Result<Vec<LlmUsageRecord>, AppError> {
    let mut tx = begin_tenant_tx(&store.pool, tenant_id).await?;
    let rows = sqlx::query(
        "SELECT u.provider_id, p.name AS provider_name, u.usage_date, u.purpose,
                u.requests, u.errors, u.prompt_tokens, u.completion_tokens
           FROM ax_llm_usage u
           JOIN ax_llm_providers p ON p.id = u.provider_id
          WHERE ($1::date IS NULL OR u.usage_date >= $1)
            AND ($2::date IS NULL OR u.usage_date <= $2)
          ORDER BY u.usage_date DESC, p.priority ASC, p.name ASC, u.purpose ASC",
    )
    .bind(query.from)
    .bind(query.to)
    .fetch_all(&mut *tx)
    .await
    .map_err(|e| AppError::Database(format!("query ax_llm_usage failed: {e}")))?;

    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(decode_usage_row(row)?);
    }
    tx.commit()
        .await
        .map_err(|e| AppError::Database(format!("commit query_llm_usage tx: {e}")))?;
    Ok(out)
}
