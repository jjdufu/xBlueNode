//! Multi-turn `AxAgent` tool-calling runtime, human approval gate (`WAITING_APPROVAL`),
//! `approval.decided` outbox resumption, structured `output_schema` repair, sub-agent delegation,
//! and `AxFlow` `AgentTaskPort` implementation (`08-STEP09` §09.5 & §09.6).

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use axpoint_domain::AppError;
use axpoint_domain::agent::{
    AgentChatRequest, AgentChatResponse, AgentEvent, AgentRecord, AgentSpec, AgentTaskInput,
    AgentTaskOutcome, AgentVersionRecord, ConversationListQuery, ConversationRecord,
    CreateAgentRequest, EndpointSpec, MessageRecord, NewConversationInput, NewMessageInput,
    ToolCallSpec, ToolRef, UpdateAgentRequest,
};
use axpoint_domain::axhub::PaginatedList;
use axpoint_domain::enums::{
    AuditAction, ConversationChannel, ConversationStatus, MessageRole, OutboxTopic, Permission,
    TokenKind,
};
use axpoint_domain::flow::{ApprovalRecord, ApprovalSourceKind, ApprovalStatus, NewApprovalInput};
use axpoint_domain::platform::{AuditRecord, BoxFuture, OutboxEvent, OutboxSubscriber};
use axpoint_domain::ports::{
    AgentRepository, AgentTaskPort, AnalyticsRepository, CredentialStore, FlowRepository,
    GraphQueryPort, GraphStorePort, MetricStorePort, RulesRepository, StudioRepository,
};
use axpoint_security::{AuditWriter, AuthPrincipal};
use chrono::Utc;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::mcp_client::McpClientManager;
use crate::model::{AgentModelPort, AgentModelRequest, AgentModelTurn, ModelMessage, ModelToolDef};
use crate::multi::{
    MAX_SUB_AGENT_DEPTH, build_sub_agent_tool_def, format_sub_agent_prompt,
    validate_sub_agent_graph,
};
use crate::publish::{
    CapabilityPublisher, PublishedAgentCall, PublishedAgentInvoker,
    validate_instance_against_schema,
};
use crate::tools::{BuiltinToolRegistry, is_known_builtin_tool, truncate_tool_output};

const STEP_EXHAUSTED_DIRECTIVE: &str = "已达到最大工具调用步数上限。请不要再调用任何工具，直接基于已有工具结果与上下文给出准确、完整的最终回答。";
const MAX_CONCURRENT_TOOL_CALLS: usize = 4;
const TOOL_EXEC_TIMEOUT_SECS: u64 = 60;
const MAX_TOTAL_MODEL_CALLS_PER_TURN: u32 = 20;

/// Dependency bundle passed to [`AgentRuntime::new`].
pub struct AgentRuntimeDeps<R> {
    /// Tenant-scoped storage repository (`PgStore` in production).
    pub store: Arc<R>,
    /// Tool-calling LLM adapter (`LlmRouter` or `CassetteModelAdapter`).
    pub model: Arc<dyn AgentModelPort>,
    /// Built-in 13-tool registry.
    pub builtin_tools: Arc<BuiltinToolRegistry<R>>,
    /// Published endpoint service (`ToolRef::Endpoint`).
    pub publisher: Arc<CapabilityPublisher<R>>,
    /// Outbound MCP client manager (`ToolRef::Mcp`).
    pub mcp_clients: Arc<McpClientManager<R>>,
    /// Optional asynchronous audit writer.
    pub audit: Option<AuditWriter>,
}

/// Invocation descriptor for channel-specific agent execution (`WORKBENCH`, `API`, `EMBED`).
#[derive(Debug, Clone)]
pub struct ChannelChatCall {
    /// Target `ax_agents.id`.
    pub agent_id: Uuid,
    /// Optional explicit version override (`None` resolves latest published or draft).
    pub explicit_version: Option<i32>,
    /// Optional pinned [`AgentSpec`] snapshot (for `PROD` published endpoints).
    pub explicit_spec: Option<AgentSpec>,
    /// Chat turn request (`message`, `conversation_id`, `draft`).
    pub request: AgentChatRequest,
    /// Originating conversation channel.
    pub channel: ConversationChannel,
    /// Optional `ax_agent_embeds.id` when `channel == EMBED`.
    pub embed_id: Option<Uuid>,
}

#[derive(Clone)]
enum ExecToolBinding {
    Builtin(String),
    Endpoint(Box<EndpointSpec>),
    Mcp {
        client_name: String,
        tool_name: String,
    },
    SubAgent {
        agent_name: String,
        spec: Box<AgentSpec>,
    },
}

struct ResolvedToolTable {
    defs: Vec<ModelToolDef>,
    bindings: HashMap<String, ExecToolBinding>,
}

struct LoopContext<'a> {
    principal: &'a AuthPrincipal,
    conv_id: Uuid,
    agent_id: Uuid,
    agent_version: i32,
    agent_name: String,
    spec: AgentSpec,
    channel: ConversationChannel,
    flow_gate: Option<(Uuid, String)>,
    depth: usize,
    total_model_calls: Arc<AtomicU32>,
}

enum BatchToolOutcome {
    AllExecuted,
    Paused { approval_id: Uuid },
}

/// Multi-turn `AxAgent` runtime coordinating tool visibility, execution, human approvals,
/// sub-agent delegation, and `AxFlow` `AGENT_TASK` steps (`08-STEP09` §09.5 & §09.6).
pub struct AgentRuntime<R> {
    store: Arc<R>,
    model: Arc<dyn AgentModelPort>,
    builtin_tools: Arc<BuiltinToolRegistry<R>>,
    publisher: Arc<CapabilityPublisher<R>>,
    mcp_clients: Arc<McpClientManager<R>>,
    audit: Option<AuditWriter>,
}

impl<R> AgentRuntime<R> {
    /// Creates a new [`AgentRuntime`] from `deps`.
    #[must_use]
    pub fn new(deps: AgentRuntimeDeps<R>) -> Self {
        Self {
            store: deps.store,
            model: deps.model,
            builtin_tools: deps.builtin_tools,
            publisher: deps.publisher,
            mcp_clients: deps.mcp_clients,
            audit: deps.audit,
        }
    }
}

