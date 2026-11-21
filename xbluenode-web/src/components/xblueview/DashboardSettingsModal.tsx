import { useState } from "react";
import { Plus, Save, Settings2, Trash2, X } from "lucide-react";
import {
  xblueviewApi,
  type DashboardDetail,
  type DashboardRecord,
  type DashboardTheme,
  type FilterDef,
  type FilterKind,
} from "@/lib/api/xblueview";
import { Button, Input } from "@/components/ui/primitives";
import { WIDGET_KIND_LABELS } from "./utils";

export interface DashboardSettingsModalProps {
  dashboard: DashboardDetail;
  onClose: () => void;
  onSaved: (updated: DashboardRecord) => void;
}

const FILTER_KINDS: readonly { value: FilterKind; label: string }[] = [
  { value: "SELECT", label: "SELECT（单值下拉/等于）" },
  { value: "MULTI_SELECT", label: "MULTI_SELECT（多选 IN）" },
  { value: "DATE_RANGE", label: "DATE_RANGE（日期区间）" },
  { value: "NUMBER_RANGE", label: "NUMBER_RANGE（数值区间）" },
  { value: "TEXT", label: "TEXT（文本模糊匹配）" },
];

export function DashboardSettingsModal({
  dashboard,
  onClose,
  onSaved,
}: DashboardSettingsModalProps) {
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
        label: `筛选条件 ${idx}`,
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
      setErrorMsg("驾驶舱名称不能为空");
      return;
    }

    const seenKeys = new Set<string>();
    const normalizedFilters: FilterDef[] = [];
    for (const f of filters) {
      const k = f.key.trim();
      if (!k) {
        setErrorMsg("全局筛选器的 key 不能为空");
        return;
      }
      if (seenKeys.has(k)) {
        setErrorMsg(`全局筛选器 key「${k}」重复`);
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
          screen_width: Math.max(640, Math.min(7680, Math.round(screenWidth) || 1920)),
          screen_height: Math.max(360, Math.min(4320, Math.round(screenHeight) || 1080)),
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
    <div
      className="fixed inset-0 z-50 flex justify-end bg-black/40"
      data-testid="dashboard-settings-backdrop"
      onClick={onClose}
    >
      <div
        className="w-full max-w-xl h-full bg-background text-foreground border-l border-border shadow-xl flex flex-col overflow-hidden"
        data-testid="dashboard-settings-modal"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="px-4 py-3 border-b border-border flex items-center justify-between">
          <h3 className="text-sm font-semibold flex items-center gap-2">
            <Settings2 className="h-4 w-4 text-xblue" />
            驾驶舱设置 · {dashboard.slug}
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
          className="flex-1 overflow-y-auto p-4 space-y-4 text-xs"
        >
          {/* Basic Info & Theme */}
          <div className="space-y-3 rounded-md border border-border p-3">
            <div className="font-semibold">基础信息与主题</div>
            <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
              <div className="space-y-1">
                <label className="text-muted-foreground block">驾驶舱名称</label>
                <Input
                  value={name}
                  onChange={(e) => setName(e.target.value)}
                  className="h-8 text-xs"
                  data-testid="dashboard-settings-name"
                />
              </div>
              <div className="space-y-1">
                <label className="text-muted-foreground block">
                  视觉主题 (light / dark / screen)
                </label>
                <select
                  value={theme}
                  onChange={(e) => setTheme(e.target.value as DashboardTheme)}
                  className="h-8 w-full rounded-md border border-input bg-background px-2 text-xs font-mono-tech"
                  data-testid="dashboard-settings-theme"
                >
                  <option value="light">light（浅色画布）</option>
                  <option value="dark">dark（深色隔离画布）</option>
                  <option value="screen">screen（指挥大屏深色）</option>
                </select>
              </div>
            </div>

            <div className="space-y-1">
              <label className="text-muted-foreground block">描述</label>
              <Input
                value={description}
                onChange={(e) => setDescription(e.target.value)}
                placeholder="驾驶舱业务用途说明"
                className="h-8 text-xs"
                data-testid="dashboard-settings-description"
              />
            </div>

            <div className="grid grid-cols-2 gap-3">
              <div className="space-y-1">
                <label className="text-muted-foreground block">
                  大屏设计宽度 (screen_width px)
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
                  大屏设计高度 (screen_height px)
                </label>
                <Input
                  type="number"
                  min={360}
                  max={4320}
                  value={screenHeight}
                  onChange={(e) => setScreenHeight(Number(e.target.value) || 1080)}
                  className="h-8 text-xs font-mono-tech"
                  data-testid="dashboard-settings-screen-height"
                />
              </div>
            </div>
          </div>

          {/* Global Filters Editor */}
          <div className="space-y-3 rounded-md border border-border p-3">
            <div className="flex items-center justify-between">
              <div>
                <div className="font-semibold">全局筛选器 (FilterDef[])</div>
                <div className="text-[11px] text-muted-foreground">
                  定义筛选键 key 并勾选关联组件映射目标列名 field_map[widgetId]
                </div>
              </div>
              <Button
                type="button"
                variant="outline"
                onClick={handleAddFilter}
                className="px-2.5 py-1 text-xs"
                data-testid="dashboard-settings-add-filter"
              >
                <Plus className="h-3.5 w-3.5 text-xblue" />
                新增筛选器
              </Button>
            </div>

            {filters.length === 0 ? (
              <div className="py-4 text-center text-muted-foreground border border-dashed border-border rounded">
                暂未配置全局筛选器
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
                          筛选键 (key)
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
                          展示标签 (label)
                        </label>
                        <Input
                          value={flt.label}
                          onChange={(e) =>
                            handleUpdateFilterMeta(fIdx, { label: e.target.value })
                          }
                          placeholder="订单状态"
                          className="h-7 text-xs"
                        />
                      </div>
                      <div className="col-span-4 space-y-1">
                        <label className="text-[11px] text-muted-foreground block">
                          筛选类型 (kind)
                        </label>
                        <select
                          value={flt.kind}
                          onChange={(e) =>
                            handleUpdateFilterMeta(fIdx, {
                              kind: e.target.value as FilterKind,
                            })
                          }
                          className="h-7 w-full rounded border border-input bg-background px-1.5 text-[11px] font-mono-tech"
                        >
                          {FILTER_KINDS.map((fk) => (
                            <option key={fk.value} value={fk.value}>
                              {fk.label}
                            </option>
                          ))}
                        </select>
                      </div>
                      <div className="col-span-1 text-right pb-0.5">
                        <button
                          type="button"
                          onClick={() => handleRemoveFilter(fIdx)}
                          className="rounded p-1 text-muted-foreground hover:text-foreground cursor-pointer"
                          title="删除该筛选器"
                        >
                          <Trash2 className="h-3.5 w-3.5" />
                        </button>
                      </div>
                    </div>

                    {dashboard.widgets.length > 0 && (
                      <div className="space-y-1.5 pt-1 border-t border-border/60">
                        <div className="text-[11px] text-muted-foreground">
                          关联组件字段映射 (field_map):
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
                                  />
                                  <span className="truncate">
                                    {w.title || WIDGET_KIND_LABELS[w.widget_kind]}
                                  </span>
                                  <span className="font-mono-tech text-[10px] text-muted-foreground shrink-0">
                                    ({w.widget_kind})
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
                                    placeholder="目标列名"
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
            <div className="rounded border border-foreground/30 bg-muted px-3 py-2 text-xs">
              {errorMsg}
            </div>
          )}

          <div className="flex items-center justify-end gap-2 pt-2 border-t border-border">
            <Button type="button" variant="outline" onClick={onClose}>
              取消
            </Button>
            <Button
              type="submit"
              disabled={saving}
              data-testid="dashboard-settings-save-btn"
            >
              <Save className="h-3.5 w-3.5" />
              {saving ? "保存中..." : "保存驾驶舱设置"}
            </Button>
          </div>
        </form>
      </div>
    </div>
  );
}
