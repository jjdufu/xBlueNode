//! `AxAgent` capability publishing, OpenAPI/Skill manifests, Streamable HTTP MCP Server & Client,
//! tool-calling autonomous agent runtime, multi-agent supervisor orchestration, and embed links (`08-STEP09`).
#![doc(test(attr(deny(warnings), forbid(unsafe_code))))]

pub use axpoint_domain::agent::{
    AgentChatRequest, AgentChatResponse, AgentEmbedRecord, AgentEvent, AgentRecord, AgentSpec,
    AgentTaskInput, AgentTaskOutcome, AgentVersionRecord, ConversationListQuery,
    ConversationRecord, CreateAgentEmbedRequest, CreateAgentEmbedResponse, CreateAgentRequest,
    CreateEndpointRequest, CreateMcpClientRequest, EmbedChatRequest, EndpointListQuery,
    EndpointSpec, KnowledgeScope, LlmProviderRecord, LlmUsageDelta, LlmUsageQuery, LlmUsageRecord,
    McpClientRecord, McpToolInfo, MessageRecord, NewConversationInput, NewMessageInput,
    ProviderTestResponse, ResolvedAgentEmbed, SaveLlmProviderRequest, SimRunToolInput,
    SimRunToolOutput, SkillInvokeSpec, SkillManifest, ToolCallSpec, ToolRef, UpdateAgentRequest,
    UpdateEndpointRequest, default_shape_for_kind,
};
pub use axpoint_domain::enums::{
    ConversationChannel, ConversationStatus, EndpointKind, Environment, MessageRole, OutputShape,
    Purpose,
};
pub use axpoint_domain::ports::{
    AgentCapabilityPort, AgentRepository, AgentTaskPort, LlmProviderStore, SimExecutionPort,
};

use axpoint_llm as _;
use axpoint_security as _;
use chrono as _;
use eventsource_stream as _;
use futures as _;
use hex as _;
use jsonschema as _;
use metrics as _;
use moka as _;
use reqwest as _;
use rmcp as _;
use serde as _;
use serde_json as _;
use sha2 as _;
use tokio as _;
use tracing as _;
use uuid as _;

#[cfg(test)]
mod tests {
    use super::{
        AgentEvent, AgentSpec, EndpointKind, OutputShape, Purpose, default_shape_for_kind,
    };

    #[test]
    fn default_agent_spec_and_event_names_match_blueprint() {
        let spec = AgentSpec::default();
        assert_eq!(spec.provider_purpose, Purpose::Agent);
        assert_eq!(spec.max_steps, 8);
        assert_eq!(spec.memory_turns, 10);
        assert_eq!(
            default_shape_for_kind(EndpointKind::ObjectType),
            OutputShape::Object
        );
        assert_eq!(
            default_shape_for_kind(EndpointKind::Metric),
            OutputShape::Rows
        );
        let ev = AgentEvent::Token {
            delta: String::from("ok"),
            agent_name: None,
        };
        assert_eq!(ev.sse_event_name(), "token");
    }
}
