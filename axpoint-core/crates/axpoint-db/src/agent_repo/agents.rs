//! Persistence helpers for `ax_agents`, `ax_agent_versions`, `ax_conversations`,
//! `ax_messages`, `ax_agent_embeds`, and `ax_resolve_agent_embed`.

use std::collections::BTreeSet;

use axpoint_domain::AppError;
use axpoint_domain::agent::{
    AgentEmbedRecord, AgentRecord, AgentSpec, AgentVersionRecord, ConversationListQuery,
    ConversationRecord, CreateAgentRequest, MessageRecord, NewAgentEmbedInput,
    NewConversationInput, NewMessageInput, ResolvedAgentEmbed, ToolCallSpec, UpdateAgentRequest,
};
use axpoint_domain::axhub::PaginatedList;
use axpoint_domain::enums::{ConversationChannel, ConversationStatus, MessageRole, Permission};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Utc};
use sqlx::Row;
use sqlx::postgres::PgRow;
use uuid::Uuid;

use crate::PgStore;
use crate::tenant::begin_tenant_tx;

fn map_agent_err(context: &str, err: &sqlx::Error) -> AppError {
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

fn parse_permission_list(raw: Vec<String>) -> Result<Vec<Permission>, AppError> {
    let mut out = Vec::with_capacity(raw.len());
    for item in raw {
        let perm: Permission = item.parse()?;
        out.push(perm);
    }
    Ok(out)
}

fn parse_permission_set(raw: Vec<String>) -> Result<BTreeSet<Permission>, AppError> {
    let mut set = BTreeSet::new();
    for item in raw {
        let perm: Permission = item.parse()?;
        set.insert(perm);
    }
    Ok(set)
}

fn encode_conv_cursor(updated_at: DateTime<Utc>, id: Uuid) -> String {
    let raw = format!("{}:{id}", updated_at.timestamp_micros());
    URL_SAFE_NO_PAD.encode(raw.as_bytes())
}

fn decode_conv_cursor(cursor: &str) -> Result<(DateTime<Utc>, Uuid), AppError> {
    let bytes = URL_SAFE_NO_PAD
        .decode(cursor.trim())
        .map_err(|_| AppError::bad("INVALID_ARGUMENT", "invalid conversation cursor encoding"))?;
    let text = String::from_utf8(bytes)
        .map_err(|_| AppError::bad("INVALID_ARGUMENT", "invalid conversation cursor UTF-8"))?;
    let (micros_str, id_str) = text
        .split_once(':')
        .ok_or_else(|| AppError::bad("INVALID_ARGUMENT", "malformed conversation cursor"))?;
    let micros: i64 = micros_str
        .parse()
        .map_err(|_| AppError::bad("INVALID_ARGUMENT", "invalid conversation cursor timestamp"))?;
    let dt = DateTime::from_timestamp_micros(micros)
        .ok_or_else(|| AppError::bad("INVALID_ARGUMENT", "out-of-range conversation cursor"))?;
    let id: Uuid = id_str
        .parse()
        .map_err(|_| AppError::bad("INVALID_ARGUMENT", "invalid conversation cursor UUID"))?;
    Ok((dt, id))
}

fn decode_agent_row(row: &PgRow) -> Result<AgentRecord, AppError> {
    let id: Uuid = row
        .try_get("id")
        .map_err(|e| AppError::Database(format!("decode ax_agents.id: {e}")))?;
    let tenant_id: String = row
        .try_get("tenant_id")
        .map_err(|e| AppError::Database(format!("decode ax_agents.tenant_id: {e}")))?;
    let name: String = row
        .try_get("name")
        .map_err(|e| AppError::Database(format!("decode ax_agents.name: {e}")))?;
    let description: String = row
        .try_get("description")
        .map_err(|e| AppError::Database(format!("decode ax_agents.description: {e}")))?;
    let raw_spec: serde_json::Value = row
        .try_get("spec")
        .map_err(|e| AppError::Database(format!("decode ax_agents.spec: {e}")))?;
    let spec: AgentSpec = serde_json::from_value(raw_spec)
        .map_err(|e| AppError::Database(format!("parse ax_agents.spec JSON: {e}")))?;
    let version: i32 = row
        .try_get("version")
        .map_err(|e| AppError::Database(format!("decode ax_agents.version: {e}")))?;
    let created_by: Option<Uuid> = row
        .try_get("created_by")
        .map_err(|e| AppError::Database(format!("decode ax_agents.created_by: {e}")))?;
    let created_at: DateTime<Utc> = row
        .try_get("created_at")
        .map_err(|e| AppError::Database(format!("decode ax_agents.created_at: {e}")))?;
    let updated_at: DateTime<Utc> = row
        .try_get("updated_at")
        .map_err(|e| AppError::Database(format!("decode ax_agents.updated_at: {e}")))?;

    Ok(AgentRecord {
        id,
        tenant_id,
        name,
        description,
        spec,
        version,
        created_by,
        created_at,
        updated_at,
    })
}

fn decode_agent_version_row(row: &PgRow) -> Result<AgentVersionRecord, AppError> {
    let tenant_id: String = row
        .try_get("tenant_id")
        .map_err(|e| AppError::Database(format!("decode ax_agent_versions.tenant_id: {e}")))?;
    let agent_id: Uuid = row
        .try_get("agent_id")
        .map_err(|e| AppError::Database(format!("decode ax_agent_versions.agent_id: {e}")))?;
    let version: i32 = row
        .try_get("version")
        .map_err(|e| AppError::Database(format!("decode ax_agent_versions.version: {e}")))?;
    let raw_spec: serde_json::Value = row
        .try_get("spec")
        .map_err(|e| AppError::Database(format!("decode ax_agent_versions.spec: {e}")))?;
    let spec: AgentSpec = serde_json::from_value(raw_spec)
        .map_err(|e| AppError::Database(format!("parse ax_agent_versions.spec JSON: {e}")))?;
    let published_by: Option<Uuid> = row
        .try_get("published_by")
        .map_err(|e| AppError::Database(format!("decode ax_agent_versions.published_by: {e}")))?;
    let published_at: DateTime<Utc> = row
        .try_get("published_at")
        .map_err(|e| AppError::Database(format!("decode ax_agent_versions.published_at: {e}")))?;

    Ok(AgentVersionRecord {
        tenant_id,
        agent_id,
        version,
        spec,
        published_by,
        published_at,
    })
}

fn decode_conversation_row(row: &PgRow) -> Result<ConversationRecord, AppError> {
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
        title,
        status,
        created_at,
        updated_at,
    })
}

