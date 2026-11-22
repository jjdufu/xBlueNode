import { useMemo, useState } from "react";
import { MessageSquare, Plus, Send } from "lucide-react";
import { useI18n } from "@/i18n";
import {
  analyticsApi,
  type AskResponse,
  type FilterSpec,
} from "@/lib/api/analytics";
import {
  xblueviewApi,
  type DashboardDetail,
  type DrillPathItem,
  type FilterDef,
  type WidgetRecord,
} from "@/lib/api/xblueview";
import { usePrincipal } from "@/lib/permissions";
import { useTheme } from "@/lib/theme";
import { Badge, Button, Drawer, Input } from "@/components/ui/primitives";
import { buildAskFilterSpecs } from "./logic";
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

function buildFilterSpecs(
  filters: readonly FilterDef[],
  values: Record<string, unknown>
): FilterSpec[] {
  return buildAskFilterSpecs(filters, values);
}

export function AskSlideOver({
  open,
  dashboard,
  filterValues,
  drillMap,
  isDark: isDarkProp,
  onClose,
  onAddedWidget,
}: AskSlideOverProps) {
  const { tx, fmtNum } = useI18n();
  const { isDark: themeDark } = useTheme();
  const { can } = usePrincipal();
  const canEdit = can("VIEW_EDIT");
  const effectiveDark = isDarkProp ?? themeDark;

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

  const activeFilterCount = useMemo(
    () =>
      Object.values(filterValues).filter(
        (v) => v !== undefined && v !== null && v !== ""
      ).length,
    [filterValues]
  );

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
          filters: buildFilterSpecs(dashboard.filters, filterValues),
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
    if (!response || !canEdit) return;
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
        title:
          response.chart?.title ||
          question.trim().slice(0, 28) ||
          tx("问数组件", "Query Widget"),
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
          title: response.chart.title ?? tx("推荐图表", "Recommended Chart"),
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
    <Drawer
      open={open}
      onClose={onClose}
      testId="ask-slide-over"
      widthClass="max-w-lg"
      title={
        <>
          <MessageSquare className="h-4 w-4 text-xblue shrink-0" />
          <span>{tx("智能问数", "Data Q&A")}</span>
        </>
      }
    >
      <div className="space-y-2.5 pb-3 border-b border-border">
        <div className="flex flex-wrap gap-1.5 text-[11px] text-muted-foreground">
          <Badge variant="outline">
            {tx(
              `数据集 ${fmtNum(datasetIds.length, 0)}`,
              `Datasets ${fmtNum(datasetIds.length, 0)}`
            )}
          </Badge>
          <Badge variant="outline">
            {tx(
              `筛选 ${fmtNum(activeFilterCount, 0)}`,
              `Filters ${fmtNum(activeFilterCount, 0)}`
            )}
          </Badge>
          {drillPathStrings.length > 0 && (
            <Badge variant="xblue">
              {tx("下钻", "Drill")}: {drillPathStrings.join(" / ")}
            </Badge>
          )}
        </div>
        <form onSubmit={(e) => void handleAsk(e)} className="flex gap-2">
          <Input
            value={question}
            onChange={(e) => setQuestion(e.target.value)}
            placeholder={tx(
              "各状态订单金额对比？",
              "Compare order amount by status?"
            )}
            className="h-8 text-xs flex-1"
            data-testid="ask-slide-input"
          />
          <Button
            type="submit"
            size="sm"
            disabled={loading}
            className="h-8"
            data-testid="ask-slide-submit"
          >
            <Send className="h-3.5 w-3.5" />
            {loading ? tx("分析中…", "Analyzing…") : tx("提问", "Ask")}
          </Button>
        </form>
      </div>

      {errorMsg && (
        <div className="rounded border border-red-500/40 bg-red-500/10 px-3 py-2 text-red-700 dark:text-red-300">
          {errorMsg}
        </div>
      )}

      {response && (
        <div className="space-y-3" data-testid="ask-slide-result">
          <div className="rounded border border-border p-3 bg-muted/20 space-y-2">
            <div className="flex items-center justify-between gap-2">
              <Badge variant="xblue">{response.intent}</Badge>
              {canEdit && (
                <Button
                  type="button"
                  size="sm"
                  disabled={adding}
                  onClick={() => void handleAddAsWidget()}
                  data-testid="ask-add-widget-btn"
                >
                  <Plus className="h-3.5 w-3.5" />
                  {adding
                    ? tx("添加中…", "Adding…")
                    : tx("添加组件", "Add Widget")}
                </Button>
              )}
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
                isDark={effectiveDark}
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
    </Drawer>
  );
}
