//! Live `PostgreSQL` + Row-Level Security (`RLS`) integration tests for `axpoint-db::agent_repo` and `tokens`.

use axpoint_db::PgStore;
use axpoint_domain::AppError;
use axpoint_domain::agent::{
    AgentSpec, ConversationListQuery, CreateAgentRequest, CredentialMutation, EndpointListQuery,
    KnowledgeScope, LlmProviderFlags, LlmStructuredCaps, LlmUsageDelta, LlmUsageQuery, McpToolInfo,
    NewAgentEmbedInput, NewApiTokenInput, NewConversationInput, NewMessageInput, SaveEndpointInput,
    SaveLlmProviderInput, SaveMcpClientInput, ToolCallSpec, ToolRef, UpdateAgentRequest,
    UpdateEndpointRequest,
};
use axpoint_domain::axhub::SealedCredential;
use axpoint_domain::enums::{
    ConversationChannel, ConversationStatus, EndpointKind, Environment, MessageRole, OutputShape,
    Permission, Purpose,
};
use axpoint_domain::ports::{
    AgentRepository as _, ApiTokenRepository as _, CredentialStore as _, LlmProviderStore as _,
};
use chrono::{Duration, NaiveDate, Utc};
use serde_json::json;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const DEFAULT_APP_URL: &str = "postgres://axpoint_app:axpoint_app_secret@127.0.0.1:5432/xbluenode";
const DEFAULT_OWNER_URL: &str = "postgres://axpoint:axpoint_secret@127.0.0.1:5432/xbluenode";