fn decode_message_row(row: &PgRow) -> Result<MessageRecord, AppError> {
    let id: i64 = row
        .try_get("id")
        .map_err(|e| AppError::Database(format!("decode ax_messages.id: {e}")))?;
    let tenant_id: String = row
        .try_get("tenant_id")
        .map_err(|e| AppError::Database(format!("decode ax_messages.tenant_id: {e}")))?;
    let conversation_id: Uuid = row
        .try_get("conversation_id")
        .map_err(|e| AppError::Database(format!("decode ax_messages.conversation_id: {e}")))?;
    let raw_role: String = row
        .try_get("role")
        .map_err(|e| AppError::Database(format!("decode ax_messages.role: {e}")))?;
    let role: MessageRole = raw_role.parse()?;
    let content: String = row
        .try_get("content")
        .map_err(|e| AppError::Database(format!("decode ax_messages.content: {e}")))?;
    let raw_calls: Option<serde_json::Value> = row
        .try_get("tool_calls")
        .map_err(|e| AppError::Database(format!("decode ax_messages.tool_calls: {e}")))?;
    let tool_calls: Option<Vec<ToolCallSpec>> =
        match raw_calls {
            Some(val) if !val.is_null() => Some(serde_json::from_value(val).map_err(|e| {
                AppError::Database(format!("parse ax_messages.tool_calls JSON: {e}"))
            })?),
            _ => None,
        };
    let tool_call_id: Option<String> = row
        .try_get("tool_call_id")
        .map_err(|e| AppError::Database(format!("decode ax_messages.tool_call_id: {e}")))?;
    let agent_name: Option<String> = row
        .try_get("agent_name")
        .map_err(|e| AppError::Database(format!("decode ax_messages.agent_name: {e}")))?;
    let provider_id: Option<Uuid> = row
        .try_get("provider_id")
        .map_err(|e| AppError::Database(format!("decode ax_messages.provider_id: {e}")))?;
    let prompt_tokens: Option<i32> = row
        .try_get("prompt_tokens")
        .map_err(|e| AppError::Database(format!("decode ax_messages.prompt_tokens: {e}")))?;
    let completion_tokens: Option<i32> = row
        .try_get("completion_tokens")
        .map_err(|e| AppError::Database(format!("decode ax_messages.completion_tokens: {e}")))?;
    let latency_ms: Option<i32> = row
        .try_get("latency_ms")
        .map_err(|e| AppError::Database(format!("decode ax_messages.latency_ms: {e}")))?;
    let created_at: DateTime<Utc> = row
        .try_get("created_at")
        .map_err(|e| AppError::Database(format!("decode ax_messages.created_at: {e}")))?;

    Ok(MessageRecord {
        id,
        tenant_id,
        conversation_id,
        role,
        content,
        tool_calls,
        tool_call_id,
        agent_name,
        provider_id,
        prompt_tokens,
        completion_tokens,
        latency_ms,
        created_at,
    })
}

