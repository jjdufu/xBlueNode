import { useEffect, useMemo, useState } from "react";
import {
  AlertTriangle,
  DatabaseBackup,
  GitBranch,
  Loader2,
  Plus,
  RefreshCw,
  RotateCcw,
  Search,
  Trash2,
  X,
} from "lucide-react";
import {
  axsimApi,
  type AxSandboxRecord,
  type GraphDirection,
  type RefreshBaseResponse,
  type SandboxScope,
  type ScopePreflightReport,
  type ScopeReadinessGrade,
} from "@/lib/api/axsim";
import {
  xblueApi,
  type BlueNode,
  type SubgraphInfo,
} from "@/lib/api/xblue";
import {
  Badge,
  Button,
  Card,
  ConfirmDialog,
  IconButton,
  Input,
  Select,
  StatusBadge,
} from "@/components/ui/primitives";
import { useI18n } from "@/i18n";
import { usePrincipal } from "@/lib/permissions";

export interface SandboxWizardProps {
  open: boolean;
  activeSandbox: AxSandboxRecord | null;
  onClose: () => void;
  onCreated: (sandbox: AxSandboxRecord) => void;
  onUpdated: (sandbox: AxSandboxRecord) => void;
  onDeleted: (sandboxId: string) => void;
  onOverlayChanged: () => void;
}