impl<R> AgentRuntime<R>
where
    R: AgentRepository
        + AnalyticsRepository
        + StudioRepository
        + FlowRepository
        + GraphStorePort
        + GraphQueryPort
        + MetricStorePort
        + RulesRepository
        + CredentialStore
        + 'static,
{
    /// Lists all `ax_agents` records in `principal.tenant_id` (`DATA_READ` required).
    ///
    /// # Errors
    /// Returns [`AppError::Forbidden`] if `DATA_READ` is missing, or [`AppError::Database`] on storage error.
    pub async fn list_agents(
        &self,
        principal: &AuthPrincipal,
    ) -> Result<Vec<AgentRecord>, AppError> {
        principal.require(Permission::DataRead)?;
        self.store.list_agents(&principal.tenant_id).await
    }

    /// Loads a single `ax_agents` record (`DATA_READ` required).
    ///
    /// # Errors
    /// Returns [`AppError::Forbidden`] if `DATA_READ` is missing, or [`AppError::NotFound`] when absent.
    pub async fn get_agent(
        &self,
        principal: &AuthPrincipal,
        agent_id: Uuid,
    ) -> Result<AgentRecord, AppError> {
        principal.require(Permission::DataRead)?;
        self.store
            .get_agent(&principal.tenant_id, agent_id)
            .await?
            .ok_or(AppError::NotFound("NOT_FOUND"))
    }

    /// Validates `req.spec` (including sub-agent cycle and `<= 2` depth checks) and creates a draft agent.
    ///
    /// # Errors
    /// Returns [`AppError`] on permission denial (`AGENT_EDIT`), invalid spec/tools, or duplicate name.
    pub async fn create_agent(
        &self,
        principal: &AuthPrincipal,
        req: CreateAgentRequest,
    ) -> Result<AgentRecord, AppError> {
        principal.require(Permission::AgentEdit)?;
        validate_agent_name(&req.name)?;
        self.validate_agent_spec(&principal.tenant_id, None, &req.spec)
            .await?;
        self.store
            .create_agent(&principal.tenant_id, non_nil_uid(principal.user_id), req)
            .await
    }

    /// Validates updated fields on `agent_id` and saves the draft (`AGENT_EDIT` required).
    ///
    /// # Errors
    /// Returns [`AppError`] on permission denial, invalid spec/cycle/depth, or missing agent.
    pub async fn update_agent(
        &self,
        principal: &AuthPrincipal,
        agent_id: Uuid,
        req: UpdateAgentRequest,
    ) -> Result<AgentRecord, AppError> {
        principal.require(Permission::AgentEdit)?;
        if let Some(ref name) = req.name {
            validate_agent_name(name)?;
        }
        self.validate_agent_spec(&principal.tenant_id, Some(agent_id), &req.spec)
            .await?;
        self.store
            .update_agent(&principal.tenant_id, agent_id, req)
            .await?
            .ok_or(AppError::NotFound("NOT_FOUND"))
    }

    /// Deletes `agent_id` (`AGENT_EDIT` required).
    ///
    /// # Errors
    /// Returns [`AppError`] on permission denial or when `agent_id` does not exist.
    pub async fn delete_agent(
        &self,
        principal: &AuthPrincipal,
        agent_id: Uuid,
    ) -> Result<(), AppError> {
        principal.require(Permission::AgentEdit)?;
        let deleted = self
            .store
            .delete_agent(&principal.tenant_id, agent_id)
            .await?;
        if !deleted {
            return Err(AppError::NotFound("NOT_FOUND"));
        }
        Ok(())
    }

    /// Validates the current draft of `agent_id` and freezes an immutable `ax_agent_versions` snapshot (`AGENT_EDIT`).
    ///
    /// # Errors
    /// Returns [`AppError`] on permission denial, missing agent, or invalid `AgentSpec`.
    pub async fn publish_agent(
        &self,
        principal: &AuthPrincipal,
        agent_id: Uuid,
    ) -> Result<AgentVersionRecord, AppError> {
        principal.enforce_no_prod_mutation()?;
        principal.require(Permission::AgentEdit)?;
        let agent = self
            .store
            .get_agent(&principal.tenant_id, agent_id)
            .await?
            .ok_or(AppError::NotFound("NOT_FOUND"))?;
        self.validate_agent_spec(&principal.tenant_id, Some(agent_id), &agent.spec)
            .await?;
        self.store
            .publish_agent_version(
                &principal.tenant_id,
                agent_id,
                non_nil_uid(principal.user_id),
            )
            .await
    }

    /// Lists workbench conversations owned by `principal.user_id` (`AGENT_RUN` required).
    ///
    /// # Errors
    /// Returns [`AppError`] if `AGENT_RUN` is missing or querying storage fails.
    pub async fn list_conversations(
        &self,
        principal: &AuthPrincipal,
        query: ConversationListQuery,
    ) -> Result<PaginatedList<ConversationRecord>, AppError> {
        principal.require(Permission::AgentRun)?;
        self.store
            .list_user_conversations(&principal.tenant_id, principal.user_id, query)
            .await
    }

    /// Lists ordered `ax_messages` in `conversation_id` (`AGENT_RUN` required; restricted to owner or `AGENT_EDIT`).
    ///
    /// # Errors
    /// Returns [`AppError`] on permission denial or when the conversation is absent/foreign.
    pub async fn list_conversation_messages(
        &self,
        principal: &AuthPrincipal,
        conversation_id: Uuid,
    ) -> Result<Vec<MessageRecord>, AppError> {
        principal.require(Permission::AgentRun)?;
        let conv = self
            .store
            .get_conversation(&principal.tenant_id, conversation_id)
            .await?
            .ok_or(AppError::NotFound("NOT_FOUND"))?;
        if conv.user_id != non_nil_uid(principal.user_id) && !principal.has(Permission::AgentEdit) {
            return Err(AppError::NotFound("NOT_FOUND"));
        }
        self.store
            .list_conversation_messages(&principal.tenant_id, conversation_id)
            .await
    }

    /// Deletes a conversation owned by `principal.user_id` (`AGENT_RUN` required).
    ///
    /// # Errors
    /// Returns [`AppError`] on permission denial or when the conversation is absent/foreign.
    pub async fn delete_conversation(
        &self,
        principal: &AuthPrincipal,
        conversation_id: Uuid,
    ) -> Result<(), AppError> {
        principal.require(Permission::AgentRun)?;
        let deleted = self
            .store
            .delete_conversation(
                &principal.tenant_id,
                conversation_id,
                non_nil_uid(principal.user_id),
            )
            .await?;
        if !deleted {
            return Err(AppError::NotFound("NOT_FOUND"));
        }
        Ok(())
    }

    /// Validates an [`AgentSpec`] (bounds, `output_schema`, referenced tools, and sub-agent DAG `<= 2`).
    ///
    /// # Errors
    /// Returns [`AppError::BadRequest`] (`INVALID_ARGUMENT` or `SCHEMA_INVALID`) when validation fails.
    pub async fn validate_agent_spec(
        &self,
        tenant_id: &str,
        agent_id: Option<Uuid>,
        spec: &AgentSpec,
    ) -> Result<(), AppError> {
        if !(1..=20).contains(&spec.max_steps) {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "AgentSpec.max_steps must be in 1..=20",
            ));
        }
        if !spec.temperature.is_finite() || !(0.0_f32..=2.0_f32).contains(&spec.temperature) {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                "AgentSpec.temperature must be in 0.0..=2.0",
            ));
        }
        if let Some(ref schema) = spec.output_schema {
            if !schema.is_object() {
                return Err(AppError::bad(
                    "SCHEMA_INVALID",
                    "AgentSpec.output_schema must be a JSON object",
                ));
            }
            jsonschema::validator_for(schema).map_err(|err| {
                AppError::bad("SCHEMA_INVALID", format!("invalid output_schema: {err}"))
            })?;
        }

        for tool in &spec.tools {
            self.validate_tool_ref(tenant_id, tool).await?;
        }
        validate_sub_agent_graph(self.store.as_ref(), tenant_id, agent_id, spec).await
    }

    async fn validate_tool_ref(&self, tenant_id: &str, tool: &ToolRef) -> Result<(), AppError> {
        match tool {
            ToolRef::Builtin { name } => {
                if !is_known_builtin_tool(name) {
                    return Err(AppError::bad(
                        "INVALID_ARGUMENT",
                        format!("unknown built-in tool '{name}'"),
                    ));
                }
                Ok(())
            }
            ToolRef::Endpoint { endpoint_id } => {
                let exists = self.store.get_endpoint(tenant_id, *endpoint_id).await?;
                if exists.is_none() {
                    return Err(AppError::bad(
                        "INVALID_ARGUMENT",
                        format!("referenced endpoint {endpoint_id} does not exist"),
                    ));
                }
                Ok(())
            }
            ToolRef::Mcp { client, tool } => {
                if client.trim().is_empty() || tool.trim().is_empty() {
                    return Err(AppError::bad(
                        "INVALID_ARGUMENT",
                        "MCP client and tool names cannot be empty",
                    ));
                }
                let exists = self
                    .store
                    .get_mcp_client_by_name(tenant_id, client.trim())
                    .await?;
                if exists.is_none() {
                    return Err(AppError::bad(
                        "INVALID_ARGUMENT",
                        format!("referenced MCP client '{client}' does not exist"),
                    ));
                }
                Ok(())
            }
            ToolRef::Agent { .. } => Ok(()),
        }
    }

    /// Executes a synchronous workbench chat turn (`POST /api/axagent/agents/{id}/chat`, JSON mode).
    ///
    /// # Errors
    /// Returns [`AppError`] on permission denial (`AGENT_RUN`, plus `AGENT_EDIT` when `draft = true`),
    /// unpublished agent, paused conversation conflict, or model/tool error.
    pub async fn chat(
        &self,
        principal: &AuthPrincipal,
        agent_id: Uuid,
        req: AgentChatRequest,
    ) -> Result<AgentChatResponse, AppError> {
        self.chat_in_channel(
            principal,
            ChannelChatCall {
                agent_id,
                explicit_version: None,
                explicit_spec: None,
                request: req,
                channel: ConversationChannel::Workbench,
                embed_id: None,
            },
            None,
        )
        .await
    }

    /// Executes a streaming workbench chat turn forwarding SSE [`AgentEvent`]s into `event_tx`.
    ///
    /// # Errors
    /// Returns [`AppError`] on permission denial, unpublished agent, or model/tool error.
    pub async fn chat_stream(
        &self,
        principal: &AuthPrincipal,
        agent_id: Uuid,
        req: AgentChatRequest,
        event_tx: mpsc::Sender<AgentEvent>,
    ) -> Result<AgentChatResponse, AppError> {
        self.chat_in_channel(
            principal,
            ChannelChatCall {
                agent_id,
                explicit_version: None,
                explicit_spec: None,
                request: req,
                channel: ConversationChannel::Workbench,
                embed_id: None,
            },
            Some(event_tx),
        )
        .await
    }

    /// Executes a multi-turn chat turn across any [`ConversationChannel`] (`WORKBENCH`, `API`, `EMBED`).
    ///
    /// # Errors
    /// Returns [`AppError`] on permission denial, invalid message, conversation state conflict, or runtime failure.
    pub async fn chat_in_channel(
        &self,
        principal: &AuthPrincipal,
        call: ChannelChatCall,
        event_tx: Option<mpsc::Sender<AgentEvent>>,
    ) -> Result<AgentChatResponse, AppError> {
        principal.require(Permission::AgentRun)?;
        if call.request.draft {
            principal.require(Permission::AgentEdit)?;
        }
        let user_msg = call.request.message.trim();
        if user_msg.is_empty() {
            return Err(AppError::bad("INVALID_ARGUMENT", "message cannot be empty"));
        }

        let (agent_name, agent_version, spec) = self.resolve_call_spec(principal, &call).await?;
        let conv = self
            .load_or_create_conversation(principal, &call, agent_version, user_msg)
            .await?;

        let _ = self
            .store
            .append_message(
                &principal.tenant_id,
                plain_message(conv.id, MessageRole::User, String::from(user_msg), None),
            )
            .await?;

        let loop_ctx = LoopContext {
            principal,
            conv_id: conv.id,
            agent_id: call.agent_id,
            agent_version,
            agent_name,
            spec,
            channel: call.channel,
            flow_gate: None,
            depth: 1,
            total_model_calls: Arc::new(AtomicU32::new(0)),
        };
        let resp = self
            .run_loop_on_conversation(&loop_ctx, event_tx.as_ref())
            .await?;

        self.emit_agent_audit(principal, call.agent_id, &resp);
        Ok(resp)
    }

    /// Resumes a `WAITING_APPROVAL` conversation when its `ax_approvals` row is decided (`08-STEP09` §09.5.2).
    ///
    /// Safe and idempotent across duplicate `approval.decided` outbox deliveries.
    ///
    /// # Errors
    /// Returns [`AppError`] if storage updates or resumed model execution fails.
    pub async fn resume_after_approval(
        &self,
        tenant_id: &str,
        approval_id: Uuid,
    ) -> Result<Option<AgentChatResponse>, AppError> {
        let Some(approval) = self.store.get_approval(tenant_id, approval_id).await? else {
            return Ok(None);
        };
        if approval.status == ApprovalStatus::Pending {
            return Ok(None);
        }
        let Some((conv_id, approved_call_id)) = extract_agent_approval_target(&approval) else {
            return Ok(None);
        };
        let Some(conv) = self.store.get_conversation(tenant_id, conv_id).await? else {
            return Ok(None);
        };
        if conv.status != ConversationStatus::WaitingApproval {
            return Ok(None);
        }

        let principal = self
            .rehydrate_approval_principal(tenant_id, &approval)
            .await?;
        let (agent_name, spec) = self
            .load_spec_for_version(tenant_id, conv.agent_id, conv.agent_version)
            .await?;

        let _ = self
            .store
            .update_conversation_status(tenant_id, conv.id, ConversationStatus::Active)
            .await?;

        let loop_ctx = LoopContext {
            principal: &principal,
            conv_id: conv.id,
            agent_id: conv.agent_id,
            agent_version: conv.agent_version,
            agent_name,
            spec,
            channel: conv.channel,
            flow_gate: approval.run_id.zip(approval.node_id.clone()),
            depth: 1,
            total_model_calls: Arc::new(AtomicU32::new(0)),
        };

        if let Some(paused) = self
            .complete_pending_tool_calls(&loop_ctx, &approval, &approved_call_id)
            .await?
        {
            return Ok(Some(paused));
        }

        let resp = self.run_loop_on_conversation(&loop_ctx, None).await?;
        self.emit_agent_audit(&principal, conv.agent_id, &resp);
        Ok(Some(resp))
    }

    async fn complete_pending_tool_calls(
        &self,
        ctx: &LoopContext<'_>,
        approval: &ApprovalRecord,
        approved_call_id: &str,
    ) -> Result<Option<AgentChatResponse>, AppError> {
        let tenant_id = &ctx.principal.tenant_id;
        let history = self
            .store
            .list_conversation_messages(tenant_id, ctx.conv_id)
            .await?;
        let pending_calls = find_unanswered_tool_calls(&history, &ctx.agent_name);
        if pending_calls.is_empty() {
            return Ok(None);
        }

        if approval.status != ApprovalStatus::Approved {
            let err_code = if approval.status == ApprovalStatus::Rejected {
                "APPROVAL_REJECTED"
            } else {
                "APPROVAL_EXPIRED"
            };
            for call in pending_calls {
                let denied_json = serde_json::json!({
                    "approved": false,
                    "error_code": err_code,
                    "message": format!("用户拒绝执行 {}", call.name),
                    "comment": approval.comment,
                });
                let _ = self
                    .store
                    .append_message(
                        tenant_id,
                        tool_message(
                            ctx.conv_id,
                            call.id,
                            denied_json.to_string(),
                            Some(ctx.agent_name.clone()),
                        ),
                    )
                    .await?;
            }
            return Ok(None);
        }

        let table = self.resolve_tool_table(ctx).await?;
        let mut bypassed = HashSet::new();
        bypassed.insert(String::from(approved_call_id));

        match self
            .execute_tool_call_batch(ctx, &table, &pending_calls, &bypassed, None)
            .await?
        {
            BatchToolOutcome::AllExecuted => Ok(None),
            BatchToolOutcome::Paused { approval_id } => {
                let messages = self
                    .store
                    .list_conversation_messages(tenant_id, ctx.conv_id)
                    .await?;
                Ok(Some(AgentChatResponse {
                    conversation_id: ctx.conv_id,
                    status: ConversationStatus::WaitingApproval,
                    answer: String::new(),
                    structured: None,
                    approval_id: Some(approval_id),
                    messages,
                }))
            }
        }
    }

    async fn run_loop_on_conversation(
        &self,
        ctx: &LoopContext<'_>,
        event_tx: Option<&mpsc::Sender<AgentEvent>>,
    ) -> Result<AgentChatResponse, AppError> {
        let tenant_id = &ctx.principal.tenant_id;
        let max_steps = usize::try_from(ctx.spec.max_steps.clamp(1, 20)).unwrap_or(8);
        let table = self.resolve_tool_table(ctx).await?;
        let mut model_messages = self
            .load_model_messages(tenant_id, ctx.conv_id, &ctx.agent_name, &ctx.spec)
            .await?;

        for _step in 0..max_steps {
            if ctx.total_model_calls.load(Ordering::Relaxed)
                >= MAX_TOTAL_MODEL_CALLS_PER_TURN.saturating_sub(1)
            {
                break;
            }
            let start = Instant::now();
            let turn = self
                .call_model_with_optional_stream(ctx, &model_messages, &table.defs, event_tx)
                .await?;
            let latency_ms = i32::try_from(start.elapsed().as_millis()).unwrap_or(i32::MAX);

            if turn.tool_calls.is_empty() {
                return self
                    .finalize_assistant_turn(ctx, &mut model_messages, turn, latency_ms, event_tx)
                    .await;
            }

            let _ = self
                .store
                .append_message(
                    tenant_id,
                    NewMessageInput {
                        conversation_id: ctx.conv_id,
                        role: MessageRole::Assistant,
                        content: turn.content.clone(),
                        tool_calls: Some(turn.tool_calls.clone()),
                        tool_call_id: None,
                        agent_name: Some(ctx.agent_name.clone()),
                        provider_id: ctx.spec.provider_id,
                        prompt_tokens: opt_i32_tokens(turn.prompt_tokens),
                        completion_tokens: opt_i32_tokens(turn.completion_tokens),
                        latency_ms: Some(latency_ms),
                    },
                )
                .await?;
            model_messages.push(ModelMessage::Assistant {
                content: turn.content,
                tool_calls: turn.tool_calls.clone(),
            });

            let empty_bypass = HashSet::new();
            match self
                .execute_tool_call_batch(ctx, &table, &turn.tool_calls, &empty_bypass, event_tx)
                .await?
            {
                BatchToolOutcome::AllExecuted => {
                    model_messages = self
                        .load_model_messages(tenant_id, ctx.conv_id, &ctx.agent_name, &ctx.spec)
                        .await?;
                }
                BatchToolOutcome::Paused { approval_id } => {
                    let messages = self
                        .store
                        .list_conversation_messages(tenant_id, ctx.conv_id)
                        .await?;
                    return Ok(AgentChatResponse {
                        conversation_id: ctx.conv_id,
                        status: ConversationStatus::WaitingApproval,
                        answer: String::new(),
                        structured: None,
                        approval_id: Some(approval_id),
                        messages,
                    });
                }
            }
        }

        self.complete_after_step_exhaustion(ctx, &mut model_messages, event_tx)
            .await
    }

    async fn complete_after_step_exhaustion(
        &self,
        ctx: &LoopContext<'_>,
        model_messages: &mut Vec<ModelMessage>,
        event_tx: Option<&mpsc::Sender<AgentEvent>>,
    ) -> Result<AgentChatResponse, AppError> {
        let tenant_id = &ctx.principal.tenant_id;
        let _ = self
            .store
            .append_message(
                tenant_id,
                plain_message(
                    ctx.conv_id,
                    MessageRole::System,
                    String::from(STEP_EXHAUSTED_DIRECTIVE),
                    Some(ctx.agent_name.clone()),
                ),
            )
            .await?;
        model_messages.push(ModelMessage::System {
            content: String::from(STEP_EXHAUSTED_DIRECTIVE),
        });

        let start = Instant::now();
        let turn = self
            .call_model_with_optional_stream(ctx, model_messages, &[], event_tx)
            .await?;
        let latency_ms = i32::try_from(start.elapsed().as_millis()).unwrap_or(i32::MAX);
        self.finalize_assistant_turn(ctx, model_messages, turn, latency_ms, event_tx)
            .await
    }

    async fn finalize_assistant_turn(
        &self,
        ctx: &LoopContext<'_>,
        model_messages: &mut Vec<ModelMessage>,
        turn: AgentModelTurn,
        latency_ms: i32,
        event_tx: Option<&mpsc::Sender<AgentEvent>>,
    ) -> Result<AgentChatResponse, AppError> {
        let tenant_id = &ctx.principal.tenant_id;
        let (final_answer, structured, p_tok, c_tok) = self
            .validate_or_repair_structured_output(ctx, model_messages, turn)
            .await?;

        let _ = self
            .store
            .append_message(
                tenant_id,
                NewMessageInput {
                    conversation_id: ctx.conv_id,
                    role: MessageRole::Assistant,
                    content: final_answer.clone(),
                    tool_calls: None,
                    tool_call_id: None,
                    agent_name: Some(ctx.agent_name.clone()),
                    provider_id: ctx.spec.provider_id,
                    prompt_tokens: opt_i32_tokens(p_tok),
                    completion_tokens: opt_i32_tokens(c_tok),
                    latency_ms: Some(latency_ms),
                },
            )
            .await?;

        if ctx.depth <= 1 {
            send_event(
                event_tx,
                AgentEvent::Final {
                    conversation_id: ctx.conv_id,
                    answer: final_answer.clone(),
                    structured: structured.clone(),
                },
            )
            .await;
        }

        metrics::counter!(
            "axpoint_agent_turns_total",
            "channel" => ctx.channel.as_str()
        )
        .increment(1);

        let messages = self
            .store
            .list_conversation_messages(tenant_id, ctx.conv_id)
            .await?;
        Ok(AgentChatResponse {
            conversation_id: ctx.conv_id,
            status: ConversationStatus::Active,
            answer: final_answer,
            structured,
            approval_id: None,
            messages,
        })
    }

    async fn validate_or_repair_structured_output(
        &self,
        ctx: &LoopContext<'_>,
        model_messages: &mut Vec<ModelMessage>,
        turn: AgentModelTurn,
    ) -> Result<(String, Option<serde_json::Value>, i64, i64), AppError> {
        let Some(ref schema) = ctx.spec.output_schema else {
            return Ok((
                turn.content,
                None,
                turn.prompt_tokens,
                turn.completion_tokens,
            ));
        };

        if let Ok(val) = parse_and_validate_structured(&turn.content, schema) {
            return Ok((
                turn.content,
                Some(val),
                turn.prompt_tokens,
                turn.completion_tokens,
            ));
        }

        model_messages.push(ModelMessage::Assistant {
            content: turn.content,
            tool_calls: Vec::new(),
        });
        model_messages.push(ModelMessage::User {
            content: format!(
                "上一条回复不符合要求的 JSON Schema。请仅输出合法的 JSON 对象（不要包含 Markdown 代码块），严格满足以下 Schema：{schema}"
            ),
        });

        let _ = ctx.total_model_calls.fetch_add(1, Ordering::Relaxed);
        let retry_turn = self
            .model
            .complete_turn(
                AgentModelRequest {
                    tenant_id: ctx.principal.tenant_id.clone(),
                    purpose: ctx.spec.provider_purpose,
                    provider_id: ctx.spec.provider_id,
                    caller_user_id: Some(ctx.principal.user_id),
                    caller_can_read_raw_pii: ctx.principal.has(Permission::PiiRawRead),
                    temperature: 0.0,
                    messages: model_messages.clone(),
                    tools: Vec::new(),
                    output_schema: Some(schema.clone()),
                },
                None,
            )
            .await?;

        let validated =
            parse_and_validate_structured(&retry_turn.content, schema).map_err(|e| {
                AppError::unprocessable(
                    "SCHEMA_INVALID",
                    format!("agent output failed output_schema validation after repair: {e}"),
                )
            })?;
        Ok((
            retry_turn.content,
            Some(validated),
            turn.prompt_tokens.saturating_add(retry_turn.prompt_tokens),
            turn.completion_tokens
                .saturating_add(retry_turn.completion_tokens),
        ))
    }

    async fn execute_tool_call_batch(
        &self,
        ctx: &LoopContext<'_>,
        table: &ResolvedToolTable,
        calls: &[ToolCallSpec],
        bypassed_call_ids: &HashSet<String>,
        event_tx: Option<&mpsc::Sender<AgentEvent>>,
    ) -> Result<BatchToolOutcome, AppError> {
        let mut ready_slice = calls;
        let mut gated_call: Option<&ToolCallSpec> = None;

        for (idx, call) in calls.iter().enumerate() {
            if !bypassed_call_ids.contains(&call.id)
                && ctx
                    .spec
                    .approval_required_tools
                    .iter()
                    .any(|t| t == &call.name)
            {
                ready_slice = calls.get(..idx).unwrap_or(&[]);
                gated_call = Some(call);
                break;
            }
        }

        if !ready_slice.is_empty() {
            self.run_concurrent_tool_slice(ctx, table, ready_slice, event_tx)
                .await?;
        }

        if let Some(call) = gated_call {
            send_event(
                event_tx,
                AgentEvent::ToolCall {
                    id: call.id.clone(),
                    name: call.name.clone(),
                    arguments: call.arguments.clone(),
                    agent_name: Some(ctx.agent_name.clone()),
                },
            )
            .await;
            if let Some(approval_id) = self.gate_on_human_approval(ctx, call, event_tx).await? {
                return Ok(BatchToolOutcome::Paused { approval_id });
            }
            self.run_concurrent_tool_slice(ctx, table, std::slice::from_ref(call), event_tx)
                .await?;
        }

        Ok(BatchToolOutcome::AllExecuted)
    }

    async fn run_concurrent_tool_slice(
        &self,
        ctx: &LoopContext<'_>,
        table: &ResolvedToolTable,
        calls: &[ToolCallSpec],
        event_tx: Option<&mpsc::Sender<AgentEvent>>,
    ) -> Result<(), AppError> {
        let tenant_id = &ctx.principal.tenant_id;
        for call in calls {
            send_event(
                event_tx,
                AgentEvent::ToolCall {
                    id: call.id.clone(),
                    name: call.name.clone(),
                    arguments: call.arguments.clone(),
                    agent_name: Some(ctx.agent_name.clone()),
                },
            )
            .await;
        }

        let mut outcomes = Vec::with_capacity(calls.len());
        for chunk in calls.chunks(MAX_CONCURRENT_TOOL_CALLS) {
            let batch_futs = chunk.iter().map(|call| async move {
                let timed = tokio::time::timeout(
                    Duration::from_secs(TOOL_EXEC_TIMEOUT_SECS),
                    self.invoke_single_tool(ctx, table, call, event_tx),
                )
                .await;
                let res = match timed {
                    Ok(inner) => inner,
                    Err(_) => Err(AppError::Timeout("UPSTREAM_TIMEOUT")),
                };
                (call, res)
            });
            outcomes.extend(futures::future::join_all(batch_futs).await);
        }

        for (call, raw_result) in outcomes {
            let json_val = match raw_result {
                Ok(val) => val,
                Err(
                    err @ (AppError::Forbidden(_)
                    | AppError::Unauthorized(_)
                    | AppError::RateLimited),
                ) => {
                    return Err(err);
                }
                Err(err) => serde_json::json!({
                    "error": true,
                    "error_code": err.error_code(),
                    "message": err.client_message(),
                }),
            };

            let (content_str, truncated) = truncate_tool_output(json_val);
            let _ = self
                .store
                .append_message(
                    tenant_id,
                    tool_message(
                        ctx.conv_id,
                        call.id.clone(),
                        content_str.clone(),
                        Some(ctx.agent_name.clone()),
                    ),
                )
                .await?;

            send_event(
                event_tx,
                AgentEvent::ToolResult {
                    tool_call_id: call.id.clone(),
                    name: call.name.clone(),
                    content: content_str,
                    truncated,
                    agent_name: Some(ctx.agent_name.clone()),
                },
            )
            .await;
        }
        Ok(())
    }

    async fn gate_on_human_approval(
        &self,
        ctx: &LoopContext<'_>,
        call: &ToolCallSpec,
        event_tx: Option<&mpsc::Sender<AgentEvent>>,
    ) -> Result<Option<Uuid>, AppError> {
        let tenant_id = &ctx.principal.tenant_id;
        let (source_kind, run_id, node_id) = match &ctx.flow_gate {
            Some((rid, nid)) => (ApprovalSourceKind::Flow, Some(*rid), Some(nid.clone())),
            None => (ApprovalSourceKind::Agent, None, None),
        };
        let approval = self
            .store
            .create_or_get_approval(
                tenant_id,
                NewApprovalInput {
                    run_id,
                    node_id,
                    source_kind,
                    source_ref: format!("{}:{}", ctx.conv_id, call.id),
                    title: format!("智能体工具调用审批: {}", call.name),
                    detail: serde_json::json!({
                        "conversation_id": ctx.conv_id,
                        "agent_id": ctx.agent_id,
                        "agent_version": ctx.agent_version,
                        "tool_call_id": call.id,
                        "tool_name": call.name,
                        "arguments": call.arguments,
                        "user_id": ctx.principal.user_id,
                        "username": ctx.principal.username,
                        "dept_id": ctx.principal.dept_id,
                        "roles": ctx.principal.roles,
                        "permissions": ctx.principal.permissions,
                    }),
                    assignee_roles: vec![String::from("ADMIN"), String::from("OPERATOR")],
                    assignee_users: Vec::new(),
                    expires_at: None,
                },
            )
            .await?;

        match approval.status {
            ApprovalStatus::Approved => Ok(None),
            ApprovalStatus::Rejected => Err(AppError::Forbidden("APPROVAL_REJECTED")),
            ApprovalStatus::Expired => Err(AppError::Conflict("APPROVAL_EXPIRED")),
            ApprovalStatus::Pending => {
                let _ = self
                    .store
                    .update_conversation_status(
                        tenant_id,
                        ctx.conv_id,
                        ConversationStatus::WaitingApproval,
                    )
                    .await?;
                send_event(
                    event_tx,
                    AgentEvent::ApprovalRequired {
                        approval_id: approval.id,
                        conversation_id: ctx.conv_id,
                        tool_call_id: call.id.clone(),
                        tool_name: call.name.clone(),
                        arguments: call.arguments.clone(),
                    },
                )
                .await;
                Ok(Some(approval.id))
            }
        }
    }

    async fn invoke_single_tool(
        &self,
        ctx: &LoopContext<'_>,
        table: &ResolvedToolTable,
        call: &ToolCallSpec,
        event_tx: Option<&mpsc::Sender<AgentEvent>>,
    ) -> Result<serde_json::Value, AppError> {
        let Some(binding) = table.bindings.get(&call.name) else {
            return Err(AppError::Forbidden("PERMISSION_DENIED"));
        };
        let idem_prefix = format!("{}:{}", ctx.conv_id, call.id);

        match binding {
            ExecToolBinding::Builtin(name) => {
                self.builtin_tools
                    .execute_builtin(
                        ctx.principal,
                        name,
                        &call.arguments,
                        &ctx.spec.knowledge_scope,
                        &idem_prefix,
                    )
                    .await
            }
            ExecToolBinding::Endpoint(ep) => {
                self.publisher
                    .invoke_for_tool(ctx.principal, ep, call.arguments.clone(), ctx.channel)
                    .await
            }
            ExecToolBinding::Mcp {
                client_name,
                tool_name,
            } => {
                self.mcp_clients
                    .call_external_tool_by_name(
                        &ctx.principal.tenant_id,
                        client_name,
                        tool_name,
                        call.arguments.clone(),
                    )
                    .await
            }
            ExecToolBinding::SubAgent { agent_name, spec } => {
                self.execute_sub_agent_delegation(ctx, agent_name, spec, &call.arguments, event_tx)
                    .await
            }
        }
    }

    fn execute_sub_agent_delegation<'a>(
        &'a self,
        parent_ctx: &'a LoopContext<'_>,
        sub_agent_name: &'a str,
        sub_spec: &'a AgentSpec,
        arguments: &'a serde_json::Value,
        event_tx: Option<&'a mpsc::Sender<AgentEvent>>,
    ) -> BoxFuture<'a, serde_json::Value> {
        Box::pin(async move {
            let next_depth = parent_ctx.depth.saturating_add(1);
            if next_depth > MAX_SUB_AGENT_DEPTH {
                return Err(AppError::bad(
                    "INVALID_ARGUMENT",
                    "SUB_AGENT runtime delegation depth exceeded",
                ));
            }

            let prompt = format_sub_agent_prompt(arguments)?;
            let mut capped_spec = sub_spec.clone();
            capped_spec.max_steps = capped_spec.max_steps.clamp(1, 10);
            capped_spec.approval_required_tools.clear();

            let child_ctx = LoopContext {
                principal: parent_ctx.principal,
                conv_id: parent_ctx.conv_id,
                agent_id: parent_ctx.agent_id,
                agent_version: parent_ctx.agent_version,
                agent_name: String::from(sub_agent_name),
                spec: capped_spec,
                channel: ConversationChannel::Api,
                flow_gate: None,
                depth: next_depth,
                total_model_calls: parent_ctx.total_model_calls.clone(),
            };
            let child_resp = self
                .run_specialist_in_parent_conversation(&child_ctx, prompt, event_tx)
                .await?;
            Ok(serde_json::json!({
                "agent": sub_agent_name,
                "summary": child_resp.answer,
                "structured": child_resp.structured,
            }))
        })
    }

    async fn run_specialist_in_parent_conversation(
        &self,
        ctx: &LoopContext<'_>,
        prompt: String,
        event_tx: Option<&mpsc::Sender<AgentEvent>>,
    ) -> Result<AgentChatResponse, AppError> {
        let tenant_id = &ctx.principal.tenant_id;
        let max_steps = usize::try_from(ctx.spec.max_steps.clamp(1, 10)).unwrap_or(5);
        let table = self.resolve_tool_table(ctx).await?;
        let mut model_messages = vec![
            ModelMessage::System {
                content: build_effective_system_prompt(&ctx.spec),
            },
            ModelMessage::User { content: prompt },
        ];

        for _step in 0..max_steps {
            if ctx.total_model_calls.load(Ordering::Relaxed)
                >= MAX_TOTAL_MODEL_CALLS_PER_TURN.saturating_sub(1)
            {
                break;
            }
            let start = Instant::now();
            let turn = self
                .call_model_with_optional_stream(ctx, &model_messages, &table.defs, event_tx)
                .await?;
            let latency_ms = i32::try_from(start.elapsed().as_millis()).unwrap_or(i32::MAX);

            if turn.tool_calls.is_empty() {
                return self
                    .finalize_assistant_turn(ctx, &mut model_messages, turn, latency_ms, event_tx)
                    .await;
            }

            let _ = self
                .store
                .append_message(
                    tenant_id,
                    NewMessageInput {
                        conversation_id: ctx.conv_id,
                        role: MessageRole::Assistant,
                        content: turn.content.clone(),
                        tool_calls: Some(turn.tool_calls.clone()),
                        tool_call_id: None,
                        agent_name: Some(ctx.agent_name.clone()),
                        provider_id: ctx.spec.provider_id,
                        prompt_tokens: opt_i32_tokens(turn.prompt_tokens),
                        completion_tokens: opt_i32_tokens(turn.completion_tokens),
                        latency_ms: Some(latency_ms),
                    },
                )
                .await?;
            model_messages.push(ModelMessage::Assistant {
                content: turn.content,
                tool_calls: turn.tool_calls.clone(),
            });

            let call_ids: HashSet<String> = turn.tool_calls.iter().map(|c| c.id.clone()).collect();
            let empty_bypass = HashSet::new();
            let _ = self
                .execute_tool_call_batch(ctx, &table, &turn.tool_calls, &empty_bypass, event_tx)
                .await?;
            let after_msgs = self
                .store
                .list_conversation_messages(tenant_id, ctx.conv_id)
                .await?;
            for msg in &after_msgs {
                if msg.role == MessageRole::Tool
                    && msg.agent_name.as_deref() == Some(ctx.agent_name.as_str())
                    && msg
                        .tool_call_id
                        .as_deref()
                        .is_some_and(|id| call_ids.contains(id))
                {
                    model_messages.push(ModelMessage::Tool {
                        tool_call_id: msg.tool_call_id.clone().unwrap_or_default(),
                        content: msg.content.clone(),
                    });
                }
            }
        }

        self.complete_after_step_exhaustion(ctx, &mut model_messages, event_tx)
            .await
    }

    async fn resolve_tool_table(
        &self,
        ctx: &LoopContext<'_>,
    ) -> Result<ResolvedToolTable, AppError> {
        let mut defs = Vec::new();
        let mut bindings = HashMap::new();
        for tool_ref in &ctx.spec.tools {
            self.register_single_tool_ref(ctx, tool_ref, &mut defs, &mut bindings)
                .await?;
        }
        Ok(ResolvedToolTable { defs, bindings })
    }

    async fn register_single_tool_ref(
        &self,
        ctx: &LoopContext<'_>,
        tool_ref: &ToolRef,
        defs: &mut Vec<ModelToolDef>,
        bindings: &mut HashMap<String, ExecToolBinding>,
    ) -> Result<(), AppError> {
        let tenant_id = &ctx.principal.tenant_id;
        match tool_ref {
            ToolRef::Builtin { name } => {
                if let Some(def) = self.builtin_tools.resolve_visible_tool(ctx.principal, name)
                    && !bindings.contains_key(&def.name)
                {
                    bindings.insert(def.name.clone(), ExecToolBinding::Builtin(name.clone()));
                    defs.push(def);
                }
            }
            ToolRef::Endpoint { endpoint_id } => {
                if let Some(ep) = self.store.get_endpoint(tenant_id, *endpoint_id).await?
                    && ep.enabled
                    && ctx.principal.has(ep.required_scope)
                {
                    let tname = ep.slug.replace('-', "_");
                    if let std::collections::hash_map::Entry::Vacant(vacant) =
                        bindings.entry(tname.clone())
                    {
                        let desc = if ep.description.trim().is_empty() {
                            ep.title.clone()
                        } else {
                            ep.description.clone()
                        };
                        defs.push(ModelToolDef {
                            name: tname,
                            description: desc,
                            parameters: ep.input_schema.clone(),
                        });
                        vacant.insert(ExecToolBinding::Endpoint(Box::new(ep)));
                    }
                }
            }
            ToolRef::Mcp { client, tool } => {
                self.register_mcp_tool_ref(tenant_id, client, tool, defs, bindings)
                    .await?;
            }
            ToolRef::Agent {
                agent_id,
                as_tool_name,
            } => {
                if ctx.depth < MAX_SUB_AGENT_DEPTH {
                    self.register_sub_agent_ref(tenant_id, *agent_id, as_tool_name, defs, bindings)
                        .await?;
                }
            }
        }
        Ok(())
    }

    async fn register_mcp_tool_ref(
        &self,
        tenant_id: &str,
        client_name: &str,
        tool_name: &str,
        defs: &mut Vec<ModelToolDef>,
        bindings: &mut HashMap<String, ExecToolBinding>,
    ) -> Result<(), AppError> {
        let Some((client, _)) = self
            .store
            .get_mcp_client_by_name(tenant_id, client_name)
            .await?
        else {
            return Ok(());
        };
        if !client.enabled
            || (!client.allowed_tools.is_empty()
                && !client.allowed_tools.iter().any(|t| t == tool_name))
        {
            return Ok(());
        }
        let cached = client.tools_cache.iter().find(|t| t.name == tool_name);
        let (desc, schema) = match cached {
            Some(info) => (info.description.clone(), info.input_schema.clone()),
            None => (
                format!("External MCP tool {tool_name} on {}", client.name),
                serde_json::json!({ "type": "object" }),
            ),
        };
        let exposed = if bindings.contains_key(tool_name) {
            format!("mcp_{}_{}", client.name, tool_name)
        } else {
            String::from(tool_name)
        };
        defs.push(ModelToolDef {
            name: exposed.clone(),
            description: desc,
            parameters: schema,
        });
        bindings.insert(
            exposed,
            ExecToolBinding::Mcp {
                client_name: client.name,
                tool_name: String::from(tool_name),
            },
        );
        Ok(())
    }

    async fn register_sub_agent_ref(
        &self,
        tenant_id: &str,
        agent_id: Uuid,
        as_tool_name: &str,
        defs: &mut Vec<ModelToolDef>,
        bindings: &mut HashMap<String, ExecToolBinding>,
    ) -> Result<(), AppError> {
        let Some(sub) = self.store.get_agent(tenant_id, agent_id).await? else {
            return Ok(());
        };
        let sub_spec = if sub.version >= 1
            && let Some(ver) = self
                .store
                .get_agent_version(tenant_id, agent_id, sub.version)
                .await?
        {
            ver.spec
        } else {
            sub.spec
        };
        let def = build_sub_agent_tool_def(&sub.name, as_tool_name, &sub.description);
        if !bindings.contains_key(&def.name) {
            bindings.insert(
                def.name.clone(),
                ExecToolBinding::SubAgent {
                    agent_name: sub.name,
                    spec: Box::new(sub_spec),
                },
            );
            defs.push(def);
        }
        Ok(())
    }

    async fn call_model_with_optional_stream(
        &self,
        ctx: &LoopContext<'_>,
        messages: &[ModelMessage],
        tools: &[ModelToolDef],
        event_tx: Option<&mpsc::Sender<AgentEvent>>,
    ) -> Result<AgentModelTurn, AppError> {
        let prev = ctx.total_model_calls.fetch_add(1, Ordering::Relaxed);
        if prev >= MAX_TOTAL_MODEL_CALLS_PER_TURN {
            return Err(AppError::bad(
                "INVALID_ARGUMENT",
                format!(
                    "single request exceeded maximum {MAX_TOTAL_MODEL_CALLS_PER_TURN} total model calls"
                ),
            ));
        }

        let req = AgentModelRequest {
            tenant_id: ctx.principal.tenant_id.clone(),
            purpose: ctx.spec.provider_purpose,
            provider_id: ctx.spec.provider_id,
            caller_user_id: Some(ctx.principal.user_id),
            caller_can_read_raw_pii: ctx.principal.has(Permission::PiiRawRead),
            temperature: ctx.spec.temperature,
            messages: messages.to_vec(),
            tools: tools.to_vec(),
            output_schema: if tools.is_empty() {
                ctx.spec.output_schema.clone()
            } else {
                None
            },
        };

        let Some(sse_tx) = event_tx.cloned() else {
            return self.model.complete_turn(req, None).await;
        };

        let (tok_tx, mut tok_rx) = mpsc::channel::<String>(64);
        let agent_label = Some(ctx.agent_name.clone());
        let forwarder = tokio::spawn(async move {
            while let Some(delta) = tok_rx.recv().await {
                if sse_tx
                    .send(AgentEvent::Token {
                        delta,
                        agent_name: agent_label.clone(),
                    })
                    .await
                    .is_err()
                {
                    break;
                }
            }
        });

        let turn = self.model.complete_turn(req, Some(tok_tx)).await?;
        if let Err(join_err) = forwarder.await {
            tracing::debug!(error = %join_err, "SSE token forwarder task aborted");
        }
        Ok(turn)
    }

    async fn load_model_messages(
        &self,
        tenant_id: &str,
        conv_id: Uuid,
        agent_name: &str,
        spec: &AgentSpec,
    ) -> Result<Vec<ModelMessage>, AppError> {
        let stored = self
            .store
            .list_conversation_messages(tenant_id, conv_id)
            .await?;
        let own_msgs: Vec<MessageRecord> = stored
            .into_iter()
            .filter(|m| m.agent_name.as_deref().is_none_or(|a| a == agent_name))
            .collect();
        let windowed = slice_recent_turns(&own_msgs, spec.memory_turns);

        let mut out = Vec::with_capacity(windowed.len().saturating_add(1));
        out.push(ModelMessage::System {
            content: build_effective_system_prompt(spec),
        });

        for msg in windowed {
            match msg.role {
                MessageRole::System => out.push(ModelMessage::System {
                    content: msg.content.clone(),
                }),
                MessageRole::User => out.push(ModelMessage::User {
                    content: msg.content.clone(),
                }),
                MessageRole::Assistant => out.push(ModelMessage::Assistant {
                    content: msg.content.clone(),
                    tool_calls: msg.tool_calls.clone().unwrap_or_default(),
                }),
                MessageRole::Tool => out.push(ModelMessage::Tool {
                    tool_call_id: msg.tool_call_id.clone().unwrap_or_default(),
                    content: msg.content.clone(),
                }),
            }
        }
        Ok(out)
    }

    async fn resolve_call_spec(
        &self,
        principal: &AuthPrincipal,
        call: &ChannelChatCall,
    ) -> Result<(String, i32, AgentSpec), AppError> {
        let tenant_id = &principal.tenant_id;
        let agent = self
            .store
            .get_agent(tenant_id, call.agent_id)
            .await?
            .ok_or(AppError::NotFound("NOT_FOUND"))?;

        if let (Some(ver), Some(spec)) = (call.explicit_version, call.explicit_spec.clone()) {
            return Ok((agent.name, ver, spec));
        }
        if call.request.draft {
            return Ok((agent.name, 0, agent.spec));
        }
        let target_ver = call.explicit_version.unwrap_or(agent.version);
        if target_ver < 1 {
            return Err(AppError::Conflict("CONFLICT"));
        }
        let ver_rec = self
            .store
            .get_agent_version(tenant_id, call.agent_id, target_ver)
            .await?
            .ok_or(AppError::NotFound("NOT_FOUND"))?;
        Ok((agent.name, ver_rec.version, ver_rec.spec))
    }

    async fn load_spec_for_version(
        &self,
        tenant_id: &str,
        agent_id: Uuid,
        version: i32,
    ) -> Result<(String, AgentSpec), AppError> {
        let agent = self
            .store
            .get_agent(tenant_id, agent_id)
            .await?
            .ok_or(AppError::NotFound("NOT_FOUND"))?;
        if version <= 0 {
            return Ok((agent.name, agent.spec));
        }
        let ver_rec = self
            .store
            .get_agent_version(tenant_id, agent_id, version)
            .await?
            .ok_or(AppError::NotFound("NOT_FOUND"))?;
        Ok((agent.name, ver_rec.spec))
    }

    async fn load_or_create_conversation(
        &self,
        principal: &AuthPrincipal,
        call: &ChannelChatCall,
        agent_version: i32,
        first_message: &str,
    ) -> Result<ConversationRecord, AppError> {
        let tenant_id = &principal.tenant_id;
        if let Some(conv_id) = call.request.conversation_id {
            let conv = self
                .store
                .get_conversation(tenant_id, conv_id)
                .await?
                .ok_or(AppError::NotFound("NOT_FOUND"))?;
            if conv.agent_id != call.agent_id || conv.embed_id != call.embed_id {
                return Err(AppError::NotFound("NOT_FOUND"));
            }
            if call.channel != ConversationChannel::Embed
                && conv.user_id.is_some()
                && conv.user_id != non_nil_uid(principal.user_id)
            {
                return Err(AppError::NotFound("NOT_FOUND"));
            }
            if conv.status != ConversationStatus::Active {
                return Err(AppError::Conflict("CONFLICT"));
            }
            return Ok(conv);
        }

        let owner_uid = if call.channel == ConversationChannel::Embed {
            None
        } else {
            non_nil_uid(principal.user_id)
        };
        let title: String = first_message.chars().take(60).collect();
        self.store
            .create_conversation(
                tenant_id,
                NewConversationInput {
                    agent_id: call.agent_id,
                    agent_version,
                    user_id: owner_uid,
                    channel: call.channel,
                    embed_id: call.embed_id,
                    im_connector_id: None,
                    external_chat_id: None,
                    title,
                },
            )
            .await
    }

    async fn rehydrate_approval_principal(
        &self,
        tenant_id: &str,
        approval: &ApprovalRecord,
    ) -> Result<AuthPrincipal, AppError> {
        let saved_uid = approval
            .detail
            .get("user_id")
            .and_then(serde_json::Value::as_str)
            .and_then(|s| Uuid::parse_str(s).ok())
            .unwrap_or_else(Uuid::nil);

        if !saved_uid.is_nil()
            && let Some(rec) = self.store.load_principal(tenant_id, saved_uid).await?
        {
            return Ok(AuthPrincipal {
                user_id: rec.user_id,
                tenant_id: rec.tenant_id,
                username: rec.username,
                dept_id: rec.dept_id,
                roles: rec.roles,
                permissions: rec.permissions,
                token_kind: TokenKind::Access,
                token_profile: axpoint_domain::enums::TokenProfile::General,
                web_base_url: None,
            });
        }

        let username = approval
            .detail
            .get("username")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("system")
            .to_owned();
        let dept_id = approval
            .detail
            .get("dept_id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("default")
            .to_owned();
        let roles: Vec<String> = approval
            .detail
            .get("roles")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        let permissions: BTreeSet<Permission> = approval
            .detail
            .get("permissions")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();

        Ok(AuthPrincipal {
            user_id: saved_uid,
            tenant_id: String::from(tenant_id),
            username,
            dept_id,
            roles,
            permissions,
            token_kind: TokenKind::Access,
            token_profile: axpoint_domain::enums::TokenProfile::General,
            web_base_url: None,
        })
    }

    fn emit_agent_audit(
        &self,
        principal: &AuthPrincipal,
        agent_id: Uuid,
        resp: &AgentChatResponse,
    ) {
        let Some(ref writer) = self.audit else {
            return;
        };
        writer.record(AuditRecord {
            tenant_id: principal.tenant_id.clone(),
            user_id: non_nil_uid(principal.user_id),
            username: principal.username.clone(),
            action: AuditAction::AgentChat,
            resource_ref: format!("agent:{agent_id}"),
            status_code: 200,
            client_ip: String::from("127.0.0.1"),
            trace_id: String::new(),
            detail_json: serde_json::json!({
                "conversation_id": resp.conversation_id,
                "status": resp.status,
                "approval_id": resp.approval_id,
            }),
            created_at: Utc::now(),
        });
    }
}

