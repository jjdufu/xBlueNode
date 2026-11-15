import { useEffect, useState } from "react";
import { ArrowRight, Trash2 } from "lucide-react";
import type { Cardinality } from "@/lib/api/enums";
import type { EdgeTypeSpec, NodeTypeSpec } from "@/lib/api/xblue";
import { Badge, Button, Input, Textarea } from "@/components/ui/primitives";

export interface EdgeTypeEditorProps {
  edgeType: EdgeTypeSpec;
  availableNodeTypes: NodeTypeSpec[];
  onChange: (updated: EdgeTypeSpec) => void;
  onDelete: () => void;
}

const CARDINALITY_OPTIONS: readonly { value: Cardinality; label: string }[] = [
  { value: "ONE_TO_ONE", label: "1:1 (ONE_TO_ONE)" },
  { value: "ONE_TO_MANY", label: "1:N (ONE_TO_MANY)" },
  { value: "MANY_TO_ONE", label: "N:1 (MANY_TO_ONE)" },
  { value: "MANY_TO_MANY", label: "M:N (MANY_TO_MANY)" },
];

export function EdgeTypeEditor({
  edgeType,
  availableNodeTypes,
  onChange,
  onDelete,
}: EdgeTypeEditorProps) {
  const [propagationText, setPropagationText] = useState(() =>
    JSON.stringify(edgeType.propagation ?? {}, null, 2)
  );
  const [jsonError, setJsonError] = useState<string | null>(null);

  useEffect(() => {
    setPropagationText(JSON.stringify(edgeType.propagation ?? {}, null, 2));
    setJsonError(null);
  }, [edgeType.name, edgeType.src_type_name, edgeType.dst_type_name]);

  const handlePropagationChange = (raw: string) => {
    setPropagationText(raw);
    try {
      const parsed: unknown = JSON.parse(raw);
      if (
        typeof parsed !== "object" ||
        parsed === null ||
        Array.isArray(parsed)
      ) {
        setJsonError("传导配置必须为 JSON 对象 {...}");
        return;
      }
      setJsonError(null);
      onChange({
        ...edgeType,
        propagation: parsed as Record<string, unknown>,
      });
    } catch {
      setJsonError("JSON 语法未闭合或格式有误");
    }
  };

  return (
    <div className="space-y-3 text-xs" data-testid="edge-type-editor">
      <div className="flex items-center justify-between rounded border border-border bg-muted/30 px-2.5 py-2 font-mono-tech">
        <div className="flex items-center gap-1.5 truncate">
          <span className="font-semibold">{edgeType.src_type_name || "?"}</span>
          <ArrowRight className="h-3.5 w-3.5 text-muted-foreground" />
          <Badge variant="xblue">{edgeType.name || "UNNAMED"}</Badge>
          <ArrowRight className="h-3.5 w-3.5 text-muted-foreground" />
          <span className="font-semibold">{edgeType.dst_type_name || "?"}</span>
        </div>
        <Badge variant="outline">{edgeType.cardinality}</Badge>
      </div>

      <div>
        <label className="block font-medium mb-1">
          关系英文标识 (UPPER_SNAKE_CASE)
        </label>
        <Input
          data-testid="edge-type-name-input"
          value={edgeType.name}
          onChange={(e) => onChange({ ...edgeType, name: e.target.value })}
          placeholder="如 FULFILLS、SUPPLIES"
          className="h-8 text-xs font-mono-tech"
        />
      </div>

      <div className="grid grid-cols-2 gap-2">
        <div>
          <label className="block font-medium mb-1">源节点类型 (src)</label>
          <select
            value={edgeType.src_type_name}
            onChange={(e) =>
              onChange({ ...edgeType, src_type_name: e.target.value })
            }
            className="h-8 w-full rounded-md border border-input bg-background px-2 text-xs font-mono-tech"
          >
            <option value="">选择源类型...</option>
            {availableNodeTypes.map((nt) => (
              <option key={nt.name} value={nt.name}>
                {nt.name} ({nt.display_name})
              </option>
            ))}
          </select>
        </div>

        <div>
          <label className="block font-medium mb-1">目标节点类型 (dst)</label>
          <select
            value={edgeType.dst_type_name}
            onChange={(e) =>
              onChange({ ...edgeType, dst_type_name: e.target.value })
            }
            className="h-8 w-full rounded-md border border-input bg-background px-2 text-xs font-mono-tech"
          >
            <option value="">选择目标类型...</option>
            {availableNodeTypes.map((nt) => (
              <option key={nt.name} value={nt.name}>
                {nt.name} ({nt.display_name})
              </option>
            ))}
          </select>
        </div>
      </div>

      <div className="grid grid-cols-2 gap-2">
        <div>
          <label className="block font-medium mb-1">关系基数 (cardinality)</label>
          <select
            data-testid="edge-cardinality-select"
            value={edgeType.cardinality}
            onChange={(e) =>
              onChange({
                ...edgeType,
                cardinality: e.target.value as Cardinality,
              })
            }
            className="h-8 w-full rounded-md border border-input bg-background px-2 text-xs font-mono-tech"
          >
            {CARDINALITY_OPTIONS.map((opt) => (
              <option key={opt.value} value={opt.value}>
                {opt.label}
              </option>
            ))}
          </select>
        </div>

        <div>
          <label className="block font-medium mb-1">
            因果传导权重 impact_weight ∈ (0, 1]
          </label>
          <div className="flex items-center gap-2">
            <input
              type="range"
              min="0.05"
              max="1"
              step="0.05"
              value={edgeType.impact_weight}
              onChange={(e) =>
                onChange({
                  ...edgeType,
                  impact_weight: Number(e.target.value),
                })
              }
              className="flex-1 accent-xblue cursor-pointer"
            />
            <Input
              type="number"
              min={0.01}
              max={1}
              step={0.05}
              value={edgeType.impact_weight}
              onChange={(e) =>
                onChange({
                  ...edgeType,
                  impact_weight: Number(e.target.value),
                })
              }
              className="h-8 w-20 px-2 text-xs font-mono-tech"
            />
          </div>
        </div>
      </div>

      <div>
        <label className="block font-medium mb-1">关系说明 (description)</label>
        <Input
          value={edgeType.description}
          onChange={(e) =>
            onChange({ ...edgeType, description: e.target.value })
          }
          placeholder="描述业务关联语义与传导方向..."
          className="h-8 text-xs"
        />
      </div>

      <div>
        <div className="flex items-center justify-between mb-1">
          <label className="font-medium">
            AxSim 因果传导参数 (propagation JSON)
          </label>
          {jsonError ? (
            <span className="text-[10px] text-red-600 dark:text-red-400">
              {jsonError}
            </span>
          ) : (
            <span className="text-[10px] text-emerald-600 dark:text-emerald-400 font-mono-tech">
              JSON 合法
            </span>
          )}
        </div>
        <Textarea
          rows={3}
          value={propagationText}
          onChange={(e) => handlePropagationChange(e.target.value)}
          className="font-mono-tech text-xs"
          placeholder='{"lag_hours": 4, "attenuation": "linear"}'
        />
      </div>

      <div className="flex justify-end pt-2 border-t border-border">
        <Button
          variant="outline"
          onClick={onDelete}
          className="h-7 px-2.5 py-1 text-red-600 dark:text-red-400 hover:bg-red-500/10"
        >
          <Trash2 className="h-3.5 w-3.5" />
          删除关系类型
        </Button>
      </div>
    </div>
  );
}
