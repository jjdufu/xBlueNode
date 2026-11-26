import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import {
  AlertTriangle,
  Bot,
  CheckCircle2,
  Cpu,
  MessageSquarePlus,
  PanelLeftClose,
  PanelLeftOpen,
  PanelRightClose,
  PanelRightOpen,
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
import {
  Badge,
  Button,
  Card,
  Input,
  Select,
  StatusBadge,
  Switch,
  Textarea,
} from "@/components/ui/primitives";
import { useI18n } from "@/i18n";
import { usePrincipal } from "@/lib/permissions";
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
  const { tx, fmtNum } = useI18n();
  const { can, canAny } = usePrincipal();

  const canRun = canAny(["AGENT_RUN", "AGENT_EDIT"]);
  const canDecideApproval = can("APPROVAL_DECIDE");

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
  const [showLeftRail, setShowLeftRail] = useState(true);
  const [showRightRail, setShowRightRail] = useState(true);
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
          err instanceof Error
            ? err.message
            : tx("加载会话消息失败", "Failed to load conversation messages")
        );
      }
    },
    [tx]
  );

  const handleDeleteConversation = useCallback(
    async (convId: string) => {
      if (!canRun) return;
      try {
        await deleteConversation(convId);
        if (activeConversationId === convId) {
          handleNewConversation();
        }
        await queryClient.invalidateQueries({
          queryKey: ["axagent", "conversations"],
        });
      } catch (err) {
        setErrorBanner(
          err instanceof Error
            ? err.message
            : tx("删除会话失败", "Failed to delete conversation")
        );
      }
    },
    [activeConversationId, canRun, handleNewConversation, queryClient, tx]
  );

  const handleSendMessage = useCallback(async () => {
    const trimmed = inputText.trim();
    if (!canRun || !trimmed || !effectiveAgent || isStreaming) {
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
          err instanceof Error
            ? err.message
            : tx("对话请求失败", "Chat request failed")
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
    canRun,
    effectiveAgent,
    inputText,
    isStreaming,
    queryClient,
    tx,
    useDraft,
  ]);

  const handleApprovalDecision = useCallback(
    async (decision: "APPROVE" | "REJECT") => {
      if (!pendingApproval || !canDecideApproval) return;
      setDecidingApproval(true);
      setErrorBanner(null);
      try {
        await axflowApi.decideApproval(pendingApproval.approvalId, {
          decision,
          comment:
            approvalComment.trim() ||
            (decision === "APPROVE"
              ? tx("批准放行", "Approved")
              : tx("驳回执行", "Rejected")),
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
          err instanceof Error
            ? err.message
            : tx("审批提交失败", "Failed to submit approval decision")
        );
      } finally {
        setDecidingApproval(false);
      }
    },
    [approvalComment, canDecideApproval, pendingApproval, queryClient, tx]
  );

  const centerSpanClass =
    showLeftRail && showRightRail
      ? "lg:col-span-6"
      : showLeftRail || showRightRail
        ? "lg:col-span-9"
        : "lg:col-span-12";

  return (
    <div className="grid grid-cols-1 lg:grid-cols-12 gap-4 lg:h-[calc(100dvh-215px)] lg:min-h-[560px]">
      {/* Left Sidebar: Agent Picker, Draft Toggle & Conversation History */}
      {showLeftRail ? (
        <Card className="lg:col-span-3 p-3.5 flex flex-col justify-between gap-4 overflow-y-auto">
        <div className="space-y-3">
          <div className="space-y-1.5">
            <label
              htmlFor="workbench-agent-select"
              className="text-[11px] font-medium text-muted-foreground"
            >
              {tx("当前智能体", "Active Agent")}
            </label>
            <Select
              id="workbench-agent-select"
              data-testid="workbench-agent-select"
              value={effectiveAgent?.id ?? ""}
              onChange={(e) => {
                onSelectAgentId(e.target.value);
                handleNewConversation();
              }}
              className="w-full font-medium"
            >
              {agents.length === 0 ? (
                <option value="">{tx("暂无智能体", "No agents")}</option>
              ) : (
                agents.map((ag) => (
                  <option key={ag.id} value={ag.id}>
                    {ag.name} (v{ag.version})
                  </option>
                ))
              )}
            </Select>
          </div>

          <div className="flex items-center justify-between gap-2 rounded-md border border-border bg-muted/40 px-2.5 py-2 text-xs">
            <label
              htmlFor="workbench-draft-toggle"
              className="font-medium text-foreground cursor-pointer"
            >
              {tx("草稿调试模式", "Draft Mode")}
            </label>
            <Switch
              id="workbench-draft-toggle"
              data-testid="workbench-draft-toggle"
              aria-label={tx("草稿调试模式", "Draft Mode")}
              checked={useDraft}
              onCheckedChange={setUseDraft}
            />
          </div>

          <Button
            type="button"
            variant="outline"
            data-testid="workbench-new-conv-btn"
            onClick={handleNewConversation}
            className="w-full justify-center"
          >
            <MessageSquarePlus className="h-3.5 w-3.5" />
            {tx("新建会话", "New Chat")}
          </Button>

          <div className="pt-2 border-t border-border space-y-1.5">
            <div className="text-[11px] font-medium text-muted-foreground flex items-center justify-between">
              <span>{tx("历史会话", "History")}</span>
              <span className="font-mono-tech">{fmtNum(conversations.length, 0)}</span>
            </div>

            {conversations.length === 0 ? (
              <p className="text-[11px] text-muted-foreground py-3 text-center">
                {tx("暂无会话记录", "No conversations")}
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
                          {conv.title || conv.id.slice(0, 8)}
                        </div>
                        <div className="flex items-center gap-1.5 text-[10px]">
                          <StatusBadge status={conv.status} />
                          <span className="font-mono-tech text-muted-foreground">
                            v{conv.agent_version}
                          </span>
                        </div>
                      </button>
                      {canRun && (
                        <button
                          type="button"
                          title={tx("删除会话", "Delete")}
                          data-testid={`delete-conv-btn-${conv.id}`}
                          onClick={() => void handleDeleteConversation(conv.id)}
                          className="opacity-60 hover:opacity-100 p-1 rounded hover:bg-muted text-muted-foreground hover:text-foreground cursor-pointer"
                        >
                          <Trash2 className="h-3.5 w-3.5" />
                        </button>
                      )}
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
              <span>{tx("最大推理步数", "Max Steps")}</span>
              <span className="font-mono-tech">
                {effectiveAgent.spec.max_steps}
              </span>
            </div>
            <div className="flex items-center justify-between">
              <span>{tx("记忆轮数", "Memory Turns")}</span>
              <span className="font-mono-tech">
                {effectiveAgent.spec.memory_turns}
              </span>
            </div>
          </div>
        ) : null}
      </Card>
      ) : null}

      {/* Center Panel: Multi-turn Streaming Chat & Inline Approval Gate */}
      <Card
        className={cn(
          centerSpanClass,
          "flex flex-col min-h-[540px] lg:min-h-0 lg:h-full overflow-hidden"
        )}
      >
        <div className="border-b border-border px-4 py-3 flex items-center justify-between gap-2 flex-wrap bg-muted/20">
          <div className="flex items-center gap-2 min-w-0">
            <Button
              type="button"
              variant="ghost"
              size="sm"
              className="hidden lg:inline-flex h-7 w-7 px-0"
              onClick={() => setShowLeftRail((v) => !v)}
              title={tx("切换左侧边栏", "Toggle left sidebar")}
              aria-label={tx("切换左侧边栏", "Toggle left sidebar")}
            >
              {showLeftRail ? (
                <PanelLeftClose className="h-3.5 w-3.5" />
              ) : (
                <PanelLeftOpen className="h-3.5 w-3.5" />
              )}
            </Button>
            <Bot className="h-4 w-4 text-xblue shrink-0" />
            <span className="text-sm font-semibold truncate">
              {effectiveAgent?.name ?? tx("未配置智能体", "No Agent")}
            </span>
            {effectiveAgent ? (
              <Badge variant={useDraft ? "outline" : "xblue"}>
                {useDraft
                  ? tx("草稿", "Draft")
                  : `v${effectiveAgent.version}`}
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
                {tx("专家", "Specialist")}: {activeSpecialist}
              </div>
            ) : null}
            <StatusBadge status={conversationStatus} />
            <Button
              type="button"
              variant="ghost"
              size="sm"
              className="hidden lg:inline-flex h-7 w-7 px-0"
              onClick={() => setShowRightRail((v) => !v)}
              title={tx("切换右侧契约栏", "Toggle right contract sidebar")}
              aria-label={tx("切换右侧契约栏", "Toggle right contract sidebar")}
            >
              {showRightRail ? (
                <PanelRightClose className="h-3.5 w-3.5" />
              ) : (
                <PanelRightOpen className="h-3.5 w-3.5" />
              )}
            </Button>
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
              {tx("关闭", "Close")}
            </button>
          </div>
        ) : null}

        {conversationStatus === "WAITING_APPROVAL" || pendingApproval ? (
          <div
            data-testid="workbench-approval-banner"
            className="mx-4 mt-3 rounded-md border border-xblue/40 bg-xblue-soft/40 p-3 space-y-2.5 text-xs"
          >
            <div className="flex items-center justify-between gap-2">
              <div className="flex items-center gap-2 font-semibold text-foreground">
                <ShieldAlert className="h-4 w-4 text-xblue shrink-0" />
                <span>
                  {tx("工具调用待审批", "Tool Approval Required")}:
                  <span className="font-mono-tech ml-1">
                    {pendingApproval?.toolName ?? tx("受控工具", "Controlled Tool")}
                  </span>
                </span>
              </div>
              <StatusBadge status="WAITING_APPROVAL" />
            </div>
            {pendingApproval ? (
              <pre className="rounded border border-border bg-background/80 p-2 text-[11px] font-mono-tech overflow-x-auto max-h-28">
                {JSON.stringify(pendingApproval.arguments, null, 2)}
              </pre>
            ) : null}
            {pendingApproval && canDecideApproval ? (
              <div className="flex flex-col sm:flex-row items-stretch sm:items-center gap-2">
                <Input
                  value={approvalComment}
                  onChange={(e) => setApprovalComment(e.target.value)}
                  placeholder={tx("审批备注", "Approval note")}
                  className="h-8 text-xs flex-1"
                />
                <div className="flex items-center gap-2 shrink-0">
                  <Button
                    type="button"
                    size="sm"
                    data-testid="approve-tool-btn"
                    disabled={decidingApproval}
                    onClick={() => void handleApprovalDecision("APPROVE")}
                  >
                    <CheckCircle2 className="h-3.5 w-3.5" />
                    {tx("批准", "Approve")}
                  </Button>
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    data-testid="reject-tool-btn"
                    disabled={decidingApproval}
                    onClick={() => void handleApprovalDecision("REJECT")}
                  >
                    <XCircle className="h-3.5 w-3.5" />
                    {tx("驳回", "Reject")}
                  </Button>
                </div>
              </div>
            ) : null}
          </div>
        ) : null}

        <div
          data-testid="workbench-messages"
          className="flex-1 min-h-0 p-4 overflow-y-auto"
        >
          <MessageView
            messages={messages}
            streamingToolCalls={streamingTools}
            streamingText={streamingText}
            streamingAgentName={activeSpecialist}
            isStreaming={isStreaming}
          />
        </div>

        <div className="border-t border-border p-3 bg-muted/20 space-y-1.5">
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
                  ? tx(
                      `向 ${effectiveAgent.name} 提问…`,
                      `Ask ${effectiveAgent.name}…`
                    )
                  : tx("暂无可用智能体", "No active agent")
              }
              disabled={!canRun || !effectiveAgent || isStreaming}
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
                {tx("停止", "Stop")}
              </Button>
            ) : (
              <Button
                type="button"
                variant="primary"
                data-testid="workbench-send-btn"
                disabled={
                  !canRun || !effectiveAgent || inputText.trim().length === 0
                }
                onClick={() => void handleSendMessage()}
                className="h-[56px] px-4 shrink-0"
              >
                <Send className="h-3.5 w-3.5" />
                {tx("发送", "Send")}
              </Button>
            )}
          </div>
          <div className="flex items-center justify-between text-[10px] text-muted-foreground px-0.5">
            <span>{tx("Enter 发送 · Shift+Enter 换行", "Enter to send · Shift+Enter for newline")}</span>
          </div>
        </div>
      </Card>

      {/* Right Rail: Agent Capabilities, Knowledge Scope & Structured Output */}
      {showRightRail ? (
        <Card className="lg:col-span-3 p-3.5 space-y-4 text-xs overflow-y-auto">
        <div>
          <div className="font-semibold text-foreground mb-1">
            {tx("智能体能力契约", "Agent Contract")}
          </div>
          <p className="text-[11px] text-muted-foreground leading-relaxed">
            {effectiveAgent?.description ||
              tx(
                "多模型路由、工具并发调用与专家协作闭环推理。",
                "Multi-model routing, tool invocation, and specialist delegation."
              )}
          </p>
        </div>

        {effectiveAgent ? (
          <>
            <div className="space-y-1.5 border-t border-border pt-3">
              <div className="text-[11px] font-medium text-muted-foreground">
                {tx("已挂载工具", "Mounted Tools")} (
                {effectiveAgent.spec.tools.length})
              </div>
              {effectiveAgent.spec.tools.length === 0 ? (
                <p className="text-[11px] text-muted-foreground">
                  {tx("未挂载工具", "No tools")}
                </p>
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
                {tx("知识与问数范围", "Knowledge Scope")}
              </div>
              <div className="space-y-1 text-[11px]">
                <div className="flex items-center justify-between gap-2">
                  <span className="text-muted-foreground">
                    {tx("文档前缀", "Doc Prefix")}:
                  </span>
                  <span className="font-mono-tech">
                    {effectiveAgent.spec.knowledge_scope.doc_title_prefix ||
                      tx("全部可见文档", "All visible docs")}
                  </span>
                </div>
                <div className="space-y-1">
                  <span className="text-muted-foreground">
                    {tx("限定数据集", "Datasets")}:
                  </span>
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
                    <span className="font-mono-tech ml-1">
                      {tx("全部授权数据集", "All permitted datasets")}
                    </span>
                  )}
                </div>
              </div>
            </div>

            {effectiveAgent.spec.approval_required_tools.length > 0 ? (
              <div className="space-y-1.5 border-t border-border pt-3">
                <div className="text-[11px] font-medium text-muted-foreground">
                  {tx("需审批工具", "Approval Required")}
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
              {tx("结构化输出", "Structured Output")}
            </div>
            <pre className="rounded border border-border bg-muted/50 p-2 text-[11px] font-mono-tech overflow-x-auto max-h-48">
              {JSON.stringify(structuredOutput, null, 2)}
            </pre>
          </div>
        ) : null}
      </Card>
      ) : null}
    </div>
  );
}