fn decode_embed_row(row: &PgRow) -> Result<AgentEmbedRecord, AppError> {
    let id: Uuid = row
        .try_get("id")
        .map_err(|e| AppError::Database(format!("decode ax_agent_embeds.id: {e}")))?;
    let tenant_id: String = row
        .try_get("tenant_id")
        .map_err(|e| AppError::Database(format!("decode ax_agent_embeds.tenant_id: {e}")))?;
    let agent_id: Uuid = row
        .try_get("agent_id")
        .map_err(|e| AppError::Database(format!("decode ax_agent_embeds.agent_id: {e}")))?;
    let allowed_origins: Vec<String> = row
        .try_get("allowed_origins")
        .map_err(|e| AppError::Database(format!("decode ax_agent_embeds.allowed_origins: {e}")))?;
    let raw_perms: Vec<String> = row
        .try_get("permissions")
        .map_err(|e| AppError::Database(format!("decode ax_agent_embeds.permissions: {e}")))?;
    let permissions = parse_permission_list(raw_perms)?;
    let rate_limit_rpm: i32 = row
        .try_get("rate_limit_rpm")
        .map_err(|e| AppError::Database(format!("decode ax_agent_embeds.rate_limit_rpm: {e}")))?;
    let expires_at: Option<DateTime<Utc>> = row
        .try_get("expires_at")
        .map_err(|e| AppError::Database(format!("decode ax_agent_embeds.expires_at: {e}")))?;
    let revoked_at: Option<DateTime<Utc>> = row
        .try_get("revoked_at")
        .map_err(|e| AppError::Database(format!("decode ax_agent_embeds.revoked_at: {e}")))?;
    let created_by: Uuid = row
        .try_get("created_by")
        .map_err(|e| AppError::Database(format!("decode ax_agent_embeds.created_by: {e}")))?;
    let created_at: DateTime<Utc> = row
        .try_get("created_at")
        .map_err(|e| AppError::Database(format!("decode ax_agent_embeds.created_at: {e}")))?;

    Ok(AgentEmbedRecord {
        id,
        tenant_id,
        agent_id,
        allowed_origins,
        permissions,
        rate_limit_rpm,
        expires_at,
        revoked_at,
        created_by,
        created_at,
    })
}

pub(super) async fn list_agents_impl(
    store: &PgStore,
    tenant_id: &str,
) -> Result<Vec<AgentRecord>, AppError> {
    let mut tx = begin_tenant_tx(&store.pool, tenant_id).await?;
    let rows = sqlx::query("SELECT * FROM ax_agents ORDER BY updated_at DESC, name ASC")
        .fetch_all(&mut *tx)
        .await
        .map_err(|e| AppError::Database(format!("list ax_agents failed: {e}")))?;

    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(decode_agent_row(row)?);
    }
    tx.commit()
        .await
        .map_err(|e| AppError::Database(format!("commit list_agents tx: {e}")))?;
    Ok(out)
}

pub(super) async fn get_agent_impl(
    store: &PgStore,
    tenant_id: &str,
    agent_id: Uuid,
) -> Result<Option<AgentRecord>, AppError> {
    let mut tx = begin_tenant_tx(&store.pool, tenant_id).await?;
    let row_opt = sqlx::query("SELECT * FROM ax_agents WHERE id = $1")
        .bind(agent_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|e| AppError::Database(format!("get ax_agents failed: {e}")))?;

    let rec = match row_opt {
        Some(ref r) => Some(decode_agent_row(r)?),
        None => None,
    };
    tx.commit()
        .await
        .map_err(|e| AppError::Database(format!("commit get_agent tx: {e}")))?;
    Ok(rec)
}

