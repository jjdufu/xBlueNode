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
import {
  Badge,
  Button,
  Card,
  Input,
  Select,
  StatusBadge,
  Textarea,
} from "@/components/ui/primitives";
import { useI18n } from "@/i18n";
import { cn } from "@/lib/utils";
import { getSeverityBadgeClass } from "./AlertTable";

export interface RuleEditorProps {
  rules: readonly RuleDef[];
  channels: readonly NotifyChannelRecord[];
  flows: readonly FlowRecord[];
  datasets: readonly Dataset[];
  knownTypeNames: readonly string[];
  saving: boolean;
  canEdit: boolean;
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
  canEdit,
  dryRunningId,
  dryRunResult,
  onCreateRule,
  onUpdateRule,
  onDeleteRule,
  onDryRunSaved,
  onDryRunAdhoc,
}: RuleEditorProps) {
  const { isZh, tx, tEnum, fmtNum } = useI18n();
  const [editingRuleId, setEditingRuleId] = useState<string | null>(null);
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
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
    "[{{severity}}] {{node.title}} {{metric}}={{value}} ({{threshold}})"
  );
  const [bodyTemplate, setBodyTemplate] = useState(
    "{{rule.name}}: {{node.title}} = {{value}}"
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

  const comparisonOps: readonly { value: ComparisonOp; label: string }[] = [
    { value: "LT", label: tx("< 小于", "< Less Than") },
    { value: "LTE", label: tx("<= 小于等于", "<= Less or Equal") },
    { value: "GT", label: tx("> 大于", "> Greater Than") },
    { value: "GTE", label: tx(">= 大于等于", ">= Greater or Equal") },
    { value: "EQ", label: tx("== 等于", "== Equal") },
    { value: "NE", label: tx("!= 不等于", "!= Not Equal") },
  ];

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
    setBodyTemplate("{{rule.name}}: {{node.title}} = {{value}}");
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

  const buildRulePayload = (allowDefaultName = false): CreateRuleRequest | null => {
    const effectiveName = name.trim() || (allowDefaultName ? "warehouse_low_stock" : "");
    if (!effectiveName) {
      setFormValidationError(tx("规则名称不能为空", "Rule name is required"));
      return null;
    }
    if (!titleTemplate.trim()) {
      setFormValidationError(tx("告警标题模板不能为空", "Title template is required"));
      return null;
    }

    let target: Record<string, unknown> = {};
    if (scopeKind === "METRIC") {
      if (!targetTypeName.trim() || !targetMetric.trim()) {
        setFormValidationError(
          tx("需填写节点类型与目标指标", "Node type and metric are required")
        );
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
        setFormValidationError(
          tx("需填写事件主题", "Event topic is required")
        );
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
      name: effectiveName,
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
    if (!canEdit) return;
    const payload = buildRulePayload(false);
    if (!payload) return;

    if (editingRuleId) {
      await onUpdateRule(editingRuleId, payload);
    } else {
      await onCreateRule(payload);
      resetForm();
    }
  };

  const handleFormDryRun = async () => {
    const payload = buildRulePayload(true);
    if (!payload) return;
    await onDryRunAdhoc(payload, editingRuleId);
  };

  const insertTemplateChip = (token: string) => {
    if (!canEdit) return;
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
    if (!canEdit) return;
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
              <span>{tx("已注册规则", "Registered Rules")}</span>
              <Badge variant="outline">{rules.length}</Badge>
            </div>
            {canEdit && (
              <Button
                type="button"
                variant="outline"
                size="sm"
                data-testid="new-rule-btn"
                onClick={resetForm}
              >
                <Plus className="h-3.5 w-3.5" />
                {tx("新建规则", "New Rule")}
              </Button>
            )}
          </div>

          <div className="divide-y divide-border max-h-[680px] overflow-y-auto">
            {rules.length === 0 ? (
              <div className="p-8 text-center text-xs text-muted-foreground">
                {tx("暂无规则记录", "No rules")}
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
                            {tEnum(rule.severity)}
                          </span>
                          <StatusBadge
                            status={rule.enabled ? "ENABLED" : "DISABLED"}
                          />
                        </div>
                        {rule.description && (
                          <div className="text-[11px] text-muted-foreground mt-0.5">
                            {rule.description}
                          </div>
                        )}
                      </div>
                    </div>

                    <div className="flex flex-wrap items-center gap-1.5 text-[11px]">
                      <Badge variant="neutral">{tEnum(rule.scope_kind)}</Badge>
                      <span className="font-mono-tech text-muted-foreground">
                        {formatConditionSummary(rule.condition, isZh)}
                      </span>
                    </div>

                    <div className="flex flex-wrap items-center justify-between gap-2 pt-1">
                      <div className="flex flex-wrap items-center gap-1.5 text-[11px] text-muted-foreground">
                        <span>
                          {tx("渠道", "Channels")} ×{rule.channel_ids.length}
                        </span>
                        {flowName && (
                          <Badge variant="outline" className="gap-1">
                            <GitBranch className="h-2.5 w-2.5 text-xblue" />
                            {flowName}
                          </Badge>
                        )}
                      </div>

                      <div className="flex items-center gap-1">
                        <Button
                          type="button"
                          variant="outline"
                          size="sm"
                          className="border-xblue/40 text-xblue hover:bg-xblue-soft"
                          data-testid={`dry-run-rule-btn-${rule.id}`}
                          disabled={isDryRunning}
                          onClick={() => {
                            void onDryRunSaved(rule.id);
                          }}
                        >
                          <FlaskConical className="h-3 w-3" />
                          {isDryRunning
                            ? tx("试跑中…", "Running…")
                            : tx("试跑", "Dry Run")}
                        </Button>

                        {canEdit && (
                          <>
                            <Button
                              type="button"
                              variant="ghost"
                              size="sm"
                              data-testid={`toggle-rule-btn-${rule.id}`}
                              onClick={() => {
                                void onUpdateRule(rule.id, {
                                  enabled: !rule.enabled,
                                });
                              }}
                            >
                              <Power className="h-3 w-3" />
                              {rule.enabled
                                ? tx("停用", "Disable")
                                : tx("启用", "Enable")}
                            </Button>

                            <Button
                              type="button"
                              variant="ghost"
                              size="sm"
                              data-testid={`edit-rule-btn-${rule.id}`}
                              onClick={() => loadRuleIntoEditor(rule)}
                            >
                              <Edit3 className="h-3 w-3" />
                              {tx("编辑", "Edit")}
                            </Button>

                            <Button
                              type="button"
                              variant="ghost"
                              size="sm"
                              className="text-red-600 dark:text-red-400"
                              data-testid={`delete-rule-btn-${rule.id}`}
                              onClick={() => {
                                void onDeleteRule(rule.id);
                              }}
                            >
                              <Trash2 className="h-3 w-3" />
                            </Button>
                          </>
                        )}
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
                  {editingRuleId
                    ? tx(`编辑规则 · ${name}`, `Edit Rule · ${name}`)
                    : tx("新建规则", "New Rule")}
                </div>
                {editingRuleId && (
                  <Button
                    type="button"
                    variant="ghost"
                    size="sm"
                    onClick={resetForm}
                  >
                    <RotateCcw className="h-3 w-3" />
                    {tx("重置", "Reset")}
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
                    {tx("规则名称", "Rule Name")}
                  </label>
                  <Input
                    data-testid="rule-name-input"
                    value={name}
                    disabled={!canEdit}
                    onChange={(e) => setName(e.target.value)}
                    placeholder="warehouse_low_stock"
                    className="h-8 text-xs font-mono-tech"
                  />
                </div>

                <div>
                  <label className="block text-[11px] text-muted-foreground mb-1">
                    {tx("监控范围", "Scope")}
                  </label>
                  <Select
                    data-testid="rule-scope-select"
                    value={scopeKind}
                    disabled={!canEdit}
                    onChange={(e) =>
                      handleScopeChange(e.target.value as RuleScopeKind)
                    }
                    className="h-8 w-full"
                  >
                    <option value="METRIC">
                      {tx("业务节点指标", "Node Metric")}
                    </option>
                    <option value="DATASET_QUALITY">
                      {tx("数据质量门禁", "Dataset Quality")}
                    </option>
                    <option value="EVENT">
                      {tx("事务外发事件", "Outbox Event")}
                    </option>
                  </Select>
                </div>

                <div>
                  <label className="block text-[11px] text-muted-foreground mb-1">
                    {tx("告警级别", "Severity")}
                  </label>
                  <Select
                    data-testid="rule-severity-select"
                    value={severity}
                    disabled={!canEdit}
                    onChange={(e) => setSeverity(e.target.value as Severity)}
                    className="h-8 w-full"
                  >
                    <option value="CRITICAL">{tEnum("CRITICAL")}</option>
                    <option value="WARN">{tEnum("WARN")}</option>
                    <option value="INFO">{tEnum("INFO")}</option>
                  </Select>
                </div>
              </div>

              <div>
                <label className="block text-[11px] text-muted-foreground mb-1">
                  {tx("规则说明", "Description")}
                </label>
                <Input
                  data-testid="rule-desc-input"
                  value={description}
                  disabled={!canEdit}
                  onChange={(e) => setDescription(e.target.value)}
                  placeholder={tx("核心仓储安全库存预警", "Core warehouse low-stock alert")}
                  className="h-8 text-xs"
                />
              </div>

              {/* Scope-specific Target */}
              <div className="rounded-md border border-border bg-muted/20 p-3 space-y-2.5">
                <div className="text-[11px] font-semibold text-foreground">
                  {tx("监控目标", "Target")}
                </div>

                {scopeKind === "METRIC" && (
                  <div className="grid grid-cols-1 sm:grid-cols-3 gap-2.5">
                    <div>
                      <label className="block text-[11px] text-muted-foreground mb-1">
                        {tx("节点类型", "Node Type")}
                      </label>
                      <Input
                        list="axalerts-rule-type-names"
                        data-testid="rule-target-type-input"
                        value={targetTypeName}
                        disabled={!canEdit}
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
                        {tx("目标指标", "Metric")}
                      </label>
                      <Input
                        data-testid="rule-target-metric-input"
                        value={targetMetric}
                        disabled={!canEdit}
                        onChange={(e) => setTargetMetric(e.target.value)}
                        placeholder="inventory"
                        className="h-8 text-xs font-mono-tech"
                      />
                    </div>

                    <div>
                      <label className="block text-[11px] text-muted-foreground mb-1">
                        {tx("节点过滤条件", "Node Filter")}
                      </label>
                      <Input
                        data-testid="rule-target-filter-input"
                        value={targetNodeFilter}
                        disabled={!canEdit}
                        onChange={(e) => setTargetNodeFilter(e.target.value)}
                        placeholder="prop.region == 'east'"
                        className="h-8 text-xs font-mono-tech"
                      />
                    </div>
                  </div>
                )}

                {scopeKind === "DATASET_QUALITY" && (
                  <div>
                    <label className="block text-[11px] text-muted-foreground mb-1">
                      {tx("目标数据集", "Target Dataset")}
                    </label>
                    <Select
                      data-testid="rule-target-dataset-select"
                      value={targetDatasetId}
                      disabled={!canEdit}
                      onChange={(e) => setTargetDatasetId(e.target.value)}
                      className="h-8 w-full"
                    >
                      <option value="">
                        {tx("全部数据集", "All Datasets")}
                      </option>
                      {datasets.map((ds) => (
                        <option key={ds.id} value={ds.id}>
                          {ds.display_name || ds.name}
                        </option>
                      ))}
                    </Select>
                  </div>
                )}

                {scopeKind === "EVENT" && (
                  <div className="grid grid-cols-1 sm:grid-cols-2 gap-2.5">
                    <div>
                      <label className="block text-[11px] text-muted-foreground mb-1">
                        {tx("事件主题", "Event Topic")}
                      </label>
                      <Select
                        data-testid="rule-target-topic-select"
                        value={targetTopic}
                        disabled={!canEdit}
                        onChange={(e) => setTargetTopic(e.target.value)}
                        className="h-8 w-full font-mono-tech"
                      >
                        {OUTBOX_TOPICS.map((t) => (
                          <option key={t} value={t}>
                            {t}
                          </option>
                        ))}
                      </Select>
                    </div>
                    <div>
                      <label className="block text-[11px] text-muted-foreground mb-1">
                        {tx("载荷过滤表达式", "Payload Filter")}
                      </label>
                      <Input
                        data-testid="rule-target-event-filter-input"
                        value={targetEventFilter}
                        disabled={!canEdit}
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
                    {tx("判定条件", "Condition")}
                  </label>
                  <Select
                    data-testid="rule-condition-kind-select"
                    value={condition.kind}
                    disabled={!canEdit}
                    onChange={(e) =>
                      setCondition(
                        buildDefaultCondition(e.target.value as ConditionKind)
                      )
                    }
                    className="h-8"
                  >
                    <option value="THRESHOLD">
                      {tx("静态阈值", "Threshold")}
                    </option>
                    <option value="CHANGE_RATE">
                      {tx("窗口变化率", "Change Rate")}
                    </option>
                    <option value="ABSENCE">
                      {tx("时序点缺失", "Absence")}
                    </option>
                    <option value="ANOMALY_ZSCORE">
                      {tx("Z-Score 统计异常", "Z-Score Anomaly")}
                    </option>
                    <option value="EXPR">
                      {tx("布尔表达式", "Boolean Expression")}
                    </option>
                    <option value="QUALITY_FAILED">
                      {tx("质量门禁失败", "Quality Check Failed")}
                    </option>
                    <option value="EVENT_MATCH">
                      {tx("事件匹配", "Event Match")}
                    </option>
                  </Select>
                </div>

                {condition.kind === "THRESHOLD" && (
                  <div className="grid grid-cols-1 sm:grid-cols-3 gap-2.5">
                    <div>
                      <label className="block text-[11px] text-muted-foreground mb-1">
                        {tx("比较符", "Operator")}
                      </label>
                      <Select
                        data-testid="cond-threshold-op"
                        value={condition.op}
                        disabled={!canEdit}
                        onChange={(e) =>
                          setCondition({
                            ...condition,
                            op: e.target.value as ComparisonOp,
                          })
                        }
                        className="h-8 w-full font-mono-tech"
                      >
                        {comparisonOps.map((item) => (
                          <option key={item.value} value={item.value}>
                            {item.label}
                          </option>
                        ))}
                      </Select>
                    </div>
                    <div>
                      <label className="block text-[11px] text-muted-foreground mb-1">
                        {tx("阈值", "Threshold")}
                      </label>
                      <Input
                        type="number"
                        step="any"
                        data-testid="cond-threshold-value"
                        value={condition.value}
                        disabled={!canEdit}
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
                        {tx("持续分钟数", "Duration (min)")}
                      </label>
                      <Input
                        type="number"
                        min={0}
                        data-testid="cond-threshold-for-minutes"
                        value={condition.for_minutes}
                        disabled={!canEdit}
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
                        {tx("窗口分钟数", "Window (min)")}
                      </label>
                      <Input
                        type="number"
                        min={1}
                        value={condition.window_minutes}
                        disabled={!canEdit}
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
                        {tx("比较符", "Operator")}
                      </label>
                      <Select
                        value={condition.op}
                        disabled={!canEdit}
                        onChange={(e) =>
                          setCondition({
                            ...condition,
                            op: e.target.value as ComparisonOp,
                          })
                        }
                        className="h-8 w-full font-mono-tech"
                      >
                        {comparisonOps.map((item) => (
                          <option key={item.value} value={item.value}>
                            {item.label}
                          </option>
                        ))}
                      </Select>
                    </div>
                    <div>
                      <label className="block text-[11px] text-muted-foreground mb-1">
                        {tx("变化比例", "Ratio")}
                      </label>
                      <Input
                        type="number"
                        step="0.01"
                        value={condition.pct}
                        disabled={!canEdit}
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
                      {tx("缺失分钟数", "Absence Window (min)")}
                    </label>
                    <Input
                      type="number"
                      min={1}
                      value={condition.minutes}
                      disabled={!canEdit}
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
                        {tx("回看点数", "Lookback Points")}
                      </label>
                      <Input
                        type="number"
                        min={2}
                        value={condition.lookback_points}
                        disabled={!canEdit}
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
                        {tx("Z-Score 阈值", "Z-Score Threshold")}
                      </label>
                      <Input
                        type="number"
                        step="0.1"
                        value={condition.z}
                        disabled={!canEdit}
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
                        {tx("最少样本点数", "Min Points")}
                      </label>
                      <Input
                        type="number"
                        min={2}
                        value={condition.min_points}
                        disabled={!canEdit}
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
                      {tx("布尔表达式", "Boolean Expression")}
                    </label>
                    <Input
                      data-testid="cond-expr-input"
                      value={condition.expression}
                      disabled={!canEdit}
                      onChange={(e) =>
                        setCondition({
                          ...condition,
                          expression: e.target.value,
                        })
                      }
                      placeholder="inventory < total_demand"
                      className="h-8 text-xs font-mono-tech"
                    />
                  </div>
                )}

                {condition.kind === "QUALITY_FAILED" && (
                  <div>
                    <label className="block text-[11px] text-muted-foreground mb-1">
                      {tx("最低失败级别", "Minimum Failure Severity")}
                    </label>
                    <Select
                      value={condition.min_severity}
                      disabled={!canEdit}
                      onChange={(e) =>
                        setCondition({
                          ...condition,
                          min_severity: e.target.value as RuleQualityMinSeverity,
                        })
                      }
                      className="h-8 max-w-xs w-full"
                    >
                      <option value="WARN">{tx("警告及以上", "Warning or Block")}</option>
                      <option value="BLOCK">{tx("仅阻断级", "Block Only")}</option>
                    </Select>
                  </div>
                )}

                {condition.kind === "EVENT_MATCH" && (
                  <div className="text-[11px] text-muted-foreground">
                    {tx(
                      "匹配目标事件主题与过滤表达式时触发。",
                      "Triggers when outbox events match the target topic and filter."
                    )}
                  </div>
                )}
              </div>

              {/* Templates & Variable Chips */}
              <div className="space-y-2.5">
                <div className="flex flex-wrap items-center justify-between gap-1.5">
                  <span className="text-[11px] font-semibold text-foreground">
                    {tx("通知模板变量", "Template Variables")}
                  </span>
                  <div
                    className="flex flex-wrap items-center gap-1"
                    data-testid="rule-template-chips"
                  >
                    {TEMPLATE_CHIPS.map((chip) => (
                      <button
                        key={chip.token}
                        type="button"
                        disabled={!canEdit}
                        data-testid={`template-chip-${chip.token}`}
                        onClick={() => insertTemplateChip(chip.token)}
                        className="rounded border border-border bg-muted/60 px-1.5 py-0.5 text-[10px] font-mono-tech text-foreground hover:border-xblue hover:text-xblue disabled:opacity-40 cursor-pointer"
                      >
                        {chip.label}
                      </button>
                    ))}
                  </div>
                </div>

                <div>
                  <label className="block text-[11px] text-muted-foreground mb-1">
                    {tx("标题模板", "Title Template")}
                  </label>
                  <Input
                    data-testid="rule-title-template-input"
                    value={titleTemplate}
                    disabled={!canEdit}
                    onFocus={() => setActiveTemplateField("title")}
                    onChange={(e) => setTitleTemplate(e.target.value)}
                    className="h-8 text-xs"
                  />
                </div>

                <div>
                  <label className="block text-[11px] text-muted-foreground mb-1">
                    {tx("正文模板", "Body Template")}
                  </label>
                  <Textarea
                    rows={2}
                    data-testid="rule-body-template-input"
                    value={bodyTemplate}
                    disabled={!canEdit}
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
                    {tx("冷却窗口（分）", "Cooldown (min)")}
                  </label>
                  <Input
                    type="number"
                    min={0}
                    value={cooldownMinutes}
                    disabled={!canEdit}
                    onChange={(e) =>
                      setCooldownMinutes(Math.max(0, Number(e.target.value)))
                    }
                    className="h-8 text-xs font-mono-tech"
                  />
                </div>
                <div>
                  <label className="block text-[11px] text-muted-foreground mb-1">
                    {tx("自动恢复（分）", "Auto-Resolve (min)")}
                  </label>
                  <Input
                    type="number"
                    min={0}
                    value={resolveAfterMinutes}
                    disabled={!canEdit}
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
                    {tx("超时升级（分）", "Escalate After (min)")}
                  </label>
                  <Input
                    type="number"
                    min={1}
                    placeholder="15"
                    value={escalateAfterInput}
                    disabled={!canEdit}
                    onChange={(e) => setEscalateAfterInput(e.target.value)}
                    className="h-8 text-xs font-mono-tech"
                  />
                </div>
                <div>
                  <label className="block text-[11px] text-muted-foreground mb-1">
                    {tx("联动编排流程", "Linked Flow")}
                  </label>
                  <Select
                    data-testid="rule-run-flow-select"
                    value={runFlowId}
                    disabled={!canEdit}
                    onChange={(e) => setRunFlowId(e.target.value)}
                    className="h-8 w-full"
                  >
                    <option value="">{tx("无", "None")}</option>
                    {flows.map((f) => (
                      <option key={f.id} value={f.id}>
                        {f.name}
                      </option>
                    ))}
                  </Select>
                </div>
              </div>

              {/* Channel Multi-Select */}
              <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
                <div className="rounded-md border border-border p-2.5 space-y-1.5">
                  <div className="text-[11px] font-medium text-foreground">
                    {tx("首发 / 恢复通知渠道", "Primary Channels")}
                  </div>
                  {channels.length === 0 ? (
                    <div className="text-[11px] text-muted-foreground">
                      {tx("暂无渠道", "No channels")}
                    </div>
                  ) : (
                    <div className="flex flex-wrap gap-1.5">
                      {channels.map((ch) => {
                        const active = selectedChannelIds.includes(ch.id);
                        return (
                          <button
                            key={ch.id}
                            type="button"
                            disabled={!canEdit}
                            data-testid={`rule-channel-toggle-${ch.id}`}
                            onClick={() =>
                              toggleIdInList(
                                selectedChannelIds,
                                ch.id,
                                setSelectedChannelIds
                              )
                            }
                            className={cn(
                              "rounded px-2 py-1 text-[11px] border transition-colors cursor-pointer disabled:opacity-50",
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
                    {tx("超时升级渠道", "Escalation Channels")}
                  </div>
                  {channels.length === 0 ? (
                    <div className="text-[11px] text-muted-foreground">
                      {tx("暂无渠道", "No channels")}
                    </div>
                  ) : (
                    <div className="flex flex-wrap gap-1.5">
                      {channels.map((ch) => {
                        const active = selectedEscalationIds.includes(ch.id);
                        return (
                          <button
                            key={ch.id}
                            type="button"
                            disabled={!canEdit}
                            data-testid={`rule-escalation-channel-toggle-${ch.id}`}
                            onClick={() =>
                              toggleIdInList(
                                selectedEscalationIds,
                                ch.id,
                                setSelectedEscalationIds
                              )
                            }
                            className={cn(
                              "rounded px-2 py-1 text-[11px] border transition-colors cursor-pointer disabled:opacity-50",
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
                    disabled={!canEdit}
                    onChange={(e) => setEnabled(e.target.checked)}
                    className="h-3.5 w-3.5 rounded border-border accent-xblue"
                  />
                  <span>{tx("启用规则", "Enable Rule")}</span>
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
                    {dryRunningId === "adhoc"
                      ? tx("试跑中…", "Running…")
                      : tx("试判定预览", "Dry Run")}
                  </Button>

                  {canEdit && (
                    <Button
                      type="submit"
                      variant="primary"
                      data-testid="save-rule-btn"
                      disabled={saving}
                    >
                      <Save className="h-3.5 w-3.5" />
                      {saving
                        ? tx("保存中…", "Saving…")
                        : editingRuleId
                          ? tx("保存规则", "Save Rule")
                          : tx("创建规则", "Create Rule")}
                    </Button>
                  )}
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
                  <span>{tx("试判定结果预览", "Dry Run Results")}</span>
                </div>
                <div className="flex items-center gap-2">
                  <Badge variant="outline">
                    {tx(
                      `扫描对象: ${fmtNum(dryRunResult.evaluated_objects, 0)}`,
                      `Evaluated: ${fmtNum(dryRunResult.evaluated_objects, 0)}`
                    )}
                  </Badge>
                  <Badge variant="xblue" data-testid="dry-run-matched-count">
                    {tx(
                      `命中触发: ${fmtNum(dryRunResult.matched_count, 0)}`,
                      `Matched: ${fmtNum(dryRunResult.matched_count, 0)}`
                    )}
                  </Badge>
                </div>
              </div>

              {dryRunResult.would_trigger.length === 0 ? (
                <div className="rounded-md border border-dashed border-border p-3 text-center text-xs text-muted-foreground">
                  {tx("无命中对象", "No matching objects")}
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
                            {tEnum(hit.severity)}
                          </span>
                          <span className="font-medium text-foreground">
                            {hit.title}
                          </span>
                        </div>
                        <span className="font-mono-tech text-[11px] text-muted-foreground">
                          {hit.value !== null
                            ? `${tx("观测值", "Value")}=${fmtNum(hit.value, 4)}`
                            : hit.dedup_key}
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
