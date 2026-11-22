//! `AxAgent` `PostgreSQL` persistence implementing [`AgentRepository`] and [`LlmProviderStore`] on [`PgStore`].

mod agents;
mod endpoints;
mod providers;

use std::collections::BTreeMap;

use axpoint_domain::AppError;
use axpoint_domain::agent::{
    AgentEmbedRecord, AgentRecord, AgentVersionRecord, ConversationListQuery, ConversationRecord,
    CreateAgentRequest, EndpointListQuery, EndpointSpec, LlmProviderRecord, LlmUsageDelta,
    LlmUsageQuery, LlmUsageRecord, McpClientRecord, McpToolInfo, MessageRecord, NewAgentEmbedInput,
    NewConversationInput, NewMessageInput, ResolvedAgentEmbed, SaveEndpointInput,
    SaveLlmProviderInput, SaveMcpClientInput, UpdateAgentRequest, UpdateEndpointRequest,
    UserLlmPreferenceRecord,
};
use axpoint_domain::axhub::{PaginatedList, SealedCredential};
use axpoint_domain::enums::{ConversationStatus, Environment, Purpose};
use axpoint_domain::platform::BoxFuture;
use axpoint_domain::ports::{AgentRepository, LlmProviderStore};
use uuid::Uuid;

use crate::PgStore;

impl LlmProviderStore for PgStore {
    fn list_tenant_providers<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> BoxFuture<'a, Vec<LlmProviderRecord>> {
        Box::pin(async move { providers::list_llm_providers_impl(self, tenant_id).await })
    }

    fn record_usage<'a>(&'a self, tenant_id: &'a str, delta: LlmUsageDelta) -> BoxFuture<'a, ()> {
        Box::pin(async move { providers::record_llm_usage_impl(self, tenant_id, delta).await })
    }

    fn list_user_preferences<'a>(
        &'a self,
        tenant_id: &'a str,
        user_id: Uuid,
    ) -> BoxFuture<'a, BTreeMap<Purpose, Uuid>> {
        Box::pin(async move {
            providers::list_user_preferences_map_impl(self, tenant_id, user_id).await
        })
    }
}

impl AgentRepository for PgStore {
    async fn list_llm_providers(
        &self,
        tenant_id: &str,
    ) -> Result<Vec<LlmProviderRecord>, AppError> {
        providers::list_llm_providers_impl(self, tenant_id).await
    }

    async fn list_visible_llm_providers(
        &self,
        tenant_id: &str,
        viewer_user_id: Uuid,
    ) -> Result<Vec<LlmProviderRecord>, AppError> {
        providers::list_visible_llm_providers_impl(self, tenant_id, viewer_user_id).await
    }

    async fn list_user_llm_preferences(
        &self,
        tenant_id: &str,
        user_id: Uuid,
    ) -> Result<Vec<UserLlmPreferenceRecord>, AppError> {
        providers::list_user_llm_preferences_impl(self, tenant_id, user_id).await
    }

    async fn replace_user_llm_preferences(
        &self,
        tenant_id: &str,
        user_id: Uuid,
        items: &[(Purpose, Uuid)],
    ) -> Result<Vec<UserLlmPreferenceRecord>, AppError> {
        providers::replace_user_llm_preferences_impl(self, tenant_id, user_id, items).await
    }

    async fn get_llm_provider(
        &self,
        tenant_id: &str,
        provider_id: Uuid,
    ) -> Result<Option<(LlmProviderRecord, Option<SealedCredential>)>, AppError> {
        providers::get_llm_provider_impl(self, tenant_id, provider_id).await
    }

    async fn create_llm_provider(
        &self,
        tenant_id: &str,
        input: SaveLlmProviderInput,
    ) -> Result<LlmProviderRecord, AppError> {
        providers::create_llm_provider_impl(self, tenant_id, input).await
    }

    async fn update_llm_provider(
        &self,
        tenant_id: &str,
        provider_id: Uuid,
        input: SaveLlmProviderInput,
    ) -> Result<Option<LlmProviderRecord>, AppError> {
        providers::update_llm_provider_impl(self, tenant_id, provider_id, input).await
    }

    async fn delete_llm_provider(
        &self,
        tenant_id: &str,
        provider_id: Uuid,
    ) -> Result<bool, AppError> {
        providers::delete_llm_provider_impl(self, tenant_id, provider_id).await
    }

    async fn record_llm_usage(
        &self,
        tenant_id: &str,
        delta: LlmUsageDelta,
    ) -> Result<(), AppError> {
        providers::record_llm_usage_impl(self, tenant_id, delta).await
    }

