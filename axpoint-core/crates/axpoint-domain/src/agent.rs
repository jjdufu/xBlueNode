//! `AxAgent` publishing, multi-model routing, MCP, autonomous agent runtime, embed links,
//! and personal access token contracts (`08-STEP09-AXAGENT-PUBLISHING-MCP-AGENTS.md`).

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::axhub::SealedCredential;
use crate::enums::{
    ConversationChannel, ConversationStatus, EndpointKind, Environment, MessageRole, OutputShape,
    Permission, Purpose,
};

const fn default_priority() -> i32 {
    100
}

const fn default_temperature() -> f32 {
    0.2
}

const fn default_max_tokens() -> i32 {
    4096
}

const fn default_timeout_s() -> i32 {
    60
}

const fn default_true() -> bool {
    true
}

const fn default_rate_limit_rpm() -> i32 {
    60
}

const fn default_embed_rate_limit_rpm() -> i32 {
    20
}

const fn default_max_steps() -> u32 {
    8
}

const fn default_memory_turns() -> u32 {
    10
}

const fn default_agent_purpose() -> Purpose {
    Purpose::Agent
}

const fn default_required_scope() -> Permission {
    Permission::AgentRun
}

const fn default_output_shape() -> OutputShape {
    OutputShape::Rows
}

/// Summary view of a user's personal access token (`GET /api/auth/tokens`, `08-STEP09` §09.8).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonalAccessTokenSummary {
    /// Token UUID.
    pub id: Uuid,
    /// User-defined label.
    pub name: String,
    /// Non-secret prefix (`axp_<prefix8>`).
    pub token_prefix: String,
    /// Explicit permission scopes (empty inherits all user permissions).
    pub scopes: Vec<Permission>,
    /// Optional expiration timestamp.
    pub expires_at: Option<DateTime<Utc>>,
    /// Most recent usage timestamp.
    pub last_used_at: Option<DateTime<Utc>>,
    /// Revocation timestamp, if revoked.
    pub revoked_at: Option<DateTime<Utc>>,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
}

/// Request payload for `POST /api/auth/tokens` (`08-STEP09` §09.8).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateApiTokenRequest {
    /// Human-readable token name.
    pub name: String,
    /// Permission subset (`scopes` must be a subset of caller's permissions).
    #[serde(default)]
    pub scopes: Vec<Permission>,
    /// Optional validity duration in days (`1..=365`).
    #[serde(default)]
    pub expires_in_days: Option<u32>,
}

/// Response payload for `POST /api/auth/tokens` returning the plaintext `axp_...` secret once.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateApiTokenResponse {
    /// Token UUID.
    pub id: Uuid,
    /// Token name.
    pub name: String,
    /// Non-secret prefix.
    pub token_prefix: String,
    /// One-time plaintext bearer token (`axp_<prefix8>_<secret32>`).
    pub token: String,
    /// Granted permission scopes.
    pub scopes: Vec<Permission>,
    /// Optional expiration timestamp.
    pub expires_at: Option<DateTime<Utc>>,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
}

/// Repository input for inserting a new `sec_api_tokens` row.
#[derive(Debug, Clone)]
pub struct NewApiTokenInput {
    /// Owning user UUID.
    pub user_id: Uuid,
    /// Token label.
    pub name: String,
    /// Display prefix (`axp_<prefix8>`).
    pub token_prefix: String,
    /// Hex SHA-256 digest of the full plaintext token.
    pub token_hash: String,
    /// Granted permission scopes.
    pub scopes: Vec<Permission>,
    /// Optional expiration timestamp.
    pub expires_at: Option<DateTime<Utc>>,
}

/// Tool-calling and network security capability flags on an LLM provider.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct LlmProviderFlags {
    /// Whether `OpenAI` tool/function calling is supported (default `true`).
    #[serde(default = "default_true")]
    pub supports_tools: bool,
    /// Whether private/loopback network targets are permitted (`GUARD_ADMIN` required to set `true`).
    #[serde(default)]
    pub allow_private_network: bool,
}

impl Default for LlmProviderFlags {
    fn default() -> Self {
        Self {
            supports_tools: true,
            allow_private_network: false,
        }
    }
}

/// Persisted LLM provider registration (`ax_llm_providers`, `08-STEP09` §09.1–§09.2).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmProviderRecord {
    /// Provider UUID.
    pub id: Uuid,
    /// Owning tenant identifier.
    pub tenant_id: String,
    /// Unique provider name within the tenant.
    pub name: String,
    /// `OpenAI`-compatible base URL.
    pub base_url: String,
    /// Model identifier.
    pub model: String,
    /// Supported workload purposes.
    pub purposes: Vec<Purpose>,
    /// Whether an encrypted API key credential is attached.
    pub has_api_key: bool,
    /// Internal sealed credential reference (omitted from public JSON).
    #[serde(skip_serializing, default)]
    pub credential_id: Option<Uuid>,
    /// Routing priority (lower value wins within a purpose).
    pub priority: i32,
    /// Default sampling temperature.
    pub temperature: f32,
    /// Default maximum completion tokens.
    pub max_tokens: i32,
    /// Request timeout in seconds.
    pub timeout_s: i32,
    /// Optional per-process requests-per-minute token bucket limit.
    pub rpm_limit: Option<i32>,
    /// Optional explicit embedding vector dimension.
    pub embedding_dim: Option<i32>,
    /// Tool and network capability flags (`supports_tools`, `allow_private_network`).
    #[serde(flatten)]
    pub flags: LlmProviderFlags,
    /// Whether this provider is active for routing.
    pub enabled: bool,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
}

