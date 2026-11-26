import { useState } from "react";
import {
  ArrowRight,
  HelpCircle,
  MessageSquare,
  Send,
  Terminal,
} from "lucide-react";
import {
  axsimApi,
  type ScenarioRecord,
  type SimAskCandidate,
  type SimAskResponse,
} from "@/lib/api/axsim";
import { Badge, Button, Card, Select, Textarea } from "@/components/ui/primitives";
import { useI18n } from "@/i18n";

export interface SimAskProps {
  sandboxId: string;
  scenarios: ScenarioRecord[];
  activeScenarioId: string | null;
  onSelectScenario?: (scenarioId: string) => void;
  onScenarioCreated?: (scenarioId: string) => void;
}

export function SimAsk({
  sandboxId,
  scenarios,
  activeScenarioId,
  onSelectScenario,
  onScenarioCreated,
}: SimAskProps) {
  const { tx, fmtNum } = useI18n();
  const [question, setQuestion] = useState("");
  const [contextScenarioId, setContextScenarioId] = useState<string>(
    activeScenarioId ?? ""
  );
  const [loading, setLoading] = useState(false);
  const [response, setResponse] = useState<SimAskResponse | null>(null);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  const samplePrompts: readonly string[] = [
    tx(
      "如果关键上游节点负荷下降 30%，会对哪些下游节点造成传导影响？",
      "If key upstream capacity drops by 30%, which downstream nodes are affected?"
    ),
    tx(
      "当前推演中受影响幅度最大的前 5 个核心节点有哪些？",
      "Which top 5 nodes have the largest impact in the current scenario?"
    ),
  ];

  const effectiveScenarioId = contextScenarioId || activeScenarioId || undefined;

  const submitQuestion = async (overrideQuestion?: string) => {
    const q = (overrideQuestion ?? question).trim();
    if (!q || !sandboxId) return;
    setLoading(true);
    setErrorMsg(null);
    try {
      const res = await axsimApi.ask({
        question: q,
        sandbox_id: sandboxId,
        scenario_id: effectiveScenarioId ?? null,
      });
      setResponse(res);
      if (res.scenario_id && res.intent === "WHAT_IF") {
        onScenarioCreated?.(res.scenario_id);
      }
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  };

  const applyCandidate = (cand: SimAskCandidate) => {
    const refined = `${question.trim()} (${cand.type_name} ${cand.title} [${cand.node_key}])`;
    setQuestion(refined);
    void submitQuestion(refined);
  };

  return (
    <Card className="p-4 space-y-4" data-testid="sim-ask-card">
      <div className="flex flex-wrap items-center justify-between gap-2 border-b border-border pb-2.5">
        <div className="flex items-center gap-2 text-sm font-medium">
          <MessageSquare className="h-4 w-4 text-xblue" />
          <span>{tx("推演自然语言追问", "Simulation Copilot")}</span>
        </div>

        <div className="flex items-center gap-2 text-xs">
          <span className="text-muted-foreground">
            {tx("上下文场景:", "Context:")}
          </span>
          <Select
            value={contextScenarioId || activeScenarioId || ""}
            onChange={(e) => setContextScenarioId(e.target.value)}
            className="h-7 w-48"
            data-testid="sim-ask-scenario-select"
          >
            <option value="">{tx("自动 / 新建推演", "Auto / New What-If")}</option>
            {scenarios.map((s) => (
              <option key={s.id} value={s.id}>
                {s.name}
              </option>
            ))}
          </Select>
        </div>
      </div>

      {/* Quick prompts */}
      <div className="flex flex-wrap items-center gap-1.5">
        <span className="text-[11px] text-muted-foreground">
          {tx("示例:", "Examples:")}
        </span>
        {samplePrompts.map((prompt) => (
          <button
            key={prompt}
            type="button"
            onClick={() => setQuestion(prompt)}
            className="rounded border border-border bg-muted/30 px-2 py-1 text-[11px] text-muted-foreground hover:text-foreground cursor-pointer"
          >
            {prompt}
          </button>
        ))}
      </div>

      <form
        onSubmit={(e) => {
          e.preventDefault();
          void submitQuestion();
        }}
        className="space-y-2.5"
      >
        <Textarea
          rows={2}
          value={question}
          onChange={(e) => setQuestion(e.target.value)}
          placeholder={tx(
            "针对当前沙盘的节点与指标输入自然语言推演或追问…",
            "Ask a what-if or impact question about the current sandbox…"
          )}
          className="text-xs"
          data-testid="sim-ask-input"
        />
        <div className="flex justify-end">
          <Button
            type="submit"
            disabled={loading || !question.trim()}
            data-testid="sim-ask-submit"
          >
            <Send className="h-3.5 w-3.5" />
            {loading
              ? tx("分析中…", "Analyzing…")
              : tx("发起追问", "Ask")}
          </Button>
        </div>
      </form>

      {errorMsg && (
        <div className="rounded-md border border-red-500/40 bg-red-500/10 px-3 py-2 text-xs text-red-700 dark:text-red-300">
          {errorMsg}
        </div>
      )}

      {response && (
        <div
          className="rounded-md border border-border bg-muted/20 p-3.5 space-y-3 text-xs"
          data-testid="sim-ask-response"
        >
          <div className="flex flex-wrap items-center justify-between gap-2">
            <div className="flex items-center gap-2">
              <Badge
                variant={response.intent === "WHAT_IF" ? "xblue" : "neutral"}
                data-testid="sim-ask-intent-badge"
              >
                {response.intent}
              </Badge>
              {response.need_clarification && (
                <Badge variant="warning">
                  {tx("需指明对象", "Clarification Needed")}
                </Badge>
              )}
            </div>

            {response.scenario_id && (
              <Button
                type="button"
                variant="outline"
                size="sm"
                onClick={() => onSelectScenario?.(response.scenario_id ?? "")}
              >
                {tx("查看场景", "Open Scenario")}
                <ArrowRight className="h-3 w-3" />
              </Button>
            )}
          </div>

          {/* Clarification candidates */}
          {response.need_clarification &&
            Array.isArray(response.candidates) &&
            response.candidates.length > 0 && (
              <div className="rounded border border-amber-500/40 bg-amber-500/10 p-2.5 space-y-2">
                <div className="flex items-center gap-1.5 font-medium text-amber-700 dark:text-amber-300">
                  <HelpCircle className="h-3.5 w-3.5" />
                  <span>
                    {tx(
                      "匹配到多个候选节点：",
                      "Multiple candidate nodes matched:"
                    )}
                  </span>
                </div>
                <div className="flex flex-wrap gap-1.5">
                  {response.candidates.map((cand) => (
                    <button
                      key={cand.node_id}
                      type="button"
                      onClick={() => applyCandidate(cand)}
                      className="rounded border border-border bg-background px-2.5 py-1 text-[11px] hover:border-xblue hover:text-xblue cursor-pointer"
                    >
                      [{cand.type_name}] {cand.title} ({cand.node_key})
                    </button>
                  ))}
                </div>
              </div>
            )}

          {/* Grounded answer */}
          <div className="leading-relaxed whitespace-pre-wrap text-foreground">
            {response.answer}
          </div>

          {/* SQL block */}
          {response.sql && (
            <div className="space-y-1">
              <div className="flex items-center gap-1 text-[11px] text-muted-foreground font-mono-tech">
                <Terminal className="h-3 w-3" />
                <span>{tx("影响查询 SQL", "Impact Query SQL")}</span>
              </div>
              <pre className="overflow-x-auto rounded border border-border bg-background p-2.5 font-mono-tech text-[11px]">
                {response.sql}
              </pre>
            </div>
          )}

          {/* DuckDB Query Rows */}
          {Array.isArray(response.rows) && response.rows.length > 0 && (
            <div className="overflow-x-auto">
              <table
                className="w-full text-xs"
                data-testid="sim-ask-rows-table"
              >
                <thead>
                  <tr className="border-b border-border text-left text-muted-foreground">
                    {(response.columns?.length
                      ? response.columns.map((c) => c.name)
                      : Object.keys(response.rows[0] ?? {})
                    ).map((colName) => (
                      <th key={colName} className="py-1.5 pr-3 font-mono-tech">
                        {colName}
                      </th>
                    ))}
                  </tr>
                </thead>
                <tbody>
                  {response.rows.map((r, rIdx) => {
                    const colNames = response.columns?.length
                      ? response.columns.map((c) => c.name)
                      : Object.keys(r);
                    return (
                      <tr key={rIdx} className="border-b border-border/60">
                        {colNames.map((col) => {
                          const cell = r[col];
                          return (
                            <td
                              key={col}
                              className="py-1.5 pr-3 font-mono-tech"
                            >
                              {typeof cell === "number"
                                ? fmtNum(cell, 4)
                                : String(cell ?? "-")}
                            </td>
                          );
                        })}
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            </div>
          )}

          {/* What-if quick impacts summary */}
          {Array.isArray(response.impacts) && response.impacts.length > 0 && (
            <div className="space-y-1.5">
              <div className="text-[11px] font-medium text-muted-foreground">
                {tx(
                  `关键影响预览 (${response.impacts.length})`,
                  `Top Impacts (${response.impacts.length})`
                )}
              </div>
              <div className="grid grid-cols-1 sm:grid-cols-2 gap-1.5">
                {response.impacts.slice(0, 6).map((imp) => (
                  <div
                    key={`${imp.node_id}-${imp.metric}`}
                    className="rounded border border-border bg-background px-2.5 py-1.5 flex items-center justify-between"
                  >
                    <span>
                      [{imp.type_name}] {imp.title} ·{" "}
                      <span className="font-mono-tech">{imp.metric}</span>
                    </span>
                    <span
                      className={`font-mono-tech font-medium ${
                        (imp.delta ?? 0) < 0
                          ? "text-red-600 dark:text-red-400"
                          : "text-emerald-600 dark:text-emerald-400"
                      }`}
                    >
                      {imp.delta_ratio != null
                        ? `${imp.delta_ratio > 0 ? "+" : ""}${(imp.delta_ratio * 100).toFixed(1)}%`
                        : fmtNum(imp.after_val, 4)}
                    </span>
                  </div>
                ))}
              </div>
            </div>
          )}
        </div>
      )}
    </Card>
  );
}
