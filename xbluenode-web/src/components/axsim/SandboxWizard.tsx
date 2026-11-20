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
} from "@/lib/api/axsim";
import {
  xblueApi,
  type BlueNode,
  type SubgraphInfo,
} from "@/lib/api/xblue";
import { Badge, Button, Card, Input } from "@/components/ui/primitives";

export interface SandboxWizardProps {
  open: boolean;
  activeSandbox: AxSandboxRecord | null;
  onClose: () => void;
  onCreated: (sandbox: AxSandboxRecord) => void;
  onUpdated: (sandbox: AxSandboxRecord) => void;
  onDeleted: (sandboxId: string) => void;
  onOverlayChanged: () => void;
}

const DIRECTION_OPTIONS: ReadonlyArray<{ value: GraphDirection; label: string }> = [
  { value: "BOTH", label: "双向 (BOTH)" },
  { value: "OUT", label: "下游 (OUT)" },
  { value: "IN", label: "上游 (IN)" },
];

export function SandboxWizard({
  open,
  activeSandbox,
  onClose,
  onCreated,
  onUpdated,
  onDeleted,
  onOverlayChanged,
}: SandboxWizardProps) {
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
  const [creating, setCreating] = useState(false);
  const [actionBusy, setActionBusy] = useState<string | null>(null);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [refreshReport, setRefreshReport] = useState<RefreshBaseResponse | null>(null);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

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

  // Reset delete confirmation & refresh report when switching sandboxes
  useEffect(() => {
    setConfirmDelete(false);
    setRefreshReport(null);
  }, [activeSandbox?.id]);

  // Load subgraphs and schema when wizard opens
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
        const schema = await xblueApi.getPublishedSchema(selectedSubgraph).catch(() => null);
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

  // Estimate matching node count for wizard preview
  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    void (async () => {
      try {
        const res = await xblueApi
          .querySubgraph({
            type_names: selectedTypes.length > 0 ? selectedTypes.join(",") : undefined,
            link_types: selectedLinkTypes.length > 0 ? selectedLinkTypes.join(",") : undefined,
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
  }, [open, selectedTypes, selectedLinkTypes]);

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
      prev.includes(typeName) ? prev.filter((t) => t !== typeName) : [...prev, typeName]
    );
  };

  const toggleLinkType = (linkType: string) => {
    setSelectedLinkTypes((prev) =>
      prev.includes(linkType) ? prev.filter((t) => t !== linkType) : [...prev, linkType]
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
      parts.push(`${selectedSeeds.length} 个种子节点`);
    } else {
      parts.push("全图范围");
    }
    parts.push(`深度 ${maxDepth || "3"} (${direction})`);
    if (selectedTypes.length > 0) {
      parts.push(`类型: ${selectedTypes.join(", ")}`);
    }
    return parts.join(" · ");
  }, [direction, maxDepth, selectedSeeds.length, selectedTypes]);

  const handleCreate = async (e: React.FormEvent) => {
    e.preventDefault();
    const trimmedName = name.trim();
    if (!trimmedName) {
      setErrorMsg("请输入沙盘分支名称");
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
    if (!activeSandbox) return;
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
    if (!activeSandbox) return;
    setActionBusy("reset");
    setErrorMsg(null);
    try {
      await axsimApi.resetOverlay(activeSandbox.id);
      onOverlayChanged();
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setActionBusy(null);
    }
  };

  const handleDeleteSandbox = async () => {
    if (!activeSandbox) return;
    if (!confirmDelete) {
      setConfirmDelete(true);
      return;
    }
    setActionBusy("delete");
    setErrorMsg(null);
    try {
      await axsimApi.deleteSandbox(activeSandbox.id);
      setConfirmDelete(false);
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
              沙盘「{activeSandbox.name}」正在后台克隆生产快照（CLONING），就绪后将自动刷新…
            </span>
          </div>
          <Badge variant="xblue">POLLING</Badge>
        </div>
      )}

      {activeSandbox && (
        <div className="flex flex-wrap items-center justify-between gap-2 rounded-md border border-border bg-muted/30 px-3 py-2 text-xs">
          <div className="flex flex-wrap items-center gap-2 text-muted-foreground">
            <span className="font-mono-tech">
              基线捕获:{" "}
              {activeSandbox.base_captured_at
                ? activeSandbox.base_captured_at.replace("T", " ").slice(0, 19)
                : "实时快照"}
            </span>
            {activeSandbox.schema_version != null && (
              <Badge variant="outline">Schema v{activeSandbox.schema_version}</Badge>
            )}
          </div>

          <div className="flex flex-wrap items-center gap-2">
            <Button
              type="button"
              variant="outline"
              className="h-7 px-2.5 text-[11px]"
              onClick={() => void handleRefreshBase()}
              disabled={actionBusy !== null}
              data-testid="sandbox-refresh-base-btn"
            >
              <RefreshCw
                className={`h-3 w-3 ${actionBusy === "refresh" ? "animate-spin" : ""}`}
              />
              刷新生产基线
            </Button>
            <Button
              type="button"
              variant="outline"
              className="h-7 px-2.5 text-[11px]"
              onClick={() => void handleResetOverlay()}
              disabled={actionBusy !== null}
              data-testid="sandbox-reset-overlay-btn"
            >
              <RotateCcw className="h-3 w-3" />
              清空覆盖层
            </Button>
            <Button
              type="button"
              variant="outline"
              className={`h-7 px-2.5 text-[11px] ${
                confirmDelete ? "border-red-500 text-red-600 dark:text-red-400" : ""
              }`}
              onClick={() => void handleDeleteSandbox()}
              disabled={actionBusy !== null}
              data-testid="sandbox-delete-btn"
            >
              <Trash2 className="h-3 w-3" />
              {confirmDelete ? "确认删除沙盘" : "删除沙盘"}
            </Button>
          </div>
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
              基线已刷新，检测到以下覆盖层条目在最新生产快照中已不存在（标记为孤立）：
            </div>
            <div className="font-mono-tech text-[11px] text-muted-foreground">
              孤立节点: {refreshReport.orphaned_node_ids.join(", ") || "无"} · 孤立关系:{" "}
              {refreshReport.orphaned_link_ids.join(", ") || "无"}
            </div>
          </div>
        )}

      {errorMsg && (
        <div className="rounded-md border border-red-500/40 bg-red-500/10 px-3 py-2 text-xs text-red-700 dark:text-red-300">
          {errorMsg}
        </div>
      )}

      {open && (
        <Card className="p-4 space-y-4 border-xblue/40" data-testid="sandbox-wizard-panel">
          <div className="flex items-center justify-between border-b border-border pb-2.5">
            <div className="flex items-center gap-2 text-sm font-medium">
              <GitBranch className="h-4 w-4 text-xblue" />
              <span>新建隔离推演沙盘（范围克隆 CoW）</span>
            </div>
            <Button
              type="button"
              variant="ghost"
              className="h-7 px-2"
              onClick={onClose}
              aria-label="关闭向导"
            >
              <X className="h-3.5 w-3.5" />
            </Button>
          </div>

          <form onSubmit={(e) => void handleCreate(e)} className="space-y-4">
            <div className="grid grid-cols-1 md:grid-cols-3 gap-3">
              <div className="md:col-span-2 space-y-1">
                <label className="text-xs font-medium text-muted-foreground">
                  沙盘名称 <span className="text-red-500">*</span>
                </label>
                <Input
                  value={name}
                  onChange={(e) => setName(e.target.value)}
                  placeholder="例如：华东断供 3 天应急推演"
                  data-testid="wizard-sandbox-name"
                />
              </div>

              <div className="space-y-1">
                <label className="text-xs font-medium text-muted-foreground">
                  参考子图模型 (Subgraph)
                </label>
                <select
                  value={selectedSubgraph}
                  onChange={(e) => setSelectedSubgraph(e.target.value)}
                  className="h-9 w-full rounded-md border border-input bg-background px-3 text-xs"
                  data-testid="wizard-subgraph-select"
                >
                  <option value="main">main (企业主图)</option>
                  {subgraphs
                    .filter((sg) => sg.slug !== "main")
                    .map((sg) => (
                      <option key={sg.id} value={sg.slug}>
                        {sg.slug} · {sg.display_name}
                      </option>
                    ))}
                </select>
              </div>
            </div>

            {/* Scope params */}
            <div className="grid grid-cols-1 md:grid-cols-3 gap-3">
              <div className="space-y-1">
                <label className="text-xs font-medium text-muted-foreground">
                  最大扩展深度 (1–6)
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
                  拓扑遍历方向
                </label>
                <select
                  value={direction}
                  onChange={(e) => setDirection(e.target.value as GraphDirection)}
                  className="h-9 w-full rounded-md border border-input bg-background px-3 text-xs"
                  data-testid="wizard-direction"
                >
                  {DIRECTION_OPTIONS.map((opt) => (
                    <option key={opt.value} value={opt.value}>
                      {opt.label}
                    </option>
                  ))}
                </select>
              </div>
              <div className="space-y-1">
                <label className="text-xs font-medium text-muted-foreground">
                  最大节点上限 (max_nodes)
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
                  节点类型过滤 (type_names，留空克隆全类型)
                </label>
                <div className="flex items-center gap-1.5">
                  <Input
                    value={customTypeInput}
                    onChange={(e) => setCustomTypeInput(e.target.value)}
                    placeholder="补充类型名..."
                    className="h-7 w-32 text-xs"
                  />
                  <Button
                    type="button"
                    variant="outline"
                    className="h-7 px-2 text-[11px]"
                    onClick={addCustomType}
                  >
                    <Plus className="h-3 w-3" />
                  </Button>
                </div>
              </div>
              <div className="flex flex-wrap gap-1.5">
                {Array.from(new Set([...availableTypes, ...selectedTypes])).map((t) => {
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
                })}
                {availableTypes.length === 0 && selectedTypes.length === 0 && (
                  <span className="text-[11px] text-muted-foreground">
                    当前 Schema 无预设类型，默认全量克隆
                  </span>
                )}
              </div>
            </div>

            {availableLinkTypes.length > 0 && (
              <div className="space-y-1.5">
                <label className="text-xs font-medium text-muted-foreground">
                  关系类型过滤 (link_types)
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
                <span>种子根节点检索 (seed_node_ids，留空从全图克隆)</span>
                {estimatedNodes !== null && (
                  <Badge variant="outline">
                    预计范围节点数: ~{estimatedNodes}
                    {estimatedNodes >= 500 ? "+" : ""}
                  </Badge>
                )}
              </div>

              <div className="flex flex-wrap gap-2">
                <div className="flex-1 min-w-[200px] flex gap-1.5">
                  <Input
                    value={seedQuery}
                    onChange={(e) => setSeedQuery(e.target.value)}
                    placeholder="按关键字搜索生产节点（如：F-1、华东仓）..."
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
                    检索
                  </Button>
                </div>

                <div className="flex gap-1.5">
                  <Input
                    value={manualSeedId}
                    onChange={(e) => setManualSeedId(e.target.value)}
                    placeholder="或直接输入节点 UUID"
                    className="h-8 w-48 text-xs font-mono-tech"
                  />
                  <Button
                    type="button"
                    variant="outline"
                    className="h-8 px-2.5 text-xs"
                    onClick={addManualSeedId}
                  >
                    添加
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
                  <span className="text-[11px] text-muted-foreground">已选种子:</span>
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

            <div className="flex flex-wrap items-center justify-between gap-2 pt-1">
              <div className="text-[11px] font-mono-tech text-muted-foreground">
                范围摘要：{scopePreview}
              </div>
              <div className="flex items-center gap-2">
                <Button type="button" variant="outline" onClick={onClose}>
                  取消
                </Button>
                <Button
                  type="submit"
                  disabled={creating || !name.trim()}
                  data-testid="wizard-create-submit"
                >
                  <DatabaseBackup className="h-3.5 w-3.5" />
                  {creating ? "克隆创建中..." : "创建并克隆沙盘"}
                </Button>
              </div>
            </div>
          </form>
        </Card>
      )}
    </div>
  );
}
