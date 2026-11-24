import { useState } from "react";
import { Handle, Position, type Node, type NodeProps } from "@xyflow/react";
import {
  BookOpen,
  Box,
  Check,
  Cpu,
  Database,
  Factory,
  GitBranch,
  ShoppingCart,
  Trash2,
  Truck,
  Users,
  Warehouse,
} from "lucide-react";
import { useI18n } from "@/i18n";
import { usePrincipal } from "@/lib/permissions";
import type { SemanticLayerTier } from "@/lib/api/enums";
import {
  computeNodeBadgeCounts,
  hasProposedElements,
  type NodeConsumptionCounts,
  type NodeTypeSpec,
} from "@/lib/api/xblue";
import {
  Badge,
  Button,
  Input,
  Select,
  Switch,
  Textarea,
} from "@/components/ui/primitives";
import { cn } from "@/lib/utils";

export interface StudioNodeCardData extends Record<string, unknown> {
  spec: NodeTypeSpec;
  selected: boolean;
  issueCount: number;
  hasError: boolean;
  consumption?: NodeConsumptionCounts | null;
  onConfirmNode?: (nodeName: string) => void;
  onDoubleClickNode?: (nodeName: string) => void;
  onOpenAppPackaging?: (nodeName: string) => void;
}

export type StudioFlowNode = Node<StudioNodeCardData, "studioNode">;

const SEMANTIC_TIER_OPTIONS: readonly SemanticLayerTier[] = [
  "STATIC_FACT",
  "EVENT_DYNAMIC",
  "CUBE_ROLLUP",
];

const ICON_OPTIONS = [
  { key: "box", zh: "通用实体", en: "Generic Object" },
  { key: "warehouse", zh: "仓储枢纽", en: "Warehouse" },
  { key: "factory", zh: "制造工厂", en: "Factory" },
  { key: "shopping-cart", zh: "履约订单", en: "Order" },
  { key: "truck", zh: "物流运单", en: "Shipment" },
  { key: "database", zh: "数据资产", en: "Dataset" },
  { key: "cpu", zh: "设备算力", en: "Compute" },
  { key: "users", zh: "组织客户", en: "Organization" },
] as const;

const COLOR_SWATCHES = [
  "#2563eb",
  "#059669",
  "#d97706",
  "#7c3aed",
  "#db2777",
  "#0f766e",
] as const;

export function renderNodeIcon(iconKey: string) {
  const cls = "h-3.5 w-3.5 shrink-0";
  switch (iconKey) {
    case "warehouse":
      return <Warehouse className={cls} />;
    case "factory":
      return <Factory className={cls} />;
    case "shopping-cart":
      return <ShoppingCart className={cls} />;
    case "truck":
      return <Truck className={cls} />;
    case "database":
      return <Database className={cls} />;
    case "cpu":
      return <Cpu className={cls} />;
    case "users":
      return <Users className={cls} />;
    default:
      return <Box className={cls} />;
  }
}

function formatNodeBadgeSummary(
  counts: {
    readonly property_count: number;
    readonly metric_count: number;
    readonly rule_count: number;
  },
  isZh: boolean
): string {
  if (isZh) {
    const base = `${counts.property_count} 属性 · ${counts.metric_count} 指标`;
    return counts.rule_count > 0 ? `${base} · ${counts.rule_count} 规则` : base;
  }
  const base = `${counts.property_count} Attr · ${counts.metric_count} Metric`;
  return counts.rule_count > 0 ? `${base} · ${counts.rule_count} Rule` : base;
}