pub(super) async fn get_agent_by_name_impl(
    store: &PgStore,
    tenant_id: &str,
    name: &str,
) -> Result<Option<AgentRecord>, AppError> {
    let mut tx = begin_tenant_tx(&store.pool, tenant_id).await?;
    let row_opt = sqlx::query("SELECT * FROM ax_agents WHERE name = $1")
        .bind(name)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|e| AppError::Database(format!("get ax_agents by name failed: {e}")))?;

    let rec = match row_opt {
        Some(ref r) => Some(decode_agent_row(r)?),
        None => None,
    };
    tx.commit()
        .await
        .map_err(|e| AppError::Database(format!("commit get_agent_by_name tx: {e}")))?;
    Ok(rec)
}

pub(super) async fn create_agent_impl(
    store: &PgStore,
    tenant_id: &str,
    created_by: Option<Uuid>,
    req: CreateAgentRequest,
) -> Result<AgentRecord, AppError> {
    let mut tx = begin_tenant_tx(&store.pool, tenant_id).await?;
    let spec_json = serde_json::to_value(&req.spec)
        .map_err(|e| AppError::Config(format!("serialize AgentSpec failed: {e}")))?;
    let row = sqlx::query(
        "INSERT INTO ax_agents (tenant_id, name, description, spec, version, created_by)
         VALUES ($1, $2, $3, $4, 0, $5)
         RETURNING *",
    )
    .bind(tenant_id)
    .bind(&req.name)
    .bind(&req.description)
    .bind(spec_json)
    .bind(created_by)
    .fetch_one(&mut *tx)
    .await
    .map_err(|err| map_agent_err("insert ax_agents", &err))?;

    let rec = decode_agent_row(&row)?;
    tx.commit()
        .await
        .map_err(|e| AppError::Database(format!("commit create_agent tx: {e}")))?;
    Ok(rec)
}

pub(super) async fn update_agent_impl(
    store: &PgStore,
    tenant_id: &str,
    agent_id: Uuid,
    req: UpdateAgentRequest,
) -> Result<Option<AgentRecord>, AppError> {
    let mut tx = begin_tenant_tx(&store.pool, tenant_id).await?;
    let spec_json = serde_json::to_value(&req.spec)
        .map_err(|e| AppError::Config(format!("serialize AgentSpec failed: {e}")))?;
    let row_opt = sqlx::query(
        "UPDATE ax_agents
            SET name        = COALESCE($2, name),
                description = COALESCE($3, description),
                spec        = $4,
                updated_at  = NOW()
          WHERE id = $1
         RETURNING *",
    )
    .bind(agent_id)
    .bind(req.name)
    .bind(req.description)
    .bind(spec_json)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|err| map_agent_err("update ax_agents", &err))?;

    let rec = match row_opt {
        Some(ref r) => Some(decode_agent_row(r)?),
        None => None,
    };
    tx.commit()
        .await
        .map_err(|e| AppError::Database(format!("commit update_agent tx: {e}")))?;
    Ok(rec)
}

pub(super) async fn delete_agent_impl(
    store: &PgStore,
    tenant_id: &str,
    agent_id: Uuid,
) -> Result<bool, AppError> {
    let mut tx = begin_tenant_tx(&store.pool, tenant_id).await?;
    let res = sqlx::query("DELETE FROM ax_agents WHERE id = $1")
        .bind(agent_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| AppError::Database(format!("delete ax_agents failed: {e}")))?;
    tx.commit()
        .await
        .map_err(|e| AppError::Database(format!("commit delete_agent tx: {e}")))?;
    Ok(res.rows_affected() > 0)
}