/// Request payload for `POST /api/axagent/providers` and `PUT /api/axagent/providers/{id}`.
#[derive(Debug, Clone, Deserialize)]
pub struct SaveLlmProviderRequest {
    /// Unique provider name.
    pub name: String,
    /// `OpenAI`-compatible API base URL.
    pub base_url: String,
    /// Model name.
    pub model: String,
    /// Supported purposes (`CHAT`, `EMBED`, `NL2SQL`, `EXTRACT`, `AGENT`, `SUMMARIZE`).
    pub purposes: Vec<Purpose>,
    /// Optional plaintext API key (`None` on update preserves existing credential; `Some("")` clears).
    #[serde(default)]
    pub api_key: Option<String>,
    /// Priority (default `100`).
    #[serde(default = "default_priority")]
    pub priority: i32,
    /// Sampling temperature (default `0.2`).
    #[serde(default = "default_temperature")]
    pub temperature: f32,
    /// Maximum tokens (default `4096`).
    #[serde(default = "default_max_tokens")]
    pub max_tokens: i32,
    /// Timeout in seconds (default `60`).
    #[serde(default = "default_timeout_s")]
    pub timeout_s: i32,
    /// Optional RPM limit.
    #[serde(default)]
    pub rpm_limit: Option<i32>,
    /// Optional embedding dimension.
    #[serde(default)]
    pub embedding_dim: Option<i32>,
    /// Tool and network capability flags (`supports_tools`, `allow_private_network`).
    #[serde(flatten)]
    pub flags: LlmProviderFlags,
    /// Enabled flag (default `true`).
    #[serde(default = "default_true")]
    pub enabled: bool,
}

/// Credential mutation directive when saving a provider or MCP client.
#[derive(Debug, Clone)]
pub enum CredentialMutation {
    /// Keep existing `credential_id` unchanged.
    Keep,
    /// Detach any bound `credential_id`.
    Clear,
    /// Insert a new `ds_credentials` row and bind its ID.
    Set(SealedCredential),
}

/// Repository input for creating or updating an `ax_llm_providers` row.
#[derive(Debug, Clone)]
pub struct SaveLlmProviderInput {
    /// Provider name.
    pub name: String,
    /// Validated base URL.
    pub base_url: String,
    /// Model identifier.
    pub model: String,
    /// Supported purposes.
    pub purposes: Vec<Purpose>,
    /// Credential update action.
    pub credential: CredentialMutation,
    /// Priority value.
    pub priority: i32,
    /// Temperature value.
    pub temperature: f32,
    /// Max tokens value.
    pub max_tokens: i32,
    /// Timeout seconds.
    pub timeout_s: i32,
    /// Optional RPM limit.
    pub rpm_limit: Option<i32>,
    /// Optional embedding dimension.
    pub embedding_dim: Option<i32>,
    /// Tool and network capability flags (`supports_tools`, `allow_private_network`).
    pub flags: LlmProviderFlags,
    /// Enabled flag.
    pub enabled: bool,
}

/// Response payload for `POST /api/axagent/providers/{id}/test` (`08-STEP09` §09.2).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderTestResponse {
    /// Connectivity outcome.
    pub ok: bool,
    /// Measured round-trip latency in milliseconds.
    pub latency_ms: u64,
    /// Echoed model identifier.
    pub model: String,
    /// Short model reply snippet.
    pub reply: String,
}

/// Query filter for `GET /api/axagent/usage` (`08-STEP09` §09.9).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LlmUsageQuery {
    /// Inclusive start date (`YYYY-MM-DD`).
    #[serde(default)]
    pub from: Option<NaiveDate>,
    /// Inclusive end date (`YYYY-MM-DD`).
    #[serde(default)]
    pub to: Option<NaiveDate>,
}

/// Daily aggregated LLM token and request counters (`ax_llm_usage`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmUsageRecord {
    /// Provider UUID.
    pub provider_id: Uuid,
    /// Provider display name.
    pub provider_name: String,
    /// UTC calendar date.
    pub usage_date: NaiveDate,
    /// Workload purpose.
    pub purpose: Purpose,
    /// Total requests attempted.
    pub requests: i64,
    /// Failed requests count.
    pub errors: i64,
    /// Total prompt tokens consumed.
    pub prompt_tokens: i64,
    /// Total completion tokens generated.
    pub completion_tokens: i64,
}

/// Incremental counter delta persisted into `ax_llm_usage` after an LLM call.
#[derive(Debug, Clone)]
pub struct LlmUsageDelta {
    /// Provider UUID.
    pub provider_id: Uuid,
    /// UTC usage date.
    pub usage_date: NaiveDate,
    /// Workload purpose.
    pub purpose: Purpose,
    /// Request increment (`1`).
    pub requests: i64,
    /// Error increment (`0` or `1`).
    pub errors: i64,
    /// Prompt tokens consumed.
    pub prompt_tokens: i64,
    /// Completion tokens generated.
    pub completion_tokens: i64,
}