function formatFieldTypeCompact(
  kind: string | undefined,
  dataType: string,
  isZh: boolean
): string {
  const upper = dataType.toUpperCase();
  let baseLabel: string;
  switch (upper) {
    case "STRING":
    case "TEXT":
      baseLabel = isZh ? "文本" : "Text";
      break;
    case "FLOAT64":
    case "FLOAT":
    case "DOUBLE":
    case "NUMBER":
      baseLabel = isZh ? "浮点" : "Float";
      break;
    case "INT64":
    case "INT":
    case "INTEGER":
      baseLabel = isZh ? "整数" : "Int";
      break;
    case "DECIMAL":
      baseLabel = isZh ? "金额" : "Decimal";
      break;
    case "BOOLEAN":
    case "BOOL":
      baseLabel = isZh ? "布尔" : "Bool";
      break;
    case "DATE":
      baseLabel = isZh ? "日期" : "Date";
      break;
    case "TIMESTAMP":
    case "DATETIME":
      baseLabel = isZh ? "时间" : "Timestamp";
      break;
    case "JSON":
      baseLabel = isZh ? "对象" : "JSON";
      break;
    case "LIST":
      baseLabel = isZh ? "列表" : "List";
      break;
    case "STRUCT":
      baseLabel = isZh ? "结构" : "Struct";
      break;
    default:
      baseLabel = dataType;
      break;
  }
  if (kind === "METRIC") {
    return isZh ? `指标·${baseLabel}` : `Metric·${baseLabel}`;
  }
  return baseLabel;
}

