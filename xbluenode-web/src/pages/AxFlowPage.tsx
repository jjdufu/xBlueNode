import { useCallback, useEffect, useMemo, useState } from "react";
import { useSearchParams } from "react-router";
import {
  Activity,
  Bell,
  ChevronDown,
  ChevronUp,
  GitBranch,
  Play,
  Plus,
  Rocket,
  Save,
  Settings2,
  Trash2,
  UserCheck,
  Webhook,
  Workflow,
  X,
} from "lucide-react";
import { useI18n } from "@/i18n";
import { MODULE_META } from "@/modules/meta";
import { usePrincipal } from "@/lib/permissions";
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
import {
  OperatorPalette,
  resolveOperatorTitle,
} from "@/components/axflow/OperatorPalette";
import {
  RunHistory,
  renderRunStatusBadge,
} from "@/components/axflow/RunHistory";
import { RunTimeline } from "@/components/axflow/RunTimeline";
import { TriggerEditor } from "@/components/axflow/TriggerEditor";
import { VersionDeployPanel } from "@/components/axflow/VersionDeployPanel";
import {
  Badge,
  Button,
  Card,
  Drawer,
  Input,
  Modal,
  PageHeader,
  SegmentedControl,
  Select,
  StatText,
  Tabs,
} from "@/components/ui/primitives";
import { ResizableWorkspace } from "@/components/ui/ResizableWorkspace";
import { AxAlertsPage } from "@/pages/AxAlertsPage";

