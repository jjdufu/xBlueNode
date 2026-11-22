#![doc(test(attr(deny(warnings), forbid(unsafe_code))))]
//! `AxAgent` capability publishing, Streamable HTTP `MCP` server & client,
//! 13-tool multi-turn agent runtime, multi-agent delegation, and external embed tokens (`08-STEP09`).

// Ensure Cargo manifest dependencies not directly imported at crate root remain acknowledged.
use hex as _;
use sha2 as _;

pub mod embed;
pub mod mcp_client;
pub mod mcp_server;
pub mod model;
pub mod multi;
pub mod openapi;
pub mod publish;
pub mod rate_limit;
pub mod runtime;
pub mod tools;

pub use axpoint_domain::agent::{
    AgentChatRequest, AgentChatResponse, AgentEmbedRecord, AgentEvent, AgentRecord, AgentSpec,
    AgentTaskInput, AgentTaskOutcome, AgentVersionRecord, ConversationListQuery,
    ConversationRecord, CreateAgentEmbedRequest, CreateAgentEmbedResponse, CreateAgentRequest,
    CreateApiTokenRequest, CreateApiTokenResponse, CreateEndpointRequest, CreateMcpClientRequest,
    CredentialMutation, EmbedChatRequest, EndpointListQuery, EndpointSpec, KnowledgeScope,
    LlmProviderFlags, LlmProviderRecord, LlmStructuredCaps, LlmUsageDelta, LlmUsageQuery,
    LlmUsageRecord, McpClientRecord, McpToolInfo, MessageRecord, NewAgentEmbedInput,
    NewApiTokenInput, NewConversationInput, NewMessageInput, PersonalAccessTokenSummary,
    ProviderScope, ProviderTestResponse, PutUserLlmPreferencesRequest, ResolvedAgentEmbed,
    SaveEndpointInput, SaveLlmProviderInput, SaveLlmProviderRequest, SaveMcpClientInput,
    SimRunToolInput, SimRunToolOutput, SkillInvokeSpec, SkillManifest, ToolCallSpec, ToolRef,
    UpdateAgentRequest, UpdateEndpointRequest, UserLlmPreferenceItem, UserLlmPreferenceRecord,
};
pub use axpoint_domain::enums::EndpointKind;
pub use axpoint_domain::ports::{
    AgentCapabilityPort, AgentRepository, AgentTaskPort, ApiTokenRepository, LlmProviderStore,
    SimExecutionPort,
};

pub use embed::{EmbedRequestMeta, EmbedService, generate_embed_token};
pub use mcp_client::McpClientManager;
pub use mcp_server::{McpHttpResponse, McpServerHub};
pub use model::{
    AgentCassetteEntry, AgentModelPort, AgentModelRequest, AgentModelTurn, CassetteModelAdapter,
    ModelMessage, ModelToolDef, compute_agent_request_sha256,
};
pub use multi::{
    MAX_SUB_AGENT_DEPTH, build_sub_agent_tool_def, format_sub_agent_prompt, sub_agent_tool_name,
    validate_as_tool_name, validate_sub_agent_graph,
};
pub use openapi::{build_openapi_spec, build_skill_manifest, env_path_segment};
pub use publish::{
    CapabilityPublisher, PublishedAgentCall, PublishedAgentInvoker, PublishedCallContext,
    PublishedInvokeOutcome, PublishedRouteTarget, build_agent_schema, build_flow_schema,
    build_metric_schema, build_object_type_schema, build_saved_query_schema,
    validate_endpoint_slug, validate_instance_against_schema,
};
pub use runtime::{
    AgentApprovalDecidedSubscriber, AgentRuntime, AgentRuntimeDeps, ChannelChatCall,
};
pub use tools::{
    ALL_BUILTIN_TOOL_NAMES, BuiltinToolRegistry, MAX_TOOL_OUTPUT_BYTES, MAX_TOOL_OUTPUT_ROWS,
    TRUNCATION_HINT, is_known_builtin_tool, required_builtin_permission, truncate_tool_output,
};
