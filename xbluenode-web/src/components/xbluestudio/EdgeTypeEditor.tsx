import { useEffect, useState } from "react";
import { ArrowRight, Trash2 } from "lucide-react";
import { useI18n } from "@/i18n";
import { usePrincipal } from "@/lib/permissions";
import type { Cardinality } from "@/lib/api/enums";
import type { EdgeTypeSpec, NodeTypeSpec } from "@/lib/api/xblue";
import {
  Badge,
  Button,
  Input,
  NumberInput,
  Select,
  Textarea,
} from "@/components/ui/primitives";

export interface EdgeTypeEditorProps {
  edgeType: EdgeTypeSpec;
  availableNodeTypes: NodeTypeSpec[];
  onChange: (updated: EdgeTypeSpec) => void;
  onDelete: () => void;
}

const CARDINALITY_OPTIONS: readonly {
  value: Cardinality;
  zh: string;
  en: string;
}[] = [
  { value: "ONE_TO_ONE", zh: "1:1 一对一", en: "1:1 One-to-One" },
  { value: "ONE_TO_MANY", zh: "1:N 一对多", en: "1:N One-to-Many" },
  { value: "MANY_TO_ONE", zh: "N:1 多对一", en: "N:1 Many-to-One" },
  { value: "MANY_TO_MANY", zh: "M:N 多对多", en: "M:N Many-to-Many" },
];

export function EdgeTypeEditor({
  edgeType,
  availableNodeTypes,
  onChange,
  onDelete,
}: EdgeTypeEditorProps) {
  const { tx } = useI18n();
  const { can } = usePrincipal();
  const canEdit = can("STUDIO_EDIT");

  const [propagationText, setPropagationText] = useState(() =>
    JSON.stringify(edgeType.propagation ?? {}, null, 2)
  );
  const [jsonError, setJsonError] = useState<string | null>(null);
  const [confirmDelete, setConfirmDelete] = useState(false);

  useEffect(() => {
    setPropagationText(JSON.stringify(edgeType.propagation ?? {}, null, 2));
    setJsonError(null);
    setConfirmDelete(false);
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
        setJsonError(tx("需为 JSON 对象", "JSON object required"));
        return;
      }
      setJsonError(null);
      onChange({
        ...edgeType,
        propagation: parsed as Record<string, unknown>,
      });
    } catch {
      setJsonError(tx("JSON 语法错误", "Invalid JSON"));
    }
  };

  return (
    <div className="space-y-3 text-xs" data-testid="edge-type-editor">
      <div className="flex items-center justify-between rounded-md bg-muted/40 px-2.5 py-2 font-mono-tech">
        <div className="flex items-center gap-1.5 truncate">
          <span className="font-semibold">{edgeType.src_type_name || "?"}</span>
          <ArrowRight className="h-3.5 w-3.5 text-muted-foreground shrink-0" />
          <Badge variant="xblue">{edgeType.name || "UNNAMED"}</Badge>
          <ArrowRight className="h-3.5 w-3.5 text-muted-foreground shrink-0" />
          <span className="font-semibold">{edgeType.dst_type_name || "?"}</span>
        </div>
        <Badge variant="outline">{edgeType.cardinality}</Badge>
      </div>

      <div>
        <label className="block font-medium mb-1">
          {tx("关系标识", "Relation Key")}
        </label>
        <Input
          data-testid="edge-type-name-input"
          value={edgeType.name}
          disabled={!canEdit}
          onChange={(e) => onChange({ ...edgeType, name: e.target.value })}
          placeholder="FULFILLS"
          className="h-8 text-xs font-mono-tech"
        />
      </div>

      <div className="grid grid-cols-2 gap-2">
        <div>
          <label className="block font-medium mb-1">
            {tx("源节点类型", "Source Type")}
          </label>
          <Select
            value={edgeType.src_type_name}
            disabled={!canEdit}
            onChange={(e) =>
              onChange({ ...edgeType, src_type_name: e.target.value })
            }
            className="h-8 w-full font-mono-tech"
          >
            <option value="">{tx("未指定", "None")}</option>
            {availableNodeTypes.map((nt) => (
              <option key={nt.name} value={nt.name}>
                {nt.name} ({nt.display_name})
              </option>
            ))}
          </Select>
        </div>

        <div>
          <label className="block font-medium mb-1">
            {tx("目标节点类型", "Target Type")}
          </label>
          <Select
            value={edgeType.dst_type_name}
            disabled={!canEdit}
            onChange={(e) =>
              onChange({ ...edgeType, dst_type_name: e.target.value })
            }
            className="h-8 w-full font-mono-tech"
          >
            <option value="">{tx("未指定", "None")}</option>
            {availableNodeTypes.map((nt) => (
              <option key={nt.name} value={nt.name}>
                {nt.name} ({nt.display_name})
              </option>
            ))}
          </Select>
        </div>
      </div>

      <div className="grid grid-cols-2 gap-2">
        <div>
          <label className="block font-medium mb-1">
            {tx("关系基数", "Cardinality")}
          </label>
          <Select
            data-testid="edge-cardinality-select"
            value={edgeType.cardinality}
            disabled={!canEdit}
            onChange={(e) =>
              onChange({
                ...edgeType,
                cardinality: e.target.value as Cardinality,
              })
            }
            className="h-8 w-full font-mono-tech"
          >
            {CARDINALITY_OPTIONS.map((opt) => (
              <option key={opt.value} value={opt.value}>
                {tx(opt.zh, opt.en)}
              </option>
            ))}
          </Select>
        </div>

        <div>
          <label className="block font-medium mb-1">
            {tx("因果传导权重", "Impact Weight")}
          </label>
          <NumberInput
            min={0.05}
            max={1}
            step={0.05}
            disabled={!canEdit}
            value={edgeType.impact_weight}
            onChange={(val) =>
              onChange({
                ...edgeType,
                impact_weight: Math.min(1, Math.max(0.01, Number(val.toFixed(2)))),
              })
            }
            className="w-full"
          />
        </div>
      </div>

      <div>
        <label className="block font-medium mb-1">
          {tx("关系说明", "Description")}
        </label>
        <Input
          value={edgeType.description}
          disabled={!canEdit}
          onChange={(e) =>
            onChange({ ...edgeType, description: e.target.value })
          }
          placeholder={tx("仓库向订单履约发货", "Warehouse fulfills order")}
          className="h-8 text-xs"
        />
      </div>

      <div>
        <div className="flex items-center justify-between mb-1">
          <label className="font-medium">
            {tx("推演传导参数", "Propagation Config")}
          </label>
          {jsonError ? (
            <span className="text-[10px] text-red-600 dark:text-red-400">
              {jsonError}
            </span>
          ) : (
            <span className="text-[10px] text-emerald-600 dark:text-emerald-400 font-mono-tech">
              {tx("JSON 合法", "Valid JSON")}
            </span>
          )}
        </div>
        <Textarea
          rows={3}
          value={propagationText}
          disabled={!canEdit}
          onChange={(e) => handlePropagationChange(e.target.value)}
          className="font-mono-tech text-xs"
          placeholder='{"lag_hours": 4, "attenuation": "linear"}'
        />
      </div>

      {canEdit && (
        <div className="flex items-center justify-end gap-1.5 pt-2 border-t border-border">
          {confirmDelete ? (
            <>
              <span className="text-[11px] text-red-600 dark:text-red-400 mr-auto">
                {tx("移除该关系类型定义", "Remove this relation type")}
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
              {tx("删除关系", "Delete")}
            </Button>
          )}
        </div>
      )}
    </div>
  );
}
