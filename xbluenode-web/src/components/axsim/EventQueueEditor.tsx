import { CalendarClock, Plus, ShieldAlert, Trash2 } from "lucide-react";
import type {
  SandboxGraphNode,
  SimEventSpec,
  SimGoalConstraint,
} from "@/lib/api/axsim";
import type {
  GeoPredicateOp,
  InterventionMode,
  SimEventTriggerKind,
  SimThresholdOp,
} from "@/lib/api/enums";
import type { GeoShapeDto, SpatialFilterSpec } from "@/lib/api/xblue";
import { Button, Input, Select } from "@/components/ui/primitives";
import { useI18n } from "@/i18n";

export interface EventDraft {
  id: string;
  name: string;
  trigger_kind: SimEventTriggerKind;
  at_step: string;
  watch_node_id: string;
  watch_metric: string;
  op: SimThresholdOp;
  threshold: string;
  cooldown_steps: string;
  max_fires: string;
  action_node_id: string;
  action_metric_key: string;
  action_mode: InterventionMode;
  action_value: string;
}

export type GoalTargetScope = "NODE" | "TYPE";
export type GoalSpatialKind = "NONE" | "GEO_WITHIN_POLYGON" | "GEO_DWITHIN_METERS";

export interface GoalDraft {
  id: string;
  name: string;
  scope: GoalTargetScope;
  target_node_id: string;
  target_type_name: string;
  metric_key: string;
  op: SimThresholdOp;
  threshold: string;
  controllable_node_id: string;
  spatial_kind: GoalSpatialKind;
  polygon_json: string;
  center_lng: string;
  center_lat: string;
  radius_meters: string;
}

export interface EventQueueEditorProps {
  nodes: SandboxGraphNode[];
  events: EventDraft[];
  goals: GoalDraft[];
  disabled?: boolean;
  onChangeEvents: (next: EventDraft[]) => void;
  onChangeGoals: (next: GoalDraft[]) => void;
}

export function createEmptyEventDraft(nodes: SandboxGraphNode[]): EventDraft {
  const firstNode = nodes[0];
  const metricKeys = firstNode ? Object.keys(firstNode.metrics ?? {}) : [];
  const firstMetric = metricKeys[0] ?? "output";
  const shortId = Math.random().toString(36).slice(2, 6);
  return {
    id: `ev_${shortId}`,
    name: "",
    trigger_kind: "AT_STEP",
    at_step: "4",
    watch_node_id: firstNode?.id ?? "",
    watch_metric: firstMetric,
    op: "LT",
    threshold: "200",
    cooldown_steps: "2",
    max_fires: "1",
    action_node_id: firstNode?.id ?? "",
    action_metric_key: firstMetric,
    action_mode: "ADD",
    action_value: "-100",
  };
}

export function createEmptyGoalDraft(nodes: SandboxGraphNode[]): GoalDraft {
  const firstNode = nodes[0];
  const metricKeys = firstNode ? Object.keys(firstNode.metrics ?? {}) : [];
  const firstMetric = metricKeys[0] ?? "inventory";
  const shortId = Math.random().toString(36).slice(2, 6);
  return {
    id: `goal_${shortId}`,
    name: "",
    scope: "NODE",
    target_node_id: firstNode?.id ?? "",
    target_type_name: firstNode?.type_name ?? "",
    metric_key: firstMetric,
    op: "GTE",
    threshold: "200",
    controllable_node_id: "",
    spatial_kind: "NONE",
    polygon_json: "",
    center_lng: "121.47",
    center_lat: "31.23",
    radius_meters: "100000",
  };
}

function normalizeRingCoords(
  rawRings: unknown
): Array<Array<[number, number]>> | null {
  if (!Array.isArray(rawRings) || rawRings.length === 0) return null;
  const rings: Array<Array<[number, number]>> = [];
  for (const ring of rawRings) {
    if (!Array.isArray(ring) || ring.length < 4) return null;
    const pts: Array<[number, number]> = [];
    for (const pair of ring) {
      if (
        !Array.isArray(pair) ||
        pair.length < 2 ||
        typeof pair[0] !== "number" ||
        typeof pair[1] !== "number" ||
        !Number.isFinite(pair[0]) ||
        !Number.isFinite(pair[1])
      ) {
        return null;
      }
      pts.push([pair[0], pair[1]]);
    }
    rings.push(pts);
  }
  return rings;
}

