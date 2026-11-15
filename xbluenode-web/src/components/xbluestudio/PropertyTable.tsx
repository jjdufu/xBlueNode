import { Plus, Trash2 } from "lucide-react";
import type { DataType, PiiCategory } from "@/lib/api/enums";
import type { PropertyKind, PropertySpec } from "@/lib/api/xblue";
import { Badge, Button, Input } from "@/components/ui/primitives";

export interface PropertyTableProps {
  properties: PropertySpec[];
  onChange: (next: PropertySpec[]) => void;
}

const DATA_TYPE_OPTIONS: readonly DataType[] = [
  "STRING",
  "INT64",
  "FLOAT64",
  "DECIMAL",
  "BOOLEAN",
  "DATE",
  "TIMESTAMP",
  "BINARY",
  "JSON",
  "LIST",
  "STRUCT",
];

const PII_OPTIONS: readonly PiiCategory[] = [
  "NONE",
  "PHONE",
  "ID_CARD",
  "EMAIL",
  "BANK_CARD",
  "NAME",
  "ADDRESS",
  "SECRET",
];

export function PropertyTable({ properties, onChange }: PropertyTableProps) {
  const addRow = (kind: PropertyKind) => {
    const idx = properties.length + 1;
    const defaultName =
      kind === "METRIC" ? `metric_${idx}` : `attr_${idx}`;
    onChange([
      ...properties,
      {
        name: defaultName,
        display_name: kind === "METRIC" ? `指标 ${idx}` : `属性 ${idx}`,
        description: "",
        unit: kind === "METRIC" ? "" : undefined,
        data_type: kind === "METRIC" ? "FLOAT64" : "STRING",
        kind,
        required: false,
        pii_category: "NONE",
      },
    ]);
  };

  const updateRow = (index: number, patch: Partial<PropertySpec>) => {
    onChange(
      properties.map((row, i) => (i === index ? { ...row, ...patch } : row))
    );
  };

  const removeRow = (index: number) => {
    onChange(properties.filter((_, i) => i !== index));
  };

  return (
    <div className="space-y-2.5 text-xs" data-testid="property-table-editor">
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-1.5">
          <span className="font-semibold">属性与指标定义</span>
          <Badge variant="neutral">
            属性 {properties.filter((p) => p.kind === "ATTRIBUTE").length}
          </Badge>
          <Badge variant="xblue">
            指标 {properties.filter((p) => p.kind === "METRIC").length}
          </Badge>
        </div>
        <div className="flex items-center gap-1.5">
          <Button
            variant="outline"
            className="h-7 px-2 py-1 text-xs"
            data-testid="add-attribute-btn"
            onClick={() => addRow("ATTRIBUTE")}
          >
            <Plus className="h-3 w-3" />
            添加属性
          </Button>
          <Button
            variant="outline"
            className="h-7 px-2 py-1 text-xs"
            data-testid="add-metric-btn"
            onClick={() => addRow("METRIC")}
          >
            <Plus className="h-3 w-3 text-xblue" />
            添加指标
          </Button>
        </div>
      </div>

      {properties.length === 0 ? (
        <div className="rounded border border-dashed border-border p-4 text-center text-muted-foreground">
          暂无属性或指标定义，点击上方“添加属性”或“添加指标”开始建模。
        </div>
      ) : (
        <div className="overflow-x-auto rounded border border-border">
          <table className="w-full border-collapse text-left">
            <thead className="bg-muted/60 text-[11px] text-muted-foreground">
              <tr>
                <th className="px-2 py-1.5 font-medium">字段名 (snake_case)</th>
                <th className="px-2 py-1.5 font-medium">中文名</th>
                <th className="px-2 py-1.5 font-medium">分类</th>
                <th className="px-2 py-1.5 font-medium">数据类型</th>
                <th className="px-2 py-1.5 font-medium">单位</th>
                <th className="px-2 py-1.5 font-medium text-center">必填</th>
                <th className="px-2 py-1.5 font-medium">PII 分级</th>
                <th className="px-2 py-1.5 font-medium w-8" />
              </tr>
            </thead>
            <tbody className="divide-y divide-border">
              {properties.map((row, idx) => (
                <tr key={idx} className="hover:bg-muted/20">
                  <td className="p-1.5">
                    <Input
                      value={row.name}
                      onChange={(e) =>
                        updateRow(idx, { name: e.target.value })
                      }
                      placeholder="inventory"
                      className="h-7 px-2 text-xs font-mono-tech min-w-[96px]"
                    />
                  </td>
                  <td className="p-1.5">
                    <Input
                      value={row.display_name}
                      onChange={(e) =>
                        updateRow(idx, { display_name: e.target.value })
                      }
                      placeholder="在库库存"
                      className="h-7 px-2 text-xs min-w-[84px]"
                    />
                  </td>
                  <td className="p-1.5">
                    <select
                      value={row.kind}
                      onChange={(e) => {
                        const nextKind = e.target.value as PropertyKind;
                        updateRow(idx, {
                          kind: nextKind,
                          data_type:
                            nextKind === "METRIC" &&
                            row.data_type !== "INT64" &&
                            row.data_type !== "FLOAT64" &&
                            row.data_type !== "DECIMAL"
                              ? "FLOAT64"
                              : row.data_type,
                        });
                      }}
                      className="h-7 rounded border border-input bg-background px-1.5 text-[11px] font-mono-tech"
                    >
                      <option value="ATTRIBUTE">ATTRIBUTE</option>
                      <option value="METRIC">METRIC</option>
                    </select>
                  </td>
                  <td className="p-1.5">
                    <select
                      value={row.data_type}
                      onChange={(e) =>
                        updateRow(idx, {
                          data_type: e.target.value as DataType,
                        })
                      }
                      className="h-7 rounded border border-input bg-background px-1.5 text-[11px] font-mono-tech"
                    >
                      {DATA_TYPE_OPTIONS.map((dt) => (
                        <option key={dt} value={dt}>
                          {dt}
                        </option>
                      ))}
                    </select>
                  </td>
                  <td className="p-1.5">
                    <Input
                      value={row.unit ?? ""}
                      onChange={(e) =>
                        updateRow(idx, {
                          unit: e.target.value || undefined,
                        })
                      }
                      placeholder="件 / 元 / %"
                      className="h-7 px-2 text-xs w-20"
                    />
                  </td>
                  <td className="p-1.5 text-center">
                    <input
                      type="checkbox"
                      checked={row.required}
                      onChange={(e) =>
                        updateRow(idx, { required: e.target.checked })
                      }
                      className="cursor-pointer"
                    />
                  </td>
                  <td className="p-1.5">
                    <select
                      value={row.pii_category}
                      onChange={(e) =>
                        updateRow(idx, {
                          pii_category: e.target.value as PiiCategory,
                        })
                      }
                      className="h-7 rounded border border-input bg-background px-1.5 text-[11px] font-mono-tech"
                    >
                      {PII_OPTIONS.map((pii) => (
                        <option key={pii} value={pii}>
                          {pii}
                        </option>
                      ))}
                    </select>
                  </td>
                  <td className="p-1.5 text-right">
                    <button
                      type="button"
                      onClick={() => removeRow(idx)}
                      className="rounded p-1 text-muted-foreground hover:text-red-600 hover:bg-muted cursor-pointer"
                      title="删除字段"
                    >
                      <Trash2 className="h-3.5 w-3.5" />
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}