/** Custom `@xyflow/react` canvas card renderer for a `NodeTypeSpec`. */
export function StudioCanvasNodeCard({ data }: NodeProps<StudioFlowNode>) {
  const { isZh, tx, tEnum } = useI18n();
  const { can } = usePrincipal();
  const canEdit = can("STUDIO_EDIT");
  const {
    spec,
    selected,
    issueCount,
    hasError,
    consumption,
    onConfirmNode,
    onDoubleClickNode,
    onOpenAppPackaging,
  } = data;
  const counts = computeNodeBadgeCounts(spec);
  const isProposed = hasProposedElements(spec);
  const accentColor = spec.color || "#2563eb";
  const totalDownstream =
    (consumption?.endpoint_count ?? 0) +
    (consumption?.agent_count ?? 0) +
    (consumption?.dashboard_count ?? 0);

  return (
    <div
      data-testid={`canvas-node-${spec.name}`}
      onDoubleClick={(e) => {
        if (onDoubleClickNode) {
          e.stopPropagation();
          onDoubleClickNode(spec.name);
        }
      }}
      className={cn(
        "w-60 rounded-md border bg-card text-card-foreground shadow-sm transition-shadow select-none overflow-hidden",
        selected
          ? "border-xblue ring-2 ring-xblue/30 shadow-md"
          : "border-border hover:border-foreground/40",
        isProposed && "border-dashed border-amber-500/70",
        hasError && "border-red-500"
      )}
    >
      <div className="h-1.5 w-full" style={{ backgroundColor: accentColor }} />
      <Handle
        type="target"
        position={Position.Left}
        className="!h-2.5 !w-2.5 !border-2 !border-background !bg-foreground"
      />

      <div className="p-2.5 space-y-2">
        <div className="flex items-start justify-between gap-1.5">
          <div className="flex items-center gap-1.5 min-w-0">
            <span
              className="inline-flex h-6 w-6 items-center justify-center rounded bg-muted text-foreground shrink-0"
              style={{ color: accentColor }}
            >
              {renderNodeIcon(spec.icon)}
            </span>
            <div className="min-w-0">
              <div className="truncate text-xs font-semibold leading-tight">
                {spec.display_name || spec.name}
              </div>
              <div className="truncate font-mono-tech text-[11px] text-muted-foreground">
                {spec.name}
              </div>
            </div>
          </div>

          <div className="flex items-center gap-1 shrink-0">
            <Badge
              variant="neutral"
              data-testid={`canvas-node-badge-${spec.name}`}
              className="tabular-nums text-[10px] px-1.5 py-0.5"
            >
              {formatNodeBadgeSummary(counts, isZh)}
            </Badge>
            {issueCount > 0 && (
              <span
                className={cn(
                  "rounded px-1.5 py-0.5 font-mono-tech text-[10px] font-semibold tabular-nums",
                  hasError
                    ? "bg-red-500/15 text-red-600 dark:text-red-400"
                    : "bg-amber-500/15 text-amber-600 dark:text-amber-400"
                )}
              >
                {issueCount}
              </span>
            )}
          </div>
        </div>

        {(spec.semantic_layer_tier ||
          spec.domain_group ||
          isProposed ||
          spec.sync_to_knowledge) && (
          <div className="flex flex-wrap items-center justify-between gap-1">
            <div className="flex flex-wrap items-center gap-1 min-w-0">
              {spec.semantic_layer_tier && (
                <Badge
                  variant="xblue"
                  data-testid={`canvas-node-tier-${spec.name}`}
                  className="text-[10px]"
                >
                  {tEnum(spec.semantic_layer_tier)}
                </Badge>
              )}
              {spec.domain_group && (
                <Badge variant="outline" className="text-[10px]">
                  {spec.domain_group}
                </Badge>
              )}
              {isProposed && (
                <Badge
                  variant="warning"
                  data-testid={`canvas-node-proposed-${spec.name}`}
                  className="text-[10px]"
                >
                  {tx("待确认", "Proposed")}
                </Badge>
              )}
              {spec.sync_to_knowledge && (
                <Badge
                  variant="outline"
                  title={tx("已开启知识超图同步", "Knowledge sync enabled")}
                >
                  <BookOpen className="h-2.5 w-2.5 mr-0.5 text-xblue" />
                  {tx("超图", "KG")}
                </Badge>
              )}
            </div>

            {isProposed && canEdit && onConfirmNode && (
              <button
                type="button"
                data-testid={`confirm-node-btn-${spec.name}`}
                onClick={(e) => {
                  e.stopPropagation();
                  onConfirmNode(spec.name);
                }}
                onDoubleClick={(e) => e.stopPropagation()}
                className="nodrag nopan inline-flex items-center gap-0.5 rounded border border-amber-500/50 bg-amber-500/15 px-1.5 py-0.5 text-[10px] font-medium text-amber-700 dark:text-amber-300 hover:bg-amber-500/25 cursor-pointer shrink-0"
              >
                <Check className="h-2.5 w-2.5" />
                {tx("确认", "Confirm")}
              </button>
            )}
          </div>
        )}

        {spec.properties.length > 0 && (
          <div className="border-t border-border pt-1.5 space-y-0.5">
            {spec.properties.slice(0, 3).map((prop) => (
              <div
                key={prop.name}
                className="flex items-center justify-between gap-1 text-[10px] font-mono-tech"
              >
                <span className="truncate text-foreground/90 flex items-center gap-1">
                  <span className="truncate">{prop.name}</span>
                  {prop.maturity === "PROPOSED" && (
                    <span className="h-1.5 w-1.5 rounded-full bg-amber-500 shrink-0" />
                  )}
                </span>
                <span
                  className="font-sans text-muted-foreground shrink-0"
                  title={`${prop.kind ?? "ATTRIBUTE"} · ${prop.data_type}`}
                >
                  {formatFieldTypeCompact(prop.kind, prop.data_type, isZh)}
                </span>
              </div>
            ))}
            {spec.properties.length > 3 && (
              <div className="text-[10px] text-muted-foreground">
                +{spec.properties.length - 3} {tx("项字段", "fields")}
              </div>
            )}
          </div>
        )}

        {totalDownstream > 0 && (
          <div className="border-t border-border pt-1.5 flex items-center">
            <button
              type="button"
              data-testid={`node-consumption-badge-${spec.name}`}
              onClick={(e) => {
                e.stopPropagation();
                onOpenAppPackaging?.(spec.name);
              }}
              onDoubleClick={(e) => e.stopPropagation()}
              className="nodrag nopan inline-flex items-center gap-1 rounded-full bg-muted/80 hover:bg-xblue-soft hover:text-xblue px-2 py-0.5 text-[10px] font-mono-tech text-muted-foreground transition-colors cursor-pointer"
            >
              {tx(
                `${consumption?.agent_count ?? 0} 智能体 · ${consumption?.dashboard_count ?? 0} 看板`,
                `${consumption?.agent_count ?? 0} Agent · ${consumption?.dashboard_count ?? 0} View`
              )}
            </button>
          </div>
        )}
      </div>

      <Handle
        type="source"
        position={Position.Right}
        className="!h-2.5 !w-2.5 !border-2 !border-background !bg-xblue"
      />
    </div>
  );
}

