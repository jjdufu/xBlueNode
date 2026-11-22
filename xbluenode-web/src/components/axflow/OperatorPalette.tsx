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
import { useI18n } from "@/i18n";
import { usePrincipal } from "@/lib/permissions";
import type { FlowOperator } from "@/lib/api/enums";
import type { OperatorSpecDto } from "@/lib/api/axflow";
import { Badge, Button, Input } from "@/components/ui/primitives";
import { cn } from "@/lib/utils";

export const OPERATOR_I18N: Readonly<
  Record<
    FlowOperator,
    {
      readonly zh: string;
      readonly en: string;
      readonly descZh: string;
      readonly descEn: string;
    }
  >
> = {
  SOURCE_INGEST: {
    zh: "数据源同步",
    en: "Source Sync",
    descZh: "触发 AxHub 数据集同步并等待快照落湖",
    descEn: "Trigger AxHub dataset ingestion into lakehouse snapshot",
  },
  SQL_TRANSFORM: {
    zh: "SQL 变换",
    en: "SQL Transform",
    descZh: "在受保护 DuckDB 沙箱执行只读变换或核验查询",
    descEn: "Execute guarded DuckDB transform or quality query",
  },
  LLM_EXTRACT: {
    zh: "大模型抽取",
    en: "LLM Extract",
    descZh: "按结构化 Schema 从非结构化文本抽取字段",
    descEn: "Extract structured fields from text via LLM schema",
  },
  XBLUE_MAP: {
    zh: "图谱节点映射",
    en: "Graph Mapping",
    descZh: "执行已发布映射规则物化业务节点与关系边",
    descEn: "Materialize lakehouse rows into graph nodes and edges",
  },
  AGG_TRIGGER: {
    zh: "动态指标重算",
    en: "Aggregate Rollup",
    descZh: "按拓扑序重算指定节点类型的跨边聚合与派生指标",
    descEn: "Recompute cross-edge rollups and derived node metrics",
  },
  CAUSAL_SIM: {
    zh: "因果沙盘推演",
    en: "Causal Simulation",
    descZh: "在推演沙盘运行反事实干预并输出受影响节点",
    descEn: "Run counterfactual simulation and output top impacts",
  },
  HUMAN_APPROVAL: {
    zh: "人工审批门禁",
    en: "Human Approval",
    descZh: "挂起当前分支等待指定角色或人员审批裁决",
    descEn: "Pause branch execution until assigned reviewer decides",
  },
  ACTION_EMIT: {
    zh: "业务动作回写",
    en: "Action Writeback",
    descZh: "向图谱指标或外部系统幂等下发回写动作",
    descEn: "Emit idempotent metric writebacks or operational actions",
  },
  HTTP_CALL: {
    zh: "HTTP 外部调用",
    en: "HTTP Request",
    descZh: "调用受 SSRF 守卫保护的外部业务 REST 接口",
    descEn: "Invoke SSRF-guarded external HTTP endpoint",
  },
  AGENT_TASK: {
    zh: "智能体协同任务",
    en: "Agent Task",
    descZh: "委派已配置的领域智能体执行分析推理任务",
    descEn: "Delegate domain analysis task to configured AI agent",
  },
};

export function resolveOperatorTitle(
  spec: OperatorSpecDto,
  isZh: boolean
): string {
  const entry = OPERATOR_I18N[spec.kind];
  if (!isZh && entry) {
    return entry.en;
  }
  const cleaned = spec.display_name.replace(/\s*\([A-Z_]+\)\s*$/, "").trim();
  return cleaned || entry?.zh || spec.kind;
}

export function resolveOperatorDesc(
  spec: OperatorSpecDto,
  isZh: boolean
): string {
  const entry = OPERATOR_I18N[spec.kind];
  if (!isZh && entry) {
    return entry.descEn;
  }
  return spec.description || entry?.descZh || "";
}