/// Published capability endpoint specification (`ax_published_endpoints`, `08-STEP09` §09.3).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EndpointSpec {
    /// Endpoint UUID.
    pub id: Uuid,
    /// Owning tenant identifier.
    pub tenant_id: String,
    /// URL-safe slug (`^[a-z0-9-]{3,64}$`).
    pub slug: String,
    /// Deployment environment (`TEST` or `PROD`).
    pub environment: Environment,
    /// Target capability kind (`SAVED_QUERY`, `METRIC`, `FLOW`, `AGENT`, `OBJECT_TYPE`).
    pub kind: EndpointKind,
    /// Target identifier (saved query UUID, metric name, flow UUID, agent UUID, or main `NodeType.name`).
    pub target_id: String,
    /// Pinned target version in `PROD` (`None` in `TEST` follows live definition).
    pub target_version: Option<i32>,
    /// Frozen definition snapshot in `PROD` (`SavedQuery`, `MetricDef`, or `NodeTypeSpec`).
    pub target_snapshot: Option<serde_json::Value>,
    /// Human-readable title.
    pub title: String,
    /// Optional endpoint description.
    pub description: String,
    /// Draft 2020-12 JSON Schema for request body validation.
    pub input_schema: serde_json::Value,
    /// Output shape (`ROWS`, `OBJECT`, `TEXT`, `RUN`).
    pub output_shape: OutputShape,
    /// Permission scope required on the caller's PAT.
    pub required_scope: Permission,
    /// Per-`(endpoint_id, token_id)` requests-per-minute limit.
    pub rate_limit_rpm: i32,
    /// Response cache TTL in seconds (`0` disables caching).
    pub cache_ttl_s: i32,
    /// Whether this `PROD` endpoint is exposed as an MCP tool on `/api/mcp`.
    pub expose_mcp: bool,
    /// Whether this endpoint renders as an interactive `AppCard` in the Workbench Apps tab.
    pub show_as_app: bool,
    /// Whether external/MCP invocations are enabled.
    pub enabled: bool,
    /// Publisher user UUID.
    pub published_by: Option<Uuid>,
    /// Publication timestamp.
    pub published_at: DateTime<Utc>,
}

/// Query filter for `GET /api/axagent/endpoints`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EndpointListQuery {
    /// Optional environment filter (`TEST` or `PROD`).
    #[serde(default)]
    pub environment: Option<Environment>,
    /// Optional target kind filter.
    #[serde(default)]
    pub kind: Option<EndpointKind>,
    /// Optional `show_as_app` filter.
    #[serde(default)]
    pub show_as_app: Option<bool>,
}

/// Request payload for `POST /api/axagent/endpoints` (`08-STEP09` §09.3.1).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateEndpointRequest {
    /// Endpoint slug (`^[a-z0-9-]{3,64}$`).
    pub slug: String,
    /// Target environment (`TEST` or `PROD`, default `TEST`).
    #[serde(default = "default_test_env")]
    pub environment: Environment,
    /// Capability kind.
    pub kind: EndpointKind,
    /// Target resource identifier.
    pub target_id: String,
    /// Display title.
    pub title: String,
    /// Optional description.
    #[serde(default)]
    pub description: String,
    /// Optional explicit JSON Schema (`None` or `null` auto-generates from target).
    #[serde(default)]
    pub input_schema: Option<serde_json::Value>,
    /// Optional output shape override (`None` inferred from `kind`).
    #[serde(default)]
    pub output_shape: Option<OutputShape>,
    /// Required permission scope (default `AGENT_RUN`).
    #[serde(default = "default_required_scope")]
    pub required_scope: Permission,
    /// Rate limit in RPM (default `60`).
    #[serde(default = "default_rate_limit_rpm")]
    pub rate_limit_rpm: i32,
    /// Cache TTL in seconds (default `0`).
    #[serde(default)]
    pub cache_ttl_s: i32,
    /// Expose as MCP tool (default `false`).
    #[serde(default)]
    pub expose_mcp: bool,
    /// Show in Query-as-an-App tab (default `true`).
    #[serde(default = "default_true")]
    pub show_as_app: bool,
    /// Enabled flag (default `true`).
    #[serde(default = "default_true")]
    pub enabled: bool,
}

const fn default_test_env() -> Environment {
    Environment::Test
}

/// Request payload for `PUT /api/axagent/endpoints/{id}`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateEndpointRequest {
    /// Optional updated title.
    #[serde(default)]
    pub title: Option<String>,
    /// Optional updated description.
    #[serde(default)]
    pub description: Option<String>,
    /// Optional updated JSON Schema.
    #[serde(default)]
    pub input_schema: Option<serde_json::Value>,
    /// Optional updated required permission scope.
    #[serde(default)]
    pub required_scope: Option<Permission>,
    /// Optional updated rate limit RPM.
    #[serde(default)]
    pub rate_limit_rpm: Option<i32>,
    /// Optional updated cache TTL seconds.
    #[serde(default)]
    pub cache_ttl_s: Option<i32>,
    /// Optional updated MCP exposure flag.
    #[serde(default)]
    pub expose_mcp: Option<bool>,
    /// Optional updated app visibility flag.
    #[serde(default)]
    pub show_as_app: Option<bool>,
    /// Optional updated enabled flag.
    #[serde(default)]
    pub enabled: Option<bool>,
}