pub(super) async fn publish_agent_version_impl(
    store: &PgStore,
    tenant_id: &str,
    agent_id: Uuid,
    published_by: Option<Uuid>,
) -> Result<AgentVersionRecord, AppError> {
    let mut tx = begin_tenant_tx(&store.pool, tenant_id).await?;
    let locked_opt = sqlx::query("SELECT spec, version FROM ax_agents WHERE id = $1 FOR UPDATE")
        .bind(agent_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|e| AppError::Database(format!("lock ax_agents FOR UPDATE failed: {e}")))?;

    let locked = locked_opt.ok_or(AppError::NotFound("AGENT_NOT_FOUND"))?;
    let spec_json: serde_json::Value = locked
        .try_get("spec")
        .map_err(|e| AppError::Database(format!("decode ax_agents.spec: {e}")))?;
    let current_ver: i32 = locked
        .try_get("version")
        .map_err(|e| AppError::Database(format!("decode ax_agents.version: {e}")))?;
    let next_ver = current_ver
        .checked_add(1)
        .ok_or_else(|| AppError::Database(String::from("agent version overflow")))?;

    sqlx::query("UPDATE ax_agents SET version = $2, updated_at = NOW() WHERE id = $1")
        .bind(agent_id)
        .bind(next_ver)
        .execute(&mut *tx)
        .await
        .map_err(|e| AppError::Database(format!("increment ax_agents.version failed: {e}")))?;

    let ver_row = sqlx::query(
        "INSERT INTO ax_agent_versions (tenant_id, agent_id, version, spec, published_by)
         VALUES ($1, $2, $3, $4, $5)
         RETURNING *",
    )
    .bind(tenant_id)
    .bind(agent_id)
    .bind(next_ver)
    .bind(spec_json)
    .bind(published_by)
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| AppError::Database(format!("insert ax_agent_versions failed: {e}")))?;

    let ver_rec = decode_agent_version_row(&ver_row)?;
    tx.commit()
        .await
        .map_err(|e| AppError::Database(format!("commit publish_agent_version tx: {e}")))?;
    Ok(ver_rec)
}

pub(super) async fn get_agent_version_impl(
    store: &PgStore,
    tenant_id: &str,
    agent_id: Uuid,
    version: i32,
) -> Result<Option<AgentVersionRecord>, AppError> {
    let mut tx = begin_tenant_tx(&store.pool, tenant_id).await?;
    let row_opt =
        sqlx::query("SELECT * FROM ax_agent_versions WHERE agent_id = $1 AND version = $2")
            .bind(agent_id)
            .bind(version)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|e| AppError::Database(format!("get ax_agent_versions failed: {e}")))?;

    let rec = match row_opt {
        Some(ref r) => Some(decode_agent_version_row(r)?),
        None => None,
    };
    tx.commit()
        .await
        .map_err(|e| AppError::Database(format!("commit get_agent_version tx: {e}")))?;
    Ok(rec)
}

pub(super) async fn list_agent_versions_impl(
    store: &PgStore,
    tenant_id: &str,
    agent_id: Uuid,
) -> Result<Vec<AgentVersionRecord>, AppError> {
    let mut tx = begin_tenant_tx(&store.pool, tenant_id).await?;
    let rows =
        sqlx::query("SELECT * FROM ax_agent_versions WHERE agent_id = $1 ORDER BY version DESC")
            .bind(agent_id)
            .fetch_all(&mut *tx)
            .await
            .map_err(|e| AppError::Database(format!("list ax_agent_versions failed: {e}")))?;

    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(decode_agent_version_row(row)?);
    }
    tx.commit()
        .await
        .map_err(|e| AppError::Database(format!("commit list_agent_versions tx: {e}")))?;
    Ok(out)
}

pub(super) async fn create_conversation_impl(
    store: &PgStore,
    tenant_id: &str,
    input: NewConversationInput,
) -> Result<ConversationRecord, AppError> {
    let mut tx = begin_tenant_tx(&store.pool, tenant_id).await?;
    let row = sqlx::query(
        "INSERT INTO ax_conversations
            (tenant_id, agent_id, agent_version, user_id, channel, embed_id, title, status)
         VALUES ($1, $2, $3, $4, $5, $6, $7, 'ACTIVE')
         RETURNING *",
    )
    .bind(tenant_id)
    .bind(input.agent_id)
    .bind(input.agent_version)
    .bind(input.user_id)
    .bind(input.channel.as_str())
    .bind(input.embed_id)
    .bind(&input.title)
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| AppError::Database(format!("insert ax_conversations failed: {e}")))?;

    let rec = decode_conversation_row(&row)?;
    tx.commit()
        .await
        .map_err(|e| AppError::Database(format!("commit create_conversation tx: {e}")))?;
    Ok(rec)
}

