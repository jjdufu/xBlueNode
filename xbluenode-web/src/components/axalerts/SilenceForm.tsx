import { useMemo, useState } from "react";
import { BellOff, Clock, Plus, Trash2 } from "lucide-react";
import type {
  AlertSilenceRecord,
  CreateSilenceRequest,
  RuleDef,
  Severity,
} from "@/lib/api";
import { Badge, Button, Card, Input } from "@/components/ui/primitives";
import { cn } from "@/lib/utils";
import { getSeverityBadgeClass } from "./AlertTable";

export interface SilenceFormProps {
  silences: readonly AlertSilenceRecord[];
  rules: readonly RuleDef[];
  knownTypeNames: readonly string[];
  saving: boolean;
  onCreateSilence: (req: CreateSilenceRequest) => Promise<void>;
  onDeleteSilence: (id: string) => Promise<void>;
}

const ALL_SEVERITIES: readonly Severity[] = ["CRITICAL", "WARN", "INFO"];

function toLocalDatetimeInputValue(date: Date): string {
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(
    date.getDate()
  )}T${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

function formatWindowTime(iso: string): string {
  if (!iso) return "—";
  const parsed = new Date(iso);
  if (Number.isNaN(parsed.getTime())) return iso;
  return parsed.toLocaleString("zh-CN", {
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    hour12: false,
  });
}

function getSilenceWindowState(
  silence: AlertSilenceRecord
): { label: string; active: boolean } {
  const now = Date.now();
  const start = new Date(silence.starts_at).getTime();
  const end = new Date(silence.ends_at).getTime();
  if (Number.isFinite(start) && Number.isFinite(end)) {
    if (now < start) return { label: "UPCOMING · 待生效", active: false };
    if (now >= end) return { label: "EXPIRED · 已过期", active: false };
  }
  return { label: "ACTIVE · 生效中", active: true };
}

export function SilenceForm({
  silences,
  rules,
  knownTypeNames,
  saving,
  onCreateSilence,
  onDeleteSilence,
}: SilenceFormProps) {
  const [selectedSeverities, setSelectedSeverities] = useState<Severity[]>([
    "WARN",
  ]);
  const [selectedRuleIds, setSelectedRuleIds] = useState<string[]>([]);
  const [typeNamesCsv, setTypeNamesCsv] = useState("Warehouse");
  const [nodeIdsCsv, setNodeIdsCsv] = useState("");
  const [startsLocal, setStartsLocal] = useState(() =>
    toLocalDatetimeInputValue(new Date())
  );
  const [endsLocal, setEndsLocal] = useState(() =>
    toLocalDatetimeInputValue(new Date(Date.now() + 2 * 3600 * 1000))
  );
  const [reason, setReason] = useState("");
  const [formError, setFormError] = useState<string | null>(null);

  const ruleNameMap = useMemo(() => {
    const map = new Map<string, string>();
    for (const r of rules) {
      map.set(r.id, r.name);
    }
    return map;
  }, [rules]);

  const applyDurationPreset = (minutes: number) => {
    const now = new Date();
    const end = new Date(now.getTime() + minutes * 60 * 1000);
    setStartsLocal(toLocalDatetimeInputValue(now));
    setEndsLocal(toLocalDatetimeInputValue(end));
  };

  const toggleSeverity = (sev: Severity) => {
    setSelectedSeverities((prev) =>
      prev.includes(sev) ? prev.filter((item) => item !== sev) : [...prev, sev]
    );
  };

  const toggleRuleId = (id: string) => {
    setSelectedRuleIds((prev) =>
      prev.includes(id) ? prev.filter((item) => item !== id) : [...prev, id]
    );
  };

  const appendTypeName = (tn: string) => {
    const existing = typeNamesCsv
      .split(",")
      .map((s) => s.trim())
      .filter((s) => s.length > 0);
    if (!existing.includes(tn)) {
      setTypeNamesCsv([...existing, tn].join(", "));
    }
  };

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!reason.trim()) {
      setFormError("请填写静默原因 (reason)");
      return;
    }

    const startDate = new Date(startsLocal);
    const endDate = new Date(endsLocal);
    if (
      Number.isNaN(startDate.getTime()) ||
      Number.isNaN(endDate.getTime()) ||
      endDate.getTime() <= startDate.getTime()
    ) {
      setFormError("静默结束时间 (ends_at) 必须晚于开始时间 (starts_at)");
      return;
    }

    const typeNames = typeNamesCsv
      .split(",")
      .map((s) => s.trim())
      .filter((s) => s.length > 0);
    const nodeIds = nodeIdsCsv
      .split(",")
      .map((s) => s.trim())
      .filter((s) => s.length > 0);

    setFormError(null);
    await onCreateSilence({
      matcher: {
        severities: selectedSeverities,
        rule_ids: selectedRuleIds,
        type_names: typeNames,
        node_ids: nodeIds,
      },
      starts_at: startDate.toISOString(),
      ends_at: endDate.toISOString(),
      reason: reason.trim(),
    });

    setReason("");
  };

  return (
    <div className="grid grid-cols-1 lg:grid-cols-12 gap-4 items-start">
      {/* Left: Silences List */}
      <Card className="lg:col-span-7 overflow-hidden" data-testid="silences-list">
        <div className="flex items-center justify-between border-b border-border bg-muted/30 px-3.5 py-3">
          <div className="flex items-center gap-2 text-xs font-medium">
            <BellOff className="h-3.5 w-3.5 text-xblue" />
            <span>告警静默窗口 (ax_alert_silences)</span>
            <Badge variant="outline">{silences.length}</Badge>
          </div>
        </div>

        <div className="divide-y divide-border">
          {silences.length === 0 ? (
            <div className="p-8 text-center text-xs text-muted-foreground">
              当前无静默窗口，所有活动告警按渠道配置正常投递
            </div>
          ) : (
            silences.map((s) => {
              const state = getSilenceWindowState(s);
              return (
                <div
                  key={s.id}
                  data-testid={`silence-row-${s.id}`}
                  className="p-3.5 space-y-2 text-xs"
                >
                  <div className="flex items-start justify-between gap-2">
                    <div className="space-y-1">
                      <div className="flex flex-wrap items-center gap-1.5">
                        <Badge variant={state.active ? "xblue" : "outline"}>
                          {state.label}
                        </Badge>
                        <span className="font-medium text-foreground">
                          {s.reason}
                        </span>
                      </div>
                      <div className="inline-flex items-center gap-1.5 text-[11px] font-mono-tech text-muted-foreground">
                        <Clock className="h-3 w-3" />
                        <span>
                          {formatWindowTime(s.starts_at)} →{" "}
                          {formatWindowTime(s.ends_at)}
                        </span>
                      </div>
                    </div>

                    <Button
                      type="button"
                      variant="ghost"
                      className="h-7 px-2 text-[11px] text-red-600 dark:text-red-400"
                      data-testid={`delete-silence-btn-${s.id}`}
                      onClick={() => {
                        void onDeleteSilence(s.id);
                      }}
                    >
                      <Trash2 className="h-3.5 w-3.5" />
                      删除
                    </Button>
                  </div>

                  {/* Matcher Summary */}
                  <div className="flex flex-wrap items-center gap-1.5">
                    {s.matcher.severities.map((sev) => (
                      <span
                        key={sev}
                        className={cn(
                          "inline-flex items-center rounded px-1.5 py-0.5 text-[10px] font-mono-tech font-medium",
                          getSeverityBadgeClass(sev)
                        )}
                      >
                        {sev}
                      </span>
                    ))}

                    {s.matcher.type_names.map((tn) => (
                      <Badge key={tn} variant="neutral">
                        类型: {tn}
                      </Badge>
                    ))}

                    {s.matcher.rule_ids.map((rid) => (
                      <Badge key={rid} variant="outline">
                        规则: {ruleNameMap.get(rid) ?? rid.slice(0, 8)}
                      </Badge>
                    ))}

                    {s.matcher.node_ids.map((nid) => (
                      <Badge key={nid} variant="outline">
                        节点: {nid.slice(0, 8)}
                      </Badge>
                    ))}

                    {s.matcher.severities.length === 0 &&
                      s.matcher.type_names.length === 0 &&
                      s.matcher.rule_ids.length === 0 &&
                      s.matcher.node_ids.length === 0 && (
                        <Badge variant="outline">全局匹配（全部告警）</Badge>
                      )}
                  </div>
                </div>
              );
            })
          )}
        </div>
      </Card>

      {/* Right: Create Silence Form */}
      <Card className="lg:col-span-5 p-4">
        <form
          onSubmit={(e) => {
            void handleSubmit(e);
          }}
          className="space-y-3.5"
          data-testid="silence-form"
        >
          <div className="text-xs font-semibold border-b border-border pb-2.5">
            创建维护期静默窗口
          </div>

          {formError && (
            <div className="rounded-md border border-red-500/40 bg-red-500/10 px-3 py-2 text-xs text-red-600 dark:text-red-400">
              {formError}
            </div>
          )}

          {/* Severity Matcher */}
          <div className="space-y-1.5">
            <label className="block text-[11px] text-muted-foreground">
              限定严重级别 (severities，留空不限)
            </label>
            <div className="flex flex-wrap gap-1.5">
              {ALL_SEVERITIES.map((sev) => {
                const active = selectedSeverities.includes(sev);
                return (
                  <button
                    key={sev}
                    type="button"
                    data-testid={`silence-severity-${sev}`}
                    onClick={() => toggleSeverity(sev)}
                    className={cn(
                      "rounded px-2.5 py-1 text-[11px] font-mono-tech border transition-colors cursor-pointer",
                      active
                        ? "border-xblue bg-xblue-soft text-xblue font-semibold"
                        : "border-border bg-background text-muted-foreground"
                    )}
                  >
                    {sev}
                  </button>
                );
              })}
            </div>
          </div>

          {/* Rule IDs Matcher */}
          <div className="space-y-1.5">
            <label className="block text-[11px] text-muted-foreground">
              限定规则 (rule_ids，留空不限)
            </label>
            {rules.length === 0 ? (
              <div className="text-[11px] text-muted-foreground">暂无规则可选</div>
            ) : (
              <div className="flex flex-wrap gap-1.5 max-h-28 overflow-y-auto">
                {rules.map((r) => {
                  const active = selectedRuleIds.includes(r.id);
                  return (
                    <button
                      key={r.id}
                      type="button"
                      data-testid={`silence-rule-${r.id}`}
                      onClick={() => toggleRuleId(r.id)}
                      className={cn(
                        "rounded px-2 py-0.5 text-[11px] border transition-colors cursor-pointer",
                        active
                          ? "border-xblue bg-xblue-soft text-xblue font-medium"
                          : "border-border bg-background text-muted-foreground"
                      )}
                    >
                      {r.name}
                    </button>
                  );
                })}
              </div>
            )}
          </div>

          {/* Type Names & Node IDs */}
          <div>
            <div className="flex items-center justify-between mb-1">
              <label className="text-[11px] text-muted-foreground">
                限定业务节点类型 (type_names，逗号分隔)
              </label>
              <div className="flex items-center gap-1">
                {knownTypeNames.slice(0, 3).map((tn) => (
                  <button
                    key={tn}
                    type="button"
                    onClick={() => appendTypeName(tn)}
                    className="text-[10px] font-mono-tech text-xblue hover:underline cursor-pointer"
                  >
                    +{tn}
                  </button>
                ))}
              </div>
            </div>
            <Input
              data-testid="silence-type-names-input"
              value={typeNamesCsv}
              onChange={(e) => setTypeNamesCsv(e.target.value)}
              placeholder="Warehouse, Order"
              className="h-8 text-xs font-mono-tech"
            />
          </div>

          <div>
            <label className="block text-[11px] text-muted-foreground mb-1">
              限定特定节点 UUID (node_ids，可选，逗号分隔)
            </label>
            <Input
              data-testid="silence-node-ids-input"
              value={nodeIdsCsv}
              onChange={(e) => setNodeIdsCsv(e.target.value)}
              placeholder="00000000-0000-0000-0000-000000000000"
              className="h-8 text-xs font-mono-tech"
            />
          </div>

          {/* Time Window */}
          <div className="space-y-2">
            <div className="flex items-center justify-between">
              <span className="text-[11px] text-muted-foreground">
                静默时间窗口 (ends_at &gt; starts_at)
              </span>
              <div className="flex items-center gap-1">
                {[
                  { label: "30m", min: 30 },
                  { label: "2h", min: 120 },
                  { label: "6h", min: 360 },
                  { label: "24h", min: 1440 },
                ].map((preset) => (
                  <button
                    key={preset.label}
                    type="button"
                    onClick={() => applyDurationPreset(preset.min)}
                    className="rounded border border-border bg-muted/50 px-1.5 py-0.5 text-[10px] font-mono-tech hover:border-xblue hover:text-xblue cursor-pointer"
                  >
                    {preset.label}
                  </button>
                ))}
              </div>
            </div>

            <div className="grid grid-cols-1 sm:grid-cols-2 gap-2">
              <div>
                <label className="block text-[10px] text-muted-foreground mb-0.5">
                  开始时间 (starts_at)
                </label>
                <Input
                  type="datetime-local"
                  data-testid="silence-starts-input"
                  value={startsLocal}
                  onChange={(e) => setStartsLocal(e.target.value)}
                  className="h-8 text-xs font-mono-tech"
                />
              </div>
              <div>
                <label className="block text-[10px] text-muted-foreground mb-0.5">
                  结束时间 (ends_at)
                </label>
                <Input
                  type="datetime-local"
                  data-testid="silence-ends-input"
                  value={endsLocal}
                  onChange={(e) => setEndsLocal(e.target.value)}
                  className="h-8 text-xs font-mono-tech"
                />
              </div>
            </div>
          </div>

          <div>
            <label className="block text-[11px] text-muted-foreground mb-1">
              静默原因说明 (reason)
            </label>
            <Input
              data-testid="silence-reason-input"
              value={reason}
              onChange={(e) => setReason(e.target.value)}
              placeholder="例如：华东仓例行盘点维护窗口"
              className="h-8 text-xs"
            />
          </div>

          <div className="flex justify-end pt-1">
            <Button
              type="submit"
              variant="primary"
              data-testid="create-silence-btn"
              disabled={saving}
            >
              <Plus className="h-3.5 w-3.5" />
              {saving ? "创建中..." : "创建静默窗口"}
            </Button>
          </div>
        </form>
      </Card>
    </div>
  );
}