export function SandboxWizard({
  open,
  activeSandbox,
  onClose,
  onCreated,
  onUpdated,
  onDeleted,
  onOverlayChanged,
}: SandboxWizardProps) {
  const { tx, fmtTime, fmtNum } = useI18n();
  const { can } = usePrincipal();
  const canSimRun = can("SIM_RUN");

  const [name, setName] = useState("");
  const [subgraphs, setSubgraphs] = useState<SubgraphInfo[]>([]);
  const [selectedSubgraph, setSelectedSubgraph] = useState("main");
  const [availableTypes, setAvailableTypes] = useState<string[]>([]);
  const [availableLinkTypes, setAvailableLinkTypes] = useState<string[]>([]);

  const [selectedTypes, setSelectedTypes] = useState<string[]>([]);
  const [selectedLinkTypes, setSelectedLinkTypes] = useState<string[]>([]);
  const [customTypeInput, setCustomTypeInput] = useState("");
  const [maxDepth, setMaxDepth] = useState("3");
  const [direction, setDirection] = useState<GraphDirection>("BOTH");
  const [maxNodes, setMaxNodes] = useState("50000");

  const [seedQuery, setSeedQuery] = useState("");
  const [seedCandidates, setSeedCandidates] = useState<BlueNode[]>([]);
  const [selectedSeeds, setSelectedSeeds] = useState<BlueNode[]>([]);
  const [manualSeedId, setManualSeedId] = useState("");
  const [searchingSeeds, setSearchingSeeds] = useState(false);

  const [estimatedNodes, setEstimatedNodes] = useState<number | null>(null);
  const [preflight, setPreflight] = useState<ScopePreflightReport | null>(null);
  const [preflightLoading, setPreflightLoading] = useState(false);
  const [preflightNonce, setPreflightNonce] = useState(0);
  const [creating, setCreating] = useState(false);
  const [actionBusy, setActionBusy] = useState<string | null>(null);
  const [confirmResetOpen, setConfirmResetOpen] = useState(false);
  const [confirmDeleteOpen, setConfirmDeleteOpen] = useState(false);
  const [refreshReport, setRefreshReport] = useState<RefreshBaseResponse | null>(
    null
  );
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  const directionOptions: ReadonlyArray<{
    value: GraphDirection;
    label: string;
  }> = [
    { value: "BOTH", label: tx("双向", "Bidirectional") },
    { value: "OUT", label: tx("下游", "Downstream") },
    { value: "IN", label: tx("上游", "Upstream") },
  ];

  // Poll when activeSandbox is in CLONING state
  useEffect(() => {
    if (!activeSandbox || activeSandbox.status !== "CLONING") {
      return;
    }
    let cancelled = false;
    const timer = window.setInterval(() => {
      void (async () => {
        try {
          const latest = await axsimApi.getSandbox(activeSandbox.id);
          if (!cancelled && latest && latest.id) {
            onUpdated(latest);
          }
        } catch {
          // keep polling silently on transient failure
        }
      })();
    }, 1500);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [activeSandbox, onUpdated]);

  useEffect(() => {
    setConfirmDeleteOpen(false);
    setConfirmResetOpen(false);
    setRefreshReport(null);
  }, [activeSandbox?.id]);

  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    void (async () => {
      try {
        const sgs = await xblueApi.listSubgraphs().catch(() => []);
        if (!cancelled && Array.isArray(sgs)) {
          setSubgraphs(sgs);
        }
      } catch {
        // ignore
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [open]);

  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    void (async () => {
      try {
        const schema = await xblueApi
          .getPublishedSchema(selectedSubgraph)
          .catch(() => null);
        if (!cancelled && schema) {
          const ntNames = Array.isArray(schema.node_types)
            ? schema.node_types.map((n) => n.name)
            : [];
          const etNames = Array.isArray(schema.edge_types)
            ? schema.edge_types.map((e) => e.name)
            : [];
          setAvailableTypes(ntNames);
          setAvailableLinkTypes(etNames);
        } else {
          const fallbackSchema = await xblueApi.getSchema().catch(() => null);
          if (!cancelled && fallbackSchema) {
            setAvailableTypes(
              Array.isArray(fallbackSchema.node_types)
                ? fallbackSchema.node_types.map((n) => n.name)
                : []
            );
            setAvailableLinkTypes(
              Array.isArray(fallbackSchema.edge_types)
                ? fallbackSchema.edge_types.map((e) => e.name)
                : []
            );
          }
        }
      } catch {
        // ignore
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [open, selectedSubgraph]);

  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    void (async () => {
      try {
        const res = await xblueApi
          .querySubgraph({
            type_names:
              selectedTypes.length > 0 ? selectedTypes.join(",") : undefined,
            link_types:
              selectedLinkTypes.length > 0
                ? selectedLinkTypes.join(",")
                : undefined,
            limit: 500,
          })
          .catch(() => null);
        if (!cancelled && res && Array.isArray(res.nodes)) {
          setEstimatedNodes(res.nodes.length);
        }
      } catch {
        // ignore
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [open, selectedLinkTypes, selectedTypes]);

  const seedIdsKey = useMemo(
    () => selectedSeeds.map((s) => s.id).join(","),
    [selectedSeeds]
  );

  useEffect(() => {
    if (!open || !canSimRun) {
      setPreflight(null);
      setPreflightLoading(false);
      return;
    }
    let cancelled = false;
    setPreflightLoading(true);
    const timer = window.setTimeout(() => {
      void (async () => {
        try {
          const parsedDepth = Number.parseInt(maxDepth, 10);
          const parsedMaxNodes = Number.parseInt(maxNodes, 10);
          const rootIds = seedIdsKey
            ? seedIdsKey.split(",").filter(Boolean)
            : [];
          const candidateScope: SandboxScope = {
            root_node_ids: rootIds,
            depth: Number.isFinite(parsedDepth)
              ? Math.min(6, Math.max(1, parsedDepth))
              : 3,
            direction,
            type_names: selectedTypes,
            link_types: selectedLinkTypes,
            max_nodes: Number.isFinite(parsedMaxNodes)
              ? Math.max(1, parsedMaxNodes)
              : 50000,
          };
          const report = await axsimApi
            .preflightSandbox({
              subgraph_slug: selectedSubgraph || "main",
              scope: candidateScope,
            })
            .catch(() => null);
          if (!cancelled) {
            setPreflight(report);
            if (report) {
              setEstimatedNodes(report.estimated_nodes);
            }
          }
        } finally {
          if (!cancelled) {
            setPreflightLoading(false);
          }
        }
      })();
    }, 350);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [
    canSimRun,
    direction,
    maxDepth,
    maxNodes,
    open,
    preflightNonce,
    seedIdsKey,
    selectedLinkTypes,
    selectedSubgraph,
    selectedTypes,
  ]);

  const handleFillMissingTypes = () => {
    if (!preflight || preflight.missing_upstream_types.length === 0) return;
    const missing = preflight.missing_upstream_types;
    setSelectedTypes((prev) => Array.from(new Set([...prev, ...missing])));
    setSelectedLinkTypes([]);
    setPreflightNonce((n) => n + 1);
  };

  const renderPreflightGradeBadge = (grade: ScopeReadinessGrade) => {
    if (grade === "READY") {
      return (
        <Badge variant="success" data-testid="sim-preflight-grade-badge">
          {tx("就绪", "READY")}
        </Badge>
      );
    }
    if (grade === "DEGRADED") {
      return (
        <Badge variant="warning" data-testid="sim-preflight-grade-badge">
          {tx("有损", "DEGRADED")}
        </Badge>
      );
    }
    return (
      <Badge variant="danger" data-testid="sim-preflight-grade-badge">
        {tx("阻断", "BLOCKED")}
      </Badge>
    );
  };

  const searchSeedNodes = async () => {
    setSearchingSeeds(true);
    try {
      if (seedQuery.trim().length > 0) {
        const res = await xblueApi
          .searchNodes({ q: seedQuery.trim(), k: 10 })
          .catch(() => ({ items: [] }));
        if (Array.isArray(res?.items) && res.items.length > 0) {
          setSeedCandidates(res.items);
          return;
        }
      }
      const list = await xblueApi
        .listNodes({ q: seedQuery.trim() || undefined, limit: 12 })
        .catch(() => ({ items: [], nodes: [], links: [], next_cursor: null }));
      const items =
        Array.isArray(list.items) && list.items.length > 0
          ? list.items
          : Array.isArray(list.nodes)
            ? list.nodes
            : [];
      setSeedCandidates(items);
    } finally {
      setSearchingSeeds(false);
    }
  };

  const toggleType = (typeName: string) => {
    setSelectedTypes((prev) =>
      prev.includes(typeName)
        ? prev.filter((t) => t !== typeName)
        : [...prev, typeName]
    );
  };

  const toggleLinkType = (linkType: string) => {
    setSelectedLinkTypes((prev) =>
      prev.includes(linkType)
        ? prev.filter((t) => t !== linkType)
        : [...prev, linkType]
    );
  };

  const addCustomType = () => {
    const trimmed = customTypeInput.trim();
    if (!trimmed) return;
    if (!selectedTypes.includes(trimmed)) {
      setSelectedTypes((prev) => [...prev, trimmed]);
    }
    setCustomTypeInput("");
  };

  const toggleSeed = (node: BlueNode) => {
    setSelectedSeeds((prev) =>
      prev.some((n) => n.id === node.id)
        ? prev.filter((n) => n.id !== node.id)
        : [...prev, node]
    );
  };

  const addManualSeedId = () => {
    const trimmed = manualSeedId.trim();
    if (!trimmed) return;
    if (!selectedSeeds.some((s) => s.id === trimmed)) {
      setSelectedSeeds((prev) => [
        ...prev,
        {
          id: trimmed,
          type_name: "Node",
          node_key: trimmed.slice(0, 8),
          title: trimmed,
          metrics: {},
        },
      ]);
    }
    setManualSeedId("");
  };

  const scopePreview = useMemo(() => {
    const parts: string[] = [];
    if (selectedSeeds.length > 0) {
      parts.push(
        tx(`${selectedSeeds.length} 个种子节点`, `${selectedSeeds.length} seeds`)
      );
    } else {
      parts.push(tx("全图范围", "Full graph"));
    }
    parts.push(
      tx(`深度 ${maxDepth || "3"}`, `Depth ${maxDepth || "3"}`) +
        ` (${direction})`
    );
    if (selectedTypes.length > 0) {
      parts.push(`${selectedTypes.join(", ")}`);
    }
    return parts.join(" · ");
  }, [direction, maxDepth, selectedSeeds.length, selectedTypes, tx]);

  const handleCreate = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!canSimRun || preflight?.readiness_grade === "BLOCKED") return;
    const trimmedName = name.trim();
    if (!trimmedName) {
      setErrorMsg(tx("请填写沙盘名称", "Sandbox name is required"));
      return;
    }
    setCreating(true);
    setErrorMsg(null);
    try {
      const parsedDepth = Number.parseInt(maxDepth, 10);
      const parsedMaxNodes = Number.parseInt(maxNodes, 10);
      const hasScope =
        selectedSeeds.length > 0 ||
        selectedTypes.length > 0 ||
        selectedLinkTypes.length > 0 ||
        parsedDepth !== 3 ||
        direction !== "BOTH" ||
        parsedMaxNodes !== 50000;

      const scope: SandboxScope | undefined = hasScope
        ? {
            ...(selectedSeeds.length > 0
              ? { root_node_ids: selectedSeeds.map((s) => s.id) }
              : {}),
            depth: Number.isFinite(parsedDepth)
              ? Math.min(6, Math.max(1, parsedDepth))
              : 3,
            direction,
            ...(selectedTypes.length > 0 ? { type_names: selectedTypes } : {}),
            ...(selectedLinkTypes.length > 0
              ? { link_types: selectedLinkTypes }
              : {}),
            max_nodes: Number.isFinite(parsedMaxNodes)
              ? Math.max(1, parsedMaxNodes)
              : 50000,
          }
        : undefined;

      const created = await axsimApi.createSandbox({
        name: trimmedName,
        ...(scope ? { scope } : {}),
      });
      setName("");
      setSelectedSeeds([]);
      setSelectedTypes([]);
      setSelectedLinkTypes([]);
      onCreated(created);
      onClose();
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setCreating(false);
    }
  };

  const handleRefreshBase = async () => {
    if (!activeSandbox || !canSimRun) return;
    setActionBusy("refresh");
    setErrorMsg(null);
    try {
      const res = await axsimApi.refreshBase(activeSandbox.id);
      setRefreshReport(res);
      if (res.sandbox) {
        onUpdated(res.sandbox);
      }
      onOverlayChanged();
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setActionBusy(null);
    }
  };

  const handleResetOverlay = async () => {
    if (!activeSandbox || !canSimRun) return;
    setActionBusy("reset");
    setErrorMsg(null);
    try {
      await axsimApi.resetOverlay(activeSandbox.id);
      setConfirmResetOpen(false);
      onOverlayChanged();
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setActionBusy(null);
    }
  };

  const handleDeleteSandbox = async () => {
    if (!activeSandbox || !canSimRun) return;
    setActionBusy("delete");
    setErrorMsg(null);
    try {
      await axsimApi.deleteSandbox(activeSandbox.id);
      setConfirmDeleteOpen(false);
      onDeleted(activeSandbox.id);
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setActionBusy(null);
    }
  };

  return (
    <div className="space-y-3" data-testid="sandbox-wizard-container">
      {activeSandbox?.status === "CLONING" && (
        <div
          data-testid="sandbox-cloning-banner"
          className="flex items-center justify-between gap-3 rounded-md border border-xblue/40 bg-xblue-soft/60 px-3.5 py-2.5 text-xs"
        >
          <div className="flex items-center gap-2 text-xblue font-medium">
            <Loader2 className="h-4 w-4 animate-spin" />
            <span>
              {tx(
                `沙盘「${activeSandbox.name}」克隆中…`,
                `Cloning sandbox "${activeSandbox.name}"…`
              )}
            </span>
          </div>
          <StatusBadge status="POLL" />
        </div>
      )}

      {activeSandbox && (
        <div className="flex flex-wrap items-center justify-between gap-2 rounded-md border border-border bg-muted/30 px-3 py-2 text-xs">
          <div className="flex flex-wrap items-center gap-2 text-muted-foreground">
            <span className="font-mono-tech">
              {tx("基线捕获: ", "Baseline: ")}
              {activeSandbox.base_captured_at
                ? fmtTime(activeSandbox.base_captured_at)
                : tx("实时快照", "Live snapshot")}
            </span>
            {activeSandbox.schema_version != null && (
              <Badge variant="outline">
                Schema v{activeSandbox.schema_version}
              </Badge>
            )}
          </div>

          {canSimRun && (
            <div className="flex flex-wrap items-center gap-2">
              <Button
                type="button"
                variant="outline"
                size="sm"
                className="h-7 px-2.5 text-[11px]"
                onClick={() => void handleRefreshBase()}
                disabled={actionBusy !== null}
                data-testid="sandbox-refresh-base-btn"
              >
                <RefreshCw
                  className={`h-3 w-3 ${
                    actionBusy === "refresh" ? "animate-spin" : ""
                  }`}
                />
                {tx("刷新基线", "Sync Base")}
              </Button>
              <Button
                type="button"
                variant="outline"
                size="sm"
                className="h-7 px-2.5 text-[11px]"
                onClick={() => setConfirmResetOpen(true)}
                disabled={actionBusy !== null}
                data-testid="sandbox-reset-overlay-btn"
              >
                <RotateCcw className="h-3 w-3" />
                {tx("清空覆盖层", "Reset Overlay")}
              </Button>
              <Button
                type="button"
                variant="outline"
                size="sm"
                className="h-7 px-2.5 text-[11px]"
                onClick={() => setConfirmDeleteOpen(true)}
                disabled={actionBusy !== null}
                data-testid="sandbox-delete-btn"
              >
                <Trash2 className="h-3 w-3" />
                {tx("删除沙盘", "Delete")}
              </Button>
            </div>
          )}
        </div>
      )}

      {refreshReport &&
        (refreshReport.orphaned_node_ids.length > 0 ||
          refreshReport.orphaned_link_ids.length > 0) && (
          <div
            data-testid="sandbox-orphan-warning"
            className="rounded-md border border-amber-500/40 bg-amber-500/10 px-3 py-2 text-xs space-y-1"
          >
            <div className="flex items-center gap-1.5 font-medium text-amber-700 dark:text-amber-300">
              <AlertTriangle className="h-3.5 w-3.5" />
              {tx(
                "基线已刷新，以下覆盖层条目在最新生产快照中已不存在：",
                "Baseline synced; the following overlay items no longer exist in production:"
              )}
            </div>
            <div className="font-mono-tech text-[11px] text-muted-foreground">
              {tx("孤立节点: ", "Orphaned nodes: ")}
              {refreshReport.orphaned_node_ids.join(", ") || tx("无", "None")} ·{" "}
              {tx("孤立关系: ", "Orphaned links: ")}
              {refreshReport.orphaned_link_ids.join(", ") || tx("无", "None")}
            </div>
          </div>
        )}

      {errorMsg && (
        <div className="rounded-md border border-red-500/40 bg-red-500/10 px-3 py-2 text-xs text-red-700 dark:text-red-300">
          {errorMsg}
        </div>
      )}

      {open && canSimRun && (
        <Card
          className="p-4 space-y-4 border-xblue/40"
          data-testid="sandbox-wizard-panel"
        >
          <div className="flex items-center justify-between border-b border-border pb-2.5">
            <div className="flex items-center gap-2 text-sm font-medium">
              <GitBranch className="h-4 w-4 text-xblue" />
              <span>{tx("新建推演沙盘", "New Simulation Sandbox")}</span>
            </div>
            <IconButton
              size="sm"
              onClick={onClose}
              aria-label={tx("关闭", "Close")}
            >
              <X className="h-3.5 w-3.5" />
            </IconButton>
          </div>

          <form onSubmit={(e) => void handleCreate(e)} className="space-y-4">
            <div className="grid grid-cols-1 md:grid-cols-3 gap-3">
              <div className="md:col-span-2 space-y-1">
                <label className="text-xs font-medium text-muted-foreground">
                  {tx("沙盘名称", "Sandbox Name")}{" "}
                  <span className="text-red-500">*</span>
                </label>
                <Input
                  value={name}
                  onChange={(e) => setName(e.target.value)}
                  placeholder={tx(
                    "华东断供 3 天应急推演",
                    "East China 3-day outage simulation"
                  )}
                  data-testid="wizard-sandbox-name"
                />
              </div>

              <div className="space-y-1">
                <label className="text-xs font-medium text-muted-foreground">
                  {tx("参考子图", "Subgraph")}
                </label>
                <Select
                  value={selectedSubgraph}
                  onChange={(e) => setSelectedSubgraph(e.target.value)}
                  className="h-9 w-full"
                  data-testid="wizard-subgraph-select"
                >
                  <option value="main">
                    {tx("main · 企业主图", "main · Enterprise Graph")}
                  </option>
                  {subgraphs
                    .filter((sg) => sg.slug !== "main")
                    .map((sg) => (
                      <option key={sg.id} value={sg.slug}>
                        {sg.slug} · {sg.display_name}
                      </option>
                    ))}
                </Select>
              </div>
            </div>

            {/* Scope params */}
            <div className="grid grid-cols-1 md:grid-cols-3 gap-3">
              <div className="space-y-1">
                <label className="text-xs font-medium text-muted-foreground">
                  {tx("最大扩展深度", "Max Depth")}
                </label>
                <Input
                  type="number"
                  min={1}
                  max={6}
                  value={maxDepth}
                  onChange={(e) => setMaxDepth(e.target.value)}
                  data-testid="wizard-max-depth"
                />
              </div>
              <div className="space-y-1">
                <label className="text-xs font-medium text-muted-foreground">
                  {tx("遍历方向", "Traversal Direction")}
                </label>
                <Select
                  value={direction}
                  onChange={(e) =>
                    setDirection(e.target.value as GraphDirection)
                  }
                  className="h-9 w-full"
                  data-testid="wizard-direction"
                >
                  {directionOptions.map((opt) => (
                    <option key={opt.value} value={opt.value}>
                      {opt.label}
                    </option>
                  ))}
                </Select>
              </div>
              <div className="space-y-1">
                <label className="text-xs font-medium text-muted-foreground">
                  {tx("最大节点上限", "Node Limit")}
                </label>
                <Input
                  type="number"
                  min={1}
                  max={50000}
                  value={maxNodes}
                  onChange={(e) => setMaxNodes(e.target.value)}
                  data-testid="wizard-max-nodes"
                />
              </div>
            </div>

            {/* Type names filter */}
            <div className="space-y-2">
              <div className="flex items-center justify-between">
                <label className="text-xs font-medium text-muted-foreground">
                  {tx("节点类型过滤", "Node Type Filter")}
                </label>
                <div className="flex items-center gap-1.5">
                  <Input
                    value={customTypeInput}
                    onChange={(e) => setCustomTypeInput(e.target.value)}
                    placeholder="Warehouse"
                    className="h-7 w-32 text-xs"
                  />
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    className="h-7 px-2 text-[11px]"
                    onClick={addCustomType}
                  >
                    <Plus className="h-3 w-3" />
                  </Button>
                </div>
              </div>
              <div className="flex flex-wrap gap-1.5">
                {Array.from(new Set([...availableTypes, ...selectedTypes])).map(
                  (t) => {
                    const active = selectedTypes.includes(t);
                    return (
                      <button
                        key={t}
                        type="button"
                        onClick={() => toggleType(t)}
                        className={`rounded px-2 py-1 text-[11px] font-mono-tech border transition-colors cursor-pointer ${
                          active
                            ? "border-xblue bg-xblue-soft text-xblue font-semibold"
                            : "border-border bg-background text-muted-foreground hover:text-foreground"
                        }`}
                      >
                        {t}
                      </button>
                    );
                  }
                )}
                {availableTypes.length === 0 && selectedTypes.length === 0 && (
                  <span className="text-[11px] text-muted-foreground">
                    {tx("默认全量克隆", "Clones all node types by default")}
                  </span>
                )}
              </div>
            </div>

            {availableLinkTypes.length > 0 && (
              <div className="space-y-1.5">
                <label className="text-xs font-medium text-muted-foreground">
                  {tx("关系类型过滤", "Link Type Filter")}
                </label>
                <div className="flex flex-wrap gap-1.5">
                  {availableLinkTypes.map((lt) => {
                    const active = selectedLinkTypes.includes(lt);
                    return (
                      <button
                        key={lt}
                        type="button"
                        onClick={() => toggleLinkType(lt)}
                        className={`rounded px-2 py-1 text-[11px] font-mono-tech border transition-colors cursor-pointer ${
                          active
                            ? "border-xblue bg-xblue-soft text-xblue font-semibold"
                            : "border-border bg-background text-muted-foreground hover:text-foreground"
                        }`}
                      >
                        {lt}
                      </button>
                    );
                  })}
                </div>
              </div>
            )}

            {/* Seed node search */}
            <div className="space-y-2 rounded-md border border-border p-3 bg-muted/20">
              <div className="text-xs font-medium flex items-center justify-between">
                <span>{tx("种子节点检索", "Seed Nodes")}</span>
                {estimatedNodes !== null && (
                  <Badge variant="outline">
                    {tx(
                      `预计节点: ~${fmtNum(estimatedNodes, 0)}${estimatedNodes >= 500 ? "+" : ""}`,
                      `Est. nodes: ~${fmtNum(estimatedNodes, 0)}${estimatedNodes >= 500 ? "+" : ""}`
                    )}
                  </Badge>
                )}
              </div>

              <div className="flex flex-wrap gap-2">
                <div className="flex-1 min-w-[200px] flex gap-1.5">
                  <Input
                    value={seedQuery}
                    onChange={(e) => setSeedQuery(e.target.value)}
                    placeholder={tx("F-1 / 华东仓", "F-1 / East Warehouse")}
                    className="h-8 text-xs"
                    data-testid="wizard-seed-search"
                  />
                  <Button
                    type="button"
                    variant="outline"
                    className="h-8 px-2.5 text-xs shrink-0"
                    onClick={() => void searchSeedNodes()}
                    disabled={searchingSeeds}
                  >
                    <Search className="h-3.5 w-3.5" />
                    {tx("检索", "Search")}
                  </Button>
                </div>

                <div className="flex gap-1.5">
                  <Input
                    value={manualSeedId}
                    onChange={(e) => setManualSeedId(e.target.value)}
                    placeholder="00000000-0000-0000-0000-000000000001"
                    className="h-8 w-48 text-xs font-mono-tech"
                  />
                  <Button
                    type="button"
                    variant="outline"
                    className="h-8 px-2.5 text-xs"
                    onClick={addManualSeedId}
                  >
                    {tx("添加", "Add")}
                  </Button>
                </div>
              </div>

              {seedCandidates.length > 0 && (
                <div className="flex flex-wrap gap-1.5 pt-1">
                  {seedCandidates.map((cand) => {
                    const picked = selectedSeeds.some((s) => s.id === cand.id);
                    return (
                      <button
                        key={cand.id}
                        type="button"
                        onClick={() => toggleSeed(cand)}
                        className={`rounded border px-2 py-1 text-[11px] cursor-pointer ${
                          picked
                            ? "border-xblue bg-xblue-soft text-xblue"
                            : "border-border bg-background hover:bg-muted"
                        }`}
                      >
                        [{cand.type_name}] {cand.title} ({cand.node_key})
                      </button>
                    );
                  })}
                </div>
              )}

              {selectedSeeds.length > 0 && (
                <div className="flex flex-wrap items-center gap-1.5 pt-1">
                  <span className="text-[11px] text-muted-foreground">
                    {tx("已选种子:", "Selected:")}
                  </span>
                  {selectedSeeds.map((seed) => (
                    <span
                      key={seed.id}
                      className="inline-flex items-center gap-1 rounded bg-xblue-soft text-xblue px-2 py-0.5 text-[11px] font-mono-tech"
                    >
                      {seed.title}
                      <button
                        type="button"
                        onClick={() => toggleSeed(seed)}
                        className="hover:opacity-75 cursor-pointer"
                      >
                        <X className="h-3 w-3" />
                      </button>
                    </span>
                  ))}
                </div>
              )}
            </div>

            {(preflight !== null || preflightLoading) && (
              <div
                data-testid="sim-preflight-card"
                className="rounded-md border border-border bg-muted/25 p-3 space-y-2.5 text-xs"
              >
                <div className="flex items-center justify-between gap-2">
                  <div className="flex items-center gap-2 font-medium text-foreground">
                    <span>{tx("范围预检", "Scope Preflight")}</span>
                    {preflightLoading && (
                      <Loader2 className="h-3.5 w-3.5 animate-spin text-xblue" />
                    )}
                    {preflight && (
                      <span className="font-mono-tech text-[11px] text-muted-foreground tabular-nums">
                        {tx(
                          `关系 ${fmtNum(preflight.estimated_links, 0)}`,
                          `Links ${fmtNum(preflight.estimated_links, 0)}`
                        )}
                      </span>
                    )}
                  </div>
                  {preflight &&
                    renderPreflightGradeBadge(preflight.readiness_grade)}
                </div>

                {preflight && (
                  <>
                    <div className="grid grid-cols-2 sm:grid-cols-4 gap-2">
                      <div className="rounded border border-border/70 bg-background px-2.5 py-1.5">
                        <div className="text-[11px] text-muted-foreground">
                          {tx("预计节点", "Est. Nodes")}
                        </div>
                        <div className="font-mono-tech text-sm font-semibold tabular-nums">
                          {fmtNum(preflight.estimated_nodes, 0)}
                        </div>
                      </div>
                      <div className="rounded border border-border/70 bg-background px-2.5 py-1.5">
                        <div className="text-[11px] text-muted-foreground">
                          {tx("因果覆盖率", "Causal Coverage")}
                        </div>
                        <div className="font-mono-tech text-sm font-semibold tabular-nums">
                          {fmtNum(
                            preflight.causal_edge_coverage_ratio * 100,
                            1
                          )}
                          %
                        </div>
                      </div>
                      <div className="rounded border border-border/70 bg-background px-2.5 py-1.5">
                        <div className="text-[11px] text-muted-foreground">
                          {tx("聚合闭合度", "Agg Closure")}
                        </div>
                        <div className="font-mono-tech text-sm font-semibold tabular-nums">
                          {fmtNum(preflight.agg_rule_closure_ratio * 100, 1)}%
                        </div>
                      </div>
                      <div className="rounded border border-border/70 bg-background px-2.5 py-1.5">
                        <div className="text-[11px] text-muted-foreground">
                          {tx("孤立占比", "Isolated Ratio")}
                        </div>
                        <div className="font-mono-tech text-sm font-semibold tabular-nums">
                          {fmtNum(preflight.isolated_node_ratio * 100, 1)}%
                        </div>
                      </div>
                    </div>

                    {preflight.reason_codes.length > 0 && (
                      <div className="flex flex-wrap items-center gap-1.5">
                        {preflight.reason_codes.map((code) => (
                          <Badge key={code} variant="outline">
                            {code}
                          </Badge>
                        ))}
                      </div>
                    )}

                    {preflight.missing_upstream_types.length > 0 && (
                      <div className="flex flex-wrap items-center justify-between gap-2 rounded border border-amber-500/35 bg-amber-500/10 px-2.5 py-2">
                        <div className="flex flex-wrap items-center gap-1.5 min-w-0">
                          <span className="text-[11px] text-amber-800 dark:text-amber-300 font-medium">
                            {tx("缺失类型", "Missing Types")}
                          </span>
                          {preflight.missing_upstream_types.map((mt) => (
                            <Badge key={mt} variant="warning">
                              {mt}
                            </Badge>
                          ))}
                        </div>
                        <Button
                          type="button"
                          variant="outline"
                          size="sm"
                          onClick={handleFillMissingTypes}
                          data-testid="sim-preflight-fill-types-btn"
                        >
                          <Plus className="h-3 w-3 text-xblue" />
                          {tx("补齐类型", "Fill Types")}
                        </Button>
                      </div>
                    )}
                  </>
                )}
              </div>
            )}

            <div className="flex flex-wrap items-center justify-between gap-2 pt-1">
              <div className="text-[11px] font-mono-tech text-muted-foreground">
                {tx("范围：", "Scope: ")}
                {scopePreview}
              </div>
              <div className="flex items-center gap-2">
                <Button type="button" variant="outline" onClick={onClose}>
                  {tx("取消", "Cancel")}
                </Button>
                <Button
                  type="submit"
                  disabled={
                    creating ||
                    !name.trim() ||
                    preflight?.readiness_grade === "BLOCKED"
                  }
                  data-testid="wizard-create-submit"
                >
                  <DatabaseBackup className="h-3.5 w-3.5" />
                  {creating
                    ? tx("创建中…", "Creating…")
                    : tx("创建沙盘", "Create Sandbox")}
                </Button>
              </div>
            </div>
          </form>
        </Card>
      )}

      <ConfirmDialog
        open={confirmResetOpen}
        onClose={() => setConfirmResetOpen(false)}
        onConfirm={() => void handleResetOverlay()}
        loading={actionBusy === "reset"}
        title={tx("清空沙盘覆盖层", "Reset Sandbox Overlay")}
        description={tx(
          "该操作将清除当前沙盘中所有虚拟节点、虚拟关系及指标补丁，恢复至基线快照。",
          "All virtual nodes, virtual links, and metric patches in this sandbox will be discarded and restored to the baseline snapshot."
        )}
        confirmLabel={tx("清空", "Reset")}
      />

      <ConfirmDialog
        open={confirmDeleteOpen}
        onClose={() => setConfirmDeleteOpen(false)}
        onConfirm={() => void handleDeleteSandbox()}
        loading={actionBusy === "delete"}
        title={tx("删除推演沙盘", "Delete Sandbox")}
        description={tx(
          `将永久删除沙盘「${activeSandbox?.name ?? ""}」及其关联的所有推演场景与覆盖层数据。`,
          `Permanently deletes sandbox "${activeSandbox?.name ?? ""}" and all of its scenarios and overlay edits.`
        )}
        confirmLabel={tx("删除", "Delete")}
      />
    </div>
  );
}