impl<R> PublishedAgentInvoker for AgentRuntime<R>
where
    R: AgentRepository
        + AnalyticsRepository
        + StudioRepository
        + FlowRepository
        + GraphStorePort
        + GraphQueryPort
        + MetricStorePort
        + RulesRepository
        + CredentialStore
        + 'static,
{
    fn invoke_published_agent<'a>(
        &'a self,
        principal: &'a AuthPrincipal,
        call: PublishedAgentCall,
        event_tx: Option<mpsc::Sender<AgentEvent>>,
    ) -> BoxFuture<'a, AgentChatResponse> {
        Box::pin(async move {
            self.chat_in_channel(
                principal,
                ChannelChatCall {
                    agent_id: call.agent_id,
                    explicit_version: call.pinned_version,
                    explicit_spec: call.pinned_spec,
                    request: call.request,
                    channel: call.channel,
                    embed_id: None,
                },
                event_tx,
            )
            .await
        })
    }
}

impl<R> AgentTaskPort for AgentRuntime<R>
where
    R: AgentRepository
        + AnalyticsRepository
        + StudioRepository
        + FlowRepository
        + GraphStorePort
        + GraphQueryPort
        + MetricStorePort
        + RulesRepository
        + CredentialStore
        + 'static,
{
    fn execute_flow_agent_task(&self, input: AgentTaskInput) -> BoxFuture<'_, AgentTaskOutcome> {
        Box::pin(async move {
            let idem_title = format!("flow:{}", input.idempotency_key);
            if let Some(reused) = self
                .try_reuse_completed_flow_task(&input, &idem_title)
                .await?
            {
                return Ok(reused);
            }

            let (agent_name, target_ver, mut spec) = self
                .load_published_flow_agent_spec(&input.tenant_id, input.agent_id, input.version)
                .await?;
            if let Some(ref schema_override) = input.output_schema {
                spec.output_schema = Some(schema_override.clone());
            }
            if let Some(steps_override) = input.max_steps {
                spec.max_steps = steps_override.clamp(1, 20);
            }

            let conv = self
                .prepare_flow_conversation(&input, target_ver, &idem_title)
                .await?;
            let principal = AuthPrincipal {
                user_id: input.user_id,
                tenant_id: input.tenant_id.clone(),
                username: input.username.clone(),
                dept_id: input.dept_id.clone(),
                roles: input.roles.clone(),
                permissions: input.permissions.clone(),
                token_kind: TokenKind::Access,
                token_profile: axpoint_domain::enums::TokenProfile::General,
                web_base_url: None,
            };

            let loop_ctx = LoopContext {
                principal: &principal,
                conv_id: conv.id,
                agent_id: input.agent_id,
                agent_version: target_ver,
                agent_name,
                spec,
                channel: ConversationChannel::Flow,
                flow_gate: Some((input.run_id, input.node_id)),
                depth: 1,
                total_model_calls: Arc::new(AtomicU32::new(0)),
            };
            let resp = self.run_loop_on_conversation(&loop_ctx, None).await?;
            if let (ConversationStatus::WaitingApproval, Some(approval_id)) =
                (resp.status, resp.approval_id)
            {
                Ok(AgentTaskOutcome::WaitingApproval {
                    conversation_id: resp.conversation_id,
                    approval_id,
                })
            } else {
                Ok(AgentTaskOutcome::Completed {
                    conversation_id: resp.conversation_id,
                    answer: resp.answer,
                    structured: resp.structured,
                })
            }
        })
    }
}

