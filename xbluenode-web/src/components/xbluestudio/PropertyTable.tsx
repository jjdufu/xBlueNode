import { useState } from "react";
import {
  Check,
  ChevronDown,
  ChevronRight,
  Maximize2,
  Plus,
  Trash2,
} from "lucide-react";
import { useI18n } from "@/i18n";
import { usePrincipal } from "@/lib/permissions";
import type { DataType, PiiCategory } from "@/lib/api/enums";
import type { PropertyKind, PropertySpec } from "@/lib/api/xblue";
import {
  Badge,
  Button,
  Drawer,
  EmptyState,
  Input,
  Select,
  StatText,
  Switch,
} from "@/components/ui/primitives";
import { cn } from "@/lib/utils";

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

  const [expandedIndex, setExpandedIndex] = useState<number | null>(null);
  const [sheetDrawerOpen, setSheetDrawerOpen] = useState(false);

  const attrCount = properties.filter((p) => p.kind === "ATTRIBUTE").length;
  const metricCount = properties.filter((p) => p.kind === "METRIC").length;

  const addRow = (kind: PropertyKind) => {
    if (!canEdit) return;
    const idx = properties.length + 1;
    const defaultName = kind === "METRIC" ? `metric_${idx}` : `attr_${idx}`;
    const nextList: PropertySpec[] = [
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
    ];
    onChange(nextList);
    setExpandedIndex(nextList.length - 1);
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
    if (expandedIndex === index) {
      setExpandedIndex(null);
    } else if (expandedIndex !== null && expandedIndex > index) {
      setExpandedIndex(expandedIndex - 1);
    }
  };

  return (
    <div className="space-y-2.5 text-xs" data-testid="property-table-editor">
      {/* Header: Borderless StatText counts + Wide Drawer Trigger + Add Actions */}
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div className="flex items-center gap-2">
          <StatText label={tx("属性", "Attr")} value={attrCount} />
          <span className="text-border">·</span>
          <StatText label={tx("指标", "Metric")} value={metricCount} />
        </div>

        <div className="flex items-center gap-1.5">
          {properties.length > 0 && (
            <Button
              variant="ghost"
              size="sm"
              onClick={() => setSheetDrawerOpen(true)}
              title={tx("宽屏批量编辑", "Wide Table Editor")}
            >
              <Maximize2 className="h-3 w-3 text-xblue" />
              {tx("表格编辑", "Table")}
            </Button>
          )}
          {canEdit && (
            <>
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
            </>
          )}
        </div>
      </div>

      {/* Compact Collapsible Field Card List (Zero Horizontal Truncation in Narrow Sidebar) */}
      {properties.length === 0 ? (
        <EmptyState title={tx("暂无属性或指标定义", "No properties defined")} />
      ) : (
        <div className="space-y-1.5">
          {properties.map((row, idx) => {
            const expanded = expandedIndex === idx;
            const isMetric = row.kind === "METRIC";
            const isProposed = row.maturity === "PROPOSED";
            return (
              <div
                key={`${idx}-${row.kind}`}
                className={cn(
                  "rounded-md border transition-colors",
                  expanded
                    ? "border-xblue/50 bg-xblue-soft/10"
                    : "border-border bg-background hover:border-foreground/30",
                  isProposed && "border-dashed border-amber-500/70"
                )}
              >
                <div className="flex items-center justify-between gap-2 px-2.5 py-2">
                  <button
                    type="button"
                    onClick={() =>
                      setExpandedIndex((prev) => (prev === idx ? null : idx))
                    }
                    className="flex items-center gap-1.5 min-w-0 flex-1 text-left cursor-pointer"
                  >
                    {expanded ? (
                      <ChevronDown className="h-3.5 w-3.5 text-xblue shrink-0" />
                    ) : (
                      <ChevronRight className="h-3.5 w-3.5 text-muted-foreground shrink-0" />
                    )}
                    <span
                      className={cn(
                        "font-mono-tech font-semibold truncate",
                        isMetric ? "text-xblue" : "text-foreground"
                      )}
                    >
                      {row.name || tx("未命名", "unnamed")}
                    </span>
                    {row.display_name && (
                      <span className="text-muted-foreground truncate">
                        · {row.display_name}
                      </span>
                    )}
                  </button>

                  <div className="flex items-center gap-1 shrink-0">
                    {isProposed && (
                      <Badge
                        variant="warning"
                        data-testid={`prop-proposed-badge-${row.name || idx}`}
                      >
                        {tx("待确认", "Proposed")}
                      </Badge>
                    )}
                    {isProposed && canEdit && (
                      <button
                        type="button"
                        data-testid={`prop-confirm-btn-${row.name || idx}`}
                        onClick={() =>
                          updateRow(idx, { maturity: "CONFIRMED" })
                        }
                        className="inline-flex items-center gap-0.5 rounded border border-amber-500/50 bg-amber-500/15 px-1.5 py-0.5 text-[10px] font-medium text-amber-700 dark:text-amber-300 hover:bg-amber-500/25 cursor-pointer"
                      >
                        <Check className="h-2.5 w-2.5" />
                        {tx("确认", "Confirm")}
                      </button>
                    )}
                    {row.unit && (
                      <span className="rounded bg-muted px-1.5 py-0.5 text-[10px] text-muted-foreground">
                        {row.unit}
                      </span>
                    )}
                    <Badge
                      variant={isMetric ? "xblue" : "outline"}
                      title={row.data_type}
                    >
                      {tEnum(row.data_type)}
                    </Badge>
                    {canEdit && (
                      <button
                        type="button"
                        onClick={() => removeRow(idx)}
                        className="rounded p-1 text-muted-foreground hover:text-red-600 hover:bg-muted cursor-pointer"
                        title={tx("删除字段", "Delete field")}
                      >
                        <Trash2 className="h-3.5 w-3.5" />
                      </button>
                    )}
                  </div>
                </div>

                {expanded && (
                  <div className="border-t border-border/70 px-2.5 py-2.5 space-y-2 bg-card/60">
                    <div className="grid grid-cols-2 gap-2">
                      <div>
                        <label className="block text-[11px] text-muted-foreground mb-1">
                          {tx("字段名", "Field Key")}
                        </label>
                        <Input
                          value={row.name}
                          disabled={!canEdit}
                          onChange={(e) =>
                            updateRow(idx, { name: e.target.value })
                          }
                          placeholder="inventory"
                          className="h-8 font-mono-tech text-xs"
                        />
                      </div>
                      <div>
                        <label className="block text-[11px] text-muted-foreground mb-1">
                          {tx("显示名", "Display Label")}
                        </label>
                        <Input
                          value={row.display_name}
                          disabled={!canEdit}
                          onChange={(e) =>
                            updateRow(idx, { display_name: e.target.value })
                          }
                          placeholder={tx("在库库存", "Inventory")}
                          className="h-8 text-xs"
                        />
                      </div>
                    </div>

                    <div className="grid grid-cols-2 gap-2">
                      <div>
                        <label className="block text-[11px] text-muted-foreground mb-1">
                          {tx("字段分类", "Field Kind")}
                        </label>
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
                          className="h-8 w-full font-mono-tech"
                        >
                          <option value="ATTRIBUTE">
                            {tx("静态属性", "Attribute")}
                          </option>
                          <option value="METRIC">
                            {tx("时序指标", "Metric")}
                          </option>
                        </Select>
                      </div>
                      <div>
                        <label className="block text-[11px] text-muted-foreground mb-1">
                          {tx("数据类型", "Data Type")}
                        </label>
                        <Select
                          value={row.data_type}
                          disabled={!canEdit}
                          onChange={(e) =>
                            updateRow(idx, {
                              data_type: e.target.value as DataType,
                            })
                          }
                          className="h-8 w-full font-mono-tech"
                        >
                          {DATA_TYPE_OPTIONS.map((dt) => (
                            <option key={dt} value={dt}>
                              {dt}
                            </option>
                          ))}
                        </Select>
                      </div>
                    </div>

                    <div className="grid grid-cols-2 gap-2">
                      <div>
                        <label className="block text-[11px] text-muted-foreground mb-1">
                          {tx("计量单位", "Unit")}
                        </label>
                        <Input
                          value={row.unit ?? ""}
                          disabled={!canEdit}
                          onChange={(e) =>
                            updateRow(idx, {
                              unit: e.target.value || undefined,
                            })
                          }
                          placeholder={tx("件", "pcs")}
                          className="h-8 text-xs"
                        />
                      </div>
                      <div>
                        <label className="block text-[11px] text-muted-foreground mb-1">
                          {tx("敏感分级", "PII Category")}
                        </label>
                        <Select
                          value={row.pii_category}
                          disabled={!canEdit}
                          onChange={(e) =>
                            updateRow(idx, {
                              pii_category: e.target.value as PiiCategory,
                            })
                          }
                          className="h-8 w-full"
                        >
                          {PII_OPTIONS.map((pii) => (
                            <option key={pii} value={pii}>
                              {tEnum(pii)}
                            </option>
                          ))}
                        </Select>
                      </div>
                    </div>

                    <div className="flex items-center justify-between pt-0.5">
                      <span className="text-[11px] text-muted-foreground">
                        {tx("必填校验", "Required")}
                      </span>
                      <Switch
                        checked={row.required}
                        disabled={!canEdit}
                        onCheckedChange={(checked) =>
                          updateRow(idx, { required: checked })
                        }
                        aria-label={tx("必填校验", "Required")}
                      />
                    </div>
                  </div>
                )}
              </div>
            );
          })}
        </div>
      )}

      {/* Wide Spreadsheet Drawer for Batch Property Editing */}
      <Drawer
        open={sheetDrawerOpen}
        onClose={() => setSheetDrawerOpen(false)}
        widthClass="max-w-4xl"
        title={tx("字段批量表格编辑", "Batch Field Table Editor")}
        footer={
          <>
            {canEdit && (
              <div className="flex items-center gap-1.5 mr-auto">
                <Button
                  variant="outline"
                  onClick={() => addRow("ATTRIBUTE")}
                >
                  <Plus className="h-3.5 w-3.5" />
                  {tx("添加属性", "Add Attr")}
                </Button>
                <Button
                  variant="outline"
                  onClick={() => addRow("METRIC")}
                >
                  <Plus className="h-3.5 w-3.5 text-xblue" />
                  {tx("添加指标", "Add Metric")}
                </Button>
              </div>
            )}
            <Button
              variant="primary"
              onClick={() => setSheetDrawerOpen(false)}
            >
              {tx("完成", "Done")}
            </Button>
          </>
        }
      >
        <div className="overflow-x-auto rounded-md border border-border">
          <table className="w-full border-collapse text-left text-xs">
            <thead className="bg-muted/60 text-[11px] text-muted-foreground">
              <tr>
                <th className="px-2.5 py-2 font-medium">
                  {tx("字段名", "Field")}
                </th>
                <th className="px-2.5 py-2 font-medium">
                  {tx("显示名", "Label")}
                </th>
                <th className="px-2.5 py-2 font-medium">
                  {tx("分类", "Kind")}
                </th>
                <th className="px-2.5 py-2 font-medium">
                  {tx("数据类型", "Type")}
                </th>
                <th className="px-2.5 py-2 font-medium">
                  {tx("单位", "Unit")}
                </th>
                <th className="px-2.5 py-2 font-medium text-center">
                  {tx("必填", "Req")}
                </th>
                <th className="px-2.5 py-2 font-medium">
                  {tx("敏感分级", "PII")}
                </th>
                {canEdit && <th className="px-2 py-2 font-medium w-10" />}
              </tr>
            </thead>
            <tbody className="divide-y divide-border">
              {properties.map((row, idx) => (
                <tr key={idx} className="hover:bg-muted/20">
                  <td className="p-2">
                    <div className="flex items-center gap-1.5">
                      <Input
                        value={row.name}
                        disabled={!canEdit}
                        onChange={(e) =>
                          updateRow(idx, { name: e.target.value })
                        }
                        placeholder="inventory"
                        className="h-8 font-mono-tech min-w-[120px]"
                      />
                      {row.maturity === "PROPOSED" && (
                        <Badge variant="warning">
                          {tx("待确认", "Proposed")}
                        </Badge>
                      )}
                    </div>
                  </td>
                  <td className="p-2">
                    <Input
                      value={row.display_name}
                      disabled={!canEdit}
                      onChange={(e) =>
                        updateRow(idx, { display_name: e.target.value })
                      }
                      placeholder={tx("在库库存", "Inventory")}
                      className="h-8 min-w-[120px]"
                    />
                  </td>
                  <td className="p-2">
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
                      className="h-8 w-28 font-mono-tech"
                    >
                      <option value="ATTRIBUTE">
                        {tx("静态属性", "Attribute")}
                      </option>
                      <option value="METRIC">
                        {tx("时序指标", "Metric")}
                      </option>
                    </Select>
                  </td>
                  <td className="p-2">
                    <Select
                      value={row.data_type}
                      disabled={!canEdit}
                      onChange={(e) =>
                        updateRow(idx, {
                          data_type: e.target.value as DataType,
                        })
                      }
                      className="h-8 w-28 font-mono-tech"
                    >
                      {DATA_TYPE_OPTIONS.map((dt) => (
                        <option key={dt} value={dt}>
                          {dt}
                        </option>
                      ))}
                    </Select>
                  </td>
                  <td className="p-2">
                    <Input
                      value={row.unit ?? ""}
                      disabled={!canEdit}
                      onChange={(e) =>
                        updateRow(idx, {
                          unit: e.target.value || undefined,
                        })
                      }
                      placeholder={tx("件", "pcs")}
                      className="h-8 w-20"
                    />
                  </td>
                  <td className="p-2 text-center">
                    <Switch
                      checked={row.required}
                      disabled={!canEdit}
                      onCheckedChange={(checked) =>
                        updateRow(idx, { required: checked })
                      }
                      aria-label={tx("必填", "Required")}
                    />
                  </td>
                  <td className="p-2">
                    <Select
                      value={row.pii_category}
                      disabled={!canEdit}
                      onChange={(e) =>
                        updateRow(idx, {
                          pii_category: e.target.value as PiiCategory,
                        })
                      }
                      className="h-8 w-28"
                    >
                      {PII_OPTIONS.map((pii) => (
                        <option key={pii} value={pii}>
                          {tEnum(pii)}
                        </option>
                      ))}
                    </Select>
                  </td>
                  {canEdit && (
                    <td className="p-2 text-right">
                      <button
                        type="button"
                        onClick={() => removeRow(idx)}
                        className="rounded p-1.5 text-muted-foreground hover:text-red-600 hover:bg-muted cursor-pointer"
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
      </Drawer>
    </div>
  );
}
