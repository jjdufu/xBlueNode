import React, { useState } from "react";
import { useNavigate } from "react-router";
import {
  ChevronDown,
  ChevronUp,
  Code2,
  FileText,
  Network,
  RotateCcw,
  Send,
  ThumbsDown,
  ThumbsUp,
  Wrench,
} from "lucide-react";
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
import { Badge, Button, Card, Input, Textarea } from "@/components/ui/primitives";

const CHANNEL_LABEL: Record<RecallChannel, string> = {
  vector: "Vector 语义",
  bm25: "BM25 关键词",
  entity_link: "实体关联",
  context: "上下文扩展",
  multi_hop: "多跳推理",
};

const INTENT_META: Record<
  AskIntent,
  { label: string; variant: "neutral" | "xblue" | "outline" }
> = {
  DOC: { label: "DOC · 文档溯源", variant: "neutral" },
  DATA: { label: "DATA · 智能问数", variant: "xblue" },
  HYBRID: { label: "HYBRID · 混合研判", variant: "xblue" },
  CHITCHAT: { label: "CHITCHAT · 对话", variant: "outline" },
};

const MODE_OPTIONS: ReadonlyArray<{ mode: ChatQueryMode; label: string }> = [
  { mode: "AUTO", label: "AUTO 智能路由" },
  { mode: "DOC", label: "DOC 知识检索" },
  { mode: "DATA", label: "DATA 结构化问数" },
];

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

function renderAnswerWithCitationLinks(
  answer: string,
  onClickCitationNumber: (oneBasedIndex: number) => void
): React.ReactNode {
  const parts = answer.split(/(\[文\d+\]|\[表\])/g);
  return parts.map((part, idx) => {
    const docMatch = /^\[文(\d+)\]$/.exec(part);
    if (docMatch) {
      const citNum = Number(docMatch[1]);
      return (
        <button
          key={idx}
          type="button"
          onClick={() => onClickCitationNumber(citNum)}
          className="mx-0.5 inline-flex items-center rounded bg-xblue-soft px-1.5 py-0.5 font-mono-tech text-[11px] font-medium text-xblue hover:underline cursor-pointer"
        >
          {part}
        </button>
      );
    }
    if (part === "[表]") {
      return (
        <span
          key={idx}
          className="mx-0.5 inline-flex items-center rounded border border-xblue/30 bg-xblue-soft/60 px-1.5 py-0.5 font-mono-tech text-[11px] font-medium text-xblue"
        >
          [表]
        </span>
      );
    }
    return <React.Fragment key={idx}>{part}</React.Fragment>;
  });
}