impl<R> AgentRuntime<R>
where
    R: AgentRepository + 'static,
{
    async fn try_reuse_completed_flow_task(
        &self,
        input: &AgentTaskInput,
        idem_title: &str,
    ) -> Result<Option<AgentTaskOutcome>, AppError> {
        let Some(existing) = self
            .store
            .find_flow_conversation(&input.tenant_id, input.agent_id, idem_title)
            .await?
        else {
            return Ok(None);
        };
        if existing.status != ConversationStatus::Active {
            return Ok(None);
        }
        let msgs = self
            .store
            .list_conversation_messages(&input.tenant_id, existing.id)
            .await?;
        let Some(last) = msgs.last() else {
            return Ok(None);
        };
        if last.role != MessageRole::Assistant || last.tool_calls.is_some() {
            return Ok(None);
        }
        let structured = match &input.output_schema {
            Some(schema) => parse_and_validate_structured(&last.content, schema).ok(),
            None => None,
        };
        Ok(Some(AgentTaskOutcome::Completed {
            conversation_id: existing.id,
            answer: last.content.clone(),
            structured,
        }))
    }

    async fn load_published_flow_agent_spec(
        &self,
        tenant_id: &str,
        agent_id: Uuid,
        explicit_ver: Option<i32>,
    ) -> Result<(String, i32, AgentSpec), AppError> {
        let agent = self
            .store
            .get_agent(tenant_id, agent_id)
            .await?
            .ok_or(AppError::NotFound("NOT_FOUND"))?;
        let target_ver = explicit_ver.unwrap_or(agent.version);
        if target_ver < 1 {
            return Err(AppError::Conflict("CONFLICT"));
        }
        let ver_rec = self
            .store
            .get_agent_version(tenant_id, agent_id, target_ver)
            .await?
            .ok_or(AppError::NotFound("NOT_FOUND"))?;
        Ok((agent.name, ver_rec.version, ver_rec.spec))
    }

    async fn prepare_flow_conversation(
        &self,
        input: &AgentTaskInput,
        target_ver: i32,
        idem_title: &str,
    ) -> Result<ConversationRecord, AppError> {
        if let Some(existing) = self
            .store
            .find_flow_conversation(&input.tenant_id, input.agent_id, idem_title)
            .await?
        {
            if existing.status != ConversationStatus::Active {
                let _ = self
                    .store
                    .update_conversation_status(
                        &input.tenant_id,
                        existing.id,
                        ConversationStatus::Active,
                    )
                    .await?;
            }
            return Ok(existing);
        }

        let conv = self
            .store
            .create_conversation(
                &input.tenant_id,
                NewConversationInput {
                    agent_id: input.agent_id,
                    agent_version: target_ver,
                    user_id: non_nil_uid(input.user_id),
                    channel: ConversationChannel::Flow,
                    embed_id: None,
                    im_connector_id: None,
                    external_chat_id: None,
                    title: String::from(idem_title),
                },
            )
            .await?;

        let user_prompt = if input.input.is_null() {
            input.instruction.clone()
        } else {
            format!(
                "{}\n\n输入数据：\n{}",
                input.instruction,
                serde_json::to_string_pretty(&input.input).unwrap_or_else(|_| String::from("{}"))
            )
        };
        let _ = self
            .store
            .append_message(
                &input.tenant_id,
                plain_message(conv.id, MessageRole::User, user_prompt, None),
            )
            .await?;
        Ok(conv)
    }
}

