import { useEffect, useState } from "react";
import { Eye, Save } from "lucide-react";
import { useI18n } from "@/i18n";
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
import { useTheme } from "@/lib/theme";
import {
  Button,
  Drawer,
  Input,
  Select,
  Textarea,
} from "@/components/ui/primitives";
import {
  ALL_WIDGET_KINDS,
  defaultDataSourceForKind,
  defaultPositionForKind,
  getWidgetKindLabel,
} from "./utils";
import { WidgetRenderer } from "./widgets/WidgetRenderer";

export interface WidgetEditorProps {
  open?: boolean;
  dashboardId: string;
  editingWidget: WidgetRecord | null;
  existingWidgets: WidgetRecord[];
  isDark?: boolean;
  onClose: () => void;
  onSaved: (widget: WidgetRecord, isNew: boolean) => void;
}

export function WidgetEditor({
  open = true,
  dashboardId,
  editingWidget,
  existingWidgets,
  isDark: isDarkProp,
  onClose,
  onSaved,
}: WidgetEditorProps) {
  const { tx, isZh } = useI18n();
  const { isDark: themeDark } = useTheme();
  const effectiveDark = isDarkProp ?? themeDark;
  const isNew = !editingWidget;

  const [title, setTitle] = useState(
    editingWidget?.title ?? tx("新建组件", "New Widget")
  );
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
    if (!open) return;
    xblueviewApi
      .listAssets({ limit: 100 })
      .then((res) => setAssets(Array.isArray(res?.items) ? res.items : []))
      .catch(() => setAssets([]));
  }, [open]);

  const handleKindChange = (nextKind: WidgetKind) => {
    setWidgetKind(nextKind);
    if (isNew) {
      setDataSourceJson(
        JSON.stringify(defaultDataSourceForKind(nextKind), null, 2)
      );
      if (nextKind === "MARKDOWN_NOTE") {
        setOptionsJson(
          JSON.stringify(
            {
              content: tx(
                "### 业务说明\n\n在此编写 Markdown 说明文档。",
                "### Notes\n\nWrite Markdown documentation here."
              ),
            },
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
      throw new Error(
        tx(
          "data_source 必须包含 kind 字段",
          'data_source must contain a "kind" field'
        )
      );
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
      throw new Error(tx("drill 必须是 JSON 数组", "drill must be a JSON array"));
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
      const fallbackTitle = getWidgetKindLabel(widgetKind, isZh);
      if (editingWidget) {
        const updated = await xblueviewApi.updateWidget(editingWidget.id, {
          widget_kind: widgetKind,
          title: title.trim() || fallbackTitle,
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
          title: title.trim() || fallbackTitle,
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
    <Drawer
      open={open}
      onClose={onClose}
      testId="widget-editor"
      widthClass="max-w-xl"
      title={
        isNew
          ? tx("添加组件", "Add Widget")
          : `${tx("编辑组件", "Edit Widget")} · ${editingWidget.title}`
      }
      footer={
        <>
          <Button
            type="button"
            variant="outline"
            onClick={onClose}
            disabled={saving}
          >
            {tx("取消", "Cancel")}
          </Button>
          <Button
            type="submit"
            form="widget-editor-form"
            disabled={saving}
            data-testid="widget-editor-save-btn"
          >
            <Save className="h-3.5 w-3.5" />
            {saving ? tx("保存中…", "Saving…") : tx("保存组件", "Save")}
          </Button>
        </>
      }
    >
      <form
        id="widget-editor-form"
        onSubmit={(e) => void handleSave(e)}
        className="space-y-3 text-xs"
      >
        <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
          <div className="space-y-1">
            <label className="text-muted-foreground block">
              {tx("组件标题", "Title")}
            </label>
            <Input
              value={title}
              onChange={(e) => setTitle(e.target.value)}
              className="h-8 text-xs"
              data-testid="widget-editor-title"
            />
          </div>

          <div className="space-y-1">
            <label className="text-muted-foreground block">
              {tx("组件类型", "Widget Type")}
            </label>
            <Select
              value={widgetKind}
              onChange={(e) => handleKindChange(e.target.value as WidgetKind)}
              className="h-8 w-full text-xs font-mono-tech"
              data-testid="widget-editor-kind"
            >
              {ALL_WIDGET_KINDS.map((k) => (
                <option key={k} value={k}>
                  {getWidgetKindLabel(k, isZh)} ({k})
                </option>
              ))}
            </Select>
          </div>
        </div>

        <div className="grid grid-cols-2 gap-3">
          <div className="space-y-1">
            <label className="text-muted-foreground block">
              {tx("刷新策略", "Refresh Mode")}
            </label>
            <Select
              value={refreshMode}
              onChange={(e) => setRefreshMode(e.target.value as RefreshMode)}
              className="h-8 w-full text-xs"
            >
              <option value="LIVE">{tx("实时推送", "Live SSE")}</option>
              <option value="INTERVAL">{tx("定时轮询", "Interval")}</option>
              <option value="MANUAL">{tx("手动刷新", "Manual")}</option>
            </Select>
          </div>
          <div className="space-y-1">
            <label className="text-muted-foreground block">
              {tx("轮询间隔 (秒)", "Interval (sec)")}
            </label>
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
                {tx("绑定空间资产", "Spatial Asset")}
              </label>
              <Select
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
                className="h-8 w-full text-xs"
              >
                <option value="">{tx("未绑定", "Unbound")}</option>
                {assets.map((a) => (
                  <option key={a.id} value={a.id}>
                    [{a.asset_kind}] {a.name} ({a.id.slice(0, 8)})
                  </option>
                ))}
              </Select>
            </div>
          )}

        <div className="space-y-1">
          <label className="text-muted-foreground block">
            {tx("数据源配置 (JSON)", "Data Source (JSON)")}
          </label>
          <Textarea
            rows={5}
            value={dataSourceJson}
            onChange={(e) => setDataSourceJson(e.target.value)}
            className="font-mono-tech text-[11px]"
            data-testid="widget-editor-datasource"
          />
        </div>

        <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
          <div className="space-y-1">
            <label className="text-muted-foreground block">
              {tx("字段映射 (JSON)", "Encoding (JSON)")}
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
              {tx("展示选项 (JSON)", "Options (JSON)")}
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
            {tx("下钻层级 (JSON)", "Drill Path (JSON)")}
          </label>
          <Textarea
            rows={2}
            value={drillJson}
            onChange={(e) => setDrillJson(e.target.value)}
            className="font-mono-tech text-[11px]"
          />
        </div>

        {errorMsg && (
          <div className="rounded border border-red-500/40 bg-red-500/10 px-2.5 py-1.5 text-red-700 dark:text-red-300">
            {errorMsg}
          </div>
        )}

        <div className="rounded-md border border-border p-3 space-y-2 bg-muted/20">
          <div className="flex items-center justify-between gap-2">
            <span className="font-semibold text-muted-foreground">
              {tx("实时预览", "Live Preview")}
            </span>
            <Button
              type="button"
              variant="outline"
              size="sm"
              disabled={previewing}
              onClick={() => void handlePreview()}
              data-testid="widget-editor-preview-btn"
            >
              <Eye className="h-3.5 w-3.5 text-xblue" />
              {previewing ? tx("加载中…", "Loading…") : tx("预览", "Preview")}
            </Button>
          </div>
          <div className="h-48 border border-border rounded bg-background p-2 overflow-hidden">
            <WidgetRenderer
              widget={previewWidgetRecord}
              data={previewData}
              isDark={effectiveDark}
            />
          </div>
        </div>
      </form>
    </Drawer>
  );
}
