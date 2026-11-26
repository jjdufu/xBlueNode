import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useLocation, useNavigate } from "react-router";
import {
  ArrowRight,
  Bot,
  Code2,
  Compass,
  History,
  Loader2,
  MessageSquarePlus,
  Send,
  Sparkles,
  Trash2,
  X,
} from "lucide-react";
import { useI18n } from "@/i18n";
import { analyticsApi } from "@/lib/api/analytics";
import {
  chatWithAgent,
  getConversationMessages,
  listAgents,
  listConversations,
  type AgentRecord,
  type ConversationRecord,
} from "@/lib/api/axagent";
import {
  setFloatingCopilotOpen,
  useCopilotBridge,
} from "@/lib/copilotBridge";
import { usePrincipal } from "@/lib/permissions";
import { cn } from "@/lib/utils";
import { Badge, Button, IconButton, Select, Textarea } from "./primitives";
import { StudioCopilotPanel } from "../xbluestudio/StudioCopilotPanel";

const STORAGE_KEY = "axpoint.floating_ai.sessions.v1";

interface RoutePresetConfig {
  readonly domainKey: string;
  readonly zhTitle: string;
  readonly enTitle: string;
  readonly quickPrompts: readonly { zh: string; en: string }[];
}

function resolveRoutePreset(pathname: string): RoutePresetConfig {
  if (pathname.startsWith("/studio")) {
    return {
      domainKey: "studio",
      zhTitle: "建模工作室",
      enTitle: "Modeling Studio",
      quickPrompts: [
        {
          zh: "分析当前业务模型节点的指标与关系结构是否完备",
          en: "Analyze completeness of current schema nodes and relations",
        },
        {
          zh: "如何把湖表列准确映射到主数据节点属性与指标？",
          en: "How to map lake table columns cleanly to properties and metrics?",
        },
      ],
    };
  }
  if (pathname.startsWith("/analytics")) {
    return {
      domainKey: "analytics",
      zhTitle: "查询与制图",
      enTitle: "Analytics & SQL",
      quickPrompts: [
        {
          zh: "统计订单总金额与订单数量",
          en: "Summarize total order amount and count",
        },
        {
          zh: "按仓储枢纽查看当前可用库存分布",
          en: "Check inventory distribution by warehouse",
        },
        {
          zh: "查询高风险对公资金划转前 10 条明细",
          en: "Top 10 wire transfer records",
        },
      ],
    };
  }
  if (pathname.startsWith("/hub")) {
    return {
      domainKey: "hub",
      zhTitle: "数据与知识",
      enTitle: "Data & Knowledge",
      quickPrompts: [
        {
          zh: "总结华东仓知识文档与履约约束制度",
          en: "Summarize warehouse knowledge docs and fulfillment rules",
        },
        {
          zh: "梳理当前已入湖数据集及上下游血缘关系",
          en: "Review ingested datasets and upstream/downstream lineage",
        },
      ],
    };
  }
  if (pathname.startsWith("/sim")) {
    return {
      domainKey: "sim",
      zhTitle: "推演沙盘",
      enTitle: "Simulation",
      quickPrompts: [
        {
          zh: "解释红线干预窗口与最晚干预步数的含义",
          en: "Explain intervention deadlines and breach timing",
        },
        {
          zh: "评估仓储库存下降 30% 对下游履约的传导影响",
          en: "Trace impact path of a 30% warehouse inventory drop",
        },
      ],
    };
  }
  if (pathname.startsWith("/flow")) {
    return {
      domainKey: "flow",
      zhTitle: "编排与告警",
      enTitle: "Automation & Alerts",
      quickPrompts: [
        {
          zh: "汇总当前活跃业务告警及建议优先处置动作",
          en: "Summarize active alerts and recommended triage actions",
        },
        {
          zh: "说明动态指标聚合算子参数与审批门禁配置要点",
          en: "Explain dynamic aggregation parameters and approval gates",
        },
      ],
    };
  }
  if (pathname.startsWith("/view")) {
    return {
      domainKey: "view",
      zhTitle: "数字看板",
      enTitle: "Cockpits",
      quickPrompts: [
        {
          zh: "解读当前驾驶舱核心指标波动与异常告警",
          en: "Interpret KPI trends and alerts in the current cockpit",
        },
        {
          zh: "给出适合本驾驶舱补充的关键可视化指标建议",
          en: "Recommend additional KPIs for this cockpit",
        },
      ],
    };
  }
  if (pathname.startsWith("/agents")) {
    return {
      domainKey: "agents",
      zhTitle: "智能体与接口",
      enTitle: "Agents & APIs",
      quickPrompts: [
        {
          zh: "对比当前已发布智能体的职能分工与工具范围",
          en: "Compare published agents and their tool permissions",
        },
        {
          zh: "发布外部 API 与模型协议端点前有哪些安全核查项？",
          en: "Security checklist before publishing endpoints and MCP tools",
        },
      ],
    };
  }
  if (pathname.startsWith("/settings")) {
    return {
      domainKey: "settings",
      zhTitle: "系统设置",
      enTitle: "Settings",
      quickPrompts: [
        {
          zh: "如何排查系统健康巡检降级项与异步队列积压？",
          en: "How to troubleshoot degraded health probes or queue backlog?",
        },
        {
          zh: "列级敏感脱敏与只读 SQL 守卫是如何协同生效的？",
          en: "How do PII column masking and SQL guards work together?",
        },
      ],
    };
  }
  return {
    domainKey: "dashboard",
    zhTitle: "总控看板",
    enTitle: "Overview",
    quickPrompts: [
      {
        zh: "汇总当前待处置告警与高风险业务子图",
        en: "Summarize active alerts and high-risk subgraphs",
      },
      {
        zh: "当前平台数据入湖、模型与看板就绪状态如何？",
        en: "Summarize readiness across data, models, and cockpits",
      },
    ],
  };
}

