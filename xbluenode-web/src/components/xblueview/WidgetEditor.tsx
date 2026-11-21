import { useEffect, useState } from "react";
import { Eye, Save, X } from "lucide-react";
import type { WidgetKind } from "@/lib/api/enums";
import {
  xblueviewApi,
  type AssetRecord,
  type CreateWidgetRequest,
  type DrillLevel,
  type RefreshMode,
  type WidgetDataResponse,
  type WidgetDataSource,
  type WidgetRecord,
} from "@/lib/api/xblueview";
import { Button, Input, Textarea } from "@/components/ui/primitives";
import {
  ALL_WIDGET_KINDS,
  WIDGET_KIND_LABELS,
  defaultDataSourceForKind,
  defaultPositionForKind,
} from "./utils";
import { WidgetRenderer } from "./widgets/WidgetRenderer";

export interface WidgetEditorProps {
  dashboardId: string;
  editingWidget: WidgetRecord | null;
  existingWidgets: WidgetRecord[];
  isDark?: boolean;
  onClose: () => void;
  onSaved: (widget: WidgetRecord, isNew: boolean) => void;
}

export function WidgetEditor({
  dashboardId,
  editingWidget,
  existingWidgets,
  isDark = false,
  onClose,
  onSaved,
}: WidgetEditorProps) {
  const isNew = !editingWidget;

  const [title, setTitle] = useState(editingWidget?.title ?? "新建组件");
  const [widgetKind, setWidgetKind] = useState<WidgetKind>(
    editingWidget?.widget_kind ?? "BAR_CHART"
  );
  const [refreshMode, setRefreshMode] = useState<RefreshMode>(
    editingWidget?.refresh_mode ?? "LIVE"
  );
  const [refreshSeconds, setRefreshSeconds] = useState<number>(
    editingWidget?.refresh_seconds ?? 60
  );
  const [dataSourceJson, setDataSourceJson] = useState<string>(() =>
    JSON.stringify(
      editingWidget?.data_source ?? defaultDataSourceForKind("BAR_CHART"),
      null,
      2
    )
  );
  const [encodingJson, setEncodingJson] = useState<string>(() =>
    JSON.stringify(editingWidget?.encoding ?? {}, null, 2)
  );
  const [optionsJson, setOptionsJson] = useState<string>(() =>
    JSON.stringify(editingWidget?.options ?? {}, null, 2)
  );
  const [drillJson, setDrillJson] = useState<string>(() =>
    JSON.stringify(editingWidget?.drill ?? [], null, 2)
  );

  const [assets, setAssets] = useState<AssetRecord[]>([]);
  const [previewData, setPreviewData] = useState<WidgetDataResponse | null>(
    null
  );
  const [previewing, setPreviewing] = useState(false);
  const [saving, setSaving] = useState(false);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  useEffect(() => {
    xblueviewApi
      .listAssets({ limit: 100 })
      .then((res) => setAssets(Array.isArray(res?.items) ? res.items : []))
      .catch(() => setAssets([]));
  }, []);

  const handleKindChange = (nextKind: WidgetKind) => {
    setWidgetKind(nextKind);
    if (isNew) {
      setDataSourceJson(
        JSON.stringify(defaultDataSourceForKind(nextKind), null, 2)
      );
      if (nextKind === "MARKDOWN_NOTE") {
        setOptionsJson(
          JSON.stringify(
            { content: "### 业务说明\n\n在此编写 Markdown 说明文档。" },
            null,
            2
          )
        );
      }
    }
  };

  const parseConfigFields = (): {
    dataSource: WidgetDataSource;
    encoding: Record<string, unknown>;
    options: Record<string, unknown>;
    drill: DrillLevel[];
  } => {
    const ds = JSON.parse(dataSourceJson) as WidgetDataSource;
    const enc = JSON.parse(encodingJson) as Record<string, unknown>;
    const opt = JSON.parse(optionsJson) as Record<string, unknown>;
    const drl = JSON.parse(drillJson) as DrillLevel[];
    if (!ds || typeof ds !== "object" || !("kind" in ds)) {
      throw new Error("data_source 必须包含 kind 字段");
    }
    if (ds.kind === "ACTION") {
      ds.flow_id =
        typeof ds.flow_id === "string" && ds.flow_id.trim().length > 0
          ? ds.flow_id.trim()
          : undefined;
      ds.node_id =
        typeof ds.node_id === "string" && ds.node_id.trim().length > 0
          ? ds.node_id.trim()
          : undefined;
    }
    if (!Array.isArray(drl)) {
      throw new Error("drill 必须是 JSON 数组");
    }
    return { dataSource: ds, encoding: enc, options: opt, drill: drl };
  };

  const handlePreview = async () => {
    setPreviewing(true);
    setErrorMsg(null);
    try {
      const cfg = parseConfigFields();
      const res = await xblueviewApi.previewWidget({
        widget_kind: widgetKind,
        data_source: cfg.dataSource,
        encoding: cfg.encoding,
        options: cfg.options,
        drill: cfg.drill,
      });
      setPreviewData(res);
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setPreviewing(false);
    }
  };

  const handleSave = async (e: React.FormEvent) => {
    e.preventDefault();
    setSaving(true);
    setErrorMsg(null);
    try {
      const cfg = parseConfigFields();
      if (editingWidget) {
        const updated = await xblueviewApi.updateWidget(editingWidget.id, {
          widget_kind: widgetKind,
          title: title.trim() || WIDGET_KIND_LABELS[widgetKind],
          data_source: cfg.dataSource,
          encoding: cfg.encoding,
          options: cfg.options,
          drill: cfg.drill,
          refresh_mode: refreshMode,
          refresh_seconds: Math.max(5, Math.min(86400, refreshSeconds)),
        });
        onSaved(updated, false);
      } else {
        const req: CreateWidgetRequest = {
          widget_kind: widgetKind,
          title: title.trim() || WIDGET_KIND_LABELS[widgetKind],
          position: defaultPositionForKind(widgetKind, existingWidgets),
          data_source: cfg.dataSource,
          encoding: cfg.encoding,
          options: cfg.options,
          drill: cfg.drill,
          refresh_mode: refreshMode,
          refresh_seconds: Math.max(5, Math.min(86400, refreshSeconds)),
        };
        const created = await xblueviewApi.createWidget(dashboardId, req);
        onSaved(created, true);
      }
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setSaving(false);
    }
  };

  const previewWidgetRecord: WidgetRecord = {
    id: editingWidget?.id ?? "preview-widget",
    tenant_id: editingWidget?.tenant_id ?? "default",
    dashboard_id: dashboardId,
    widget_kind: widgetKind,
    title,
    position: editingWidget?.position ?? { x: 0, y: 0, w: 8, h: 6 },
    data_source: (() => {
      try {
        return JSON.parse(dataSourceJson) as WidgetDataSource;
      } catch {
        return { kind: "STATIC", rows: [] };
      }
    })(),
    encoding: (() => {
      try {
        return JSON.parse(encodingJson) as Record<string, unknown>;
      } catch {
        return {};
      }
    })(),
    options: (() => {
      try {
        return JSON.parse(optionsJson) as Record<string, unknown>;
      } catch {
        return {};
      }
    })(),
    drill: [],
    refresh_mode: refreshMode,
    refresh_seconds: refreshSeconds,
    deps: editingWidget?.deps ?? {
      dataset_ids: [],
      type_names: [],
      metrics: [],
      alerts: false,
      scenario_ids: [],
    },
    created_at: "",
    updated_at: "",
  };

  return (
    <div
      className="fixed inset-0 z-50 flex justify-end bg-black/40"
      data-testid="widget-editor-backdrop"
      onClick={onClose}
    >
      <div
        className="w-full max-w-xl h-full bg-background border-l border-border shadow-xl flex flex-col overflow-hidden"
        data-testid="widget-editor"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="px-4 py-3 border-b border-border flex items-center justify-between">
          <h3 className="text-sm font-semibold">
            {isNew ? "添加驾驶舱组件" : `编辑组件 · ${editingWidget.title}`}
          </h3>
          <button
            type="button"
            onClick={onClose}
            className="rounded p-1 text-muted-foreground hover:text-foreground hover:bg-muted cursor-pointer"
          >
            <X className="h-4 w-4" />
          </button>
        </div>

        <form
          onSubmit={(e) => void handleSave(e)}
          className="flex-1 overflow-y-auto p-4 space-y-3 text-xs"
        >
          <div className="grid grid-cols-2 gap-3">
            <div className="space-y-1">
              <label className="text-muted-foreground block">组件标题</label>
              <Input
                value={title}
                onChange={(e) => setTitle(e.target.value)}
                className="h-8 text-xs"
                data-testid="widget-editor-title"
              />
            </div>

            <div className="space-y-1">
              <label className="text-muted-foreground block">组件类型 (20 种)</label>
              <select
                value={widgetKind}
                onChange={(e) => handleKindChange(e.target.value as WidgetKind)}
                className="h-8 w-full rounded-md border border-input bg-background px-2 text-xs font-mono-tech"
                data-testid="widget-editor-kind"
              >
                {ALL_WIDGET_KINDS.map((k) => (
                  <option key={k} value={k}>
                    {k} · {WIDGET_KIND_LABELS[k]}
                  </option>
                ))}
              </select>
            </div>
          </div>

          <div className="grid grid-cols-2 gap-3">
            <div className="space-y-1">
              <label className="text-muted-foreground block">刷新策略</label>
              <select
                value={refreshMode}
                onChange={(e) => setRefreshMode(e.target.value as RefreshMode)}
                className="h-8 w-full rounded-md border border-input bg-background px-2 text-xs font-mono-tech"
              >
                <option value="LIVE">LIVE（SSE 事件实时失效）</option>
                <option value="INTERVAL">INTERVAL（定时轮询）</option>
                <option value="MANUAL">MANUAL（手动刷新）</option>
              </select>
            </div>
            <div className="space-y-1">
              <label className="text-muted-foreground block">轮询间隔 (秒)</label>
              <Input
                type="number"
                min={5}
                max={86400}
                value={refreshSeconds}
                disabled={refreshMode !== "INTERVAL"}
                onChange={(e) => setRefreshSeconds(Number(e.target.value) || 60)}
                className="h-8 text-xs font-mono-tech"
              />
            </div>
          </div>

          {(widgetKind === "FLOOR_PLAN" ||
            widgetKind === "SCENE_3D" ||
            widgetKind === "MAP") &&
            assets.length > 0 && (
              <div className="space-y-1">
                <label className="text-muted-foreground block">
                  快速绑定空间资产
                </label>
                <select
                  onChange={(e) => {
                    const aid = e.target.value;
                    if (!aid) return;
                    try {
                      const ds = JSON.parse(dataSourceJson) as Record<
                        string,
                        unknown
                      >;
                      if (widgetKind === "MAP") {
                        const opt = JSON.parse(optionsJson) as Record<
                          string,
                          unknown
                        >;
                        opt.geojson_asset_id = aid;
                        setOptionsJson(JSON.stringify(opt, null, 2));
                      } else {
                        ds.asset_id = aid;
                        setDataSourceJson(JSON.stringify(ds, null, 2));
                      }
                    } catch {
                      // Ignore invalid JSON while editing
                    }
                  }}
                  className="h-8 w-full rounded-md border border-input bg-background px-2 text-xs"
                >
                  <option value="">选择已上传的空间资产...</option>
                  {assets.map((a) => (
                    <option key={a.id} value={a.id}>
                      [{a.asset_kind}] {a.name} ({a.id.slice(0, 8)})
                    </option>
                  ))}
                </select>
              </div>
            )}

          <div className="space-y-1">
            <label className="text-muted-foreground block">
              数据源配置 (data_source JSON)
            </label>
            <Textarea
              rows={5}
              value={dataSourceJson}
              onChange={(e) => setDataSourceJson(e.target.value)}
              className="font-mono-tech text-[11px]"
              data-testid="widget-editor-datasource"
            />
          </div>

          <div className="grid grid-cols-2 gap-3">
            <div className="space-y-1">
              <label className="text-muted-foreground block">
                视觉映射 (encoding JSON)
              </label>
              <Textarea
                rows={4}
                value={encodingJson}
                onChange={(e) => setEncodingJson(e.target.value)}
                className="font-mono-tech text-[11px]"
              />
            </div>

            <div className="space-y-1">
              <label className="text-muted-foreground block">
                选项与阈值 (options JSON)
              </label>
              <Textarea
                rows={4}
                value={optionsJson}
                onChange={(e) => setOptionsJson(e.target.value)}
                className="font-mono-tech text-[11px]"
              />
            </div>
          </div>

          <div className="space-y-1">
            <label className="text-muted-foreground block">
              下钻层级 (drill JSON)
            </label>
            <Textarea
              rows={2}
              value={drillJson}
              onChange={(e) => setDrillJson(e.target.value)}
              className="font-mono-tech text-[11px]"
            />
          </div>

          {errorMsg && (
            <div className="rounded border border-foreground/30 bg-muted px-2.5 py-1.5 text-xs">
              {errorMsg}
            </div>
          )}

          {/* Live Preview Box */}
          <div className="rounded-md border border-border p-3 space-y-2 bg-muted/20">
            <div className="flex items-center justify-between">
              <span className="font-semibold text-muted-foreground">
                实时数据预览
              </span>
              <Button
                type="button"
                variant="outline"
                disabled={previewing}
                onClick={() => void handlePreview()}
                className="px-2.5 py-1 text-xs"
                data-testid="widget-editor-preview-btn"
              >
                <Eye className="h-3.5 w-3.5 text-xblue" />
                {previewing ? "预览中..." : "预览组件"}
              </Button>
            </div>
            <div className="h-48 border border-border rounded bg-background p-2 overflow-hidden">
              <WidgetRenderer
                widget={previewWidgetRecord}
                data={previewData}
                isDark={isDark}
              />
            </div>
          </div>

          <div className="flex items-center justify-end gap-2 pt-2 border-t border-border">
            <Button type="button" variant="outline" onClick={onClose}>
              取消
            </Button>
            <Button
              type="submit"
              disabled={saving}
              data-testid="widget-editor-save-btn"
            >
              <Save className="h-3.5 w-3.5" />
              {saving ? "保存中..." : "保存组件"}
            </Button>
          </div>
        </form>
      </div>
    </div>
  );
}