/// Repository input for inserting or upserting a `PROD`/`TEST` published endpoint.
#[derive(Debug, Clone)]
pub struct SaveEndpointInput {
    /// Endpoint slug.
    pub slug: String,
    /// Environment (`TEST` or `PROD`).
    pub environment: Environment,
    /// Target kind.
    pub kind: EndpointKind,
    /// Target identifier.
    pub target_id: String,
    /// Optional pinned version.
    pub target_version: Option<i32>,
    /// Optional frozen target snapshot JSON.
    pub target_snapshot: Option<serde_json::Value>,
    /// Endpoint title.
    pub title: String,
    /// Endpoint description.
    pub description: String,
    /// Validated input JSON Schema.
    pub input_schema: serde_json::Value,
    /// Output shape (`ROWS`, `OBJECT`, `TEXT`, `RUN`).
    pub output_shape: OutputShape,
    /// Required PAT scope.
    pub required_scope: Permission,
    /// Rate limit RPM.
    pub rate_limit_rpm: i32,
    /// Cache TTL seconds.
    pub cache_ttl_s: i32,
    /// Expose on `/api/mcp`.
    pub expose_mcp: bool,
    /// Show as `AppCard`.
    pub show_as_app: bool,
    /// Enabled flag.
    pub enabled: bool,
    /// Publisher user UUID.
    pub published_by: Option<Uuid>,
}

/// HTTP invocation target inside [`SkillManifest`] (`08-STEP09` §09.3.3).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillInvokeSpec {
    /// HTTP method (`"POST"`).
    pub method: String,
    /// Full or relative published endpoint URL.
    pub url: String,
}

/// External agent skill manifest (`GET /api/axagent/endpoints/{id}/skill.json`, `08-STEP09` §09.3.3).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillManifest {
    /// Tool/skill identifier (`slug` with `-` replaced by `_`).
    pub name: String,
    /// Skill description.
    pub description: String,
    /// Input JSON Schema.
    pub input_schema: serde_json::Value,
    /// HTTP invocation descriptor.
    pub invoke: SkillInvokeSpec,
    /// Authentication scheme (`"bearer"`).
    pub auth: String,
}

/// Cached MCP tool descriptor stored in `ax_mcp_clients.tools_cache` (`08-STEP09` §09.4.2).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolInfo {
    /// Remote MCP tool name.
    pub name: String,
    /// Tool description.
    #[serde(default)]
    pub description: String,
    /// Tool JSON Schema (`inputSchema`).
    #[serde(default = "default_object_schema")]
    pub input_schema: serde_json::Value,
}

fn default_object_schema() -> serde_json::Value {
    serde_json::json!({ "type": "object" })
}

/// Registered external MCP server client (`ax_mcp_clients`, `08-STEP09` §09.4.2).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpClientRecord {
    /// MCP client UUID.
    pub id: Uuid,
    /// Owning tenant identifier.
    pub tenant_id: String,
    /// Short identifier (`^[a-z][a-z0-9_]{0,31}$`).
    pub name: String,
    /// Streamable HTTP endpoint URL.
    pub url: String,
    /// Whether encrypted custom headers / bearer credentials are configured.
    pub has_headers: bool,
    /// Internal sealed credential reference.
    #[serde(skip_serializing, default)]
    pub credential_id: Option<Uuid>,
    /// Allowlist of tool names (empty = all tools allowed).
    pub allowed_tools: Vec<String>,
    /// Cached `tools/list` snapshot from the most recent sync.
    pub tools_cache: Vec<McpToolInfo>,
    /// Allow private/loopback network access (`GUARD_ADMIN` required to set `true`).
    pub allow_private_network: bool,
    /// Whether this MCP client is enabled.
    pub enabled: bool,
    /// Timestamp of most recent successful `/sync`.
    pub last_synced_at: Option<DateTime<Utc>>,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
}

/// Request payload for `POST /api/axagent/mcp-clients`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateMcpClientRequest {
    /// Client machine name (`^[a-z][a-z0-9_]{0,31}$`).
    pub name: String,
    /// Streamable HTTP endpoint URL.
    pub url: String,
    /// Optional custom HTTP headers (e.g., `{"authorization": "Bearer ..."}`) sealed into `ds_credentials`.
    #[serde(default)]
    pub headers: Option<BTreeMap<String, String>>,
    /// Optional allowlist of remote tool names (empty = allow all).
    #[serde(default)]
    pub allowed_tools: Vec<String>,
    /// Allow private network flag (default `false`).
    #[serde(default)]
    pub allow_private_network: bool,
    /// Enabled flag (default `true`).
    #[serde(default = "default_true")]
    pub enabled: bool,
}

/// Repository input for inserting an `ax_mcp_clients` row.
#[derive(Debug, Clone)]
pub struct SaveMcpClientInput {
    /// Client name.
    pub name: String,
    /// Validated endpoint URL.
    pub url: String,
    /// Optional sealed headers credential.
    pub credential: Option<SealedCredential>,
    /// Allowed tool names.
    pub allowed_tools: Vec<String>,
    /// Private network flag.
    pub allow_private_network: bool,
    /// Enabled flag.
    pub enabled: bool,
}