export interface NodeTypeCardProps {
  nodeType: NodeTypeSpec;
  consumption?: NodeConsumptionCounts | null;
  onChange: (updated: NodeTypeSpec) => void;
  onDelete: () => void;
  onConfirmNode?: (nodeName: string) => void;
  onOpenLineage?: (nodeName: string) => void;
  onOpenAppPackaging?: (nodeName: string) => void;
}

/** Sidebar basic metadata inspector for a selected `NodeTypeSpec`. */
export function NodeTypeCard({
  nodeType,
  consumption,
  onChange,
  onDelete,
  onConfirmNode,
  onOpenLineage,
  onOpenAppPackaging,
}: NodeTypeCardProps) {
  const { isZh, tx, tEnum } = useI18n();
  const { can } = usePrincipal();
  const canEdit = can("STUDIO_EDIT");
  const [confirmDelete, setConfirmDelete] = useState(false);

  const counts = computeNodeBadgeCounts(nodeType);
  const isProposed = hasProposedElements(nodeType);
  const totalDownstream =
    (consumption?.endpoint_count ?? 0) +
    (consumption?.agent_count ?? 0) +
    (consumption?.dashboard_count ?? 0);

  const attrNames = nodeType.properties
    .filter((p) => p.kind === "ATTRIBUTE" && p.name.trim().length > 0)
    .map((p) => p.name);

  const appendTemplateToken = (token: string) => {
    if (!canEdit) return;
    const cur = nodeType.title_template || "";
    const next = cur.includes(token) ? cur : `${cur}${cur ? " " : ""}${token}`;
    onChange({ ...nodeType, title_template: next });
  };

  return (
    <div className="space-y-3 text-xs" data-testid="node-type-card-editor">
      <div className="flex flex-wrap items-center justify-between gap-1.5 rounded-md border border-border bg-muted/25 px-2.5 py-1.5">
        <div className="flex flex-wrap items-center gap-1.5">
          <Badge
            variant="neutral"
            data-testid="inspector-node-badge-counts"
            className="tabular-nums"
          >
            {formatNodeBadgeSummary(counts, isZh)}
          </Badge>
          {nodeType.semantic_layer_tier && (
            <Badge variant="xblue">{tEnum(nodeType.semantic_layer_tier)}</Badge>
          )}
          {isProposed && (
            <Badge variant="warning" data-testid="inspector-node-proposed-badge">
              {tx("待确认", "Proposed")}
            </Badge>
          )}
        </div>

        <div className="flex items-center gap-1.5">
          {isProposed && canEdit && onConfirmNode && (
            <Button
              type="button"
              variant="outline"
              size="sm"
              data-testid="inspector-confirm-node-btn"
              onClick={() => onConfirmNode(nodeType.name)}
              className="h-6 px-2 border-amber-500/50 text-amber-700 dark:text-amber-300"
            >
              <Check className="h-3 w-3" />
              {tx("确认", "Confirm")}
            </Button>
          )}
          {onOpenLineage && nodeType.name.trim().length > 0 && (
            <Button
              type="button"
              variant="outline"
              size="sm"
              data-testid="inspector-open-lineage-btn"
              onClick={() => onOpenLineage(nodeType.name)}
              className="h-6 px-2"
            >
              <GitBranch className="h-3 w-3 text-xblue" />
              {tx("血缘视图", "Lineage")}
            </Button>
          )}
        </div>
      </div>

      <div className="grid grid-cols-2 gap-2.5">
        <div>
          <label className="block font-medium mb-1">
            {tx("类型标识", "Type Key")}
          </label>
          <Input
            data-testid="node-type-name-input"
            value={nodeType.name}
            disabled={!canEdit}
            onChange={(e) => onChange({ ...nodeType, name: e.target.value })}
            placeholder="Warehouse"
            className="h-8 text-xs font-mono-tech"
          />
        </div>
        <div>
          <label className="block font-medium mb-1">
            {tx("显示名称", "Display Name")}
          </label>
          <Input
            data-testid="node-type-display-input"
            value={nodeType.display_name}
            disabled={!canEdit}
            onChange={(e) =>
              onChange({ ...nodeType, display_name: e.target.value })
            }
            placeholder={tx("仓储枢纽", "Warehouse Hub")}
            className="h-8 text-xs"
          />
        </div>
      </div>

      <div className="grid grid-cols-2 gap-2.5">
        <div>
          <label className="block font-medium mb-1">
            {tx("语义分层", "Semantic Tier")}
          </label>
          <Select
            data-testid="node-type-tier-select"
            value={nodeType.semantic_layer_tier ?? ""}
            disabled={!canEdit}
            onChange={(e) => {
              const raw = e.target.value;
              const nextTier: SemanticLayerTier | null =
                raw === "STATIC_FACT" ||
                raw === "EVENT_DYNAMIC" ||
                raw === "CUBE_ROLLUP"
                  ? raw
                  : null;
              onChange({ ...nodeType, semantic_layer_tier: nextTier });
            }}
            className="h-8 w-full"
          >
            <option value="">{tx("未分层", "Unassigned")}</option>
            {SEMANTIC_TIER_OPTIONS.map((tier) => (
              <option key={tier} value={tier}>
                {tEnum(tier)}
              </option>
            ))}
          </Select>
        </div>
        <div>
          <label className="block font-medium mb-1">
            {tx("业务域", "Domain Group")}
          </label>
          <Input
            data-testid="node-type-domain-group-input"
            value={nodeType.domain_group ?? ""}
            disabled={!canEdit}
            onChange={(e) =>
              onChange({
                ...nodeType,
                domain_group: e.target.value || null,
              })
            }
            placeholder={tx("履约域", "Fulfillment")}
            className="h-8 text-xs"
          />
        </div>
      </div>

      <div className="space-y-2.5">
        <div>
          <label className="block font-medium mb-1">
            {tx("图标", "Icon")}
          </label>
          <Select
            value={nodeType.icon || "box"}
            disabled={!canEdit}
            onChange={(e) => onChange({ ...nodeType, icon: e.target.value })}
            options={ICON_OPTIONS.map((opt) => ({
              value: opt.key,
              label: tx(opt.zh, opt.en),
              text: `${opt.zh} ${opt.en} ${opt.key}`,
              icon: renderNodeIcon(opt.key),
            }))}
            className="h-8 w-full"
          />
        </div>

        <div>
          <label className="block font-medium mb-1">
            {tx("色条标识", "Accent Color")}
          </label>
          <div className="flex items-center justify-between gap-2">
            <div className="flex items-center gap-1.5 shrink-0">
              {COLOR_SWATCHES.map((hex) => (
                <button
                  key={hex}
                  type="button"
                  disabled={!canEdit}
                  onClick={() => onChange({ ...nodeType, color: hex })}
                  className={cn(
                    "h-6 w-6 rounded border cursor-pointer disabled:opacity-50",
                    nodeType.color === hex
                      ? "ring-2 ring-foreground border-background"
                      : "border-border"
                  )}
                  style={{ backgroundColor: hex }}
                  title={hex}
                />
              ))}
            </div>
            <Input
              value={nodeType.color}
              disabled={!canEdit}
              onChange={(e) => onChange({ ...nodeType, color: e.target.value })}
              className="h-8 w-28 min-w-[104px] shrink-0 px-2.5 text-xs font-mono-tech"
              placeholder="#2563eb"
            />
          </div>
        </div>
      </div>

      <div>
        <label className="block font-medium mb-1">
          {tx("节点标题模板", "Title Template")}
        </label>
        <Input
          value={nodeType.title_template}
          disabled={!canEdit}
          onChange={(e) =>
            onChange({ ...nodeType, title_template: e.target.value })
          }
          placeholder="{node_key}"
          className="h-8 text-xs font-mono-tech"
        />
        <div className="flex flex-wrap items-center gap-1 mt-1">
          <span className="text-[10px] text-muted-foreground">
            {tx("快捷变量:", "Tokens:")}
          </span>
          <button
            type="button"
            disabled={!canEdit}
            onClick={() => appendTemplateToken("{node_key}")}
            className="rounded border border-border bg-muted px-1.5 py-0.5 text-[10px] font-mono-tech hover:border-xblue cursor-pointer disabled:opacity-50"
          >
            {"{node_key}"}
          </button>
          {attrNames.map((attr) => (
            <button
              key={attr}
              type="button"
              disabled={!canEdit}
              onClick={() => appendTemplateToken(`{prop.${attr}}`)}
              className="rounded border border-border bg-muted px-1.5 py-0.5 text-[10px] font-mono-tech hover:border-xblue cursor-pointer disabled:opacity-50"
            >
              {`{prop.${attr}}`}
            </button>
          ))}
        </div>
      </div>

      <div>
        <label className="block font-medium mb-1">
          {tx("领域说明", "Description")}
        </label>
        <Textarea
          rows={2}
          value={nodeType.description}
          disabled={!canEdit}
          onChange={(e) =>
            onChange({ ...nodeType, description: e.target.value })
          }
          placeholder={tx("区域履约仓储中心", "Regional fulfillment hub")}
          className="text-xs"
        />
      </div>

      <div className="flex items-center justify-between gap-3 rounded-md bg-muted/40 px-2.5 py-2">
        <div className="min-w-0">
          <div className="font-medium">
            {tx("同步至知识超图", "Sync to Knowledge Graph")}
          </div>
          <div className="text-[11px] text-muted-foreground">
            {tx(
              "生成节点知识档案供混合问答检索",
              "Generate hypergraph profile for hybrid QA"
            )}
          </div>
        </div>
        <Switch
          data-testid="node-type-sync-knowledge"
          checked={nodeType.sync_to_knowledge}
          disabled={!canEdit}
          onCheckedChange={(checked) =>
            onChange({ ...nodeType, sync_to_knowledge: checked })
          }
          aria-label={tx("同步至知识超图", "Sync to Knowledge Graph")}
        />
      </div>

      {totalDownstream > 0 && (
        <div className="flex items-center justify-between gap-2 pt-1">
          <button
            type="button"
            data-testid="inspector-node-consumption-badge"
            onClick={() => onOpenAppPackaging?.(nodeType.name)}
            className="inline-flex items-center gap-1 rounded-full bg-muted/80 hover:bg-xblue-soft hover:text-xblue px-2.5 py-0.5 text-[11px] font-mono-tech text-muted-foreground transition-colors cursor-pointer"
          >
            {tx(
              `${consumption?.agent_count ?? 0} 智能体 · ${consumption?.dashboard_count ?? 0} 看板`,
              `${consumption?.agent_count ?? 0} Agent · ${consumption?.dashboard_count ?? 0} View`
            )}
          </button>
        </div>
      )}

      {canEdit && (
        <div className="flex items-center justify-end gap-1.5 pt-2 border-t border-border">
          {confirmDelete ? (
            <>
              <span className="text-[11px] text-red-600 dark:text-red-400 mr-auto">
                {tx(
                  "删除节点类型将同步移除关联边定义",
                  "Deleting removes connected relation specs"
                )}
              </span>
              <Button
                variant="ghost"
                size="sm"
                onClick={() => setConfirmDelete(false)}
              >
                {tx("取消", "Cancel")}
              </Button>
              <Button
                variant="danger"
                size="sm"
                onClick={() => {
                  setConfirmDelete(false);
                  onDelete();
                }}
              >
                {tx("删除", "Delete")}
              </Button>
            </>
          ) : (
            <Button
              variant="outline"
              size="sm"
              onClick={() => setConfirmDelete(true)}
              className="text-red-600 hover:bg-red-500/10 hover:border-red-500/40 dark:text-red-400"
            >
              <Trash2 className="h-3.5 w-3.5" />
              {tx("删除类型", "Delete Type")}
            </Button>
          )}
        </div>
      )}
    </div>
  );
}
