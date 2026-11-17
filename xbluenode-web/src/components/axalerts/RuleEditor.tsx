import { useMemo, useState } from "react";
import {
  Edit3,
  FlaskConical,
  GitBranch,
  Plus,
  Power,
  RotateCcw,
  Save,
  Trash2,
} from "lucide-react";
import {
  formatConditionSummary,
  type ComparisonOp,
  type Condition,
  type ConditionKind,
  type CreateRuleRequest,
  type FlowRecord,
  type NotifyChannelRecord,
  type OutboxTopic,
  type RuleDef,
  type RuleDryRunResponse,
  type RuleQualityMinSeverity,
  type RuleScopeKind,
  type Severity,
  type UpdateRuleRequest,
} from "@/lib/api";
import type { Dataset } from "@/lib/api/axhub";
import { Badge, Button, Card, Input, Textarea } from "@/components/ui/primitives";
import { cn } from "@/lib/utils";
import { getSeverityBadgeClass } from "./AlertTable";

export interface RuleEditorProps {
  rules: readonly RuleDef[];
  channels: readonly NotifyChannelRecord[];
  flows: readonly FlowRecord[];
  datasets: readonly Dataset[];
  knownTypeNames: readonly string[];
  saving: boolean;
  dryRunningId: string | null;
  dryRunResult: RuleDryRunResponse | null;
  onCreateRule: (req: CreateRuleRequest) => Promise<void>;
  onUpdateRule: (id: string, req: UpdateRuleRequest) => Promise<void>;
  onDeleteRule: (id: string) => Promise<void>;
  onDryRunSaved: (id: string) => Promise<void>;
  onDryRunAdhoc: (req: CreateRuleRequest, editingRuleId: string | null) => Promise<void>;
}

const TEMPLATE_CHIPS: readonly { token: string; label: string }[] = [
  { token: "{{rule.name}}", label: "{{rule.name}}" },
  { token: "{{node.title}}", label: "{{node.title}}" },
  { token: "{{node.type_name}}", label: "{{node.type_name}}" },
  { token: "{{metric}}", label: "{{metric}}" },
  { token: "{{value}}", label: "{{value}}" },
  { token: "{{threshold}}", label: "{{threshold}}" },
  { token: "{{severity}}", label: "{{severity}}" },
  { token: "{{dataset.name}}", label: "{{dataset.name}}" },
  { token: "{{link}}", label: "{{link}}" },
];

const OUTBOX_TOPICS: readonly OutboxTopic[] = [
  "dataset.synced",
  "node.metrics_changed",
  "node.upserted",
  "approval.decided",
  "sim.applied",
  "alert.opened",
  "alert.resolved",
  "export.finished",
];

const COMPARISON_OPS: readonly { value: ComparisonOp; label: string }[] = [
  { value: "LT", label: "LT (< 小于)" },
  { value: "LTE", label: "LTE (<= 小于等于)" },
  { value: "GT", label: "GT (> 大于)" },
  { value: "GTE", label: "GTE (>= 大于等于)" },
  { value: "EQ", label: "EQ (== 等于)" },
  { value: "NE", label: "NE (!= 不等于)" },
];

function buildDefaultCondition(kind: ConditionKind): Condition {
  switch (kind) {
    case "THRESHOLD":
      return { kind: "THRESHOLD", op: "LT", value: 50, for_minutes: 0 };
    case "CHANGE_RATE":
      return {
        kind: "CHANGE_RATE",
        window_minutes: 15,
        op: "GT",
        pct: 0.2,
      };
    case "ABSENCE":
      return { kind: "ABSENCE", minutes: 10 };
    case "ANOMALY_ZSCORE":
      return {
        kind: "ANOMALY_ZSCORE",
        lookback_points: 96,
        z: 3.0,
        min_points: 30,
      };
    case "EXPR":
      return {
        kind: "EXPR",
        expression: "inventory < total_demand",
      };
    case "QUALITY_FAILED":
      return {
        kind: "QUALITY_FAILED",
        min_severity: "WARN",
      };
    case "EVENT_MATCH":
      return {
        kind: "EVENT_MATCH",
      };
  }
}

