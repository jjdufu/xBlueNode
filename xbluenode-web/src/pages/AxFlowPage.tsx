import { useCallback, useEffect, useMemo, useState } from "react";
import {
  GitBranch,
  Play,
  Plus,
  Rocket,
  Save,
  Settings2,
  Trash2,
  Webhook,
} from "lucide-react";
import {
  DEFAULT_RETRY_POLICY,
  DEFAULT_TRIGGER_CFG,
  FALLBACK_OPERATOR_SPECS,
  axflowApi,
  fetchMe,
  getAccessToken,
  type ActionLogRecord,
  type ApprovalDecision,
  type ApprovalRecord,
  type ApprovalStatus,
  type Environment,
  type FlowDeploymentRecord,
  type FlowEdgeDef,
  type FlowIssue,
  type FlowNodeDef,
  type FlowOperator,
  type FlowRecord,
  type FlowRunRecord,
  type FlowStepRecord,
  type FlowVersionRecord,
  type OperatorSpecDto,
  type StartFlowRunRequest,
  type TriggerCfg,
} from "@/lib/api";
import { ApprovalInbox } from "@/components/axflow/ApprovalInbox";
import {
  FlowCanvas,
  type NodeStepRuntimeBadge,
} from "@/components/axflow/FlowCanvas";
import { NodeConfigForm } from "@/components/axflow/NodeConfigForm";
import { OperatorPalette } from "@/components/axflow/OperatorPalette";
import { RunHistory } from "@/components/axflow/RunHistory";
import { RunTimeline } from "@/components/axflow/RunTimeline";
import { TriggerEditor } from "@/components/axflow/TriggerEditor";
import { VersionDeployPanel } from "@/components/axflow/VersionDeployPanel";
import { Badge, Button, Card, Input } from "@/components/ui/primitives";
import { cn } from "@/lib/utils";

type InspectorTab = "node" | "trigger" | "version";

function buildDefaultConfigFromSpec(
  spec: OperatorSpecDto
): Record<string, unknown> {
  const cfg: Record<string, unknown> = {};
  const props = spec.config_schema.properties ?? {};
  for (const [key, propSpec] of Object.entries(props)) {
    if (propSpec.default !== undefined) {
      cfg[key] = structuredClone(propSpec.default);
    }
  }
  return cfg;
}

function generateUniqueNodeId(
  kind: FlowOperator,
  existingNodes: readonly FlowNodeDef[]
): string {
  const existing = new Set(existingNodes.map((n) => n.id));
  const base = kind
    .toLowerCase()
    .replace(/[^a-z0-9_]/g, "_")
    .replace(/^[^a-z]+/, "n_")
    .slice(0, 24);
  let idx = 1;
  while (idx < 999) {
    const candidate = `${base}_${idx}`.slice(0, 32);
    if (!existing.has(candidate)) {
      return candidate;
    }
    idx += 1;
  }
  return `node_${Date.now() % 100000}`;
}