/// Outbox subscriber resuming `WAITING_APPROVAL` agent conversations on `approval.decided` events (`08-STEP09` §09.5.2).
pub struct AgentApprovalDecidedSubscriber<R> {
    runtime: Arc<AgentRuntime<R>>,
}

impl<R> AgentApprovalDecidedSubscriber<R> {
    /// Creates a new [`AgentApprovalDecidedSubscriber`] wrapping `runtime`.
    #[must_use]
    pub const fn new(runtime: Arc<AgentRuntime<R>>) -> Self {
        Self { runtime }
    }
}

impl<R> OutboxSubscriber for AgentApprovalDecidedSubscriber<R>
where
    R: AgentRepository
        + AnalyticsRepository
        + StudioRepository
        + FlowRepository
        + GraphStorePort
        + GraphQueryPort
        + MetricStorePort
        + RulesRepository
        + CredentialStore
        + 'static,
{
    fn on_event<'a>(&'a self, event: &'a OutboxEvent) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            if event.topic != OutboxTopic::ApprovalDecided {
                return Ok(());
            }
            let Some(approval_id) = event
                .payload
                .get("approval_id")
                .and_then(serde_json::Value::as_str)
                .and_then(|s| Uuid::parse_str(s).ok())
            else {
                return Ok(());
            };
            let _ = self
                .runtime
                .resume_after_approval(&event.tenant_id, approval_id)
                .await?;
            Ok(())
        })
    }
}

