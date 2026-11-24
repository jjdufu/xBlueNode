import { useEffect, useMemo, useState } from "react";
import { useNavigate } from "react-router";
import { CheckCircle2, ExternalLink, Pin } from "lucide-react";
import { useI18n } from "@/i18n";
import { ApiError } from "@/lib/api/client";
import type { WidgetKind } from "@/lib/api/enums";
import {
  xblueviewApi,
  type DashboardRecord,
  type PinChartToDashboardResponse,
  type SemanticZoneKind,
  type WidgetDataSource,
} from "@/lib/api/xblueview";
import {
  Badge,
  Button,
  Input,
  Modal,
  Select,
} from "@/components/ui/primitives";

export interface PinChartDraftSpec {
  savedChartId?: string | null;
  name: string;
  description?: string;
  widgetKind: WidgetKind;
  dataSource: WidgetDataSource;
  encoding?: Record<string, unknown>;
  options?: Record<string, unknown>;
}

export interface PinToDashboardModalProps {
  open: boolean;
  draft: PinChartDraftSpec | null;
  onClose: () => void;
  onPinned?: (resp: PinChartToDashboardResponse, savedChartId: string) => void;
}

const FIVE_ZONE_OPTIONS: ReadonlyArray<{
  zone: SemanticZoneKind;
  zh: string;
  en: string;
}> = [
  { zone: "KPI", zh: "核心指标", en: "KPI" },
  { zone: "INSIGHT", zh: "归因洞察", en: "Insight" },
  { zone: "SCENE", zh: "孪生态势", en: "Scene" },
  { zone: "DECISION", zh: "协同决策", en: "Decision" },
  { zone: "CONTROL", zh: "全局控制", en: "Control" },
];

function defaultZoneForWidget(kind: WidgetKind): SemanticZoneKind {
  if (kind === "KPI_CARD") return "KPI";
  if (
    kind === "GRAPH_TOPOLOGY" ||
    kind === "MAP" ||
    kind === "FLOOR_PLAN" ||
    kind === "SCENE_3D"
  ) {
    return "SCENE";
  }
  if (
    kind === "ALERT_LIST" ||
    kind === "SIM_COMPARE" ||
    kind === "ACTION_BUTTON" ||
    kind === "ACTION_FORM"
  ) {
    return "DECISION";
  }
  return "INSIGHT";
}

