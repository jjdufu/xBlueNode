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
import { Input, StatText } from "@/components/ui/primitives";
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
    descZh: "触发数据集同步并等待快照落湖",
    descEn: "Trigger dataset ingestion into lakehouse snapshot",
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
    descZh: "将湖表数据按业务模型映射规则物化为节点与关系",
    descEn: "Materialize lake tables into nodes and links via model mappings",
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
      className="flex flex-col h-full min-h-0 rounded-md border border-border bg-card text-card-foreground overflow-hidden"
      data-testid="axflow-operator-palette"
    >
      <div className="p-2.5 border-b border-border space-y-2 shrink-0">
        <div className="flex items-center justify-between">
          <span className="text-xs font-semibold tracking-tight">
            {tx("算子组件库", "Operators")}
          </span>
          <StatText label="" value={operators.length} />
        </div>
        <div className="relative">
          <Search className="h-3.5 w-3.5 text-muted-foreground absolute left-2.5 top-2 pointer-events-none" />
          <Input
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            placeholder={tx("搜索算子…", "Search operators…")}
            className="h-7 pl-8 text-xs"
            data-testid="operator-palette-search"
          />
        </div>
      </div>

      {/* Compact Single-Row Draggable List (Long descriptions in native Tooltip) */}
      <div className="flex-1 min-h-0 overflow-y-auto p-1.5 pb-3 space-y-0.5">
        {filtered.map((spec) => {
          const title = resolveOperatorTitle(spec, isZh);
          const desc = resolveOperatorDesc(spec, isZh);
          const tip = desc ? `${title} (${spec.kind})\n${desc}` : `${title} (${spec.kind})`;
          return (
            <div
              key={spec.kind}
              title={tip}
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
                "group flex items-center justify-between gap-2 rounded-md px-2 py-1.5 transition-colors hover:bg-muted/70",
                canEdit && "cursor-grab active:cursor-grabbing"
              )}
            >
              <div className="flex items-center gap-2 min-w-0">
                <span className="inline-flex h-6 w-6 shrink-0 items-center justify-center rounded bg-xblue-soft/75 text-xblue">
                  {renderOperatorIcon(spec.kind)}
                </span>
                <div className="min-w-0">
                  <div className="truncate text-xs font-medium leading-tight text-foreground">
                    {title}
                  </div>
                </div>
              </div>

              {canEdit && (
                <button
                  type="button"
                  data-testid={`palette-add-${spec.kind}`}
                  onClick={() => onAddOperator(spec)}
                  title={tx(`添加 ${title}`, `Add ${title}`)}
                  aria-label={tx(`添加 ${title}`, `Add ${title}`)}
                  className="inline-flex h-6 w-6 shrink-0 items-center justify-center rounded text-muted-foreground hover:text-xblue hover:bg-xblue-soft/60 transition-colors cursor-pointer"
                >
                  <Plus className="h-3.5 w-3.5" />
                </button>
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