fn plain_message(
    conversation_id: Uuid,
    role: MessageRole,
    content: String,
    agent_name: Option<String>,
) -> NewMessageInput {
    NewMessageInput {
        conversation_id,
        role,
        content,
        tool_calls: None,
        tool_call_id: None,
        agent_name,
        provider_id: None,
        prompt_tokens: None,
        completion_tokens: None,
        latency_ms: None,
    }
}

fn tool_message(
    conversation_id: Uuid,
    tool_call_id: String,
    content: String,
    agent_name: Option<String>,
) -> NewMessageInput {
    NewMessageInput {
        conversation_id,
        role: MessageRole::Tool,
        content,
        tool_calls: None,
        tool_call_id: Some(tool_call_id),
        agent_name,
        provider_id: None,
        prompt_tokens: None,
        completion_tokens: None,
        latency_ms: None,
    }
}

fn opt_i32_tokens(val: i64) -> Option<i32> {
    if val <= 0 {
        None
    } else {
        Some(i32::try_from(val).unwrap_or(i32::MAX))
    }
}

fn slice_recent_turns(stored: &[MessageRecord], memory_turns: u32) -> &[MessageRecord] {
    if memory_turns == 0 {
        return stored;
    }
    let max_user_turns = usize::try_from(memory_turns).unwrap_or(10).max(1);
    let mut user_seen = 0_usize;
    let mut start_idx = 0_usize;
    for (idx, msg) in stored.iter().enumerate().rev() {
        if msg.role == MessageRole::User {
            user_seen = user_seen.saturating_add(1);
            if user_seen >= max_user_turns {
                start_idx = idx;
                break;
            }
        }
    }
    stored.get(start_idx..).unwrap_or(stored)
}

