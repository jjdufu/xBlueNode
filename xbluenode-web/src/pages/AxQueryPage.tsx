import { useState } from "react";
import Markdown from "react-markdown";
import { useNavigate } from "react-router";
import {
  ChevronDown,
  ChevronUp,
  Code2,
  FileText,
  Loader2,
  Network,
  RotateCcw,
  Send,
  Sparkles,
  ThumbsDown,
  ThumbsUp,
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
  type RecallChannel,
} from "@/lib/api";
import { ChartPanel } from "@/components/analytics/ChartPanel";
import { ResultGrid } from "@/components/analytics/ResultGrid";
import {
  Badge,
  Button,
  Card,
  Input,
  PageHeader,
  SegmentedControl,
  Textarea,
} from "@/components/ui/primitives";

interface ConversationTurn {
  localId: string;
  question: string;
  requestedMode: ChatQueryMode;
  resp: ChatQueryResponse;
  sqlExpanded: boolean;
  highlightedCitationIdx: number | null;
  feedback: AskFeedbackKind | null;
  correctSqlOpen: boolean;
  correctedSqlText: string;
  feedbackSubmitting: boolean;
  feedbackNotice: string | null;
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
      // Drop pure markdown table separator rows such as | --- | :---: |
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
  const { tx, fmtNum } = useI18n();

  const [query, setQuery] = useState("");
  const [mode, setMode] = useState<ChatQueryMode>("AUTO");
  const [sessionId, setSessionId] = useState<string | undefined>(undefined);
  const [turns, setTurns] = useState<ConversationTurn[]>([]);
  const [loading, setLoading] = useState(false);
  const [pendingQuestion, setPendingQuestion] = useState<string>("");
  const [error, setError] = useState<string | null>(null);

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

  const starterQuestions = [
    tx(
      "已支付订单总金额是多少？华东仓履约制度是什么？",
      "What is the total paid order amount and East Warehouse fulfillment SLA?"
    ),
    tx(
      "按履约状态统计订单笔数与金额分布",
      "Break down order counts and amounts by fulfillment status"
    ),
    tx(
      "华东仓安全库存预警与调拨规范有哪些要求？",
      "What are the safety stock thresholds and transfer rules for East Warehouse?"
    ),
    tx(
      "各仓库在库库存与下游需求匹配情况如何？",
      "Compare warehouse inventory levels against downstream order demand"
    ),
  ];

  const submitQuestion = async (rawQuestion: string) => {
    const trimmed = rawQuestion.trim();
    if (!trimmed || loading) return;

    setLoading(true);
    setPendingQuestion(trimmed);
    setError(null);
    try {
      const data = await api.chatQuery({
        query: trimmed,
        top_k: 6,
        mode,
        ...(sessionId ? { session_id: sessionId } : {}),
      });

      if (data.session_id) {
        setSessionId(data.session_id);
      }

      const nextTurn: ConversationTurn = {
        localId: data.turn_id || `turn-${Date.now()}`,
        question: trimmed,
        requestedMode: mode,
        resp: data,
        sqlExpanded: false,
        highlightedCitationIdx: null,
        feedback: null,
        correctSqlOpen: false,
        correctedSqlText: data.sql ?? "",
        feedbackSubmitting: false,
        feedbackNotice: null,
      };

      setTurns((prev) => [...prev, nextTurn]);
      setQuery("");
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
    setSessionId(undefined);
    setTurns([]);
    setError(null);
  };

  const updateTurn = (
    localId: string,
    updater: (t: ConversationTurn) => ConversationTurn
  ) => {
    setTurns((prev) =>
      prev.map((t) => (t.localId === localId ? updater(t) : t))
    );
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

  return (
    <div className="space-y-5" data-testid="axquery-page">
      <PageHeader
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
          <>
            <div data-testid="axquery-mode-switcher">
              <SegmentedControl
                value={mode}
                onChange={(next) => setMode(next)}
                items={[
                  {
                    key: "AUTO",
                    label: tx("智能路由", "Auto"),
                    testId: "axquery-mode-AUTO",
                  },
                  {
                    key: "DOC",
                    label: tx("知识检索", "Docs"),
                    testId: "axquery-mode-DOC",
                  },
                  {
                    key: "DATA",
                    label: tx("智能问数", "Data"),
                    testId: "axquery-mode-DATA",
                  },
                ]}
              />
            </div>

            {turns.length > 0 && (
              <Button
                type="button"
                variant="outline"
                onClick={handleResetSession}
                data-testid="axquery-new-session-btn"
              >
                <RotateCcw className="h-3.5 w-3.5" />
                {tx("新会话", "New Session")}
              </Button>
            )}
          </>
        }
      />

      {/* Question Input Bar */}
      <div className="flex items-center gap-2">
        <Input
          className="flex-1 min-w-0"
          placeholder={tx(
            "已支付订单总金额是多少？对应履约制度是什么？",
            "What is the total paid order amount and fulfillment policy?"
          )}
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") void handleAsk();
          }}
          data-testid="axquery-input"
        />
        <Button
          type="button"
          className="whitespace-nowrap shrink-0"
          onClick={() => void handleAsk()}
          disabled={loading || !query.trim()}
          data-testid="axquery-submit-btn"
        >
          <Send className="h-3.5 w-3.5" />
          {loading ? tx("研判中…", "Running…") : tx("提问", "Ask")}
        </Button>
      </div>

