import { useMemo, useState } from "react";
import {
  Bot,
  Calculator,
  Code2,
  Database,
  GitBranch,
  Globe,
  Network,
  Plus,
  Search,
  Send,
  Sparkles,
  UserCheck,
} from "lucide-react";
import type { FlowOperator } from "@/lib/api/enums";
import type { OperatorSpecDto } from "@/lib/api/axflow";
import { Badge, Button, Input } from "@/components/ui/primitives";

export function renderOperatorIcon(kind: FlowOperator, className = "h-3.5 w-3.5") {
  switch (kind) {
    case "SOURCE_INGEST":
      return <Database className={className} />;
    case "SQL_TRANSFORM":
      return <Code2 className={className} />;
    case "LLM_EXTRACT":
      return <Sparkles className={className} />;
    case "XBLUE_MAP":
      return <Network className={className} />;
    case "AGG_TRIGGER":
      return <Calculator className={className} />;
    case "CAUSAL_SIM":
      return <GitBranch className={className} />;
    case "HUMAN_APPROVAL":
      return <UserCheck className={className} />;
    case "ACTION_EMIT":
      return <Send className={className} />;
    case "HTTP_CALL":
      return <Globe className={className} />;
    case "AGENT_TASK":
      return <Bot className={className} />;
  }
}

export interface OperatorPaletteProps {
  operators: OperatorSpecDto[];
  onAddOperator: (spec: OperatorSpecDto) => void;
}

export function OperatorPalette({
  operators,
  onAddOperator,
}: OperatorPaletteProps) {
  const [search, setSearch] = useState("");

  const filtered = useMemo(() => {
    const q = search.trim().toLowerCase();
    if (!q) return operators;
    return operators.filter(
      (op) =>
        op.kind.toLowerCase().includes(q) ||
        op.display_name.toLowerCase().includes(q) ||
        op.description.toLowerCase().includes(q)
    );
  }, [operators, search]);

  return (
    <div
      className="flex flex-col h-[540px] rounded-md border border-border bg-card text-card-foreground overflow-hidden"
      data-testid="axflow-operator-palette"
    >
      <div className="p-3 border-b border-border space-y-2">
        <div className="flex items-center justify-between">
          <span className="text-xs font-semibold tracking-tight">
            算子组件库
          </span>
          <Badge variant="neutral">{operators.length}</Badge>
        </div>
        <div className="relative">
          <Search className="h-3.5 w-3.5 text-muted-foreground absolute left-2.5 top-2.5 pointer-events-none" />
          <Input
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            placeholder="搜索算子或编码..."
            className="h-8 pl-8 text-xs"
            data-testid="operator-palette-search"
          />
        </div>
      </div>

      <div className="flex-1 overflow-y-auto p-2.5 space-y-2">
        {filtered.map((spec) => (
          <div
            key={spec.kind}
            draggable
            onDragStart={(e) => {
              e.dataTransfer.setData("application/axflow-operator", spec.kind);
              e.dataTransfer.effectAllowed = "move";
            }}
            data-testid={`palette-item-${spec.kind}`}
            className="group rounded-md border border-border bg-background hover:border-xblue/60 p-2.5 transition-colors cursor-grab active:cursor-grabbing space-y-1.5"
          >
            <div className="flex items-start justify-between gap-1.5">
              <div className="flex items-center gap-1.5 min-w-0">
                <span className="inline-flex h-6 w-6 shrink-0 items-center justify-center rounded border border-border bg-muted text-xblue">
                  {renderOperatorIcon(spec.kind)}
                </span>
                <div className="min-w-0">
                  <div className="truncate text-xs font-medium leading-tight">
                    {spec.display_name.replace(/\s*\([A-Z_]+\)\s*$/, "")}
                  </div>
                  <div className="truncate font-mono-tech text-[10px] text-muted-foreground">
                    {spec.kind}
                  </div>
                </div>
              </div>
              <Button
                type="button"
                variant="outline"
                className="h-6 px-2 py-0 text-[11px] shrink-0"
                data-testid={`palette-add-${spec.kind}`}
                onClick={() => onAddOperator(spec)}
                title={`添加 ${spec.kind} 到画布`}
              >
                <Plus className="h-3 w-3" />
                添加算子
              </Button>
            </div>
            {spec.description && (
              <p className="text-[11px] text-muted-foreground leading-snug line-clamp-2">
                {spec.description}
              </p>
            )}
          </div>
        ))}

        {filtered.length === 0 && (
          <div className="py-8 text-center text-xs text-muted-foreground">
            未找到匹配算子
          </div>
        )}
      </div>

      <div className="px-3 py-2 border-t border-border bg-muted/30 text-[10px] text-muted-foreground">
        支持拖入画布或点击「添加算子」快速编排
      </div>
    </div>
  );
}
