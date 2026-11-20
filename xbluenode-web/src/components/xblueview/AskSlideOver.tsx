import { useMemo, useState } from "react";
import { MessageSquare, Plus, Send, X } from "lucide-react";
import {
  analyticsApi,
  type AskResponse,
  type FilterSpec,
} from "@/lib/api/analytics";
import {
  xblueviewApi,
  type DashboardDetail,
  type DrillPathItem,
  type WidgetRecord,
} from "@/lib/api/xblueview";
import { Badge, Button, Input } from "@/components/ui/primitives";
import { defaultPositionForKind } from "./utils";
import { WidgetRenderer } from "./widgets/WidgetRenderer";

export interface AskSlideOverProps {
  open: boolean;
  dashboard: DashboardDetail;
  filterValues: Record<string, unknown>;
  drillMap: Record<string, DrillPathItem[]>;
  isDark?: boolean;
  onClose: () => void;
  onAddedWidget: (widget: WidgetRecord) => void;
}

function buildFilterSpecs(values: Record<string, unknown>): FilterSpec[] {
  const specs: FilterSpec[] = [];
  for (const [field, value] of Object.entries(values)) {
    if (value === undefined || value === null || value === "") continue;
    if (Array.isArray(value)) {
      if (value.length === 0) continue;
      specs.push({ field, op: "In", value });
    } else {
      specs.push({ field, op: "Eq", value });
    }
  }
  return specs;
}

