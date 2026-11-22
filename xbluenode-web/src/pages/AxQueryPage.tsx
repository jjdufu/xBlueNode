import { useEffect, useMemo, useRef, useState } from "react";
import Markdown from "react-markdown";
import { useNavigate, useSearchParams } from "react-router";
import {
  ChevronDown,
  ChevronUp,
  Code2,
  Database,
  FileText,
  History,
  Loader2,
  MessageSquarePlus,
  Network,
  PanelLeftClose,
  PanelLeftOpen,
  Send,
  Sparkles,
  ThumbsDown,
  ThumbsUp,
  Trash2,
  Wrench,
} from "lucide-react";
import { useI18n } from "@/i18n";
import { MODULE_META } from "@/modules/meta";
import {
  analyticsApi,
  api,
  type AskCitation,
  type AskFeedbackKind,
  type AskIntent,
  type ChatQueryMode,
  type ChatQueryResponse,
  type ChatQueryScope,
  type RecallChannel,
} from "@/lib/api";
import {
  QueryScopeBar,
  type QueryScopeBarHandle,
} from "@/components/axquery/QueryScopeBar";
import {
  detectAtMention,
  formatAppliedScopeBadge,
  normalizeChatQueryScope,
  stripAtMention,
} from "@/components/axquery/logic";
import { ChartPanel } from "@/components/analytics/ChartPanel";
import { GroundingBadge } from "@/components/analytics/GroundingBadge";
import { ResultGrid } from "@/components/analytics/ResultGrid";
import {
  Badge,
  Button,
  Card,
  IconButton,
  PageHeader,
  Textarea,
} from "@/components/ui/primitives";
import { cn } from "@/lib/utils";

interface ConversationTurn {
  localId: string;
  question: string;
  requestedMode: ChatQueryMode;
  resp: ChatQueryResponse;
  sqlExpanded: boolean;
  slotExpanded?: boolean;
  highlightedCitationIdx: number | null;
  feedback: AskFeedbackKind | null;
  correctSqlOpen: boolean;
  correctedSqlText: string;
  feedbackSubmitting: boolean;
  feedbackNotice: string | null;
}

interface StoredConversationSession {
  id: string;
  title: string;
  updatedAt: string;
  sessionId?: string;
  scope?: ChatQueryScope;
  turns: ConversationTurn[];
}

const SESSIONS_STORAGE_KEY = "axquery_sessions_v1";

function loadStoredSessions(): StoredConversationSession[] {
  try {
    if (typeof localStorage === "undefined") return [];
    const raw = localStorage.getItem(SESSIONS_STORAGE_KEY);
    if (!raw) return [];
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) return [];
    return parsed.slice(0, 20) as StoredConversationSession[];
  } catch {
    return [];
  }
}

function saveStoredSessions(sessions: StoredConversationSession[]): void {
  try {
    if (typeof localStorage === "undefined") return;
    localStorage.setItem(
      SESSIONS_STORAGE_KEY,
      JSON.stringify(sessions.slice(0, 20))
    );
  } catch {
    // Ignore storage quota errors
  }
}

function preprocessAnswerMarkdown(answer: string): string {
  return answer
    .replace(/\[文(\d+)\]/g, "[文$1](#cite-$1)")
    .replace(/\[Doc\s*(\d+)\]/gi, "[Doc $1](#cite-$1)")
    .replace(/\[表\]/g, "[表](#cite-table)")
    .replace(/\[Table\]/gi, "[Table](#cite-table)");
}