async fn test_pools() -> Result<Option<(PgPool, PgPool)>, Box<dyn std::error::Error>> {
    let req = std::env::var("AXPOINT_REQUIRE_DB_TESTS")
        .is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"));
    let app_url = std::env::var("DATABASE_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| String::from(DEFAULT_APP_URL));
    let own_url = std::env::var("MIGRATION_DATABASE_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| String::from(DEFAULT_OWNER_URL));

    let app = match PgPoolOptions::new()
        .max_connections(6)
        .connect(&app_url)
        .await
    {
        Ok(p) => p,
        Err(e) if req => return Err(Box::new(e)),
        Err(_) => return Ok(None),
    };
    let own = match PgPoolOptions::new()
        .max_connections(4)
        .connect(&own_url)
        .await
    {
        Ok(p) => p,
        Err(e) if req => return Err(Box::new(e)),
        Err(_) => return Ok(None),
    };
    Ok(Some((app, own)))
}

fn unique_tenant(prefix: &str) -> String {
    let short: String = Uuid::new_v4()
        .simple()
        .to_string()
        .chars()
        .take(10)
        .collect();
    format!("{prefix}_{short}")
}

async fn setup_tenant_with_user(
    owner_pool: &PgPool,
    tenant_id: &str,
    user_id: Uuid,
    username: &str,
) -> TestResult {
    sqlx::query(
        "INSERT INTO sec_tenants (tenant_id, display_name) VALUES ($1, $1) ON CONFLICT DO NOTHING",
    )
    .bind(tenant_id)
    .execute(owner_pool)
    .await?;
    sqlx::query(
        "INSERT INTO sec_users (id, tenant_id, username, display_name, dept_id)
         VALUES ($1, $2, $3, $3, 'default')
         ON CONFLICT DO NOTHING",
    )
    .bind(user_id)
    .bind(tenant_id)
    .bind(username)
    .execute(owner_pool)
    .await?;
    Ok(())
}

async fn cleanup_tenant(owner_pool: &PgPool, tenant_id: &str) -> TestResult {
    for stmt in [
        "DELETE FROM ax_agent_embeds WHERE tenant_id = $1",
        "DELETE FROM ax_messages WHERE tenant_id = $1",
        "DELETE FROM ax_conversations WHERE tenant_id = $1",
        "DELETE FROM ax_agent_versions WHERE tenant_id = $1",
        "DELETE FROM ax_agents WHERE tenant_id = $1",
        "DELETE FROM ax_mcp_clients WHERE tenant_id = $1",
        "DELETE FROM ax_published_endpoints WHERE tenant_id = $1",
        "DELETE FROM ax_llm_usage WHERE tenant_id = $1",
        "DELETE FROM ax_llm_providers WHERE tenant_id = $1",
        "DELETE FROM ds_credentials WHERE tenant_id = $1",
        "DELETE FROM sec_api_tokens WHERE tenant_id = $1",
        "DELETE FROM sec_users WHERE tenant_id = $1",
        "DELETE FROM sec_tenants WHERE tenant_id = $1",
    ] {
        sqlx::query(stmt)
            .bind(tenant_id)
            .execute(owner_pool)
            .await?;
    }
    Ok(())
}

fn sample_provider_input(
    name: &str,
    priority: i32,
    credential: CredentialMutation,
) -> SaveLlmProviderInput {
    SaveLlmProviderInput {
        name: String::from(name),
        base_url: String::from("https://api.example.com/v1"),
        model: String::from("qwen-plus"),
        purposes: vec![Purpose::Chat, Purpose::Agent],
        credential,
        priority,
        temperature: 0.2,
        max_tokens: 2048,
        timeout_s: 30,
        rpm_limit: Some(60),
        embedding_dim: None,
        flags: LlmProviderFlags {
            supports_tools: true,
            allow_private_network: false,
            allow_pii: false,
        },
        structured_caps: LlmStructuredCaps::default(),
        owner_user_id: None,
        enabled: true,
    }
}

fn sample_agent_spec(prompt: &str) -> AgentSpec {
    AgentSpec {
        system_prompt: String::from(prompt),
        provider_purpose: Purpose::Agent,
        provider_id: None,
        temperature: 0.2,
        max_steps: 8,
        memory_turns: 10,
        tools: vec![ToolRef::Builtin {
            name: String::from("ask_data"),
        }],
        approval_required_tools: vec![String::from("run_flow")],
        knowledge_scope: KnowledgeScope::default(),
        output_schema: None,
    }
}

fn sample_endpoint_input(u_a: Uuid, env: Environment) -> SaveEndpointInput {
    SaveEndpointInput {
        slug: String::from("paid-amount-by-status"),
        environment: env,
        kind: EndpointKind::Metric,
        target_id: String::from("paid_amount"),
        target_version: None,
        target_snapshot: None,
        title: String::from("已支付金额"),
        description: String::from("测试端点"),
        input_schema: json!({"type": "object"}),
        output_shape: OutputShape::Rows,
        required_scope: Permission::DataRead,
        rate_limit_rpm: 60,
        cache_ttl_s: 0,
        expose_mcp: true,
        show_as_app: true,
        enabled: true,
        published_by: Some(u_a),
    }
}

#[tokio::test]
async fn agent_repo_providers_crud_sealed_credentials_and_rls_isolation() -> TestResult {
    let Some((app_pool, owner_pool)) = test_pools().await? else {
        return Ok(());
    };
    let store = PgStore::new(app_pool);
    let t_a = unique_tenant("prov_a");
    let t_b = unique_tenant("prov_b");
    setup_tenant_with_user(&owner_pool, &t_a, Uuid::new_v4(), "alice").await?;
    setup_tenant_with_user(&owner_pool, &t_b, Uuid::new_v4(), "bob").await?;

    let cred = SealedCredential {
        id: Uuid::new_v4(),
        key_version: 1,
        nonce: vec![1_u8; 12],
        ciphertext: vec![9_u8; 24],
    };
    let p_low = store
        .create_llm_provider(
            &t_a,
            sample_provider_input("secondary", 50, CredentialMutation::Keep),
        )
        .await?;
    let p_high = store
        .create_llm_provider(
            &t_a,
            sample_provider_input("primary", 10, CredentialMutation::Set(cred.clone())),
        )
        .await?;
    assert!(p_high.has_api_key);

    let (loaded, loaded_cred) = store
        .get_llm_provider(&t_a, p_high.id)
        .await?
        .ok_or("missing primary provider")?;
    assert_eq!(loaded.id, p_high.id);
    assert_eq!(loaded_cred.map(|c| c.id), Some(cred.id));

    let dup = store
        .create_llm_provider(
            &t_a,
            sample_provider_input("primary", 20, CredentialMutation::Keep),
        )
        .await;
    assert!(matches!(dup, Err(AppError::Conflict("NAME_TAKEN"))));

    let list_a = store.list_tenant_providers(&t_a).await?;
    assert_eq!(list_a.len(), 2);
    assert_eq!(list_a.first().map(|p| p.id), Some(p_high.id));
    assert_eq!(list_a.get(1).map(|p| p.id), Some(p_low.id));
    assert_eq!(store.list_llm_providers(&t_b).await?.len(), 0);
    assert!(store.get_llm_provider(&t_b, p_high.id).await?.is_none());
    assert!(!store.delete_llm_provider(&t_b, p_high.id).await?);
    assert!(store.delete_llm_provider(&t_a, p_high.id).await?);

    cleanup_tenant(&owner_pool, &t_a).await?;
    cleanup_tenant(&owner_pool, &t_b).await?;
    Ok(())
}

#[tokio::test]
async fn agent_repo_llm_usage_accumulates_on_conflict_and_isolates_tenants() -> TestResult {
    let Some((app_pool, owner_pool)) = test_pools().await? else {
        return Ok(());
    };
    let store = PgStore::new(app_pool);
    let t_a = unique_tenant("usage_a");
    let t_b = unique_tenant("usage_b");
    setup_tenant_with_user(&owner_pool, &t_a, Uuid::new_v4(), "alice").await?;
    setup_tenant_with_user(&owner_pool, &t_b, Uuid::new_v4(), "bob").await?;

    let prov = store
        .create_llm_provider(
            &t_a,
            sample_provider_input("primary", 10, CredentialMutation::Keep),
        )
        .await?;
    let day = NaiveDate::from_ymd_opt(2026, 11, 17).unwrap_or(NaiveDate::MIN);

    store
        .record_usage(
            &t_a,
            LlmUsageDelta {
                provider_id: prov.id,
                usage_date: day,
                purpose: Purpose::Chat,
                requests: 1,
                errors: 0,
                prompt_tokens: 100,
                completion_tokens: 40,
            },
        )
        .await?;
    store
        .record_llm_usage(
            &t_a,
            LlmUsageDelta {
                provider_id: prov.id,
                usage_date: day,
                purpose: Purpose::Chat,
                requests: 2,
                errors: 1,
                prompt_tokens: 250,
                completion_tokens: 60,
            },
        )
        .await?;

    let usage_a = store
        .query_llm_usage(
            &t_a,
            LlmUsageQuery {
                from: Some(day),
                to: Some(day),
            },
        )
        .await?;
    assert_eq!(usage_a.len(), 1);
    let u0 = usage_a.first().ok_or("missing usage row")?;
    assert_eq!(u0.provider_name, "primary");
    assert_eq!(
        (
            u0.requests,
            u0.errors,
            u0.prompt_tokens,
            u0.completion_tokens
        ),
        (3, 1, 350, 100)
    );
    assert_eq!(
        store
            .query_llm_usage(&t_b, LlmUsageQuery::default())
            .await?
            .len(),
        0
    );

    cleanup_tenant(&owner_pool, &t_a).await?;
    cleanup_tenant(&owner_pool, &t_b).await?;
    Ok(())
}

#[tokio::test]
async fn agent_repo_endpoints_and_mcp_clients_upsert_promote_and_isolate_tenants() -> TestResult {
    let Some((app_pool, owner_pool)) = test_pools().await? else {
        return Ok(());
    };
    let store = PgStore::new(app_pool);
    let t_a = unique_tenant("ep_a");
    let t_b = unique_tenant("ep_b");
    let u_a = Uuid::new_v4();
    setup_tenant_with_user(&owner_pool, &t_a, u_a, "alice").await?;
    setup_tenant_with_user(&owner_pool, &t_b, Uuid::new_v4(), "bob").await?;

    let ep_test = store
        .create_endpoint(&t_a, sample_endpoint_input(u_a, Environment::Test))
        .await?;
    let mut prod_v1 = sample_endpoint_input(u_a, Environment::Prod);
    prod_v1.target_version = Some(1);
    prod_v1.target_snapshot = Some(json!({"filter_sql": "status = 'PAID'"}));
    let ep_prod_1 = store.upsert_endpoint_by_slug(&t_a, prod_v1.clone()).await?;

    prod_v1.target_version = Some(2);
    let ep_prod_2 = store.upsert_endpoint_by_slug(&t_a, prod_v1).await?;
    assert_eq!(ep_prod_1.id, ep_prod_2.id);
    assert_eq!(ep_prod_2.target_version, Some(2));
    assert_eq!(
        store
            .list_endpoints(
                &t_a,
                EndpointListQuery {
                    environment: Some(Environment::Prod),
                    ..Default::default()
                }
            )
            .await?
            .len(),
        1
    );

    let updated = store
        .update_endpoint(
            &t_a,
            ep_test.id,
            UpdateEndpointRequest {
                title: Some(String::from("已支付金额(更新)")),
                description: None,
                input_schema: None,
                required_scope: None,
                rate_limit_rpm: Some(120),
                cache_ttl_s: Some(30),
                expose_mcp: None,
                show_as_app: None,
                enabled: None,
            },
        )
        .await?
        .ok_or("missing updated endpoint")?;
    assert_eq!(
        (updated.title.as_str(), updated.rate_limit_rpm),
        ("已支付金额(更新)", 120)
    );
    assert!(
        store
            .get_endpoint_by_slug(&t_b, Environment::Prod, "paid-amount-by-status")
            .await?
            .is_none()
    );

    let mcp = store
        .create_mcp_client(
            &t_a,
            SaveMcpClientInput {
                name: String::from("erp_mcp"),
                url: String::from("https://mcp.example.com/sse"),
                credential: None,
                allowed_tools: vec![String::from("get_order")],
                allow_private_network: false,
                enabled: true,
            },
        )
        .await?;
    let synced = store
        .update_mcp_client_tools(
            &t_a,
            mcp.id,
            &[McpToolInfo {
                name: String::from("get_order"),
                description: String::from("Fetch ERP order"),
                input_schema: json!({"type": "object"}),
            }],
        )
        .await?
        .ok_or("missing synced mcp")?;
    assert_eq!(synced.tools_cache.len(), 1);
    assert!(synced.last_synced_at.is_some());
    assert!(
        store
            .get_mcp_client_by_name(&t_b, "erp_mcp")
            .await?
            .is_none()
    );

    cleanup_tenant(&owner_pool, &t_a).await?;
    cleanup_tenant(&owner_pool, &t_b).await?;
    Ok(())
}

#[tokio::test]
async fn agent_repo_publish_increments_monotonically_and_snapshots_immutable_versions() -> TestResult
{
    let Some((app_pool, owner_pool)) = test_pools().await? else {
        return Ok(());
    };
    let store = PgStore::new(app_pool);
    let t_a = unique_tenant("ag_a");
    let t_b = unique_tenant("ag_b");
    let u_a = Uuid::new_v4();
    setup_tenant_with_user(&owner_pool, &t_a, u_a, "alice").await?;
    setup_tenant_with_user(&owner_pool, &t_b, Uuid::new_v4(), "bob").await?;

    let draft = store
        .create_agent(
            &t_a,
            Some(u_a),
            CreateAgentRequest {
                name: String::from("ops_supervisor"),
                description: String::from("运营主管智能体"),
                spec: sample_agent_spec("v1 system prompt"),
            },
        )
        .await?;
    assert_eq!(draft.version, 0);
    assert!(
        store
            .get_agent_by_name(&t_a, "ops_supervisor")
            .await?
            .is_some()
    );

    let v1 = store
        .publish_agent_version(&t_a, draft.id, Some(u_a))
        .await?;
    assert_eq!(
        (v1.version, v1.spec.system_prompt.as_str()),
        (1, "v1 system prompt")
    );

    let edited = store
        .update_agent(
            &t_a,
            draft.id,
            UpdateAgentRequest {
                name: None,
                description: Some(String::from("草稿更新")),
                spec: sample_agent_spec("v2 draft prompt"),
            },
        )
        .await?
        .ok_or("missing edited agent")?;
    assert_eq!(
        (edited.version, edited.spec.system_prompt.as_str()),
        (1, "v2 draft prompt")
    );

    let v1_reload = store
        .get_agent_version(&t_a, draft.id, 1)
        .await?
        .ok_or("missing v1")?;
    assert_eq!(v1_reload.spec.system_prompt, "v1 system prompt");

    let v2 = store
        .publish_agent_version(&t_a, draft.id, Some(u_a))
        .await?;
    assert_eq!(
        (v2.version, v2.spec.system_prompt.as_str()),
        (2, "v2 draft prompt")
    );
    assert_eq!(store.list_agent_versions(&t_a, draft.id).await?.len(), 2);

    assert!(store.get_agent(&t_b, draft.id).await?.is_none());
    let cross_pub = store.publish_agent_version(&t_b, draft.id, None).await;
    assert!(matches!(
        cross_pub,
        Err(AppError::NotFound("AGENT_NOT_FOUND"))
    ));

    cleanup_tenant(&owner_pool, &t_a).await?;
    cleanup_tenant(&owner_pool, &t_b).await?;
    Ok(())
}

fn new_conv_input(
    agent_id: Uuid,
    user_id: Uuid,
    channel: ConversationChannel,
    title: impl Into<String>,
) -> NewConversationInput {
    NewConversationInput {
        agent_id,
        agent_version: 1,
        user_id: Some(user_id),
        channel,
        embed_id: None,
        title: title.into(),
    }
}

fn plain_msg_input(conversation_id: Uuid, role: MessageRole, content: &str) -> NewMessageInput {
    NewMessageInput {
        conversation_id,
        role,
        content: String::from(content),
        tool_calls: None,
        tool_call_id: None,
        agent_name: None,
        provider_id: None,
        prompt_tokens: None,
        completion_tokens: None,
        latency_ms: None,
    }
}

#[tokio::test]
async fn agent_repo_conversations_isolate_users_and_paginate_cursors() -> TestResult {
    let Some((app_pool, owner_pool)) = test_pools().await? else {
        return Ok(());
    };
    let store = PgStore::new(app_pool);
    let t_a = unique_tenant("conv_a");
    let user_a = Uuid::new_v4();
    let user_b = Uuid::new_v4();
    setup_tenant_with_user(&owner_pool, &t_a, user_a, "alice").await?;
    setup_tenant_with_user(&owner_pool, &t_a, user_b, "bob").await?;

    let agent = store
        .create_agent(
            &t_a,
            Some(user_a),
            CreateAgentRequest {
                name: String::from("analyst"),
                description: String::new(),
                spec: sample_agent_spec("analyst prompt"),
            },
        )
        .await?;

    let mut conv_ids_a = Vec::new();
    for idx in 0..3_u8 {
        let c = store
            .create_conversation(
                &t_a,
                new_conv_input(
                    agent.id,
                    user_a,
                    ConversationChannel::Workbench,
                    format!("Alice-{idx}"),
                ),
            )
            .await?;
        conv_ids_a.push(c.id);
    }
    let conv_b = store
        .create_conversation(
            &t_a,
            new_conv_input(
                agent.id,
                user_b,
                ConversationChannel::Workbench,
                "Bob私密会话",
            ),
        )
        .await?;
    let flow_conv = store
        .create_conversation(
            &t_a,
            new_conv_input(
                agent.id,
                user_a,
                ConversationChannel::Flow,
                "flow:run1:node1",
            ),
        )
        .await?;
    assert_eq!(
        store
            .find_flow_conversation(&t_a, agent.id, "flow:run1:node1")
            .await?
            .map(|c| c.id),
        Some(flow_conv.id)
    );

    let q1 = ConversationListQuery {
        agent_id: None,
        limit: Some(2),
        cursor: None,
    };
    let page1 = store.list_user_conversations(&t_a, user_a, q1).await?;
    assert_eq!(page1.items.len(), 2);
    assert!(page1.next_cursor.is_some());

    let q2 = ConversationListQuery {
        agent_id: None,
        limit: Some(5),
        cursor: page1.next_cursor,
    };
    let page2 = store.list_user_conversations(&t_a, user_a, q2).await?;
    assert_eq!((page2.items.len(), page2.next_cursor.is_none()), (2, true));
    assert!(
        !page1
            .items
            .iter()
            .chain(page2.items.iter())
            .any(|c| c.id == conv_b.id)
    );

    let first_conv_id = *conv_ids_a.first().ok_or("missing alice conv")?;
    assert!(
        !store
            .delete_conversation(&t_a, first_conv_id, Some(user_b))
            .await?
    );

    cleanup_tenant(&owner_pool, &t_a).await?;
    Ok(())
}

#[tokio::test]
async fn agent_repo_conversation_messages_preserve_chronological_order_and_speaker() -> TestResult {
    let Some((app_pool, owner_pool)) = test_pools().await? else {
        return Ok(());
    };
    let store = PgStore::new(app_pool);
    let t_a = unique_tenant("msg_a");
    let user_a = Uuid::new_v4();
    setup_tenant_with_user(&owner_pool, &t_a, user_a, "alice").await?;

    let agent = store
        .create_agent(
            &t_a,
            Some(user_a),
            CreateAgentRequest {
                name: String::from("analyst"),
                description: String::new(),
                spec: sample_agent_spec("analyst prompt"),
            },
        )
        .await?;
    let conv = store
        .create_conversation(
            &t_a,
            new_conv_input(agent.id, user_a, ConversationChannel::Workbench, "多轮测试"),
        )
        .await?;

    store
        .append_message(
            &t_a,
            plain_msg_input(conv.id, MessageRole::User, "查询金额"),
        )
        .await?;
    let mut asst = plain_msg_input(conv.id, MessageRole::Assistant, "");
    asst.tool_calls = Some(vec![ToolCallSpec {
        id: String::from("call_1"),
        name: String::from("ask_data"),
        arguments: json!({"question": "查询金额"}),
    }]);
    asst.agent_name = Some(String::from("data_analyst"));
    asst.prompt_tokens = Some(30);
    asst.completion_tokens = Some(12);
    asst.latency_ms = Some(45);
    store.append_message(&t_a, asst).await?;

    let mut tool_msg = plain_msg_input(conv.id, MessageRole::Tool, "{\"rows\":[{\"paid\":100}]}");
    tool_msg.tool_call_id = Some(String::from("call_1"));
    store.append_message(&t_a, tool_msg).await?;

    let updated = store
        .update_conversation_status(&t_a, conv.id, ConversationStatus::WaitingApproval)
        .await?
        .ok_or("missing updated conv")?;
    assert_eq!(updated.status, ConversationStatus::WaitingApproval);

    let msgs = store.list_conversation_messages(&t_a, conv.id).await?;
    assert_eq!(msgs.len(), 3);
    assert_eq!(msgs.first().map(|m| m.role), Some(MessageRole::User));
    assert_eq!(
        msgs.get(1).and_then(|m| m.agent_name.as_deref()),
        Some("data_analyst")
    );
    assert_eq!(
        msgs.get(2).and_then(|m| m.tool_call_id.as_deref()),
        Some("call_1")
    );

    cleanup_tenant(&owner_pool, &t_a).await?;
    Ok(())
}

#[tokio::test]
async fn agent_repo_embeds_resolve_active_and_reject_revoked_or_expired() -> TestResult {
    let Some((app_pool, owner_pool)) = test_pools().await? else {
        return Ok(());
    };
    let store = PgStore::new(app_pool);
    let t_a = unique_tenant("emb_a");
    let t_b = unique_tenant("emb_b");
    let user_a = Uuid::new_v4();
    setup_tenant_with_user(&owner_pool, &t_a, user_a, "alice").await?;
    setup_tenant_with_user(&owner_pool, &t_b, Uuid::new_v4(), "bob").await?;

    let agent = store
        .create_agent(
            &t_a,
            Some(user_a),
            CreateAgentRequest {
                name: String::from("embed_bot"),
                description: String::new(),
                spec: sample_agent_spec("embed prompt"),
            },
        )
        .await?;

    let active_hash = format!("emb_active_{}", Uuid::new_v4().simple());
    let expired_hash = format!("emb_expired_{}", Uuid::new_v4().simple());
    let perms = vec![Permission::DataRead, Permission::AgentRun];

    let emb_active = store
        .create_agent_embed(
            &t_a,
            NewAgentEmbedInput {
                agent_id: agent.id,
                token_hash: active_hash.clone(),
                allowed_origins: vec![String::from("https://app.example.com")],
                permissions: perms.clone(),
                rate_limit_rpm: 25,
                expires_at: Utc::now().checked_add_signed(Duration::hours(2)),
                created_by: user_a,
            },
        )
        .await?;
    store
        .create_agent_embed(
            &t_a,
            NewAgentEmbedInput {
                agent_id: agent.id,
                token_hash: expired_hash.clone(),
                allowed_origins: Vec::new(),
                permissions: perms,
                rate_limit_rpm: 20,
                expires_at: Utc::now().checked_sub_signed(Duration::minutes(5)),
                created_by: user_a,
            },
        )
        .await?;

    let resolved = store
        .resolve_agent_embed(&active_hash)
        .await?
        .ok_or("active embed did not resolve")?;
    assert_eq!(
        (
            resolved.embed_id,
            resolved.tenant_id.as_str(),
            resolved.rate_limit_rpm
        ),
        (emb_active.id, t_a.as_str(), 25)
    );
    assert!(store.resolve_agent_embed(&expired_hash).await?.is_none());
    assert!(!store.revoke_agent_embed(&t_b, emb_active.id).await?);
    assert!(store.revoke_agent_embed(&t_a, emb_active.id).await?);
    assert!(store.resolve_agent_embed(&active_hash).await?.is_none());

    cleanup_tenant(&owner_pool, &t_a).await?;
    cleanup_tenant(&owner_pool, &t_b).await?;
    Ok(())
}

#[tokio::test]
async fn agent_repo_api_tokens_create_list_resolve_and_revoke_with_rls() -> TestResult {
    let Some((app_pool, owner_pool)) = test_pools().await? else {
        return Ok(());
    };
    let store = PgStore::new(app_pool);
    let t_a = unique_tenant("pat_a");
    let t_b = unique_tenant("pat_b");
    let user_a = Uuid::new_v4();
    setup_tenant_with_user(&owner_pool, &t_a, user_a, "alice").await?;
    setup_tenant_with_user(&owner_pool, &t_b, Uuid::new_v4(), "bob").await?;

    let pat_hash = format!("pat_active_{}", Uuid::new_v4().simple());
    let pat_exp_hash = format!("pat_exp_{}", Uuid::new_v4().simple());
    let pat = store
        .create_api_token(
            &t_a,
            NewApiTokenInput {
                user_id: user_a,
                name: String::from("ci-token"),
                token_prefix: String::from("axp_12345678"),
                token_hash: pat_hash.clone(),
                scopes: vec![Permission::DataRead],
                expires_at: None,
            },
        )
        .await?;
    store
        .create_api_token(
            &t_a,
            NewApiTokenInput {
                user_id: user_a,
                name: String::from("expired-token"),
                token_prefix: String::from("axp_87654321"),
                token_hash: pat_exp_hash.clone(),
                scopes: vec![Permission::DataRead],
                expires_at: Utc::now().checked_sub_signed(Duration::minutes(5)),
            },
        )
        .await?;

    assert_eq!(store.list_user_api_tokens(&t_a, user_a).await?.len(), 2);
    assert_eq!(store.list_user_api_tokens(&t_b, user_a).await?.len(), 0);
    assert!(store.resolve_api_token(&pat_hash).await?.is_some());
    assert!(store.resolve_api_token(&pat_exp_hash).await?.is_none());

    assert!(!store.revoke_api_token(&t_b, user_a, pat.id).await?);
    assert!(store.revoke_api_token(&t_a, user_a, pat.id).await?);
    assert!(store.resolve_api_token(&pat_hash).await?.is_none());

    cleanup_tenant(&owner_pool, &t_a).await?;
    cleanup_tenant(&owner_pool, &t_b).await?;
    Ok(())
}