export interface FloatingChatMsg {
  id: string;
  role: "user" | "assistant";
  content: string;
  sql?: string;
  routeTitle?: string;
  createdAt: string;
}

export interface FloatingAiSession {
  id: string;
  title: string;
  agentId: string;
  agentName: string;
  conversationId?: string;
  updatedAt: string;
  messages: FloatingChatMsg[];
}

interface StoredAiState {
  activeSessionId: string;
  sessions: FloatingAiSession[];
}

function makeFreshSession(
  agentId = "",
  agentName = "ops_supervisor"
): FloatingAiSession {
  const now = new Date().toISOString();
  return {
    id: `local-${Date.now()}-${Math.random().toString(36).slice(2, 6)}`,
    title: "",
    agentId,
    agentName,
    updatedAt: now,
    messages: [],
  };
}

function readStoredSessions(): StoredAiState {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (raw) {
      const parsed = JSON.parse(raw) as Partial<StoredAiState>;
      if (
        Array.isArray(parsed.sessions) &&
        parsed.sessions.length > 0 &&
        typeof parsed.activeSessionId === "string"
      ) {
        const hasActive = parsed.sessions.some(
          (s) => s.id === parsed.activeSessionId
        );
        return {
          activeSessionId: hasActive
            ? parsed.activeSessionId
            : parsed.sessions[0]!.id,
          sessions: parsed.sessions.slice(0, 30),
        };
      }
    }
  } catch {
    // ignore storage errors
  }
  const initial = makeFreshSession();
  return { activeSessionId: initial.id, sessions: [initial] };
}

function writeStoredSessions(state: StoredAiState): void {
  try {
    localStorage.setItem(
      STORAGE_KEY,
      JSON.stringify({
        activeSessionId: state.activeSessionId,
        sessions: state.sessions.slice(0, 30),
      })
    );
  } catch {
    // ignore quota errors
  }
}