function parsePolygonFilterInput(raw: string): GeoShapeDto | null {
  const trimmed = raw.trim();
  if (!trimmed) return null;
  try {
    const parsed: unknown = JSON.parse(trimmed);
    if (Array.isArray(parsed)) {
      const rings = normalizeRingCoords(parsed);
      return rings ? { type: "Polygon", coordinates: rings } : null;
    }
    if (
      typeof parsed === "object" &&
      parsed !== null &&
      (parsed as { type?: unknown }).type === "Polygon"
    ) {
      const rings = normalizeRingCoords(
        (parsed as { coordinates?: unknown }).coordinates
      );
      return rings ? { type: "Polygon", coordinates: rings } : null;
    }
    return null;
  } catch {
    return null;
  }
}

export function compileEventsAndGoals(
  events: readonly EventDraft[],
  goals: readonly GoalDraft[],
  tx: (zh: string, en: string) => string
):
  | {
      ok: true;
      scheduledEvents: SimEventSpec[];
      goalConstraints: SimGoalConstraint[];
    }
  | { ok: false; error: string } {
  const scheduledEvents: SimEventSpec[] = [];

  for (const ev of events) {
    const cleanId = ev.id.trim();
    if (!cleanId) {
      return {
        ok: false,
        error: tx("事件标识不能为空", "Event ID is required"),
      };
    }
    if (!ev.action_node_id || !ev.action_metric_key.trim()) {
      return {
        ok: false,
        error: tx(
          "事件动作需指定节点与指标",
          "Event action requires node and metric"
        ),
      };
    }
    const actionVal = Number.parseFloat(ev.action_value);
    if (!Number.isFinite(actionVal)) {
      return {
        ok: false,
        error: tx("事件变动值必须是有效数字", "Event action value must be numeric"),
      };
    }
    if (ev.action_mode === "RATIO" && actionVal <= -1) {
      return {
        ok: false,
        error: tx("事件比例变动值必须大于 -1", "Event ratio delta must exceed -1"),
      };
    }

    if (ev.trigger_kind === "AT_STEP") {
      const stepNum = Number.parseInt(ev.at_step, 10);
      if (!Number.isFinite(stepNum) || stepNum < 0) {
        return {
          ok: false,
          error: tx("定时事件步数必须 >= 0", "Scheduled step must be >= 0"),
        };
      }
      scheduledEvents.push({
        id: cleanId,
        name: ev.name.trim() || cleanId,
        trigger_kind: "AT_STEP",
        at_step: stepNum,
        actions: [
          {
            target_node_id: ev.action_node_id,
            metric_key: ev.action_metric_key.trim(),
            mode: ev.action_mode,
            value: actionVal,
          },
        ],
      });
    } else {
      if (!ev.watch_node_id || !ev.watch_metric.trim()) {
        return {
          ok: false,
          error: tx(
            "阈值事件需指定监控节点与指标",
            "Threshold event requires watched node and metric"
          ),
        };
      }
      const threshVal = Number.parseFloat(ev.threshold);
      if (!Number.isFinite(threshVal)) {
        return {
          ok: false,
          error: tx("触发阈值必须是有效数字", "Trigger threshold must be numeric"),
        };
      }
      const cooldown = Math.max(0, Number.parseInt(ev.cooldown_steps, 10) || 1);
      const maxFires = Math.max(1, Number.parseInt(ev.max_fires, 10) || 1);
      scheduledEvents.push({
        id: cleanId,
        name: ev.name.trim() || cleanId,
        trigger_kind: "METRIC_THRESHOLD",
        watch_node_id: ev.watch_node_id,
        watch_metric: ev.watch_metric.trim(),
        op: ev.op,
        threshold: threshVal,
        cooldown_steps: cooldown,
        max_fires: maxFires,
        actions: [
          {
            target_node_id: ev.action_node_id,
            metric_key: ev.action_metric_key.trim(),
            mode: ev.action_mode,
            value: actionVal,
          },
        ],
      });
    }
  }

  const goalConstraints: SimGoalConstraint[] = [];
  for (const g of goals) {
    const cleanId = g.id.trim();
    if (!cleanId) {
      return {
        ok: false,
        error: tx("红线标识不能为空", "Goal ID is required"),
      };
    }
    if (!g.metric_key.trim()) {
      return {
        ok: false,
        error: tx("红线考核指标不能为空", "Goal metric is required"),
      };
    }
    const goalThresh = Number.parseFloat(g.threshold);
    if (!Number.isFinite(goalThresh)) {
      return {
        ok: false,
        error: tx("红线阈值必须是有效数字", "Goal threshold must be numeric"),
      };
    }

    let spatialFilter: SpatialFilterSpec | null = null;
    if (g.spatial_kind === "GEO_WITHIN_POLYGON") {
      const poly = parsePolygonFilterInput(g.polygon_json);
      if (!poly) {
        return {
          ok: false,
          error: tx("多边形围栏 JSON 格式无效", "Invalid polygon GeoJSON"),
        };
      }
      spatialFilter = {
        op: "GEO_WITHIN_POLYGON" satisfies GeoPredicateOp,
        polygon: poly,
      };
    } else if (g.spatial_kind === "GEO_DWITHIN_METERS") {
      const lng = Number.parseFloat(g.center_lng);
      const lat = Number.parseFloat(g.center_lat);
      const rad = Number.parseFloat(g.radius_meters);
      if (!Number.isFinite(lng) || !Number.isFinite(lat) || !Number.isFinite(rad) || rad <= 0) {
        return {
          ok: false,
          error: tx(
            "球面半径过滤坐标或半径无效",
            "Invalid center coordinates or radius"
          ),
        };
      }
      spatialFilter = {
        op: "GEO_DWITHIN_METERS" satisfies GeoPredicateOp,
        center: { lng, lat },
        radius_meters: rad,
      };
    }

    goalConstraints.push({
      id: cleanId,
      name: g.name.trim() || cleanId,
      target_node_id: g.scope === "NODE" ? g.target_node_id || null : null,
      target_type_name:
        g.scope === "TYPE" ? g.target_type_name.trim() || null : null,
      metric_key: g.metric_key.trim(),
      op: g.op,
      threshold: goalThresh,
      spatial_filter: spatialFilter,
      controllable_node_ids: g.controllable_node_id
        ? [g.controllable_node_id]
        : [],
    });
  }

  return { ok: true, scheduledEvents, goalConstraints };
}