pub(super) async fn get_conversation_impl(
    store: &PgStore,
    tenant_id: &str,
    conversation_id: Uuid,
) -> Result<Option<ConversationRecord>, AppError> {
    let mut tx = begin_tenant_tx(&store.pool, tenant_id).await?;
    let row_opt = sqlx::query("SELECT * FROM ax_conversations WHERE id = $1")
        .bind(conversation_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|e| AppError::Database(format!("get ax_conversations failed: {e}")))?;

    let rec = match row_opt {
        Some(ref r) => Some(decode_conversation_row(r)?),
        None => None,
    };
    tx.commit()
        .await
        .map_err(|e| AppError::Database(format!("commit get_conversation tx: {e}")))?;
    Ok(rec)
}

pub(super) async fn find_flow_conversation_impl(
    store: &PgStore,
    tenant_id: &str,
    agent_id: Uuid,
    idempotency_title: &str,
) -> Result<Option<ConversationRecord>, AppError> {
    let mut tx = begin_tenant_tx(&store.pool, tenant_id).await?;
    let row_opt = sqlx::query(
        "SELECT * FROM ax_conversations
          WHERE agent_id = $1 AND channel = 'FLOW' AND title = $2
          ORDER BY created_at DESC
          LIMIT 1",
    )
    .bind(agent_id)
    .bind(idempotency_title)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|e| AppError::Database(format!("find flow conversation failed: {e}")))?;

    let rec = match row_opt {
        Some(ref r) => Some(decode_conversation_row(r)?),
        None => None,
    };
    tx.commit()
        .await
        .map_err(|e| AppError::Database(format!("commit find_flow_conversation tx: {e}")))?;
    Ok(rec)
}

pub(super) async fn list_user_conversations_impl(
    store: &PgStore,
    tenant_id: &str,
    user_id: Uuid,
    query: ConversationListQuery,
) -> Result<PaginatedList<ConversationRecord>, AppError> {
    let clamped = query.limit.unwrap_or(50).clamp(1, 200);
    let fetch_limit = i64::from(clamped).saturating_add(1);
    let parsed_cursor = match query
        .cursor
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(raw) => Some(decode_conv_cursor(raw)?),
        None => None,
    };
    let cursor_ts = parsed_cursor.map(|(ts, _)| ts);
    let cursor_id = parsed_cursor.map(|(_, id)| id);

    let mut tx = begin_tenant_tx(&store.pool, tenant_id).await?;
    let rows = sqlx::query(
        "SELECT * FROM ax_conversations
          WHERE user_id = $1
            AND ($2::uuid IS NULL OR agent_id = $2)
            AND ($3::timestamptz IS NULL OR (updated_at, id) < ($3, $4))
          ORDER BY updated_at DESC, id DESC
          LIMIT $5",
    )
    .bind(user_id)
    .bind(query.agent_id)
    .bind(cursor_ts)
    .bind(cursor_id)
    .bind(fetch_limit)
    .fetch_all(&mut *tx)
    .await
    .map_err(|e| AppError::Database(format!("list ax_conversations failed: {e}")))?;

    let page_cap = usize::try_from(clamped).unwrap_or(50);
    let mut items = Vec::with_capacity(rows.len().min(page_cap));
    for row in &rows {
        items.push(decode_conversation_row(row)?);
    }

    let next_cursor = if items.len() > page_cap {
        items.truncate(page_cap);
        items
            .last()
            .map(|last| encode_conv_cursor(last.updated_at, last.id))
    } else {
        None
    };

    tx.commit()
        .await
        .map_err(|e| AppError::Database(format!("commit list_user_conversations tx: {e}")))?;
    Ok(PaginatedList { items, next_cursor })
}