export function FloatingAiCopilot({
  isFullBleed,
}: {
  readonly isFullBleed: boolean;
}) {
  const location = useLocation();
  const navigate = useNavigate();
  const { tx, fmtTime } = useI18n();
  const { canAny } = usePrincipal();
  const bridge = useCopilotBridge();

  const preset = useMemo(
    () => resolveRoutePreset(location.pathname),
    [location.pathname]
  );

  const [store, setStore] = useState<StoredAiState>(() => readStoredSessions());
  const [viewTab, setViewTab] = useState<"chat" | "history" | "studio_patch">(
    "chat"
  );
  const [attachRouteContext, setAttachRouteContext] = useState(true);
  const [agents, setAgents] = useState<AgentRecord[]>([]);
  const [remoteConversations, setRemoteConversations] = useState<
    ConversationRecord[]
  >([]);
  const [loadingHistory, setLoadingHistory] = useState(false);
  const [inputText, setInputText] = useState("");
  const [sending, setSending] = useState(false);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);
  const scrollEndRef = useRef<HTMLDivElement | null>(null);

  const isStudioAvailable =
    preset.domainKey === "studio" && bridge.studio !== null;
  const isAnalyticsRoute = preset.domainKey === "analytics";
  const canUseAgent = canAny(["AGENT_RUN", "AGENT_EDIT"]);

  // Fall back smoothly if user leaves /studio while on studio_patch view
  useEffect(() => {
    if (!isStudioAvailable && viewTab === "studio_patch") {
      setViewTab("chat");
    }
  }, [isStudioAvailable, viewTab]);

  // Persist session changes to localStorage continuously across routes and reloads
  useEffect(() => {
    writeStoredSessions(store);
  }, [store]);

  const activeSession = useMemo(() => {
    return (
      store.sessions.find((s) => s.id === store.activeSessionId) ??
      store.sessions[0] ??
      makeFreshSession()
    );
  }, [store]);

  const activeMessages = activeSession.messages;

  // Escape closes the floating window
  useEffect(() => {
    if (!bridge.open) return;
    const onKey = (ev: KeyboardEvent) => {
      if (ev.key === "Escape") {
        setFloatingCopilotOpen(false);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [bridge.open]);

  // Load agents once panel is opened
  useEffect(() => {
    if (!bridge.open || agents.length > 0) return;
    let cancelled = false;
    void listAgents()
      .then((list) => {
        if (cancelled) return;
        const arr = Array.isArray(list) ? list : [];
        setAgents(arr);
        const defaultAgent =
          arr.find((a) => a.name === "ops_supervisor") ??
          arr.find((a) => a.version > 0) ??
          arr[0];
        if (defaultAgent && !activeSession.agentId) {
          setStore((prev) => ({
            ...prev,
            sessions: prev.sessions.map((s) =>
              s.id === activeSession.id
                ? { ...s, agentId: defaultAgent.id, agentName: defaultAgent.name }
                : s
            ),
          }));
        }
      })
      .catch(() => {
        if (!cancelled) setAgents([]);
      });
    return () => {
      cancelled = true;
    };
  }, [activeSession.agentId, activeSession.id, agents.length, bridge.open]);

  const refreshRemoteConversations = useCallback(async () => {
    setLoadingHistory(true);
    try {
      const list = await listConversations({ limit: 25 });
      setRemoteConversations(Array.isArray(list) ? list : []);
    } catch {
      setRemoteConversations([]);
    } finally {
      setLoadingHistory(false);
    }
  }, []);

  useEffect(() => {
    if (bridge.open && viewTab === "history") {
      void refreshRemoteConversations();
    }
  }, [bridge.open, refreshRemoteConversations, viewTab]);

  useEffect(() => {
    if (viewTab === "chat") {
      scrollEndRef.current?.scrollIntoView({ behavior: "smooth" });
    }
  }, [activeMessages.length, sending, bridge.open, viewTab]);

  const startNewSession = () => {
    const defaultAgent =
      agents.find((a) => a.id === activeSession.agentId) ??
      agents.find((a) => a.name === "ops_supervisor") ??
      agents.find((a) => a.version > 0) ??
      agents[0];
    const next = makeFreshSession(
      defaultAgent?.id ?? "",
      defaultAgent?.name ?? "ops_supervisor"
    );
    setStore((prev) => ({
      activeSessionId: next.id,
      sessions: [next, ...prev.sessions].slice(0, 30),
    }));
    setViewTab("chat");
    setErrorMsg(null);
  };

  const selectLocalSession = (sessionId: string) => {
    setStore((prev) => ({ ...prev, activeSessionId: sessionId }));
    setViewTab("chat");
    setErrorMsg(null);
  };

  const deleteLocalSession = (sessionId: string) => {
    setStore((prev) => {
      const remaining = prev.sessions.filter((s) => s.id !== sessionId);
      if (remaining.length === 0) {
        const fresh = makeFreshSession(
          activeSession.agentId,
          activeSession.agentName
        );
        return { activeSessionId: fresh.id, sessions: [fresh] };
      }
      const nextActive =
        prev.activeSessionId === sessionId
          ? remaining[0]!.id
          : prev.activeSessionId;
      return { activeSessionId: nextActive, sessions: remaining };
    });
  };

  const loadRemoteConversation = async (conv: ConversationRecord) => {
    const existingLocal = store.sessions.find(
      (s) => s.conversationId === conv.id
    );
    if (existingLocal && existingLocal.messages.length > 0) {
      setStore((prev) => ({ ...prev, activeSessionId: existingLocal.id }));
      setViewTab("chat");
      return;
    }
    setLoadingHistory(true);
    try {
      const records = await getConversationMessages(conv.id);
      const mappedMsgs: FloatingChatMsg[] = records
        .filter((r) => r.role === "USER" || r.role === "ASSISTANT")
        .map((r) => ({
          id: `remote-msg-${r.id}`,
          role: r.role === "USER" ? "user" : "assistant",
          content: r.content,
          createdAt: r.created_at,
        }));
      const agentObj = agents.find((a) => a.id === conv.agent_id);
      const imported: FloatingAiSession = {
        id: `conv-${conv.id}`,
        title: conv.title || tx("历史对话", "Conversation"),
        agentId: conv.agent_id,
        agentName: agentObj?.name ?? "Agent",
        conversationId: conv.id,
        updatedAt: conv.updated_at || conv.created_at,
        messages: mappedMsgs,
      };
      setStore((prev) => ({
        activeSessionId: imported.id,
        sessions: [
          imported,
          ...prev.sessions.filter((s) => s.conversationId !== conv.id),
        ].slice(0, 30),
      }));
      setViewTab("chat");
    } catch (err) {
      setErrorMsg(
        err instanceof Error ? err.message : tx("读取历史失败", "Load failed")
      );
    } finally {
      setLoadingHistory(false);
    }
  };

  const handleSend = async (
    overrideText?: string,
    preferSqlAsk = false
  ) => {
    const raw = (overrideText ?? inputText).trim();
    if (!raw || sending) return;
    setErrorMsg(null);
    if (!overrideText) setInputText("");

    const now = new Date().toISOString();
    const userMsg: FloatingChatMsg = {
      id: `u-${Date.now()}`,
      role: "user",
      content: raw,
      routeTitle: tx(preset.zhTitle, preset.enTitle),
      createdAt: now,
    };

    const targetSessionId = activeSession.id;
    setStore((prev) => ({
      ...prev,
      sessions: prev.sessions.map((s) =>
        s.id === targetSessionId
          ? {
              ...s,
              title: s.title || raw.slice(0, 32),
              updatedAt: now,
              messages: [...s.messages, userMsg],
            }
          : s
      ),
    }));

    setSending(true);
    try {
      const shouldUseSqlAsk =
        preferSqlAsk ||
        (isAnalyticsRoute &&
          /sql|查询|统计|订单|库存|流水|汇总|分布|排名|前/i.test(raw));

      if (shouldUseSqlAsk) {
        const res = await analyticsApi.ask({ question: raw });
        const botMsg: FloatingChatMsg = {
          id: `a-${Date.now()}`,
          role: "assistant",
          content:
            res.answer ||
            res.explanation ||
            tx("已生成查询 SQL。", "Generated query SQL."),
          sql: res.sql || undefined,
          createdAt: new Date().toISOString(),
        };
        setStore((prev) => ({
          ...prev,
          sessions: prev.sessions.map((s) =>
            s.id === targetSessionId
              ? {
                  ...s,
                  updatedAt: botMsg.createdAt,
                  messages: [...s.messages, botMsg],
                }
              : s
          ),
        }));
      } else {
        if (!canUseAgent) {
          setErrorMsg(
            tx("当前账号缺少智能体调用权限。", "No AGENT_RUN permission.")
          );
          return;
        }
        const targetAgent =
          agents.find((a) => a.id === activeSession.agentId) ??
          agents.find((a) => a.name === "ops_supervisor") ??
          agents.find((a) => a.version > 0) ??
          agents[0];
        if (!targetAgent) {
          setErrorMsg(
            tx(
              "暂未检测到可用智能体，请先在「智能体与接口」发布。",
              "No agent available yet."
            )
          );
          return;
        }
        const routePrefix = attachRouteContext
          ? `[当前页面: ${tx(preset.zhTitle, preset.enTitle)} (${location.pathname}${location.search})]\n`
          : "";
        const res = await chatWithAgent(targetAgent.id, {
          message: `${routePrefix}${raw}`,
          ...(activeSession.conversationId
            ? { conversation_id: activeSession.conversationId }
            : {}),
        });
        const botMsg: FloatingChatMsg = {
          id: `a-${Date.now()}`,
          role: "assistant",
          content: res.answer || tx("已完成处理。", "Completed."),
          createdAt: new Date().toISOString(),
        };
        setStore((prev) => ({
          ...prev,
          sessions: prev.sessions.map((s) =>
            s.id === targetSessionId
              ? {
                  ...s,
                  agentId: targetAgent.id,
                  agentName: targetAgent.name,
                  conversationId: res.conversation_id || s.conversationId,
                  updatedAt: botMsg.createdAt,
                  messages: [...s.messages, botMsg],
                }
              : s
          ),
        }));
      }
    } catch (err) {
      setErrorMsg(
        err instanceof Error
          ? err.message
          : tx("助手请求失败，请稍后重试", "Assistant request failed")
      );
    } finally {
      setSending(false);
    }
  };

  // Merge local sessions + remote conversations not yet imported into local list
  const remoteOnlyConversations = useMemo(() => {
    const localConvIds = new Set(
      store.sessions.map((s) => s.conversationId).filter(Boolean)
    );
    return remoteConversations.filter((c) => !localConvIds.has(c.id));
  }, [remoteConversations, store.sessions]);

  const panelBottomClass = isFullBleed ? "bottom-12" : "bottom-4";

  return (
    <>
      {bridge.open && (
        <div
          data-testid="floating-ai-panel"
          role="dialog"
          aria-label={tx("全局 AI 助手", "Global AI Copilot")}
          className={cn(
            "fixed right-3 z-50 w-[430px] max-w-[calc(100vw-2rem)] h-[min(74vh,720px)] max-h-[calc(100vh-5rem)] rounded-xl border border-border bg-card text-card-foreground shadow-2xl flex flex-col overflow-hidden",
            panelBottomClass
          )}
        >
          {/* Header with Persistent History & New Chat controls */}
          <div className="shrink-0 flex items-center justify-between gap-2 border-b border-border bg-muted/30 px-3.5 py-2.5">
            <div className="flex items-center gap-2 min-w-0">
              <span className="inline-flex h-6 w-6 items-center justify-center rounded-md bg-xblue-soft text-xblue shrink-0">
                <Sparkles className="h-3.5 w-3.5" />
              </span>
              <span className="font-semibold text-xs truncate">
                {tx("AI 助手", "AI Copilot")}
              </span>
              <Badge
                variant="xblue"
                data-testid="floating-ai-route-badge"
                className="text-[10px] truncate max-w-[110px]"
              >
                {tx(preset.zhTitle, preset.enTitle)}
              </Badge>
            </div>

            <div className="flex items-center gap-1 shrink-0">
              <IconButton
                size="sm"
                data-testid="floating-ai-new-chat-btn"
                onClick={startNewSession}
                aria-label={tx("新建对话", "New Chat")}
                title={tx("新建对话", "New Chat")}
              >
                <MessageSquarePlus className="h-3.5 w-3.5" />
              </IconButton>
              <IconButton
                size="sm"
                active={viewTab === "history"}
                data-testid="floating-ai-history-btn"
                onClick={() =>
                  setViewTab((v) => (v === "history" ? "chat" : "history"))
                }
                aria-label={tx("历史记录", "History")}
                title={tx("历史记录", "History")}
              >
                <History className="h-3.5 w-3.5" />
              </IconButton>
              <IconButton
                size="sm"
                data-testid="floating-ai-close-btn"
                onClick={() => setFloatingCopilotOpen(false)}
                aria-label={tx("关闭浮窗", "Close")}
                title={tx("关闭", "Close")}
              >
                <X className="h-4 w-4" />
              </IconButton>
            </div>
          </div>

          {/* Sub-mode switcher when inside /studio so user can access Schema Patching without losing Chat History */}
          {isStudioAvailable && viewTab !== "history" && (
            <div className="shrink-0 border-b border-border bg-muted/15 px-3 py-1.5 flex items-center justify-between gap-2">
              <div className="inline-flex rounded-md border border-border bg-muted/50 p-0.5 gap-0.5 text-[11px]">
                <button
                  type="button"
                  data-testid="floating-ai-mode-chat"
                  onClick={() => setViewTab("chat")}
                  className={cn(
                    "rounded px-2.5 py-0.5 transition-colors cursor-pointer",
                    viewTab === "chat"
                      ? "bg-card text-xblue font-semibold shadow-2xs"
                      : "text-muted-foreground hover:text-foreground"
                  )}
                >
                  {tx("智能对话（跨页记忆）", "Persistent Chat")}
                </button>
                <button
                  type="button"
                  data-testid="floating-ai-mode-studio"
                  onClick={() => setViewTab("studio_patch")}
                  className={cn(
                    "rounded px-2.5 py-0.5 transition-colors cursor-pointer",
                    viewTab === "studio_patch"
                      ? "bg-card text-xblue font-semibold shadow-2xs"
                      : "text-muted-foreground hover:text-foreground"
                  )}
                >
                  {tx("模型诊断改写", "Schema Copilot")}
                </button>
              </div>
            </div>
          )}

          {viewTab === "studio_patch" && bridge.studio ? (
            <StudioCopilotPanel
              embedded
              activeDraft={bridge.studio.activeDraft}
              selectedNodeName={bridge.studio.selectedNodeName}
              selectedEdgeName={bridge.studio.selectedEdgeName}
              activeInspectorTab={bridge.studio.activeInspectorTab}
              datasets={bridge.studio.datasets}
              onEnsureDraft={bridge.studio.onEnsureDraft}
              onDraftUpdated={bridge.studio.onDraftUpdated}
              onDiagnosticsUpdated={bridge.studio.onDiagnosticsUpdated}
              onLocateTargetPath={bridge.studio.onLocateTargetPath}
            />
          ) : viewTab === "history" ? (
            <div
              data-testid="floating-ai-history-panel"
              className="flex-1 min-h-0 flex flex-col p-3 gap-2.5 overflow-y-auto text-xs"
            >
              <div className="flex items-center justify-between">
                <span className="font-semibold text-muted-foreground">
                  {tx("会话历史记录", "Conversation History")}
                </span>
                <Button
                  size="sm"
                  variant="outline"
                  onClick={startNewSession}
                  className="h-7 px-2 text-[11px]"
                >
                  <MessageSquarePlus className="h-3 w-3" />
                  {tx("新对话", "New")}
                </Button>
              </div>

              <div className="space-y-1.5">
                {store.sessions.map((s) => {
                  const isCurrent = s.id === activeSession.id;
                  return (
                    <div
                      key={s.id}
                      className={cn(
                        "flex items-center justify-between gap-2 rounded-md border px-2.5 py-2 transition-colors",
                        isCurrent
                          ? "border-xblue bg-xblue-soft/35"
                          : "border-border bg-background hover:bg-muted/40"
                      )}
                    >
                      <button
                        type="button"
                        onClick={() => selectLocalSession(s.id)}
                        className="flex-1 min-w-0 text-left cursor-pointer space-y-0.5"
                      >
                        <div className="font-medium truncate">
                          {s.title || tx("未命名新对话", "New Conversation")}
                        </div>
                        <div className="text-[10px] text-muted-foreground flex items-center gap-1.5 font-mono-tech">
                          <span>{s.messages.length} 条消息</span>
                          <span>·</span>
                          <span>{fmtTime(s.updatedAt, true)}</span>
                        </div>
                      </button>
                      <IconButton
                        size="sm"
                        onClick={() => deleteLocalSession(s.id)}
                        aria-label={tx("删除会话", "Delete")}
                      >
                        <Trash2 className="h-3.5 w-3.5 text-muted-foreground hover:text-red-500" />
                      </IconButton>
                    </div>
                  );
                })}
              </div>

              {loadingHistory && (
                <div className="flex items-center gap-1.5 text-[11px] text-muted-foreground py-1">
                  <Loader2 className="h-3 w-3 animate-spin text-xblue" />
                  <span>{tx("同步云端历史会话…", "Syncing server history…")}</span>
                </div>
              )}

              {remoteOnlyConversations.length > 0 && (
                <div className="space-y-1.5 pt-2 border-t border-border">
                  <div className="text-[11px] font-medium text-muted-foreground">
                    {tx("云端智能体历史会话", "Server Agent History")}
                  </div>
                  {remoteOnlyConversations.map((c) => (
                    <button
                      key={c.id}
                      type="button"
                      onClick={() => void loadRemoteConversation(c)}
                      className="w-full text-left rounded-md border border-border bg-background hover:bg-muted/40 px-2.5 py-2 space-y-0.5 cursor-pointer"
                    >
                      <div className="font-medium truncate">
                        {c.title || c.id.slice(0, 8)}
                      </div>
                      <div className="text-[10px] text-muted-foreground font-mono-tech">
                        {fmtTime(c.updated_at || c.created_at, true)}
                      </div>
                    </button>
                  ))}
                </div>
              )}
            </div>
          ) : (
            <div className="flex-1 min-h-0 flex flex-col p-3 gap-2.5 overflow-hidden text-xs">
              {/* Top Bar: Route Context Toggle + Agent Picker */}
              <div className="shrink-0 flex items-center justify-between gap-2 rounded-md border border-border bg-muted/25 px-2.5 py-1.5 text-[11px]">
                <button
                  type="button"
                  onClick={() => setAttachRouteContext((v) => !v)}
                  className={cn(
                    "inline-flex items-center gap-1 rounded px-1.5 py-0.5 transition-colors cursor-pointer truncate",
                    attachRouteContext
                      ? "bg-xblue-soft text-xblue font-medium"
                      : "text-muted-foreground hover:text-foreground"
                  )}
                  title={tx(
                    "开启后会在提问时自动附带当前页面上下文",
                    "Attach current route context when sending"
                  )}
                >
                  <Compass className="h-3 w-3 shrink-0" />
                  <span className="truncate">
                    {attachRouteContext
                      ? tx(`携带位置：${preset.zhTitle}`, `Context: ${preset.enTitle}`)
                      : tx("未携带页面位置", "No Route Context")}
                  </span>
                </button>
                {agents.length > 0 && (
                  <div className="shrink-0 w-36">
                    <Select
                      value={activeSession.agentId}
                      onChange={(e) => {
                        const nextId = e.target.value;
                        const found = agents.find((a) => a.id === nextId);
                        setStore((prev) => ({
                          ...prev,
                          sessions: prev.sessions.map((s) =>
                            s.id === activeSession.id
                              ? {
                                  ...s,
                                  agentId: nextId,
                                  agentName: found?.name ?? s.agentName,
                                }
                              : s
                          ),
                        }));
                      }}
                      className="h-6 px-1.5 text-[11px] font-mono-tech"
                    >
                      {agents.map((a) => (
                        <option key={a.id} value={a.id}>
                          {a.name}
                        </option>
                      ))}
                    </Select>
                  </div>
                )}
              </div>

              {/* Conversation Stream */}
              <div className="flex-1 min-h-0 overflow-y-auto space-y-2.5 pr-1">
                {activeMessages.length === 0 ? (
                  <div className="space-y-2 pt-1">
                    <div className="text-[11px] text-muted-foreground">
                      {tx(
                        "对话记录会自动保存在本地与历史记录中，切换页面也不会丢失：",
                        "Conversations persist across pages and reloads:"
                      )}
                    </div>
                    <div className="flex flex-col gap-1.5">
                      {preset.quickPrompts.map((qp, idx) => (
                        <button
                          key={idx}
                          type="button"
                          onClick={() =>
                            void handleSend(
                              tx(qp.zh, qp.en),
                              isAnalyticsRoute
                            )
                          }
                          className="text-left rounded-md border border-border bg-background hover:border-xblue hover:bg-xblue-soft/30 px-2.5 py-2 text-[11px] leading-snug transition-colors cursor-pointer flex items-center justify-between gap-2"
                        >
                          <span>{tx(qp.zh, qp.en)}</span>
                          <ArrowRight className="h-3 w-3 text-xblue shrink-0" />
                        </button>
                      ))}
                    </div>
                  </div>
                ) : (
                  activeMessages.map((msg) => (
                    <div
                      key={msg.id}
                      className={cn(
                        "rounded-lg px-3 py-2 space-y-1.5",
                        msg.role === "user"
                          ? "bg-xblue text-white ml-8"
                          : "bg-muted/45 border border-border mr-4"
                      )}
                    >
                      {msg.role === "user" && msg.routeTitle && (
                        <div className="text-[10px] opacity-75">
                          @ {msg.routeTitle}
                        </div>
                      )}
                      <div className="whitespace-pre-wrap leading-relaxed break-words">
                        {msg.content}
                      </div>
                      {msg.sql && (
                        <div className="rounded border border-border bg-card p-2 space-y-1.5 text-foreground">
                          <div className="flex items-center justify-between gap-2 text-[10px] text-muted-foreground font-mono-tech">
                            <span className="inline-flex items-center gap-1">
                              <Code2 className="h-3 w-3 text-xblue" />
                              SQL
                            </span>
                            <Button
                              size="sm"
                              variant="outline"
                              className="h-6 px-2 text-[10px]"
                              onClick={() => {
                                if (bridge.analytics) {
                                  bridge.analytics.applySql(msg.sql!);
                                } else {
                                  void navigate("/analytics?tab=sql");
                                }
                              }}
                            >
                              {tx("填入 SQL 编辑器", "Use in SQL Editor")}
                            </Button>
                          </div>
                          <pre className="font-mono-tech text-[11px] overflow-x-auto whitespace-pre-wrap break-all">
                            {msg.sql}
                          </pre>
                        </div>
                      )}
                    </div>
                  ))
                )}

                {sending && (
                  <div className="flex items-center gap-2 text-muted-foreground px-1 py-1">
                    <Loader2 className="h-3.5 w-3.5 animate-spin text-xblue" />
                    <span>{tx("正在思考并检索上下文…", "Thinking…")}</span>
                  </div>
                )}
                <div ref={scrollEndRef} />
              </div>

              {errorMsg && (
                <div className="shrink-0 rounded border border-red-500/40 bg-red-500/10 px-2.5 py-1.5 text-[11px] text-red-600 dark:text-red-400">
                  {errorMsg}
                </div>
              )}

              {/* Input Footer */}
              <div className="shrink-0 border-t border-border pt-2 space-y-1.5">
                <Textarea
                  rows={2}
                  data-testid="floating-ai-input"
                  value={inputText}
                  disabled={sending}
                  onChange={(e) => setInputText(e.target.value)}
                  onKeyDown={(e) => {
                    if (
                      e.key === "Enter" &&
                      (e.metaKey || e.ctrlKey || !e.shiftKey)
                    ) {
                      e.preventDefault();
                      void handleSend();
                    }
                  }}
                  placeholder={tx(
                    "随时提问，切页保留记录（Enter 发送）…",
                    "Ask anything; history persists across pages…"
                  )}
                  className="text-xs resize-none"
                />
                <div className="flex items-center justify-between gap-2 text-[11px] text-muted-foreground">
                  <span className="inline-flex items-center gap-1 truncate">
                    <Bot className="h-3 w-3 text-xblue shrink-0" />
                    <span className="truncate">
                      {activeSession.agentName || "ops_supervisor"}
                    </span>
                  </span>
                  <div className="flex items-center gap-1.5">
                    {isAnalyticsRoute && (
                      <Button
                        type="button"
                        size="sm"
                        variant="outline"
                        disabled={sending || !inputText.trim()}
                        onClick={() => void handleSend(undefined, true)}
                        className="h-7 px-2 text-[11px]"
                      >
                        <Code2 className="h-3 w-3 text-xblue" />
                        {tx("生成 SQL", "SQL")}
                      </Button>
                    )}
                    <Button
                      type="button"
                      size="sm"
                      data-testid="floating-ai-send-btn"
                      disabled={sending || !inputText.trim()}
                      onClick={() => void handleSend()}
                    >
                      {sending ? (
                        <Loader2 className="h-3.5 w-3.5 animate-spin" />
                      ) : (
                        <Send className="h-3.5 w-3.5" />
                      )}
                      {tx("发送", "Send")}
                    </Button>
                  </div>
                </div>
              </div>
            </div>
          )}
        </div>
      )}
    </>
  );
}
