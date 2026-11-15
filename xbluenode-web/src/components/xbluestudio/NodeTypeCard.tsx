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
import type { NodeTypeSpec } from "@/lib/api/xblue";
import { Badge, Button, Input, Textarea } from "@/components/ui/primitives";
import { cn } from "@/lib/utils";

export interface StudioNodeCardData extends Record<string, unknown> {
  spec: NodeTypeSpec;
  selected: boolean;
  issueCount: number;
  hasError: boolean;
}

export type StudioFlowNode = Node<StudioNodeCardData, "studioNode">;

const ICON_OPTIONS = [
  { key: "box", label: "通用实体 (box)" },
  { key: "warehouse", label: "仓储枢纽 (warehouse)" },
  { key: "factory", label: "制造工厂 (factory)" },
  { key: "shopping-cart", label: "交易订单 (shopping-cart)" },
  { key: "truck", label: "物流运单 (truck)" },
  { key: "database", label: "数据资产 (database)" },
  { key: "cpu", label: "设备算力 (cpu)" },
  { key: "users", label: "组织客户 (users)" },
] as const;

const COLOR_SWATCHES = [
  "#2563eb",
  "#09090b",
  "#1d4ed8",
  "#334155",
  "#0f766e",
  "#4f46e5",
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
export function StudioCanvasNodeCard({
  data,
}: NodeProps<StudioFlowNode>) {
  const { spec, selected, issueCount, hasError } = data;
  const attrCount = spec.properties.filter((p) => p.kind === "ATTRIBUTE").length;
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
          <Badge variant="neutral">属性 {attrCount}</Badge>
          <Badge variant="xblue">指标 {metricCount}</Badge>
          {ruleCount > 0 && <Badge variant="outline">规则 {ruleCount}</Badge>}
          {spec.sync_to_knowledge && (
            <Badge variant="outline" title="已开启知识超图同步">
              <BookOpen className="h-2.5 w-2.5 mr-0.5" />
              知识
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
                +{spec.properties.length - 3} 项字段...
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
  const attrNames = nodeType.properties
    .filter((p) => p.kind === "ATTRIBUTE" && p.name.trim().length > 0)
    .map((p) => p.name);

  const appendTemplateToken = (token: string) => {
    const cur = nodeType.title_template || "";
    const next = cur.includes(token) ? cur : `${cur}${cur ? " " : ""}${token}`;
    onChange({ ...nodeType, title_template: next });
  };

  return (
    <div className="space-y-3 text-xs" data-testid="node-type-card-editor">
      <div className="grid grid-cols-2 gap-2">
        <div>
          <label className="block font-medium mb-1">
            英文类型名 (PascalCase)
          </label>
          <Input
            data-testid="node-type-name-input"
            value={nodeType.name}
            onChange={(e) => onChange({ ...nodeType, name: e.target.value })}
            placeholder="如 Warehouse"
            className="h-8 text-xs font-mono-tech"
          />
        </div>
        <div>
          <label className="block font-medium mb-1">中文显示名</label>
          <Input
            data-testid="node-type-display-input"
            value={nodeType.display_name}
            onChange={(e) =>
              onChange({ ...nodeType, display_name: e.target.value })
            }
            placeholder="如 仓储中心"
            className="h-8 text-xs"
          />
        </div>
      </div>

      <div className="grid grid-cols-2 gap-2">
        <div>
          <label className="block font-medium mb-1">图标 (icon)</label>
          <select
            value={nodeType.icon || "box"}
            onChange={(e) => onChange({ ...nodeType, icon: e.target.value })}
            className="h-8 w-full rounded-md border border-input bg-background px-2 text-xs"
          >
            {ICON_OPTIONS.map((opt) => (
              <option key={opt.key} value={opt.key}>
                {opt.label}
              </option>
            ))}
          </select>
        </div>

        <div>
          <label className="block font-medium mb-1">色条标识 (color)</label>
          <div className="flex items-center gap-1.5">
            <div className="flex items-center gap-1">
              {COLOR_SWATCHES.map((hex) => (
                <button
                  key={hex}
                  type="button"
                  onClick={() => onChange({ ...nodeType, color: hex })}
                  className={cn(
                    "h-5 w-5 rounded border cursor-pointer",
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
              onChange={(e) => onChange({ ...nodeType, color: e.target.value })}
              className="h-8 text-xs font-mono-tech"
              placeholder="#2563eb"
            />
          </div>
        </div>
      </div>

      <div>
        <label className="block font-medium mb-1">
          节点标题模板 (title_template)
        </label>
        <Input
          value={nodeType.title_template}
          onChange={(e) =>
            onChange({ ...nodeType, title_template: e.target.value })
          }
          placeholder="{node_key}"
          className="h-8 text-xs font-mono-tech"
        />
        <div className="flex flex-wrap items-center gap-1 mt-1">
          <span className="text-[10px] text-muted-foreground">快速插入:</span>
          <button
            type="button"
            onClick={() => appendTemplateToken("{node_key}")}
            className="rounded border border-border bg-muted px-1.5 py-0.5 text-[10px] font-mono-tech hover:border-xblue cursor-pointer"
          >
            {"{node_key}"}
          </button>
          {attrNames.map((attr) => (
            <button
              key={attr}
              type="button"
              onClick={() => appendTemplateToken(`{prop.${attr}}`)}
              className="rounded border border-border bg-muted px-1.5 py-0.5 text-[10px] font-mono-tech hover:border-xblue cursor-pointer"
            >
              {`{prop.${attr}}`}
            </button>
          ))}
        </div>
      </div>

      <div>
        <label className="block font-medium mb-1">领域说明 (description)</label>
        <Textarea
          rows={2}
          value={nodeType.description}
          onChange={(e) =>
            onChange({ ...nodeType, description: e.target.value })
          }
          placeholder="描述该业务实体的边界、上游来源与下游消费场景..."
          className="text-xs"
        />
      </div>

      <div className="flex items-center justify-between rounded border border-border bg-muted/40 px-2.5 py-2">
        <div>
          <div className="font-medium">同步至知识超图 (sync_to_knowledge)</div>
          <div className="text-[11px] text-muted-foreground">
            启用后节点档案自动生成 Markdown 并写入超图供五路融合问答检索
          </div>
        </div>
        <input
          type="checkbox"
          data-testid="node-type-sync-knowledge"
          checked={nodeType.sync_to_knowledge}
          onChange={(e) =>
            onChange({ ...nodeType, sync_to_knowledge: e.target.checked })
          }
          className="h-4 w-4 cursor-pointer"
        />
      </div>

      <div className="flex items-center justify-between pt-2 border-t border-border">
        <span className="font-mono-tech text-[11px] text-muted-foreground">
          画布坐标: ({Math.round(nodeType.pos_x)}, {Math.round(nodeType.pos_y)})
        </span>
        <Button
          variant="outline"
          onClick={onDelete}
          className="h-7 px-2.5 py-1 text-red-600 dark:text-red-400 hover:bg-red-500/10"
        >
          <Trash2 className="h-3.5 w-3.5" />
          删除节点类型
        </Button>
      </div>
    </div>
  );
}
