import { useEffect, useMemo, useState } from "react";
import { Braces, FormInput, Sparkles, Trash2 } from "lucide-react";
import { useI18n } from "@/i18n";
import { usePrincipal } from "@/lib/permissions";
import {
  computeDagAncestors,
  type BackoffStrategy,
  type FlowEdgeDef,
  type FlowNodeDef,
  type JoinMode,
  type JsonSchemaPropertySpec,
  type OnFailure,
  type OperatorSpecDto,
} from "@/lib/api/axflow";
import type { FlowOperator } from "@/lib/api/enums";
import {
  Badge,
  Button,
  EmptyState,
  Input,
  NumberInput,
  Select,
  Switch,
  Textarea,
} from "@/components/ui/primitives";
import { resolveOperatorDesc } from "./OperatorPalette";

function suggestedOutputPathsForOperator(kind: FlowOperator): string[] {
  switch (kind) {
    case "SOURCE_INGEST":
      return ["rows_written", "snapshot_id", "job_id"];
    case "SQL_TRANSFORM":
      return ["row_count", "rows", "dataset_id"];
    case "LLM_EXTRACT":
      return ["result"];
    case "XBLUE_MAP":
      return ["nodes_upserted", "links_upserted"];
    case "AGG_TRIGGER":
      return ["nodes_changed"];
    case "CAUSAL_SIM":
      return ["affected_count", "impacts_top"];
    case "HUMAN_APPROVAL":
      return ["approved", "comment", "decided_by"];
    case "ACTION_EMIT":
      return ["done", "dry_run", "failed"];
    case "HTTP_CALL":
      return ["status", "body"];
    case "AGENT_TASK":
      return ["answer"];
  }
}

function primaryJsonSchemaType(prop: JsonSchemaPropertySpec): string {
  if (Array.isArray(prop.type)) {
    return prop.type.find((t) => t !== "null") ?? "string";
  }
  return prop.type ?? "string";
}

export interface NodeConfigFormProps {
  node: FlowNodeDef | null;
  allNodes: FlowNodeDef[];
  allEdges: FlowEdgeDef[];
  operators: OperatorSpecDto[];
  selectedEdge: { index: number; edge: FlowEdgeDef } | null;
  onChangeNode: (updated: FlowNodeDef) => void;
  onDeleteNode: (nodeId: string) => void;
  onUpdateEdge: (index: number, updated: FlowEdgeDef) => void;
  onDeleteEdge: (index: number) => void;
}