export function PinToDashboardModal({
  open,
  draft,
  onClose,
  onPinned,
}: PinToDashboardModalProps) {
  const { isZh, tx } = useI18n();
  const navigate = useNavigate();

  const [dashboards, setDashboards] = useState<DashboardRecord[]>([]);
  const [loadingDashboards, setLoadingDashboards] = useState(false);
  const [selectedDashboardId, setSelectedDashboardId] = useState("");
  const [targetZone, setTargetZone] = useState<SemanticZoneKind>("INSIGHT");
  const [chartName, setChartName] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);
  const [pinnedResult, setPinnedResult] =
    useState<PinChartToDashboardResponse | null>(null);

  useEffect(() => {
    if (!open || !draft) return;
    setPinnedResult(null);
    setErrorMsg(null);
    setChartName(draft.name || tx("业务分析图表", "Analytics Chart"));
    setTargetZone(defaultZoneForWidget(draft.widgetKind));

    let cancelled = false;
    setLoadingDashboards(true);
    xblueviewApi
      .listDashboards({ limit: 100 })
      .then((res) => {
        if (cancelled) return;
        const list = Array.isArray(res?.items) ? res.items : [];
        setDashboards(list);
        setSelectedDashboardId((prev) =>
          list.some((d) => d.id === prev) ? prev : (list[0]?.id ?? "")
        );
      })
      .catch((err) => {
        if (cancelled) return;
        setErrorMsg(err instanceof Error ? err.message : String(err));
      })
      .finally(() => {
        if (!cancelled) {
          setLoadingDashboards(false);
        }
      });

    return () => {
      cancelled = true;
    };
  }, [draft, open, tx]);

  const selectedDashboard = useMemo(
    () => dashboards.find((d) => d.id === selectedDashboardId) ?? null,
    [dashboards, selectedDashboardId]
  );

  const isFiveZoneTarget =
    selectedDashboard?.canvas?.zone_layout === "STANDARD_FIVE_ZONE";

  const handlePinSubmit = async () => {
    if (!draft || !selectedDashboardId) return;
    setSubmitting(true);
    setErrorMsg(null);
    try {
      let chartId = draft.savedChartId?.trim() || "";
      if (!chartId) {
        const baseName =
          chartName.trim() || draft.name.trim() || tx("业务图表", "Chart");
        try {
          const created = await xblueviewApi.createSavedChart({
            name: baseName,
            description: draft.description ?? "",
            widget_kind: draft.widgetKind,
            data_source: draft.dataSource,
            encoding: draft.encoding ?? {},
            options: draft.options ?? {},
          });
          chartId = created.id;
        } catch (err) {
          if (err instanceof ApiError && err.status === 409) {
            const retry = await xblueviewApi.createSavedChart({
              name: `${baseName.slice(0, 110)}-${Date.now().toString(36).slice(-4)}`,
              description: draft.description ?? "",
              widget_kind: draft.widgetKind,
              data_source: draft.dataSource,
              encoding: draft.encoding ?? {},
              options: draft.options ?? {},
            });
            chartId = retry.id;
          } else {
            throw err;
          }
        }
      }

      const resp = await xblueviewApi.pinSavedChart(chartId, {
        dashboard_id: selectedDashboardId,
        ...(isFiveZoneTarget ? { target_zone: targetZone } : {}),
      });
      setPinnedResult(resp);
      onPinned?.(resp, chartId);
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <Modal
      open={open}
      onClose={onClose}
      testId="pin-to-dashboard-modal"
      maxWidthClass="max-w-md"
      title={
        <>
          <Pin className="h-4 w-4 text-xblue shrink-0" />
          <span>{tx("钉入看板", "Pin Chart")}</span>
        </>
      }
      footer={
        pinnedResult ? (
          <>
            <Button
              type="button"
              variant="outline"
              onClick={onClose}
              data-testid="pin-modal-close-btn"
            >
              {tx("关闭", "Close")}
            </Button>
            <Button
              type="button"
              onClick={() => {
                const link = pinnedResult.deep_link;
                onClose();
                void navigate(link);
              }}
              data-testid="pin-modal-open-dashboard-btn"
            >
              <ExternalLink className="h-3.5 w-3.5" />
              {tx("打开看板", "Open")}
            </Button>
          </>
        ) : (
          <>
            <Button
              type="button"
              variant="outline"
              onClick={onClose}
              disabled={submitting}
            >
              {tx("取消", "Cancel")}
            </Button>
            <Button
              type="button"
              disabled={
                submitting ||
                loadingDashboards ||
                !selectedDashboardId ||
                (!draft?.savedChartId && !chartName.trim())
              }
              onClick={() => void handlePinSubmit()}
              data-testid="pin-modal-confirm-btn"
            >
              <Pin className="h-3.5 w-3.5" />
              {submitting ? tx("投送中…", "Pinning…") : tx("钉入", "Pin")}
            </Button>
          </>
        )
      }
    >
      {pinnedResult ? (
        <div
          className="rounded-md border border-xblue/35 bg-xblue-soft/35 p-3 space-y-2"
          data-testid="pin-modal-success"
        >
          <div className="flex items-center justify-between gap-2">
            <span className="inline-flex items-center gap-1.5 font-semibold text-xblue">
              <CheckCircle2 className="h-4 w-4" />
              {tx("已钉入", "Pinned")}
            </span>
            {pinnedResult.semantic_zone && (
              <Badge variant="xblue" data-testid="pin-modal-result-zone">
                {FIVE_ZONE_OPTIONS.find(
                  (z) => z.zone === pinnedResult.semantic_zone
                )?.[isZh ? "zh" : "en"] ?? pinnedResult.semantic_zone}
              </Badge>
            )}
          </div>
          <div className="font-mono-tech text-[11px] text-muted-foreground">
            {pinnedResult.deep_link} · ({pinnedResult.widget.position.x},{" "}
            {pinnedResult.widget.position.y}, {pinnedResult.widget.position.w}×
            {pinnedResult.widget.position.h})
          </div>
        </div>
      ) : (
        <div className="space-y-3">
          {!draft?.savedChartId && (
            <div className="space-y-1">
              <label className="text-muted-foreground block">
                {tx("图表名称", "Chart Name")}
              </label>
              <Input
                value={chartName}
                onChange={(e) => setChartName(e.target.value)}
                placeholder={tx("订单状态分布", "Order Status Breakdown")}
                className="h-8 text-xs"
                data-testid="pin-modal-chart-name"
              />
            </div>
          )}

          <div className="space-y-1">
            <label className="text-muted-foreground block">
              {tx("目标驾驶舱", "Target Cockpit")}
            </label>
            <Select
              value={selectedDashboardId}
              onChange={(e) => setSelectedDashboardId(e.target.value)}
              disabled={loadingDashboards || dashboards.length === 0}
              className="h-8 w-full text-xs"
              data-testid="pin-modal-dashboard-select"
            >
              {dashboards.length === 0 && (
                <option value="">
                  {loadingDashboards
                    ? tx("加载中…", "Loading…")
                    : tx("暂无驾驶舱", "No dashboards")}
                </option>
              )}
              {dashboards.map((d) => (
                <option key={d.id} value={d.id}>
                  {d.name} (/view/{d.slug})
                </option>
              ))}
            </Select>
          </div>

          {isFiveZoneTarget && (
            <div className="space-y-1">
              <label className="text-muted-foreground block">
                {tx("语义分区", "Semantic Zone")}
              </label>
              <Select
                value={targetZone}
                onChange={(e) =>
                  setTargetZone(e.target.value as SemanticZoneKind)
                }
                className="h-8 w-full text-xs"
                data-testid="pin-modal-zone-select"
              >
                {FIVE_ZONE_OPTIONS.map((z) => (
                  <option key={z.zone} value={z.zone}>
                    {isZh ? z.zh : z.en}
                  </option>
                ))}
              </Select>
            </div>
          )}

          {errorMsg && (
            <div
              className="rounded border border-red-500/40 bg-red-500/10 px-2.5 py-2 text-red-700 dark:text-red-300"
              data-testid="pin-modal-error"
            >
              {errorMsg}
            </div>
          )}
        </div>
      )}
    </Modal>
  );
}
