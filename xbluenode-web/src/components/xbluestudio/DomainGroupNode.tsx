import { Handle, Position, type Node, type NodeProps } from "@xyflow/react";
import { ChevronDown, ChevronRight, Layers } from "lucide-react";
import { useI18n } from "@/i18n";
import { Badge } from "@/components/ui/primitives";
import { cn } from "@/lib/utils";

export interface DomainGroupNodeData extends Record<string, unknown> {
  domainGroup: string;
  memberCount: number;
  collapsed: boolean;
  width: number;
  height: number;
  onToggleCollapse: (domainGroup: string) => void;
}

export type StudioDomainGroupNode = Node<DomainGroupNodeData, "domainGroup">;

/**
 * Compound React Flow swimlane container for `NodeTypeSpec`s sharing a `domain_group`.
 */
export function DomainGroupNode({ data }: NodeProps<StudioDomainGroupNode>) {
  const { tx } = useI18n();
  const { domainGroup, memberCount, collapsed, width, height, onToggleCollapse } =
    data;

  return (
    <div
      data-testid={`domain-group-${domainGroup}`}
      style={{
        width: `${width}px`,
        height: `${height}px`,
      }}
      className={cn(
        "rounded-lg border border-xblue/30 bg-xblue-soft/20 backdrop-blur-xs transition-colors select-none",
        collapsed && "bg-card/95 shadow-xs border-xblue/45"
      )}
    >
      <Handle
        type="target"
        position={Position.Left}
        className="!h-2.5 !w-2.5 !border-2 !border-background !bg-xblue"
      />

      <div className="flex items-center justify-between gap-2 border-b border-xblue/20 bg-card/75 px-3 py-1.5 rounded-t-lg">
        <div className="flex items-center gap-1.5 min-w-0">
          <Layers className="h-3.5 w-3.5 text-xblue shrink-0" />
          <span className="truncate text-xs font-semibold text-foreground">
            {domainGroup}
          </span>
          <Badge variant="xblue" className="text-[10px] tabular-nums">
            {memberCount}
          </Badge>
        </div>

        <button
          type="button"
          data-testid={`domain-group-toggle-${domainGroup}`}
          onClick={(e) => {
            e.stopPropagation();
            onToggleCollapse(domainGroup);
          }}
          className="nodrag nopan inline-flex items-center gap-0.5 rounded border border-border bg-background px-1.5 py-0.5 text-[10px] font-medium text-foreground hover:border-xblue cursor-pointer shrink-0"
        >
          {collapsed ? (
            <>
              <ChevronRight className="h-3 w-3 text-xblue" />
              {tx("展开", "Expand")}
            </>
          ) : (
            <>
              <ChevronDown className="h-3 w-3 text-xblue" />
              {tx("折叠", "Collapse")}
            </>
          )}
        </button>
      </div>

      {collapsed && (
        <div className="px-3 py-2.5 text-[11px] text-muted-foreground font-mono-tech">
          {tx(`${memberCount} 项业务对象已收纳`, `${memberCount} object types collapsed`)}
        </div>
      )}

      <Handle
        type="source"
        position={Position.Right}
        className="!h-2.5 !w-2.5 !border-2 !border-background !bg-xblue"
      />
    </div>
  );
}