export function AxQueryPage() {
  const navigate = useNavigate();

  const [query, setQuery] = useState("");
  const [mode, setMode] = useState<ChatQueryMode>("AUTO");
  const [sessionId, setSessionId] = useState<string | undefined>(undefined);
  const [turns, setTurns] = useState<ConversationTurn[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const handleAsk = async () => {
    const trimmed = query.trim();
    if (!trimmed || loading) return;

    setLoading(true);
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
    }
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
          ? "修正 SQL 已提交并通过守卫保存"
          : kind === "UP"
          ? "已记录点赞反馈"
          : "已记录反馈，可进一步提交修正 SQL",
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

  return (
    <div className="space-y-6" data-testid="axquery-page">
      {/* Header & Routing Mode Switcher */}
      <div className="flex flex-wrap items-center justify-between gap-3 border-b border-border pb-4">
        <div>
          <h1 className="text-lg font-semibold tracking-tight">
            AxQuery · 五路融合问答
          </h1>
          <p className="text-xs text-muted-foreground mt-0.5">
            多模态意图路由（DOC 五路召回 / DATA 智能问数 / HYBRID 图数混合）与多轮会话反馈学习
          </p>
        </div>

        <div className="flex flex-wrap items-center gap-2">
          {sessionId && (
            <Badge
              variant="outline"
              data-testid="axquery-session-badge"
              title={sessionId}
            >
              会话 {sessionId.slice(0, 8)}
            </Badge>
          )}

          <div
            className="flex items-center rounded-md border border-border bg-muted/40 p-0.5"
            data-testid="axquery-mode-switcher"
          >
            {MODE_OPTIONS.map((opt) => (
              <button
                key={opt.mode}
                type="button"
                onClick={() => setMode(opt.mode)}
                data-testid={`axquery-mode-${opt.mode}`}
                className={`rounded px-2.5 py-1.5 text-xs font-medium transition-colors cursor-pointer ${
                  mode === opt.mode
                    ? "bg-background text-foreground border border-border"
                    : "text-muted-foreground hover:text-foreground"
                }`}
              >
                {opt.label}
              </button>
            ))}
          </div>

          {turns.length > 0 && (
            <Button
              type="button"
              variant="outline"
              onClick={handleResetSession}
              data-testid="axquery-new-session-btn"
            >
              <RotateCcw className="h-3.5 w-3.5" />
              新会话
            </Button>
          )}
        </div>
      </div>

      {/* Question Input Bar */}
      <div className="flex gap-2">
        <Input
          placeholder="输入业务知识、指标统计或混合研判问题，例如：已支付订单总金额是多少？对应履约制度是什么？"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") void handleAsk();
          }}
          data-testid="axquery-input"
        />
        <Button
          type="button"
          onClick={() => void handleAsk()}
          disabled={loading || !query.trim()}
          data-testid="axquery-submit-btn"
        >
          <Send className="h-3.5 w-3.5" />
          {loading ? "研判中..." : "提问"}
        </Button>
      </div>

      {error && (
        <Card className="p-4 border-foreground/40 bg-muted/40 text-xs">
          <span className="font-mono-tech font-semibold">请求失败：</span>
          {error}
        </Card>
      )}

      {/* Multi-Turn Conversation Stream */}
      <div className="space-y-6" data-testid="axquery-turns-list">
        {turns.map((turn, turnIndex) => {
          const { resp } = turn;
          const intent: AskIntent = resp.intent ?? "DOC";
          const intentMeta = INTENT_META[intent];
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
                    variant={intentMeta.variant}
                    data-testid="axquery-intent-badge"
                  >
                    {intentMeta.label}
                  </Badge>
                  <span className="text-[11px] font-mono-tech text-muted-foreground">
                    耗时 {resp.elapsed_ms} ms
                  </span>
                </div>
              </div>

              <div className="grid grid-cols-1 lg:grid-cols-12 gap-5">
                {/* Left Column: Answer + Expandable SQL/Data + Related Nodes + Feedback */}
                <div className="lg:col-span-7 space-y-4">
                  <Card className="p-4 space-y-3">
                    <div className="text-sm leading-relaxed whitespace-pre-wrap">
                      {renderAnswerWithCitationLinks(resp.answer, (citNum) => {
                        scrollToCitation(
                          turn.localId,
                          Math.max(0, citNum - 1)
                        );
                      })}
                    </div>

                    {seedEntities.length > 0 && (
                      <div className="pt-2.5 border-t border-border flex flex-wrap items-center gap-1.5">
                        <span className="text-[11px] text-muted-foreground mr-1">
                          提取种子实体：
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
                          <span>关联业务节点 ({relatedNodes.length})</span>
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
                          variant={
                            turn.feedback === "UP" ? "primary" : "outline"
                          }
                          className="px-2.5 py-1 text-xs"
                          disabled={turn.feedbackSubmitting}
                          onClick={() => void handleFeedback(turn, "UP")}
                          data-testid="axquery-feedback-up"
                        >
                          <ThumbsUp className="h-3 w-3" />
                          点赞
                        </Button>

                        <Button
                          type="button"
                          variant={
                            turn.feedback === "DOWN" ? "primary" : "outline"
                          }
                          className="px-2.5 py-1 text-xs"
                          disabled={turn.feedbackSubmitting}
                          onClick={() => void handleFeedback(turn, "DOWN")}
                          data-testid="axquery-feedback-down"
                        >
                          <ThumbsDown className="h-3 w-3" />
                          点踩
                        </Button>

                        {hasDataPayload && (
                          <Button
                            type="button"
                            variant="outline"
                            className="px-2.5 py-1 text-xs"
                            onClick={() =>
                              updateTurn(turn.localId, (t) => ({
                                ...t,
                                correctSqlOpen: !t.correctSqlOpen,
                              }))
                            }
                            data-testid="axquery-correct-sql-toggle"
                          >
                            <Wrench className="h-3 w-3 text-xblue" />
                            修正 SQL
                          </Button>
                        )}
                      </div>

                      {hasDataPayload && (
                        <Button
                          type="button"
                          variant="outline"
                          className="px-2.5 py-1 text-xs"
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
                            ? "收起 SQL 与数据结果"
                            : "展开 SQL 与数据结果"}
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
                          输入人工修正后的 DuckDB SQL（经守卫校验后加入租户少样本库）：
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
                          placeholder="SELECT ..."
                          className="font-mono-tech text-xs"
                          data-testid="axquery-corrected-sql-input"
                        />
                        <div className="flex justify-end gap-2">
                          <Button
                            type="button"
                            variant="ghost"
                            className="py-1"
                            onClick={() =>
                              updateTurn(turn.localId, (t) => ({
                                ...t,
                                correctSqlOpen: false,
                              }))
                            }
                          >
                            取消
                          </Button>
                          <Button
                            type="button"
                            variant="primary"
                            className="py-1"
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
                            提交修正 SQL
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
                            生成并经 SQL Guard 校验的查询语句
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
                  <div className="flex items-center justify-between text-xs font-medium text-muted-foreground uppercase tracking-wider">
                    <span className="inline-flex items-center gap-1">
                      <FileText className="h-3.5 w-3.5 text-xblue" />
                      文档引用与召回溯源 ({citations.length || hits.length})
                    </span>
                  </div>

                  {citations.length === 0 && hits.length === 0 ? (
                    <Card className="p-4 text-xs text-muted-foreground">
                      本轮为纯结构化数值问数或无关联文档段落引用
                    </Card>
                  ) : citations.length > 0 ? (
                    citations.map((cit, idx) => {
                      const highlighted = turn.highlightedCitationIdx === idx;
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
                              [文{idx + 1}] {cit.doc_title}
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
                            {cit.snippet}
                          </p>
                        </Card>
                      );
                    })
                  ) : (
                    hits.map((hit, idx) => {
                      const highlighted = turn.highlightedCitationIdx === idx;
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
                                {CHANNEL_LABEL[ch]}
                              </Badge>
                            ))}
                          </div>
                          <p
                            className={`text-xs text-muted-foreground leading-relaxed ${
                              highlighted ? "" : "line-clamp-3"
                            }`}
                          >
                            {hit.content}
                          </p>
                          {hit.traces.length > 0 && (
                            <div className="text-[11px] font-mono-tech text-muted-foreground bg-muted/60 rounded px-2 py-1">
                              {hit.traces.join(" | ")}
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