/// Reference to a tool mounted onto an [`AgentSpec`] (`08-STEP09` §09.5.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum ToolRef {
    /// Platform built-in tool (`ask_data`, `search_knowledge`, `run_sql`, `query_objects`, etc.).
    #[serde(rename = "BUILTIN")]
    Builtin {
        /// Built-in tool name.
        name: String,
    },
    /// Published `AxAgent` endpoint exposed as a tool.
    #[serde(rename = "ENDPOINT")]
    Endpoint {
        /// Target `ax_published_endpoints.id`.
        endpoint_id: Uuid,
    },
    /// External MCP server tool registered in `ax_mcp_clients`.
    #[serde(rename = "MCP")]
    Mcp {
        /// `ax_mcp_clients.name`.
        client: String,
        /// Remote tool name.
        tool: String,
    },
    /// Specialist sub-agent delegated via multi-agent supervisor pattern (`08-STEP09` §09.5.3).
    #[serde(rename = "AGENT")]
    Agent {
        /// Specialist `ax_agents.id`.
        agent_id: Uuid,
        /// Tool function name exposed to the supervisor (`^[a-z][a-z0-9_]{0,63}$`).
        as_tool_name: String,
    },
}

/// Retrieval and NL2SQL scoping restrictions on an [`AgentSpec`] (`08-STEP09` §09.5.1).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnowledgeScope {
    /// Optional document title prefix restricting `search_knowledge` recall.
    #[serde(default)]
    pub doc_title_prefix: Option<String>,
    /// Optional `AxHub` dataset names restricting `ask_data` schema linking.
    #[serde(default)]
    pub dataset_names: Vec<String>,
}

/// Declarative configuration of an `AxAgent` assistant (`08-STEP09` §09.5.1).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSpec {
    /// System prompt governing persona, grounding rules, and tool usage.
    #[serde(default)]
    pub system_prompt: String,
    /// Routing purpose (defaults to `AGENT`).
    #[serde(default = "default_agent_purpose")]
    pub provider_purpose: Purpose,
    /// Optional pinned `ax_llm_providers.id` (`None` uses `LlmRouter` priority failover).
    #[serde(default)]
    pub provider_id: Option<Uuid>,
    /// Sampling temperature (default `0.2`).
    #[serde(default = "default_temperature")]
    pub temperature: f32,
    /// Maximum tool-calling loop iterations (`1..=20`, default `8`).
    #[serde(default = "default_max_steps")]
    pub max_steps: u32,
    /// Number of prior conversation turns included in context (default `10`).
    #[serde(default = "default_memory_turns")]
    pub memory_turns: u32,
    /// Mounted tools (built-in, published endpoints, MCP tools, or specialist sub-agents).
    #[serde(default)]
    pub tools: Vec<ToolRef>,
    /// Tool names requiring explicit human approval (`ax_approvals`) before execution.
    #[serde(default)]
    pub approval_required_tools: Vec<String>,
    /// Knowledge and dataset scope constraints.
    #[serde(default)]
    pub knowledge_scope: KnowledgeScope,
    /// Optional JSON Schema enforced on the final assistant answer.
    #[serde(default)]
    pub output_schema: Option<serde_json::Value>,
}

impl Default for AgentSpec {
    fn default() -> Self {
        Self {
            system_prompt: String::new(),
            provider_purpose: default_agent_purpose(),
            provider_id: None,
            temperature: default_temperature(),
            max_steps: default_max_steps(),
            memory_turns: default_memory_turns(),
            tools: Vec::new(),
            approval_required_tools: Vec::new(),
            knowledge_scope: KnowledgeScope::default(),
            output_schema: None,
        }
    }
}

/// Persisted `AxAgent` definition and head published version (`ax_agents`, `08-STEP09` §09.1).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRecord {
    /// Agent UUID.
    pub id: Uuid,
    /// Owning tenant identifier.
    pub tenant_id: String,
    /// Unique agent name within the tenant.
    pub name: String,
    /// Human-readable description.
    pub description: String,
    /// Editable draft [`AgentSpec`].
    pub spec: AgentSpec,
    /// Latest published version (`0` when never published).
    pub version: i32,
    /// Creator user UUID.
    pub created_by: Option<Uuid>,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Last draft update or publish timestamp.
    pub updated_at: DateTime<Utc>,
}

/// Immutable published snapshot of an `AxAgent` (`ax_agent_versions`, `08-STEP09` §09.5.4).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentVersionRecord {
    /// Owning tenant identifier.
    pub tenant_id: String,
    /// Parent agent UUID.
    pub agent_id: Uuid,
    /// 1-based monotonic version number.
    pub version: i32,
    /// Frozen [`AgentSpec`].
    pub spec: AgentSpec,
    /// Publisher user UUID.
    pub published_by: Option<Uuid>,
    /// Publication timestamp.
    pub published_at: DateTime<Utc>,
}

/// Request payload for `POST /api/axagent/agents`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateAgentRequest {
    /// Unique agent name.
    pub name: String,
    /// Optional description.
    #[serde(default)]
    pub description: String,
    /// Initial draft [`AgentSpec`].
    #[serde(default)]
    pub spec: AgentSpec,
}

/// Request payload for `PUT /api/axagent/agents/{id}`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateAgentRequest {
    /// Optional updated name.
    #[serde(default)]
    pub name: Option<String>,
    /// Optional updated description.
    #[serde(default)]
    pub description: Option<String>,
    /// Updated draft [`AgentSpec`].
    pub spec: AgentSpec,
}

