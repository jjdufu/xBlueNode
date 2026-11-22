import { useEffect, useMemo, useState } from "react";
import { CheckCircle2, Play, Send } from "lucide-react";
import { useI18n } from "@/i18n";
import {
  xblueviewApi,
  type InvokeWidgetActionResponse,
  type WidgetDataResponse,
  type WidgetRecord,
} from "@/lib/api/xblueview";
import { Badge, Button, Input, Select } from "@/components/ui/primitives";
import { resolveActionTargetNodeId } from "./logic";
import {
  buildColumnIndexMap,
  getCell,
  isRecord,
  readBoolean,
  readString,
  type KnownNodeSummary,
} from "./utils";

export interface ActionWidgetProps {
  widget: WidgetRecord;
  data: WidgetDataResponse | null;
  readOnly?: boolean;
  selectedNodeId?: string | null;
  filterValues?: Record<string, unknown>;
  knownNodes?: Readonly<Record<string, KnownNodeSummary>>;
  onActionSuccess?: () => void;
}

interface SchemaPropertyField {
  key: string;
  title: string;
  type: "number" | "integer" | "boolean" | "string";
  required: boolean;
  enumValues?: string[];
  defaultValue?: unknown;
}

function parseInputSchemaFields(
  schema: Record<string, unknown> | undefined
): SchemaPropertyField[] {
  if (!schema) return [];
  const rawProps = isRecord(schema.properties) ? schema.properties : {};
  const requiredList = Array.isArray(schema.required)
    ? schema.required.filter(
        (item): item is string => typeof item === "string"
      )
    : [];
  const requiredSet = new Set(requiredList);

  const fields: SchemaPropertyField[] = [];
  for (const [key, propVal] of Object.entries(rawProps)) {
    if (!isRecord(propVal)) continue;
    const rawType = readString(propVal, "type") ?? "string";
    const fieldType: SchemaPropertyField["type"] =
      rawType === "number"
        ? "number"
        : rawType === "integer"
          ? "integer"
          : rawType === "boolean"
            ? "boolean"
            : "string";
    const enumValues = Array.isArray(propVal.enum)
      ? propVal.enum.map((v) => String(v))
      : undefined;
    fields.push({
      key,
      title: readString(propVal, "title") ?? key,
      type: fieldType,
      required: requiredSet.has(key),
      enumValues,
      defaultValue: propVal.default,
    });
  }
  return fields;
}

function coercePrefillValue(
  raw: unknown,
  fieldType: SchemaPropertyField["type"]
): unknown {
  if (raw === undefined || raw === null || raw === "") return undefined;
  if (Array.isArray(raw)) {
    return raw.length > 0 ? coercePrefillValue(raw[0], fieldType) : undefined;
  }
  if (fieldType === "boolean") {
    return Boolean(raw);
  }
  return String(raw);
}