export function AxFlowPage() {
  const [operators, setOperators] = useState<OperatorSpecDto[]>([
    ...FALLBACK_OPERATOR_SPECS,
  ]);
  const [tenantId, setTenantId] = useState("default");
  const [flows, setFlows] = useState<FlowRecord[]>([]);
  const [selectedFlowId, setSelectedFlowId] = useState<string | null>(null);

  // Create flow drawer state
  const [createOpen, setCreateOpen] = useState(false);
  const [newFlowName, setNewFlowName] = useState("");
  const [newFlowDesc, setNewFlowDesc] = useState("");
  const [newFlowSlug, setNewFlowSlug] = useState("");
  const [creating, setCreating] = useState(false);

  // Draft DAG editing state
  const [draftName, setDraftName] = useState("");
  const [draftDesc, setDraftDesc] = useState("");
  const [draftSlug, setDraftSlug] = useState<string | null>(null);
  const [draftNodes, setDraftNodes] = useState<FlowNodeDef[]>([]);
  const [draftEdges, setDraftEdges] = useState<FlowEdgeDef[]>([]);
  const [draftTrigger, setDraftTrigger] =
    useState<TriggerCfg>(DEFAULT_TRIGGER_CFG);
  const [draftIssues, setDraftIssues] = useState<FlowIssue[]>([]);
  const [isDirty, setIsDirty] = useState(false);

  // Selection & Inspector state
  const [selectedNodeId, setSelectedNodeId] = useState<string | null>(null);
  const [selectedEdgeIndex, setSelectedEdgeIndex] = useState<number | null>(
    null
  );
  const [inspectorTab, setInspectorTab] = useState<InspectorTab>("node");

  // Versions & Deployments state
  const [versions, setVersions] = useState<FlowVersionRecord[]>([]);
  const [deployments, setDeployments] = useState<FlowDeploymentRecord[]>([]);
  const [saving, setSaving] = useState(false);
  const [validating, setValidating] = useState(false);
  const [publishing, setPublishing] = useState(false);
  const [deployingEnv, setDeployingEnv] = useState<Environment | null>(null);

  // Runs & Timeline state
  const [runEnvironment, setRunEnvironment] = useState<Environment>("TEST");
  const [startingRun, setStartingRun] = useState(false);
  const [runs, setRuns] = useState<FlowRunRecord[]>([]);
  const [selectedRunId, setSelectedRunId] = useState<string | null>(null);
  const [activeRunDetail, setActiveRunDetail] =
    useState<FlowRunRecord | null>(null);
  const [runSteps, setRunSteps] = useState<FlowStepRecord[]>([]);
  const [runActions, setRunActions] = useState<ActionLogRecord[]>([]);

  // Approvals state
  const [approvals, setApprovals] = useState<ApprovalRecord[]>([]);
  const [approvalStatusFilter, setApprovalStatusFilter] = useState<
    ApprovalStatus | "ALL"
  >("PENDING");
  const [approvalMineOnly, setApprovalMineOnly] = useState(true);
  const [decidingApprovalId, setDecidingApprovalId] = useState<string | null>(
    null
  );

  // Feedback banners
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  const hydrateDraftFromFlow = useCallback((flow: FlowRecord) => {
    setDraftName(flow.name);
    setDraftDesc(flow.description);
    setDraftSlug(flow.slug ?? null);
    setDraftNodes(flow.nodes);
    setDraftEdges(flow.edges);
    setDraftTrigger(flow.trigger_cfg);
    setDraftIssues(flow.issues);
    setIsDirty(false);
    setSelectedNodeId(flow.nodes[0]?.id ?? null);
    setSelectedEdgeIndex(null);
  }, []);

  const loadApprovals = useCallback(async () => {
    try {
      const res = await axflowApi.listApprovals({
        status:
          approvalStatusFilter === "ALL" ? undefined : approvalStatusFilter,
        mine: approvalMineOnly ? true : undefined,
      });
      setApprovals(res.items);
    } catch {
      // ignore offline
    }
  }, [approvalStatusFilter, approvalMineOnly]);

  const loadFlowVersionsAndRuns = useCallback(
    async (flowId: string, preferredRunId?: string | null) => {
      try {
        const [verRes, runsRes] = await Promise.all([
          axflowApi.listVersions(flowId),
          axflowApi.listRuns(flowId),
        ]);
        setVersions(verRes.items);
        setDeployments(verRes.deployments);
        setRuns(runsRes.items);

        const nextRunId =
          preferredRunId !== undefined
            ? preferredRunId
            : runsRes.items[0]?.id ?? null;
        setSelectedRunId(nextRunId);
        if (!nextRunId) {
          setActiveRunDetail(null);
          setRunSteps([]);
          setRunActions([]);
        }
      } catch {
        setVersions([]);
        setDeployments([]);
        setRuns([]);
      }
    },
    []
  );

  const refreshRunTelemetry = useCallback(
    async (runId: string, fallbackList?: FlowRunRecord[]) => {
      try {
        const [detail, stepsRes, actionsRes] = await Promise.all([
          axflowApi.getRun(runId),
          axflowApi.listRunSteps(runId),
          axflowApi.listActions({ run_id: runId }),
        ]);

        if (detail.id === runId) {
          setActiveRunDetail(detail);
          setRuns((prev) => {
            const exists = prev.some((r) => r.id === runId);
            if (!exists) return [detail, ...prev];
            return prev.map((r) => (r.id === runId ? detail : r));
          });
        } else if (fallbackList) {
          const matched = fallbackList.find((r) => r.id === runId) ?? null;
          if (matched) setActiveRunDetail(matched);
        }

        setRunSteps(stepsRes.items);
        setRunActions(actionsRes.items);
      } catch {
        // ignore transient polling errors
      }
    },
    []
  );

  // Initial bootstrap
  useEffect(() => {
    let cancelled = false;
    const bootstrap = async () => {
      try {
        const [ops, flowPage] = await Promise.all([
          axflowApi.listOperators().catch(() => [...FALLBACK_OPERATOR_SPECS]),
          axflowApi.listFlows(),
        ]);
        if (cancelled) return;
        setOperators(ops);
        setFlows(flowPage.items);

        const first = flowPage.items[0] ?? null;
        if (first) {
          setSelectedFlowId(first.id);
          hydrateDraftFromFlow(first);
        }
      } catch {
        // ignore offline
      }

      if (getAccessToken()) {
        try {
          const me = await fetchMe();
          if (!cancelled && me.tenant_id) {
            setTenantId(me.tenant_id);
          }
        } catch {
          // ignore
        }
      }
    };

    void bootstrap();
    return () => {
      cancelled = true;
    };
  }, [hydrateDraftFromFlow]);

  // Reload versions & runs when selected flow changes
  useEffect(() => {
    if (!selectedFlowId) {
      setVersions([]);
      setDeployments([]);
      setRuns([]);
      setSelectedRunId(null);
      setActiveRunDetail(null);
      setRunSteps([]);
      setRunActions([]);
      return;
    }

    let cancelled = false;
    const syncSelectedFlow = async () => {
      try {
        const detail = await axflowApi.getFlow(selectedFlowId);
        if (!cancelled && detail.id === selectedFlowId) {
          setFlows((prev) =>
            prev.map((f) => (f.id === selectedFlowId ? detail : f))
          );
          hydrateDraftFromFlow(detail);
        }
      } catch {
        // keep list item state
      }
      if (!cancelled) {
        await loadFlowVersionsAndRuns(selectedFlowId);
      }
    };

    void syncSelectedFlow();
    return () => {
      cancelled = true;
    };
  }, [selectedFlowId, hydrateDraftFromFlow, loadFlowVersionsAndRuns]);

  // Reload approvals when filter changes
  useEffect(() => {
    void loadApprovals();
  }, [loadApprovals]);

  // Load telemetry when selectedRunId changes
  useEffect(() => {
    if (!selectedRunId) {
      setActiveRunDetail(null);
      setRunSteps([]);
      setRunActions([]);
      return;
    }
    const listMatch = runs.find((r) => r.id === selectedRunId) ?? null;
    if (listMatch) {
      setActiveRunDetail(listMatch);
    }
    void refreshRunTelemetry(selectedRunId);
  }, [selectedRunId, refreshRunTelemetry]);

  // Auto-poll while selected run is non-terminal
  const isRunActive =
    activeRunDetail !== null &&
    (activeRunDetail.status === "PENDING" ||
      activeRunDetail.status === "RUNNING" ||
      activeRunDetail.status === "WAITING_APPROVAL");

  useEffect(() => {
    if (!isRunActive || !selectedRunId) return;
    const timer = window.setInterval(() => {
      void refreshRunTelemetry(selectedRunId);
      void loadApprovals();
    }, 1500);
    return () => window.clearInterval(timer);
  }, [isRunActive, selectedRunId, refreshRunTelemetry, loadApprovals]);

  // Compute latest step attempt badge per node for FlowCanvas
  const stepStatusByNodeId = useMemo(() => {
    const map: Record<string, NodeStepRuntimeBadge> = {};
    for (const st of runSteps) {
      const prev = map[st.node_id];
      if (!prev || st.attempt >= prev.attempt) {
        map[st.node_id] = { status: st.status, attempt: st.attempt };
      }
    }
    return map;
  }, [runSteps]);

  const selectedNode = useMemo(
    () => draftNodes.find((n) => n.id === selectedNodeId) ?? null,
    [draftNodes, selectedNodeId]
  );

  const selectedEdgeObj = useMemo(() => {
    if (selectedEdgeIndex === null) return null;
    const edge = draftEdges[selectedEdgeIndex];
    return edge ? { index: selectedEdgeIndex, edge } : null;
  }, [draftEdges, selectedEdgeIndex]);

  const testDeployment = useMemo(
    () => deployments.find((d) => d.environment === "TEST") ?? null,
    [deployments]
  );
  const prodDeployment = useMemo(
    () => deployments.find((d) => d.environment === "PROD") ?? null,
    [deployments]
  );

  // Handlers: Flow CRUD
  const handleSelectFlowId = (nextId: string) => {
    if (nextId === selectedFlowId) return;
    setError(null);
    setNotice(null);
    setSelectedFlowId(nextId);
    const found = flows.find((f) => f.id === nextId);
    if (found) {
      hydrateDraftFromFlow(found);
    }
  };

  const handleCreateFlow = async () => {
    const trimmed = newFlowName.trim();
    if (!trimmed) return;
    setCreating(true);
    setError(null);
    setNotice(null);
    try {
      const starterNodes: FlowNodeDef[] = [
        {
          id: "ingest_source",
          kind: "SOURCE_INGEST",
          label: "数据源同步入湖",
          config: { dataset_id: "", wait: true, timeout_s: 300 },
          retry: { ...DEFAULT_RETRY_POLICY },
          timeout_s: 600,
          on_failure: "FAIL_RUN",
          join: "ALL_SUCCESS",
          pos: { x: 80, y: 150 },
        },
        {
          id: "transform_check",
          kind: "SQL_TRANSFORM",
          label: "SQL 变换与质量核验",
          config: { sql: "SELECT 1 AS n", max_rows: 100 },
          retry: { ...DEFAULT_RETRY_POLICY },
          timeout_s: 600,
          on_failure: "FAIL_RUN",
          join: "ALL_SUCCESS",
          pos: { x: 360, y: 150 },
        },
      ];
      const starterEdges: FlowEdgeDef[] = [
        { from: "ingest_source", to: "transform_check", when: null },
      ];

      const created = await axflowApi.createFlow({
        name: trimmed,
        description:
          newFlowDesc.trim() || "持久化有向无环图编排与自动回写流水线",
        slug: newFlowSlug.trim() || undefined,
        nodes: starterNodes,
        edges: starterEdges,
        trigger_cfg: { ...DEFAULT_TRIGGER_CFG },
      });

      const refreshed = await axflowApi.listFlows();
      const list =
        refreshed.items.length > 0 ? refreshed.items : [created, ...flows];
      setFlows(list);
      const target = list.find((f) => f.id === created.id) ?? created;
      setSelectedFlowId(target.id);
      hydrateDraftFromFlow(target);
      setNewFlowName("");
      setNewFlowDesc("");
      setNewFlowSlug("");
      setCreateOpen(false);
      setNotice(`已创建流水线草稿「${target.name}」`);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setCreating(false);
    }
  };

  const handleDeleteCurrentFlow = async () => {
    if (!selectedFlowId) return;
    setError(null);
    setNotice(null);
    try {
      await axflowApi.deleteFlow(selectedFlowId);
      const remaining = flows.filter((f) => f.id !== selectedFlowId);
      setFlows(remaining);
      const next = remaining[0] ?? null;
      setSelectedFlowId(next?.id ?? null);
      if (next) {
        hydrateDraftFromFlow(next);
      } else {
        setDraftNodes([]);
        setDraftEdges([]);
        setDraftName("");
        setDraftDesc("");
      }
      setNotice("已删除流水线");
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  // Handlers: Canvas & Node Editing
  const addOperatorNode = useCallback(
    (spec: OperatorSpecDto, explicitPos?: { x: number; y: number }) => {
      const newId = generateUniqueNodeId(spec.kind, draftNodes);
      const pos = explicitPos ?? {
        x: 80 + (draftNodes.length % 4) * 260,
        y: 120 + Math.floor(draftNodes.length / 4) * 160,
      };
      const newNode: FlowNodeDef = {
        id: newId,
        kind: spec.kind,
        label: spec.display_name.replace(/\s*\([A-Z_]+\)\s*$/, ""),
        config: buildDefaultConfigFromSpec(spec),
        retry: { ...DEFAULT_RETRY_POLICY },
        timeout_s: 600,
        on_failure: "FAIL_RUN",
        join: "ALL_SUCCESS",
        pos,
      };
      setDraftNodes((prev) => [...prev, newNode]);
      setSelectedEdgeIndex(null);
      setSelectedNodeId(newId);
      setInspectorTab("node");
      setIsDirty(true);
    },
    [draftNodes]
  );

  const handleDropOperator = useCallback(
    (kind: FlowOperator, pos: { x: number; y: number }) => {
      const spec =
        operators.find((o) => o.kind === kind) ??
        FALLBACK_OPERATOR_SPECS.find((o) => o.kind === kind);
      if (spec) {
        addOperatorNode(spec, pos);
      }
    },
    [operators, addOperatorNode]
  );

  const handleUpdateNodePos = useCallback(
    (nodeId: string, pos: { x: number; y: number }) => {
      setDraftNodes((prev) =>
        prev.map((n) => (n.id === nodeId ? { ...n, pos } : n))
      );
      setIsDirty(true);
    },
    []
  );

  const handleApplyLayout = useCallback(
    (positions: Record<string, { x: number; y: number }>) => {
      setDraftNodes((prev) =>
        prev.map((n) => (positions[n.id] ? { ...n, pos: positions[n.id] } : n))
      );
      setIsDirty(true);
    },
    []
  );

  const handleConnectNodes = useCallback(
    (from: string, to: string, when?: string | null) => {
      if (!from || !to || from === to) return;
      setDraftEdges((prev) => {
        const exists = prev.some((e) => e.from === from && e.to === to);
        if (exists) return prev;
        return [...prev, { from, to, when: when ?? null }];
      });
      setIsDirty(true);
    },
    []
  );

  const handleUpdateNode = useCallback((updated: FlowNodeDef) => {
    setDraftNodes((prev) =>
      prev.map((n) => (n.id === updated.id ? updated : n))
    );
    setIsDirty(true);
  }, []);

  const handleDeleteNode = useCallback((nodeId: string) => {
    setDraftNodes((prev) => prev.filter((n) => n.id !== nodeId));
    setDraftEdges((prev) =>
      prev.filter((e) => e.from !== nodeId && e.to !== nodeId)
    );
    setSelectedNodeId(null);
    setIsDirty(true);
  }, []);

  const handleUpdateEdge = useCallback(
    (edgeIndex: number, updated: FlowEdgeDef) => {
      setDraftEdges((prev) =>
        prev.map((e, idx) => (idx === edgeIndex ? updated : e))
      );
      setIsDirty(true);
    },
    []
  );

  const handleDeleteEdge = useCallback((edgeIndex: number) => {
    setDraftEdges((prev) => prev.filter((_, idx) => idx !== edgeIndex));
    setSelectedEdgeIndex(null);
    setIsDirty(true);
  }, []);

  // Handlers: Save / Validate / Publish / Deploy
  const handleSaveDraft = async (): Promise<boolean> => {
    if (!selectedFlowId) return false;
    setSaving(true);
    setError(null);
    setNotice(null);
    try {
      const updated = await axflowApi.updateFlow(selectedFlowId, {
        name: draftName,
        description: draftDesc,
        slug: draftSlug ?? "",
        nodes: draftNodes,
        edges: draftEdges,
        trigger_cfg: draftTrigger,
      });
      if (updated.id) {
        setFlows((prev) =>
          prev.map((f) => (f.id === selectedFlowId ? updated : f))
        );
        setDraftIssues(updated.issues);
      }
      setIsDirty(false);
      setNotice("流程草稿已保存");
      return true;
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
      return false;
    } finally {
      setSaving(false);
    }
  };

  const handleValidateDraft = async () => {
    if (!selectedFlowId) return;
    setValidating(true);
    setError(null);
    try {
      if (isDirty) {
        const ok = await handleSaveDraft();
        if (!ok) return;
      }
      const res = await axflowApi.validateFlow(selectedFlowId);
      setDraftIssues(res.issues);
      setNotice(
        res.valid
          ? "静态校验通过，可发布新版本"
          : `静态校验发现 ${res.issues.length} 项问题`
      );
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setValidating(false);
    }
  };

  const handlePublishVersion = async (note: string) => {
    if (!selectedFlowId) return;
    setPublishing(true);
    setError(null);
    setNotice(null);
    try {
      if (isDirty) {
        const saved = await handleSaveDraft();
        if (!saved) return;
      }
      const ver = await axflowApi.publishVersion(selectedFlowId, { note });
      await loadFlowVersionsAndRuns(selectedFlowId, selectedRunId);
      setNotice(`已发布不可变版本 v${ver.version}`);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setPublishing(false);
    }
  };

  const handleDeployVersion = async (
    environment: Environment,
    version: number
  ) => {
    if (!selectedFlowId) return;
    setDeployingEnv(environment);
    setError(null);
    setNotice(null);
    try {
      await axflowApi.deployFlow(selectedFlowId, { environment, version });
      const verRes = await axflowApi.listVersions(selectedFlowId);
      setVersions(verRes.items);
      setDeployments(verRes.deployments);
      setNotice(`已将 v${version} 部署至 ${environment} 环境`);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setDeployingEnv(null);
    }
  };

  // Handlers: Runs & Approvals
  const handleStartRun = async (req?: StartFlowRunRequest) => {
    if (!selectedFlowId) return;
    setStartingRun(true);
    setError(null);
    setNotice(null);
    try {
      const effectiveReq: StartFlowRunRequest = {
        environment: req?.environment ?? runEnvironment,
        version: req?.version,
        payload: req?.payload ?? {},
        idempotency_key: req?.idempotency_key,
      };
      const started = await axflowApi.startRun(selectedFlowId, effectiveReq);
      const runsRes = await axflowApi.listRuns(selectedFlowId);
      setRuns(runsRes.items);

      const runId = started.run_id || runsRes.items[0]?.id || null;
      if (runId) {
        setSelectedRunId(runId);
        await refreshRunTelemetry(runId, runsRes.items);
      }
      await loadApprovals();
      setNotice(
        started.existing
          ? `命中幂等键，已复用运行 #${started.run_id.slice(0, 8)}`
          : `已启动 ${effectiveReq.environment} 运行 #${started.run_id.slice(0, 8)}`
      );
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setStartingRun(false);
    }
  };

  const handleCancelRun = async (runId: string) => {
    setError(null);
    try {
      const canceled = await axflowApi.cancelRun(runId);
      setActiveRunDetail(canceled);
      await refreshRunTelemetry(runId);
      if (selectedFlowId) {
        const runsRes = await axflowApi.listRuns(selectedFlowId);
        setRuns(runsRes.items);
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  const handleRetryRun = async (runId: string) => {
    setError(null);
    try {
      const retried = await axflowApi.retryRun(runId);
      setActiveRunDetail(retried);
      await refreshRunTelemetry(runId);
      if (selectedFlowId) {
        const runsRes = await axflowApi.listRuns(selectedFlowId);
        setRuns(runsRes.items);
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  const handleDecideApproval = async (
    approvalId: string,
    decision: ApprovalDecision,
    comment: string
  ) => {
    setDecidingApprovalId(approvalId);
    setError(null);
    setNotice(null);
    try {
      await axflowApi.decideApproval(approvalId, { decision, comment });
      await loadApprovals();
      if (selectedRunId) {
        await refreshRunTelemetry(selectedRunId);
      }
      if (selectedFlowId) {
        const runsRes = await axflowApi.listRuns(selectedFlowId);
        setRuns(runsRes.items);
      }
      setNotice(
        decision === "APPROVE"
          ? "审批已通过，下游分支正在继续推进"
          : "已拒绝该审批门禁"
      );
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setDecidingApprovalId(null);
    }
  };

  return (
    <div className="space-y-4">
      {/* Page Header & Global Flow Bar */}
      <div className="flex flex-col lg:flex-row lg:items-center lg:justify-between gap-3 border-b border-border pb-4">
        <div>
          <h1 className="text-lg font-semibold tracking-tight">
            AxFlow · 智能编排流水线
          </h1>
          <p className="text-xs text-muted-foreground mt-0.5">
            有向无环图（DAG）持久化编排 · TEST/PROD 双环境版本治理 · 人工审批门禁与业务动作回写
          </p>
        </div>

        <div className="flex flex-wrap items-center gap-2">
          {/* Flow Selector */}
          <select
            data-testid="axflow-flow-select"
            value={selectedFlowId ?? ""}
            onChange={(e) => handleSelectFlowId(e.target.value)}
            disabled={flows.length === 0}
            className="h-8 min-w-48 rounded-md border border-input bg-background px-2.5 text-xs font-medium"
          >
            {flows.length === 0 ? (
              <option value="">暂无流水线</option>
            ) : (
              flows.map((f) => (
                <option key={f.id} value={f.id}>
                  {f.name}
                </option>
              ))
            )}
          </select>

          {/* Active Deployment Badges */}
          {testDeployment && (
            <Badge variant="neutral" data-testid="axflow-badge-test">
              TEST v{testDeployment.version}
            </Badge>
          )}
          {prodDeployment && (
            <Badge variant="xblue" data-testid="axflow-badge-prod">
              PROD v{prodDeployment.version}
            </Badge>
          )}

          <Button
            type="button"
            variant="outline"
            className="h-8 px-2.5 text-xs"
            data-testid="axflow-new-flow-toggle"
            onClick={() => setCreateOpen((v) => !v)}
          >
            <Plus className="h-3.5 w-3.5" />
            新建流水线
          </Button>

          {selectedFlowId && (
            <>
              <Button
                type="button"
                variant="outline"
                className="h-8 px-2.5 text-xs"
                data-testid="header-save-flow-btn"
                disabled={saving}
                onClick={() => void handleSaveDraft()}
              >
                <Save className="h-3.5 w-3.5" />
                保存草稿
              </Button>

              <div className="flex items-center gap-1 rounded-md border border-border bg-muted/30 p-0.5">
                <select
                  data-testid="axflow-run-env-select"
                  value={runEnvironment}
                  onChange={(e) =>
                    setRunEnvironment(e.target.value as Environment)
                  }
                  className="h-7 rounded bg-background px-2 font-mono-tech text-[11px] border border-input"
                >
                  <option value="TEST">TEST</option>
                  <option value="PROD">PROD</option>
                </select>
                <Button
                  type="button"
                  className="h-7 px-3 text-xs"
                  data-testid="axflow-run-btn"
                  disabled={startingRun}
                  onClick={() => void handleStartRun()}
                >
                  <Play className="h-3.5 w-3.5" />
                  {startingRun ? "启动中..." : "运行流水线"}
                </Button>
              </div>
            </>
          )}
        </div>
      </div>

      {/* Error & Notice Alerts */}
      {error && (
        <Card
          data-testid="axflow-error-banner"
          className="p-3 border-red-500/40 bg-red-500/5 text-xs text-red-700 dark:text-red-300"
        >
          <span className="font-mono-tech font-semibold">操作异常：</span>
          {error}
        </Card>
      )}

      {notice && (
        <Card
          data-testid="axflow-notice-banner"
          className="p-2.5 border-xblue/30 bg-xblue-soft/60 text-xs text-foreground"
        >
          {notice}
        </Card>
      )}

      {/* Inline Create Flow Form */}
      {createOpen && (
        <Card
          data-testid="axflow-create-card"
          className="p-3.5 flex flex-col md:flex-row items-stretch md:items-center gap-2"
        >
          <Input
            data-testid="new-flow-name-input"
            placeholder="流水线名称（如：daily_supply_refresh）"
            value={newFlowName}
            onChange={(e) => setNewFlowName(e.target.value)}
            className="h-8 text-xs"
          />
          <Input
            data-testid="new-flow-desc-input"
            placeholder="业务用途说明（可选）"
            value={newFlowDesc}
            onChange={(e) => setNewFlowDesc(e.target.value)}
            className="h-8 text-xs"
          />
          <Input
            data-testid="new-flow-slug-input"
            placeholder="Webhook Slug（可选）"
            value={newFlowSlug}
            onChange={(e) => setNewFlowSlug(e.target.value)}
            className="h-8 text-xs font-mono-tech md:w-48"
          />
          <div className="flex items-center gap-1.5 shrink-0">
            <Button
              type="button"
              className="h-8 text-xs"
              data-testid="confirm-create-flow-btn"
              disabled={!newFlowName.trim() || creating}
              onClick={() => void handleCreateFlow()}
            >
              <Plus className="h-3.5 w-3.5" />
              {creating ? "创建中..." : "确认创建"}
            </Button>
            <Button
              type="button"
              variant="ghost"
              className="h-8 text-xs"
              onClick={() => setCreateOpen(false)}
            >
              取消
            </Button>
          </div>
        </Card>
      )}

      {/* Flow Metadata Strip */}
      {selectedFlowId && (
        <Card className="px-3.5 py-2.5 flex flex-wrap items-center justify-between gap-2 text-xs">
          <div className="flex flex-wrap items-center gap-2 flex-1 min-w-60">
            <GitBranch className="h-3.5 w-3.5 text-xblue shrink-0" />
            <Input
              data-testid="flow-draft-name-input"
              value={draftName}
              onChange={(e) => {
                setDraftName(e.target.value);
                setIsDirty(true);
              }}
              className="h-7 w-52 text-xs font-semibold"
            />
            <Input
              data-testid="flow-draft-desc-input"
              value={draftDesc}
              onChange={(e) => {
                setDraftDesc(e.target.value);
                setIsDirty(true);
              }}
              placeholder="流程说明..."
              className="h-7 flex-1 min-w-44 text-xs text-muted-foreground"
            />
          </div>
          <div className="flex items-center gap-2">
            <Badge variant="outline">触发: {draftTrigger.type}</Badge>
            {isDirty && <Badge variant="xblue">草稿已变更</Badge>}
            <Button
              type="button"
              variant="ghost"
              className="h-7 px-2 text-xs"
              data-testid="delete-flow-btn"
              onClick={() => void handleDeleteCurrentFlow()}
              title="删除当前流水线"
            >
              <Trash2 className="h-3.5 w-3.5" />
            </Button>
          </div>
        </Card>
      )}

      {/* Main 3-Column DAG Workbench */}
      <div className="grid grid-cols-1 xl:grid-cols-12 gap-4 items-start">
        {/* Left: Operator Palette */}
        <div className="xl:col-span-3">
          <OperatorPalette
            operators={operators}
            onAddOperator={(spec) => addOperatorNode(spec)}
          />
        </div>

        {/* Center: ReactFlow DAG Canvas */}
        <div className="xl:col-span-6">
          <FlowCanvas
            nodes={draftNodes}
            edges={draftEdges}
            issues={draftIssues}
            stepStatusByNodeId={stepStatusByNodeId}
            selectedNodeId={selectedNodeId}
            selectedEdgeIndex={selectedEdgeIndex}
            onSelectNode={(id) => {
              setSelectedNodeId(id);
              if (id) setInspectorTab("node");
            }}
            onSelectEdge={(idx) => {
              setSelectedEdgeIndex(idx);
              if (idx !== null) setInspectorTab("node");
            }}
            onUpdateNodePos={handleUpdateNodePos}
            onApplyLayout={handleApplyLayout}
            onConnectNodes={handleConnectNodes}
            onUpdateEdge={handleUpdateEdge}
            onDeleteEdge={handleDeleteEdge}
            onDropOperator={handleDropOperator}
          />
        </div>

        {/* Right: Tabbed Inspector (Node / Trigger / Version & Deploy) */}
        <div className="xl:col-span-3 flex flex-col h-[540px] rounded-md border border-border bg-card text-card-foreground overflow-hidden">
          <div className="grid grid-cols-3 border-b border-border bg-muted/30 p-1 gap-1 text-xs">
            <button
              type="button"
              data-testid="inspector-tab-node"
              onClick={() => setInspectorTab("node")}
              className={cn(
                "inline-flex items-center justify-center gap-1 rounded py-1.5 font-medium transition-colors cursor-pointer",
                inspectorTab === "node"
                  ? "bg-card text-foreground shadow-xs"
                  : "text-muted-foreground hover:text-foreground"
              )}
            >
              <Settings2 className="h-3.5 w-3.5" />
              节点配置
            </button>
            <button
              type="button"
              data-testid="inspector-tab-trigger"
              onClick={() => setInspectorTab("trigger")}
              className={cn(
                "inline-flex items-center justify-center gap-1 rounded py-1.5 font-medium transition-colors cursor-pointer",
                inspectorTab === "trigger"
                  ? "bg-card text-foreground shadow-xs"
                  : "text-muted-foreground hover:text-foreground"
              )}
            >
              <Webhook className="h-3.5 w-3.5" />
              触发器
            </button>
            <button
              type="button"
              data-testid="inspector-tab-version"
              onClick={() => setInspectorTab("version")}
              className={cn(
                "inline-flex items-center justify-center gap-1 rounded py-1.5 font-medium transition-colors cursor-pointer",
                inspectorTab === "version"
                  ? "bg-card text-foreground shadow-xs"
                  : "text-muted-foreground hover:text-foreground"
              )}
            >
              <Rocket className="h-3.5 w-3.5" />
              版本部署
            </button>
          </div>

          <div className="flex-1 overflow-y-auto p-3">
            {inspectorTab === "node" && (
              <NodeConfigForm
                node={selectedNode}
                allNodes={draftNodes}
                allEdges={draftEdges}
                operators={operators}
                selectedEdge={selectedEdgeObj}
                onChangeNode={handleUpdateNode}
                onDeleteNode={handleDeleteNode}
                onUpdateEdge={handleUpdateEdge}
                onDeleteEdge={handleDeleteEdge}
              />
            )}

            {inspectorTab === "trigger" && (
              <TriggerEditor
                flowId={selectedFlowId}
                slug={draftSlug}
                triggerCfg={draftTrigger}
                tenantId={tenantId}
                onChangeTrigger={(next) => {
                  setDraftTrigger(next);
                  setIsDirty(true);
                }}
                onChangeSlug={(nextSlug) => {
                  setDraftSlug(nextSlug);
                  setIsDirty(true);
                }}
              />
            )}

            {inspectorTab === "version" && (
              <VersionDeployPanel
                isDirty={isDirty}
                saving={saving}
                validating={validating}
                publishing={publishing}
                deployingEnv={deployingEnv}
                issues={draftIssues}
                versions={versions}
                deployments={deployments}
                onSaveDraft={() => void handleSaveDraft()}
                onValidateDraft={() => void handleValidateDraft()}
                onPublishVersion={handlePublishVersion}
                onDeployVersion={handleDeployVersion}
                onFocusNode={(nid) => {
                  setSelectedEdgeIndex(null);
                  setSelectedNodeId(nid);
                  setInspectorTab("node");
                }}
              />
            )}
          </div>
        </div>
      </div>

      {/* Bottom Operations & Governance Console: RunHistory + RunTimeline + ApprovalInbox */}
      <div className="grid grid-cols-1 xl:grid-cols-12 gap-4 items-start">
        <div className="xl:col-span-4">
          <RunHistory
            runs={runs}
            selectedRunId={selectedRunId}
            runEnvironment={runEnvironment}
            startingRun={startingRun}
            onChangeEnvironment={setRunEnvironment}
            onStartRun={handleStartRun}
            onSelectRun={(id) => setSelectedRunId(id)}
            onCancelRun={handleCancelRun}
            onRetryRun={handleRetryRun}
            onRefresh={async () => {
              if (selectedFlowId) {
                const runsRes = await axflowApi.listRuns(selectedFlowId);
                setRuns(runsRes.items);
              }
              if (selectedRunId) {
                await refreshRunTelemetry(selectedRunId);
              }
            }}
          />
        </div>

        <div className="xl:col-span-5">
          <RunTimeline
            run={activeRunDetail}
            steps={runSteps}
            actions={runActions}
            nodes={draftNodes}
            polling={isRunActive}
          />
        </div>

        <div className="xl:col-span-3">
          <ApprovalInbox
            approvals={approvals}
            statusFilter={approvalStatusFilter}
            mineOnly={approvalMineOnly}
            decidingId={decidingApprovalId}
            onChangeStatusFilter={setApprovalStatusFilter}
            onChangeMineOnly={setApprovalMineOnly}
            onDecide={handleDecideApproval}
            onRefresh={loadApprovals}
          />
        </div>
      </div>
    </div>
  );
}