      {/* Starter Question Pills (Empty State) */}
      {turns.length === 0 && !loading && (
        <Card
          className="p-4 space-y-3 bg-muted/15"
          data-testid="axquery-starter-prompts"
        >
          <div className="flex items-center gap-1.5 text-xs font-medium text-muted-foreground">
            <Sparkles className="h-3.5 w-3.5 text-xblue" />
            <span>{tx("示例问题", "Starter Questions")}</span>
          </div>
          <div className="flex flex-wrap gap-2">
            {starterQuestions.map((item, idx) => (
              <button
                key={idx}
                type="button"
                onClick={() => {
                  setQuery(item);
                  void submitQuestion(item);
                }}
                data-testid={`axquery-starter-${idx}`}
                className="inline-flex items-center gap-1.5 rounded-md border border-border bg-background px-3 py-1.5 text-left text-xs text-foreground hover:border-xblue hover:bg-xblue-soft/25 transition-colors cursor-pointer"
              >
                <span>{item}</span>
              </button>
            ))}
          </div>
        </Card>
      )}

      {/* Multi-Stage Loading Skeleton Card */}
      {loading && (
        <Card
          className="p-4 space-y-4 border-xblue/35 bg-card"
          data-testid="axquery-loading-skeleton"
        >
          <div className="flex flex-wrap items-center justify-between gap-2 border-b border-border pb-3">
            <div className="flex items-center gap-2 text-xs font-medium text-foreground">
              <Loader2 className="h-4 w-4 animate-spin text-xblue shrink-0" />
              <span>
                {tx(
                  "正在执行五路超图召回与大模型研判…",
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

          <div className="grid grid-cols-1 sm:grid-cols-3 gap-2 text-xs">
            <div className="rounded border border-xblue/30 bg-xblue-soft/25 px-3 py-2 flex items-center gap-2">
              <span className="h-2 w-2 rounded-full bg-xblue animate-pulse shrink-0" />
              <span className="font-medium">
                {tx("1. 意图识别与模式路由", "1. Intent & Mode Routing")}
              </span>
            </div>
            <div className="rounded border border-border bg-muted/30 px-3 py-2 flex items-center gap-2">
              <span className="h-2 w-2 rounded-full bg-xblue/70 animate-pulse shrink-0" />
              <span className="text-muted-foreground">
                {tx("2. 五路召回 / 受控 SQL 执行", "2. 5-Way Recall & Guarded SQL")}
              </span>
            </div>
            <div className="rounded border border-border bg-muted/30 px-3 py-2 flex items-center gap-2">
              <span className="h-2 w-2 rounded-full bg-muted-foreground/50 animate-pulse shrink-0" />
              <span className="text-muted-foreground">
                {tx("3. 证据融合与引文合成", "3. Grounded Answer Synthesis")}
              </span>
            </div>
          </div>

          <div className="space-y-2 pt-1">
            <div className="h-3 w-4/5 rounded bg-muted animate-pulse" />
            <div className="h-3 w-11/12 rounded bg-muted animate-pulse" />
            <div className="h-3 w-3/5 rounded bg-muted animate-pulse" />
          </div>
        </Card>
      )}

      {error && (
        <Card className="p-3.5 border-red-500/30 bg-red-500/5 text-xs">
          <span className="font-semibold text-red-600 dark:text-red-400">
            {tx("请求失败：", "Request failed: ")}
          </span>
          <span>{error}</span>
        </Card>
      )}

      {/* Multi-Turn Conversation Stream */}
      <div className="space-y-5" data-testid="axquery-turns-list">
        {turns.map((turn, turnIndex) => {
          const { resp } = turn;
          const intent: AskIntent = resp.intent ?? "DOC";
          const meta = intentMeta(intent);
          const sqlText = resp.sql ?? "";
          const dataColumns = Array.isArray(resp.columns) ? resp.columns : [];
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

                <div className="flex items-center gap-2">
                  <Badge
                    variant={meta.variant}
                    data-testid="axquery-intent-badge"
                  >
                    {meta.label}
                  </Badge>
                  <span className="text-[11px] font-mono-tech text-muted-foreground">
                    {fmtNum(resp.elapsed_ms, 0)} ms
                  </span>
                </div>
              </div>

              <div className="grid grid-cols-1 lg:grid-cols-12 gap-5">
                {/* Left Column: Markdown Answer + Expandable SQL/Data + Related Nodes + Feedback */}
                <div className="lg:col-span-7 space-y-4">
                  <Card className="p-4 space-y-3">
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
                            <li className="leading-relaxed">{children}</li>
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
                            const citeMatch = /^#cite-(\d+)$/.exec(href ?? "");
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
                                  [{tx(`文${citNum}`, `Doc ${citNum}`)}]
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

                    {/* Related Business Nodes */}
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
                              <Badge variant="outline">{node.type_name}</Badge>
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
                            turn.feedback === "UP" ? "primary" : "outline"
                          }
                          disabled={turn.feedbackSubmitting}
                          onClick={() => void handleFeedback(turn, "UP")}
                          data-testid="axquery-feedback-up"
                        >
                          <ThumbsUp className="h-3 w-3" />
                          {tx("有帮助", "Helpful")}
                        </Button>

                        <Button
                          type="button"
                          size="sm"
                          variant={
                            turn.feedback === "DOWN" ? "primary" : "outline"
                          }
                          disabled={turn.feedbackSubmitting}
                          onClick={() => void handleFeedback(turn, "DOWN")}
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

                    {turn.feedbackNotice && (
                      <div
                        className="text-[11px] font-mono-tech text-xblue"
                        data-testid="axquery-feedback-notice"
                      >
                        {turn.feedbackNotice}
                      </div>
                    )}

                    {/* Corrected SQL Submission Box */}
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

                  {/* Expandable SQL + ResultGrid + ChartPanel */}
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
                <div className="lg:col-span-5 space-y-3">
                  <div className="flex items-center justify-between text-xs font-medium text-muted-foreground">
                    <span className="inline-flex items-center gap-1">
                      <FileText className="h-3.5 w-3.5 text-xblue" />
                      {tx("引用与召回证据", "Citations & Recall Evidence")} (
                      {fmtNum(citations.length || hits.length, 0)})
                    </span>
                  </div>

                  {citations.length === 0 && hits.length === 0 ? (
                    <Card className="p-4 text-xs text-muted-foreground">
                      {tx("无关联文档引用", "No document citations")}
                    </Card>
                  ) : citations.length > 0 ? (
                    citations.map((cit, idx) => {
                      const highlighted = turn.highlightedCitationIdx === idx;
                      const previewText = cleanCitationSnippet(cit.snippet);
                      return (
                        <Card
                          key={`${cit.para_id}-${idx}`}
                          id={`citation-${turn.localId}-${idx}`}
                          onClick={() => scrollToCitation(turn.localId, idx)}
                          data-testid={`axquery-citation-${idx}`}
                          className={`p-3.5 space-y-1.5 transition-colors cursor-pointer ${
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
                      const highlighted = turn.highlightedCitationIdx === idx;
                      const previewText = cleanCitationSnippet(hit.content);
                      return (
                        <Card
                          key={hit.para_id}
                          id={`citation-${turn.localId}-${idx}`}
                          onClick={() => scrollToCitation(turn.localId, idx)}
                          data-testid={`axquery-hit-${idx}`}
                          className={`p-3.5 space-y-2 transition-colors cursor-pointer ${
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
                                  ch === "multi_hop" ? "xblue" : "neutral"
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
      </div>
    </div>
  );
}