    async fn query_llm_usage(
        &self,
        tenant_id: &str,
        query: LlmUsageQuery,
    ) -> Result<Vec<LlmUsageRecord>, AppError> {
        providers::query_llm_usage_impl(self, tenant_id, query).await
    }

    async fn list_endpoints(
        &self,
        tenant_id: &str,
        query: EndpointListQuery,
    ) -> Result<Vec<EndpointSpec>, AppError> {
        endpoints::list_endpoints_impl(self, tenant_id, query).await
    }

    async fn get_endpoint(
        &self,
        tenant_id: &str,
        endpoint_id: Uuid,
    ) -> Result<Option<EndpointSpec>, AppError> {
        endpoints::get_endpoint_impl(self, tenant_id, endpoint_id).await
    }

    async fn get_endpoint_by_slug(
        &self,
        tenant_id: &str,
        environment: Environment,
        slug: &str,
    ) -> Result<Option<EndpointSpec>, AppError> {
        endpoints::get_endpoint_by_slug_impl(self, tenant_id, environment, slug).await
    }

    async fn create_endpoint(
        &self,
        tenant_id: &str,
        input: SaveEndpointInput,
    ) -> Result<EndpointSpec, AppError> {
        endpoints::create_endpoint_impl(self, tenant_id, input).await
    }

    async fn update_endpoint(
        &self,
        tenant_id: &str,
        endpoint_id: Uuid,
        req: UpdateEndpointRequest,
    ) -> Result<Option<EndpointSpec>, AppError> {
        endpoints::update_endpoint_impl(self, tenant_id, endpoint_id, req).await
    }

    async fn upsert_endpoint_by_slug(
        &self,
        tenant_id: &str,
        input: SaveEndpointInput,
    ) -> Result<EndpointSpec, AppError> {
        endpoints::upsert_endpoint_by_slug_impl(self, tenant_id, input).await
    }

    async fn delete_endpoint(&self, tenant_id: &str, endpoint_id: Uuid) -> Result<bool, AppError> {
        endpoints::delete_endpoint_impl(self, tenant_id, endpoint_id).await
    }

    async fn list_mcp_clients(&self, tenant_id: &str) -> Result<Vec<McpClientRecord>, AppError> {
        endpoints::list_mcp_clients_impl(self, tenant_id).await
    }

    async fn get_mcp_client(
        &self,
        tenant_id: &str,
        client_id: Uuid,
    ) -> Result<Option<(McpClientRecord, Option<SealedCredential>)>, AppError> {
        endpoints::get_mcp_client_impl(self, tenant_id, client_id).await
    }

    async fn get_mcp_client_by_name(
        &self,
        tenant_id: &str,
        name: &str,
    ) -> Result<Option<(McpClientRecord, Option<SealedCredential>)>, AppError> {
        endpoints::get_mcp_client_by_name_impl(self, tenant_id, name).await
    }

    async fn create_mcp_client(
        &self,
        tenant_id: &str,
        input: SaveMcpClientInput,
    ) -> Result<McpClientRecord, AppError> {
        endpoints::create_mcp_client_impl(self, tenant_id, input).await
    }

    async fn delete_mcp_client(&self, tenant_id: &str, client_id: Uuid) -> Result<bool, AppError> {
        endpoints::delete_mcp_client_impl(self, tenant_id, client_id).await
    }

    async fn update_mcp_client_tools(
        &self,
        tenant_id: &str,
        client_id: Uuid,
        tools: &[McpToolInfo],
    ) -> Result<Option<McpClientRecord>, AppError> {
        endpoints::update_mcp_client_tools_impl(self, tenant_id, client_id, tools).await
    }

    async fn list_agents(&self, tenant_id: &str) -> Result<Vec<AgentRecord>, AppError> {
        agents::list_agents_impl(self, tenant_id).await
    }

    async fn get_agent(
        &self,
        tenant_id: &str,
        agent_id: Uuid,
    ) -> Result<Option<AgentRecord>, AppError> {
        agents::get_agent_impl(self, tenant_id, agent_id).await
    }

    async fn get_agent_by_name(
        &self,
        tenant_id: &str,
        name: &str,
    ) -> Result<Option<AgentRecord>, AppError> {
        agents::get_agent_by_name_impl(self, tenant_id, name).await
    }

    async fn create_agent(
        &self,
        tenant_id: &str,
        created_by: Option<Uuid>,
        req: CreateAgentRequest,
    ) -> Result<AgentRecord, AppError> {
        agents::create_agent_impl(self, tenant_id, created_by, req).await
    }