export function EventQueueEditor({
  nodes,
  events,
  goals,
  disabled = false,
  onChangeEvents,
  onChangeGoals,
}: EventQueueEditorProps) {
  const { tx, tEnum } = useI18n();

  const distinctTypes = Array.from(
    new Set(nodes.map((n) => n.type_name).filter(Boolean))
  );

  const thresholdOps: ReadonlyArray<SimThresholdOp> = ["LT", "LTE", "GT", "GTE"];
  const actionModes: ReadonlyArray<{ value: InterventionMode; label: string }> = [
    { value: "ADD", label: tx("增量叠加", "Additive") },
    { value: "RATIO", label: tx("比例变动", "Ratio") },
    { value: "ABSOLUTE", label: tx("绝对设定", "Absolute") },
  ];

  const updateEvent = (id: string, patch: Partial<EventDraft>) => {
    onChangeEvents(
      events.map((row) => (row.id === id ? { ...row, ...patch } : row))
    );
  };

  const updateGoal = (id: string, patch: Partial<GoalDraft>) => {
    onChangeGoals(
      goals.map((row) => (row.id === id ? { ...row, ...patch } : row))
    );
  };

  return (
    <div className="space-y-4" data-testid="event-queue-editor">
      {/* Scheduled & Conditional Events */}
      <div className="rounded-md border border-border bg-muted/15 p-3 space-y-2.5">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <div className="flex items-center gap-1.5 text-xs font-medium">
            <CalendarClock className="h-3.5 w-3.5 text-xblue" />
            <span>{tx("未来事件队列", "Event Queue")}</span>
          </div>
          {!disabled && (
            <Button
              type="button"
              variant="outline"
              size="sm"
              onClick={() =>
                onChangeEvents([...events, createEmptyEventDraft(nodes)])
              }
              data-testid="add-sim-event-btn"
            >
              <Plus className="h-3.5 w-3.5" />
              {tx("新增事件", "Add Event")}
            </Button>
          )}
        </div>

        {events.map((ev, idx) => {
          const watchNode = nodes.find((n) => n.id === ev.watch_node_id);
          const watchMetrics = watchNode
            ? Object.keys(watchNode.metrics ?? {})
            : [];
          const actNode = nodes.find((n) => n.id === ev.action_node_id);
          const actMetrics = actNode ? Object.keys(actNode.metrics ?? {}) : [];

          return (
            <div
              key={ev.id}
              data-testid={`sim-event-row-${idx}`}
              className="rounded border border-border bg-background p-2.5 space-y-2"
            >
              <div className="grid grid-cols-1 md:grid-cols-12 gap-2 items-end">
                <div className="md:col-span-3 space-y-1">
                  <label className="text-[11px] text-muted-foreground">
                    {tx("事件名称", "Event Name")}
                  </label>
                  <Input
                    value={ev.name}
                    disabled={disabled}
                    onChange={(e) => updateEvent(ev.id, { name: e.target.value })}
                    placeholder={ev.id}
                    className="h-8"
                    data-testid={`sim-event-name-${idx}`}
                  />
                </div>

                <div className="md:col-span-3 space-y-1">
                  <label className="text-[11px] text-muted-foreground">
                    {tx("触发类型", "Trigger")}
                  </label>
                  <Select
                    value={ev.trigger_kind}
                    disabled={disabled}
                    onChange={(e) =>
                      updateEvent(ev.id, {
                        trigger_kind: e.target.value as SimEventTriggerKind,
                      })
                    }
                    className="h-8 w-full"
                    data-testid={`sim-event-kind-${idx}`}
                  >
                    <option value="AT_STEP">{tEnum("AT_STEP")}</option>
                    <option value="METRIC_THRESHOLD">
                      {tEnum("METRIC_THRESHOLD")}
                    </option>
                  </Select>
                </div>

                {ev.trigger_kind === "AT_STEP" ? (
                  <div className="md:col-span-5 space-y-1">
                    <label className="text-[11px] text-muted-foreground">
                      {tx("生效步", "Step")}
                    </label>
                    <Input
                      type="number"
                      min={0}
                      max={104}
                      value={ev.at_step}
                      disabled={disabled}
                      onChange={(e) =>
                        updateEvent(ev.id, { at_step: e.target.value })
                      }
                      placeholder="4"
                      className="h-8"
                      data-testid={`sim-event-step-${idx}`}
                    />
                  </div>
                ) : (
                  <>
                    <div className="md:col-span-3 space-y-1">
                      <label className="text-[11px] text-muted-foreground">
                        {tx("监控节点", "Watch Node")}
                      </label>
                      <Select
                        value={ev.watch_node_id}
                        disabled={disabled}
                        onChange={(e) =>
                          updateEvent(ev.id, { watch_node_id: e.target.value })
                        }
                        className="h-8 w-full"
                      >
                        {nodes.map((n) => (
                          <option key={n.id} value={n.id}>
                            {n.node_key} · {n.title}
                          </option>
                        ))}
                      </Select>
                    </div>
                    <div className="md:col-span-2 space-y-1">
                      <label className="text-[11px] text-muted-foreground">
                        {tx("监控指标", "Metric")}
                      </label>
                      {watchMetrics.length > 0 ? (
                        <Select
                          value={ev.watch_metric}
                          disabled={disabled}
                          onChange={(e) =>
                            updateEvent(ev.id, { watch_metric: e.target.value })
                          }
                          className="h-8 w-full"
                        >
                          {watchMetrics.map((k) => (
                            <option key={k} value={k}>
                              {k}
                            </option>
                          ))}
                        </Select>
                      ) : (
                        <Input
                          value={ev.watch_metric}
                          disabled={disabled}
                          onChange={(e) =>
                            updateEvent(ev.id, { watch_metric: e.target.value })
                          }
                          placeholder="inventory"
                          className="h-8"
                        />
                      )}
                    </div>
                  </>
                )}

                <div className="md:col-span-1 flex justify-end">
                  <Button
                    type="button"
                    variant="ghost"
                    size="sm"
                    disabled={disabled}
                    onClick={() =>
                      onChangeEvents(events.filter((r) => r.id !== ev.id))
                    }
                    aria-label={tx("删除", "Delete")}
                    data-testid={`remove-sim-event-${idx}`}
                  >
                    <Trash2 className="h-3.5 w-3.5 text-muted-foreground" />
                  </Button>
                </div>
              </div>

              {ev.trigger_kind === "METRIC_THRESHOLD" && (
                <div className="grid grid-cols-2 md:grid-cols-4 gap-2">
                  <div className="space-y-1">
                    <label className="text-[11px] text-muted-foreground">
                      {tx("比较算子", "Operator")}
                    </label>
                    <Select
                      value={ev.op}
                      disabled={disabled}
                      onChange={(e) =>
                        updateEvent(ev.id, {
                          op: e.target.value as SimThresholdOp,
                        })
                      }
                      className="h-8 w-full"
                    >
                      {thresholdOps.map((op) => (
                        <option key={op} value={op}>
                          {tEnum(op)}
                        </option>
                      ))}
                    </Select>
                  </div>
                  <div className="space-y-1">
                    <label className="text-[11px] text-muted-foreground">
                      {tx("阈值", "Threshold")}
                    </label>
                    <Input
                      type="number"
                      step="any"
                      value={ev.threshold}
                      disabled={disabled}
                      onChange={(e) =>
                        updateEvent(ev.id, { threshold: e.target.value })
                      }
                      placeholder="200"
                      className="h-8"
                    />
                  </div>
                  <div className="space-y-1">
                    <label className="text-[11px] text-muted-foreground">
                      {tx("冷却步", "Cooldown")}
                    </label>
                    <Input
                      type="number"
                      min={1}
                      value={ev.cooldown_steps}
                      disabled={disabled}
                      onChange={(e) =>
                        updateEvent(ev.id, { cooldown_steps: e.target.value })
                      }
                      placeholder="2"
                      className="h-8"
                    />
                  </div>
                  <div className="space-y-1">
                    <label className="text-[11px] text-muted-foreground">
                      {tx("上限次数", "Max Fires")}
                    </label>
                    <Input
                      type="number"
                      min={1}
                      value={ev.max_fires}
                      disabled={disabled}
                      onChange={(e) =>
                        updateEvent(ev.id, { max_fires: e.target.value })
                      }
                      placeholder="1"
                      className="h-8"
                    />
                  </div>
                </div>
              )}

              <div className="grid grid-cols-1 md:grid-cols-12 gap-2 pt-1 border-t border-border/60">
                <div className="md:col-span-5 space-y-1">
                  <label className="text-[11px] text-muted-foreground">
                    {tx("作用节点", "Action Node")}
                  </label>
                  <Select
                    value={ev.action_node_id}
                    disabled={disabled}
                    onChange={(e) =>
                      updateEvent(ev.id, { action_node_id: e.target.value })
                    }
                    className="h-8 w-full"
                  >
                    {nodes.map((n) => (
                      <option key={n.id} value={n.id}>
                        [{n.type_name}] {n.node_key} · {n.title}
                      </option>
                    ))}
                  </Select>
                </div>
                <div className="md:col-span-3 space-y-1">
                  <label className="text-[11px] text-muted-foreground">
                    {tx("作用指标", "Action Metric")}
                  </label>
                  {actMetrics.length > 0 ? (
                    <Select
                      value={ev.action_metric_key}
                      disabled={disabled}
                      onChange={(e) =>
                        updateEvent(ev.id, { action_metric_key: e.target.value })
                      }
                      className="h-8 w-full"
                    >
                      {actMetrics.map((k) => (
                        <option key={k} value={k}>
                          {k}
                        </option>
                      ))}
                    </Select>
                  ) : (
                    <Input
                      value={ev.action_metric_key}
                      disabled={disabled}
                      onChange={(e) =>
                        updateEvent(ev.id, { action_metric_key: e.target.value })
                      }
                      placeholder="output"
                      className="h-8"
                    />
                  )}
                </div>
                <div className="md:col-span-2 space-y-1">
                  <label className="text-[11px] text-muted-foreground">
                    {tx("模式", "Mode")}
                  </label>
                  <Select
                    value={ev.action_mode}
                    disabled={disabled}
                    onChange={(e) =>
                      updateEvent(ev.id, {
                        action_mode: e.target.value as InterventionMode,
                      })
                    }
                    className="h-8 w-full"
                  >
                    {actionModes.map((m) => (
                      <option key={m.value} value={m.value}>
                        {m.label}
                      </option>
                    ))}
                  </Select>
                </div>
                <div className="md:col-span-2 space-y-1">
                  <label className="text-[11px] text-muted-foreground">
                    {tx("变动值", "Value")}
                  </label>
                  <Input
                    type="number"
                    step="any"
                    value={ev.action_value}
                    disabled={disabled}
                    onChange={(e) =>
                      updateEvent(ev.id, { action_value: e.target.value })
                    }
                    placeholder="-150"
                    className="h-8"
                  />
                </div>
              </div>
            </div>
          );
        })}
      </div>

      {/* Redline Goal Constraints */}
      <div className="rounded-md border border-border bg-muted/15 p-3 space-y-2.5">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <div className="flex items-center gap-1.5 text-xs font-medium">
            <ShieldAlert className="h-3.5 w-3.5 text-amber-600 dark:text-amber-400" />
            <span>{tx("目标门禁红线", "Goal Constraints")}</span>
          </div>
          {!disabled && (
            <Button
              type="button"
              variant="outline"
              size="sm"
              onClick={() =>
                onChangeGoals([...goals, createEmptyGoalDraft(nodes)])
              }
              data-testid="add-sim-goal-btn"
            >
              <Plus className="h-3.5 w-3.5" />
              {tx("新增红线", "Add Goal")}
            </Button>
          )}
        </div>

        {goals.map((g, idx) => {
          const targetNode = nodes.find((n) => n.id === g.target_node_id);
          const metricCandidates = targetNode
            ? Object.keys(targetNode.metrics ?? {})
            : [];

          return (
            <div
              key={g.id}
              data-testid={`sim-goal-row-${idx}`}
              className="rounded border border-border bg-background p-2.5 space-y-2"
            >
              <div className="grid grid-cols-1 md:grid-cols-12 gap-2 items-end">
                <div className="md:col-span-3 space-y-1">
                  <label className="text-[11px] text-muted-foreground">
                    {tx("红线名称", "Goal Name")}
                  </label>
                  <Input
                    value={g.name}
                    disabled={disabled}
                    onChange={(e) => updateGoal(g.id, { name: e.target.value })}
                    placeholder={g.id}
                    className="h-8"
                    data-testid={`sim-goal-name-${idx}`}
                  />
                </div>

                <div className="md:col-span-2 space-y-1">
                  <label className="text-[11px] text-muted-foreground">
                    {tx("范围模式", "Scope")}
                  </label>
                  <Select
                    value={g.scope}
                    disabled={disabled}
                    onChange={(e) =>
                      updateGoal(g.id, {
                        scope: e.target.value as GoalTargetScope,
                      })
                    }
                    className="h-8 w-full"
                  >
                    <option value="NODE">{tx("单节点", "Single Node")}</option>
                    <option value="TYPE">{tx("对象类型", "Node Type")}</option>
                  </Select>
                </div>

                <div className="md:col-span-3 space-y-1">
                  <label className="text-[11px] text-muted-foreground">
                    {g.scope === "NODE"
                      ? tx("目标节点", "Target Node")
                      : tx("对象类型", "Node Type")}
                  </label>
                  {g.scope === "NODE" ? (
                    <Select
                      value={g.target_node_id}
                      disabled={disabled}
                      onChange={(e) =>
                        updateGoal(g.id, { target_node_id: e.target.value })
                      }
                      className="h-8 w-full"
                    >
                      {nodes.map((n) => (
                        <option key={n.id} value={n.id}>
                          {n.node_key} · {n.title}
                        </option>
                      ))}
                    </Select>
                  ) : distinctTypes.length > 0 ? (
                    <Select
                      value={g.target_type_name}
                      disabled={disabled}
                      onChange={(e) =>
                        updateGoal(g.id, { target_type_name: e.target.value })
                      }
                      className="h-8 w-full"
                    >
                      {distinctTypes.map((t) => (
                        <option key={t} value={t}>
                          {t}
                        </option>
                      ))}
                    </Select>
                  ) : (
                    <Input
                      value={g.target_type_name}
                      disabled={disabled}
                      onChange={(e) =>
                        updateGoal(g.id, { target_type_name: e.target.value })
                      }
                      placeholder="Warehouse"
                      className="h-8"
                    />
                  )}
                </div>

                <div className="md:col-span-3 space-y-1">
                  <label className="text-[11px] text-muted-foreground">
                    {tx("考核指标", "Metric")}
                  </label>
                  {g.scope === "NODE" && metricCandidates.length > 0 ? (
                    <Select
                      value={g.metric_key}
                      disabled={disabled}
                      onChange={(e) =>
                        updateGoal(g.id, { metric_key: e.target.value })
                      }
                      className="h-8 w-full"
                    >
                      {metricCandidates.map((k) => (
                        <option key={k} value={k}>
                          {k}
                        </option>
                      ))}
                    </Select>
                  ) : (
                    <Input
                      value={g.metric_key}
                      disabled={disabled}
                      onChange={(e) =>
                        updateGoal(g.id, { metric_key: e.target.value })
                      }
                      placeholder="inventory"
                      className="h-8"
                    />
                  )}
                </div>

                <div className="md:col-span-1 flex justify-end">
                  <Button
                    type="button"
                    variant="ghost"
                    size="sm"
                    disabled={disabled}
                    onClick={() =>
                      onChangeGoals(goals.filter((r) => r.id !== g.id))
                    }
                    aria-label={tx("删除", "Delete")}
                    data-testid={`remove-sim-goal-${idx}`}
                  >
                    <Trash2 className="h-3.5 w-3.5 text-muted-foreground" />
                  </Button>
                </div>
              </div>

              <div className="grid grid-cols-1 md:grid-cols-12 gap-2 pt-1 border-t border-border/60">
                <div className="md:col-span-2 space-y-1">
                  <label className="text-[11px] text-muted-foreground">
                    {tx("比较算子", "Operator")}
                  </label>
                  <Select
                    value={g.op}
                    disabled={disabled}
                    onChange={(e) =>
                      updateGoal(g.id, { op: e.target.value as SimThresholdOp })
                    }
                    className="h-8 w-full"
                  >
                    {thresholdOps.map((op) => (
                      <option key={op} value={op}>
                        {tEnum(op)}
                      </option>
                    ))}
                  </Select>
                </div>

                <div className="md:col-span-2 space-y-1">
                  <label className="text-[11px] text-muted-foreground">
                    {tx("红线阈值", "Threshold")}
                  </label>
                  <Input
                    type="number"
                    step="any"
                    value={g.threshold}
                    disabled={disabled}
                    onChange={(e) =>
                      updateGoal(g.id, { threshold: e.target.value })
                    }
                    placeholder="200"
                    className="h-8"
                  />
                </div>

                <div className="md:col-span-4 space-y-1">
                  <label className="text-[11px] text-muted-foreground">
                    {tx("可控源头", "Controllable Source")}
                  </label>
                  <Select
                    value={g.controllable_node_id}
                    disabled={disabled}
                    onChange={(e) =>
                      updateGoal(g.id, { controllable_node_id: e.target.value })
                    }
                    className="h-8 w-full"
                  >
                    <option value="">{tx("全部上游", "All Upstream")}</option>
                    {nodes.map((n) => (
                      <option key={n.id} value={n.id}>
                        [{n.type_name}] {n.node_key} · {n.title}
                      </option>
                    ))}
                  </Select>
                </div>

                <div className="md:col-span-4 space-y-1">
                  <label className="text-[11px] text-muted-foreground">
                    {tx("空间过滤", "Spatial Filter")}
                  </label>
                  <Select
                    value={g.spatial_kind}
                    disabled={disabled}
                    onChange={(e) =>
                      updateGoal(g.id, {
                        spatial_kind: e.target.value as GoalSpatialKind,
                      })
                    }
                    className="h-8 w-full"
                  >
                    <option value="NONE">{tx("不限区域", "Unbounded")}</option>
                    <option value="GEO_WITHIN_POLYGON">
                      {tEnum("GEO_WITHIN_POLYGON")}
                    </option>
                    <option value="GEO_DWITHIN_METERS">
                      {tEnum("GEO_DWITHIN_METERS")}
                    </option>
                  </Select>
                </div>
              </div>

              {g.spatial_kind === "GEO_WITHIN_POLYGON" && (
                <div className="space-y-1">
                  <label className="text-[11px] text-muted-foreground">
                    {tx("多边形坐标环", "Polygon Rings")}
                  </label>
                  <Input
                    value={g.polygon_json}
                    disabled={disabled}
                    onChange={(e) =>
                      updateGoal(g.id, { polygon_json: e.target.value })
                    }
                    placeholder="[[[119.5,29.8],[122.2,29.8],[122.2,32.0],[119.5,32.0],[119.5,29.8]]]"
                    className="h-8 font-mono text-xs"
                  />
                </div>
              )}

              {g.spatial_kind === "GEO_DWITHIN_METERS" && (
                <div className="grid grid-cols-3 gap-2">
                  <div className="space-y-1">
                    <label className="text-[11px] text-muted-foreground">
                      {tx("中心经度", "Center Lng")}
                    </label>
                    <Input
                      type="number"
                      step="any"
                      value={g.center_lng}
                      disabled={disabled}
                      onChange={(e) =>
                        updateGoal(g.id, { center_lng: e.target.value })
                      }
                      placeholder="121.47"
                      className="h-8"
                    />
                  </div>
                  <div className="space-y-1">
                    <label className="text-[11px] text-muted-foreground">
                      {tx("中心纬度", "Center Lat")}
                    </label>
                    <Input
                      type="number"
                      step="any"
                      value={g.center_lat}
                      disabled={disabled}
                      onChange={(e) =>
                        updateGoal(g.id, { center_lat: e.target.value })
                      }
                      placeholder="31.23"
                      className="h-8"
                    />
                  </div>
                  <div className="space-y-1">
                    <label className="text-[11px] text-muted-foreground">
                      {tx("半径米数", "Radius (m)")}
                    </label>
                    <Input
                      type="number"
                      min={1}
                      step="any"
                      value={g.radius_meters}
                      disabled={disabled}
                      onChange={(e) =>
                        updateGoal(g.id, { radius_meters: e.target.value })
                      }
                      placeholder="100000"
                      className="h-8"
                    />
                  </div>
                </div>
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}