export function RuleEditor({
  rules,
  channels,
  flows,
  datasets,
  knownTypeNames,
  saving,
  dryRunningId,
  dryRunResult,
  onCreateRule,
  onUpdateRule,
  onDeleteRule,
  onDryRunSaved,
  onDryRunAdhoc,
}: RuleEditorProps) {
  const [editingRuleId, setEditingRuleId] = useState<string | null>(null);
  const [name, setName] = useState("low_inventory_guard");
  const [description, setDescription] = useState("核心仓储库存跌破安全水位告警");
  const [scopeKind, setScopeKind] = useState<RuleScopeKind>("METRIC");

  // Scope target fields
  const [targetTypeName, setTargetTypeName] = useState("Warehouse");
  const [targetMetric, setTargetMetric] = useState("inventory");
  const [targetNodeFilter, setTargetNodeFilter] = useState("");
  const [targetDatasetId, setTargetDatasetId] = useState("");
  const [targetTopic, setTargetTopic] = useState<string>("dataset.synced");
  const [targetEventFilter, setTargetEventFilter] = useState("");

  // Condition state
  const [condition, setCondition] = useState<Condition>(() =>
    buildDefaultCondition("THRESHOLD")
  );
  const [severity, setSeverity] = useState<Severity>("CRITICAL");

  // Templates & active template target
  const [titleTemplate, setTitleTemplate] = useState(
    "[{{severity}}] {{node.title}} 指标 {{metric}}={{value}} (阈值 {{threshold}})"
  );
  const [bodyTemplate, setBodyTemplate] = useState(
    "规则 {{rule.name}} 命中：对象 {{node.title}} 当前值 {{value}} 触发告警。"
  );
  const [activeTemplateField, setActiveTemplateField] = useState<
    "title" | "body"
  >("title");

  // Lifecycle & linkage
  const [cooldownMinutes, setCooldownMinutes] = useState(30);
  const [resolveAfterMinutes, setResolveAfterMinutes] = useState(10);
  const [escalateAfterInput, setEscalateAfterInput] = useState("");
  const [selectedChannelIds, setSelectedChannelIds] = useState<string[]>([]);
  const [selectedEscalationIds, setSelectedEscalationIds] = useState<string[]>(
    []
  );
  const [runFlowId, setRunFlowId] = useState("");
  const [enabled, setEnabled] = useState(true);
  const [formValidationError, setFormValidationError] = useState<string | null>(
    null
  );

  const flowNameMap = useMemo(() => {
    const map = new Map<string, string>();
    for (const f of flows) {
      map.set(f.id, f.name);
    }
    return map;
  }, [flows]);

  const resetForm = () => {
    setEditingRuleId(null);
    setName("");
    setDescription("");
    setScopeKind("METRIC");
    setTargetTypeName("Warehouse");
    setTargetMetric("inventory");
    setTargetNodeFilter("");
    setTargetDatasetId("");
    setTargetTopic("dataset.synced");
    setTargetEventFilter("");
    setCondition(buildDefaultCondition("THRESHOLD"));
    setSeverity("WARN");
    setTitleTemplate(
      "[{{severity}}] {{rule.name}} · {{node.title}} ({{value}})"
    );
    setBodyTemplate("规则 {{rule.name}} 触发告警，当前值 {{value}}。");
    setCooldownMinutes(30);
    setResolveAfterMinutes(10);
    setEscalateAfterInput("");
    setSelectedChannelIds([]);
    setSelectedEscalationIds([]);
    setRunFlowId("");
    setEnabled(true);
    setFormValidationError(null);
  };

  const loadRuleIntoEditor = (rule: RuleDef) => {
    setEditingRuleId(rule.id);
    setName(rule.name);
    setDescription(rule.description);
    setScopeKind(rule.scope_kind);
    if (rule.scope_kind === "METRIC") {
      setTargetTypeName(
        typeof rule.target.type_name === "string" ? rule.target.type_name : ""
      );
      setTargetMetric(
        typeof rule.target.metric === "string" ? rule.target.metric : ""
      );
      setTargetNodeFilter(
        typeof rule.target.node_filter === "string"
          ? rule.target.node_filter
          : ""
      );
    } else if (rule.scope_kind === "DATASET_QUALITY") {
      setTargetDatasetId(
        typeof rule.target.dataset_id === "string" ? rule.target.dataset_id : ""
      );
    } else {
      setTargetTopic(
        typeof rule.target.topic === "string"
          ? rule.target.topic
          : "dataset.synced"
      );
      setTargetEventFilter(
        typeof rule.target.filter === "string" ? rule.target.filter : ""
      );
    }
    setCondition(rule.condition);
    setSeverity(rule.severity);
    setTitleTemplate(rule.title_template);
    setBodyTemplate(rule.body_template);
    setCooldownMinutes(rule.cooldown_minutes);
    setResolveAfterMinutes(rule.resolve_after_minutes);
    setEscalateAfterInput(
      rule.escalate_after_minutes !== null
        ? String(rule.escalate_after_minutes)
        : ""
    );
    setSelectedChannelIds([...rule.channel_ids]);
    setSelectedEscalationIds([...rule.escalation_channel_ids]);
    setRunFlowId(rule.run_flow_id ?? "");
    setEnabled(rule.enabled);
    setFormValidationError(null);
  };

  const handleScopeChange = (nextScope: RuleScopeKind) => {
    setScopeKind(nextScope);
    if (nextScope === "DATASET_QUALITY") {
      setCondition(buildDefaultCondition("QUALITY_FAILED"));
    } else if (nextScope === "EVENT") {
      setCondition(buildDefaultCondition("EVENT_MATCH"));
    } else if (
      condition.kind === "QUALITY_FAILED" ||
      condition.kind === "EVENT_MATCH"
    ) {
      setCondition(buildDefaultCondition("THRESHOLD"));
    }
  };

  const buildRulePayload = (): CreateRuleRequest | null => {
    if (!name.trim()) {
      setFormValidationError("请填写规则唯一名称");
      return null;
    }
    if (!titleTemplate.trim()) {
      setFormValidationError("请填写告警标题模板");
      return null;
    }

    let target: Record<string, unknown> = {};
    if (scopeKind === "METRIC") {
      if (!targetTypeName.trim() || !targetMetric.trim()) {
        setFormValidationError("METRIC 作用域必须指定节点类型 (type_name) 与指标名 (metric)");
        return null;
      }
      target = {
        type_name: targetTypeName.trim(),
        metric: targetMetric.trim(),
        ...(targetNodeFilter.trim()
          ? { node_filter: targetNodeFilter.trim() }
          : {}),
      };
    } else if (scopeKind === "DATASET_QUALITY") {
      target = targetDatasetId.trim()
        ? { dataset_id: targetDatasetId.trim() }
        : {};
    } else {
      if (!targetTopic.trim()) {
        setFormValidationError("EVENT 作用域必须指定事件主题 (topic)");
        return null;
      }
      target = {
        topic: targetTopic.trim(),
        ...(targetEventFilter.trim()
          ? { filter: targetEventFilter.trim() }
          : {}),
      };
    }

    const parsedEscalate = escalateAfterInput.trim()
      ? Number(escalateAfterInput)
      : null;

    setFormValidationError(null);
    return {
      name: name.trim(),
      description: description.trim(),
      scope_kind: scopeKind,
      target,
      condition,
      severity,
      title_template: titleTemplate.trim(),
      body_template: bodyTemplate,
      cooldown_minutes: Math.max(0, Math.trunc(cooldownMinutes)),
      resolve_after_minutes: Math.max(0, Math.trunc(resolveAfterMinutes)),
      escalate_after_minutes:
        parsedEscalate !== null &&
        Number.isFinite(parsedEscalate) &&
        parsedEscalate > 0
          ? Math.trunc(parsedEscalate)
          : null,
      channel_ids: selectedChannelIds,
      escalation_channel_ids: selectedEscalationIds,
      run_flow_id: runFlowId.trim() ? runFlowId.trim() : null,
      enabled,
    };
  };

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    const payload = buildRulePayload();
    if (!payload) return;

    if (editingRuleId) {
      await onUpdateRule(editingRuleId, payload);
    } else {
      await onCreateRule(payload);
      resetForm();
    }
  };

  const handleFormDryRun = async () => {
    const payload = buildRulePayload();
    if (!payload) return;
    await onDryRunAdhoc(payload, editingRuleId);
  };

  const insertTemplateChip = (token: string) => {
    if (activeTemplateField === "title") {
      setTitleTemplate((prev) => `${prev}${prev ? " " : ""}${token}`);
    } else {
      setBodyTemplate((prev) => `${prev}${prev ? " " : ""}${token}`);
    }
  };

  const toggleIdInList = (
    list: readonly string[],
    id: string,
    setter: (next: string[]) => void
  ) => {
    setter(
      list.includes(id) ? list.filter((item) => item !== id) : [...list, id]
    );
  };

  return (
    <div className="space-y-4" data-testid="rule-editor-workspace">
      <div className="grid grid-cols-1 lg:grid-cols-12 gap-4 items-start">
        {/* Left: Saved Rules List */}
        <Card className="lg:col-span-5 overflow-hidden" data-testid="rules-list-card">
          <div className="flex items-center justify-between border-b border-border bg-muted/30 px-3.5 py-3">
            <div className="flex items-center gap-2 text-xs font-medium">
              <span>已注册规则定义</span>
              <Badge variant="outline">{rules.length}</Badge>
            </div>
            <Button
              type="button"
              variant="outline"
              className="h-7 px-2.5 text-xs"
              data-testid="new-rule-btn"
              onClick={resetForm}
            >
              <Plus className="h-3.5 w-3.5" />
              新建规则
            </Button>
          </div>

          <div className="divide-y divide-border max-h-[680px] overflow-y-auto">
            {rules.length === 0 ? (
              <div className="p-8 text-center text-xs text-muted-foreground">
                暂无规则定义，可在右侧编辑器新建并试跑验证
              </div>
            ) : (
              rules.map((rule) => {
                const isEditing = editingRuleId === rule.id;
                const isDryRunning = dryRunningId === rule.id;
                const flowName = rule.run_flow_id
                  ? flowNameMap.get(rule.run_flow_id) ??
                    rule.run_flow_id.slice(0, 8)
                  : null;

                return (
                  <div
                    key={rule.id}
                    data-testid={`rule-item-${rule.id}`}
                    className={cn(
                      "p-3.5 space-y-2 text-xs transition-colors",
                      isEditing && "bg-xblue-soft/40"
                    )}
                  >
                    <div className="flex items-start justify-between gap-2">
                      <div className="min-w-0">
                        <div className="flex flex-wrap items-center gap-1.5">
                          <span className="font-semibold text-foreground truncate">
                            {rule.name}
                          </span>
                          <Badge variant="outline">v{rule.version}</Badge>
                          <span
                            className={cn(
                              "inline-flex items-center rounded px-1.5 py-0.5 text-[10px] font-mono-tech font-medium",
                              getSeverityBadgeClass(rule.severity)
                            )}
                          >
                            {rule.severity}
                          </span>
                          <Badge variant={rule.enabled ? "xblue" : "neutral"}>
                            {rule.enabled ? "ENABLED" : "DISABLED"}
                          </Badge>
                        </div>
                        {rule.description && (
                          <div className="text-[11px] text-muted-foreground mt-0.5">
                            {rule.description}
                          </div>
                        )}
                      </div>
                    </div>

                    <div className="flex flex-wrap items-center gap-1.5 text-[11px]">
                      <Badge variant="neutral">{rule.scope_kind}</Badge>
                      <span className="font-mono-tech text-muted-foreground">
                        {formatConditionSummary(rule.condition)}
                      </span>
                    </div>

                    <div className="flex flex-wrap items-center justify-between gap-2 pt-1">
                      <div className="flex flex-wrap items-center gap-1.5 text-[11px] text-muted-foreground">
                        <span>渠道 ×{rule.channel_ids.length}</span>
                        {flowName && (
                          <Badge variant="outline" className="gap-1">
                            <GitBranch className="h-2.5 w-2.5 text-xblue" />
                            流程: {flowName}
                          </Badge>
                        )}
                      </div>

                      <div className="flex items-center gap-1">
                        <Button
                          type="button"
                          variant="outline"
                          className="h-7 px-2 text-[11px] border-xblue/40 text-xblue hover:bg-xblue-soft"
                          data-testid={`dry-run-rule-btn-${rule.id}`}
                          disabled={isDryRunning}
                          onClick={() => {
                            void onDryRunSaved(rule.id);
                          }}
                        >
                          <FlaskConical className="h-3 w-3" />
                          {isDryRunning ? "试跑中..." : "试跑 (Dry Run)"}
                        </Button>

                        <Button
                          type="button"
                          variant="ghost"
                          className="h-7 px-2 text-[11px]"
                          data-testid={`toggle-rule-btn-${rule.id}`}
                          onClick={() => {
                            void onUpdateRule(rule.id, {
                              enabled: !rule.enabled,
                            });
                          }}
                        >
                          <Power className="h-3 w-3" />
                          {rule.enabled ? "停用" : "启用"}
                        </Button>

                        <Button
                          type="button"
                          variant="ghost"
                          className="h-7 px-2 text-[11px]"
                          data-testid={`edit-rule-btn-${rule.id}`}
                          onClick={() => loadRuleIntoEditor(rule)}
                        >
                          <Edit3 className="h-3 w-3" />
                          编辑
                        </Button>

                        <Button
                          type="button"
                          variant="ghost"
                          className="h-7 px-2 text-[11px] text-red-600 dark:text-red-400"
                          data-testid={`delete-rule-btn-${rule.id}`}
                          onClick={() => {
                            void onDeleteRule(rule.id);
                          }}
                        >
                          <Trash2 className="h-3 w-3" />
                        </Button>
                      </div>
                    </div>
                  </div>
                );
              })
            )}
          </div>
        </Card>

        {/* Right: Rule Editor Form & Dry-Run Preview */}
        <div className="lg:col-span-7 space-y-4">
          <Card className="p-4" data-testid="rule-editor-form-card">
            <form
              onSubmit={(e) => {
                void handleSubmit(e);
              }}
              className="space-y-4"
              data-testid="rule-editor-form"
            >
              <div className="flex items-center justify-between border-b border-border pb-2.5">
                <div className="text-xs font-semibold">
                  {editingRuleId ? `编辑规则 (${name})` : "创建判定规则"}
                </div>
                {editingRuleId && (
                  <Button
                    type="button"
                    variant="ghost"
                    className="h-7 px-2 text-xs"
                    onClick={resetForm}
                  >
                    <RotateCcw className="h-3 w-3" />
                    退出编辑
                  </Button>
                )}
              </div>

              {formValidationError && (
                <div className="rounded-md border border-red-500/40 bg-red-500/10 px-3 py-2 text-xs text-red-600 dark:text-red-400">
                  {formValidationError}
                </div>
              )}

              {/* Basic Identity */}
              <div className="grid grid-cols-1 sm:grid-cols-3 gap-3">
                <div>
                  <label className="block text-[11px] text-muted-foreground mb-1">
                    规则标识名 (name)
                  </label>
                  <Input
                    data-testid="rule-name-input"
                    value={name}
                    onChange={(e) => setName(e.target.value)}
                    placeholder="例如 low_inventory"
                    className="h-8 text-xs font-mono-tech"
                  />
                </div>

                <div>
                  <label className="block text-[11px] text-muted-foreground mb-1">
                    作用域类别 (scope_kind)
                  </label>
                  <select
                    data-testid="rule-scope-select"
                    value={scopeKind}
                    onChange={(e) =>
                      handleScopeChange(e.target.value as RuleScopeKind)
                    }
                    className="h-8 w-full rounded-md border border-input bg-background px-2.5 text-xs"
                  >
                    <option value="METRIC">METRIC · 业务节点指标</option>
                    <option value="DATASET_QUALITY">
                      DATASET_QUALITY · 数据集质量门禁
                    </option>
                    <option value="EVENT">EVENT · 事务外发事件</option>
                  </select>
                </div>

                <div>
                  <label className="block text-[11px] text-muted-foreground mb-1">
                    告警严重度 (severity)
                  </label>
                  <select
                    data-testid="rule-severity-select"
                    value={severity}
                    onChange={(e) => setSeverity(e.target.value as Severity)}
                    className="h-8 w-full rounded-md border border-input bg-background px-2.5 text-xs"
                  >
                    <option value="CRITICAL">CRITICAL · 紧急</option>
                    <option value="WARN">WARN · 警告</option>
                    <option value="INFO">INFO · 提示</option>
                  </select>
                </div>
              </div>

              <div>
                <label className="block text-[11px] text-muted-foreground mb-1">
                  规则说明 (description)
                </label>
                <Input
                  data-testid="rule-desc-input"
                  value={description}
                  onChange={(e) => setDescription(e.target.value)}
                  placeholder="描述该规则监控的业务风险与处置动作..."
                  className="h-8 text-xs"
                />
              </div>

              {/* Scope-specific Target */}
              <div className="rounded-md border border-border bg-muted/20 p-3 space-y-2.5">
                <div className="text-[11px] font-semibold text-foreground">
                  监控目标配置 (target · {scopeKind})
                </div>

                {scopeKind === "METRIC" && (
                  <div className="grid grid-cols-1 sm:grid-cols-3 gap-2.5">
                    <div>
                      <label className="block text-[11px] text-muted-foreground mb-1">
                        节点类型 (type_name)
                      </label>
                      <Input
                        list="axalerts-rule-type-names"
                        data-testid="rule-target-type-input"
                        value={targetTypeName}
                        onChange={(e) => setTargetTypeName(e.target.value)}
                        placeholder="Warehouse"
                        className="h-8 text-xs font-mono-tech"
                      />
                      <datalist id="axalerts-rule-type-names">
                        {knownTypeNames.map((tn) => (
                          <option key={tn} value={tn} />
                        ))}
                      </datalist>
                    </div>

                    <div>
                      <label className="block text-[11px] text-muted-foreground mb-1">
                        目标指标名 (metric)
                      </label>
                      <Input
                        data-testid="rule-target-metric-input"
                        value={targetMetric}
                        onChange={(e) => setTargetMetric(e.target.value)}
                        placeholder="inventory"
                        className="h-8 text-xs font-mono-tech"
                      />
                    </div>

                    <div>
                      <label className="block text-[11px] text-muted-foreground mb-1">
                        节点过滤表达式 (node_filter 可选)
                      </label>
                      <Input
                        data-testid="rule-target-filter-input"
                        value={targetNodeFilter}
                        onChange={(e) => setTargetNodeFilter(e.target.value)}
                        placeholder="prop.region == '华东'"
                        className="h-8 text-xs font-mono-tech"
                      />
                    </div>
                  </div>
                )}

                {scopeKind === "DATASET_QUALITY" && (
                  <div>
                    <label className="block text-[11px] text-muted-foreground mb-1">
                      限定数据集 (dataset_id，留空则匹配租户内全部数据集)
                    </label>
                    <select
                      data-testid="rule-target-dataset-select"
                      value={targetDatasetId}
                      onChange={(e) => setTargetDatasetId(e.target.value)}
                      className="h-8 w-full rounded-md border border-input bg-background px-2.5 text-xs"
                    >
                      <option value="">全部数据集 (All Datasets)</option>
                      {datasets.map((ds) => (
                        <option key={ds.id} value={ds.id}>
                          {ds.display_name || ds.name} ({ds.id.slice(0, 8)})
                        </option>
                      ))}
                    </select>
                  </div>
                )}

                {scopeKind === "EVENT" && (
                  <div className="grid grid-cols-1 sm:grid-cols-2 gap-2.5">
                    <div>
                      <label className="block text-[11px] text-muted-foreground mb-1">
                        事件主题 (topic)
                      </label>
                      <select
                        data-testid="rule-target-topic-select"
                        value={targetTopic}
                        onChange={(e) => setTargetTopic(e.target.value)}
                        className="h-8 w-full rounded-md border border-input bg-background px-2.5 text-xs font-mono-tech"
                      >
                        {OUTBOX_TOPICS.map((t) => (
                          <option key={t} value={t}>
                            {t}
                          </option>
                        ))}
                      </select>
                    </div>
                    <div>
                      <label className="block text-[11px] text-muted-foreground mb-1">
                        载荷过滤表达式 (filter 可选)
                      </label>
                      <Input
                        data-testid="rule-target-event-filter-input"
                        value={targetEventFilter}
                        onChange={(e) => setTargetEventFilter(e.target.value)}
                        placeholder="rows_written == 0"
                        className="h-8 text-xs font-mono-tech"
                      />
                    </div>
                  </div>
                )}
              </div>

              {/* 7 Condition Kinds */}
              <div className="rounded-md border border-border bg-muted/20 p-3 space-y-3">
                <div className="flex flex-wrap items-center justify-between gap-2">
                  <label className="text-[11px] font-semibold text-foreground">
                    判定条件算子 (7 类 Condition)
                  </label>
                  <select
                    data-testid="rule-condition-kind-select"
                    value={condition.kind}
                    onChange={(e) =>
                      setCondition(
                        buildDefaultCondition(e.target.value as ConditionKind)
                      )
                    }
                    className="h-8 rounded-md border border-input bg-background px-2.5 text-xs font-mono-tech"
                  >
                    <option value="THRESHOLD">
                      THRESHOLD · 静态阈值 / 持续窗口判定
                    </option>
                    <option value="CHANGE_RATE">
                      CHANGE_RATE · 窗口变化率判定
                    </option>
                    <option value="ABSENCE">ABSENCE · 时序点缺失判定</option>
                    <option value="ANOMALY_ZSCORE">
                      ANOMALY_ZSCORE · Z-Score 统计异常
                    </option>
                    <option value="EXPR">
                      EXPR · 节点多指标/属性布尔表达式
                    </option>
                    <option value="QUALITY_FAILED">
                      QUALITY_FAILED · 数据质量检查失败
                    </option>
                    <option value="EVENT_MATCH">
                      EVENT_MATCH · 事件主题与过滤匹配
                    </option>
                  </select>
                </div>

                {condition.kind === "THRESHOLD" && (
                  <div className="grid grid-cols-1 sm:grid-cols-3 gap-2.5">
                    <div>
                      <label className="block text-[11px] text-muted-foreground mb-1">
                        比较符 (op)
                      </label>
                      <select
                        data-testid="cond-threshold-op"
                        value={condition.op}
                        onChange={(e) =>
                          setCondition({
                            ...condition,
                            op: e.target.value as ComparisonOp,
                          })
                        }
                        className="h-8 w-full rounded-md border border-input bg-background px-2.5 text-xs font-mono-tech"
                      >
                        {COMPARISON_OPS.map((item) => (
                          <option key={item.value} value={item.value}>
                            {item.label}
                          </option>
                        ))}
                      </select>
                    </div>
                    <div>
                      <label className="block text-[11px] text-muted-foreground mb-1">
                        阈值 (value)
                      </label>
                      <Input
                        type="number"
                        step="any"
                        data-testid="cond-threshold-value"
                        value={condition.value}
                        onChange={(e) =>
                          setCondition({
                            ...condition,
                            value: Number(e.target.value),
                          })
                        }
                        className="h-8 text-xs font-mono-tech"
                      />
                    </div>
                    <div>
                      <label className="block text-[11px] text-muted-foreground mb-1">
                        持续分钟数 (for_minutes，0=最新点)
                      </label>
                      <Input
                        type="number"
                        min={0}
                        data-testid="cond-threshold-for-minutes"
                        value={condition.for_minutes}
                        onChange={(e) =>
                          setCondition({
                            ...condition,
                            for_minutes: Math.max(0, Number(e.target.value)),
                          })
                        }
                        className="h-8 text-xs font-mono-tech"
                      />
                    </div>
                  </div>
                )}

                {condition.kind === "CHANGE_RATE" && (
                  <div className="grid grid-cols-1 sm:grid-cols-3 gap-2.5">
                    <div>
                      <label className="block text-[11px] text-muted-foreground mb-1">
                        回看窗口分钟 (window_minutes)
                      </label>
                      <Input
                        type="number"
                        min={1}
                        value={condition.window_minutes}
                        onChange={(e) =>
                          setCondition({
                            ...condition,
                            window_minutes: Math.max(1, Number(e.target.value)),
                          })
                        }
                        className="h-8 text-xs font-mono-tech"
                      />
                    </div>
                    <div>
                      <label className="block text-[11px] text-muted-foreground mb-1">
                        比较符 (op)
                      </label>
                      <select
                        value={condition.op}
                        onChange={(e) =>
                          setCondition({
                            ...condition,
                            op: e.target.value as ComparisonOp,
                          })
                        }
                        className="h-8 w-full rounded-md border border-input bg-background px-2.5 text-xs font-mono-tech"
                      >
                        {COMPARISON_OPS.map((item) => (
                          <option key={item.value} value={item.value}>
                            {item.label}
                          </option>
                        ))}
                      </select>
                    </div>
                    <div>
                      <label className="block text-[11px] text-muted-foreground mb-1">
                        变化率比例 (pct，0.2 = 20%)
                      </label>
                      <Input
                        type="number"
                        step="0.01"
                        value={condition.pct}
                        onChange={(e) =>
                          setCondition({
                            ...condition,
                            pct: Number(e.target.value),
                          })
                        }
                        className="h-8 text-xs font-mono-tech"
                      />
                    </div>
                  </div>
                )}

                {condition.kind === "ABSENCE" && (
                  <div>
                    <label className="block text-[11px] text-muted-foreground mb-1">
                      无数据点判定窗口分钟数 (minutes)
                    </label>
                    <Input
                      type="number"
                      min={1}
                      value={condition.minutes}
                      onChange={(e) =>
                        setCondition({
                          ...condition,
                          minutes: Math.max(1, Number(e.target.value)),
                        })
                      }
                      className="h-8 max-w-xs text-xs font-mono-tech"
                    />
                  </div>
                )}

                {condition.kind === "ANOMALY_ZSCORE" && (
                  <div className="grid grid-cols-1 sm:grid-cols-3 gap-2.5">
                    <div>
                      <label className="block text-[11px] text-muted-foreground mb-1">
                        历史点数 (lookback_points)
                      </label>
                      <Input
                        type="number"
                        min={2}
                        value={condition.lookback_points}
                        onChange={(e) =>
                          setCondition({
                            ...condition,
                            lookback_points: Math.max(2, Number(e.target.value)),
                          })
                        }
                        className="h-8 text-xs font-mono-tech"
                      />
                    </div>
                    <div>
                      <label className="block text-[11px] text-muted-foreground mb-1">
                        Z-Score 阈值 (z)
                      </label>
                      <Input
                        type="number"
                        step="0.1"
                        value={condition.z}
                        onChange={(e) =>
                          setCondition({
                            ...condition,
                            z: Number(e.target.value),
                          })
                        }
                        className="h-8 text-xs font-mono-tech"
                      />
                    </div>
                    <div>
                      <label className="block text-[11px] text-muted-foreground mb-1">
                        最少样本点数 (min_points)
                      </label>
                      <Input
                        type="number"
                        min={2}
                        value={condition.min_points}
                        onChange={(e) =>
                          setCondition({
                            ...condition,
                            min_points: Math.max(2, Number(e.target.value)),
                          })
                        }
                        className="h-8 text-xs font-mono-tech"
                      />
                    </div>
                  </div>
                )}

                {condition.kind === "EXPR" && (
                  <div>
                    <label className="block text-[11px] text-muted-foreground mb-1">
                      布尔判定表达式 (expression，支持节点指标与 prop.*)
                    </label>
                    <Input
                      data-testid="cond-expr-input"
                      value={condition.expression}
                      onChange={(e) =>
                        setCondition({
                          ...condition,
                          expression: e.target.value,
                        })
                      }
                      placeholder="inventory < total_demand && prop.region == '华东'"
                      className="h-8 text-xs font-mono-tech"
                    />
                  </div>
                )}

                {condition.kind === "QUALITY_FAILED" && (
                  <div>
                    <label className="block text-[11px] text-muted-foreground mb-1">
                      最低触发质量失败级别 (min_severity)
                    </label>
                    <select
                      value={condition.min_severity}
                      onChange={(e) =>
                        setCondition({
                          ...condition,
                          min_severity: e.target.value as RuleQualityMinSeverity,
                        })
                      }
                      className="h-8 max-w-xs w-full rounded-md border border-input bg-background px-2.5 text-xs font-mono-tech"
                    >
                      <option value="WARN">WARN · 警告或阻断均触发</option>
                      <option value="BLOCK">BLOCK · 仅阻断级失败触发</option>
                    </select>
                  </div>
                )}

                {condition.kind === "EVENT_MATCH" && (
                  <div className="text-[11px] text-muted-foreground">
                    当外发事件匹配上方 <code className="font-mono-tech">target.topic</code>{" "}
                    且满足可选 <code className="font-mono-tech">target.filter</code> 时立即触发。
                  </div>
                )}
              </div>

              {/* Templates & Variable Chips */}
              <div className="space-y-2.5">
                <div className="flex flex-wrap items-center justify-between gap-1.5">
                  <span className="text-[11px] font-semibold text-foreground">
                    通知渲染模板（点击占位符插入当前焦点字段）
                  </span>
                  <div
                    className="flex flex-wrap items-center gap-1"
                    data-testid="rule-template-chips"
                  >
                    {TEMPLATE_CHIPS.map((chip) => (
                      <button
                        key={chip.token}
                        type="button"
                        data-testid={`template-chip-${chip.token}`}
                        onClick={() => insertTemplateChip(chip.token)}
                        className="rounded border border-border bg-muted/60 px-1.5 py-0.5 text-[10px] font-mono-tech text-foreground hover:border-xblue hover:text-xblue cursor-pointer"
                      >
                        {chip.label}
                      </button>
                    ))}
                  </div>
                </div>

                <div>
                  <label className="block text-[11px] text-muted-foreground mb-1">
                    告警标题模板 (title_template)
                  </label>
                  <Input
                    data-testid="rule-title-template-input"
                    value={titleTemplate}
                    onFocus={() => setActiveTemplateField("title")}
                    onChange={(e) => setTitleTemplate(e.target.value)}
                    className="h-8 text-xs"
                  />
                </div>

                <div>
                  <label className="block text-[11px] text-muted-foreground mb-1">
                    告警正文模板 (body_template)
                  </label>
                  <Textarea
                    rows={2}
                    data-testid="rule-body-template-input"
                    value={bodyTemplate}
                    onFocus={() => setActiveTemplateField("body")}
                    onChange={(e) => setBodyTemplate(e.target.value)}
                    className="text-xs"
                  />
                </div>
              </div>

              {/* Cooldown, Auto-Resolve, Escalation & Linked Flow */}
              <div className="grid grid-cols-1 sm:grid-cols-4 gap-2.5">
                <div>
                  <label className="block text-[11px] text-muted-foreground mb-1">
                    冷却窗口 (分钟)
                  </label>
                  <Input
                    type="number"
                    min={0}
                    value={cooldownMinutes}
                    onChange={(e) =>
                      setCooldownMinutes(Math.max(0, Number(e.target.value)))
                    }
                    className="h-8 text-xs font-mono-tech"
                  />
                </div>
                <div>
                  <label className="block text-[11px] text-muted-foreground mb-1">
                    自动恢复持续 (分钟)
                  </label>
                  <Input
                    type="number"
                    min={0}
                    value={resolveAfterMinutes}
                    onChange={(e) =>
                      setResolveAfterMinutes(
                        Math.max(0, Number(e.target.value))
                      )
                    }
                    className="h-8 text-xs font-mono-tech"
                  />
                </div>
                <div>
                  <label className="block text-[11px] text-muted-foreground mb-1">
                    未确认升级 (分钟, 可空)
                  </label>
                  <Input
                    type="number"
                    min={1}
                    placeholder="不升级"
                    value={escalateAfterInput}
                    onChange={(e) => setEscalateAfterInput(e.target.value)}
                    className="h-8 text-xs font-mono-tech"
                  />
                </div>
                <div>
                  <label className="block text-[11px] text-muted-foreground mb-1">
                    联动触发流程 (run_flow_id)
                  </label>
                  <select
                    data-testid="rule-run-flow-select"
                    value={runFlowId}
                    onChange={(e) => setRunFlowId(e.target.value)}
                    className="h-8 w-full rounded-md border border-input bg-background px-2 text-xs"
                  >
                    <option value="">不触发联动流程</option>
                    {flows.map((f) => (
                      <option key={f.id} value={f.id}>
                        {f.name}
                      </option>
                    ))}
                  </select>
                </div>
              </div>

              {/* Channel Multi-Select */}
              <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
                <div className="rounded-md border border-border p-2.5 space-y-1.5">
                  <div className="text-[11px] font-medium text-foreground">
                    首发/恢复通知渠道 (channel_ids)
                  </div>
                  {channels.length === 0 ? (
                    <div className="text-[11px] text-muted-foreground">
                      暂无可用通知渠道
                    </div>
                  ) : (
                    <div className="flex flex-wrap gap-1.5">
                      {channels.map((ch) => {
                        const active = selectedChannelIds.includes(ch.id);
                        return (
                          <button
                            key={ch.id}
                            type="button"
                            data-testid={`rule-channel-toggle-${ch.id}`}
                            onClick={() =>
                              toggleIdInList(
                                selectedChannelIds,
                                ch.id,
                                setSelectedChannelIds
                              )
                            }
                            className={cn(
                              "rounded px-2 py-1 text-[11px] border transition-colors cursor-pointer",
                              active
                                ? "border-xblue bg-xblue-soft text-xblue font-medium"
                                : "border-border bg-background text-muted-foreground hover:text-foreground"
                            )}
                          >
                            {ch.name} ({ch.kind})
                          </button>
                        );
                      })}
                    </div>
                  )}
                </div>

                <div className="rounded-md border border-border p-2.5 space-y-1.5">
                  <div className="text-[11px] font-medium text-foreground">
                    超时升级渠道 (escalation_channel_ids)
                  </div>
                  {channels.length === 0 ? (
                    <div className="text-[11px] text-muted-foreground">
                      暂无可用升级渠道
                    </div>
                  ) : (
                    <div className="flex flex-wrap gap-1.5">
                      {channels.map((ch) => {
                        const active = selectedEscalationIds.includes(ch.id);
                        return (
                          <button
                            key={ch.id}
                            type="button"
                            data-testid={`rule-escalation-channel-toggle-${ch.id}`}
                            onClick={() =>
                              toggleIdInList(
                                selectedEscalationIds,
                                ch.id,
                                setSelectedEscalationIds
                              )
                            }
                            className={cn(
                              "rounded px-2 py-1 text-[11px] border transition-colors cursor-pointer",
                              active
                                ? "border-xblue bg-xblue-soft text-xblue font-medium"
                                : "border-border bg-background text-muted-foreground hover:text-foreground"
                            )}
                          >
                            {ch.name} ({ch.kind})
                          </button>
                        );
                      })}
                    </div>
                  )}
                </div>
              </div>

              {/* Footer Actions */}
              <div className="flex flex-wrap items-center justify-between gap-2 pt-2 border-t border-border">
                <label className="inline-flex items-center gap-2 text-xs cursor-pointer">
                  <input
                    type="checkbox"
                    checked={enabled}
                    onChange={(e) => setEnabled(e.target.checked)}
                    className="h-3.5 w-3.5 rounded border-border accent-xblue"
                  />
                  <span>保存后立即启用规则判定</span>
                </label>

                <div className="flex items-center gap-2">
                  <Button
                    type="button"
                    variant="outline"
                    data-testid="rule-form-dry-run-btn"
                    disabled={dryRunningId !== null}
                    onClick={() => {
                      void handleFormDryRun();
                    }}
                  >
                    <FlaskConical className="h-3.5 w-3.5 text-xblue" />
                    {dryRunningId === "adhoc" ? "试跑中..." : "试判定预览 (Dry Run)"}
                  </Button>

                  <Button
                    type="submit"
                    variant="primary"
                    data-testid="save-rule-btn"
                    disabled={saving}
                  >
                    <Save className="h-3.5 w-3.5" />
                    {saving
                      ? "保存中..."
                      : editingRuleId
                        ? "更新规则"
                        : "创建规则"}
                  </Button>
                </div>
              </div>
            </form>
          </Card>

          {/* Dry Run Preview Output */}
          {dryRunResult && (
            <Card
              className="p-4 space-y-3 border-xblue/35"
              data-testid="rule-dry-run-results"
            >
              <div className="flex flex-wrap items-center justify-between gap-2">
                <div className="flex items-center gap-2 text-xs font-semibold">
                  <FlaskConical className="h-4 w-4 text-xblue" />
                  <span>只读试判定结果预览（不落库、不发通知）</span>
                </div>
                <div className="flex items-center gap-2">
                  <Badge variant="outline">
                    扫描对象: {dryRunResult.evaluated_objects}
                  </Badge>
                  <Badge variant="xblue" data-testid="dry-run-matched-count">
                    命中触发: {dryRunResult.matched_count}
                  </Badge>
                </div>
              </div>

              {dryRunResult.would_trigger.length === 0 ? (
                <div className="rounded-md border border-dashed border-border p-3 text-center text-xs text-muted-foreground">
                  当前数据快照下无命中对象（0 条告警将会触发）
                </div>
              ) : (
                <div className="divide-y divide-border rounded-md border border-border">
                  {dryRunResult.would_trigger.map((hit) => (
                    <div
                      key={hit.dedup_key}
                      data-testid={`dry-run-hit-${hit.dedup_key}`}
                      className="p-2.5 text-xs space-y-1"
                    >
                      <div className="flex flex-wrap items-center justify-between gap-2">
                        <div className="flex items-center gap-2">
                          <span
                            className={cn(
                              "inline-flex items-center rounded px-1.5 py-0.5 text-[10px] font-mono-tech font-medium",
                              getSeverityBadgeClass(hit.severity)
                            )}
                          >
                            {hit.severity}
                          </span>
                          <span className="font-medium text-foreground">
                            {hit.title}
                          </span>
                        </div>
                        <span className="font-mono-tech text-[11px] text-muted-foreground">
                          {hit.value !== null ? `观测值=${hit.value}` : hit.dedup_key}
                        </span>
                      </div>
                      {hit.body && (
                        <div className="text-[11px] text-muted-foreground">
                          {hit.body}
                        </div>
                      )}
                    </div>
                  ))}
                </div>
              )}
            </Card>
          )}
        </div>
      </div>
    </div>
  );
}