    async fn update_agent(
        &self,
        tenant_id: &str,
        agent_id: Uuid,
        req: UpdateAgentRequest,
    ) -> Result<Option<AgentRecord>, AppError> {
        agents::update_agent_impl(self, tenant_id, agent_id, req).await
    }

    async fn delete_agent(&self, tenant_id: &str, agent_id: Uuid) -> Result<bool, AppError> {
        agents::delete_agent_impl(self, tenant_id, agent_id).await
    }

    async fn publish_agent_version(
        &self,
        tenant_id: &str,
        agent_id: Uuid,
        published_by: Option<Uuid>,
    ) -> Result<AgentVersionRecord, AppError> {
        agents::publish_agent_version_impl(self, tenant_id, agent_id, published_by).await
    }

    async fn get_agent_version(
        &self,
        tenant_id: &str,
        agent_id: Uuid,
        version: i32,
    ) -> Result<Option<AgentVersionRecord>, AppError> {
        agents::get_agent_version_impl(self, tenant_id, agent_id, version).await
    }

    async fn list_agent_versions(
        &self,
        tenant_id: &str,
        agent_id: Uuid,
    ) -> Result<Vec<AgentVersionRecord>, AppError> {
        agents::list_agent_versions_impl(self, tenant_id, agent_id).await
    }

    async fn create_conversation(
        &self,
        tenant_id: &str,
        input: NewConversationInput,
    ) -> Result<ConversationRecord, AppError> {
        agents::create_conversation_impl(self, tenant_id, input).await
    }

    async fn get_conversation(
        &self,
        tenant_id: &str,
        conversation_id: Uuid,
    ) -> Result<Option<ConversationRecord>, AppError> {
        agents::get_conversation_impl(self, tenant_id, conversation_id).await
    }

    async fn find_flow_conversation(
        &self,
        tenant_id: &str,
        agent_id: Uuid,
        idempotency_title: &str,
    ) -> Result<Option<ConversationRecord>, AppError> {
        agents::find_flow_conversation_impl(self, tenant_id, agent_id, idempotency_title).await
    }

    async fn list_user_conversations(
        &self,
        tenant_id: &str,
        user_id: Uuid,
        query: ConversationListQuery,
    ) -> Result<PaginatedList<ConversationRecord>, AppError> {
        agents::list_user_conversations_impl(self, tenant_id, user_id, query).await
    }

    async fn update_conversation_status(
        &self,
        tenant_id: &str,
        conversation_id: Uuid,
        status: ConversationStatus,
    ) -> Result<Option<ConversationRecord>, AppError> {
        agents::update_conversation_status_impl(self, tenant_id, conversation_id, status).await
    }

    async fn delete_conversation(
        &self,
        tenant_id: &str,
        conversation_id: Uuid,
        owner_user_id: Option<Uuid>,
    ) -> Result<bool, AppError> {
        agents::delete_conversation_impl(self, tenant_id, conversation_id, owner_user_id).await
    }

    async fn append_message(
        &self,
        tenant_id: &str,
        input: NewMessageInput,
    ) -> Result<MessageRecord, AppError> {
        agents::append_message_impl(self, tenant_id, input).await
    }

    async fn list_conversation_messages(
        &self,
        tenant_id: &str,
        conversation_id: Uuid,
    ) -> Result<Vec<MessageRecord>, AppError> {
        agents::list_conversation_messages_impl(self, tenant_id, conversation_id).await
    }

    async fn create_agent_embed(
        &self,
        tenant_id: &str,
        input: NewAgentEmbedInput,
    ) -> Result<AgentEmbedRecord, AppError> {
        agents::create_agent_embed_impl(self, tenant_id, input).await
    }

    async fn list_agent_embeds(
        &self,
        tenant_id: &str,
        agent_id: Uuid,
    ) -> Result<Vec<AgentEmbedRecord>, AppError> {
        agents::list_agent_embeds_impl(self, tenant_id, agent_id).await
    }

    async fn revoke_agent_embed(&self, tenant_id: &str, embed_id: Uuid) -> Result<bool, AppError> {
        agents::revoke_agent_embed_impl(self, tenant_id, embed_id).await
    }

    async fn resolve_agent_embed(
        &self,
        token_hash: &str,
    ) -> Result<Option<ResolvedAgentEmbed>, AppError> {
        agents::resolve_agent_embed_impl(self, token_hash).await
    }
}