fn validate_agent_name(raw: &str) -> Result<(), AppError> {
    let trimmed = raw.trim();
    let bytes = trimmed.as_bytes();
    let Some(&first) = bytes.first() else {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            "agent name cannot be empty",
        ));
    };
    if bytes.len() > 63
        || !first.is_ascii_lowercase()
        || !bytes
            .iter()
            .all(|&b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
    {
        return Err(AppError::bad(
            "INVALID_ARGUMENT",
            format!("invalid agent name '{trimmed}': must match ^[a-z][a-z0-9_-]{{0,62}}$"),
        ));
    }
    Ok(())
}

fn non_nil_uid(uid: Uuid) -> Option<Uuid> {
    if uid.is_nil() { None } else { Some(uid) }
}

fn build_effective_system_prompt(spec: &AgentSpec) -> String {
    let base = if spec.system_prompt.trim().is_empty() {
        String::from("你是 AxPoint 智能助手，请基于真实工具返回的数据与知识回答，严禁编造数据。")
    } else {
        spec.system_prompt.clone()
    };
    match &spec.output_schema {
        Some(schema) => format!(
            "{base}\n\n最终回复必须是符合以下 JSON Schema 的纯 JSON 对象（不含 Markdown 代码围栏）：\n{schema}"
        ),
        None => base,
    }
}

