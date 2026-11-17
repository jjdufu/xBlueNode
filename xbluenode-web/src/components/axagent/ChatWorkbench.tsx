import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import {
  AlertTriangle,
  Bot,
  CheckCircle2,
  Cpu,
  MessageSquarePlus,
  Send,
  ShieldAlert,
  Square,
  Trash2,
  XCircle,
} from "lucide-react";
import { axflowApi } from "@/lib/api/axflow";
import { ApiError } from "@/lib/api/client";
import {
  chatWithAgent,
  deleteConversation,
  getConversationMessages,
  listConversations,
  streamAgentChat,
  type AgentEvent,
  type AgentRecord,
  type ConversationRecord,
  type ConversationStatus,
  type MessageRecord,
} from "@/lib/api/axagent";
import { Badge, Button, Card, Input, Textarea } from "@/components/ui/primitives";
import { cn } from "@/lib/utils";
import { MessageView, type StreamingToolItem } from "./MessageView";

export interface ChatWorkbenchProps {
  agents: AgentRecord[];
  selectedAgentId: string | null;
  onSelectAgentId: (id: string) => void;
}

interface PendingApprovalState {
  approvalId: string;
  conversationId: string;
  toolName: string;
  arguments: Record<string, unknown>;
}

export function ChatWorkbench({
  agents,
  selectedAgentId,
  onSelectAgentId,
}: ChatWorkbenchProps) {
  const queryClient = useQueryClient();

  const effectiveAgent = useMemo(() => {
    if (agents.length === 0) return null;
    if (selectedAgentId) {
      const found = agents.find((a) => a.id === selectedAgentId);
      if (found) return found;
    }
    return (
      agents.find((a) => a.name === "ops_supervisor") ??
      agents.find((a) => a.version > 0) ??
      agents[0] ??
      null
    );
  }, [agents, selectedAgentId]);

  useEffect(() => {
    if (effectiveAgent && effectiveAgent.id !== selectedAgentId) {
      onSelectAgentId(effectiveAgent.id);
    }
  }, [effectiveAgent, onSelectAgentId, selectedAgentId]);

  const [useDraft, setUseDraft] = useState(false);
  const [activeConversationId, setActiveConversationId] = useState<
    string | null
  >(null);
  const [conversationStatus, setConversationStatus] =
    useState<ConversationStatus>("ACTIVE");
  const [messages, setMessages] = useState<MessageRecord[]>([]);
  const [inputText, setInputText] = useState("");
  const [isStreaming, setIsStreaming] = useState(false);
  const [streamingText, setStreamingText] = useState("");
  const [streamingTools, setStreamingTools] = useState<StreamingToolItem[]>([]);
  const [activeSpecialist, setActiveSpecialist] = useState<string | null>(null);
  const [structuredOutput, setStructuredOutput] = useState<Record<
    string,
    unknown
  > | null>(null);
  const [pendingApproval, setPendingApproval] =
    useState<PendingApprovalState | null>(null);
  const [approvalComment, setApprovalComment] = useState("");
  const [decidingApproval, setDecidingApproval] = useState(false);
  const [errorBanner, setErrorBanner] = useState<string | null>(null);

  const abortRef = useRef<AbortController | null>(null);
  const nextLocalMsgIdRef = useRef<number>(-1);

  const conversationsQuery = useQuery({
    queryKey: ["axagent", "conversations", effectiveAgent?.id ?? "none"],
    queryFn: () =>
      effectiveAgent
        ? listConversations({ agent_id: effectiveAgent.id, limit: 50 })
        : Promise.resolve([]),
    enabled: Boolean(effectiveAgent),
  });

  const conversations: ConversationRecord[] = useMemo(
    () => conversationsQuery.data ?? [],
    [conversationsQuery.data]
  );

  const handleNewConversation = useCallback(() => {
    abortRef.current?.abort();
    abortRef.current = null;
    setIsStreaming(false);
    setActiveConversationId(null);
    setConversationStatus("ACTIVE");
    setMessages([]);
    setStreamingText("");
    setStreamingTools([]);
    setActiveSpecialist(null);
    setStructuredOutput(null);
    setPendingApproval(null);
    setErrorBanner(null);
  }, []);

  const handleSelectConversation = useCallback(
    async (conv: ConversationRecord) => {
      abortRef.current?.abort();
      abortRef.current = null;
      setIsStreaming(false);
      setStreamingText("");
      setStreamingTools([]);
      setErrorBanner(null);
      setActiveConversationId(conv.id);
      setConversationStatus(conv.status);
      setPendingApproval(null);

      try {
        const persisted = await getConversationMessages(conv.id);
        setMessages(persisted);
        if (conv.status === "WAITING_APPROVAL") {
          try {
            const approvalsPage = await axflowApi.listApprovals({
              status: "PENDING",
              limit: 50,
            });
            const matched = approvalsPage.items.find(
              (item) =>
                item.source_kind === "AGENT" && item.source_ref === conv.id
            );
            if (matched) {
              setPendingApproval({
                approvalId: matched.id,
                conversationId: conv.id,
                toolName:
                  typeof matched.detail.tool_name === "string"
                    ? matched.detail.tool_name
                    : matched.title,
                arguments: matched.detail,
              });
            }
          } catch {
            // Non-blocking if user lacks APPROVAL_DECIDE.
          }
        }
      } catch (err) {
        setErrorBanner(
          err instanceof Error ? err.message : "加载历史会话消息失败"
        );
      }
    },
    []
  );

  const handleDeleteConversation = useCallback(
    async (convId: string) => {
      try {
        await deleteConversation(convId);
        if (activeConversationId === convId) {
          handleNewConversation();
        }
        await queryClient.invalidateQueries({
          queryKey: ["axagent", "conversations"],
        });
      } catch (err) {
        setErrorBanner(err instanceof Error ? err.message : "删除会话失败");
      }
    },
    [activeConversationId, handleNewConversation, queryClient]
  );

  const handleSendMessage = useCallback(async () => {
    const trimmed = inputText.trim();
    if (!trimmed || !effectiveAgent || isStreaming) {
      return;
    }

    setErrorBanner(null);
    setInputText("");
    setStructuredOutput(null);

    const nowIso = new Date().toISOString();
    const optimisticUserId = nextLocalMsgIdRef.current--;
    const userMsg: MessageRecord = {
      id: optimisticUserId,
      tenant_id: effectiveAgent.tenant_id,
      conversation_id: activeConversationId ?? "pending",
      role: "USER",
      content: trimmed,
      created_at: nowIso,
    };

    setMessages((prev) => [...prev, userMsg]);
    setIsStreaming(true);
    setStreamingText("");
    setStreamingTools([]);

    const controller = new AbortController();
    abortRef.current = controller;

    let resolvedConvId = activeConversationId;
    let accText = "";
    let finalAnswer = "";
    let lastSpeaker: string | undefined;
    let receivedAnyEvent = false;
    let pausedForApproval = false;
    const accTools: StreamingToolItem[] = [];

    const onStreamEvent = (ev: AgentEvent) => {
      receivedAnyEvent = true;
      switch (ev.type) {
        case "conversation_started": {
          resolvedConvId = ev.conversation_id;
          setActiveConversationId(ev.conversation_id);
          break;
        }
        case "agent_switched": {
          lastSpeaker = ev.agent_name;
          setActiveSpecialist(ev.agent_name);
          break;
        }
        case "token": {
          accText += ev.delta;
          if (ev.agent_name) {
            lastSpeaker = ev.agent_name;
            setActiveSpecialist(ev.agent_name);
          }
          setStreamingText(accText);
          break;
        }
        case "tool_call": {
          if (ev.agent_name) {
            lastSpeaker = ev.agent_name;
            setActiveSpecialist(ev.agent_name);
          }
          const item: StreamingToolItem = {
            id: ev.id,
            name: ev.name,
            arguments: ev.arguments,
            agentName: ev.agent_name,
          };
          accTools.push(item);
          setStreamingTools([...accTools]);
          break;
        }
        case "tool_result": {
          if (ev.agent_name) {
            lastSpeaker = ev.agent_name;
            setActiveSpecialist(ev.agent_name);
          }
          const existingIdx = accTools.findIndex(
            (t) => t.id === ev.tool_call_id
          );
          if (existingIdx >= 0) {
            const target = accTools[existingIdx];
            if (target) {
              accTools[existingIdx] = {
                ...target,
                resultContent: ev.content,
                truncated: ev.truncated,
                agentName: ev.agent_name ?? target.agentName,
              };
            }
          } else {
            accTools.push({
              id: ev.tool_call_id,
              name: ev.name,
              arguments: {},
              resultContent: ev.content,
              truncated: ev.truncated,
              agentName: ev.agent_name,
            });
          }
          setStreamingTools([...accTools]);
          break;
        }
        case "approval_required": {
          pausedForApproval = true;
          resolvedConvId = ev.conversation_id;
          setActiveConversationId(ev.conversation_id);
          setConversationStatus("WAITING_APPROVAL");
          setPendingApproval({
            approvalId: ev.approval_id,
            conversationId: ev.conversation_id,
            toolName: ev.tool_name,
            arguments: ev.arguments,
          });
          break;
        }
        case "final": {
          resolvedConvId = ev.conversation_id;
          setActiveConversationId(ev.conversation_id);
          finalAnswer = ev.answer || accText;
          if (ev.structured) {
            setStructuredOutput(ev.structured);
          }
          setConversationStatus("ACTIVE");
          break;
        }
        case "done": {
          if (ev.conversation_id) {
            resolvedConvId = ev.conversation_id;
            setActiveConversationId(ev.conversation_id);
          }
          finalAnswer = ev.answer ?? accText;
          if (ev.structured) {
            setStructuredOutput(ev.structured);
          }
          if (!pausedForApproval) {
            setConversationStatus("ACTIVE");
          }
          break;
        }
        case "error": {
          setErrorBanner(`[${ev.error_code}] ${ev.message}`);
          break;
        }
      }
    };

    try {
      const reqPayload = {
        message: trimmed,
        ...(activeConversationId
          ? { conversation_id: activeConversationId }
          : {}),
        ...(useDraft ? { draft: true } : {}),
      };

      try {
        await streamAgentChat(
          effectiveAgent.id,
          reqPayload,
          onStreamEvent,
          controller.signal
        );
      } catch (streamErr) {
        if (controller.signal.aborted || streamErr instanceof ApiError) {
          throw streamErr;
        }
        if (!receivedAnyEvent) {
          const syncResp = await chatWithAgent(effectiveAgent.id, reqPayload);
          resolvedConvId = syncResp.conversation_id;
          setActiveConversationId(syncResp.conversation_id);
          setConversationStatus(syncResp.status);
          finalAnswer = syncResp.answer;
          if (syncResp.structured) {
            setStructuredOutput(syncResp.structured);
          }
          if (syncResp.messages.length > 0) {
            setMessages(syncResp.messages);
            return;
          }
        } else {
          throw streamErr;
        }
      }

      const convIdForLocal = resolvedConvId ?? "local-conv";
      const synthesizedTurnMessages: MessageRecord[] = [];

      if (accTools.length > 0) {
        synthesizedTurnMessages.push({
          id: nextLocalMsgIdRef.current--,
          tenant_id: effectiveAgent.tenant_id,
          conversation_id: convIdForLocal,
          role: "ASSISTANT",
          content: "",
          tool_calls: accTools.map((tc) => ({
            id: tc.id,
            name: tc.name,
            arguments: tc.arguments,
          })),
          agent_name: lastSpeaker,
          created_at: new Date().toISOString(),
        });
        for (const tc of accTools) {
          if (tc.resultContent !== undefined) {
            synthesizedTurnMessages.push({
              id: nextLocalMsgIdRef.current--,
              tenant_id: effectiveAgent.tenant_id,
              conversation_id: convIdForLocal,
              role: "TOOL",
              content: tc.resultContent,
              tool_call_id: tc.id,
              agent_name: tc.agentName,
              created_at: new Date().toISOString(),
            });
          }
        }
      }

      const resolvedFinalText = (finalAnswer || accText).trim();
      if (resolvedFinalText.length > 0) {
        synthesizedTurnMessages.push({
          id: nextLocalMsgIdRef.current--,
          tenant_id: effectiveAgent.tenant_id,
          conversation_id: convIdForLocal,
          role: "ASSISTANT",
          content: resolvedFinalText,
          agent_name: lastSpeaker,
          created_at: new Date().toISOString(),
        });
      }

      if (synthesizedTurnMessages.length > 0) {
        setMessages((prev) => [...prev, ...synthesizedTurnMessages]);
      }

      if (resolvedConvId) {
        void queryClient.invalidateQueries({
          queryKey: ["axagent", "conversations"],
        });
        try {
          const persisted = await getConversationMessages(resolvedConvId);
          if (persisted.length > 0) {
            setMessages(persisted);
          }
        } catch {
          // Keep synthesized SSE messages when history endpoint is unavailable.
        }
      }
    } catch (err) {
      if (!controller.signal.aborted) {
        setErrorBanner(
          err instanceof Error ? err.message : "智能体对话请求失败"
        );
      }
    } finally {
      setIsStreaming(false);
      setStreamingText("");
      setStreamingTools([]);
      abortRef.current = null;
    }
  }, [
    activeConversationId,
    effectiveAgent,
    inputText,
    isStreaming,
    queryClient,
    useDraft,
  ]);

  const handleApprovalDecision = useCallback(
    async (decision: "APPROVE" | "REJECT") => {
      if (!pendingApproval) return;
      setDecidingApproval(true);
      setErrorBanner(null);
      try {
        await axflowApi.decideApproval(pendingApproval.approvalId, {
          decision,
          comment:
            approvalComment.trim() ||
            (decision === "APPROVE" ? "工作台确认放行" : "工作台拒绝执行"),
        });
        const convId = pendingApproval.conversationId;
        setPendingApproval(null);
        setApprovalComment("");
        setConversationStatus("ACTIVE");
        await queryClient.invalidateQueries({
          queryKey: ["axagent", "conversations"],
        });
        const refreshed = await getConversationMessages(convId);
        if (refreshed.length > 0) {
          setMessages(refreshed);
        }
      } catch (err) {
        setErrorBanner(
          err instanceof Error ? err.message : "提交工具审批决定失败"
        );
      } finally {
        setDecidingApproval(false);
      }
    },
    [approvalComment, pendingApproval, queryClient]
  );

  return (
    <div className="grid grid-cols-1 lg:grid-cols-12 gap-4 min-h-[600px]">
      {/* Left Sidebar: Agent Picker, Draft Toggle & Conversation History */}
      <Card className="lg:col-span-3 p-3.5 flex flex-col justify-between gap-4">
        <div className="space-y-3">
          <div className="space-y-1.5">
            <label
              htmlFor="workbench-agent-select"
              className="text-[11px] font-medium text-muted-foreground"
            >
              当前对话智能体
            </label>
            <select
              id="workbench-agent-select"
              data-testid="workbench-agent-select"
              value={effectiveAgent?.id ?? ""}
              onChange={(e) => {
                onSelectAgentId(e.target.value);
                handleNewConversation();
              }}
              className="h-9 w-full rounded-md border border-input bg-background px-2.5 text-xs font-medium text-foreground focus:outline-none focus:ring-1 focus:ring-foreground"
            >
              {agents.length === 0 ? (
                <option value="">暂无可用智能体</option>
              ) : (
                agents.map((ag) => (
                  <option key={ag.id} value={ag.id}>
                    {ag.name} (v{ag.version})
                  </option>
                ))
              )}
            </select>
          </div>

          <label className="flex items-center justify-between gap-2 rounded-md border border-border bg-muted/40 px-2.5 py-2 text-xs cursor-pointer">
            <span className="font-medium text-foreground">草稿调试模式</span>
            <input
              type="checkbox"
              data-testid="workbench-draft-toggle"
              checked={useDraft}
              onChange={(e) => setUseDraft(e.target.checked)}
              className="accent-[#2563eb]"
            />
          </label>

          <Button
            type="button"
            variant="outline"
            data-testid="workbench-new-conv-btn"
            onClick={handleNewConversation}
            className="w-full justify-center"
          >
            <MessageSquarePlus className="h-3.5 w-3.5" />
            新建会话
          </Button>

          <div className="pt-2 border-t border-border space-y-1.5">
            <div className="text-[11px] font-medium text-muted-foreground flex items-center justify-between">
              <span>历史会话</span>
              <span className="font-mono-tech">{conversations.length}</span>
            </div>

            {conversations.length === 0 ? (
              <p className="text-[11px] text-muted-foreground py-3 text-center">
                暂无会话记录
              </p>
            ) : (
              <div className="space-y-1 max-h-[340px] overflow-y-auto pr-0.5">
                {conversations.map((conv) => {
                  const isActive = conv.id === activeConversationId;
                  return (
                    <div
                      key={conv.id}
                      data-testid={`conversation-item-${conv.id}`}
                      className={cn(
                        "group flex items-center justify-between gap-1.5 rounded-md border px-2.5 py-2 text-xs transition-colors",
                        isActive
                          ? "border-xblue bg-xblue-soft/50 text-foreground"
                          : "border-transparent hover:border-border hover:bg-muted/60 text-muted-foreground hover:text-foreground"
                      )}
                    >
                      <button
                        type="button"
                        onClick={() => void handleSelectConversation(conv)}
                        className="flex-1 min-w-0 text-left space-y-0.5 cursor-pointer"
                      >
                        <div className="font-medium truncate text-foreground">
                          {conv.title || `会话 ${conv.id.slice(0, 8)}`}
                        </div>
                        <div className="flex items-center gap-1.5 text-[10px]">
                          <Badge
                            variant={
                              conv.status === "WAITING_APPROVAL"
                                ? "xblue"
                                : "outline"
                            }
                          >
                            {conv.status}
                          </Badge>
                          <span className="font-mono-tech text-muted-foreground">
                            v{conv.agent_version}
                          </span>
                        </div>
                      </button>
                      <button
                        type="button"
                        title="删除会话"
                        data-testid={`delete-conv-btn-${conv.id}`}
                        onClick={() => void handleDeleteConversation(conv.id)}
                        className="opacity-60 hover:opacity-100 p-1 rounded hover:bg-muted text-muted-foreground hover:text-foreground cursor-pointer"
                      >
                        <Trash2 className="h-3.5 w-3.5" />
                      </button>
                    </div>
                  );
                })}
              </div>
            )}
          </div>
        </div>

        {effectiveAgent ? (
          <div className="text-[11px] text-muted-foreground border-t border-border pt-2.5 space-y-1">
            <div className="flex items-center justify-between">
              <span>最大推理步数</span>
              <span className="font-mono-tech">
                {effectiveAgent.spec.max_steps}
              </span>
            </div>
            <div className="flex items-center justify-between">
              <span>记忆轮数</span>
              <span className="font-mono-tech">
                {effectiveAgent.spec.memory_turns}
              </span>
            </div>
          </div>
        ) : null}
      </Card>

      {/* Center Panel: Multi-turn Streaming Chat & Inline Approval Gate */}
      <Card className="lg:col-span-6 flex flex-col min-h-[600px] overflow-hidden">
        <div className="border-b border-border px-4 py-3 flex items-center justify-between gap-2 flex-wrap bg-muted/20">
          <div className="flex items-center gap-2 min-w-0">
            <Bot className="h-4 w-4 text-xblue shrink-0" />
            <span className="text-sm font-semibold truncate">
              {effectiveAgent?.name ?? "未选择智能体"}
            </span>
            {effectiveAgent ? (
              <Badge variant={useDraft ? "outline" : "xblue"}>
                {useDraft ? "草稿调试" : `发布版 v${effectiveAgent.version}`}
              </Badge>
            ) : null}
          </div>

          <div className="flex items-center gap-2">
            {activeSpecialist ? (
              <div
                data-testid="specialist-switch-banner"
                className="inline-flex items-center gap-1.5 rounded-full border border-xblue/30 bg-xblue-soft px-2.5 py-0.5 text-[11px] font-mono-tech text-xblue"
              >
                <Cpu className="h-3 w-3" />
                专家协作: {activeSpecialist}
              </div>
            ) : null}
            <Badge
              variant={
                conversationStatus === "WAITING_APPROVAL" ? "xblue" : "neutral"
              }
            >
              {conversationStatus}
            </Badge>
          </div>
        </div>

        {errorBanner ? (
          <div
            data-testid="workbench-error-banner"
            className="mx-4 mt-3 rounded-md border border-border bg-muted px-3 py-2 text-xs text-foreground flex items-center justify-between gap-2"
          >
            <div className="flex items-center gap-2">
              <AlertTriangle className="h-3.5 w-3.5 text-xblue shrink-0" />
              <span>{errorBanner}</span>
            </div>
            <button
              type="button"
              onClick={() => setErrorBanner(null)}
              className="text-muted-foreground hover:text-foreground text-[11px] cursor-pointer"
            >
              关闭
            </button>
          </div>
        ) : null}

        {(conversationStatus === "WAITING_APPROVAL" || pendingApproval) ? (
          <div
            data-testid="workbench-approval-banner"
            className="mx-4 mt-3 rounded-md border border-xblue/40 bg-xblue-soft/40 p-3 space-y-2.5 text-xs"
          >
            <div className="flex items-center justify-between gap-2">
              <div className="flex items-center gap-2 font-semibold text-foreground">
                <ShieldAlert className="h-4 w-4 text-xblue shrink-0" />
                <span>
                  高危工具人工审批门禁：
                  <span className="font-mono-tech ml-1">
                    {pendingApproval?.toolName ?? "受控工具调用"}
                  </span>
                </span>
              </div>
              <Badge variant="xblue">WAITING_APPROVAL</Badge>
            </div>
            {pendingApproval ? (
              <pre className="rounded border border-border bg-background/80 p-2 text-[11px] font-mono-tech overflow-x-auto max-h-28">
                {JSON.stringify(pendingApproval.arguments, null, 2)}
              </pre>
            ) : null}
            {pendingApproval ? (
              <div className="flex flex-col sm:flex-row items-stretch sm:items-center gap-2">
                <Input
                  value={approvalComment}
                  onChange={(e) => setApprovalComment(e.target.value)}
                  placeholder="审批备注（选填）"
                  className="h-8 text-xs flex-1"
                />
                <div className="flex items-center gap-2 shrink-0">
                  <Button
                    type="button"
                    data-testid="approve-tool-btn"
                    disabled={decidingApproval}
                    onClick={() => void handleApprovalDecision("APPROVE")}
                    className="h-8 px-3"
                  >
                    <CheckCircle2 className="h-3.5 w-3.5" />
                    批准执行
                  </Button>
                  <Button
                    type="button"
                    variant="outline"
                    data-testid="reject-tool-btn"
                    disabled={decidingApproval}
                    onClick={() => void handleApprovalDecision("REJECT")}
                    className="h-8 px-3"
                  >
                    <XCircle className="h-3.5 w-3.5" />
                    拒绝执行
                  </Button>
                </div>
              </div>
            ) : null}
          </div>
        ) : null}

        <div
          data-testid="workbench-messages"
          className="flex-1 p-4 overflow-y-auto max-h-[500px]"
        >
          <MessageView
            messages={messages}
            streamingToolCalls={streamingTools}
            streamingText={streamingText}
            streamingAgentName={activeSpecialist}
            isStreaming={isStreaming}
          />
        </div>

        <div className="border-t border-border p-3 bg-muted/20">
          <div className="flex items-end gap-2">
            <Textarea
              data-testid="workbench-chat-input"
              value={inputText}
              onChange={(e) => setInputText(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter" && !e.shiftKey) {
                  e.preventDefault();
                  void handleSendMessage();
                }
              }}
              rows={2}
              placeholder={
                effectiveAgent
                  ? `向 ${effectiveAgent.name} 提问（Enter 发送，Shift+Enter 换行）…`
                  : "请先创建或选择一个智能体"
              }
              disabled={!effectiveAgent || isStreaming}
              className="resize-none text-xs sm:text-sm min-h-[56px]"
            />
            {isStreaming ? (
              <Button
                type="button"
                variant="outline"
                onClick={() => abortRef.current?.abort()}
                className="h-[56px] px-3.5 shrink-0"
              >
                <Square className="h-3.5 w-3.5" />
                停止
              </Button>
            ) : (
              <Button
                type="button"
                data-testid="workbench-send-btn"
                disabled={!effectiveAgent || inputText.trim().length === 0}
                onClick={() => void handleSendMessage()}
                className="h-[56px] px-4 shrink-0"
              >
                <Send className="h-3.5 w-3.5" />
                发送
              </Button>
            )}
          </div>
        </div>
      </Card>

      {/* Right Rail: Agent Capabilities, Knowledge Scope & Structured Output */}
      <Card className="lg:col-span-3 p-3.5 space-y-4 text-xs">
        <div>
          <div className="font-semibold text-foreground mb-1">
            智能体能力契约
          </div>
          <p className="text-[11px] text-muted-foreground leading-relaxed">
            {effectiveAgent?.description ||
              "基于多模型路由、工具并发调用与专家子智能体协作执行闭环推理。"}
          </p>
        </div>

        {effectiveAgent ? (
          <>
            <div className="space-y-1.5 border-t border-border pt-3">
              <div className="text-[11px] font-medium text-muted-foreground">
                已挂载工具 ({effectiveAgent.spec.tools.length})
              </div>
              {effectiveAgent.spec.tools.length === 0 ? (
                <p className="text-[11px] text-muted-foreground">未配置工具</p>
              ) : (
                <div className="flex flex-wrap gap-1.5">
                  {effectiveAgent.spec.tools.map((tool, i) => {
                    const label =
                      tool.kind === "BUILTIN"
                        ? tool.name
                        : tool.kind === "AGENT"
                          ? `agent:${tool.as_tool_name}`
                          : tool.kind === "MCP"
                            ? `${tool.client}/${tool.tool}`
                            : `ep:${tool.endpoint_id.slice(0, 8)}`;
                    return (
                      <Badge
                        key={`${tool.kind}-${i}`}
                        variant={tool.kind === "AGENT" ? "xblue" : "neutral"}
                      >
                        {label}
                      </Badge>
                    );
                  })}
                </div>
              )}
            </div>

            <div className="space-y-1.5 border-t border-border pt-3">
              <div className="text-[11px] font-medium text-muted-foreground">
                知识与问数范围 (Knowledge Scope)
              </div>
              <div className="space-y-1 text-[11px]">
                <div className="flex items-center justify-between gap-2">
                  <span className="text-muted-foreground">文档前缀:</span>
                  <span className="font-mono-tech">
                    {effectiveAgent.spec.knowledge_scope.doc_title_prefix ||
                      "全量可见文档"}
                  </span>
                </div>
                <div className="space-y-1">
                  <span className="text-muted-foreground">限定数据集:</span>
                  {effectiveAgent.spec.knowledge_scope.dataset_names.length >
                  0 ? (
                    <div className="flex flex-wrap gap-1 pt-0.5">
                      {effectiveAgent.spec.knowledge_scope.dataset_names.map(
                        (ds) => (
                          <Badge key={ds} variant="outline">
                            {ds}
                          </Badge>
                        )
                      )}
                    </div>
                  ) : (
                    <span className="font-mono-tech ml-1">全量授权数据集</span>
                  )}
                </div>
              </div>
            </div>

            {effectiveAgent.spec.approval_required_tools.length > 0 ? (
              <div className="space-y-1.5 border-t border-border pt-3">
                <div className="text-[11px] font-medium text-muted-foreground">
                  需人工审批工具
                </div>
                <div className="flex flex-wrap gap-1">
                  {effectiveAgent.spec.approval_required_tools.map((t) => (
                    <Badge key={t} variant="xblue">
                      {t}
                    </Badge>
                  ))}
                </div>
              </div>
            ) : null}
          </>
        ) : null}

        {structuredOutput ? (
          <div className="space-y-1.5 border-t border-border pt-3">
            <div className="text-[11px] font-medium text-muted-foreground">
              结构化输出 (Output Schema)
            </div>
            <pre className="rounded border border-border bg-muted/50 p-2 text-[11px] font-mono-tech overflow-x-auto max-h-48">
              {JSON.stringify(structuredOutput, null, 2)}
            </pre>
          </div>
        ) : null}
      </Card>
    </div>
  );
}
