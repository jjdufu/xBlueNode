import { ArrowRight } from "lucide-react";
import type { SchemaDiff, SectionDiff } from "@/lib/api/xblue";

export interface DiffViewProps {
  diff: SchemaDiff;
  fromLabel?: string;
  toLabel?: string;
}

function formatDiffValue(val: unknown): string {
  if (val === undefined) return "undefined";
  if (val === null) return "null";
  if (typeof val === "string") return `"${val}"`;
  if (typeof val === "number" || typeof val === "boolean") return String(val);
  try {
    return JSON.stringify(val);
  } catch {
    return String(val);
  }
}

function SectionBlock({
  title,
  section,
}: {
  title: string;
  section: SectionDiff;
}) {
  const empty =
    section.added.length === 0 &&
    section.removed.length === 0 &&
    section.changed.length === 0;

  if (empty) return null;

  return (
    <div className="space-y-1.5">
      <div className="text-[11px] font-semibold text-muted-foreground uppercase tracking-wider">
        {title}
      </div>

      {/* Added: Green */}
      {section.added.map((name) => (
        <div
          key={`add-${name}`}
          className="flex items-center justify-between rounded border border-emerald-500/35 bg-emerald-500/10 px-2.5 py-1.5 font-mono-tech text-xs text-emerald-700 dark:text-emerald-300"
        >
          <span>+ {name}</span>
          <span className="text-[10px] font-semibold">新增</span>
        </div>
      ))}

      {/* Removed: Red */}
      {section.removed.map((name) => (
        <div
          key={`rem-${name}`}
          className="flex items-center justify-between rounded border border-red-500/35 bg-red-500/10 px-2.5 py-1.5 font-mono-tech text-xs text-red-700 dark:text-red-300"
        >
          <span>- {name}</span>
          <span className="text-[10px] font-semibold">删除</span>
        </div>
      ))}

      {/* Changed: Amber */}
      {section.changed.map((item) => (
        <div
          key={`chg-${item.name}`}
          className="rounded border border-amber-500/40 bg-amber-500/10 p-2 space-y-1 text-xs"
        >
          <div className="flex items-center justify-between font-mono-tech font-semibold text-amber-700 dark:text-amber-300">
            <span>~ {item.name}</span>
            <span className="text-[10px]">
              变更 ({item.fields.length} 项字段)
            </span>
          </div>
          <div className="space-y-1 pt-0.5">
            {item.fields.map((f, idx) => (
              <div
                key={`${f.path}-${idx}`}
                className="rounded bg-background/80 border border-border px-2 py-1 font-mono-tech text-[11px] flex flex-wrap items-center gap-1.5"
              >
                <span className="font-semibold text-foreground">{f.path}:</span>
                <span className="text-red-600 dark:text-red-400 break-all">
                  {formatDiffValue(f.before)}
                </span>
                <ArrowRight className="h-3 w-3 text-muted-foreground shrink-0" />
                <span className="text-emerald-600 dark:text-emerald-400 break-all">
                  {formatDiffValue(f.after)}
                </span>
              </div>
            ))}
          </div>
        </div>
      ))}
    </div>
  );
}

export function DiffView({ diff, fromLabel, toLabel }: DiffViewProps) {
  const isEmpty =
    diff.node_types.added.length === 0 &&
    diff.node_types.removed.length === 0 &&
    diff.node_types.changed.length === 0 &&
    diff.edge_types.added.length === 0 &&
    diff.edge_types.removed.length === 0 &&
    diff.edge_types.changed.length === 0 &&
    diff.mappings.added.length === 0 &&
    diff.mappings.removed.length === 0 &&
    diff.mappings.changed.length === 0;

  return (
    <div className="space-y-3 text-xs" data-testid="studio-diff-view">
      {(fromLabel || toLabel) && (
        <div className="flex items-center gap-2 font-mono-tech text-[11px] text-muted-foreground">
          <span>{fromLabel ?? "基准版本"}</span>
          <ArrowRight className="h-3 w-3" />
          <span className="text-foreground font-semibold">
            {toLabel ?? "目标版本"}
          </span>
        </div>
      )}

      {isEmpty ? (
        <div className="rounded border border-border bg-muted/20 p-3 text-center text-muted-foreground">
          两版本模型结构完全一致，无新增、删除或字段差异。
        </div>
      ) : (
        <div className="space-y-3">
          <SectionBlock title="节点类型 (node_types)" section={diff.node_types} />
          <SectionBlock title="关系类型 (edge_types)" section={diff.edge_types} />
          <SectionBlock title="数据映射 (mappings)" section={diff.mappings} />
        </div>
      )}
    </div>
  );
}
