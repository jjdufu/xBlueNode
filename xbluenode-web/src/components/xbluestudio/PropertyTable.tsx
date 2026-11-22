import { Plus, Trash2 } from "lucide-react";
import { useI18n } from "@/i18n";
import { usePrincipal } from "@/lib/permissions";
import type { DataType, PiiCategory } from "@/lib/api/enums";
import type { PropertyKind, PropertySpec } from "@/lib/api/xblue";
import {
  Badge,
  Button,
  EmptyState,
  Input,
  Select,
} from "@/components/ui/primitives";

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
  const { tx, tEnum } = useI18n();
  const { can } = usePrincipal();
  const canEdit = can("STUDIO_EDIT");

  const addRow = (kind: PropertyKind) => {
    if (!canEdit) return;
    const idx = properties.length + 1;
    const defaultName = kind === "METRIC" ? `metric_${idx}` : `attr_${idx}`;
    onChange([
      ...properties,
      {
        name: defaultName,
        display_name:
          kind === "METRIC"
            ? tx(`指标 ${idx}`, `Metric ${idx}`)
            : tx(`属性 ${idx}`, `Attr ${idx}`),
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
    if (!canEdit) return;
    onChange(
      properties.map((row, i) => (i === index ? { ...row, ...patch } : row))
    );
  };

  const removeRow = (index: number) => {
    if (!canEdit) return;
    onChange(properties.filter((_, i) => i !== index));
  };

  return (
    <div className="space-y-2.5 text-xs" data-testid="property-table-editor">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div className="flex items-center gap-1.5">
          <span className="font-semibold">
            {tx("属性与指标", "Properties & Metrics")}
          </span>
          <Badge variant="neutral">
            {tx("属性", "Attr")}{" "}
            {properties.filter((p) => p.kind === "ATTRIBUTE").length}
          </Badge>
          <Badge variant="xblue">
            {tx("指标", "Metric")}{" "}
            {properties.filter((p) => p.kind === "METRIC").length}
          </Badge>
        </div>
        {canEdit && (
          <div className="flex items-center gap-1.5">
            <Button
              variant="outline"
              size="sm"
              data-testid="add-attribute-btn"
              onClick={() => addRow("ATTRIBUTE")}
            >
              <Plus className="h-3 w-3" />
              {tx("添加属性", "Add Attr")}
            </Button>
            <Button
              variant="outline"
              size="sm"
              data-testid="add-metric-btn"
              onClick={() => addRow("METRIC")}
            >
              <Plus className="h-3 w-3 text-xblue" />
              {tx("添加指标", "Add Metric")}
            </Button>
          </div>
        )}
      </div>

      {properties.length === 0 ? (
        <EmptyState title={tx("暂无属性或指标定义", "No properties defined")} />
      ) : (
        <div className="overflow-x-auto rounded border border-border">
          <table className="w-full border-collapse text-left">
            <thead className="bg-muted/60 text-[11px] text-muted-foreground">
              <tr>
                <th className="px-2 py-1.5 font-medium">
                  {tx("字段名", "Field")}
                </th>
                <th className="px-2 py-1.5 font-medium">
                  {tx("显示名", "Label")}
                </th>
                <th className="px-2 py-1.5 font-medium">
                  {tx("分类", "Kind")}
                </th>
                <th className="px-2 py-1.5 font-medium">
                  {tx("数据类型", "Type")}
                </th>
                <th className="px-2 py-1.5 font-medium">
                  {tx("单位", "Unit")}
                </th>
                <th className="px-2 py-1.5 font-medium text-center">
                  {tx("必填", "Req")}
                </th>
                <th className="px-2 py-1.5 font-medium">
                  {tx("敏感分级", "PII")}
                </th>
                {canEdit && <th className="px-2 py-1.5 font-medium w-8" />}
              </tr>
            </thead>
            <tbody className="divide-y divide-border">
              {properties.map((row, idx) => (
                <tr key={idx} className="hover:bg-muted/20">
                  <td className="p-1.5">
                    <Input
                      value={row.name}
                      disabled={!canEdit}
                      onChange={(e) =>
                        updateRow(idx, { name: e.target.value })
                      }
                      placeholder="inventory"
                      className="h-7 px-2 text-xs font-mono-tech min-w-[92px]"
                    />
                  </td>
                  <td className="p-1.5">
                    <Input
                      value={row.display_name}
                      disabled={!canEdit}
                      onChange={(e) =>
                        updateRow(idx, { display_name: e.target.value })
                      }
                      placeholder={tx("在库库存", "Inventory")}
                      className="h-7 px-2 text-xs min-w-[84px]"
                    />
                  </td>
                  <td className="p-1.5">
                    <Select
                      value={row.kind}
                      disabled={!canEdit}
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
                      className="h-7 px-1.5 text-[11px] font-mono-tech"
                    >
                      <option value="ATTRIBUTE">
                        {tx("静态属性", "Attribute")}
                      </option>
                      <option value="METRIC">
                        {tx("时序指标", "Metric")}
                      </option>
                    </Select>
                  </td>
                  <td className="p-1.5">
                    <Select
                      value={row.data_type}
                      disabled={!canEdit}
                      onChange={(e) =>
                        updateRow(idx, {
                          data_type: e.target.value as DataType,
                        })
                      }
                      className="h-7 px-1.5 text-[11px] font-mono-tech"
                    >
                      {DATA_TYPE_OPTIONS.map((dt) => (
                        <option key={dt} value={dt}>
                          {dt}
                        </option>
                      ))}
                    </Select>
                  </td>
                  <td className="p-1.5">
                    <Input
                      value={row.unit ?? ""}
                      disabled={!canEdit}
                      onChange={(e) =>
                        updateRow(idx, {
                          unit: e.target.value || undefined,
                        })
                      }
                      placeholder={tx("件", "pcs")}
                      className="h-7 px-2 text-xs w-16"
                    />
                  </td>
                  <td className="p-1.5 text-center">
                    <input
                      type="checkbox"
                      checked={row.required}
                      disabled={!canEdit}
                      onChange={(e) =>
                        updateRow(idx, { required: e.target.checked })
                      }
                      className="cursor-pointer"
                    />
                  </td>
                  <td className="p-1.5">
                    <Select
                      value={row.pii_category}
                      disabled={!canEdit}
                      onChange={(e) =>
                        updateRow(idx, {
                          pii_category: e.target.value as PiiCategory,
                        })
                      }
                      className="h-7 px-1.5 text-[11px]"
                    >
                      {PII_OPTIONS.map((pii) => (
                        <option key={pii} value={pii}>
                          {tEnum(pii)}
                        </option>
                      ))}
                    </Select>
                  </td>
                  {canEdit && (
                    <td className="p-1.5 text-right">
                      <button
                        type="button"
                        onClick={() => removeRow(idx)}
                        className="rounded p-1 text-muted-foreground hover:text-red-600 hover:bg-muted cursor-pointer"
                        title={tx("删除字段", "Delete field")}
                      >
                        <Trash2 className="h-3.5 w-3.5" />
                      </button>
                    </td>
                  )}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}
