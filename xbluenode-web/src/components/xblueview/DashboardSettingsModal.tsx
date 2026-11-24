import { useState } from "react";
import { Plus, Save, Settings2, Trash2 } from "lucide-react";
import { useI18n } from "@/i18n";
import {
  xblueviewApi,
  type DashboardDetail,
  type DashboardRecord,
  type DashboardTheme,
  type FilterDef,
  type FilterKind,
} from "@/lib/api/xblueview";
import {
  Button,
  Drawer,
  IconButton,
  Input,
  Select,
} from "@/components/ui/primitives";
import { getWidgetKindLabel } from "./utils";

export interface DashboardSettingsModalProps {
  open?: boolean;
  dashboard: DashboardDetail;
  onClose: () => void;
  onSaved: (updated: DashboardRecord) => void;
}

const FILTER_KINDS: readonly {
  value: FilterKind;
  zh: string;
  en: string;
}[] = [
  { value: "SELECT", zh: "单值下拉", en: "Single Select" },
  { value: "MULTI_SELECT", zh: "多值筛选", en: "Multi Select" },
  { value: "DATE_RANGE", zh: "日期区间", en: "Date Range" },
  { value: "NUMBER_RANGE", zh: "数值区间", en: "Number Range" },
  { value: "TEXT", zh: "文本匹配", en: "Text Match" },
];