pub(super) async fn update_conversation_status_impl(
    store: &PgStore,
    tenant_id: &str,
    conversation_id: Uuid,
    status: ConversationStatus,
) -> Result<Option<ConversationRecord>, AppError> {
    let mut tx = begin_tenant_tx(&store.pool, tenant_id).await?;
    let row_opt = sqlx::query(
        "UPDATE ax_conversations
            SET status     = $2,
                updated_at = NOW()
          WHERE id = $1
         RETURNING *",
    )
    .bind(conversation_id)
    .bind(status.as_str())
    .fetch_optional(&mut *tx)
    .await
    .map_err(|e| AppError::Database(format!("update ax_conversations failed: {e}")))?;

    let rec = match row_opt {
        Some(ref r) => Some(decode_conversation_row(r)?),
        None => None,
    };
    tx.commit()
        .await
        .map_err(|e| AppError::Database(format!("commit update_conversation_status tx: {e}")))?;
    Ok(rec)
}

pub(super) async fn delete_conversation_impl(
    store: &PgStore,
    tenant_id: &str,
    conversation_id: Uuid,
    owner_user_id: Option<Uuid>,
) -> Result<bool, AppError> {
    let mut tx = begin_tenant_tx(&store.pool, tenant_id).await?;
    let res = sqlx::query(
        "DELETE FROM ax_conversations
          WHERE id = $1
            AND ($2::uuid IS NULL OR user_id = $2)",
    )
    .bind(conversation_id)
    .bind(owner_user_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| AppError::Database(format!("delete ax_conversations failed: {e}")))?;
    tx.commit()
        .await
        .map_err(|e| AppError::Database(format!("commit delete_conversation tx: {e}")))?;
    Ok(res.rows_affected() > 0)
}

pub(super) async fn append_message_impl(
    store: &PgStore,
    tenant_id: &str,
    input: NewMessageInput,
) -> Result<MessageRecord, AppError> {
    let mut tx = begin_tenant_tx(&store.pool, tenant_id).await?;
    let calls_json: Option<serde_json::Value> = match &input.tool_calls {
        Some(calls) if !calls.is_empty() => Some(
            serde_json::to_value(calls)
                .map_err(|e| AppError::Config(format!("serialize tool_calls failed: {e}")))?,
        ),
        _ => None,
    };
    let row = sqlx::query(
        "INSERT INTO ax_messages (
            tenant_id, conversation_id, role, content, tool_calls, tool_call_id,
            agent_name, provider_id, prompt_tokens, completion_tokens, latency_ms
         )
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
         RETURNING *",
    )
    .bind(tenant_id)
    .bind(input.conversation_id)
    .bind(input.role.as_str())
    .bind(&input.content)
    .bind(calls_json)
    .bind(&input.tool_call_id)
    .bind(&input.agent_name)
    .bind(input.provider_id)
    .bind(input.prompt_tokens)
    .bind(input.completion_tokens)
    .bind(input.latency_ms)
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| AppError::Database(format!("insert ax_messages failed: {e}")))?;

    sqlx::query("UPDATE ax_conversations SET updated_at = NOW() WHERE id = $1")
        .bind(input.conversation_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| AppError::Database(format!("touch ax_conversations updated_at: {e}")))?;

    let rec = decode_message_row(&row)?;
    tx.commit()
        .await
        .map_err(|e| AppError::Database(format!("commit append_message tx: {e}")))?;
    Ok(rec)
}

pub(super) async fn list_conversation_messages_impl(
    store: &PgStore,
    tenant_id: &str,
    conversation_id: Uuid,
) -> Result<Vec<MessageRecord>, AppError> {
    let mut tx = begin_tenant_tx(&store.pool, tenant_id).await?;
    let rows = sqlx::query("SELECT * FROM ax_messages WHERE conversation_id = $1 ORDER BY id ASC")
        .bind(conversation_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(|e| AppError::Database(format!("list ax_messages failed: {e}")))?;

    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(decode_message_row(row)?);
    }
    tx.commit()
        .await
        .map_err(|e| AppError::Database(format!("commit list_conversation_messages tx: {e}")))?;
    Ok(out)
}