export function ActionWidget({
  widget,
  data,
  readOnly = false,
  selectedNodeId,
  filterValues,
  knownNodes,
  onActionSuccess,
}: ActionWidgetProps) {
  const { tx, tEnum, fmtTime } = useI18n();
  const ds = widget.data_source.kind === "ACTION" ? widget.data_source : null;
  const fields = useMemo(
    () => parseInputSchemaFields(ds?.input_schema),
    [ds?.input_schema]
  );

  const [touchedKeys, setTouchedKeys] = useState<Set<string>>(() => new Set());
  const [formValues, setFormValues] = useState<Record<string, unknown>>(() => {
    const initial: Record<string, unknown> = {};
    for (const f of fields) {
      const fromFilter = coercePrefillValue(filterValues?.[f.key], f.type);
      if (f.defaultValue !== undefined) {
        initial[f.key] = f.defaultValue;
      } else if (fromFilter !== undefined) {
        initial[f.key] = fromFilter;
      } else if (f.type === "boolean") {
        initial[f.key] = false;
      } else {
        initial[f.key] = "";
      }
    }
    return initial;
  });

  useEffect(() => {
    if (!filterValues) return;
    setFormValues((prev) => {
      let changed = false;
      const next = { ...prev };
      for (const f of fields) {
        if (touchedKeys.has(f.key) || f.defaultValue !== undefined) continue;
        const fromFilter = coercePrefillValue(filterValues[f.key], f.type);
        if (fromFilter !== undefined && next[f.key] !== fromFilter) {
          next[f.key] = fromFilter;
          changed = true;
        }
      }
      return changed ? next : prev;
    });
  }, [fields, filterValues, touchedKeys]);

  const [confirming, setConfirming] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [lastResult, setLastResult] =
    useState<InvokeWidgetActionResponse | null>(null);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  if (!ds) {
    return (
      <div className="h-full flex items-center justify-center text-xs text-muted-foreground">
        {tx("动作配置缺失", "Action config missing")}
      </div>
    );
  }

  const isButton = widget.widget_kind === "ACTION_BUTTON";
  const buttonLabel =
    readString(widget.options, "button_label") ??
    (ds.action_type === "RUN_FLOW"
      ? tx("触发流程", "Run Flow")
      : tx("提交回写", "Submit"));

  const resolvedNodeId = resolveActionTargetNodeId(ds.node_id, selectedNodeId);
  const knownTargetNode = resolvedNodeId ? knownNodes?.[resolvedNodeId] : undefined;
  const targetNodeLabel = knownTargetNode
    ? knownTargetNode.nodeKey && knownTargetNode.nodeKey !== knownTargetNode.title
      ? `${knownTargetNode.title} (${knownTargetNode.nodeKey})`
      : knownTargetNode.title
    : null;

  const updateFieldValue = (key: string, value: unknown) => {
    setTouchedKeys((prev) => {
      if (prev.has(key)) return prev;
      const next = new Set(prev);
      next.add(key);
      return next;
    });
    setFormValues((prev) => ({ ...prev, [key]: value }));
  };

  const executeInvoke = async () => {
    if (readOnly) return;
    setSubmitting(true);
    setErrorMsg(null);
    try {
      const inputs: Record<string, unknown> = {};
      for (const f of fields) {
        const raw = formValues[f.key];
        if (f.type === "number" || f.type === "integer") {
          const str =
            raw === undefined || raw === null ? "" : String(raw).trim();
          if (str === "") {
            if (f.required) {
              throw new Error(
                tx(`请填写「${f.title}」`, `${f.title} is required`)
              );
            }
            continue;
          }
          const num = Number(str);
          if (!Number.isFinite(num)) {
            throw new Error(
              tx(`「${f.title}」必须是有效数字`, `${f.title} must be numeric`)
            );
          }
          inputs[f.key] = f.type === "integer" ? Math.trunc(num) : num;
        } else if (f.type === "boolean") {
          inputs[f.key] = Boolean(raw);
        } else {
          const str = raw === undefined || raw === null ? "" : String(raw);
          if (f.required && str.trim() === "") {
            throw new Error(
              tx(`请填写「${f.title}」`, `${f.title} is required`)
            );
          }
          if (str !== "") {
            inputs[f.key] = str;
          }
        }
      }

      const idempotencyKey = `xv-${widget.id}-${Date.now()}`;
      const res = await xblueviewApi.invokeWidgetAction(widget.id, {
        inputs,
        target_node_id: resolvedNodeId,
        idempotency_key: idempotencyKey,
      });
      setLastResult(res);
      setConfirming(false);
      onActionSuccess?.();
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setSubmitting(false);
    }
  };

  const recentLogs = data?.rows ?? [];
  const logColMap = buildColumnIndexMap(data);
  const skipConfirm = readBoolean(widget.options, "skip_confirm") === true;
  const isCompactSingleField =
    !isButton && fields.length === 1 && fields[0]?.type !== "boolean";
  const singleField = isCompactSingleField ? fields[0] : null;

  return (
    <div
      className="h-full flex flex-col justify-between gap-1.5 text-xs"
      data-testid={`widget-action-${widget.id}`}
    >
      <div className="space-y-1.5">
        {/* Compact Target Summary */}
        <div className="flex flex-wrap items-center justify-between gap-1.5 text-[11px] text-muted-foreground">
          <div className="flex flex-wrap items-center gap-1.5 min-w-0">
            <Badge variant="xblue">{tEnum(ds.action_type)}</Badge>
            {(targetNodeLabel || ds.key) && (
              <span className="font-mono-tech truncate">
                {targetNodeLabel ? `${targetNodeLabel}` : ""}
                {targetNodeLabel && ds.key ? " · " : ""}
                {ds.key ? ds.key : ""}
              </span>
            )}
          </div>
          {singleField && (
            <span className="text-[11px] text-muted-foreground shrink-0">
              {singleField.title}
              {singleField.required && (
                <span className="text-xblue ml-0.5">*</span>
              )}
            </span>
          )}
        </div>

        {/* Single-field ultra-compact side-by-side form */}
        {singleField && (
          <form
            onSubmit={(e) => {
              e.preventDefault();
              void executeInvoke();
            }}
            className="flex items-center gap-1.5"
          >
            {singleField.enumValues && singleField.enumValues.length > 0 ? (
              <Select
                disabled={readOnly}
                value={String(formValues[singleField.key] ?? "")}
                onChange={(e) =>
                  updateFieldValue(singleField.key, e.target.value)
                }
                className="h-7 flex-1 min-w-0"
                data-testid={`action-input-${singleField.key}`}
              >
                <option value="">{tx("未选择", "Select")}</option>
                {singleField.enumValues.map((opt) => (
                  <option key={opt} value={opt}>
                    {opt}
                  </option>
                ))}
              </Select>
            ) : (
              <Input
                type={
                  singleField.type === "number" ||
                  singleField.type === "integer"
                    ? "number"
                    : "text"
                }
                step={singleField.type === "number" ? "any" : undefined}
                disabled={readOnly}
                value={String(formValues[singleField.key] ?? "")}
                onChange={(e) =>
                  updateFieldValue(singleField.key, e.target.value)
                }
                placeholder={
                  singleField.type === "number" ||
                  singleField.type === "integer"
                    ? "0"
                    : singleField.title
                }
                className="h-7 flex-1 min-w-0 text-xs font-mono-tech"
                data-testid={`action-input-${singleField.key}`}
              />
            )}
            <Button
              type="submit"
              size="sm"
              disabled={readOnly || submitting}
              className="h-7 px-2.5 shrink-0"
              data-testid={`action-submit-${widget.id}`}
            >
              <Send className="h-3 w-3" />
              {submitting ? tx("提交中…", "Saving…") : buttonLabel}
            </Button>
          </form>
        )}

        {/* Multi-field or boolean form */}
        {!isButton && !singleField && fields.length > 0 && (
          <form
            onSubmit={(e) => {
              e.preventDefault();
              void executeInvoke();
            }}
            className="space-y-1.5"
          >
            {fields.map((f) => (
              <div key={f.key} className="space-y-0.5">
                <label className="block text-[11px] text-muted-foreground">
                  {f.title}
                  {f.required && <span className="text-xblue ml-0.5">*</span>}
                </label>
                {f.type === "boolean" ? (
                  <label className="inline-flex items-center gap-2 cursor-pointer">
                    <input
                      type="checkbox"
                      disabled={readOnly}
                      checked={Boolean(formValues[f.key])}
                      onChange={(e) =>
                        updateFieldValue(f.key, e.target.checked)
                      }
                    />
                    <span>{tx("启用", "Enabled")}</span>
                  </label>
                ) : f.enumValues && f.enumValues.length > 0 ? (
                  <Select
                    disabled={readOnly}
                    value={String(formValues[f.key] ?? "")}
                    onChange={(e) => updateFieldValue(f.key, e.target.value)}
                    className="h-7 w-full"
                  >
                    <option value="">{tx("未选择", "Select")}</option>
                    {f.enumValues.map((opt) => (
                      <option key={opt} value={opt}>
                        {opt}
                      </option>
                    ))}
                  </Select>
                ) : (
                  <Input
                    type={
                      f.type === "number" || f.type === "integer"
                        ? "number"
                        : "text"
                    }
                    step={f.type === "number" ? "any" : undefined}
                    disabled={readOnly}
                    value={String(formValues[f.key] ?? "")}
                    onChange={(e) => updateFieldValue(f.key, e.target.value)}
                    placeholder={
                      f.type === "number" || f.type === "integer"
                        ? "0"
                        : f.title
                    }
                    className="h-7 text-xs font-mono-tech"
                    data-testid={`action-input-${f.key}`}
                  />
                )}
              </div>
            ))}

            <Button
              type="submit"
              size="sm"
              disabled={readOnly || submitting}
              className="w-full"
              data-testid={`action-submit-${widget.id}`}
            >
              <Send className="h-3 w-3" />
              {submitting ? tx("提交中…", "Saving…") : buttonLabel}
            </Button>
          </form>
        )}

        {/* Action Button / Zero-field mode */}
        {(isButton || fields.length === 0) && (
          <div className="space-y-1.5">
            {confirming && ds.confirm_text ? (
              <div className="rounded border border-xblue/40 bg-xblue-soft/50 p-2 space-y-1.5">
                <div className="text-xs font-medium">{ds.confirm_text}</div>
                <div className="flex items-center gap-2">
                  <Button
                    type="button"
                    size="sm"
                    disabled={readOnly || submitting}
                    onClick={() => void executeInvoke()}
                  >
                    {submitting
                      ? tx("执行中…", "Running…")
                      : tx("确认", "Confirm")}
                  </Button>
                  <Button
                    type="button"
                    size="sm"
                    variant="outline"
                    onClick={() => setConfirming(false)}
                  >
                    {tx("取消", "Cancel")}
                  </Button>
                </div>
              </div>
            ) : (
              <Button
                type="button"
                size="sm"
                disabled={readOnly || submitting}
                onClick={() => {
                  if (ds.confirm_text && !skipConfirm) {
                    setConfirming(true);
                  } else {
                    void executeInvoke();
                  }
                }}
                className="w-full"
                data-testid={`action-trigger-${widget.id}`}
              >
                <Play className="h-3 w-3" />
                {submitting ? tx("执行中…", "Running…") : buttonLabel}
              </Button>
            )}
          </div>
        )}

        {errorMsg && (
          <div className="rounded border border-red-500/40 bg-red-500/10 px-2 py-1 text-[11px] text-red-700 dark:text-red-300">
            {errorMsg}
          </div>
        )}

        {lastResult && (
          <div
            className="rounded border border-xblue/30 bg-xblue-soft/40 px-2 py-0.5 text-[11px] flex items-center justify-between"
            data-testid={`action-result-${widget.id}`}
          >
            <span className="inline-flex items-center gap-1 font-medium text-xblue">
              <CheckCircle2 className="h-3 w-3" />
              {tEnum(lastResult.status)} ({lastResult.status})
            </span>
            {lastResult.run_id && (
              <span className="font-mono-tech text-[10px] text-muted-foreground">
                run: {lastResult.run_id.slice(0, 8)}
              </span>
            )}
          </div>
        )}
      </div>

      {/* Compact Recent Execution Status */}
      {recentLogs.length > 0 && (
        <div className="border-t border-border/70 pt-1 space-y-1">
          {recentLogs.slice(0, 2).map((r, idx) => {
            const statusStr = String(
              getCell(r, logColMap, "status", 3) ?? "DONE"
            );
            const createdAtRaw = getCell(r, logColMap, "created_at", -1);
            const errRaw = getCell(r, logColMap, "error_message", -1);
            const createdStr =
              typeof createdAtRaw === "string" && createdAtRaw.trim()
                ? fmtTime(createdAtRaw, true)
                : "";
            const errStr =
              typeof errRaw === "string" && errRaw.trim() ? errRaw.trim() : "";

            return (
              <div
                key={idx}
                className="text-[10px] font-mono-tech bg-muted/35 rounded px-1.5 py-0.5 flex items-center justify-between gap-1.5"
              >
                <span className="text-muted-foreground truncate">
                  {errStr || createdStr || tx("最近执行", "Last run")}
                </span>
                <Badge variant="outline" className="px-1 py-0 text-[10px]">
                  {statusStr}
                </Badge>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}
