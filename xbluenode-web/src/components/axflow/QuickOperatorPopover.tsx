import { useEffect, useMemo, useState } from "react";
import { Search, X } from "lucide-react";
import { useI18n } from "@/i18n";
import type { FlowOperator } from "@/lib/api/enums";
import type { OperatorSpecDto } from "@/lib/api/axflow";
import { Input } from "@/components/ui/primitives";
import {
  renderOperatorIcon,
  resolveOperatorDesc,
  resolveOperatorTitle,
} from "./OperatorPalette";

export interface QuickOperatorPopoverProps {
  operators: readonly OperatorSpecDto[];
  screenX: number;
  screenY: number;
  onSelectOperator: (kind: FlowOperator) => void;
  onClose: () => void;
}

/**
 * Anchored searchable operator selector popped at edge midpoints (`+`) or blank canvas drop points.
 */
export function QuickOperatorPopover({
  operators,
  screenX,
  screenY,
  onSelectOperator,
  onClose,
}: QuickOperatorPopoverProps) {
  const { isZh, tx } = useI18n();
  const [search, setSearch] = useState("");

  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        onClose();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [onClose]);

  const filtered = useMemo(() => {
    const q = search.trim().toLowerCase();
    if (!q) return operators;
    return operators.filter((op) => {
      const title = resolveOperatorTitle(op, isZh).toLowerCase();
      const desc = resolveOperatorDesc(op, isZh).toLowerCase();
      return (
        op.kind.toLowerCase().includes(q) ||
        title.includes(q) ||
        desc.includes(q)
      );
    });
  }, [isZh, operators, search]);

  return (
    <div
      data-testid="quick-operator-popover"
      style={{
        left: `${Math.max(12, screenX - 116)}px`,
        top: `${Math.max(12, screenY + 12)}px`,
      }}
      onClick={(e) => e.stopPropagation()}
      className="nodrag nopan absolute z-30 w-60 rounded-md border border-border bg-card p-2 shadow-xl space-y-1.5 text-xs"
    >
      <div className="flex items-center justify-between gap-1.5">
        <span className="font-semibold text-foreground">
          {tx("插入算子", "Insert Operator")}
        </span>
        <button
          type="button"
          onClick={onClose}
          aria-label={tx("关闭", "Close")}
          className="rounded p-0.5 text-muted-foreground hover:text-foreground hover:bg-muted cursor-pointer"
        >
          <X className="h-3.5 w-3.5" />
        </button>
      </div>

      <div className="relative">
        <Search className="h-3.5 w-3.5 text-muted-foreground absolute left-2 top-2 pointer-events-none" />
        <Input
          autoFocus
          data-testid="quick-operator-search-input"
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          placeholder={tx("搜索算子…", "Search…")}
          className="h-7 pl-7 text-xs"
        />
      </div>

      <div className="max-h-48 overflow-y-auto space-y-0.5 pr-0.5">
        {filtered.map((spec) => {
          const title = resolveOperatorTitle(spec, isZh);
          return (
            <button
              key={spec.kind}
              type="button"
              data-testid={`quick-op-item-${spec.kind}`}
              onClick={() => onSelectOperator(spec.kind)}
              className="w-full flex items-center gap-2 rounded px-2 py-1.5 text-left hover:bg-xblue-soft/60 hover:text-xblue transition-colors cursor-pointer"
            >
              <span className="inline-flex h-5 w-5 shrink-0 items-center justify-center rounded bg-muted text-xblue">
                {renderOperatorIcon(spec.kind, "h-3 w-3")}
              </span>
              <span className="truncate font-medium">{title}</span>
            </button>
          );
        })}
        {filtered.length === 0 && (
          <div className="py-3 text-center text-[11px] text-muted-foreground">
            {tx("无匹配算子", "No matches")}
          </div>
        )}
      </div>
    </div>
  );
}