function cleanCitationSnippet(raw: string): string {
  if (!raw) return "";
  const lines = raw
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter((line) => {
      if (!line) return false;
      return !/^\|?\s*:?-{2,}:?\s*(\|\s*:?-{2,}:?\s*)+\|?$/.test(line);
    })
    .map((line) => {
      let cleaned = line
        .replace(/^#{1,6}\s+/, "")
        .replace(/\*\*([^*]+)\*\*/g, "$1")
        .replace(/`([^`]+)`/g, "$1");
      if (cleaned.includes("|")) {
        cleaned = cleaned
          .replace(/^\|+|\|+$/g, "")
          .split("|")
          .map((seg) => seg.trim())
          .filter(Boolean)
          .join(" · ");
      }
      return cleaned;
    })
    .filter(Boolean);

  return lines.join(" ").replace(/\s{2,}/g, " ").trim();
}

export function AxQueryPage() {
  const navigate = useNavigate();
  const [searchParams] = useSearchParams();
  const initialQueryParam = searchParams.get("q")?.trim() ?? "";
  const { isZh, tx, fmtNum, fmtTime } = useI18n();

  const [sessions, setSessions] = useState<StoredConversationSession[]>(() =>
    loadStoredSessions()
  );
  const [activeLocalSessionId, setActiveLocalSessionId] = useState<
    string | null
  >(null);
  const [historySidebarOpen, setHistorySidebarOpen] = useState(false);

  const [query, setQuery] = useState(() => initialQueryParam);
  const [caretPos, setCaretPos] = useState<number | null>(null);
  const [dismissedMentionAt, setDismissedMentionAt] = useState<number | null>(
    null
  );
  const [scope, setScope] = useState<ChatQueryScope>({});
  const [mode, setMode] = useState<ChatQueryMode>("AUTO");
  const [sessionId, setSessionId] = useState<string | undefined>(undefined);
  const [turns, setTurns] = useState<ConversationTurn[]>([]);
  const [loading, setLoading] = useState(false);
  const [pendingQuestion, setPendingQuestion] = useState<string>("");
  const [error, setError] = useState<string | null>(null);

  const streamEndRef = useRef<HTMLDivElement | null>(null);
  const scopeBarRef = useRef<QueryScopeBarHandle | null>(null);

  useEffect(() => {
    if (initialQueryParam) {
      setQuery(initialQueryParam);
    }
  }, [initialQueryParam]);

  useEffect(() => {
    if (turns.length > 0 || loading) {
      streamEndRef.current?.scrollIntoView({ behavior: "smooth" });
    }
  }, [turns.length, loading]);

  const channelLabel = (ch: RecallChannel): string => {
    switch (ch) {
      case "vector":
        return tx("语义向量", "Vector");
      case "bm25":
        return tx("全文词法", "BM25");
      case "entity_link":
        return tx("实体关联", "Entity Link");
      case "context":
        return tx("邻段扩展", "Context");
      case "multi_hop":
        return tx("多跳推理", "Multi-Hop");
      default:
        return ch;
    }
  };

  const intentMeta = (
    intent: AskIntent
  ): { label: string; variant: "neutral" | "xblue" | "outline" } => {
    switch (intent) {
      case "DATA":
        return {
          label: tx("DATA · 智能问数", "DATA · Structured SQL"),
          variant: "xblue",
        };
      case "HYBRID":
        return {
          label: tx("HYBRID · 混合研判", "HYBRID · Hybrid Synthesis"),
          variant: "xblue",
        };
      case "CHITCHAT":
        return {
          label: tx("CHITCHAT · 对话", "CHITCHAT · Chat"),
          variant: "outline",
        };
      case "DOC":
      default:
        return {
          label: tx("DOC · 文档溯源", "DOC · Knowledge Recall"),
          variant: "neutral",
        };
    }
  };

  const modeMeta = useMemo(
    () => ({
      AUTO: {
        label: tx("智能路由", "Auto"),
        hint: tx(
          "自动研判问题意图，融合五路超图召回与受控 SQL 问数",
          "Auto-routes between 5-way hypergraph recall and guarded NL2SQL"
        ),
        placeholder: tx(
          "输入业务数据或制度问题（Enter 发送，Shift+Enter 换行）…",
          "Ask across metrics, tables, and policies (Enter to send)…"
        ),
        icon: Sparkles,
      },
      DOC: {
        label: tx("知识检索", "Docs"),
        hint: tx(
          "限定执行五路知识超图召回（向量 / BM25 / 实体 / 邻段 / 多跳）",
          "5-channel knowledge recall across documents and entities"
        ),
        placeholder: tx(
          "输入制度条款、SOP 规范或实体关联问题…",
          "Ask about operational policies, SOPs, or entity relations…"
        ),
        icon: FileText,
      },
      DATA: {
        label: tx("智能问数", "Data"),
        hint: tx(
          "限定执行受控 NL2SQL 结构化查询、脱敏表格与图表生成",
          "Guarded NL2SQL structured analytics with automatic chart generation"
        ),
        placeholder: tx(
          "输入指标汇总、分组排名或时间趋势统计问题…",
          "Ask for metric aggregations, rankings, or time-series breakdowns…"
        ),
        icon: Database,
      },
    }),
    [tx]
  );

  const starterCards: readonly {
    category: ChatQueryMode;
    tag: string;
    question: string;
  }[] = [
    {
      category: "AUTO",
      tag: tx("融合研判", "Hybrid"),
      question: tx(
        "已支付订单总金额是多少？华东仓履约时效制度有何规定？",
        "What is the total paid order amount and East Warehouse fulfillment SLA?"
      ),
    },
    {
      category: "DATA",
      tag: tx("智能问数", "Data SQL"),
      question: tx(
        "按履约状态统计订单笔数与金额分布",
        "Break down order counts and amounts by fulfillment status"
      ),
    },
    {
      category: "DOC",
      tag: tx("制度检索", "Knowledge"),
      question: tx(
        "华东仓安全库存预警与调拨规范有哪些要求？",
        "What are the safety stock thresholds and transfer rules for East Warehouse?"
      ),
    },
    {
      category: "DATA",
      tag: tx("供需对比", "Data SQL"),
      question: tx(
        "各仓库在库库存与下游需求匹配情况如何？",
        "Compare warehouse inventory levels against downstream order demand"
      ),
    },
  ];

  const persistSessionSnapshot = (
    localId: string,
    nextSessionId: string | undefined,
    nextTurns: ConversationTurn[],
    nextScope: ChatQueryScope = scope
  ) => {
    if (nextTurns.length === 0) return;
    const titleText = nextTurns[0]?.question.slice(0, 36) ?? tx("会话", "Chat");
    const cleanScope = normalizeChatQueryScope(nextScope);
    setSessions((prev) => {
      const entry: StoredConversationSession = {
        id: localId,
        title: titleText,
        updatedAt: new Date().toISOString(),
        sessionId: nextSessionId,
        ...(cleanScope ? { scope: cleanScope } : {}),
        turns: nextTurns,
      };
      const filtered = prev.filter((s) => s.id !== localId);
      const updated = [entry, ...filtered].slice(0, 20);
      saveStoredSessions(updated);
      return updated;
    });
  };

  const handleScopeChange = (nextScope: ChatQueryScope) => {
    setScope(nextScope);
    if (activeLocalSessionId && turns.length > 0) {
      persistSessionSnapshot(activeLocalSessionId, sessionId, turns, nextScope);
    }
  };

  const activeMention = useMemo(() => {
    const m = detectAtMention(query, caretPos);
    if (!m) return null;
    if (dismissedMentionAt !== null && m.start === dismissedMentionAt) {
      return null;
    }
    return m;
  }, [caretPos, dismissedMentionAt, query]);

  const submitQuestion = async (
    rawQuestion: string,
    overrideMode?: ChatQueryMode
  ) => {
    const trimmed = rawQuestion.trim();
    if (!trimmed || loading) return;
    const effectiveMode = overrideMode ?? mode;
    if (overrideMode && overrideMode !== mode) {
      setMode(overrideMode);
    }

    setLoading(true);
    setPendingQuestion(trimmed);
    setError(null);
    try {
      const activeScope = normalizeChatQueryScope(scope);
      const data = await api.chatQuery({
        query: trimmed,
        top_k: 6,
        mode: effectiveMode,
        ...(sessionId ? { session_id: sessionId } : {}),
        ...(activeScope ? { scope: activeScope } : {}),
      });

      const resolvedSessionId = data.session_id ?? sessionId;
      if (data.session_id) {
        setSessionId(data.session_id);
      }

      const nextTurn: ConversationTurn = {
        localId: data.turn_id || `turn-${Date.now()}`,
        question: trimmed,
        requestedMode: effectiveMode,
        resp: data,
        sqlExpanded: false,
        slotExpanded: false,
        highlightedCitationIdx: null,
        feedback: null,
        correctSqlOpen: false,
        correctedSqlText: data.sql ?? "",
        feedbackSubmitting: false,
        feedbackNotice: null,
      };

      const localSessKey =
        activeLocalSessionId ?? resolvedSessionId ?? `sess-${Date.now()}`;
      setActiveLocalSessionId(localSessKey);
      setTurns((prev) => {
        const nextList = [...prev, nextTurn];
        persistSessionSnapshot(
          localSessKey,
          resolvedSessionId,
          nextList,
          scope
        );
        return nextList;
      });
      setQuery("");
      setCaretPos(null);
      setDismissedMentionAt(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
      setPendingQuestion("");
    }
  };

  const handleAsk = async () => {
    await submitQuestion(query);
  };

  const handleResetSession = () => {
    setActiveLocalSessionId(null);
    setSessionId(undefined);
    setScope({});
    setTurns([]);
    setError(null);
  };

  const handleSelectStoredSession = (sess: StoredConversationSession) => {
    setActiveLocalSessionId(sess.id);
    setSessionId(sess.sessionId);
    setScope(sess.scope ?? {});
    setTurns(sess.turns);
    setError(null);
  };

  const handleDeleteStoredSession = (id: string) => {
    setSessions((prev) => {
      const next = prev.filter((s) => s.id !== id);
      saveStoredSessions(next);
      return next;
    });
    if (activeLocalSessionId === id) {
      handleResetSession();
    }
  };

  const updateTurn = (
    localId: string,
    updater: (t: ConversationTurn) => ConversationTurn
  ) => {
    setTurns((prev) => {
      const next = prev.map((t) => (t.localId === localId ? updater(t) : t));
      if (activeLocalSessionId) {
        persistSessionSnapshot(activeLocalSessionId, sessionId, next);
      }
      return next;
    });
  };

  const handleFeedback = async (
    turn: ConversationTurn,
    kind: AskFeedbackKind,
    correctedSql?: string
  ) => {
    const turnId = turn.resp.turn_id;
    updateTurn(turn.localId, (t) => ({
      ...t,
      feedbackSubmitting: true,
      feedbackNotice: null,
    }));

    try {
      if (turnId) {
        await analyticsApi.submitAskFeedback(turnId, {
          feedback: kind,
          ...(correctedSql && correctedSql.trim()
            ? { corrected_sql: correctedSql.trim() }
            : {}),
        });
      }
      updateTurn(turn.localId, (t) => ({
        ...t,
        feedback: kind,
        feedbackSubmitting: false,
        correctSqlOpen: correctedSql ? false : kind === "DOWN",
        feedbackNotice: correctedSql
          ? tx("修正 SQL 已提交", "Corrected SQL submitted")
          : kind === "UP"
            ? tx("已记录点赞反馈", "Helpful feedback recorded")
            : tx("已记录反馈", "Feedback recorded"),
      }));
    } catch (err) {
      updateTurn(turn.localId, (t) => ({
        ...t,
        feedbackSubmitting: false,
        feedbackNotice: err instanceof Error ? err.message : String(err),
      }));
    }
  };

  const scrollToCitation = (turnLocalId: string, zeroBasedIdx: number) => {
    updateTurn(turnLocalId, (t) => ({
      ...t,
      highlightedCitationIdx: zeroBasedIdx,
    }));
    if (typeof document !== "undefined") {
      const el = document.getElementById(
        `citation-${turnLocalId}-${zeroBasedIdx}`
      );
      el?.scrollIntoView({ behavior: "smooth", block: "nearest" });
    }
  };

  const expandTurnSql = (turnLocalId: string) => {
    updateTurn(turnLocalId, (t) => ({
      ...t,
      sqlExpanded: true,
    }));
  };

  const isEmptyWelcome = turns.length === 0 && !loading;

  const renderComposerBox = (rowsCount: number) => (
    <div className="rounded-xl border border-border bg-card shadow-xs focus-within:border-xblue focus-within:ring-2 focus-within:ring-xblue/15 transition-all">
      <QueryScopeBar
        ref={scopeBarRef}
        scope={scope}
        onChange={handleScopeChange}
        mentionKeyword={activeMention?.keyword ?? null}
        popoverPlacement={isEmptyWelcome ? "bottom" : "top"}
        onDismissMention={() => {
          if (activeMention) {
            setDismissedMentionAt(activeMention.start);
          }
        }}
        onCommitMentionSelection={(nextScope) => {
          handleScopeChange(nextScope);
          if (activeMention) {
            const stripped = stripAtMention(query, activeMention);
            setQuery(stripped);
            setCaretPos(stripped.length);
            setDismissedMentionAt(null);
          }
        }}
      />
      <Textarea
        rows={rowsCount}
        className="border-0 bg-transparent px-3.5 pt-3 pb-1.5 text-sm leading-relaxed focus:ring-0 resize-none"
        placeholder={modeMeta[mode].placeholder}
        value={query}
        onChange={(e) => {
          setQuery(e.target.value);
          setCaretPos(e.target.selectionStart);
          setDismissedMentionAt(null);
        }}
        onClick={(e) => setCaretPos(e.currentTarget.selectionStart)}
        onKeyUp={(e) => setCaretPos(e.currentTarget.selectionStart)}
        onKeyDown={(e) => {
          if (scopeBarRef.current?.handleMentionKeyDown(e)) {
            return;
          }
          if (e.key === "Enter" && !e.shiftKey) {
            e.preventDefault();
            void handleAsk();
          }
        }}
        data-testid="axquery-input"
      />

      {/* Embedded Bottom Toolbar: Left Mode Switcher + Live Hint, Right Brand-Blue Send Button */}
      <div className="flex flex-wrap items-center justify-between gap-2 px-2.5 pb-2.5 pt-1">
        <div className="flex flex-wrap items-center gap-2 min-w-0">
          <div
            role="tablist"
            data-testid="axquery-mode-switcher"
            className="inline-flex items-center gap-0.5 rounded-lg border border-border bg-muted/50 p-0.5"
          >
            {(["AUTO", "DOC", "DATA"] as const).map((mKey) => {
              const active = mode === mKey;
              const Icon = modeMeta[mKey].icon;
              return (
                <button
                  key={mKey}
                  type="button"
                  role="tab"
                  aria-selected={active}
                  title={modeMeta[mKey].hint}
                  data-testid={`axquery-mode-${mKey}`}
                  onClick={() => setMode(mKey)}
                  className={cn(
                    "inline-flex items-center gap-1 rounded-md px-2.5 py-1 text-xs transition-colors cursor-pointer",
                    active
                      ? "bg-xblue-soft text-xblue font-semibold border border-xblue/30 shadow-2xs"
                      : "font-medium text-muted-foreground hover:text-foreground border border-transparent"
                  )}
                >
                  <Icon className="h-3.5 w-3.5 shrink-0" />
                  <span>{modeMeta[mKey].label}</span>
                </button>
              );
            })}
          </div>

          <span className="hidden md:inline text-[11px] text-muted-foreground truncate">
            {modeMeta[mode].hint}
          </span>
        </div>

        <Button
          type="button"
          size="md"
          onClick={() => void handleAsk()}
          disabled={loading || !query.trim()}
          data-testid="axquery-submit-btn"
        >
          {loading ? (
            <Loader2 className="h-3.5 w-3.5 animate-spin" />
          ) : (
            <Send className="h-3.5 w-3.5" />
          )}
          <span>{loading ? tx("研判中…", "Running…") : tx("提问", "Ask")}</span>
        </Button>
      </div>
    </div>
  );

  return (
    <div
      className="flex flex-1 min-h-0 flex-col gap-3 overflow-hidden"
      data-testid="axquery-page"
    >
      <PageHeader
        className="shrink-0 pb-2.5"
        title={tx(MODULE_META.axquery.label, MODULE_META.axquery.enLabel)}
        badges={
          sessionId ? (
            <Badge
              variant="outline"
              data-testid="axquery-session-badge"
              title={sessionId}
            >
              {tx("会话", "Session")} {sessionId.slice(0, 8)}
            </Badge>
          ) : undefined
        }
        actions={
          <div className="flex items-center gap-1.5">
            <Button
              type="button"
              variant="ghost"
              size="sm"
              onClick={() => setHistorySidebarOpen((v) => !v)}
              className={
                historySidebarOpen ? "bg-xblue-soft text-xblue" : undefined
              }
              data-testid="axquery-history-toggle-btn"
            >
              {historySidebarOpen ? (
                <PanelLeftClose className="h-3.5 w-3.5" />
              ) : (
                <History className="h-3.5 w-3.5 text-xblue" />
              )}
              <span>{tx("历史会话", "Sessions")}</span>
              {sessions.length > 0 && (
                <span className="font-mono-tech text-[11px] tabular-nums opacity-75">
                  {sessions.length}
                </span>
              )}
            </Button>

            {turns.length > 0 && (
              <Button
                type="button"
                variant="outline"
                size="sm"
                onClick={handleResetSession}
                data-testid="axquery-new-session-btn"
              >
                <MessageSquarePlus className="h-3.5 w-3.5 text-xblue" />
                {tx("新对话", "New Chat")}
              </Button>
            )}
          </div>
        }
      />

      {/* Main Conversational Body: Optional Collapsible Session Sidebar + Chat Canvas */}
      <div className="flex flex-1 min-h-0 gap-3 overflow-hidden">
        {historySidebarOpen && (
          <aside className="w-64 shrink-0 rounded-md border border-border bg-card flex flex-col overflow-hidden">
            <div className="flex items-center justify-between border-b border-border px-3 py-2 text-xs">
              <span className="font-semibold flex items-center gap-1.5">
                <History className="h-3.5 w-3.5 text-xblue" />
                {tx("会话记录", "Chat History")}
              </span>
              <Button
                type="button"
                variant="ghost"
                size="sm"
                onClick={handleResetSession}
              >
                <MessageSquarePlus className="h-3.5 w-3.5 text-xblue" />
                {tx("新建", "New")}
              </Button>
            </div>
            <div className="flex-1 overflow-y-auto p-2 space-y-1 text-xs">
              {sessions.length === 0 ? (
                <div className="py-8 text-center text-muted-foreground">
                  {tx("暂无历史会话", "No saved sessions")}
                </div>
              ) : (
                sessions.map((sess) => {
                  const active = sess.id === activeLocalSessionId;
                  return (
                    <div
                      key={sess.id}
                      className={cn(
                        "group flex items-center justify-between gap-1.5 rounded-md px-2.5 py-2 transition-colors cursor-pointer",
                        active
                          ? "bg-xblue-soft text-xblue font-medium"
                          : "hover:bg-muted/60 text-foreground"
                      )}
                      onClick={() => handleSelectStoredSession(sess)}
                    >
                      <div className="min-w-0 flex-1">
                        <div className="truncate">{sess.title}</div>
                        <div className="text-[10px] font-mono-tech text-muted-foreground">
                          {sess.turns.length} {tx("轮", "turns")} ·{" "}
                          {fmtTime(sess.updatedAt, true)}
                        </div>
                      </div>
                      <IconButton
                        size="sm"
                        onClick={(e) => {
                          e.stopPropagation();
                          handleDeleteStoredSession(sess.id);
                        }}
                        aria-label={tx("删除会话", "Delete session")}
                        className="opacity-0 group-hover:opacity-100 h-6 w-6 hover:text-red-600"
                      >
                        <Trash2 className="h-3 w-3" />
                      </IconButton>
                    </div>
                  );
                })
              )}
            </div>
          </aside>
        )}

        <div className="flex-1 min-w-0 flex flex-col min-h-0 overflow-hidden">
          {isEmptyWelcome ? (
            /* Centered Conversational Welcome Layout (Eliminates 70% dead whitespace) */
            <div className="flex-1 overflow-y-auto flex flex-col justify-center">
              <div className="w-full max-w-[800px] mx-auto px-2 py-6 space-y-5">
                <div className="text-center space-y-2">
                  <div className="inline-flex items-center gap-1.5 rounded-full bg-xblue-soft px-3 py-1 text-xs font-medium text-xblue">
                    <Sparkles className="h-3.5 w-3.5" />
                    <span>
                      {tx(
                        "五路知识超图召回 × 受控智能问数",
                        "5-Way Hypergraph Recall × Guarded NL2SQL"
                      )}
                    </span>
                  </div>
                  <h2 className="text-xl font-semibold tracking-tight text-foreground">
                    {tx(
                      "有什么业务数据或企业制度问题需要研判？",
                      "Ask across enterprise data and knowledge"
                    )}
                  </h2>
                  <p className="text-xs text-muted-foreground">
                    {tx(
                      "融合语义向量、全文词法、实体关联、邻段扩展、多跳推理与脱敏 SQL 实时计算",
                      "Combines vector, BM25, entity link, context window, multi-hop graph recall, and PII-guarded SQL"
                    )}
                  </p>
                </div>

                {error && (
                  <Card className="p-3 border-red-500/30 bg-red-500/5 text-xs">
                    <span className="font-semibold text-red-600 dark:text-red-400">
                      {tx("请求失败：", "Request failed: ")}
                    </span>
                    <span>{error}</span>
                  </Card>
                )}

                {renderComposerBox(3)}

                {/* 2x2 Categorized Starter Prompt Cards Linked with Selected Mode */}
                <div
                  className="space-y-2"
                  data-testid="axquery-starter-prompts"
                >
                  <div className="flex items-center justify-between text-xs text-muted-foreground px-0.5">
                    <span>{tx("推荐提问", "Suggested Prompts")}</span>
                    {!historySidebarOpen && sessions.length > 0 && (
                      <button
                        type="button"
                        onClick={() => setHistorySidebarOpen(true)}
                        className="inline-flex items-center gap-1 text-xblue hover:underline cursor-pointer"
                      >
                        <PanelLeftOpen className="h-3 w-3" />
                        <span>
                          {tx(
                            `恢复最近会话 (${sessions.length})`,
                            `Recent sessions (${sessions.length})`
                          )}
                        </span>
                      </button>
                    )}
                  </div>

                  <div className="grid grid-cols-1 sm:grid-cols-2 gap-2.5">
                    {starterCards.map((card, idx) => {
                      const matchesMode =
                        mode === "AUTO" || card.category === mode;
                      return (
                        <button
                          key={idx}
                          type="button"
                          onClick={() => {
                            setQuery(card.question);
                            void submitQuestion(card.question, card.category);
                          }}
                          data-testid={`axquery-starter-${idx}`}
                          className={cn(
                            "group flex flex-col items-start gap-1.5 rounded-lg border p-3 text-left text-xs transition-all cursor-pointer",
                            matchesMode
                              ? "border-border bg-card hover:border-xblue hover:bg-xblue-soft/25"
                              : "border-border/60 bg-muted/15 opacity-60 hover:opacity-100 hover:border-xblue"
                          )}
                        >
                          <Badge
                            variant={
                              card.category === mode && mode !== "AUTO"
                                ? "xblue"
                                : "neutral"
                            }
                          >
                            {card.tag}
                          </Badge>
                          <span className="text-foreground font-medium leading-snug">
                            {card.question}
                          </span>
                        </button>
                      );
                    })}
                  </div>
                </div>
              </div>
            </div>
          ) : (
            /* Active Conversation Stream + Bottom-Docked Multiline Composer */
            <>
              <div
                className="flex-1 min-h-0 overflow-y-auto pr-1 space-y-4 pb-2"
                data-testid="axquery-turns-list"
              >
                {turns.map((turn, turnIndex) => {
                  const { resp } = turn;
                  const intent: AskIntent = resp.intent ?? "DOC";
                  const meta = intentMeta(intent);
                  const sqlText = resp.sql ?? "";
                  const dataColumns = Array.isArray(resp.columns)
                    ? resp.columns
                    : [];
                  const dataRows = Array.isArray(resp.rows) ? resp.rows : [];
                  const hasDataPayload =
                    Boolean(sqlText.trim()) ||
                    dataRows.length > 0 ||
                    intent === "DATA" ||
                    intent === "HYBRID";

                  const citations: AskCitation[] = Array.isArray(resp.citations)
                    ? resp.citations
                    : [];
                  const hits = Array.isArray(resp.hits) ? resp.hits : [];
                  const relatedNodes = Array.isArray(resp.related_nodes)
                    ? resp.related_nodes
                    : [];
                  const seedEntities = Array.isArray(resp.seed_entities)
                    ? resp.seed_entities
                    : [];
                  const verifiedNumbers = resp.verified_numbers ?? 0;
                  const verifiedCitations = resp.verified_citations ?? 0;
                  const hasSlotPayload = resp.slot_payload != null;
                  const isAbstained = resp.abstained === true;
                  const appliedScopeLabel = formatAppliedScopeBadge(
                    resp.applied_scope,
                    isZh
                  );

                  return (
                    <div
                      key={turn.localId}
                      className="space-y-4 rounded-md border border-border bg-card/50 p-4"
                      data-testid={`axquery-turn-${turnIndex}`}
                    >
                      {/* Question Header & Intent Badge */}
                      <div className="flex flex-wrap items-center justify-between gap-2 border-b border-border pb-3">
                        <div className="flex flex-wrap items-center gap-2">
                          <Badge variant="outline">Q{turnIndex + 1}</Badge>
                          <span className="text-sm font-semibold text-foreground">
                            {turn.question}
                          </span>
                        </div>

                        <div className="flex flex-wrap items-center gap-2">
                          {appliedScopeLabel && (
                            <Badge
                              variant="neutral"
                              data-testid="turn-applied-scope-badge"
                            >
                              {appliedScopeLabel}
                            </Badge>
                          )}
                          <Badge
                            variant={meta.variant}
                            data-testid="axquery-intent-badge"
                          >
                            {meta.label}
                          </Badge>
                          {resp.grounding_mode && (
                            <GroundingBadge mode={resp.grounding_mode} />
                          )}
                          {(verifiedNumbers > 0 || verifiedCitations > 0) && (
                            <div
                              className="inline-flex items-center gap-1.5"
                              data-testid="grounding-evidence-counts"
                            >
                              {verifiedNumbers > 0 && (
                                <Badge variant="outline">
                                  {tx(
                                    `核验数值 ${verifiedNumbers}`,
                                    `Verified ${verifiedNumbers}`
                                  )}
                                </Badge>
                              )}
                              {verifiedCitations > 0 && (
                                <Badge variant="outline">
                                  {tx(
                                    `核验引用 ${verifiedCitations}`,
                                    `Citations ${verifiedCitations}`
                                  )}
                                </Badge>
                              )}
                            </div>
                          )}
                          <span className="text-[11px] font-mono-tech tabular-nums text-muted-foreground">
                            {fmtNum(resp.elapsed_ms, 0)} ms
                          </span>
                        </div>
                      </div>

                      <div className="grid grid-cols-1 lg:grid-cols-12 gap-4">
                        {/* Left Column: Markdown Answer + Expandable SQL/Data + Related Nodes + Feedback */}
                        <div className="lg:col-span-7 space-y-3">
                          <Card
                            className={cn(
                              "p-4 space-y-3",
                              isAbstained &&
                                "border-dashed border-muted-foreground/45 bg-muted/20"
                            )}
                          >
                            <div className="text-sm leading-relaxed space-y-2 text-foreground">
                              <Markdown
                                components={{
                                  p: ({ children }) => (
                                    <p className="leading-relaxed last:mb-0">
                                      {children}
                                    </p>
                                  ),
                                  ul: ({ children }) => (
                                    <ul className="list-disc pl-5 space-y-1">
                                      {children}
                                    </ul>
                                  ),
                                  ol: ({ children }) => (
                                    <ol className="list-decimal pl-5 space-y-1">
                                      {children}
                                    </ol>
                                  ),
                                  li: ({ children }) => (
                                    <li className="leading-relaxed">
                                      {children}
                                    </li>
                                  ),
                                  strong: ({ children }) => (
                                    <strong className="font-semibold text-foreground">
                                      {children}
                                    </strong>
                                  ),
                                  code: ({ children }) => (
                                    <code className="rounded bg-muted px-1.5 py-0.5 font-mono-tech text-xs">
                                      {children}
                                    </code>
                                  ),
                                  pre: ({ children }) => (
                                    <pre className="overflow-x-auto rounded border border-border bg-muted/40 p-2.5 font-mono-tech text-xs">
                                      {children}
                                    </pre>
                                  ),
                                  h1: ({ children }) => (
                                    <h3 className="text-sm font-semibold pt-1">
                                      {children}
                                    </h3>
                                  ),
                                  h2: ({ children }) => (
                                    <h3 className="text-sm font-semibold pt-1">
                                      {children}
                                    </h3>
                                  ),
                                  h3: ({ children }) => (
                                    <h4 className="text-xs font-semibold pt-1">
                                      {children}
                                    </h4>
                                  ),
                                  blockquote: ({ children }) => (
                                    <blockquote className="border-l-2 border-xblue pl-3 text-muted-foreground">
                                      {children}
                                    </blockquote>
                                  ),
                                  a: ({ href, children }) => {
                                    if (href === "#cite-table") {
                                      return (
                                        <button
                                          type="button"
                                          onClick={(e) => {
                                            e.preventDefault();
                                            if (hasDataPayload) {
                                              expandTurnSql(turn.localId);
                                            }
                                          }}
                                          className="mx-0.5 inline-flex items-center rounded border border-xblue/30 bg-xblue-soft/60 px-1.5 py-0.5 font-mono-tech text-[11px] font-medium text-xblue hover:bg-xblue-soft cursor-pointer align-baseline"
                                        >
                                          [{tx("表", "Table")}]
                                        </button>
                                      );
                                    }
                                    const citeMatch = /^#cite-(\d+)$/.exec(
                                      href ?? ""
                                    );
                                    if (citeMatch) {
                                      const citNum = Number(citeMatch[1]);
                                      return (
                                        <button
                                          type="button"
                                          onClick={(e) => {
                                            e.preventDefault();
                                            scrollToCitation(
                                              turn.localId,
                                              Math.max(0, citNum - 1)
                                            );
                                          }}
                                          className="mx-0.5 inline-flex items-center rounded bg-xblue-soft px-1.5 py-0.5 font-mono-tech text-[11px] font-medium text-xblue hover:underline cursor-pointer align-baseline"
                                        >
                                          [
                                          {tx(
                                            `文${citNum}`,
                                            `Doc ${citNum}`
                                          )}
                                          ]
                                        </button>
                                      );
                                    }
                                    return (
                                      <a
                                        href={href}
                                        target="_blank"
                                        rel="noreferrer"
                                        className="text-xblue underline"
                                      >
                                        {children}
                                      </a>
                                    );
                                  },
                                }}
                              >
                                {preprocessAnswerMarkdown(resp.answer)}
                              </Markdown>
                            </div>

                            {seedEntities.length > 0 && (
                              <div className="pt-2.5 border-t border-border flex flex-wrap items-center gap-1.5">
                                <span className="text-[11px] text-muted-foreground mr-1">
                                  {tx("种子实体", "Seed Entities")}
                                </span>
                                {seedEntities.map((ent) => (
                                  <Badge key={ent} variant="xblue">
                                    {ent}
                                  </Badge>
                                ))}
                              </div>
                            )}

                            {relatedNodes.length > 0 && (
                              <div
                                className="pt-2.5 border-t border-border space-y-1.5"
                                data-testid="axquery-related-nodes"
                              >
                                <div className="flex items-center gap-1 text-[11px] font-medium text-muted-foreground">
                                  <Network className="h-3 w-3 text-xblue" />
                                  <span>
                                    {tx("关联业务节点", "Related Nodes")} (
                                    {fmtNum(relatedNodes.length, 0)})
                                  </span>
                                </div>
                                <div className="flex flex-wrap gap-2">
                                  {relatedNodes.map((node) => (
                                    <button
                                      key={node.id}
                                      type="button"
                                      onClick={() => void navigate("/graph")}
                                      data-testid={`axquery-related-node-${node.id}`}
                                      className="inline-flex items-center gap-1.5 rounded border border-border bg-muted/40 px-2.5 py-1 text-xs hover:border-xblue hover:bg-xblue-soft/30 cursor-pointer"
                                    >
                                      <Badge variant="outline">
                                        {node.type_name}
                                      </Badge>
                                      <span className="font-medium text-foreground">
                                        {node.title}
                                      </span>
                                    </button>
                                  ))}
                                </div>
                              </div>
                            )}

                            {/* Feedback & SQL Correction Controls */}
                            <div className="pt-2.5 border-t border-border flex flex-wrap items-center justify-between gap-2 text-xs">
                              <div className="flex flex-wrap items-center gap-1.5">
                                <Button
                                  type="button"
                                  size="sm"
                                  variant={
                                    turn.feedback === "UP"
                                      ? "primary"
                                      : "outline"
                                  }
                                  disabled={turn.feedbackSubmitting}
                                  onClick={() =>
                                    void handleFeedback(turn, "UP")
                                  }
                                  data-testid="axquery-feedback-up"
                                >
                                  <ThumbsUp className="h-3 w-3" />
                                  {tx("有帮助", "Helpful")}
                                </Button>

                                <Button
                                  type="button"
                                  size="sm"
                                  variant={
                                    turn.feedback === "DOWN"
                                      ? "primary"
                                      : "outline"
                                  }
                                  disabled={turn.feedbackSubmitting}
                                  onClick={() =>
                                    void handleFeedback(turn, "DOWN")
                                  }
                                  data-testid="axquery-feedback-down"
                                >
                                  <ThumbsDown className="h-3 w-3" />
                                  {tx("待改进", "Not Helpful")}
                                </Button>

                                {hasDataPayload && (
                                  <Button
                                    type="button"
                                    size="sm"
                                    variant="outline"
                                    onClick={() =>
                                      updateTurn(turn.localId, (t) => ({
                                        ...t,
                                        correctSqlOpen: !t.correctSqlOpen,
                                      }))
                                    }
                                    data-testid="axquery-correct-sql-toggle"
                                  >
                                    <Wrench className="h-3 w-3 text-xblue" />
                                    {tx("修正 SQL", "Correct SQL")}
                                  </Button>
                                )}
                              </div>

                              {(hasSlotPayload || hasDataPayload) && (
                                <div className="flex flex-wrap items-center gap-1.5">
                                  {hasSlotPayload && (
                                    <Button
                                      type="button"
                                      size="sm"
                                      variant="outline"
                                      onClick={() =>
                                        updateTurn(turn.localId, (t) => ({
                                          ...t,
                                          slotExpanded: !t.slotExpanded,
                                        }))
                                      }
                                      data-testid="grounding-slot-toggle"
                                    >
                                      {tx("槽位参数", "Slots")}
                                      {turn.slotExpanded ? (
                                        <ChevronUp className="h-3.5 w-3.5" />
                                      ) : (
                                        <ChevronDown className="h-3.5 w-3.5" />
                                      )}
                                    </Button>
                                  )}

                                  {hasDataPayload && (
                                    <Button
                                      type="button"
                                      size="sm"
                                      variant="outline"
                                      onClick={() =>
                                        updateTurn(turn.localId, (t) => ({
                                          ...t,
                                          sqlExpanded: !t.sqlExpanded,
                                        }))
                                      }
                                      data-testid="axquery-toggle-sql-btn"
                                    >
                                      <Code2 className="h-3.5 w-3.5 text-xblue" />
                                      {turn.sqlExpanded
                                        ? tx("收起数据", "Hide Data")
                                        : tx("查看数据", "View Data")}
                                      {turn.sqlExpanded ? (
                                        <ChevronUp className="h-3.5 w-3.5" />
                                      ) : (
                                        <ChevronDown className="h-3.5 w-3.5" />
                                      )}
                                    </Button>
                                  )}
                                </div>
                              )}
                            </div>

                            {turn.feedbackNotice && (
                              <div
                                className="text-[11px] font-mono-tech text-xblue"
                                data-testid="axquery-feedback-notice"
                              >
                                {turn.feedbackNotice}
                              </div>
                            )}

                            {turn.correctSqlOpen && (
                              <div
                                className="space-y-2 rounded border border-border bg-muted/30 p-3"
                                data-testid="axquery-correct-sql-box"
                              >
                                <div className="text-xs font-medium text-muted-foreground">
                                  {tx("修正查询语句", "Corrected SQL")}
                                </div>
                                <Textarea
                                  rows={3}
                                  value={turn.correctedSqlText}
                                  onChange={(e) => {
                                    const val = e.target.value;
                                    updateTurn(turn.localId, (t) => ({
                                      ...t,
                                      correctedSqlText: val,
                                    }));
                                  }}
                                  placeholder="SELECT SUM(amount) FROM demo_orders WHERE status = 'PAID'"
                                  className="font-mono-tech text-xs"
                                  data-testid="axquery-corrected-sql-input"
                                />
                                <div className="flex justify-end gap-2">
                                  <Button
                                    type="button"
                                    size="sm"
                                    variant="ghost"
                                    onClick={() =>
                                      updateTurn(turn.localId, (t) => ({
                                        ...t,
                                        correctSqlOpen: false,
                                      }))
                                    }
                                  >
                                    {tx("取消", "Cancel")}
                                  </Button>
                                  <Button
                                    type="button"
                                    size="sm"
                                    variant="primary"
                                    disabled={
                                      turn.feedbackSubmitting ||
                                      !turn.correctedSqlText.trim()
                                    }
                                    onClick={() =>
                                      void handleFeedback(
                                        turn,
                                        "DOWN",
                                        turn.correctedSqlText
                                      )
                                    }
                                    data-testid="axquery-submit-corrected-sql"
                                  >
                                    {tx("提交", "Submit")}
                                  </Button>
                                </div>
                              </div>
                            )}
                          </Card>

                          {hasSlotPayload && turn.slotExpanded && (
                            <Card
                              className="p-3 bg-muted/20 space-y-1.5"
                              data-testid="grounding-slot-panel"
                            >
                              <div className="text-[11px] font-medium text-muted-foreground">
                                {tx("槽位参数", "Slots")}
                              </div>
                              <pre className="overflow-x-auto font-mono-tech text-xs text-foreground whitespace-pre-wrap">
                                {JSON.stringify(resp.slot_payload, null, 2)}
                              </pre>
                            </Card>
                          )}

                          {hasDataPayload && turn.sqlExpanded && (
                            <div
                              className="space-y-3"
                              data-testid="axquery-sql-result-panel"
                            >
                              {sqlText && (
                                <Card className="p-3 bg-muted/20 space-y-1.5">
                                  <div className="text-[11px] font-medium text-muted-foreground">
                                    {tx("受控 SQL 语句", "Guarded SQL")}
                                  </div>
                                  <pre
                                    className="overflow-x-auto font-mono-tech text-xs text-foreground whitespace-pre-wrap"
                                    data-testid="axquery-sql-code"
                                  >
                                    {sqlText}
                                  </pre>
                                </Card>
                              )}

                              <ResultGrid
                                columns={dataColumns}
                                rows={dataRows}
                                rowCount={dataRows.length}
                                maxHeight={240}
                                testId="axquery-result-grid"
                              />

                              <ChartPanel
                                columns={dataColumns}
                                rows={dataRows}
                                suggestion={resp.chart ?? null}
                                testId="axquery-chart-panel"
                              />
                            </div>
                          )}
                        </div>

                        {/* Right Column: Citations & Five-Channel Fused Recall Evidence */}
                        <div className="lg:col-span-5 space-y-2.5">
                          <div className="flex items-center justify-between text-xs font-medium text-muted-foreground">
                            <span className="inline-flex items-center gap-1">
                              <FileText className="h-3.5 w-3.5 text-xblue" />
                              {tx(
                                "引用与召回证据",
                                "Citations & Recall Evidence"
                              )}{" "}
                              ({fmtNum(citations.length || hits.length, 0)})
                            </span>
                          </div>

                          {citations.length === 0 && hits.length === 0 ? (
                            <Card className="p-4 text-xs text-muted-foreground">
                              {tx("无关联文档引用", "No document citations")}
                            </Card>
                          ) : citations.length > 0 ? (
                            citations.map((cit, idx) => {
                              const highlighted =
                                turn.highlightedCitationIdx === idx;
                              const previewText = cleanCitationSnippet(
                                cit.snippet
                              );
                              return (
                                <Card
                                  key={`${cit.para_id}-${idx}`}
                                  id={`citation-${turn.localId}-${idx}`}
                                  onClick={() =>
                                    scrollToCitation(turn.localId, idx)
                                  }
                                  data-testid={`axquery-citation-${idx}`}
                                  className={`p-3 space-y-1.5 transition-colors cursor-pointer ${
                                    highlighted
                                      ? "border-xblue bg-xblue-soft/30"
                                      : "hover:bg-muted/30"
                                  }`}
                                >
                                  <div className="flex items-center justify-between gap-2">
                                    <span className="text-xs font-medium truncate">
                                      [{tx(`文${idx + 1}`, `Doc ${idx + 1}`)}]{" "}
                                      {cit.doc_title}
                                    </span>
                                    <Badge variant="outline">
                                      {cit.para_id.slice(0, 8)}
                                    </Badge>
                                  </div>
                                  <p
                                    className={`text-xs text-muted-foreground leading-relaxed ${
                                      highlighted ? "" : "line-clamp-3"
                                    }`}
                                  >
                                    {previewText}
                                  </p>
                                </Card>
                              );
                            })
                          ) : (
                            hits.map((hit, idx) => {
                              const highlighted =
                                turn.highlightedCitationIdx === idx;
                              const previewText = cleanCitationSnippet(
                                hit.content
                              );
                              return (
                                <Card
                                  key={hit.para_id}
                                  id={`citation-${turn.localId}-${idx}`}
                                  onClick={() =>
                                    scrollToCitation(turn.localId, idx)
                                  }
                                  data-testid={`axquery-hit-${idx}`}
                                  className={`p-3 space-y-2 transition-colors cursor-pointer ${
                                    highlighted
                                      ? "border-xblue bg-xblue-soft/30"
                                      : "hover:bg-muted/30"
                                  }`}
                                >
                                  <div className="flex items-center justify-between gap-2">
                                    <span className="text-xs font-medium truncate">
                                      [{idx + 1}] {hit.doc_title}
                                    </span>
                                    <span className="text-[11px] font-mono-tech text-xblue shrink-0">
                                      RRF {hit.rrf_score.toFixed(4)}
                                    </span>
                                  </div>
                                  <div className="flex flex-wrap gap-1">
                                    {hit.channels.map((ch) => (
                                      <Badge
                                        key={ch}
                                        variant={
                                          ch === "multi_hop"
                                            ? "xblue"
                                            : "neutral"
                                        }
                                      >
                                        {channelLabel(ch)}
                                      </Badge>
                                    ))}
                                  </div>
                                  <p
                                    className={`text-xs text-muted-foreground leading-relaxed ${
                                      highlighted ? "" : "line-clamp-3"
                                    }`}
                                  >
                                    {previewText}
                                  </p>
                                  {hit.traces.length > 0 && (
                                    <div className="text-[11px] font-mono-tech text-muted-foreground bg-muted/60 rounded px-2 py-1">
                                      {hit.traces.join(" · ")}
                                    </div>
                                  )}
                                </Card>
                              );
                            })
                          )}
                        </div>
                      </div>
                    </div>
                  );
                })}

                {loading && (
                  <Card
                    className="p-4 space-y-3 border-xblue/35 bg-card"
                    data-testid="axquery-loading-skeleton"
                  >
                    <div className="flex flex-wrap items-center justify-between gap-2 border-b border-border pb-2.5">
                      <div className="flex items-center gap-2 text-xs font-medium text-foreground">
                        <Loader2 className="h-4 w-4 animate-spin text-xblue shrink-0" />
                        <span>
                          {tx(
                            "正在执行五路超图召回与研判…",
                            "Running 5-way hypergraph recall & synthesis…"
                          )}
                        </span>
                      </div>
                      {pendingQuestion && (
                        <span className="text-xs text-muted-foreground truncate max-w-md">
                          {pendingQuestion}
                        </span>
                      )}
                    </div>
                    <div className="space-y-2">
                      <div className="h-3 w-4/5 rounded bg-muted animate-pulse" />
                      <div className="h-3 w-11/12 rounded bg-muted animate-pulse" />
                      <div className="h-3 w-3/5 rounded bg-muted animate-pulse" />
                    </div>
                  </Card>
                )}

                {error && (
                  <Card className="p-3 border-red-500/30 bg-red-500/5 text-xs">
                    <span className="font-semibold text-red-600 dark:text-red-400">
                      {tx("请求失败：", "Request failed: ")}
                    </span>
                    <span>{error}</span>
                  </Card>
                )}

                <div ref={streamEndRef} />
              </div>

              {/* Sticky Bottom-Docked Composer for Active Multi-Turn Session */}
              <div className="shrink-0 pt-1 max-w-4xl w-full mx-auto">
                {renderComposerBox(2)}
              </div>
            </>
          )}
        </div>
      </div>
    </div>
  );
}
