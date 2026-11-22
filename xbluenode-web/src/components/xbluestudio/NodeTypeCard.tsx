import { Handle, Position, type Node, type NodeProps } from "@xyflow/react";
import {
  BookOpen,
  Box,
  Cpu,
  Database,
  Factory,
  ShoppingCart,
  Trash2,
  Truck,
  Users,
  Warehouse,
} from "lucide-react";
import { useI18n } from "@/i18n";
import { usePrincipal } from "@/lib/permissions";
import type { NodeTypeSpec } from "@/lib/api/xblue";
import {
  Badge,
  Button,
  Input,
  Select,
  Textarea,
} from "@/components/ui/primitives";
import { cn } from "@/lib/utils";

export interface StudioNodeCardData extends Record<string, unknown> {
  spec: NodeTypeSpec;
  selected: boolean;
  issueCount: number;
  hasError: boolean;
}

export type StudioFlowNode = Node<StudioNodeCardData, "studioNode">;

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

function renderNodeIcon(iconKey: string) {
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

/** Custom `@xyflow/react` canvas card renderer for a `NodeTypeSpec`. */
export function StudioCanvasNodeCard({ data }: NodeProps<StudioFlowNode>) {
  const { tx } = useI18n();
  const { spec, selected, issueCount, hasError } = data;
  const attrCount = spec.properties.filter(
    (p) => p.kind === "ATTRIBUTE"
  ).length;
  const metricCount = spec.properties.filter((p) => p.kind === "METRIC").length;
  const ruleCount = spec.agg_rules.length + spec.derived_rules.length;
  const accentColor = spec.color || "#2563eb";

  return (
    <div
      data-testid={`canvas-node-${spec.name}`}
      className={cn(
        "w-56 rounded-md border bg-card text-card-foreground shadow-sm transition-shadow select-none overflow-hidden",
        selected
          ? "border-xblue ring-2 ring-xblue/30 shadow-md"
          : "border-border hover:border-foreground/40",
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
              className="inline-flex h-6 w-6 items-center justify-center rounded border border-border bg-muted text-foreground shrink-0"
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

          {issueCount > 0 && (
            <span
              className={cn(
                "rounded px-1.5 py-0.5 font-mono-tech text-[10px] font-semibold",
                hasError
                  ? "bg-red-500/15 text-red-600 dark:text-red-400"
                  : "bg-amber-500/15 text-amber-600 dark:text-amber-400"
              )}
            >
              {issueCount}
            </span>
          )}
        </div>

        <div className="flex flex-wrap items-center gap-1">
          <Badge variant="neutral">
            {tx("属性", "Attr")} {attrCount}
          </Badge>
          <Badge variant="xblue">
            {tx("指标", "Metric")} {metricCount}
          </Badge>
          {ruleCount > 0 && (
            <Badge variant="outline">
              {tx("规则", "Rule")} {ruleCount}
            </Badge>
          )}
          {spec.sync_to_knowledge && (
            <Badge
              variant="outline"
              title={tx("已开启知识超图同步", "Knowledge sync enabled")}
            >
              <BookOpen className="h-2.5 w-2.5 mr-0.5" />
              {tx("知识", "KG")}
            </Badge>
          )}
        </div>

        {spec.properties.length > 0 && (
          <div className="border-t border-border pt-1.5 space-y-0.5">
            {spec.properties.slice(0, 3).map((prop) => (
              <div
                key={prop.name}
                className="flex items-center justify-between text-[10px] font-mono-tech"
              >
                <span className="truncate text-foreground/90">{prop.name}</span>
                <span className="text-muted-foreground">
                  {prop.kind === "METRIC" ? "M·" : ""}
                  {prop.data_type}
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
  onChange: (updated: NodeTypeSpec) => void;
  onDelete: () => void;
}

/** Sidebar basic metadata inspector for a selected `NodeTypeSpec`. */
export function NodeTypeCard({
  nodeType,
  onChange,
  onDelete,
}: NodeTypeCardProps) {
  const { tx } = useI18n();
  const { can } = usePrincipal();
  const canEdit = can("STUDIO_EDIT");

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

      <div className="space-y-2.5">
        <div>
          <label className="block font-medium mb-1">
            {tx("图标", "Icon")}
          </label>
          <Select
            value={nodeType.icon || "box"}
            disabled={!canEdit}
            onChange={(e) => onChange({ ...nodeType, icon: e.target.value })}
            className="h-8 w-full"
          >
            {ICON_OPTIONS.map((opt) => (
              <option key={opt.key} value={opt.key}>
                {tx(opt.zh, opt.en)} ({opt.key})
              </option>
            ))}
          </Select>
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

      <div className="flex items-center justify-between rounded border border-border bg-muted/40 px-2.5 py-2">
        <div>
          <div className="font-medium">
            {tx("同步至知识超图", "Sync to Knowledge Graph")}
          </div>
          <div className="text-[11px] text-muted-foreground">
            {tx(
              "自动生成节点知识档案供五路问答检索",
              "Generate hypergraph profile for hybrid QA"
            )}
          </div>
        </div>
        <input
          type="checkbox"
          data-testid="node-type-sync-knowledge"
          checked={nodeType.sync_to_knowledge}
          disabled={!canEdit}
          onChange={(e) =>
            onChange({ ...nodeType, sync_to_knowledge: e.target.checked })
          }
          className="h-4 w-4 cursor-pointer"
        />
      </div>

      <div className="flex items-center justify-between pt-2 border-t border-border">
        <span className="font-mono-tech text-[11px] text-muted-foreground">
          {tx("坐标", "Position")}: ({Math.round(nodeType.pos_x)},{" "}
          {Math.round(nodeType.pos_y)})
        </span>
        {canEdit && (
          <Button variant="danger" size="sm" onClick={onDelete}>
            <Trash2 className="h-3.5 w-3.5" />
            {tx("删除类型", "Delete")}
          </Button>
        )}
      </div>
    </div>
  );
}