/// Multi-turn conversation session (`ax_conversations`, `08-STEP09` §09.1).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationRecord {
    /// Conversation UUID.
    pub id: Uuid,
    /// Owning tenant identifier.
    pub tenant_id: String,
    /// Target agent UUID.
    pub agent_id: Uuid,
    /// Agent version (`0` indicates draft debug mode).
    pub agent_version: i32,
    /// Initiating user UUID.
    pub user_id: Option<Uuid>,
    /// Entry channel (`WORKBENCH`, `EMBED`, `API`, `FLOW`, `MCP`).
    pub channel: ConversationChannel,
    /// Associated `ax_agent_embeds.id` when `channel == EMBED`.
    pub embed_id: Option<Uuid>,
    /// Conversation title (derived from first user message or flow idempotency key).
    pub title: String,
    /// Current status (`ACTIVE`, `WAITING_APPROVAL`, `CLOSED`).
    pub status: ConversationStatus,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Last activity timestamp.
    pub updated_at: DateTime<Utc>,
}

/// Repository input for creating an `ax_conversations` session.
#[derive(Debug, Clone)]
pub struct NewConversationInput {
    /// Target agent UUID.
    pub agent_id: Uuid,
    /// Agent version (`0` indicates draft debug mode).
    pub agent_version: i32,
    /// Optional initiating user UUID (`None` for anonymous embed/system turns).
    pub user_id: Option<Uuid>,
    /// Entry channel (`WORKBENCH`, `EMBED`, `API`, `FLOW`, `MCP`).
    pub channel: ConversationChannel,
    /// Associated `ax_agent_embeds.id` when `channel == EMBED`.
    pub embed_id: Option<Uuid>,
    /// Conversation title (derived from first user message or flow idempotency key).
    pub title: String,
}

/// Query filter for `GET /api/axagent/conversations`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversationListQuery {
    /// Optional filter by `agent_id`.
    #[serde(default)]
    pub agent_id: Option<Uuid>,
    /// Page size (`1..=200`, default `50`).
    #[serde(default)]
    pub limit: Option<u32>,
    /// Opaque pagination cursor.
    #[serde(default)]
    pub cursor: Option<String>,
}

/// Structured tool/function invocation emitted by an assistant turn (`ax_messages.tool_calls`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCallSpec {
    /// Provider-assigned or synthesized call identifier.
    pub id: String,
    /// Invoked tool name.
    pub name: String,
    /// Parsed JSON arguments object.
    pub arguments: serde_json::Value,
}

/// Persisted conversation message (`ax_messages`, `08-STEP09` §09.1).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageRecord {
    /// Monotonic message ID.
    pub id: i64,
    /// Owning tenant identifier.
    pub tenant_id: String,
    /// Parent conversation UUID.
    pub conversation_id: Uuid,
    /// Message role (`USER`, `ASSISTANT`, `TOOL`, `SYSTEM`).
    pub role: MessageRole,
    /// Text content (or JSON string for `TOOL` results).
    pub content: String,
    /// Assistant tool calls, if any.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub tool_calls: Option<Vec<ToolCallSpec>>,
    /// Correlated `ToolCallSpec.id` when `role == TOOL`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub tool_call_id: Option<String>,
    /// Speaker agent name (identifies specialist sub-agents in multi-agent runs).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_name: Option<String>,
    /// LLM provider UUID used for this generation, if backed by `ax_llm_providers`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub provider_id: Option<Uuid>,
    /// Prompt tokens consumed.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub prompt_tokens: Option<i32>,
    /// Completion tokens generated.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub completion_tokens: Option<i32>,
    /// Generation or tool execution latency in milliseconds.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub latency_ms: Option<i32>,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
}

/// Repository input for appending a row into `ax_messages`.
#[derive(Debug, Clone)]
pub struct NewMessageInput {
    /// Parent conversation UUID.
    pub conversation_id: Uuid,
    /// Message role.
    pub role: MessageRole,
    /// Message content.
    pub content: String,
    /// Optional assistant tool calls.
    pub tool_calls: Option<Vec<ToolCallSpec>>,
    /// Optional tool call correlation ID.
    pub tool_call_id: Option<String>,
    /// Optional speaker agent name (`data_analyst`, `knowledge_expert`, etc.).
    pub agent_name: Option<String>,
    /// Optional provider UUID.
    pub provider_id: Option<Uuid>,
    /// Optional prompt token count.
    pub prompt_tokens: Option<i32>,
    /// Optional completion token count.
    pub completion_tokens: Option<i32>,
    /// Optional latency in milliseconds.
    pub latency_ms: Option<i32>,
}

/// Request payload for `POST /api/axagent/agents/{id}/chat` (`08-STEP09` §09.9).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentChatRequest {
    /// User message text.
    pub message: String,
    /// Existing conversation UUID to continue (`None` starts a new conversation).
    #[serde(default)]
    pub conversation_id: Option<Uuid>,
    /// When `true`, executes the agent's unpublished draft `spec` (`AGENT_EDIT` required).
    #[serde(default)]
    pub draft: bool,
}

/// Synchronous JSON response payload for `POST /api/axagent/agents/{id}/chat` and embed chat (`08-STEP09` §09.9).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentChatResponse {
    /// Active conversation UUID.
    pub conversation_id: Uuid,
    /// Conversation status after this turn (`ACTIVE` or `WAITING_APPROVAL`).
    pub status: ConversationStatus,
    /// Final assistant answer text (empty if paused on `WAITING_APPROVAL`).
    pub answer: String,
    /// Parsed structured output when `AgentSpec.output_schema` is configured.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub structured: Option<serde_json::Value>,
    /// Pending `ax_approvals.id` when `status == WAITING_APPROVAL`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub approval_id: Option<Uuid>,
    /// Messages produced or present in this conversation.
    pub messages: Vec<MessageRecord>,
}

