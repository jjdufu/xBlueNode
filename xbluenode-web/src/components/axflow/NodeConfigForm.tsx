import { useEffect, useMemo, useState } from "react";
import { Braces, FormInput, Sparkles, Trash2 } from "lucide-react";
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
import { Badge, Button, Input, Textarea } from "@/components/ui/primitives";

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
  }, [node?.id]);

  const operatorSpec = useMemo(
    () => (node ? operators.find((o) => o.kind === node.kind) ?? null : null),
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
            <div className="font-semibold">依赖边条件配置</div>
            <div className="font-mono-tech text-[11px] text-xblue mt-0.5">
              {edge.from} → {edge.to}
            </div>
          </div>
          <Button
            type="button"
            variant="outline"
            className="h-7 px-2.5 text-xs"
            onClick={() => onDeleteEdge(index)}
          >
            <Trash2 className="h-3.5 w-3.5" />
            删除边
          </Button>
        </div>

        <div className="space-y-1.5">
          <label className="block font-medium">
            条件守卫表达式 (when)
          </label>
          <Input
            data-testid="inspector-edge-when-input"
            value={edge.when ?? ""}
            onChange={(e) =>
              onUpdateEdge(index, {
                ...edge,
                when: e.target.value.length > 0 ? e.target.value : null,
              })
            }
            placeholder="例如: n > 0 或 row_count >= 1"
            className="h-8 font-mono-tech text-xs"
          />
          <p className="text-[11px] text-muted-foreground">
            基于上游节点输出的顶层字段求值；表达式为假时下游按汇合策略置 SKIPPED。
          </p>
        </div>

        <div className="flex flex-wrap gap-1.5">
          {["n > 0", "row_count > 0", "approved == true", "affected_count > 0"].map(
            (expr) => (
              <button
                key={expr}
                type="button"
                onClick={() => onUpdateEdge(index, { ...edge, when: expr })}
                className="rounded border border-border bg-muted px-2 py-0.5 font-mono-tech text-[10px] hover:border-xblue cursor-pointer"
              >
                {expr}
              </button>
            )
          )}
        </div>
      </div>
    );
  }

  if (!node) {
    return (
      <div
        className="py-10 text-center text-xs text-muted-foreground space-y-1"
        data-testid="axflow-node-config-empty"
      >
        <div>请在画布中点击任意算子节点或依赖边进行配置</div>
        <div className="text-[11px]">
          或从左侧算子库添加新节点至工作流
        </div>
      </div>
    );
  }

  const updateConfigField = (key: string, val: unknown) => {
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
          {operatorSpec?.description && (
            <p className="text-[11px] text-muted-foreground mt-0.5">
              {operatorSpec.description}
            </p>
          )}
        </div>
        <Button
          type="button"
          variant="outline"
          className="h-7 px-2 text-[11px] shrink-0"
          data-testid="delete-node-btn"
          onClick={() => onDeleteNode(node.id)}
        >
          <Trash2 className="h-3 w-3" />
          删除
        </Button>
      </div>

      {/* Execution & Resilience Controls */}
      <div className="grid grid-cols-2 gap-2">
        <div className="col-span-2">
          <label className="block font-medium mb-1">节点名称 (label)</label>
          <Input
            data-testid="node-label-input"
            value={node.label}
            onChange={(e) => onChangeNode({ ...node, label: e.target.value })}
            className="h-8 text-xs"
            placeholder="节点显示名称"
          />
        </div>

        <div>
          <label className="block font-medium mb-1">单次超时 (秒)</label>
          <Input
            type="number"
            min={1}
            max={86400}
            value={node.timeout_s}
            onChange={(e) =>
              onChangeNode({
                ...node,
                timeout_s: Math.min(
                  86400,
                  Math.max(1, Number(e.target.value) || 600)
                ),
              })
            }
            className="h-8 font-mono-tech text-xs"
          />
        </div>

        <div>
          <label className="block font-medium mb-1">失败策略 (on_failure)</label>
          <select
            value={node.on_failure}
            onChange={(e) =>
              onChangeNode({
                ...node,
                on_failure: e.target.value as OnFailure,
              })
            }
            className="h-8 w-full rounded-md border border-input bg-background px-2 font-mono-tech text-xs"
          >
            <option value="FAIL_RUN">FAIL_RUN (立即终止流程)</option>
            <option value="CONTINUE">CONTINUE (允许独立分支继续)</option>
          </select>
        </div>

        <div className="col-span-2">
          <label className="block font-medium mb-1">前驱汇合语义 (join)</label>
          <select
            value={node.join}
            onChange={(e) =>
              onChangeNode({
                ...node,
                join: e.target.value as JoinMode,
              })
            }
            className="h-8 w-full rounded-md border border-input bg-background px-2 font-mono-tech text-xs"
          >
            <option value="ALL_SUCCESS">ALL_SUCCESS (全部入边激活)</option>
            <option value="ANY_SUCCESS">ANY_SUCCESS (任一入边激活)</option>
            <option value="ALL_DONE">ALL_DONE (全部前驱终态即执行)</option>
          </select>
        </div>
      </div>

      {/* Retry Policy */}
      <div className="rounded-md border border-border bg-muted/20 p-2.5 space-y-2">
        <div className="font-medium text-[11px]">重试退避策略 (retry)</div>
        <div className="grid grid-cols-3 gap-1.5">
          <div>
            <label className="block text-[10px] text-muted-foreground mb-0.5">
              最大尝试 (1-10)
            </label>
            <Input
              type="number"
              min={1}
              max={10}
              value={node.retry.max_attempts}
              onChange={(e) =>
                onChangeNode({
                  ...node,
                  retry: {
                    ...node.retry,
                    max_attempts: Math.min(
                      10,
                      Math.max(1, Number(e.target.value) || 1)
                    ),
                  },
                })
              }
              className="h-7 px-2 font-mono-tech text-xs"
            />
          </div>
          <div>
            <label className="block text-[10px] text-muted-foreground mb-0.5">
              初始间隔(s)
            </label>
            <Input
              type="number"
              min={1}
              max={3600}
              value={node.retry.backoff_s}
              onChange={(e) =>
                onChangeNode({
                  ...node,
                  retry: {
                    ...node.retry,
                    backoff_s: Math.min(
                      3600,
                      Math.max(1, Number(e.target.value) || 10)
                    ),
                  },
                })
              }
              className="h-7 px-2 font-mono-tech text-xs"
            />
          </div>
          <div>
            <label className="block text-[10px] text-muted-foreground mb-0.5">
              退避曲线
            </label>
            <select
              value={node.retry.backoff}
              onChange={(e) =>
                onChangeNode({
                  ...node,
                  retry: {
                    ...node.retry,
                    backoff: e.target.value as BackoffStrategy,
                  },
                })
              }
              className="h-7 w-full rounded-md border border-input bg-background px-1.5 font-mono-tech text-[11px]"
            >
              <option value="FIXED">FIXED</option>
              <option value="EXPONENTIAL">EXPONENTIAL</option>
            </select>
          </div>
        </div>
      </div>

      {/* Operator Config Section (JSON-Schema Form + Raw JSON Toggle) */}
      <div className="space-y-2 pt-1 border-t border-border">
        <div className="flex items-center justify-between">
          <span className="font-semibold">算子参数 (config)</span>
          <Button
            type="button"
            variant="ghost"
            className="h-6 px-2 py-0 text-[11px]"
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
                表单模式
              </>
            ) : (
              <>
                <Braces className="h-3 w-3" />
                JSON 源码
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
                    setRawJsonError("算子 config 必须是 JSON 对象");
                  }
                } catch (err) {
                  setRawJsonError(
                    err instanceof Error ? err.message : "JSON 语法错误"
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

              if (Array.isArray(propSpec.enum) && propSpec.enum.length > 0) {
                return (
                  <div key={propKey}>
                    <label className="block font-medium mb-1">
                      {propSpec.title ?? propKey}
                      {isRequired && <span className="text-xblue ml-0.5">*</span>}
                    </label>
                    <select
                      data-testid={`config-field-${propKey}`}
                      value={String(val ?? propSpec.enum[0])}
                      onChange={(e) => updateConfigField(propKey, e.target.value)}
                      className="h-8 w-full rounded-md border border-input bg-background px-2 font-mono-tech text-xs"
                    >
                      {propSpec.enum.map((opt) => (
                        <option key={String(opt)} value={String(opt)}>
                          {String(opt)}
                        </option>
                      ))}
                    </select>
                  </div>
                );
              }

              if (pType === "boolean") {
                return (
                  <label
                    key={propKey}
                    className="flex items-center justify-between rounded border border-border bg-muted/20 px-2.5 py-1.5 cursor-pointer"
                  >
                    <span>{propSpec.title ?? propKey}</span>
                    <input
                      data-testid={`config-field-${propKey}`}
                      type="checkbox"
                      checked={Boolean(val)}
                      onChange={(e) =>
                        updateConfigField(propKey, e.target.checked)
                      }
                      className="h-3.5 w-3.5 accent-blue-600"
                    />
                  </label>
                );
              }

              if (pType === "integer" || pType === "number") {
                return (
                  <div key={propKey}>
                    <label className="block font-medium mb-1">
                      {propSpec.title ?? propKey}
                      {isRequired && <span className="text-xblue ml-0.5">*</span>}
                    </label>
                    <Input
                      data-testid={`config-field-${propKey}`}
                      type="number"
                      min={propSpec.minimum}
                      max={propSpec.maximum}
                      value={typeof val === "number" ? val : ""}
                      onChange={(e) => {
                        const rawStr = e.target.value;
                        if (rawStr === "") {
                          updateConfigField(propKey, undefined);
                        } else {
                          const num =
                            pType === "integer"
                              ? Math.trunc(Number(rawStr))
                              : Number(rawStr);
                          if (Number.isFinite(num)) {
                            updateConfigField(propKey, num);
                          }
                        }
                      }}
                      className="h-8 font-mono-tech text-xs"
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
                      {propSpec.title ?? propKey}
                      <span className="ml-1 font-mono-tech text-[10px] text-muted-foreground">
                        ({pType})
                      </span>
                    </label>
                    <Textarea
                      data-testid={`config-field-${propKey}`}
                      rows={3}
                      value={textValue}
                      onChange={(e) => {
                        const txt = e.target.value;
                        setObjectDrafts((prev) => ({ ...prev, [propKey]: txt }));
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
                            [propKey]: "无效 JSON",
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
                    {propSpec.title ?? propKey}
                    {isRequired && <span className="text-xblue ml-0.5">*</span>}
                  </label>
                  {isMultiline ? (
                    <Textarea
                      data-testid={`config-field-${propKey}`}
                      rows={3}
                      value={typeof val === "string" ? val : ""}
                      onFocus={() => setFocusedFieldKey(propKey)}
                      onChange={(e) =>
                        updateConfigField(propKey, e.target.value)
                      }
                      placeholder={propSpec.description ?? ""}
                      className="font-mono-tech text-xs"
                    />
                  ) : (
                    <Input
                      data-testid={`config-field-${propKey}`}
                      value={typeof val === "string" ? val : ""}
                      onFocus={() => setFocusedFieldKey(propKey)}
                      onChange={(e) =>
                        updateConfigField(propKey, e.target.value)
                      }
                      placeholder={propSpec.description ?? ""}
                      className="h-8 font-mono-tech text-xs"
                    />
                  )}
                </div>
              );
            })}
          </div>
        )}
      </div>

      {/* Ancestor Output Template Helper (§07.2.1) */}
      <div
        className="rounded-md border border-border bg-muted/30 p-2.5 space-y-2"
        data-testid="ancestor-template-helper"
      >
        <div className="flex items-center gap-1 text-[11px] font-medium">
          <Sparkles className="h-3 w-3 text-xblue" />
          参数模板占位符（严格祖先节点输出）
        </div>
        {ancestors.length > 0 ? (
          <div className="space-y-1.5">
            {ancestors.map((anc) => {
              const fields = suggestedOutputPathsForOperator(anc.kind);
              return (
                <div key={anc.id} className="space-y-1">
                  <div className="font-mono-tech text-[10px] text-muted-foreground">
                    上游 {anc.id} ({anc.kind})
                  </div>
                  <div className="flex flex-wrap gap-1">
                    {fields.map((f) => {
                      const token = `{{ nodes.${anc.id}.output.${f} }}`;
                      return (
                        <button
                          key={token}
                          type="button"
                          onClick={() => insertTemplatePlaceholder(token)}
                          className="rounded border border-border bg-card px-1.5 py-0.5 font-mono-tech text-[10px] hover:border-xblue cursor-pointer"
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
            当前节点为根节点（无上游祖先），可引用触发载荷与运行上下文：
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
              onClick={() => insertTemplatePlaceholder(ctxToken)}
              className="rounded border border-border bg-card px-1.5 py-0.5 font-mono-tech text-[10px] text-muted-foreground hover:text-foreground hover:border-xblue cursor-pointer"
            >
              {ctxToken}
            </button>
          ))}
        </div>
      </div>
    </div>
  );
}