type FlowViewMode = "workflows" | "alerts";
type InspectorTab = "node" | "version";

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
  const { isZh, tx, tEnum } = useI18n();
  const [searchParams, setSearchParams] = useSearchParams();
  const tabParam = (searchParams.get("tab") ?? "").trim().toLowerCase();
  const flowMode: FlowViewMode =
    tabParam === "alerts" || tabParam === "rules" ? "alerts" : "workflows";
  const { me, can } = usePrincipal();
  const canEdit = can("FLOW_EDIT");
  const canRun = can("FLOW_RUN");

  const [operators, setOperators] = useState<OperatorSpecDto[]>([
    ...FALLBACK_OPERATOR_SPECS,
  ]);
  const [tenantId, setTenantId] = useState(me?.tenant_id || "default");
  const [flows, setFlows] = useState<FlowRecord[]>([]);
  const [selectedFlowId, setSelectedFlowId] = useState<string | null>(null);

  // Create flow modal state
  const [createOpen, setCreateOpen] = useState(false);
  const [newFlowName, setNewFlowName] = useState("");
  const [newFlowDesc, setNewFlowDesc] = useState("");
  const [newFlowSlug, setNewFlowSlug] = useState("");
  const [creating, setCreating] = useState(false);
  const [confirmDeleteFlow, setConfirmDeleteFlow] = useState(false);

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

  // Selection, Inspector & Trigger Popover state
  const [selectedNodeId, setSelectedNodeId] = useState<string | null>(null);
  const [selectedEdgeIndex, setSelectedEdgeIndex] = useState<number | null>(
    null
  );
  const [inspectorTab, setInspectorTab] = useState<InspectorTab>("node");
  const [triggerPopoverOpen, setTriggerPopoverOpen] = useState(false);

  // Versions & Deployments state
  const [versions, setVersions] = useState<FlowVersionRecord[]>([]);
  const [deployments, setDeployments] = useState<FlowDeploymentRecord[]>([]);
  const [saving, setSaving] = useState(false);
  const [validating, setValidating] = useState(false);
  const [publishing, setPublishing] = useState(false);
  const [deployingEnv, setDeployingEnv] = useState<Environment | null>(null);

  // Runs & Collapsible IDE Run Monitor state
  const [runEnvironment, setRunEnvironment] = useState<Environment>("TEST");
  const [startingRun, setStartingRun] = useState(false);
  const [runs, setRuns] = useState<FlowRunRecord[]>([]);
  const [selectedRunId, setSelectedRunId] = useState<string | null>(null);
  const [activeRunDetail, setActiveRunDetail] =
    useState<FlowRunRecord | null>(null);
  const [runSteps, setRunSteps] = useState<FlowStepRecord[]>([]);
  const [runActions, setRunActions] = useState<ActionLogRecord[]>([]);
  // Collapsed 34px IDE status bar by default; auto-expands on run trigger
  const [runDockCollapsed, setRunDockCollapsed] = useState<boolean>(true);

  // Global Approvals Drawer state
  const [approvalDrawerOpen, setApprovalDrawerOpen] = useState(
    () => tabParam === "approvals"
  );
  const [approvals, setApprovals] = useState<ApprovalRecord[]>([]);
  const [approvalStatusFilter, setApprovalStatusFilter] = useState<
    ApprovalStatus | "ALL"
  >("PENDING");
  const [approvalMineOnly, setApprovalMineOnly] = useState(true);
  const [decidingApprovalId, setDecidingApprovalId] = useState<string | null>(
    null
  );

  useEffect(() => {
    if (tabParam === "runs") {
      setRunDockCollapsed(false);
    } else if (tabParam === "approvals") {
      setApprovalDrawerOpen(true);
    }
  }, [tabParam]);

  const handleFlowModeChange = (next: FlowViewMode) => {
    const updated = new URLSearchParams(searchParams);
    if (next === "alerts") {
      updated.set("tab", "alerts");
    } else {
      updated.delete("tab");
    }
    setSearchParams(updated, { replace: true });
  };

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
    setConfirmDeleteFlow(false);
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
            : (runsRes.items[0]?.id ?? null);
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
        return detail;
      } catch {
        return null;
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
          const profile = await fetchMe();
          if (!cancelled && profile.tenant_id) {
            setTenantId(profile.tenant_id);
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

  useEffect(() => {
    void loadApprovals();
  }, [loadApprovals]);

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
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selectedRunId, refreshRunTelemetry]);

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

  useEffect(() => {
    if (!triggerPopoverOpen) return;
    const onKey = (ev: KeyboardEvent) => {
      if (ev.key === "Escape") setTriggerPopoverOpen(false);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [triggerPopoverOpen]);

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

  const pendingApprovalsCount = useMemo(
    () => approvals.filter((a) => a.status === "PENDING").length,
    [approvals]
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
    if (!canEdit) return;
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
          label: tx("数据源同步入湖", "Sync Source Dataset"),
          config: { dataset_id: "", wait: true, timeout_s: 300 },
          retry: { ...DEFAULT_RETRY_POLICY },
          timeout_s: 600,
          on_failure: "FAIL_RUN",
          join: "ALL_SUCCESS",
          pos: { x: 80, y: 144 },
        },
        {
          id: "transform_check",
          kind: "SQL_TRANSFORM",
          label: tx("SQL 变换与核验", "SQL Transform & Check"),
          config: { sql: "SELECT 1 AS n", max_rows: 100 },
          retry: { ...DEFAULT_RETRY_POLICY },
          timeout_s: 600,
          on_failure: "FAIL_RUN",
          join: "ALL_SUCCESS",
          pos: { x: 368, y: 144 },
        },
      ];
      const starterEdges: FlowEdgeDef[] = [
        { from: "ingest_source", to: "transform_check", when: null },
      ];

      const created = await axflowApi.createFlow({
        name: trimmed,
        description:
          newFlowDesc.trim() ||
          tx("有向无环图编排流水线", "Directed acyclic pipeline"),
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
      setNotice(
        tx(
          `已创建流水线草稿「${target.name}」`,
          `Created pipeline draft "${target.name}"`
        )
      );
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setCreating(false);
    }
  };

  const handleDeleteCurrentFlow = async () => {
    if (!selectedFlowId || !canEdit) return;
    setError(null);
    setNotice(null);
    setConfirmDeleteFlow(false);
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
      setNotice(tx("已删除流水线", "Pipeline deleted"));
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  // Handlers: Canvas & Node Editing
  const addOperatorNode = useCallback(
    (spec: OperatorSpecDto, explicitPos?: { x: number; y: number }) => {
      if (!canEdit) return;
      const newId = generateUniqueNodeId(spec.kind, draftNodes);
      const pos = explicitPos ?? {
        x: 80 + (draftNodes.length % 4) * 272,
        y: 128 + Math.floor(draftNodes.length / 4) * 160,
      };
      const newNode: FlowNodeDef = {
        id: newId,
        kind: spec.kind,
        label: resolveOperatorTitle(spec, isZh),
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
      setTriggerPopoverOpen(false);
      setInspectorTab("node");
      setIsDirty(true);
    },
    [canEdit, draftNodes, isZh]
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
    if (!selectedFlowId || !canEdit) return false;
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
      setNotice(tx("草稿已保存", "Draft saved"));
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
      if (isDirty && canEdit) {
        const ok = await handleSaveDraft();
        if (!ok) return;
      }
      const res = await axflowApi.validateFlow(selectedFlowId);
      setDraftIssues(res.issues);
      setNotice(
        res.valid
          ? tx("静态校验通过", "Static validation passed")
          : tx(
              `静态校验发现 ${res.issues.length} 项问题`,
              `Validation found ${res.issues.length} issues`
            )
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
      if (isDirty && canEdit) {
        const saved = await handleSaveDraft();
        if (!saved) return;
      }
      const ver = await axflowApi.publishVersion(selectedFlowId, { note });
      await loadFlowVersionsAndRuns(selectedFlowId, selectedRunId);
      setNotice(
        tx(`已发布版本 v${ver.version}`, `Published version v${ver.version}`)
      );
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
      setNotice(
        tx(
          `已将 v${version} 部署至 ${environment} 环境`,
          `Deployed v${version} to ${environment}`
        )
      );
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setDeployingEnv(null);
    }
  };

  // Handlers: Runs & Approvals
  const handleStartRun = async (req?: StartFlowRunRequest) => {
    if (!selectedFlowId || !canRun) return;
    setStartingRun(true);
    setRunDockCollapsed(false);
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
      let detail: FlowRunRecord | null = null;
      if (runId) {
        setSelectedRunId(runId);
        detail = await refreshRunTelemetry(runId, runsRes.items);
      }
      await loadApprovals();
      if (
        detail?.status === "WAITING_APPROVAL" ||
        runsRes.items.find((r) => r.id === runId)?.status === "WAITING_APPROVAL"
      ) {
        setApprovalDrawerOpen(true);
      }
      setNotice(
        started.existing
          ? tx(
              `命中幂等键，复用运行 #${started.run_id.slice(0, 8)}`,
              `Reused idempotent run #${started.run_id.slice(0, 8)}`
            )
          : tx(
              `已启动 ${effectiveReq.environment} 运行 #${started.run_id.slice(0, 8)}`,
              `Started ${effectiveReq.environment} run #${started.run_id.slice(0, 8)}`
            )
      );
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setStartingRun(false);
    }
  };

  const handleCancelRun = async (runId: string) => {
    if (!canRun) return;
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
    if (!canRun) return;
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
          ? tx("审批已通过，下游步骤继续推进", "Approval granted")
          : tx("已驳回该审批门禁", "Approval rejected")
      );
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setDecidingApprovalId(null);
    }
  };

  return (
    <div className="flex flex-1 min-h-0 flex-col gap-2 overflow-hidden">
      {/* Unified Header — Single Source of Save & Run Actions + Global Approval Drawer Entry */}
      <PageHeader
        className="shrink-0 pb-2"
        title={tx(MODULE_META.axflow.label, MODULE_META.axflow.enLabel)}
        badges={
          <div className="flex items-center gap-2 pl-1">
            <StatText
              label={tx("流水线", "Flows")}
              value={flows.length}
            />
          </div>
        }
        actions={
          <>
            <SegmentedControl<FlowViewMode>
              value={flowMode}
              onChange={handleFlowModeChange}
              items={[
                {
                  key: "workflows",
                  label: tx("流程编排", "Workflows"),
                  icon: <Workflow className="h-3.5 w-3.5" />,
                  testId: "axflow-mode-workflows",
                },
                {
                  key: "alerts",
                  label: tx("规则告警", "Alerts"),
                  icon: <Bell className="h-3.5 w-3.5" />,
                  testId: "axflow-mode-alerts",
                },
              ]}
            />

            {flowMode === "workflows" && (
              <>
                <Select
                  data-testid="axflow-flow-select"
                  value={selectedFlowId ?? ""}
                  onChange={(e) => handleSelectFlowId(e.target.value)}
                  disabled={flows.length === 0}
                  className="h-8 w-52 font-medium"
                >
                  {flows.length === 0 ? (
                    <option value="">{tx("暂无流水线", "No pipelines")}</option>
                  ) : (
                    flows.map((f) => (
                      <option key={f.id} value={f.id}>
                        {f.name}
                      </option>
                    ))
                  )}
                </Select>

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
                  data-testid="open-approval-drawer-btn"
                  onClick={() => setApprovalDrawerOpen(true)}
                >
                  <UserCheck className="h-3.5 w-3.5 text-xblue" />
                  <span>{tx("审批待办", "Approvals")}</span>
                  {pendingApprovalsCount > 0 && (
                    <Badge variant="warning" data-testid="approval-inbox-count">
                      {pendingApprovalsCount}
                    </Badge>
                  )}
                </Button>

                {canEdit && (
                  <Button
                    type="button"
                    variant="outline"
                    data-testid="axflow-new-flow-toggle"
                    onClick={() => setCreateOpen(true)}
                  >
                    <Plus className="h-3.5 w-3.5" />
                    {tx("新建流水线", "New Flow")}
                  </Button>
                )}

                {selectedFlowId && (
                  <>
                    {canEdit && (
                      <Button
                        type="button"
                        variant="outline"
                        data-testid="header-save-flow-btn"
                        disabled={saving}
                        onClick={() => void handleSaveDraft()}
                      >
                        <Save className="h-3.5 w-3.5" />
                        {saving
                          ? tx("保存中…", "Saving…")
                          : tx("保存草稿", "Save Draft")}
                      </Button>
                    )}

                    {canRun && (
                      <div className="flex items-center gap-1 rounded-md border border-border bg-muted/25 p-0.5">
                        <Select
                          data-testid="axflow-run-env-select"
                          value={runEnvironment}
                          onChange={(e) =>
                            setRunEnvironment(e.target.value as Environment)
                          }
                          className="h-7 w-24 px-2 text-[11px] border-0 bg-transparent"
                        >
                          <option value="TEST">{tEnum("TEST")}</option>
                          <option value="PROD">{tEnum("PROD")}</option>
                        </Select>
                        <Button
                          type="button"
                          size="sm"
                          className="h-7 px-3"
                          data-testid="axflow-run-btn"
                          disabled={startingRun}
                          onClick={() => void handleStartRun()}
                        >
                          <Play className="h-3.5 w-3.5" />
                          {startingRun
                            ? tx("启动中…", "Starting…")
                            : tx("运行流水线", "Run Flow")}
                        </Button>
                      </div>
                    )}
                  </>
                )}
              </>
            )}
          </>
        }
      />

      {flowMode === "alerts" ? (
        <div className="flex-1 min-h-0 overflow-y-auto pr-1">
          <AxAlertsPage
            embedded
            initialTab={tabParam === "rules" ? "rules" : "alerts"}
          />
        </div>
      ) : (
        <>

      {/* Compact Single-Row Flow Metadata + Trigger Popover Strip */}
      {selectedFlowId && (
        <div className="shrink-0 relative rounded-md border border-border bg-card px-3 py-1.5 flex flex-wrap items-center justify-between gap-2 text-xs">
          <div className="flex flex-wrap items-center gap-2 flex-1 min-w-60">
            <GitBranch className="h-3.5 w-3.5 text-xblue shrink-0" />
            <Input
              data-testid="flow-draft-name-input"
              value={draftName}
              disabled={!canEdit}
              onChange={(e) => {
                setDraftName(e.target.value);
                setIsDirty(true);
              }}
              className="h-7 w-48 text-xs font-semibold"
            />
            <Input
              data-testid="flow-draft-desc-input"
              value={draftDesc}
              disabled={!canEdit}
              onChange={(e) => {
                setDraftDesc(e.target.value);
                setIsDirty(true);
              }}
              placeholder={tx("流程说明", "Description")}
              className="h-7 flex-1 min-w-44 text-xs text-muted-foreground"
            />
          </div>

          <div className="flex items-center gap-1.5">
            {isDirty && (
              <Badge variant="xblue">{tx("已修改", "Modified")}</Badge>
            )}

            {/* Trigger Configuration Popover Button (Replaces 80%-empty right inspector tab) */}
            <Button
              type="button"
              variant={triggerPopoverOpen ? "primary" : "outline"}
              size="sm"
              data-testid="inspector-tab-trigger"
              onClick={() => setTriggerPopoverOpen((v) => !v)}
            >
              <Webhook className="h-3.5 w-3.5" />
              <span>
                {tx("触发器:", "Trigger:")} {tEnum(draftTrigger.type)}
              </span>
            </Button>

            {canEdit &&
              (confirmDeleteFlow ? (
                <div className="flex items-center gap-1">
                  <Button
                    type="button"
                    variant="ghost"
                    size="sm"
                    onClick={() => setConfirmDeleteFlow(false)}
                  >
                    {tx("取消", "Cancel")}
                  </Button>
                  <Button
                    type="button"
                    variant="danger"
                    size="sm"
                    data-testid="delete-flow-btn"
                    onClick={() => void handleDeleteCurrentFlow()}
                  >
                    {tx("删除", "Delete")}
                  </Button>
                </div>
              ) : (
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  className="h-7 px-2 text-muted-foreground hover:text-red-600"
                  data-testid="delete-flow-btn"
                  onClick={() => setConfirmDeleteFlow(true)}
                  title={tx("删除流水线", "Delete pipeline")}
                >
                  <Trash2 className="h-3.5 w-3.5" />
                </Button>
              ))}
          </div>

          {/* Anchored Trigger Configuration Popover (Positioned left of Right Inspector so tabs stay clickable) */}
          {triggerPopoverOpen && (
            <div className="absolute right-2 xl:right-[380px] top-full mt-1.5 z-30 w-96 rounded-md border border-border bg-card p-3.5 shadow-xl space-y-2.5">
              <div className="flex items-center justify-between border-b border-border pb-2">
                <span className="font-semibold text-xs flex items-center gap-1.5">
                  <Webhook className="h-3.5 w-3.5 text-xblue" />
                  {tx("流水线触发设置", "Pipeline Trigger")}
                </span>
                <button
                  type="button"
                  onClick={() => setTriggerPopoverOpen(false)}
                  className="rounded p-1 text-muted-foreground hover:text-foreground hover:bg-muted cursor-pointer"
                  aria-label={tx("关闭触发器设置", "Close trigger settings")}
                >
                  <X className="h-3.5 w-3.5" />
                </button>
              </div>
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
            </div>
          )}
        </div>
      )}

      {/* Error & Notice Alerts */}
      {error && (
        <Card
          data-testid="axflow-error-banner"
          className="shrink-0 px-3 py-2 border-red-500/40 bg-red-500/5 text-xs text-red-700 dark:text-red-300 flex items-center justify-between"
        >
          <span>
            <strong className="font-mono-tech">
              {tx("错误：", "Error: ")}
            </strong>
            {error}
          </span>
          <button
            type="button"
            onClick={() => setError(null)}
            className="text-muted-foreground hover:text-foreground cursor-pointer"
          >
            {tx("关闭", "Close")}
          </button>
        </Card>
      )}

      {notice && (
        <Card
          data-testid="axflow-notice-banner"
          className="shrink-0 px-3 py-1.5 border-xblue/30 bg-xblue-soft/60 text-xs text-foreground flex items-center justify-between"
        >
          <span>{notice}</span>
          <button
            type="button"
            onClick={() => setNotice(null)}
            className="text-muted-foreground hover:text-foreground cursor-pointer"
          >
            {tx("关闭", "Close")}
          </button>
        </Card>
      )}

      {/* Resizable 100vh 3-Column IDE Workbench */}
      <div className="flex flex-1 min-h-0 overflow-hidden">
        <ResizableWorkspace
          storageKey="axpoint.layout.flow"
          panes={[
            {
              id: "palette",
              defaultSize: 18,
              minSize: 12,
              maxSize: 32,
              collapsible: true,
              handleTestId: "resizable-handle-left",
              className: "pr-1",
              children: (
                <OperatorPalette
                  operators={operators}
                  onAddOperator={(spec) => addOperatorNode(spec)}
                />
              ),
            },
            {
              id: "canvas",
              defaultSize: 54,
              minSize: 30,
              handleTestId: "resizable-handle-right",
              className: "px-0.5",
              children: (
                <FlowCanvas
                  nodes={draftNodes}
                  edges={draftEdges}
                  issues={draftIssues}
                  stepStatusByNodeId={stepStatusByNodeId}
                  selectedNodeId={selectedNodeId}
                  selectedEdgeIndex={selectedEdgeIndex}
                  onSelectNode={(id) => {
                    setSelectedNodeId(id);
                    if (id) {
                      setTriggerPopoverOpen(false);
                      setInspectorTab("node");
                    }
                  }}
                  onSelectEdge={(idx) => {
                    setSelectedEdgeIndex(idx);
                    if (idx !== null) {
                      setTriggerPopoverOpen(false);
                      setInspectorTab("node");
                    }
                  }}
                  onUpdateNodePos={handleUpdateNodePos}
                  onApplyLayout={handleApplyLayout}
                  onConnectNodes={handleConnectNodes}
                  onUpdateEdge={handleUpdateEdge}
                  onDeleteEdge={handleDeleteEdge}
                  onDropOperator={handleDropOperator}
                />
              ),
            },
            {
              id: "inspector",
              defaultSize: 28,
              minSize: 18,
              maxSize: 45,
              collapsible: true,
              className: "pl-1",
              children: (
                <Card className="w-full h-full min-h-0 p-3 flex flex-col gap-2.5 overflow-hidden">
                  <Tabs<InspectorTab>
                    className="shrink-0"
                    value={inspectorTab}
                    onChange={(tab) => {
                      setTriggerPopoverOpen(false);
                      setInspectorTab(tab);
                    }}
                    items={[
                      {
                        key: "node",
                        label: tx("节点配置", "Step Config"),
                        icon: <Settings2 className="h-3.5 w-3.5" />,
                        testId: "inspector-tab-node",
                      },
                      {
                        key: "version",
                        label: tx("版本部署", "Release"),
                        icon: <Rocket className="h-3.5 w-3.5" />,
                        testId: "inspector-tab-version",
                      },
                    ]}
                  />

                  <div className="flex-1 min-h-0 overflow-y-auto pr-0.5">
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

                    {inspectorTab === "version" && (
                      <VersionDeployPanel
                        isDirty={isDirty}
                        validating={validating}
                        publishing={publishing}
                        deployingEnv={deployingEnv}
                        issues={draftIssues}
                        versions={versions}
                        deployments={deployments}
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
                </Card>
              ),
            },
          ]}
        />
      </div>

      {/* Persistent Sticky Bottom IDE Run Monitor (2 Wide Columns: RunHistory + RunTimeline) */}
      <Card className="shrink-0 overflow-hidden" data-testid="axflow-run-dock">
        <div className="flex items-center justify-between gap-2 bg-muted/25 px-3 py-1">
          <button
            type="button"
            onClick={() => setRunDockCollapsed((v) => !v)}
            className="flex items-center gap-2 text-xs font-medium cursor-pointer"
          >
            <Activity className="h-3.5 w-3.5 text-xblue" />
            <span>{tx("运行监控与步骤时间线", "Run Monitor & Timeline")}</span>
            <span className="font-mono-tech text-[11px] text-muted-foreground tabular-nums">
              ({runs.length})
            </span>
            {activeRunDetail && (
              <span className="inline-flex items-center gap-1.5 pl-1">
                {renderRunStatusBadge(activeRunDetail.status)}
                <span className="font-mono-tech text-[11px] text-muted-foreground">
                  #{activeRunDetail.id.slice(0, 8)}
                </span>
              </span>
            )}
          </button>

          <Button
            type="button"
            variant="ghost"
            size="sm"
            className="h-6 px-2 text-[11px]"
            onClick={() => setRunDockCollapsed((v) => !v)}
          >
            {runDockCollapsed ? (
              <>
                <ChevronUp className="h-3 w-3" />
                {tx("展开", "Expand")}
              </>
            ) : (
              <>
                <ChevronDown className="h-3 w-3" />
                {tx("收起", "Collapse")}
              </>
            )}
          </Button>
        </div>

        {!runDockCollapsed && (
          <div className="border-t border-border p-2.5 h-64 grid grid-cols-1 xl:grid-cols-12 gap-3 overflow-hidden">
            <div className="xl:col-span-4 h-full min-h-0">
              <RunHistory
                runs={runs}
                selectedRunId={selectedRunId}
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

            <div className="xl:col-span-8 h-full min-h-0">
              <RunTimeline
                run={activeRunDetail}
                steps={runSteps}
                actions={runActions}
                nodes={draftNodes}
                polling={isRunActive}
              />
            </div>
          </div>
        )}
      </Card>

      {/* Global Pending Approvals Drawer */}
      <Drawer
        open={approvalDrawerOpen}
        onClose={() => setApprovalDrawerOpen(false)}
        widthClass="max-w-lg"
        title={
          <>
            <UserCheck className="h-4 w-4 text-xblue shrink-0" />
            <span>{tx("人工审批待办", "Approval Inbox")}</span>
            <Badge variant={pendingApprovalsCount > 0 ? "warning" : "neutral"}>
              {approvals.length}
            </Badge>
          </>
        }
      >
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
      </Drawer>

      {/* Create Pipeline Modal */}
      <Modal
        open={createOpen}
        onClose={() => setCreateOpen(false)}
        testId="axflow-create-card"
        maxWidthClass="max-w-md"
        title={tx("新建编排流水线", "New Pipeline")}
        footer={
          <>
            <Button
              type="button"
              variant="outline"
              onClick={() => setCreateOpen(false)}
            >
              {tx("取消", "Cancel")}
            </Button>
            <Button
              type="button"
              data-testid="confirm-create-flow-btn"
              disabled={!newFlowName.trim() || creating}
              onClick={() => void handleCreateFlow()}
            >
              <Plus className="h-3.5 w-3.5" />
              {creating ? tx("创建中…", "Creating…") : tx("创建", "Create")}
            </Button>
          </>
        }
      >
        <div className="space-y-2.5 text-xs">
          <div>
            <label className="block font-medium mb-1">
              {tx("流水线标识", "Pipeline Key")}
            </label>
            <Input
              data-testid="new-flow-name-input"
              placeholder="daily_supply_refresh"
              value={newFlowName}
              onChange={(e) => setNewFlowName(e.target.value)}
              className="h-8 text-xs font-mono-tech"
            />
          </div>
          <div>
            <label className="block font-medium mb-1">
              {tx("业务说明", "Description")}
            </label>
            <Input
              data-testid="new-flow-desc-input"
              placeholder={tx("每日供应链补货巡检", "Daily supply inspection")}
              value={newFlowDesc}
              onChange={(e) => setNewFlowDesc(e.target.value)}
              className="h-8 text-xs"
            />
          </div>
          <div>
            <label className="block font-medium mb-1">
              {tx("回调标识", "Webhook Slug")}
            </label>
            <Input
              data-testid="new-flow-slug-input"
              placeholder="daily-supply-hook"
              value={newFlowSlug}
              onChange={(e) => setNewFlowSlug(e.target.value)}
              className="h-8 text-xs font-mono-tech"
            />
          </div>
        </div>
      </Modal>
        </>
      )}
    </div>
  );
}