export function DashboardSettingsModal({
  open = true,
  dashboard,
  onClose,
  onSaved,
}: DashboardSettingsModalProps) {
  const { tx, isZh } = useI18n();
  const [name, setName] = useState(dashboard.name);
  const [description, setDescription] = useState(dashboard.description ?? "");
  const [theme, setTheme] = useState<DashboardTheme>(dashboard.theme);
  const [screenWidth, setScreenWidth] = useState<number>(
    dashboard.canvas.screen_width || 1920
  );
  const [screenHeight, setScreenHeight] = useState<number>(
    dashboard.canvas.screen_height || 1080
  );
  const [filters, setFilters] = useState<FilterDef[]>(() =>
    (dashboard.filters ?? []).map((f) => ({
      ...f,
      field_map: { ...(f.field_map ?? {}) },
    }))
  );

  const [saving, setSaving] = useState(false);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  const handleAddFilter = () => {
    const idx = filters.length + 1;
    setFilters((prev) => [
      ...prev,
      {
        key: `filter_${idx}`,
        label: tx(`筛选 ${idx}`, `Filter ${idx}`),
        kind: "SELECT",
        field_map: {},
      },
    ]);
  };

  const handleRemoveFilter = (index: number) => {
    setFilters((prev) => prev.filter((_, i) => i !== index));
  };

  const handleUpdateFilterMeta = (
    index: number,
    patch: Partial<Pick<FilterDef, "key" | "label" | "kind">>
  ) => {
    setFilters((prev) =>
      prev.map((item, i) => (i === index ? { ...item, ...patch } : item))
    );
  };

  const handleToggleWidgetMapping = (
    filterIdx: number,
    widgetId: string,
    checked: boolean
  ) => {
    setFilters((prev) =>
      prev.map((f, i) => {
        if (i !== filterIdx) return f;
        const nextMap = { ...f.field_map };
        if (checked) {
          nextMap[widgetId] = nextMap[widgetId]?.trim() || f.key || "status";
        } else {
          delete nextMap[widgetId];
        }
        return { ...f, field_map: nextMap };
      })
    );
  };

  const handleWidgetColumnChange = (
    filterIdx: number,
    widgetId: string,
    colName: string
  ) => {
    setFilters((prev) =>
      prev.map((f, i) => {
        if (i !== filterIdx) return f;
        return {
          ...f,
          field_map: {
            ...f.field_map,
            [widgetId]: colName,
          },
        };
      })
    );
  };

  const handleSave = async (e: React.FormEvent) => {
    e.preventDefault();
    const trimmedName = name.trim();
    if (!trimmedName) {
      setErrorMsg(tx("驾驶舱名称不能为空", "Dashboard name is required"));
      return;
    }

    const seenKeys = new Set<string>();
    const normalizedFilters: FilterDef[] = [];
    for (const f of filters) {
      const k = f.key.trim();
      if (!k) {
        setErrorMsg(tx("筛选键不能为空", "Filter key is required"));
        return;
      }
      if (seenKeys.has(k)) {
        setErrorMsg(
          tx(`筛选键「${k}」重复`, `Duplicate filter key "${k}"`)
        );
        return;
      }
      seenKeys.add(k);

      const cleanedMap: Record<string, string> = {};
      for (const [wid, col] of Object.entries(f.field_map ?? {})) {
        const trimmedCol = col.trim();
        if (trimmedCol.length > 0) {
          cleanedMap[wid] = trimmedCol;
        }
      }

      normalizedFilters.push({
        ...f,
        key: k,
        label: f.label.trim() || k,
        field_map: cleanedMap,
      });
    }

    setSaving(true);
    setErrorMsg(null);
    try {
      const updated = await xblueviewApi.updateDashboard(dashboard.id, {
        name: trimmedName,
        description: description.trim(),
        theme,
        canvas: {
          cols: dashboard.canvas.cols > 0 ? dashboard.canvas.cols : 24,
          row_height:
            dashboard.canvas.row_height > 0 ? dashboard.canvas.row_height : 30,
          screen_width: Math.max(
            640,
            Math.min(7680, Math.round(screenWidth) || 1920)
          ),
          screen_height: Math.max(
            360,
            Math.min(4320, Math.round(screenHeight) || 1080)
          ),
          ...(dashboard.canvas.zone_layout
            ? { zone_layout: dashboard.canvas.zone_layout }
            : {}),
          ...(dashboard.canvas.compact_mode
            ? { compact_mode: dashboard.canvas.compact_mode }
            : {}),
        },
        filters: normalizedFilters,
      });
      onSaved(updated);
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setSaving(false);
    }
  };

  return (
    <Drawer
      open={open}
      onClose={onClose}
      testId="dashboard-settings-modal"
      widthClass="max-w-xl"
      title={
        <>
          <Settings2 className="h-4 w-4 text-xblue shrink-0" />
          <span className="truncate">
            {tx("驾驶舱设置", "Dashboard Settings")} · {dashboard.slug}
          </span>
        </>
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
            form="dashboard-settings-form"
            disabled={saving}
            data-testid="dashboard-settings-save-btn"
          >
            <Save className="h-3.5 w-3.5" />
            {saving ? tx("保存中…", "Saving…") : tx("保存设置", "Save")}
          </Button>
        </>
      }
    >
      <form
        id="dashboard-settings-form"
        onSubmit={(e) => void handleSave(e)}
        className="space-y-4 text-xs"
      >
        <div className="space-y-3 rounded-md border border-border p-3">
          <div className="font-semibold">
            {tx("基础信息与画布", "General & Canvas")}
          </div>
          <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
            <div className="space-y-1">
              <label className="text-muted-foreground block">
                {tx("驾驶舱名称", "Name")}
              </label>
              <Input
                value={name}
                onChange={(e) => setName(e.target.value)}
                className="h-8 text-xs"
                data-testid="dashboard-settings-name"
              />
            </div>
            <div className="space-y-1">
              <label className="text-muted-foreground block">
                {tx("视觉主题", "Theme")}
              </label>
              <Select
                value={theme}
                onChange={(e) => setTheme(e.target.value as DashboardTheme)}
                className="h-8 w-full text-xs"
                data-testid="dashboard-settings-theme"
              >
                <option value="light">{tx("浅色画布", "Light")}</option>
                <option value="dark">{tx("深色画布", "Dark")}</option>
                <option value="screen">{tx("指挥大屏", "Big Screen")}</option>
              </Select>
            </div>
          </div>

          <div className="space-y-1">
            <label className="text-muted-foreground block">
              {tx("描述", "Description")}
            </label>
            <Input
              value={description}
              onChange={(e) => setDescription(e.target.value)}
              placeholder={tx(
                "华东供应链实时态势",
                "East China supply chain overview"
              )}
              className="h-8 text-xs"
              data-testid="dashboard-settings-description"
            />
          </div>

          <div className="grid grid-cols-2 gap-3">
            <div className="space-y-1">
              <label className="text-muted-foreground block">
                {tx("大屏宽度 (px)", "Screen Width (px)")}
              </label>
              <Input
                type="number"
                min={640}
                max={7680}
                value={screenWidth}
                onChange={(e) => setScreenWidth(Number(e.target.value) || 1920)}
                className="h-8 text-xs font-mono-tech"
                data-testid="dashboard-settings-screen-width"
              />
            </div>
            <div className="space-y-1">
              <label className="text-muted-foreground block">
                {tx("大屏高度 (px)", "Screen Height (px)")}
              </label>
              <Input
                type="number"
                min={360}
                max={4320}
                value={screenHeight}
                onChange={(e) =>
                  setScreenHeight(Number(e.target.value) || 1080)
                }
                className="h-8 text-xs font-mono-tech"
                data-testid="dashboard-settings-screen-height"
              />
            </div>
          </div>
        </div>

        <div className="space-y-3 rounded-md border border-border p-3">
          <div className="flex items-center justify-between gap-2">
            <div className="font-semibold">
              {tx("全局筛选器", "Global Filters")}
            </div>
            <Button
              type="button"
              variant="outline"
              size="sm"
              onClick={handleAddFilter}
              data-testid="dashboard-settings-add-filter"
            >
              <Plus className="h-3.5 w-3.5 text-xblue" />
              {tx("新增筛选", "Add Filter")}
            </Button>
          </div>

          {filters.length === 0 ? (
            <div className="py-4 text-center text-muted-foreground border border-dashed border-border rounded">
              {tx("暂无全局筛选器", "No global filters")}
            </div>
          ) : (
            <div className="space-y-3">
              {filters.map((flt, fIdx) => (
                <div
                  key={fIdx}
                  className="rounded border border-border p-2.5 space-y-2.5 bg-muted/20"
                  data-testid={`dashboard-filter-editor-${fIdx}`}
                >
                  <div className="grid grid-cols-12 gap-2 items-end">
                    <div className="col-span-3 space-y-1">
                      <label className="text-[11px] text-muted-foreground block">
                        {tx("筛选键", "Key")}
                      </label>
                      <Input
                        value={flt.key}
                        onChange={(e) =>
                          handleUpdateFilterMeta(fIdx, { key: e.target.value })
                        }
                        placeholder="status"
                        className="h-7 text-xs font-mono-tech"
                      />
                    </div>
                    <div className="col-span-4 space-y-1">
                      <label className="text-[11px] text-muted-foreground block">
                        {tx("展示名称", "Label")}
                      </label>
                      <Input
                        value={flt.label}
                        onChange={(e) =>
                          handleUpdateFilterMeta(fIdx, { label: e.target.value })
                        }
                        placeholder={tx("订单状态", "Order Status")}
                        className="h-7 text-xs"
                      />
                    </div>
                    <div className="col-span-4 space-y-1">
                      <label className="text-[11px] text-muted-foreground block">
                        {tx("筛选类型", "Type")}
                      </label>
                      <Select
                        value={flt.kind}
                        onChange={(e) =>
                          handleUpdateFilterMeta(fIdx, {
                            kind: e.target.value as FilterKind,
                          })
                        }
                        className="h-7 w-full px-1.5 text-[11px]"
                      >
                        {FILTER_KINDS.map((fk) => (
                          <option key={fk.value} value={fk.value}>
                            {isZh ? fk.zh : fk.en}
                          </option>
                        ))}
                      </Select>
                    </div>
                    <div className="col-span-1 flex justify-end">
                      <IconButton
                        size="sm"
                        onClick={() => handleRemoveFilter(fIdx)}
                        aria-label={tx("删除筛选器", "Delete filter")}
                        title={tx("删除筛选器", "Delete filter")}
                      >
                        <Trash2 className="h-3.5 w-3.5" />
                      </IconButton>
                    </div>
                  </div>

                  {dashboard.widgets.length > 0 && (
                    <div className="space-y-1.5 pt-1 border-t border-border/60">
                      <div className="text-[11px] text-muted-foreground">
                        {tx("关联组件字段映射", "Widget Column Mapping")}
                      </div>
                      <div className="space-y-1 max-h-40 overflow-y-auto">
                        {dashboard.widgets.map((w) => {
                          const mappedCol = flt.field_map?.[w.id];
                          const isChecked = mappedCol !== undefined;
                          return (
                            <div
                              key={w.id}
                              className="flex items-center justify-between gap-2 text-[11px]"
                            >
                              <label className="inline-flex items-center gap-1.5 min-w-0 flex-1 cursor-pointer">
                                <input
                                  type="checkbox"
                                  checked={isChecked}
                                  onChange={(e) =>
                                    handleToggleWidgetMapping(
                                      fIdx,
                                      w.id,
                                      e.target.checked
                                    )
                                  }
                                  className="accent-[#2563eb]"
                                />
                                <span className="truncate">
                                  {w.title ||
                                    getWidgetKindLabel(w.widget_kind, isZh)}
                                </span>
                                <span className="font-mono-tech text-[10px] text-muted-foreground shrink-0">
                                  {getWidgetKindLabel(w.widget_kind, isZh)}
                                </span>
                              </label>
                              {isChecked && (
                                <Input
                                  value={mappedCol}
                                  onChange={(e) =>
                                    handleWidgetColumnChange(
                                      fIdx,
                                      w.id,
                                      e.target.value
                                    )
                                  }
                                  placeholder="status"
                                  className="h-6 w-32 text-[11px] font-mono-tech px-1.5"
                                />
                              )}
                            </div>
                          );
                        })}
                      </div>
                    </div>
                  )}
                </div>
              ))}
            </div>
          )}
        </div>

        {errorMsg && (
          <div className="rounded border border-red-500/40 bg-red-500/10 px-3 py-2 text-red-700 dark:text-red-300">
            {errorMsg}
          </div>
        )}
      </form>
    </Drawer>
  );
}