export function AskSlideOver({
  open,
  dashboard,
  filterValues,
  drillMap,
  isDark = false,
  onClose,
  onAddedWidget,
}: AskSlideOverProps) {
  const [question, setQuestion] = useState("");
  const [loading, setLoading] = useState(false);
  const [adding, setAdding] = useState(false);
  const [response, setResponse] = useState<AskResponse | null>(null);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  const datasetIds = useMemo(() => {
    const set = new Set<string>();
    for (const w of dashboard.widgets) {
      for (const dsId of w.deps?.dataset_ids ?? []) {
        if (dsId) set.add(dsId);
      }
    }
    return Array.from(set);
  }, [dashboard.widgets]);

  const drillPathStrings = useMemo(() => {
    const list: string[] = [];
    for (const items of Object.values(drillMap)) {
      for (const item of items) {
        list.push(`${item.field}=${String(item.value)}`);
      }
    }
    return list;
  }, [drillMap]);

  if (!open) return null;

  const handleAsk = async (e: React.FormEvent) => {
    e.preventDefault();
    const q = question.trim();
    if (!q) return;
    setLoading(true);
    setErrorMsg(null);
    try {
      const res = await analyticsApi.askData({
        question: q,
        context: {
          dashboard_id: dashboard.id,
          dataset_ids: datasetIds,
          filters: buildFilterSpecs(filterValues),
          drill_path: drillPathStrings,
        },
      });
      setResponse(res);
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  };

  const handleAddAsWidget = async () => {
    if (!response) return;
    setAdding(true);
    setErrorMsg(null);
    try {
      const kind = response.chart?.kind ?? "TABLE";
      const encoding: Record<string, unknown> = {};
      if (response.chart?.x_field) {
        encoding.x = response.chart.x_field;
      }
      if (response.chart?.y_fields && response.chart.y_fields.length > 0) {
        encoding.y = response.chart.y_fields;
      }
      const created = await xblueviewApi.createWidget(dashboard.id, {
        widget_kind: kind,
        title: response.chart?.title || question.trim().slice(0, 28) || "问数生成组件",
        position: defaultPositionForKind(kind, dashboard.widgets),
        data_source: response.sql
          ? { kind: "SQL", sql: response.sql }
          : {
              kind: "STATIC",
              columns: response.columns.map((c) => ({
                name: c.name,
                data_type: c.data_type,
              })),
              rows: response.rows.map((r) =>
                response.columns.map((c) => r[c.name])
              ),
            },
        encoding,
      });
      onAddedWidget(created);
      onClose();
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setAdding(false);
    }
  };

  const chartWidgetPreview: WidgetRecord | null =
    response && response.chart
      ? {
          id: "ask-preview",
          tenant_id: dashboard.tenant_id,
          dashboard_id: dashboard.id,
          widget_kind: response.chart.kind,
          title: response.chart.title ?? "推荐图表",
          position: { x: 0, y: 0, w: 8, h: 6 },
          data_source: { kind: "STATIC", rows: [] },
          encoding: {
            x: response.chart.x_field ?? undefined,
            y: response.chart.y_fields,
          },
          options: {},
          drill: [],
          refresh_mode: "MANUAL",
          refresh_seconds: 60,
          deps: {
            dataset_ids: [],
            type_names: [],
            metrics: [],
            alerts: false,
            scenario_ids: [],
          },
          created_at: "",
          updated_at: "",
        }
      : null;

  return (
    <div
      className="fixed inset-0 z-50 flex justify-end bg-black/40"
      onClick={onClose}
    >
      <div
        className="w-full max-w-lg h-full bg-background border-l border-border shadow-xl flex flex-col overflow-hidden"
        data-testid="ask-slide-over"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="px-4 py-3 border-b border-border flex items-center justify-between">
          <h3 className="text-sm font-semibold flex items-center gap-2">
            <MessageSquare className="h-4 w-4 text-xblue" />
            问一问 · 驾驶舱上下文智能问数
          </h3>
          <button
            type="button"
            onClick={onClose}
            className="rounded p-1 text-muted-foreground hover:text-foreground hover:bg-muted cursor-pointer"
          >
            <X className="h-4 w-4" />
          </button>
        </div>

        <div className="p-4 border-b border-border space-y-2">
          <div className="flex flex-wrap gap-1.5 text-[11px] text-muted-foreground">
            <Badge variant="outline">关联数据集 {datasetIds.length}</Badge>
            <Badge variant="outline">
              活跃筛选 {Object.values(filterValues).filter((v) => v !== undefined).length}
            </Badge>
            {drillPathStrings.length > 0 && (
              <Badge variant="xblue">下钻: {drillPathStrings.join(" / ")}</Badge>
            )}
          </div>
          <form onSubmit={(e) => void handleAsk(e)} className="flex gap-2">
            <Input
              value={question}
              onChange={(e) => setQuestion(e.target.value)}
              placeholder="针对当前驾驶舱数据提问，例如：各状态订单金额对比？"
              className="h-8 text-xs flex-1"
              data-testid="ask-slide-input"
            />
            <Button
              type="submit"
              disabled={loading}
              data-testid="ask-slide-submit"
            >
              <Send className="h-3.5 w-3.5" />
              {loading ? "分析中" : "提问"}
            </Button>
          </form>
        </div>

        <div className="flex-1 overflow-y-auto p-4 space-y-3 text-xs">
          {errorMsg && (
            <div className="rounded border border-foreground/30 bg-muted px-3 py-2">
              {errorMsg}
            </div>
          )}

          {response && (
            <div className="space-y-3" data-testid="ask-slide-result">
              <div className="rounded border border-border p-3 bg-muted/20 space-y-2">
                <div className="flex items-center justify-between">
                  <Badge variant="xblue">{response.intent}</Badge>
                  <Button
                    type="button"
                    disabled={adding}
                    onClick={() => void handleAddAsWidget()}
                    className="px-2.5 py-1 text-xs"
                    data-testid="ask-add-widget-btn"
                  >
                    <Plus className="h-3.5 w-3.5" />
                    {adding ? "添加中..." : "添加为组件"}
                  </Button>
                </div>
                <p className="text-xs leading-relaxed">{response.answer}</p>
                {response.sql && (
                  <pre className="rounded bg-muted p-2 font-mono-tech text-[10px] overflow-x-auto">
                    {response.sql}
                  </pre>
                )}
              </div>

              {chartWidgetPreview && response.columns.length > 0 && (
                <div className="h-52 rounded border border-border p-2">
                  <WidgetRenderer
                    widget={chartWidgetPreview}
                    isDark={isDark}
                    data={{
                      columns: response.columns.map((c) => ({
                        name: c.name,
                        data_type: c.data_type,
                      })),
                      rows: response.rows.map((r) =>
                        response.columns.map((c) => r[c.name])
                      ),
                      row_count: response.rows.length,
                      truncated: response.truncated,
                      generated_at: new Date().toISOString(),
                      data_version: "ask",
                      drill_level: 0,
                    }}
                  />
                </div>
              )}

              {response.columns.length > 0 && (
                <div className="overflow-auto border border-border rounded max-h-60">
                  <table className="w-full border-collapse text-[11px]">
                    <thead className="sticky top-0 bg-muted border-b border-border">
                      <tr>
                        {response.columns.map((c) => (
                          <th key={c.name} className="px-2 py-1 text-left font-medium">
                            {c.name}
                          </th>
                        ))}
                      </tr>
                    </thead>
                    <tbody className="divide-y divide-border">
                      {response.rows.slice(0, 50).map((row, rIdx) => (
                        <tr key={rIdx}>
                          {response.columns.map((c) => (
                            <td
                              key={`${rIdx}-${c.name}`}
                              className="px-2 py-1 font-mono-tech whitespace-nowrap"
                            >
                              {String(row[c.name] ?? "")}
                            </td>
                          ))}
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              )}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