export interface DraggingOperatorMeta {
  readonly kind: FlowOperator;
  readonly label: string;
}

let activeDraggedOperator: DraggingOperatorMeta | null = null;

export function getActiveDraggedOperator(): DraggingOperatorMeta | null {
  return activeDraggedOperator;
}

export function setActiveDraggedOperator(
  meta: DraggingOperatorMeta | null
): void {
  activeDraggedOperator = meta;
}

export function clearActiveDraggedOperator(): void {
  activeDraggedOperator = null;
}

export function renderOperatorIcon(
  kind: FlowOperator,
  className = "h-3.5 w-3.5"
) {
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
  const { isZh, tx } = useI18n();
  const { can } = usePrincipal();
  const canEdit = can("FLOW_EDIT");

  const [search, setSearch] = useState("");

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
      className="flex flex-col h-[540px] rounded-md border border-border bg-card text-card-foreground overflow-hidden"
      data-testid="axflow-operator-palette"
    >
      <div className="p-3 border-b border-border space-y-2 shrink-0">
        <div className="flex items-center justify-between">
          <span className="text-xs font-semibold tracking-tight">
            {tx("算子组件库", "Operator Library")}
          </span>
          <Badge variant="neutral">{operators.length}</Badge>
        </div>
        <div className="relative">
          <Search className="h-3.5 w-3.5 text-muted-foreground absolute left-2.5 top-2.5 pointer-events-none" />
          <Input
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            placeholder="SQL_TRANSFORM"
            className="h-8 pl-8 text-xs"
            data-testid="operator-palette-search"
          />
        </div>
      </div>

      <div className="flex-1 overflow-y-auto p-2.5 space-y-2">
        {filtered.map((spec) => {
          const title = resolveOperatorTitle(spec, isZh);
          const desc = resolveOperatorDesc(spec, isZh);
          return (
            <div
              key={spec.kind}
              draggable={canEdit}
              onDragStart={(e) => {
                if (!canEdit) return;
                setActiveDraggedOperator({ kind: spec.kind, label: title });
                e.dataTransfer.setData(
                  "application/axflow-operator",
                  spec.kind
                );
                e.dataTransfer.effectAllowed = "move";
              }}
              onDragEnd={() => {
                clearActiveDraggedOperator();
              }}
              data-testid={`palette-item-${spec.kind}`}
              className={cn(
                "group rounded-md border border-border bg-background p-2.5 transition-colors space-y-1.5",
                canEdit &&
                  "hover:border-xblue/60 cursor-grab active:cursor-grabbing"
              )}
            >
              <div className="flex items-start justify-between gap-1.5">
                <div className="flex items-center gap-1.5 min-w-0">
                  <span className="inline-flex h-6 w-6 shrink-0 items-center justify-center rounded border border-border bg-muted text-xblue">
                    {renderOperatorIcon(spec.kind)}
                  </span>
                  <div className="min-w-0">
                    <div className="truncate text-xs font-medium leading-tight">
                      {title}
                    </div>
                    <div className="truncate font-mono-tech text-[10px] text-muted-foreground">
                      {spec.kind}
                    </div>
                  </div>
                </div>
                {canEdit && (
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    className="h-6 px-2 text-[11px]"
                    data-testid={`palette-add-${spec.kind}`}
                    onClick={() => onAddOperator(spec)}
                    title={tx(`添加 ${title}`, `Add ${title}`)}
                  >
                    <Plus className="h-3 w-3" />
                    {tx("添加", "Add")}
                  </Button>
                )}
              </div>
              {desc && (
                <p className="text-[11px] text-muted-foreground leading-snug line-clamp-2">
                  {desc}
                </p>
              )}
            </div>
          );
        })}

        {filtered.length === 0 && (
          <div className="py-8 text-center text-xs text-muted-foreground">
            {tx("无匹配算子", "No matching operators")}
          </div>
        )}
      </div>
    </div>
  );
}