/// Streaming Server-Sent Events (`SSE`) frame emitted by `AgentRuntime` (`08-STEP09` §09.5.2).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    /// Incremental text token delta (`event: token`).
    Token {
        /// Text delta chunk.
        delta: String,
        /// Optional speaking agent name.
        #[serde(skip_serializing_if = "Option::is_none", default)]
        agent_name: Option<String>,
    },
    /// Tool invocation started (`event: tool_call`).
    ToolCall {
        /// Tool call ID.
        id: String,
        /// Tool name.
        name: String,
        /// Tool arguments JSON.
        arguments: serde_json::Value,
        /// Optional speaking agent name.
        #[serde(skip_serializing_if = "Option::is_none", default)]
        agent_name: Option<String>,
    },
    /// Tool invocation finished (`event: tool_result`).
    ToolResult {
        /// Correlated tool call ID.
        tool_call_id: String,
        /// Tool name.
        name: String,
        /// Truncated JSON/text result payload.
        content: String,
        /// Whether the result was truncated to 16 KB / 50 rows.
        truncated: bool,
        /// Optional speaking agent name.
        #[serde(skip_serializing_if = "Option::is_none", default)]
        agent_name: Option<String>,
    },
    /// Execution paused awaiting human approval (`event: approval_required`).
    ApprovalRequired {
        /// Created `ax_approvals.id`.
        approval_id: Uuid,
        /// Conversation UUID.
        conversation_id: Uuid,
        /// Paused tool call ID.
        tool_call_id: String,
        /// Paused tool name.
        tool_name: String,
        /// Tool arguments JSON.
        arguments: serde_json::Value,
    },
    /// Turn completed with final answer (`event: final`).
    Final {
        /// Conversation UUID.
        conversation_id: Uuid,
        /// Final assistant answer.
        answer: String,
        /// Optional structured JSON validated against `output_schema`.
        #[serde(skip_serializing_if = "Option::is_none", default)]
        structured: Option<serde_json::Value>,
    },
    /// Terminal error during streaming (`event: error`).
    Error {
        /// Canonical `AppError` code.
        error_code: String,
        /// Public-safe message.
        message: String,
    },
}

impl AgentEvent {
    /// Returns the SSE `event:` name mandated by `08-STEP09` §09.5.2.
    #[must_use]
    pub const fn sse_event_name(&self) -> &'static str {
        match self {
            Self::Token { .. } => "token",
            Self::ToolCall { .. } => "tool_call",
            Self::ToolResult { .. } => "tool_result",
            Self::ApprovalRequired { .. } => "approval_required",
            Self::Final { .. } => "final",
            Self::Error { .. } => "error",
        }
    }
}

/// Persisted external embed link metadata (`ax_agent_embeds`, without secret hash).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentEmbedRecord {
    /// Embed UUID.
    pub id: Uuid,
    /// Owning tenant identifier.
    pub tenant_id: String,
    /// Target agent UUID.
    pub agent_id: Uuid,
    /// Allowed HTTP `Origin` values (empty restricts to same-origin).
    pub allowed_origins: Vec<String>,
    /// Scoped permissions granted to embed visitors.
    pub permissions: Vec<Permission>,
    /// Per-`(embed_id, client_ip)` requests-per-minute limit.
    pub rate_limit_rpm: i32,
    /// Optional expiration timestamp.
    pub expires_at: Option<DateTime<Utc>>,
    /// Revocation timestamp, if revoked.
    pub revoked_at: Option<DateTime<Utc>>,
    /// Creator user UUID.
    pub created_by: Uuid,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
}

/// Request payload for `POST /api/axagent/agents/{id}/embeds` (`08-STEP09` §09.7).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateAgentEmbedRequest {
    /// Allowed browser `Origin` URLs (empty = same-origin only).
    #[serde(default)]
    pub allowed_origins: Vec<String>,
    /// Granted permission subset (`permissions` must be a subset of creator's permissions).
    #[serde(default = "default_embed_permissions")]
    pub permissions: Vec<Permission>,
    /// Optional expiration timestamp.
    #[serde(default)]
    pub expires_at: Option<DateTime<Utc>>,
    /// Per-IP rate limit in RPM (default `20`).
    #[serde(default = "default_embed_rate_limit_rpm")]
    pub rate_limit_rpm: i32,
}

fn default_embed_permissions() -> Vec<Permission> {
    vec![Permission::DataRead, Permission::AgentRun]
}

/// Response payload for `POST /api/axagent/agents/{id}/embeds` returning the one-time `axe_...` token.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateAgentEmbedResponse {
    /// Embed record UUID.
    pub id: Uuid,
    /// Target agent UUID.
    pub agent_id: Uuid,
    /// One-time plaintext embed token (`axe_<32 base62 chars>`).
    pub token: String,
    /// Relative share URL (`/share/agent/<token>`).
    pub url: String,
    /// Allowed HTTP origins.
    pub allowed_origins: Vec<String>,
    /// Granted permissions.
    pub permissions: Vec<Permission>,
    /// Rate limit RPM.
    pub rate_limit_rpm: i32,
    /// Optional expiration timestamp.
    pub expires_at: Option<DateTime<Utc>>,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
}