export function NodeConfigForm({
  node,
  allNodes,
  allEdges,
  operators,
  selectedEdge,
  onChangeNode,
  onDeleteNode,
  onUpdateEdge,
  onDeleteEdge,
}: NodeConfigFormProps) {
  const { isZh, tx } = useI18n();
  const { can } = usePrincipal();
  const canEdit = can("FLOW_EDIT");

  const [rawJsonMode, setRawJsonMode] = useState(false);
  const [rawJsonDraft, setRawJsonDraft] = useState("{}");
  const [rawJsonError, setRawJsonError] = useState<string | null>(null);
  const [objectDrafts, setObjectDrafts] = useState<Record<string, string>>({});
  const [objectErrors, setObjectErrors] = useState<Record<string, string>>({});
  const [focusedFieldKey, setFocusedFieldKey] = useState<string | null>(null);

  useEffect(() => {
    if (!node) return;
    setRawJsonDraft(JSON.stringify(node.config, null, 2));
    setRawJsonError(null);
    setObjectDrafts({});
    setObjectErrors({});
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [node?.id]);

  const operatorSpec = useMemo(
    () => (node ? (operators.find((o) => o.kind === node.kind) ?? null) : null),
    [node, operators]
  );

  const ancestors = useMemo(
    () => (node ? computeDagAncestors(node.id, allNodes, allEdges) : []),
    [node, allNodes, allEdges]
  );

  const schemaProperties = useMemo(
    () => Object.entries(operatorSpec?.config_schema.properties ?? {}),
    [operatorSpec]
  );

  const requiredSet = useMemo(
    () => new Set(operatorSpec?.config_schema.required ?? []),
    [operatorSpec]
  );

  if (!node && selectedEdge) {
    const { index, edge } = selectedEdge;
    return (
      <div className="space-y-4 text-xs" data-testid="axflow-edge-config-form">
        <div className="flex items-center justify-between border-b border-border pb-2">
          <div>
            <div className="font-semibold">
              {tx("依赖边条件", "Dependency Condition")}
            </div>
            <div className="font-mono-tech text-[11px] text-xblue mt-0.5">
              {edge.from} → {edge.to}
            </div>
          </div>
          {canEdit && (
            <Button
              type="button"
              variant="danger"
              size="sm"
              onClick={() => onDeleteEdge(index)}
            >
              <Trash2 className="h-3.5 w-3.5" />
              {tx("删除边", "Delete")}
            </Button>
          )}
        </div>

        <div className="space-y-1.5">
          <label className="block font-medium">
            {tx("分支守卫条件", "Guard Expression")}
          </label>
          <Input
            data-testid="inspector-edge-when-input"
            value={edge.when ?? ""}
            disabled={!canEdit}
            onChange={(e) =>
              onUpdateEdge(index, {
                ...edge,
                when: e.target.value.length > 0 ? e.target.value : null,
              })
            }
            placeholder="n > 0"
            className="h-8 font-mono-tech text-xs"
          />
          <p className="text-[11px] text-muted-foreground">
            {tx(
              "按上游输出字段求值；不满足时下游按汇合策略跳过",
              "Evaluated against upstream output fields"
            )}
          </p>
        </div>

        {canEdit && (
          <div className="flex flex-wrap gap-1.5">
            {[
              "n > 0",
              "row_count > 0",
              "approved == true",
              "affected_count > 0",
            ].map((expr) => (
              <button
                key={expr}
                type="button"
                onClick={() => onUpdateEdge(index, { ...edge, when: expr })}
                className="rounded border border-border bg-muted px-2 py-0.5 font-mono-tech text-[10px] hover:border-xblue cursor-pointer"
              >
                {expr}
              </button>
            ))}
          </div>
        )}
      </div>
    );
  }

  if (!node) {
    return (
      <div data-testid="axflow-node-config-empty">
        <EmptyState
          title={tx("未选中算子节点或依赖边", "No step or edge selected")}
        />
      </div>
    );
  }

  const updateConfigField = (key: string, val: unknown) => {
    if (!canEdit) return;
    const nextCfg = { ...node.config };
    if (val === undefined) {
      delete nextCfg[key];
    } else {
      nextCfg[key] = val;
    }
    onChangeNode({ ...node, config: nextCfg });
    setRawJsonDraft(JSON.stringify(nextCfg, null, 2));
  };

  const insertTemplatePlaceholder = (token: string) => {
    if (!canEdit) return;
    if (rawJsonMode) {
      const next = `${rawJsonDraft}\n`
        ? rawJsonDraft.replace(/\}\s*$/, `  "_ref": "${token}"\n}`)
        : rawJsonDraft;
      setRawJsonDraft(next);
      return;
    }

    const stringKeys = schemaProperties
      .filter(([, p]) => primaryJsonSchemaType(p) === "string")
      .map(([k]) => k);
    const targetKey =
      focusedFieldKey && stringKeys.includes(focusedFieldKey)
        ? focusedFieldKey
        : stringKeys[0];

    if (targetKey) {
      const prev =
        typeof node.config[targetKey] === "string"
          ? (node.config[targetKey] as string)
          : "";
      const nextVal = prev ? `${prev} ${token}` : token;
      updateConfigField(targetKey, nextVal);
    }
  };

  const opDescription = operatorSpec
    ? resolveOperatorDesc(operatorSpec, isZh)
    : "";

  return (
    <div className="space-y-3.5 text-xs" data-testid="axflow-node-config-form">
      {/* Node Header */}
      <div className="flex items-start justify-between gap-2 border-b border-border pb-2.5">
        <div className="min-w-0">
          <div className="flex items-center gap-1.5">
            <span className="font-mono-tech font-semibold text-sm">
              {node.id}
            </span>
            <Badge variant="xblue">{node.kind}</Badge>
          </div>
          {opDescription && (
            <p className="text-[11px] text-muted-foreground mt-0.5">
              {opDescription}
            </p>
          )}
        </div>
        {canEdit && (
          <Button
            type="button"
            variant="danger"
            size="sm"
            data-testid="delete-node-btn"
            onClick={() => onDeleteNode(node.id)}
          >
            <Trash2 className="h-3 w-3" />
            {tx("删除", "Delete")}
          </Button>
        )}
      </div>

      {/* Execution & Resilience Controls */}
      <div className="grid grid-cols-2 gap-2.5">
        <div className="col-span-2">
          <label className="block font-medium mb-1">
            {tx("节点名称", "Step Name")}
          </label>
          <Input
            data-testid="node-label-input"
            value={node.label}
            disabled={!canEdit}
            onChange={(e) => onChangeNode({ ...node, label: e.target.value })}
            className="h-8 text-xs"
            placeholder={tx("同步订单数据", "Sync orders")}
          />
        </div>

        <div>
          <label className="block font-medium mb-1">
            {tx("超时上限 (秒)", "Timeout (s)")}
          </label>
          <NumberInput
            min={1}
            max={86400}
            step={30}
            disabled={!canEdit}
            value={node.timeout_s}
            onChange={(val) =>
              onChangeNode({
                ...node,
                timeout_s: Math.min(86400, Math.max(1, val || 600)),
              })
            }
          />
        </div>

        <div>
          <label className="block font-medium mb-1">
            {tx("失败策略", "Failure Policy")}
          </label>
          <Select
            value={node.on_failure}
            disabled={!canEdit}
            onChange={(e) =>
              onChangeNode({
                ...node,
                on_failure: e.target.value as OnFailure,
              })
            }
            className="h-8 w-full px-2 font-mono-tech"
          >
            <option value="FAIL_RUN">
              {tx("终止流程", "Abort Run")}
            </option>
            <option value="CONTINUE">
              {tx("继续分支", "Continue")}
            </option>
          </Select>
        </div>

        <div className="col-span-2">
          <label className="block font-medium mb-1">
            {tx("前驱汇合策略", "Join Policy")}
          </label>
          <Select
            value={node.join}
            disabled={!canEdit}
            onChange={(e) =>
              onChangeNode({
                ...node,
                join: e.target.value as JoinMode,
              })
            }
            className="h-8 w-full px-2"
          >
            <option value="ALL_SUCCESS">
              {tx("全部入边满足", "All Active")}
            </option>
            <option value="ANY_SUCCESS">
              {tx("任一入边满足", "Any Active")}
            </option>
            <option value="ALL_DONE">
              {tx("全部前驱终态", "All Settled")}
            </option>
          </Select>
        </div>
      </div>

      {/* Retry Policy (2-Column Balanced Grid, Zero Native Steppers) */}
      <div className="rounded-md border border-border bg-muted/20 p-2.5 space-y-2">
        <div className="font-medium text-[11px]">
          {tx("重试策略", "Retry Policy")}
        </div>
        <div className="grid grid-cols-2 gap-2">
          <div>
            <label className="block text-[10px] text-muted-foreground mb-0.5">
              {tx("最大尝试", "Max Retries")}
            </label>
            <NumberInput
              min={1}
              max={10}
              disabled={!canEdit}
              value={node.retry.max_attempts}
              onChange={(val) =>
                onChangeNode({
                  ...node,
                  retry: {
                    ...node.retry,
                    max_attempts: Math.min(10, Math.max(1, val || 1)),
                  },
                })
              }
            />
          </div>
          <div>
            <label className="block text-[10px] text-muted-foreground mb-0.5">
              {tx("退避间隔 (s)", "Interval (s)")}
            </label>
            <NumberInput
              min={1}
              max={3600}
              step={5}
              disabled={!canEdit}
              value={node.retry.backoff_s}
              onChange={(val) =>
                onChangeNode({
                  ...node,
                  retry: {
                    ...node.retry,
                    backoff_s: Math.min(3600, Math.max(1, val || 10)),
                  },
                })
              }
            />
          </div>
          <div className="col-span-2">
            <label className="block text-[10px] text-muted-foreground mb-0.5">
              {tx("退避方式", "Backoff")}
            </label>
            <Select
              value={node.retry.backoff}
              disabled={!canEdit}
              onChange={(e) =>
                onChangeNode({
                  ...node,
                  retry: {
                    ...node.retry,
                    backoff: e.target.value as BackoffStrategy,
                  },
                })
              }
              className="h-8 w-full px-2 font-mono-tech text-xs"
            >
              <option value="FIXED">{tx("固定间隔 (FIXED)", "FIXED")}</option>
              <option value="EXPONENTIAL">
                {tx("指数退避 (EXPONENTIAL)", "EXPONENTIAL")}
              </option>
            </Select>
          </div>
        </div>
      </div>

      {/* Operator Config Section */}
      <div className="space-y-2 pt-1 border-t border-border">
        <div className="flex items-center justify-between">
          <span className="font-semibold">
            {tx("算子参数", "Parameters")}
          </span>
          <Button
            type="button"
            variant="ghost"
            size="sm"
            className="h-6 px-2 text-[11px]"
            data-testid="toggle-config-json-btn"
            onClick={() => {
              if (!rawJsonMode) {
                setRawJsonDraft(JSON.stringify(node.config, null, 2));
                setRawJsonError(null);
              }
              setRawJsonMode((v) => !v);
            }}
          >
            {rawJsonMode ? (
              <>
                <FormInput className="h-3 w-3" />
                {tx("表单视图", "Form")}
              </>
            ) : (
              <>
                <Braces className="h-3 w-3" />
                {tx("JSON 视图", "JSON")}
              </>
            )}
          </Button>
        </div>

        {rawJsonMode || schemaProperties.length === 0 ? (
          <div className="space-y-1">
            <Textarea
              data-testid="node-config-raw-json"
              rows={7}
              value={rawJsonDraft}
              disabled={!canEdit}
              onChange={(e) => {
                const txt = e.target.value;
                setRawJsonDraft(txt);
                try {
                  const parsed: unknown = JSON.parse(txt);
                  if (
                    typeof parsed === "object" &&
                    parsed !== null &&
                    !Array.isArray(parsed)
                  ) {
                    setRawJsonError(null);
                    onChangeNode({
                      ...node,
                      config: parsed as Record<string, unknown>,
                    });
                  } else {
                    setRawJsonError(
                      tx("参数需为 JSON 对象", "Config must be JSON object")
                    );
                  }
                } catch (err) {
                  setRawJsonError(
                    err instanceof Error
                      ? err.message
                      : tx("JSON 语法错误", "Invalid JSON")
                  );
                }
              }}
              className="font-mono-tech text-xs"
            />
            {rawJsonError && (
              <div className="text-[11px] text-red-600 dark:text-red-400">
                {rawJsonError}
              </div>
            )}
          </div>
        ) : (
          <div className="space-y-2.5">
            {schemaProperties.map(([propKey, propSpec]) => {
              const pType = primaryJsonSchemaType(propSpec);
              const isRequired = requiredSet.has(propKey);
              const val = node.config[propKey] ?? propSpec.default;
              const fieldLabel = isZh
                ? (propSpec.title ?? propKey)
                : propKey;

              if (Array.isArray(propSpec.enum) && propSpec.enum.length > 0) {
                return (
                  <div key={propKey}>
                    <label className="block font-medium mb-1">
                      {fieldLabel}
                      {isRequired && <span className="text-xblue ml-0.5">*</span>}
                    </label>
                    <Select
                      data-testid={`config-field-${propKey}`}
                      value={String(val ?? propSpec.enum[0])}
                      disabled={!canEdit}
                      onChange={(e) => updateConfigField(propKey, e.target.value)}
                      className="h-8 w-full font-mono-tech"
                    >
                      {propSpec.enum.map((opt) => (
                        <option key={String(opt)} value={String(opt)}>
                          {String(opt)}
                        </option>
                      ))}
                    </Select>
                  </div>
                );
              }

              if (pType === "boolean") {
                return (
                  <div
                    key={propKey}
                    className="flex items-center justify-between rounded-md bg-muted/30 px-2.5 py-2"
                  >
                    <span>{fieldLabel}</span>
                    <Switch
                      data-testid={`config-field-${propKey}`}
                      checked={Boolean(val)}
                      disabled={!canEdit}
                      onCheckedChange={(checked) =>
                        updateConfigField(propKey, checked)
                      }
                      aria-label={fieldLabel}
                    />
                  </div>
                );
              }

              if (pType === "integer" || pType === "number") {
                return (
                  <div key={propKey}>
                    <label className="block font-medium mb-1">
                      {fieldLabel}
                      {isRequired && <span className="text-xblue ml-0.5">*</span>}
                    </label>
                    <NumberInput
                      data-testid={`config-field-${propKey}`}
                      min={propSpec.minimum}
                      max={propSpec.maximum}
                      step={pType === "integer" ? 1 : 0.1}
                      disabled={!canEdit}
                      value={typeof val === "number" ? val : 0}
                      onChange={(num) => {
                        const clean =
                          pType === "integer" ? Math.trunc(num) : num;
                        if (Number.isFinite(clean)) {
                          updateConfigField(propKey, clean);
                        }
                      }}
                    />
                  </div>
                );
              }

              if (pType === "object" || pType === "array") {
                const textValue =
                  objectDrafts[propKey] ??
                  JSON.stringify(
                    val ?? (pType === "array" ? [] : {}),
                    null,
                    2
                  );
                return (
                  <div key={propKey}>
                    <label className="block font-medium mb-1">
                      {fieldLabel}
                    </label>
                    <Textarea
                      data-testid={`config-field-${propKey}`}
                      rows={3}
                      value={textValue}
                      disabled={!canEdit}
                      onChange={(e) => {
                        const txt = e.target.value;
                        setObjectDrafts((prev) => ({
                          ...prev,
                          [propKey]: txt,
                        }));
                        try {
                          const parsed: unknown = JSON.parse(txt);
                          setObjectErrors((prev) => {
                            const copy = { ...prev };
                            delete copy[propKey];
                            return copy;
                          });
                          updateConfigField(propKey, parsed);
                        } catch {
                          setObjectErrors((prev) => ({
                            ...prev,
                            [propKey]: tx("无效 JSON", "Invalid JSON"),
                          }));
                        }
                      }}
                      className="font-mono-tech text-xs"
                    />
                    {objectErrors[propKey] && (
                      <div className="text-[10px] text-red-600 dark:text-red-400 mt-0.5">
                        {objectErrors[propKey]}
                      </div>
                    )}
                  </div>
                );
              }

              const isMultiline =
                propKey === "sql" || propKey === "instruction";
              return (
                <div key={propKey}>
                  <label className="block font-medium mb-1">
                    {fieldLabel}
                    {isRequired && <span className="text-xblue ml-0.5">*</span>}
                  </label>
                  {isMultiline ? (
                    <Textarea
                      data-testid={`config-field-${propKey}`}
                      rows={3}
                      value={typeof val === "string" ? val : ""}
                      disabled={!canEdit}
                      onFocus={() => setFocusedFieldKey(propKey)}
                      onChange={(e) =>
                        updateConfigField(propKey, e.target.value)
                      }
                      placeholder={isZh ? (propSpec.description ?? "") : propKey}
                      className="font-mono-tech text-xs"
                    />
                  ) : (
                    <Input
                      data-testid={`config-field-${propKey}`}
                      value={typeof val === "string" ? val : ""}
                      disabled={!canEdit}
                      onFocus={() => setFocusedFieldKey(propKey)}
                      onChange={(e) =>
                        updateConfigField(propKey, e.target.value)
                      }
                      placeholder={isZh ? (propSpec.description ?? "") : propKey}
                      className="h-8 font-mono-tech text-xs"
                    />
                  )}
                </div>
              );
            })}
          </div>
        )}
      </div>

      {/* Ancestor Output Template Helper */}
      <div
        className="rounded-md border border-border bg-muted/30 p-2.5 space-y-2"
        data-testid="ancestor-template-helper"
      >
        <div className="flex items-center gap-1 text-[11px] font-medium">
          <Sparkles className="h-3 w-3 text-xblue" />
          {tx("上游参数占位符", "Upstream Placeholders")}
        </div>
        {ancestors.length > 0 ? (
          <div className="space-y-1.5">
            {ancestors.map((anc) => {
              const fields = suggestedOutputPathsForOperator(anc.kind);
              return (
                <div key={anc.id} className="space-y-1">
                  <div className="font-mono-tech text-[10px] text-muted-foreground">
                    {anc.id} ({anc.kind})
                  </div>
                  <div className="flex flex-wrap gap-1">
                    {fields.map((f) => {
                      const token = `{{ nodes.${anc.id}.output.${f} }}`;
                      return (
                        <button
                          key={token}
                          type="button"
                          disabled={!canEdit}
                          onClick={() => insertTemplatePlaceholder(token)}
                          className="rounded border border-border bg-card px-1.5 py-0.5 font-mono-tech text-[10px] hover:border-xblue cursor-pointer disabled:opacity-50"
                        >
                          {token}
                        </button>
                      );
                    })}
                  </div>
                </div>
              );
            })}
          </div>
        ) : (
          <div className="text-[10px] text-muted-foreground">
            {tx("根节点上下文变量:", "Root context variables:")}
          </div>
        )}

        <div className="flex flex-wrap gap-1 pt-1 border-t border-border/60">
          {[
            "{{ trigger.payload }}",
            "{{ run.id }}",
            "{{ run.environment }}",
            "{{ flow.version }}",
          ].map((ctxToken) => (
            <button
              key={ctxToken}
              type="button"
              disabled={!canEdit}
              onClick={() => insertTemplatePlaceholder(ctxToken)}
              className="rounded border border-border bg-card px-1.5 py-0.5 font-mono-tech text-[10px] text-muted-foreground hover:text-foreground hover:border-xblue cursor-pointer disabled:opacity-50"
            >
              {ctxToken}
            </button>
          ))}
        </div>
      </div>
    </div>
  );
}
