import { ChevronRight } from "lucide-react";
import { useI18n } from "@/i18n";
import type { DrillPathItem } from "@/lib/api/xblueview";

export interface DrillBreadcrumbProps {
  drillPath: DrillPathItem[];
  onPopToLevel: (level: number) => void;
}

export function DrillBreadcrumb({
  drillPath,
  onPopToLevel,
}: DrillBreadcrumbProps) {
  const { tx } = useI18n();
  if (drillPath.length === 0) return null;

  return (
    <div
      className="flex flex-wrap items-center gap-1 text-[11px] font-mono-tech"
      data-testid="drill-breadcrumb"
    >
      <button
        type="button"
        onClick={() => onPopToLevel(0)}
        className="rounded px-1.5 py-0.5 bg-muted hover:bg-xblue-soft hover:text-xblue cursor-pointer"
      >
        {tx("全部", "All")}
      </button>
      {drillPath.map((item, idx) => (
        <span
          key={`${item.field}-${idx}`}
          className="inline-flex items-center gap-1"
        >
          <ChevronRight className="h-3 w-3 text-muted-foreground" />
          <button
            type="button"
            onClick={() => onPopToLevel(idx + 1)}
            className={`rounded px-1.5 py-0.5 cursor-pointer ${
              idx === drillPath.length - 1
                ? "bg-xblue-soft text-xblue font-semibold border border-xblue/25"
                : "bg-muted hover:bg-xblue-soft hover:text-xblue"
            }`}
          >
            {item.field}: {String(item.value)}
          </button>
        </span>
      ))}
    </div>
  );
}
