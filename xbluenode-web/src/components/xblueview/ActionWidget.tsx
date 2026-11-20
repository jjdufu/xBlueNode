import { useMemo, useState } from "react";
import { CheckCircle2, Play, Send } from "lucide-react";
import {
  xblueviewApi,
  type InvokeWidgetActionResponse,
  type WidgetDataResponse,
  type WidgetRecord,
} from "@/lib/api/xblueview";
import { Badge, Button, Input } from "@/components/ui/primitives";
import { isRecord, readBoolean, readString } from "./utils";

export interface ActionWidgetProps {
  widget: WidgetRecord;
  data: WidgetDataResponse | null;
  readOnly?: boolean;
  selectedNodeId?: string | null;
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
    ? schema.required.filter((item): item is string => typeof item === "string")
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

export function ActionWidget({
  widget,
  data,
  readOnly = false,
  selectedNodeId,
  onActionSuccess,
}: ActionWidgetProps) {
  const ds = widget.data_source.kind === "ACTION" ? widget.data_source : null;
  const fields = useMemo(
    () => parseInputSchemaFields(ds?.input_schema),
    [ds?.input_schema]
  );

  const [formValues, setFormValues] = useState<Record<string, unknown>>(() => {
    const initial: Record<string, unknown> = {};
    for (const f of fields) {
      if (f.defaultValue !== undefined) {
        initial[f.key] = f.defaultValue;
      } else if (f.type === "boolean") {
        initial[f.key] = false;
      } else {
        initial[f.key] = "";
      }
    }
    return initial;
  });

  const [confirming, setConfirming] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [lastResult, setLastResult] =
    useState<InvokeWidgetActionResponse | null>(null);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  if (!ds) {
    return (
      <div className="h-full flex items-center justify-center text-xs text-muted-foreground">
        动作数据源配置缺失
      </div>
    );
  }

  const isButton = widget.widget_kind === "ACTION_BUTTON";
  const buttonLabel =
    readString(widget.options, "button_label") ??
    (ds.action_type === "RUN_FLOW" ? "触发业务流程" : "执行节点回写");

  const executeInvoke = async () => {
    if (readOnly) return;
    setSubmitting(true);
    setErrorMsg(null);
    try {
      const inputs: Record<string, unknown> = {};
      for (const f of fields) {
        const raw = formValues[f.key];
        if (f.type === "number" || f.type === "integer") {
          const str = raw === undefined || raw === null ? "" : String(raw).trim();
          if (str === "") {
            if (f.required) {
              throw new Error(`请填写必填数值参数「${f.title}」`);
            }
            continue;
          }
          const num = Number(str);
          if (!Number.isFinite(num)) {
            throw new Error(`参数「${f.title}」必须是有效数字`);
          }
          inputs[f.key] = f.type === "integer" ? Math.trunc(num) : num;
        } else if (f.type === "boolean") {
          inputs[f.key] = Boolean(raw);
        } else {
          const str = raw === undefined || raw === null ? "" : String(raw);
          if (f.required && str.trim() === "") {
            throw new Error(`请填写必填参数「${f.title}」`);
          }
          if (str !== "") {
            inputs[f.key] = str;
          }
        }
      }

      const idempotencyKey = `xv-${widget.id}-${Date.now()}`;
      const res = await xblueviewApi.invokeWidgetAction(widget.id, {
        inputs,
        target_node_id: ds.node_id ?? selectedNodeId ?? undefined,
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
  const skipConfirm = readBoolean(widget.options, "skip_confirm") === true;

  return (
    <div
      className="h-full flex flex-col justify-between gap-2 text-xs overflow-y-auto"
      data-testid={`widget-action-${widget.id}`}
    >
      <div className="space-y-2">
        <div className="flex flex-wrap items-center gap-1.5 text-[11px] text-muted-foreground">
          <Badge variant="xblue">{ds.action_type}</Badge>
          {ds.action_type === "WRITE_NODE" && ds.key && (
            <span className="font-mono-tech">
              目标: {ds.target_kind ?? "METRIC"}.{ds.key}
            </span>
          )}
        </div>

        {!isButton && fields.length > 0 && (
          <form
            onSubmit={(e) => {
              e.preventDefault();
              void executeInvoke();
            }}
            className="space-y-2"
          >
            {fields.map((f) => (
              <div key={f.key} className="space-y-1">
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
                        setFormValues((prev) => ({
                          ...prev,
                          [f.key]: e.target.checked,
                        }))
                      }
                    />
                    <span>是 / 启用</span>
                  </label>
                ) : f.enumValues && f.enumValues.length > 0 ? (
                  <select
                    disabled={readOnly}
                    value={String(formValues[f.key] ?? "")}
                    onChange={(e) =>
                      setFormValues((prev) => ({
                        ...prev,
                        [f.key]: e.target.value,
                      }))
                    }
                    className="h-8 w-full rounded-md border border-input bg-background px-2 text-xs"
                  >
                    <option value="">请选择</option>
                    {f.enumValues.map((opt) => (
                      <option key={opt} value={opt}>
                        {opt}
                      </option>
                    ))}
                  </select>
                ) : (
                  <Input
                    type={f.type === "number" || f.type === "integer" ? "number" : "text"}
                    step={f.type === "number" ? "any" : undefined}
                    disabled={readOnly}
                    value={String(formValues[f.key] ?? "")}
                    onChange={(e) =>
                      setFormValues((prev) => ({
                        ...prev,
                        [f.key]: e.target.value,
                      }))
                    }
                    placeholder={`输入 ${f.title}`}
                    className="h-8 text-xs font-mono-tech"
                    data-testid={`action-input-${f.key}`}
                  />
                )}
              </div>
            ))}

            <Button
              type="submit"
              disabled={readOnly || submitting}
              className="w-full"
              data-testid={`action-submit-${widget.id}`}
            >
              <Send className="h-3.5 w-3.5" />
              {submitting ? "提交执行中..." : buttonLabel}
            </Button>
          </form>
        )}

        {(isButton || fields.length === 0) && (
          <div className="space-y-2">
            {confirming && ds.confirm_text ? (
              <div className="rounded border border-xblue/40 bg-xblue-soft/50 p-2 space-y-2">
                <div className="text-xs font-medium">{ds.confirm_text}</div>
                <div className="flex items-center gap-2">
                  <Button
                    type="button"
                    disabled={readOnly || submitting}
                    onClick={() => void executeInvoke()}
                    className="px-2.5 py-1 text-xs"
                  >
                    {submitting ? "执行中..." : "确认执行"}
                  </Button>
                  <Button
                    type="button"
                    variant="outline"
                    onClick={() => setConfirming(false)}
                    className="px-2.5 py-1 text-xs"
                  >
                    取消
                  </Button>
                </div>
              </div>
            ) : (
              <Button
                type="button"
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
                <Play className="h-3.5 w-3.5" />
                {submitting ? "执行中..." : buttonLabel}
              </Button>
            )}
          </div>
        )}

        {errorMsg && (
          <div className="rounded border border-foreground/30 bg-muted px-2 py-1 text-[11px]">
            {errorMsg}
          </div>
        )}

        {lastResult && (
          <div
            className="rounded border border-xblue/30 bg-xblue-soft/40 px-2 py-1 text-[11px] flex items-center justify-between"
            data-testid={`action-result-${widget.id}`}
          >
            <span className="inline-flex items-center gap-1 font-medium text-xblue">
              <CheckCircle2 className="h-3 w-3" />
              已执行 ({lastResult.status})
            </span>
            {lastResult.run_id && (
              <span className="font-mono-tech text-[10px] text-muted-foreground">
                run: {lastResult.run_id.slice(0, 8)}
              </span>
            )}
          </div>
        )}
      </div>

      {/* Recent Execution Logs */}
      {recentLogs.length > 0 && (
        <div className="border-t border-border pt-1.5 space-y-1">
          <div className="text-[10px] text-muted-foreground font-mono-tech">
            最近执行记录 ({recentLogs.length})
          </div>
          <div className="space-y-1 max-h-20 overflow-y-auto">
            {recentLogs.slice(0, 5).map((r, idx) => (
              <div
                key={idx}
                className="flex items-center justify-between text-[10px] font-mono-tech bg-muted/40 rounded px-1.5 py-0.5"
              >
                <span className="truncate">{String(r[1] ?? r[0] ?? "ACTION")}</span>
                <Badge variant="outline">{String(r[2] ?? "DONE")}</Badge>
              </div>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}