fn parse_and_validate_structured(
    raw: &str,
    schema: &serde_json::Value,
) -> Result<serde_json::Value, AppError> {
    let trimmed = strip_json_fence(raw.trim());
    let parsed: serde_json::Value = serde_json::from_str(trimmed)
        .map_err(|e| AppError::bad("SCHEMA_INVALID", format!("invalid JSON output: {e}")))?;
    validate_instance_against_schema(&parsed, schema)?;
    Ok(parsed)
}

fn strip_json_fence(s: &str) -> &str {
    let without_start = s
        .strip_prefix("```json")
        .or_else(|| s.strip_prefix("```JSON"))
        .or_else(|| s.strip_prefix("```"));
    match without_start {
        Some(inner) => inner.trim().strip_suffix("```").unwrap_or(inner).trim(),
        None => s,
    }
}

fn extract_agent_approval_target(approval: &ApprovalRecord) -> Option<(Uuid, String)> {
    if let (Some(cid_str), Some(tid_str)) = (
        approval
            .detail
            .get("conversation_id")
            .and_then(serde_json::Value::as_str),
        approval
            .detail
            .get("tool_call_id")
            .and_then(serde_json::Value::as_str),
    ) && let Ok(cid) = Uuid::parse_str(cid_str)
    {
        return Some((cid, String::from(tid_str)));
    }

    if approval.source_kind == ApprovalSourceKind::Agent
        && let Some((left, right)) = approval.source_ref.split_once(':')
        && let Ok(cid) = Uuid::parse_str(left)
    {
        return Some((cid, String::from(right)));
    }
    None
}

fn find_unanswered_tool_calls(history: &[MessageRecord], agent_name: &str) -> Vec<ToolCallSpec> {
    let answered: HashSet<&str> = history
        .iter()
        .filter(|m| {
            m.role == MessageRole::Tool && m.agent_name.as_deref().is_none_or(|a| a == agent_name)
        })
        .filter_map(|m| m.tool_call_id.as_deref())
        .collect();

    let Some(last_assistant_with_tools) = history.iter().rev().find(|m| {
        m.role == MessageRole::Assistant
            && m.tool_calls.is_some()
            && m.agent_name.as_deref().is_none_or(|a| a == agent_name)
    }) else {
        return Vec::new();
    };

    last_assistant_with_tools
        .tool_calls
        .as_ref()
        .map_or_default(|calls| {
            calls
                .iter()
                .filter(|c| !answered.contains(c.id.as_str()))
                .cloned()
                .collect()
        })
}

async fn send_event(tx: Option<&mpsc::Sender<AgentEvent>>, event: AgentEvent) {
    if let Some(sender) = tx
        && sender.send(event).await.is_err()
    {
        tracing::debug!("AgentEvent SSE receiver closed");
    }
}