pub(super) async fn create_agent_embed_impl(
    store: &PgStore,
    tenant_id: &str,
    input: NewAgentEmbedInput,
) -> Result<AgentEmbedRecord, AppError> {
    let mut tx = begin_tenant_tx(&store.pool, tenant_id).await?;
    let perm_strings: Vec<String> = input
        .permissions
        .iter()
        .map(|p| String::from(p.as_str()))
        .collect();
    let row = sqlx::query(
        "INSERT INTO ax_agent_embeds (
            tenant_id, agent_id, token_hash, allowed_origins, permissions,
            rate_limit_rpm, expires_at, created_by
         )
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
         RETURNING *",
    )
    .bind(tenant_id)
    .bind(input.agent_id)
    .bind(&input.token_hash)
    .bind(&input.allowed_origins)
    .bind(&perm_strings)
    .bind(input.rate_limit_rpm)
    .bind(input.expires_at)
    .bind(input.created_by)
    .fetch_one(&mut *tx)
    .await
    .map_err(|err| map_agent_err("insert ax_agent_embeds", &err))?;

    let rec = decode_embed_row(&row)?;
    tx.commit()
        .await
        .map_err(|e| AppError::Database(format!("commit create_agent_embed tx: {e}")))?;
    Ok(rec)
}

pub(super) async fn list_agent_embeds_impl(
    store: &PgStore,
    tenant_id: &str,
    agent_id: Uuid,
) -> Result<Vec<AgentEmbedRecord>, AppError> {
    let mut tx = begin_tenant_tx(&store.pool, tenant_id).await?;
    let rows = sqlx::query(
        "SELECT * FROM ax_agent_embeds WHERE agent_id = $1 ORDER BY created_at DESC, id DESC",
    )
    .bind(agent_id)
    .fetch_all(&mut *tx)
    .await
    .map_err(|e| AppError::Database(format!("list ax_agent_embeds failed: {e}")))?;

    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(decode_embed_row(row)?);
    }
    tx.commit()
        .await
        .map_err(|e| AppError::Database(format!("commit list_agent_embeds tx: {e}")))?;
    Ok(out)
}

pub(super) async fn revoke_agent_embed_impl(
    store: &PgStore,
    tenant_id: &str,
    embed_id: Uuid,
) -> Result<bool, AppError> {
    let mut tx = begin_tenant_tx(&store.pool, tenant_id).await?;
    let res = sqlx::query(
        "UPDATE ax_agent_embeds
            SET revoked_at = COALESCE(revoked_at, NOW())
          WHERE id = $1",
    )
    .bind(embed_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| AppError::Database(format!("revoke ax_agent_embeds failed: {e}")))?;
    tx.commit()
        .await
        .map_err(|e| AppError::Database(format!("commit revoke_agent_embed tx: {e}")))?;
    Ok(res.rows_affected() > 0)
}

pub(super) async fn resolve_agent_embed_impl(
    store: &PgStore,
    token_hash: &str,
) -> Result<Option<ResolvedAgentEmbed>, AppError> {
    let row_opt = sqlx::query(
        "SELECT embed_id, tenant_id, agent_id, created_by, permissions, allowed_origins, rate_limit_rpm
           FROM ax_resolve_agent_embed($1)",
    )
    .bind(token_hash)
    .fetch_optional(&store.pool)
    .await
    .map_err(|e| AppError::Database(format!("ax_resolve_agent_embed failed: {e}")))?;

    let Some(row) = row_opt else {
        return Ok(None);
    };

    let embed_id: Uuid = row
        .try_get("embed_id")
        .map_err(|e| AppError::Database(format!("decode embed_id: {e}")))?;
    let tenant_id: String = row
        .try_get("tenant_id")
        .map_err(|e| AppError::Database(format!("decode tenant_id: {e}")))?;
    let agent_id: Uuid = row
        .try_get("agent_id")
        .map_err(|e| AppError::Database(format!("decode agent_id: {e}")))?;
    let created_by: Uuid = row
        .try_get("created_by")
        .map_err(|e| AppError::Database(format!("decode created_by: {e}")))?;
    let raw_perms: Vec<String> = row
        .try_get("permissions")
        .map_err(|e| AppError::Database(format!("decode permissions: {e}")))?;
    let permissions = parse_permission_set(raw_perms)?;
    let allowed_origins: Vec<String> = row
        .try_get("allowed_origins")
        .map_err(|e| AppError::Database(format!("decode allowed_origins: {e}")))?;
    let rate_limit_rpm: i32 = row
        .try_get("rate_limit_rpm")
        .map_err(|e| AppError::Database(format!("decode rate_limit_rpm: {e}")))?;

    Ok(Some(ResolvedAgentEmbed {
        embed_id,
        tenant_id,
        agent_id,
        created_by,
        permissions,
        allowed_origins,
        rate_limit_rpm,
    }))
}
