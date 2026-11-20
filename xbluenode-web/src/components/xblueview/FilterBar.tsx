import { useEffect, useState } from "react";
import { Filter, RotateCcw } from "lucide-react";
import {
  xblueviewApi,
  type FilterDef,
} from "@/lib/api/xblueview";
import { Button, Input } from "@/components/ui/primitives";

export interface FilterBarProps {
  filters: FilterDef[];
  values: Record<string, unknown>;
  onChange: (key: string, value: unknown) => void;
  onReset: () => void;
}

export function FilterBar({
  filters,
  values,
  onChange,
  onReset,
}: FilterBarProps) {
  const [dynamicOptions, setDynamicOptions] = useState<
    Record<string, string[]>
  >({});

  useEffect(() => {
    let cancelled = false;
    const loadOptions = async () => {
      for (const f of filters) {
        if (!f.options_source) continue;
        try {
          const res = await xblueviewApi.previewWidget({
            widget_kind: "TABLE",
            data_source: f.options_source,
          });
          if (cancelled) return;
          const opts = Array.from(
            new Set(
              res.rows
                .map((r) => (r[0] !== undefined && r[0] !== null ? String(r[0]) : ""))
                .filter((s) => s.length > 0)
            )
          );
          setDynamicOptions((prev) => ({ ...prev, [f.key]: opts }));
        } catch {
          // Ignore options preview failure
        }
      }
    };
    void loadOptions();
    return () => {
      cancelled = true;
    };
  }, [filters]);

  if (filters.length === 0) return null;

  return (
    <div
      className="flex flex-wrap items-end gap-3 rounded-md border border-border bg-card px-3 py-2.5"
      data-testid="view-filter-bar"
    >
      <div className="flex items-center gap-1.5 text-xs font-medium text-muted-foreground pb-1.5">
        <Filter className="h-3.5 w-3.5 text-xblue" />
        全局筛选
      </div>

      {filters.map((f) => {
        const currentVal = values[f.key];
        const opts = dynamicOptions[f.key] ?? [];

        if (f.kind === "SELECT") {
          return (
            <div key={f.key} className="space-y-1 min-w-[140px]">
              <label className="block text-[11px] text-muted-foreground">
                {f.label}
              </label>
              {opts.length > 0 ? (
                <select
                  value={typeof currentVal === "string" ? currentVal : ""}
                  onChange={(e) =>
                    onChange(f.key, e.target.value || undefined)
                  }
                  className="h-8 w-full rounded-md border border-input bg-background px-2 text-xs"
                  data-testid={`filter-${f.key}`}
                >
                  <option value="">全部</option>
                  {opts.map((opt) => (
                    <option key={opt} value={opt}>
                      {opt}
                    </option>
                  ))}
                </select>
              ) : (
                <Input
                  value={typeof currentVal === "string" ? currentVal : ""}
                  onChange={(e) =>
                    onChange(f.key, e.target.value || undefined)
                  }
                  placeholder={`筛选 ${f.label}`}
                  className="h-8 text-xs"
                  data-testid={`filter-${f.key}`}
                />
              )}
            </div>
          );
        }

        if (f.kind === "MULTI_SELECT") {
          const selectedArr = Array.isArray(currentVal)
            ? currentVal.map(String)
            : [];
          return (
            <div key={f.key} className="space-y-1 min-w-[180px]">
              <label className="block text-[11px] text-muted-foreground">
                {f.label}
              </label>
              {opts.length > 0 ? (
                <div
                  className="flex flex-wrap gap-1 rounded-md border border-input bg-background px-2 py-1 min-h-8 items-center"
                  data-testid={`filter-${f.key}`}
                >
                  {opts.slice(0, 8).map((opt) => {
                    const checked = selectedArr.includes(opt);
                    return (
                      <button
                        key={opt}
                        type="button"
                        onClick={() => {
                          const next = checked
                            ? selectedArr.filter((v) => v !== opt)
                            : [...selectedArr, opt];
                          onChange(f.key, next.length > 0 ? next : undefined);
                        }}
                        className={`rounded px-1.5 py-0.5 text-[11px] font-mono-tech cursor-pointer ${
                          checked
                            ? "bg-xblue text-white"
                            : "bg-muted text-muted-foreground hover:text-foreground"
                        }`}
                      >
                        {opt}
                      </button>
                    );
                  })}
                </div>
              ) : (
                <Input
                  value={selectedArr.join(",")}
                  onChange={(e) => {
                    const parts = e.target.value
                      .split(",")
                      .map((s) => s.trim())
                      .filter(Boolean);
                    onChange(f.key, parts.length > 0 ? parts : undefined);
                  }}
                  placeholder="逗号分隔多值"
                  className="h-8 text-xs"
                  data-testid={`filter-${f.key}`}
                />
              )}
            </div>
          );
        }

        if (f.kind === "DATE_RANGE") {
          const range = Array.isArray(currentVal) ? currentVal : ["", ""];
          const fromVal = typeof range[0] === "string" ? range[0] : "";
          const toVal = typeof range[1] === "string" ? range[1] : "";
          return (
            <div key={f.key} className="space-y-1">
              <label className="block text-[11px] text-muted-foreground">
                {f.label}
              </label>
              <div className="flex items-center gap-1" data-testid={`filter-${f.key}`}>
                <Input
                  type="date"
                  value={fromVal}
                  onChange={(e) => {
                    const nextFrom = e.target.value;
                    if (!nextFrom && !toVal) {
                      onChange(f.key, undefined);
                    } else {
                      onChange(f.key, [nextFrom, toVal]);
                    }
                  }}
                  className="h-8 text-xs font-mono-tech w-32"
                />
                <span className="text-xs text-muted-foreground">~</span>
                <Input
                  type="date"
                  value={toVal}
                  onChange={(e) => {
                    const nextTo = e.target.value;
                    if (!fromVal && !nextTo) {
                      onChange(f.key, undefined);
                    } else {
                      onChange(f.key, [fromVal, nextTo]);
                    }
                  }}
                  className="h-8 text-xs font-mono-tech w-32"
                />
              </div>
            </div>
          );
        }

        if (f.kind === "NUMBER_RANGE") {
          const range = Array.isArray(currentVal) ? currentVal : [undefined, undefined];
          const minStr = range[0] !== undefined && range[0] !== null ? String(range[0]) : "";
          const maxStr = range[1] !== undefined && range[1] !== null ? String(range[1]) : "";
          return (
            <div key={f.key} className="space-y-1">
              <label className="block text-[11px] text-muted-foreground">
                {f.label}
              </label>
              <div className="flex items-center gap-1" data-testid={`filter-${f.key}`}>
                <Input
                  type="number"
                  value={minStr}
                  placeholder="最小"
                  onChange={(e) => {
                    const minN = e.target.value === "" ? undefined : Number(e.target.value);
                    const maxN = maxStr === "" ? undefined : Number(maxStr);
                    if (minN === undefined && maxN === undefined) {
                      onChange(f.key, undefined);
                    } else {
                      onChange(f.key, [minN ?? null, maxN ?? null]);
                    }
                  }}
                  className="h-8 text-xs font-mono-tech w-24"
                />
                <span className="text-xs text-muted-foreground">-</span>
                <Input
                  type="number"
                  value={maxStr}
                  placeholder="最大"
                  onChange={(e) => {
                    const minN = minStr === "" ? undefined : Number(minStr);
                    const maxN = e.target.value === "" ? undefined : Number(e.target.value);
                    if (minN === undefined && maxN === undefined) {
                      onChange(f.key, undefined);
                    } else {
                      onChange(f.key, [minN ?? null, maxN ?? null]);
                    }
                  }}
                  className="h-8 text-xs font-mono-tech w-24"
                />
              </div>
            </div>
          );
        }

        // TEXT
        return (
          <div key={f.key} className="space-y-1 min-w-[150px]">
            <label className="block text-[11px] text-muted-foreground">
              {f.label}
            </label>
            <Input
              value={typeof currentVal === "string" ? currentVal : ""}
              onChange={(e) => onChange(f.key, e.target.value || undefined)}
              placeholder={`搜索 ${f.label}...`}
              className="h-8 text-xs"
              data-testid={`filter-${f.key}`}
            />
          </div>
        );
      })}

      <Button
        type="button"
        variant="ghost"
        onClick={onReset}
        className="px-2.5 py-1.5 text-xs"
        data-testid="filter-reset-btn"
      >
        <RotateCcw className="h-3.5 w-3.5" />
        重置
      </Button>
    </div>
  );
}