/// Repository input for inserting an `ax_agent_embeds` row.
#[derive(Debug, Clone)]
pub struct NewAgentEmbedInput {
    /// Target agent UUID.
    pub agent_id: Uuid,
    /// Hex SHA-256 digest of the `axe_...` token.
    pub token_hash: String,
    /// Allowed origins.
    pub allowed_origins: Vec<String>,
    /// Granted permissions.
    pub permissions: Vec<Permission>,
    /// Rate limit RPM.
    pub rate_limit_rpm: i32,
    /// Optional expiration timestamp.
    pub expires_at: Option<DateTime<Utc>>,
    /// Creator user UUID.
    pub created_by: Uuid,
}

/// Cross-tenant embed lookup returned by `SECURITY DEFINER` function `ax_resolve_agent_embed`.
#[derive(Debug, Clone)]
pub struct ResolvedAgentEmbed {
    /// Embed UUID.
    pub embed_id: Uuid,
    /// Owning tenant identifier.
    pub tenant_id: String,
    /// Target agent UUID.
    pub agent_id: Uuid,
    /// Creator user UUID (`user_id` of the synthetic embed principal).
    pub created_by: Uuid,
    /// Granted permission set on the embed link.
    pub permissions: BTreeSet<Permission>,
    /// Allowed HTTP origins.
    pub allowed_origins: Vec<String>,
    /// Rate limit in RPM per `(embed_id, client_ip)`.
    pub rate_limit_rpm: i32,
}

/// Request payload for `POST /api/axagent/embed/{token}/chat` (`08-STEP09` §09.7).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmbedChatRequest {
    /// Visitor message text.
    pub message: String,
    /// Optional in-memory `conversation_id` for multi-turn continuation.
    #[serde(default)]
    pub conversation_id: Option<Uuid>,
}

/// Input parameters for the `run_simulation` built-in agent tool (`SimExecutionPort`, `08-STEP09` §09.0 & §09.5.1).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimRunToolInput {
    /// Optional existing sandbox UUID (`None` creates an ephemeral/new sandbox in Step 10).
    #[serde(default)]
    pub sandbox_id: Option<Uuid>,
    /// Optional scenario name.
    #[serde(default)]
    pub scenario_name: Option<String>,
    /// Interventions array or JSON specification.
    #[serde(default)]
    pub interventions: serde_json::Value,
    /// Optional propagation hop cap.
    #[serde(default)]
    pub max_hops: Option<u32>,
}

/// Structured output returned by [`crate::ports::SimExecutionPort`] (`08-STEP09` §09.0).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimRunToolOutput {
    /// Executed sandbox UUID.
    pub sandbox_id: Uuid,
    /// Executed scenario UUID, if applicable.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub scenario_id: Option<Uuid>,
    /// Structured simulation diff/KPI summary.
    pub summary: serde_json::Value,
}

/// Execution parameters passed from `AxFlow`'s `AGENT_TASK` operator to `AgentTaskPort` (`08-STEP09` §09.6).
#[derive(Debug, Clone)]
pub struct AgentTaskInput {
    /// Tenant identifier.
    pub tenant_id: String,
    /// Initiating user UUID.
    pub user_id: Uuid,
    /// Username for audit attribution.
    pub username: String,
    /// Department identifier.
    pub dept_id: String,
    /// Assigned role names.
    pub roles: Vec<String>,
    /// Effective caller permissions.
    pub permissions: BTreeSet<Permission>,
    /// Parent flow run UUID.
    pub run_id: Uuid,
    /// Flow DAG node identifier.
    pub node_id: String,
    /// Deterministic step idempotency key (`<run_id>:<node_id>`).
    pub idempotency_key: String,
    /// Target `ax_agents.id`.
    pub agent_id: Uuid,
    /// Optional explicit published version (`None` uses latest published version).
    pub version: Option<i32>,
    /// Rendered upstream input JSON (`config.input`).
    pub input: serde_json::Value,
    /// Task instruction prompt (`config.instruction`).
    pub instruction: String,
    /// Optional JSON Schema override for structured output (`config.output_schema`).
    pub output_schema: Option<serde_json::Value>,
    /// Optional step budget override (`config.max_steps`).
    pub max_steps: Option<u32>,
}

/// Outcome returned by `AgentTaskPort` to `AxFlow`'s `AGENT_TASK` operator (`08-STEP09` §09.6).
#[derive(Debug, Clone)]
pub enum AgentTaskOutcome {
    /// Agent conversation completed synchronously.
    Completed {
        /// Created or reused `ax_conversations.id`.
        conversation_id: Uuid,
        /// Final assistant answer text.
        answer: String,
        /// Optional parsed structured JSON output.
        structured: Option<serde_json::Value>,
    },
    /// Agent hit an `approval_required_tools` gate and created a `source_kind = FLOW` approval.
    WaitingApproval {
        /// Paused `ax_conversations.id`.
        conversation_id: Uuid,
        /// Created `ax_approvals.id`.
        approval_id: Uuid,
    },
}

/// Default `OutputShape` inferred from [`EndpointKind`].
#[must_use]
pub const fn default_shape_for_kind(kind: EndpointKind) -> OutputShape {
    match kind {
        EndpointKind::SavedQuery | EndpointKind::Metric => default_output_shape(),
        EndpointKind::ObjectType => OutputShape::Object,
        EndpointKind::Flow => OutputShape::Run,
        EndpointKind::Agent => OutputShape::Text,
    }
}
