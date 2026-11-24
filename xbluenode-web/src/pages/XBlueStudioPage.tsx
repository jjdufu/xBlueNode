import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useSearchParams } from "react-router";
import {
  AlertTriangle,
  ArrowLeft,
  Blocks,
  BookOpen,
  BookOpenCheck,
  CheckCheck,
  CheckCircle2,
  ChevronDown,
  ChevronUp,
  Code2,
  Database,
  GitBranch,
  GitCompare,
  History,
  Plus,
  RefreshCw,
  Save,
  ShieldAlert,
  Sparkles,
  UploadCloud,
} from "lucide-react";
import { useI18n } from "@/i18n";
import { MODULE_META } from "@/modules/meta";
import { usePrincipal } from "@/lib/permissions";
import { ApiError } from "@/lib/api/client";
import { axhubApi, type Dataset } from "@/lib/api/axhub";
import { api, type TopologyNode } from "@/lib/api/rag";
import {
  computeLocalPassCount,
  confirmProposalsLocally,
  countUnconfirmedProposals,
  xblueApi,
  type AiDraftRequest,
  type BlueNode,
  type CreateSubgraphRequest,
  type EdgeTypeSpec,
  type MappingRecord,
  type MappingSpec,
  type MergePreview,
  type MergeSubgraphRequest,
  type NodeTypeSpec,
  type SchemaDoc,
  type SchemaDraft,
  type SchemaVersionSummary,
  type SubgraphConsumptionResponse,
  type SubgraphInfo,
  type TypeMergeResolution,
  type ValidationIssue,
} from "@/lib/api/xblue";
import {
  Badge,
  Button,
  Card,
  EmptyState,
  Input,
  Modal,
  PageHeader,
  SegmentedControl,
  Select,
  StatText,
  Switch,
  Tabs,
  Textarea,
} from "@/components/ui/primitives";
import { ResizableWorkspace } from "@/components/ui/ResizableWorkspace";
import { useCanvasHistory } from "@/components/canvas/useCanvasHistory";
import { XBlueGraphPage } from "@/pages/XBlueGraphPage";
import { cn } from "@/lib/utils";
import { AiDraftWizard } from "@/components/xbluestudio/AiDraftWizard";
import { DomainApiDrawer } from "@/components/xbluestudio/DomainApiDrawer";
import { EdgeTypeEditor } from "@/components/xbluestudio/EdgeTypeEditor";
import { IssueList } from "@/components/xbluestudio/IssueList";
import { KnowledgeCandidatesModal } from "@/components/xbluestudio/KnowledgeCandidatesModal";
import { MappingEditor } from "@/components/xbluestudio/MappingEditor";
import { ModelSpecOverview } from "@/components/xbluestudio/ModelSpecOverview";
import { NodeLineageCanvas } from "@/components/xbluestudio/NodeLineageCanvas";
import {
  NodeTypeCard,
  renderNodeIcon,
} from "@/components/xbluestudio/NodeTypeCard";
import { PropertyTable } from "@/components/xbluestudio/PropertyTable";
import { RuleEditor } from "@/components/xbluestudio/RuleEditor";
import { StudioAppDrawer } from "@/components/xbluestudio/StudioAppDrawer";
import { StudioCanvas } from "@/components/xbluestudio/StudioCanvas";
import { StudioCopilotPanel } from "@/components/xbluestudio/StudioCopilotPanel";
import { StudioSubgraphBar } from "@/components/xbluestudio/StudioSubgraphBar";
import { SubgraphMergeDialog } from "@/components/xbluestudio/SubgraphMergeDialog";
import { VersionPanel } from "@/components/xbluestudio/VersionPanel";

type StudioViewMode = "model" | "explorer";
type InspectorTab = "basic" | "properties" | "rules" | "edge" | "mappings";
type DockTab = "issues" | "nodes";

const EMPTY_DOC: SchemaDoc = {
  node_types: [],
  edge_types: [],
  mappings: [],
};

export function XBlueStudioPage() {
  const { tx, fmtNum } = useI18n();
  const [searchParams, setSearchParams] = useSearchParams();
  const tabParam = searchParams.get("tab");
  const nodeParam = searchParams.get("node");
  const studioMode: StudioViewMode =
    tabParam === "explorer" || tabParam === "graph" ? "explorer" : "model";
  const { can } = usePrincipal();
  const canEdit = can("STUDIO_EDIT");
  const canPublish = can("STUDIO_PUBLISH");

  const [subgraphs, setSubgraphs] = useState<SubgraphInfo[]>([]);
  const [activeSubgraph, setActiveSubgraph] = useState<string>("main");
  const [mainDoc, setMainDoc] = useState<SchemaDoc>(EMPTY_DOC);
  const [mainVersion, setMainVersion] = useState<number>(1);

  const [publishedVersion, setPublishedVersion] = useState<number>(1);
  const [baseVersion, setBaseVersion] = useState<number>(1);
  const [workingDoc, setWorkingDoc] = useState<SchemaDoc>(EMPTY_DOC);
  const [isDirty, setIsDirty] = useState<boolean>(false);

  const canvasHistory = useCanvasHistory<SchemaDoc>({
    currentDoc: workingDoc,
    enabled: studioMode === "model" && canEdit,
    limit: 50,
    onApplySnapshot: (snapshotDoc) => {
      setWorkingDoc(snapshotDoc);
      setServerPassCount(null);
      setIsDirty(true);
    },
  });
  const resetHistoryRef = useRef(canvasHistory.resetHistory);
  resetHistoryRef.current = canvasHistory.resetHistory;
  const recordDocRef = useRef(canvasHistory.recordDoc);
  recordDocRef.current = canvasHistory.recordDoc;

  const [drafts, setDrafts] = useState<SchemaDraft[]>([]);
  const [activeDraft, setActiveDraft] = useState<SchemaDraft | null>(null);
  const [draftTitle, setDraftTitle] = useState<string>("供应链建模演进草案");
  const [changeSummary, setChangeSummary] = useState<string>("");

  const [versions, setVersions] = useState<SchemaVersionSummary[]>([]);
  const [persistedMappings, setPersistedMappings] = useState<MappingRecord[]>(
    []
  );
  const [runtimeNodes, setRuntimeNodes] = useState<BlueNode[]>([]);
  const [datasets, setDatasets] = useState<Dataset[]>([]);
  const [docNodes, setDocNodes] = useState<TopologyNode[]>([]);

  const [issues, setIssues] = useState<ValidationIssue[]>([]);
  const [serverPassCount, setServerPassCount] = useState<number | null>(null);
  const [validating, setValidating] = useState<boolean>(false);

  const [selectedNodeName, setSelectedNodeName] = useState<string | null>(null);
  const [lineageNodeName, setLineageNodeName] = useState<string | null>(null);
  const [selectedEdgeIndex, setSelectedEdgeIndex] = useState<number | null>(
    null
  );
  const [selectedMappingIndex, setSelectedMappingIndex] = useState<
    number | null
  >(null);
  const [inspectorTab, setInspectorTab] = useState<InspectorTab>("basic");
  const [dockTab, setDockTab] = useState<DockTab>("issues");
  // Collapsed 34px status bar by default in 100vh IDE mode; auto-expands on errors or tab click
  const [dockCollapsed, setDockCollapsed] = useState<boolean>(true);
  const [copilotCollapsed, setCopilotCollapsed] = useState<boolean>(true);
  const [specOverviewOpen, setSpecOverviewOpen] = useState<boolean>(false);

  useEffect(() => {
    if (tabParam === "mappings") {
      setInspectorTab("mappings");
    } else if (tabParam === "proposals") {
      setDockTab("issues");
      setDockCollapsed(false);
    }
  }, [tabParam]);

  useEffect(() => {
    if (nodeParam) {
      setSelectedNodeName(nodeParam);
    }
  }, [nodeParam]);

  const handleStudioModeChange = (nextMode: StudioViewMode) => {
    const updated = new URLSearchParams(searchParams);
    if (nextMode === "explorer") {
      updated.set("tab", "explorer");
    } else {
      updated.delete("tab");
    }
    setSearchParams(updated, { replace: true });
  };

  const [statusBanner, setStatusBanner] = useState<string | null>(null);
  const [conflictBanner, setConflictBanner] = useState<string | null>(null);
  const [breakingBanner, setBreakingBanner] = useState<string | null>(null);
  const [forcePublish, setForcePublish] = useState<boolean>(false);
  const [cascadeDelete, setCascadeDelete] = useState<boolean>(false);
  const [busyOp, setBusyOp] = useState<string | null>(null);

  const [publishModalOpen, setPublishModalOpen] = useState<boolean>(false);
  const [versionPanelOpen, setVersionPanelOpen] = useState<boolean>(false);
  const [domainDrawerOpen, setDomainDrawerOpen] = useState<boolean>(false);
  const [appDrawerOpen, setAppDrawerOpen] = useState<boolean>(false);
  const [appDrawerFilterNode, setAppDrawerFilterNode] = useState<string | null>(
    null
  );
  const [consumption, setConsumption] =
    useState<SubgraphConsumptionResponse | null>(null);
  const [aiWizardOpen, setAiWizardOpen] = useState<boolean>(false);
  const [knowledgeCandidatesOpen, setKnowledgeCandidatesOpen] =
    useState<boolean>(false);

  const [mergeDialogOpen, setMergeDialogOpen] = useState<boolean>(false);
  const [mergePreview, setMergePreview] = useState<MergePreview | null>(null);
  const [mergeLoading, setMergeLoading] = useState<boolean>(false);
  const [merging, setMerging] = useState<boolean>(false);
  const [mergeError, setMergeError] = useState<string | null>(null);

  const skipNextValidationRef = useRef<boolean>(true);

  const loadSubgraphWorkspace = useCallback(
    async (slug: string, preferredDraftId?: string | null) => {
      skipNextValidationRef.current = true;
      const [
        sgList,
        pubSchema,
        verList,
        draftList,
        mapList,
        nodeSnap,
        mainSchemaOpt,
        consumptionRes,
      ] = await Promise.all([
        xblueApi.listSubgraphs().catch(() => [] as SubgraphInfo[]),
        xblueApi.getPublishedSchema(slug).catch(() => ({
          subgraph_slug: slug,
          version: 1,
          node_types: [],
          edge_types: [],
          mappings: [],
        })),
        xblueApi
          .listSchemaVersions(slug)
          .catch(() => [] as SchemaVersionSummary[]),
        xblueApi.listDrafts(slug).catch(() => [] as SchemaDraft[]),
        xblueApi.listMappings(slug).catch(() => [] as MappingRecord[]),
        xblueApi.listNodes({ subgraph: slug }).catch(() => ({
          nodes: [] as BlueNode[],
          links: [],
          items: [] as BlueNode[],
          next_cursor: null,
        })),
        slug === "main"
          ? Promise.resolve(null)
          : xblueApi.getPublishedSchema("main").catch(() => null),
        xblueApi.getSubgraphConsumption(slug).catch(() => null),
      ]);

      setSubgraphs(sgList);
      setVersions(verList);
      setPersistedMappings(mapList);
      setPublishedVersion(pubSchema.version);
      setConsumption(consumptionRes);

      const resolvedNodes = Array.isArray(nodeSnap?.nodes)
        ? nodeSnap.nodes
        : Array.isArray(nodeSnap?.items)
          ? nodeSnap.items
          : [];
      setRuntimeNodes(resolvedNodes);

      const openDrafts = draftList.filter((d) => d.status === "DRAFT");
      setDrafts(openDrafts);

      const schemaDoc: SchemaDoc = {
        node_types: pubSchema.node_types,
        edge_types: pubSchema.edge_types,
        mappings: pubSchema.mappings,
      };

      if (slug === "main") {
        setMainDoc(schemaDoc);
        setMainVersion(pubSchema.version);
      } else if (mainSchemaOpt) {
        setMainDoc({
          node_types: mainSchemaOpt.node_types,
          edge_types: mainSchemaOpt.edge_types,
          mappings: mainSchemaOpt.mappings,
        });
        setMainVersion(mainSchemaOpt.version);
      }

      const matchedDraft = preferredDraftId
        ? openDrafts.find((d) => d.id === preferredDraftId)
        : undefined;
      const targetDraft = matchedDraft ?? openDrafts[0];

      if (targetDraft) {
        setActiveDraft(targetDraft);
        setDraftTitle(targetDraft.title);
        setBaseVersion(targetDraft.base_version);
        setWorkingDoc(targetDraft.schema_doc);
        resetHistoryRef.current(targetDraft.schema_doc);
        setIssues(targetDraft.issues);
        setServerPassCount(targetDraft.pass_count ?? null);
        setIsDirty(false);
        setSelectedNodeName(targetDraft.schema_doc.node_types[0]?.name ?? null);
      } else {
        setActiveDraft(null);
        setDraftTitle(`${slug} 模型演进草案`);
        setBaseVersion(pubSchema.version);
        setWorkingDoc(schemaDoc);
        resetHistoryRef.current(schemaDoc);
        setIssues([]);
        setServerPassCount(null);
        setIsDirty(false);
        setSelectedNodeName(schemaDoc.node_types[0]?.name ?? null);
      }

      setLineageNodeName(null);
      setSelectedEdgeIndex(null);
      setSelectedMappingIndex(null);
      setConflictBanner(null);
      setBreakingBanner(null);
    },
    []
  );

  useEffect(() => {
    let cancelled = false;
    const bootstrap = async () => {
      const searchParams =
        typeof window !== "undefined"
          ? new URLSearchParams(window.location.search)
          : new URLSearchParams();
      const initialSlug = searchParams.get("subgraph")?.trim() || "main";
      const initialDraftId = searchParams.get("draft")?.trim() || null;
      if (initialSlug !== "main") {
        setActiveSubgraph(initialSlug);
      }
      await loadSubgraphWorkspace(initialSlug, initialDraftId);
      const [dsResp, topoResp] = await Promise.all([
        axhubApi
          .listDatasets({ limit: 100 })
          .catch(() => ({ items: [] as Dataset[], next_cursor: null })),
        api
          .topology()
          .catch(() => ({ nodes: [] as TopologyNode[], edges: [] })),
      ]);
      if (cancelled) return;
      setDatasets(Array.isArray(dsResp?.items) ? dsResp.items : []);
      setDocNodes(
        Array.isArray(topoResp?.nodes)
          ? topoResp.nodes.filter((n) => n.node_type === "DOC")
          : []
      );
    };
    void bootstrap();
    return () => {
      cancelled = true;
    };
  }, [loadSubgraphWorkspace]);

  // 500ms debounced real-time schema validation on workingDoc edits
  useEffect(() => {
    if (skipNextValidationRef.current) {
      skipNextValidationRef.current = false;
      return;
    }
    let cancelled = false;
    setValidating(true);
    const timer = window.setTimeout(() => {
      void xblueApi
        .validateSchema(workingDoc, activeSubgraph)
        .then((res) => {
          if (!cancelled) {
            setIssues(res.issues);
            if (typeof res.pass_count === "number") {
              setServerPassCount(res.pass_count);
            } else {
              setServerPassCount(null);
            }
          }
        })
        .catch(() => {
          // Graceful degradation on offline / mock environments
        })
        .finally(() => {
          if (!cancelled) {
            setValidating(false);
          }
        });
    }, 500);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [workingDoc, activeSubgraph]);

  const mutateDoc = useCallback((updater: (prev: SchemaDoc) => SchemaDoc) => {
    setWorkingDoc((prev) => {
      const next = updater(prev);
      recordDocRef.current(next);
      return next;
    });
    setServerPassCount(null);
    setIsDirty(true);
  }, []);

  const availableNodeTypesForEdges = useMemo(() => {
    if (activeSubgraph === "main") return workingDoc.node_types;
    const names = new Set(workingDoc.node_types.map((n) => n.name));
    const merged = [...workingDoc.node_types];
    for (const mnt of mainDoc.node_types) {
      if (!names.has(mnt.name)) {
        merged.push(mnt);
      }
    }
    return merged;
  }, [activeSubgraph, workingDoc.node_types, mainDoc.node_types]);

  const selectedNodeIndex = useMemo(() => {
    if (selectedNodeName) {
      const idx = workingDoc.node_types.findIndex(
        (nt) => nt.name === selectedNodeName
      );
      if (idx >= 0) return idx;
    }
    return workingDoc.node_types.length > 0 ? 0 : -1;
  }, [workingDoc.node_types, selectedNodeName]);

  const selectedNode =
    selectedNodeIndex >= 0
      ? (workingDoc.node_types[selectedNodeIndex] ?? null)
      : null;

  // Auto-select first edge when switching to `edge` tab so 70% dead empty space never occurs
  const effectiveEdgeIndex =
    selectedEdgeIndex !== null && workingDoc.edge_types[selectedEdgeIndex]
      ? selectedEdgeIndex
      : workingDoc.edge_types.length > 0
        ? 0
        : null;

  const selectedEdge =
    effectiveEdgeIndex !== null
      ? (workingDoc.edge_types[effectiveEdgeIndex] ?? null)
      : null;

  const errorCount = useMemo(
    () => issues.filter((i) => i.severity === "ERROR").length,
    [issues]
  );

  const unconfirmedCount = useMemo(
    () => countUnconfirmedProposals(workingDoc),
    [workingDoc]
  );

  const passCount = useMemo(
    () =>
      serverPassCount ??
      activeDraft?.pass_count ??
      computeLocalPassCount(workingDoc),
    [serverPassCount, activeDraft?.pass_count, workingDoc]
  );

  const handleSelectSubgraph = async (slug: string) => {
    if (slug === activeSubgraph) return;
    setActiveSubgraph(slug);
    setStatusBanner(null);
    await loadSubgraphWorkspace(slug);
  };

  const handleCreateSubgraph = async (req: CreateSubgraphRequest) => {
    const created = await xblueApi.createSubgraph(req);
    setActiveSubgraph(created.slug);
    setStatusBanner(
      tx(
        `已创建部门子图 ${created.display_name} (${created.slug})`,
        `Created subgraph ${created.display_name} (${created.slug})`
      )
    );
    await loadSubgraphWorkspace(created.slug);
  };

  const handleDeleteSubgraph = async (slug: string) => {
    if (slug === "main" || !canEdit) return;
    try {
      await xblueApi.deleteSubgraph(slug);
      setActiveSubgraph("main");
      setStatusBanner(tx(`已删除子图 ${slug}`, `Deleted subgraph ${slug}`));
      await loadSubgraphWorkspace("main");
    } catch (err) {
      setStatusBanner(
        err instanceof Error
          ? err.message
          : tx("删除子图失败", "Failed to delete subgraph")
      );
    }
  };

  const handleDryRunSubgraph = async () => {
    if (!canEdit) return;
    setBusyOp("dry-run-subgraph");
    setStatusBanner(null);
    try {
      const res = await xblueApi.dryRunSubgraph(
        activeSubgraph,
        activeDraft?.id ? { draft_id: activeDraft.id } : undefined
      );
      const [nodeSnap, latestSubgraphs] = await Promise.all([
        xblueApi.listNodes({ subgraph: activeSubgraph }).catch(() => ({
          nodes: [] as BlueNode[],
          links: [],
          items: [] as BlueNode[],
          next_cursor: null,
        })),
        xblueApi.listSubgraphs().catch(() => null),
      ]);
      const resolvedNodes = Array.isArray(nodeSnap?.nodes)
        ? nodeSnap.nodes
        : Array.isArray(nodeSnap?.items)
          ? nodeSnap.items
          : [];
      setRuntimeNodes(
        resolvedNodes.length > 0
          ? resolvedNodes
          : Array.isArray(res.sample_nodes)
            ? res.sample_nodes
            : []
      );
      if (latestSubgraphs) {
        setSubgraphs(latestSubgraphs);
      }
      setDockTab("nodes");
      setDockCollapsed(false);
      setStatusBanner(
        tx(
          `试灌完成：写入 ${fmtNum(res.nodes_upserted, 0)} 节点 · ${fmtNum(res.links_upserted, 0)} 关系 · ${fmtNum(res.points_computed, 0)} 指标点`,
          `Dry run complete: ${fmtNum(res.nodes_upserted, 0)} nodes · ${fmtNum(res.links_upserted, 0)} links · ${fmtNum(res.points_computed, 0)} points`
        )
      );
    } catch (err) {
      setStatusBanner(
        err instanceof Error
          ? err.message
          : tx("试灌失败", "Dry run failed")
      );
    } finally {
      setBusyOp(null);
    }
  };

  const handleResetSubgraphNodes = async () => {
    if (!canEdit) return;
    setBusyOp("reset-subgraph");
    setStatusBanner(null);
    try {
      const res = await xblueApi.resetSubgraphNodes(activeSubgraph);
      const [nodeSnap, latestSubgraphs] = await Promise.all([
        xblueApi.listNodes({ subgraph: activeSubgraph }).catch(() => ({
          nodes: [] as BlueNode[],
          links: [],
          items: [] as BlueNode[],
          next_cursor: null,
        })),
        xblueApi.listSubgraphs().catch(() => null),
      ]);
      const resolvedNodes = Array.isArray(nodeSnap?.nodes)
        ? nodeSnap.nodes
        : Array.isArray(nodeSnap?.items)
          ? nodeSnap.items
          : [];
      setRuntimeNodes(resolvedNodes);
      if (latestSubgraphs) {
        setSubgraphs(latestSubgraphs);
      }
      setStatusBanner(
        tx(
          `已清空 ${fmtNum(res.nodes_deleted, 0)} 节点 · ${fmtNum(res.links_deleted, 0)} 关系 · ${fmtNum(res.series_points_deleted, 0)} 时序点`,
          `Reset ${fmtNum(res.nodes_deleted, 0)} nodes · ${fmtNum(res.links_deleted, 0)} links · ${fmtNum(res.series_points_deleted, 0)} series points`
        )
      );
    } catch (err) {
      setStatusBanner(
        err instanceof Error
          ? err.message
          : tx("清空节点失败", "Reset failed")
      );
    } finally {
      setBusyOp(null);
    }
  };

  const handleCreateNodeType = (name: string, displayName: string) => {
    if (!canEdit) return;
    const idx = workingDoc.node_types.length;
    const newSpec: NodeTypeSpec = {
      name,
      display_name: displayName,
      description: "",
      icon: "box",
      color: "#2563eb",
      title_template: "{node_key}",
      properties: [],
      agg_rules: [],
      derived_rules: [],
      sync_to_knowledge: false,
      pos_x: 100 + (idx % 3) * 340,
      pos_y: 120 + Math.floor(idx / 3) * 196,
    };
    mutateDoc((prev) => ({
      ...prev,
      node_types: [...prev.node_types, newSpec],
    }));
    setSelectedEdgeIndex(null);
    setSelectedNodeName(name);
    setInspectorTab("basic");
  };

  const handleCreateEdgeType = (edge: EdgeTypeSpec) => {
    if (!canEdit) return;
    const nextIdx = workingDoc.edge_types.length;
    mutateDoc((prev) => ({
      ...prev,
      edge_types: [...prev.edge_types, edge],
    }));
    setSelectedNodeName(null);
    setSelectedEdgeIndex(nextIdx);
    setInspectorTab("edge");
  };

  const handleInlineAddEdge = () => {
    const src = workingDoc.node_types[0]?.name ?? "";
    const dst =
      workingDoc.node_types[1]?.name ?? workingDoc.node_types[0]?.name ?? "";
    handleCreateEdgeType({
      name: `REL_${workingDoc.edge_types.length + 1}`,
      src_type_name: src,
      dst_type_name: dst,
      impact_weight: 0.7,
      cardinality: "ONE_TO_MANY",
      description: "",
      properties: [],
      propagation: {},
    });
  };

  const handleUpdateSelectedNode = (updated: NodeTypeSpec) => {
    if (!canEdit || selectedNodeIndex < 0) return;
    const oldName = workingDoc.node_types[selectedNodeIndex]?.name;
    mutateDoc((prev) => ({
      ...prev,
      node_types: prev.node_types.map((nt, i) =>
        i === selectedNodeIndex ? updated : nt
      ),
      edge_types:
        oldName && oldName !== updated.name
          ? prev.edge_types.map((et) => ({
              ...et,
              src_type_name:
                et.src_type_name === oldName ? updated.name : et.src_type_name,
              dst_type_name:
                et.dst_type_name === oldName ? updated.name : et.dst_type_name,
            }))
          : prev.edge_types,
    }));
    setSelectedNodeName(updated.name);
  };

  const handleDeleteSelectedNode = () => {
    if (!canEdit || !selectedNode) return;
    const targetName = selectedNode.name;
    mutateDoc((prev) => ({
      ...prev,
      node_types: prev.node_types.filter((nt) => nt.name !== targetName),
      edge_types: prev.edge_types.filter(
        (et) =>
          et.src_type_name !== targetName && et.dst_type_name !== targetName
      ),
    }));
    setSelectedNodeName(null);
  };

  const handleSaveDraft = async () => {
    if (!canEdit) return;
    setBusyOp("save-draft");
    setStatusBanner(null);
    try {
      let saved: SchemaDraft;
      if (activeDraft?.id) {
        saved = await xblueApi.updateDraft(activeDraft.id, {
          title: draftTitle,
          schema_doc: workingDoc,
        });
      } else {
        saved = await xblueApi.createDraft(
          {
            title:
              draftTitle ||
              tx(`${activeSubgraph} 草案`, `${activeSubgraph} Draft`),
            schema_doc: workingDoc,
            base_version: baseVersion,
          },
          activeSubgraph
        );
      }
      setActiveDraft(saved);
      setIssues(saved.issues);
      setServerPassCount(saved.pass_count ?? null);
      setIsDirty(false);
      const latestDrafts = await xblueApi
        .listDrafts(activeSubgraph)
        .catch(() => [saved]);
      setDrafts(latestDrafts.filter((d) => d.status === "DRAFT"));
      setStatusBanner(
        tx(
          `草案「${saved.title}」已保存 (基线 v${saved.base_version})`,
          `Draft "${saved.title}" saved (base v${saved.base_version})`
        )
      );
    } catch (err) {
      setStatusBanner(
        err instanceof Error
          ? err.message
          : tx("保存草案失败", "Failed to save draft")
      );
    } finally {
      setBusyOp(null);
    }
  };

  const ensureActiveDraft = useCallback(async (): Promise<SchemaDraft | null> => {
    if (!canEdit) return activeDraft;
    if (activeDraft?.id && !isDirty) {
      return activeDraft;
    }
    let saved: SchemaDraft;
    if (activeDraft?.id) {
      saved = await xblueApi.updateDraft(activeDraft.id, {
        title: draftTitle,
        schema_doc: workingDoc,
      });
    } else {
      saved = await xblueApi.createDraft(
        {
          subgraph_slug: activeSubgraph,
          title:
            draftTitle ||
            tx(`${activeSubgraph} 草案`, `${activeSubgraph} Draft`),
          schema_doc: workingDoc,
          base_version: baseVersion,
        },
        activeSubgraph
      );
    }
    skipNextValidationRef.current = true;
    setActiveDraft(saved);
    setIssues(saved.issues);
    setServerPassCount(saved.pass_count ?? null);
    setIsDirty(false);
    const latestDrafts = await xblueApi
      .listDrafts(activeSubgraph)
      .catch(() => [saved]);
    setDrafts(latestDrafts.filter((d) => d.status === "DRAFT"));
    return saved;
  }, [
    canEdit,
    activeDraft,
    isDirty,
    draftTitle,
    workingDoc,
    activeSubgraph,
    baseVersion,
    tx,
  ]);

  const handleApplyUpdatedDraft = useCallback(
    (draft: SchemaDraft, notice?: string) => {
      skipNextValidationRef.current = true;
      setActiveDraft(draft);
      setDraftTitle(draft.title);
      setBaseVersion(draft.base_version);
      setWorkingDoc(draft.schema_doc);
      resetHistoryRef.current(draft.schema_doc);
      setIssues(draft.issues);
      setServerPassCount(draft.pass_count ?? null);
      setIsDirty(false);
      setDrafts((prev) => {
        const filtered = prev.filter((d) => d.id !== draft.id);
        return draft.id ? [draft, ...filtered] : prev;
      });
      if (
        selectedNodeName &&
        !draft.schema_doc.node_types.some((n) => n.name === selectedNodeName)
      ) {
        setSelectedNodeName(draft.schema_doc.node_types[0]?.name ?? null);
      } else if (!selectedNodeName && draft.schema_doc.node_types.length > 0) {
        setSelectedNodeName(draft.schema_doc.node_types[0]?.name ?? null);
      }
      if (notice) {
        setStatusBanner(notice);
      }
    },
    [selectedNodeName]
  );

  const handleConfirmAllProposals = async () => {
    if (!canEdit || busyOp !== null) return;
    setBusyOp("confirm-proposals");
    setStatusBanner(null);
    try {
      const draft = await ensureActiveDraft();
      if (draft?.id) {
        const confirmed = await xblueApi.confirmDraftProposals(draft.id, {
          scope: "ALL",
        });
        if (
          confirmed &&
          confirmed.id &&
          Array.isArray(confirmed.schema_doc?.node_types)
        ) {
          handleApplyUpdatedDraft(
            confirmed,
            tx("已确认全部待审提议要素", "Confirmed all proposed items")
          );
          return;
        }
      }
      mutateDoc((prev) => confirmProposalsLocally(prev, { scope: "ALL" }));
      setStatusBanner(
        tx("已确认全部待审提议要素", "Confirmed all proposed items")
      );
    } catch (err) {
      setStatusBanner(
        err instanceof Error
          ? err.message
          : tx("确认提议失败", "Failed to confirm proposals")
      );
    } finally {
      setBusyOp(null);
    }
  };

  const handleConfirmNodeProposal = async (nodeName: string) => {
    if (!canEdit || busyOp !== null) return;
    setBusyOp("confirm-proposals");
    setStatusBanner(null);
    try {
      const draft = await ensureActiveDraft();
      if (draft?.id) {
        const confirmed = await xblueApi.confirmDraftProposals(draft.id, {
          scope: "SELECTED",
          node_types: [nodeName],
        });
        if (
          confirmed &&
          confirmed.id &&
          Array.isArray(confirmed.schema_doc?.node_types)
        ) {
          handleApplyUpdatedDraft(
            confirmed,
            tx(`已确认 ${nodeName} 提议要素`, `Confirmed proposals on ${nodeName}`)
          );
          return;
        }
      }
      mutateDoc((prev) =>
        confirmProposalsLocally(prev, {
          scope: "SELECTED",
          node_types: [nodeName],
        })
      );
      setStatusBanner(
        tx(`已确认 ${nodeName} 提议要素`, `Confirmed proposals on ${nodeName}`)
      );
    } catch (err) {
      setStatusBanner(
        err instanceof Error
          ? err.message
          : tx("确认提议失败", "Failed to confirm proposals")
      );
    } finally {
      setBusyOp(null);
    }
  };

  const handleSelectDraft = (draft: SchemaDraft) => {
    skipNextValidationRef.current = true;
    setActiveDraft(draft);
    setDraftTitle(draft.title);
    setBaseVersion(draft.base_version);
    setWorkingDoc(draft.schema_doc);
    resetHistoryRef.current(draft.schema_doc);
    setIssues(draft.issues);
    setServerPassCount(draft.pass_count ?? null);
    setIsDirty(false);
    setSelectedNodeName(draft.schema_doc.node_types[0]?.name ?? null);
    setSelectedEdgeIndex(null);
  };

  const handleDiscardActiveDraft = async () => {
    if (!canEdit || !activeDraft?.id) return;
    setBusyOp("discard-draft");
    try {
      await xblueApi.discardDraft(activeDraft.id);
      setStatusBanner(
        tx(
          `已丢弃草案「${activeDraft.title}」`,
          `Discarded draft "${activeDraft.title}"`
        )
      );
      await loadSubgraphWorkspace(activeSubgraph);
    } catch (err) {
      setStatusBanner(
        err instanceof Error
          ? err.message
          : tx("丢弃草案失败", "Failed to discard draft")
      );
    } finally {
      setBusyOp(null);
    }
  };

  const executePublish = async (forceOverride?: boolean) => {
    if (!canPublish) return;
    if (errorCount > 0) {
      setPublishModalOpen(false);
      setDockCollapsed(false);
      setDockTab("issues");
      setStatusBanner(
        tx(
          `存在 ${errorCount} 个阻断校验错误，修复后方可发布`,
          `${errorCount} blocking validation errors must be resolved before publishing`
        )
      );
      return;
    }

    setBusyOp("publish");
    setStatusBanner(null);
    setConflictBanner(null);
    setBreakingBanner(null);
    const useForce = forceOverride ?? forcePublish;

    try {
      let targetDraft = activeDraft;
      if (!targetDraft?.id) {
        targetDraft = await xblueApi.createDraft(
          {
            title:
              draftTitle ||
              tx(`${activeSubgraph} 发布草案`, `${activeSubgraph} Release`),
            schema_doc: workingDoc,
            base_version: baseVersion,
          },
          activeSubgraph
        );
      } else if (isDirty) {
        targetDraft = await xblueApi.updateDraft(targetDraft.id, {
          title: draftTitle,
          schema_doc: workingDoc,
        });
      }

      if (targetDraft.id) {
        const pubRes = await xblueApi.publishDraft(targetDraft.id, {
          change_summary:
            changeSummary.trim() ||
            tx(`发布 ${activeSubgraph} 模型更新`, `Publish ${activeSubgraph}`),
          force: useForce,
          cascade: useForce && cascadeDelete,
        });
        setStatusBanner(
          tx(
            `子图 ${pubRes.subgraph_slug} 已发布版本 v${pubRes.version}`,
            `Published ${pubRes.subgraph_slug} v${pubRes.version}`
          )
        );
      } else {
        const directRes = await xblueApi.saveSchemaDirect(
          {
            ...workingDoc,
            base_version: baseVersion,
            change_summary:
              changeSummary.trim() ||
              tx(`发布 ${activeSubgraph} 模型更新`, `Publish ${activeSubgraph}`),
            force: useForce,
            cascade: useForce && cascadeDelete,
          },
          activeSubgraph
        );
        setStatusBanner(
          tx(
            `子图 ${directRes.subgraph_slug} 已发布版本 v${directRes.version}`,
            `Published ${directRes.subgraph_slug} v${directRes.version}`
          )
        );
      }

      setPublishModalOpen(false);
      setChangeSummary("");
      setForcePublish(false);
      setCascadeDelete(false);
      await loadSubgraphWorkspace(activeSubgraph);
    } catch (err) {
      setPublishModalOpen(false);
      if (err instanceof ApiError) {
        if (err.status === 409 && err.errorCode === "VERSION_CONFLICT") {
          setConflictBanner(
            err.message ||
              tx(
                "并发版本冲突：当前子图已有新发布版本",
                "Version conflict: a newer version was published concurrently"
              )
          );
          return;
        }
        if (
          err.status === 409 &&
          err.errorCode === "SCHEMA_BREAKING_CHANGE"
        ) {
          setBreakingBanner(
            err.message ||
              tx(
                "检测到破坏性模型变更：需确认强制发布",
                "Breaking schema change detected: force publish confirmation required"
              )
          );
          return;
        }
        if (
          err.status === 400 &&
          err.errorCode === "UNCONFIRMED_PROPOSALS_REMAIN"
        ) {
          setDockCollapsed(false);
          setDockTab("issues");
          setStatusBanner(
            err.message ||
              tx(
                "存在待确认提议要素，确认后方可发布",
                "Unconfirmed proposals remain; sign off before publishing"
              )
          );
          return;
        }
      }
      setStatusBanner(
        err instanceof Error
          ? err.message
          : tx("发布失败", "Failed to publish schema")
      );
    } finally {
      setBusyOp(null);
    }
  };

  const handleRestoreVersion = async (v: number) => {
    const restored = await xblueApi.restoreSchemaVersion(v, activeSubgraph);
    skipNextValidationRef.current = true;
    setActiveDraft(restored);
    setDraftTitle(restored.title);
    setBaseVersion(restored.base_version);
    setWorkingDoc(restored.schema_doc);
    resetHistoryRef.current(restored.schema_doc);
    setIssues(restored.issues);
    setServerPassCount(restored.pass_count ?? null);
    setIsDirty(false);
    setStatusBanner(
      tx(
        `已从 v${v} 恢复草案「${restored.title}」`,
        `Restored draft "${restored.title}" from v${v}`
      )
    );
  };

  const handleGenerateAiDraft = async (req: AiDraftRequest) => {
    const generated = await xblueApi.createAiDraft(req, activeSubgraph);
    skipNextValidationRef.current = true;
    setActiveDraft(generated);
    setDraftTitle(generated.title);
    setBaseVersion(generated.base_version);
    setWorkingDoc(generated.schema_doc);
    resetHistoryRef.current(generated.schema_doc);
    setIssues(generated.issues);
    setServerPassCount(generated.pass_count ?? null);
    setIsDirty(false);
    setSelectedNodeName(generated.schema_doc.node_types[0]?.name ?? null);
    const latestDrafts = await xblueApi
      .listDrafts(activeSubgraph)
      .catch(() => [generated]);
    setDrafts(latestDrafts.filter((d) => d.status === "DRAFT"));
    setStatusBanner(
      tx(
        `AI 草案「${generated.title}」已载入画布`,
        `Loaded AI draft "${generated.title}"`
      )
    );
  };

  const openMergeDialog = async () => {
    if (activeSubgraph === "main") return;
    setMergeDialogOpen(true);
    setMergeLoading(true);
    setMergeError(null);
    try {
      const prev = await xblueApi.previewSubgraphMerge(activeSubgraph);
      setMergePreview(prev);
    } catch (err) {
      setMergeError(
        err instanceof Error
          ? err.message
          : tx("合并预演加载失败", "Failed to load merge preview")
      );
    } finally {
      setMergeLoading(false);
    }
  };

  const refreshMergePreview = async (resolutions: TypeMergeResolution[]) => {
    setMergeLoading(true);
    setMergeError(null);
    try {
      const prev = await xblueApi.previewSubgraphMerge(activeSubgraph, {
        resolutions,
      });
      setMergePreview(prev);
    } catch (err) {
      setMergeError(
        err instanceof Error
          ? err.message
          : tx("刷新预演失败", "Failed to refresh preview")
      );
    } finally {
      setMergeLoading(false);
    }
  };

  const executeSubgraphMerge = async (req: MergeSubgraphRequest) => {
    setMerging(true);
    setMergeError(null);
    try {
      const res = await xblueApi.mergeSubgraph(activeSubgraph, req);
      setMergeDialogOpen(false);
      setActiveSubgraph("main");
      await loadSubgraphWorkspace("main");
      setStatusBanner(
        tx(
          `子图 ${activeSubgraph} 已合并发布至企业主图 main (v${res.version})`,
          `Merged ${activeSubgraph} into main (v${res.version})`
        )
      );
    } catch (err) {
      setMergeError(
        err instanceof Error
          ? err.message
          : tx("合并失败", "Failed to merge subgraph")
      );
    } finally {
      setMerging(false);
    }
  };

  const handleLocateTargetPath = (rawPath: string) => {
    const nodeIdxMatch = /^node_types\[(\d+)\](?:\.([a-z_]+))?/.exec(rawPath);
    if (nodeIdxMatch) {
      const idx = Number(nodeIdxMatch[1]);
      const subField = nodeIdxMatch[2] ?? "";
      const targetNode = workingDoc.node_types[idx];
      if (targetNode) {
        setSelectedEdgeIndex(null);
        setSelectedNodeName(targetNode.name);
        if (subField === "agg_rules" || subField === "derived_rules") {
          setInspectorTab("rules");
        } else if (subField === "properties") {
          setInspectorTab("properties");
        } else {
          setInspectorTab("basic");
        }
        return;
      }
    }

    const nodeNamedMatch = /^node_types\[([A-Za-z0-9_]+)\](?:\.([a-z_]+))?/.exec(
      rawPath
    );
    if (nodeNamedMatch) {
      const targetName = nodeNamedMatch[1] ?? "";
      const subField = nodeNamedMatch[2] ?? "";
      if (workingDoc.node_types.some((n) => n.name === targetName)) {
        setSelectedEdgeIndex(null);
        setSelectedNodeName(targetName);
        if (subField === "agg_rules" || subField === "derived_rules") {
          setInspectorTab("rules");
        } else if (subField === "properties") {
          setInspectorTab("properties");
        } else {
          setInspectorTab("basic");
        }
        return;
      }
    }

    const edgeIdxMatch = /^edge_types\[(\d+)\]/.exec(rawPath);
    if (edgeIdxMatch) {
      const eIdx = Number(edgeIdxMatch[1]);
      if (workingDoc.edge_types[eIdx]) {
        setSelectedNodeName(null);
        setSelectedEdgeIndex(eIdx);
        setInspectorTab("edge");
        return;
      }
    }

    const edgeNamedMatch = /^edge_types\[([A-Za-z0-9_]+)\]/.exec(rawPath);
    if (edgeNamedMatch) {
      const eName = edgeNamedMatch[1] ?? "";
      const foundIdx = workingDoc.edge_types.findIndex((e) => e.name === eName);
      if (foundIdx >= 0) {
        setSelectedNodeName(null);
        setSelectedEdgeIndex(foundIdx);
        setInspectorTab("edge");
        return;
      }
    }

    const mapMatch = /^mappings\[(\d+)\]/.exec(rawPath);
    if (mapMatch) {
      const mIdx = Number(mapMatch[1]);
      if (workingDoc.mappings[mIdx]) {
        setSelectedMappingIndex(mIdx);
        setInspectorTab("mappings");
      }
    }
  };

  const handleLocateIssue = (issue: ValidationIssue) => {
    handleLocateTargetPath(issue.path);
  };

  const activeSubgraphInfo = subgraphs.find((s) => s.slug === activeSubgraph);

  const refreshConsumption = useCallback(async () => {
    const latest = await xblueApi
      .getSubgraphConsumption(activeSubgraph)
      .catch(() => null);
    setConsumption(latest);
  }, [activeSubgraph]);

  const consumptionByNodeType = useMemo(() => {
    if (
      consumption &&
      consumption.by_node_type &&
      typeof consumption.by_node_type === "object"
    ) {
      return consumption.by_node_type;
    }
    return {};
  }, [consumption]);

  return (
    <div className="flex flex-1 min-h-0 flex-col gap-2 overflow-hidden">
      {/* Unified Header — h1 preserves smoke.spec.ts zh-CN assertion */}
      <PageHeader
        className="shrink-0 pb-2"
        title={tx(
          MODULE_META.xbluestudio.label,
          MODULE_META.xbluestudio.enLabel
        )}
        badges={
          <div className="flex items-center gap-3 pl-1">
            {studioMode === "model" && (
              <>
                <StatText
                  label={tx("类型", "Types")}
                  value={workingDoc.node_types.length}
                />
                <span className="text-border">·</span>
                <StatText
                  label={tx("关系", "Relations")}
                  value={workingDoc.edge_types.length}
                />
              </>
            )}
            {studioMode === "model" &&
              activeSubgraphInfo?.is_dev_sandbox &&
              activeSubgraphInfo?.source_template_id && (
                <Badge variant="xblue" data-testid="studio-trial-sandbox-badge">
                  {tx(
                    `演练沙盒 · ${activeSubgraphInfo.source_template_id}`,
                    `Trial Sandbox · ${activeSubgraphInfo.source_template_id}`
                  )}
                </Badge>
              )}
          </div>
        }
        actions={
          <>
            <SegmentedControl<StudioViewMode>
              value={studioMode}
              onChange={handleStudioModeChange}
              items={[
                {
                  key: "model",
                  label: tx("模型设计", "Model"),
                  icon: <Blocks className="h-3.5 w-3.5" />,
                  testId: "studio-mode-model",
                },
                {
                  key: "explorer",
                  label: tx("图谱浏览", "Explorer"),
                  icon: <GitBranch className="h-3.5 w-3.5" />,
                  testId: "studio-mode-explorer",
                },
              ]}
            />

            {studioMode === "model" && (
              <>
                {canEdit && (
                  <>
                    <Button
                      variant="outline"
                      data-testid="open-ai-wizard-btn"
                      onClick={() => setAiWizardOpen(true)}
                    >
                      <Sparkles className="h-3.5 w-3.5 text-xblue" />
                      {tx("AI 草案", "AI Draft")}
                    </Button>

                    <Button
                      variant="outline"
                      data-testid="knowledge-candidates-btn"
                      onClick={() => setKnowledgeCandidatesOpen(true)}
                    >
                      <BookOpenCheck className="h-3.5 w-3.5 text-xblue" />
                      {tx("知识建点", "KG Candidates")}
                    </Button>
                  </>
                )}

                <Button
                  variant="outline"
                  data-testid="toggle-lineage-view-btn"
                  disabled={
                    lineageNodeName === null &&
                    workingDoc.node_types.length === 0
                  }
                  onClick={() => {
                    if (lineageNodeName !== null) {
                      setLineageNodeName(null);
                    } else {
                      const target =
                        selectedNode?.name ?? workingDoc.node_types[0]?.name;
                      if (target) {
                        setLineageNodeName(target);
                      }
                    }
                  }}
                >
                  {lineageNodeName !== null ? (
                    <>
                      <ArrowLeft className="h-3.5 w-3.5" />
                      {tx("返回画布", "Canvas")}
                    </>
                  ) : (
                    <>
                      <GitBranch className="h-3.5 w-3.5 text-xblue" />
                      {tx("血缘视图", "Lineage")}
                    </>
                  )}
                </Button>

                <Button
                  variant="outline"
                  data-testid="open-spec-doc-btn"
                  onClick={() => setSpecOverviewOpen(true)}
                >
                  <BookOpen className="h-3.5 w-3.5 text-xblue" />
                  {tx("模型规格书", "Spec Doc")}
                </Button>

                <Button
                  variant="outline"
                  data-testid="open-version-panel-btn"
                  onClick={() => setVersionPanelOpen(true)}
                >
                  <History className="h-3.5 w-3.5" />
                  {tx("版本历史", "Versions")}
                </Button>

                <Button
                  variant="outline"
                  data-testid="open-domain-drawer-btn"
                  onClick={() => setDomainDrawerOpen(true)}
                >
                  <Code2 className="h-3.5 w-3.5 text-xblue" />
                  {tx("领域接口", "Domain API")}
                </Button>

                <Button
                  variant="outline"
                  data-testid="studio-package-app-btn"
                  onClick={() => {
                    setAppDrawerFilterNode(null);
                    setAppDrawerOpen(true);
                  }}
                >
                  <Blocks className="h-3.5 w-3.5 text-xblue" />
                  {tx("应用包装", "Package App")}
                </Button>

                {canEdit && (
                  <Button
                    variant="outline"
                    data-testid="save-draft-btn"
                    disabled={busyOp !== null}
                    onClick={() => void handleSaveDraft()}
                  >
                    <Save className="h-3.5 w-3.5" />
                    {busyOp === "save-draft"
                      ? tx("保存中…", "Saving…")
                      : tx("保存草案", "Save Draft")}
                  </Button>
                )}

                {canPublish && (
                  <Button
                    variant="primary"
                    data-testid="open-publish-modal-btn"
                    disabled={busyOp !== null || errorCount > 0}
                    onClick={() => setPublishModalOpen(true)}
                  >
                    <UploadCloud className="h-3.5 w-3.5" />
                    {busyOp === "publish"
                      ? tx("发布中…", "Publishing…")
                      : tx("发布版本", "Publish")}
                  </Button>
                )}
              </>
            )}
          </>
        }
      />

      {studioMode === "explorer" ? (
        <XBlueGraphPage embedded />
      ) : (
        <>

      {/* Condensed Single-Row Subgraph & Draft Context Bar */}
      <StudioSubgraphBar
        subgraphs={subgraphs}
        activeSubgraph={activeSubgraph}
        activeDraft={activeDraft}
        drafts={drafts}
        draftTitle={draftTitle}
        baseVersion={baseVersion}
        publishedVersion={publishedVersion}
        runtimeNodeCount={runtimeNodes.length}
        isDirty={isDirty}
        busyOp={busyOp}
        mainNodeTypes={mainDoc.node_types}
        onSelectSubgraph={(slug) => void handleSelectSubgraph(slug)}
        onSelectDraft={handleSelectDraft}
        onDiscardDraft={() => void handleDiscardActiveDraft()}
        onCreateSubgraph={handleCreateSubgraph}
        onDeleteSubgraph={handleDeleteSubgraph}
        onDryRunSubgraph={handleDryRunSubgraph}
        onResetSubgraphNodes={handleResetSubgraphNodes}
        onOpenMergeDialog={() => void openMergeDialog()}
      />

      {/* 409 VERSION_CONFLICT Alert */}
      {conflictBanner && (
        <div
          data-testid="version-conflict-banner"
          className="shrink-0 rounded-md border border-amber-500/50 bg-amber-500/10 px-3 py-2 flex flex-wrap items-center justify-between gap-2 text-xs"
        >
          <div className="flex items-center gap-2 text-amber-800 dark:text-amber-300">
            <AlertTriangle className="h-4 w-4 shrink-0" />
            <span>{conflictBanner}</span>
          </div>
          <div className="flex items-center gap-2">
            <Button
              variant="outline"
              size="sm"
              onClick={() => setVersionPanelOpen(true)}
            >
              <GitCompare className="h-3.5 w-3.5" />
              {tx("查看差异", "View Diff")}
            </Button>
            <Button
              size="sm"
              onClick={() => void loadSubgraphWorkspace(activeSubgraph)}
            >
              <RefreshCw className="h-3.5 w-3.5" />
              {tx("重载最新", "Reload Latest")}
            </Button>
          </div>
        </div>
      )}

      {/* 409 SCHEMA_BREAKING_CHANGE Alert */}
      {breakingBanner && (
        <div
          data-testid="breaking-change-banner"
          className="shrink-0 rounded-md border border-red-500/50 bg-red-500/10 px-3 py-2 space-y-2 text-xs"
        >
          <div className="flex items-center gap-2 text-red-700 dark:text-red-300 font-medium">
            <AlertTriangle className="h-4 w-4 shrink-0" />
            <span>{breakingBanner}</span>
          </div>
          <div className="flex flex-wrap items-center justify-between gap-3">
            <div className="flex flex-wrap items-center gap-4">
              <div className="inline-flex items-center gap-2">
                <Switch
                  data-testid="force-publish-checkbox"
                  checked={forcePublish}
                  onCheckedChange={setForcePublish}
                  aria-label={tx("确认强制发布破坏性变更", "Confirm force publish")}
                />
                <span>
                  {tx("确认强制发布破坏性变更", "Confirm force publish")}
                </span>
              </div>
              {activeSubgraph === "main" && (
                <div className="inline-flex items-center gap-2">
                  <Switch
                    checked={cascadeDelete}
                    onCheckedChange={setCascadeDelete}
                    aria-label={tx(
                      "级联删除已移除类型的运行时实例",
                      "Cascade delete runtime instances"
                    )}
                  />
                  <span>
                    {tx(
                      "级联删除已移除类型的运行时实例",
                      "Cascade delete runtime instances"
                    )}
                  </span>
                </div>
              )}
            </div>
            <Button
              variant="danger"
              size="sm"
              data-testid="confirm-force-publish-btn"
              disabled={!canPublish || !forcePublish || busyOp !== null}
              onClick={() => void executePublish(true)}
            >
              {tx("强制发布", "Force Publish")}
            </Button>
          </div>
        </div>
      )}

      {/* Operational Status Banner */}
      {statusBanner && (
        <Card
          className="shrink-0 px-3 py-1.5 text-xs font-mono-tech flex items-center justify-between"
          data-testid="studio-status-toast"
        >
          <span>{statusBanner}</span>
          <button
            type="button"
            onClick={() => setStatusBanner(null)}
            className="text-muted-foreground hover:text-foreground cursor-pointer"
          >
            {tx("关闭", "Close")}
          </button>
        </Card>
      )}

      {/* Resizable 100vh Master IDE Split: Canvas/Lineage + Inspector + Collapsible Copilot */}
      <div className="flex flex-1 min-h-0 gap-2 overflow-hidden">
        <div className="flex-1 min-w-0 min-h-0 flex">
          <ResizableWorkspace
            storageKey="axpoint.layout.studio"
            panes={[
              {
                id: "canvas",
                defaultSize: 68,
                minSize: 35,
                handleTestId: "resizable-handle-right",
                className: "pr-1",
                children: lineageNodeName ? (
                  <NodeLineageCanvas
                    subgraphSlug={activeSubgraph}
                    nodeType={lineageNodeName}
                    activeDraftId={activeDraft?.id ?? null}
                    workingDoc={workingDoc}
                    onSelectNodeType={(nextType) => {
                      setSelectedNodeName(nextType);
                      setLineageNodeName(nextType);
                    }}
                    onBackToCanvas={() => setLineageNodeName(null)}
                  />
                ) : (
                  <StudioCanvas
                    nodeTypes={workingDoc.node_types}
                    edgeTypes={workingDoc.edge_types}
                    workingDoc={workingDoc}
                    datasetNames={datasets.map((d) => d.name)}
                    issues={issues}
                    consumptionByNodeType={consumptionByNodeType}
                    selectedNodeName={
                      inspectorTab === "edge"
                        ? null
                        : (selectedNode?.name ?? null)
                    }
                    selectedEdgeIndex={
                      inspectorTab === "edge"
                        ? effectiveEdgeIndex
                        : selectedEdgeIndex
                    }
                    selectedMappingIndex={selectedMappingIndex}
                    canUndo={canvasHistory.canUndo}
                    canRedo={canvasHistory.canRedo}
                    onUndo={() => {
                      canvasHistory.undo();
                    }}
                    onRedo={() => {
                      canvasHistory.redo();
                    }}
                    onSelectNode={(name) => {
                      if (name) {
                        setSelectedNodeName(name);
                        setSelectedEdgeIndex(null);
                        if (
                          inspectorTab === "edge" ||
                          inspectorTab === "mappings"
                        ) {
                          setInspectorTab("basic");
                        }
                      }
                    }}
                    onSelectEdge={(edgeIdx) => {
                      setSelectedEdgeIndex(edgeIdx);
                      if (edgeIdx !== null) {
                        setInspectorTab("edge");
                      }
                    }}
                    onSelectMapping={(mIdx) => {
                      setSelectedMappingIndex(mIdx);
                      setInspectorTab("mappings");
                    }}
                    onCreateMapping={(mapping) => {
                      const nextIdx = workingDoc.mappings.length;
                      mutateDoc((prev) => ({
                        ...prev,
                        mappings: [...prev.mappings, mapping],
                      }));
                      setSelectedMappingIndex(nextIdx);
                      setInspectorTab("mappings");
                    }}
                    onUpdateNodePosition={(name, posX, posY) => {
                      mutateDoc((prev) => ({
                        ...prev,
                        node_types: prev.node_types.map((nt) =>
                          nt.name === name
                            ? { ...nt, pos_x: posX, pos_y: posY }
                            : nt
                        ),
                      }));
                    }}
                    onApplyLayoutPositions={(positions) => {
                      mutateDoc((prev) => ({
                        ...prev,
                        node_types: prev.node_types.map((nt) => {
                          const pos = positions[nt.name];
                          return pos
                            ? { ...nt, pos_x: pos.x, pos_y: pos.y }
                            : nt;
                        }),
                      }));
                    }}
                    onCreateNodeType={handleCreateNodeType}
                    onCreateEdgeType={handleCreateEdgeType}
                    onConfirmNode={(name) => void handleConfirmNodeProposal(name)}
                    onOpenLineage={(name) => {
                      setSelectedNodeName(name);
                      setLineageNodeName(name);
                    }}
                    onOpenAppPackaging={(name) => {
                      setAppDrawerFilterNode(name);
                      setAppDrawerOpen(true);
                    }}
                  />
                ),
              },
              {
                id: "inspector",
                defaultSize: 32,
                minSize: 20,
                collapsible: true,
                className: "pl-1",
                children: (
                  <Card className="w-full h-full min-h-0 p-3 flex flex-col gap-2.5 overflow-hidden">
                    <Tabs<InspectorTab>
                      className="shrink-0"
                      value={inspectorTab}
                      onChange={(tab) => {
                        setInspectorTab(tab);
                        if (
                          tab === "edge" &&
                          selectedEdgeIndex === null &&
                          workingDoc.edge_types.length > 0
                        ) {
                          setSelectedEdgeIndex(0);
                        }
                      }}
                      items={[
                        {
                          key: "basic",
                          label: tx("基础信息", "General"),
                          testId: "inspector-tab-basic",
                        },
                        {
                          key: "properties",
                          label: tx("属性指标", "Fields"),
                          testId: "inspector-tab-properties",
                        },
                        {
                          key: "rules",
                          label: tx("聚合派生", "Rules"),
                          testId: "inspector-tab-rules",
                        },
                        {
                          key: "edge",
                          label: `${tx("关系", "Relations")} (${workingDoc.edge_types.length})`,
                          testId: "inspector-tab-edge",
                        },
                        {
                          key: "mappings",
                          label: `${tx("数据映射", "Mappings")} (${workingDoc.mappings.length})`,
                          testId: "inspector-tab-mappings",
                        },
                      ]}
                    />

                    {(inspectorTab === "basic" ||
                      inspectorTab === "properties" ||
                      inspectorTab === "rules") &&
                      workingDoc.node_types.length > 0 && (
                        <div className="shrink-0 flex items-center gap-2 rounded-md bg-muted/35 px-2.5 py-1.5 text-xs">
                          <span className="text-[11px] text-muted-foreground shrink-0">
                            {tx("当前节点", "Active Node")}
                          </span>
                          <Select
                            value={selectedNode?.name ?? ""}
                            onChange={(e) => setSelectedNodeName(e.target.value)}
                            options={workingDoc.node_types.map((nt) => ({
                              value: nt.name,
                              label: `${nt.display_name || nt.name} (${nt.name})`,
                              text: `${nt.display_name} ${nt.name}`,
                              icon: renderNodeIcon(nt.icon),
                            }))}
                            className="h-7 flex-1 min-w-0 font-mono-tech"
                          />
                        </div>
                      )}

                    <div className="flex-1 min-h-0 overflow-y-auto pr-0.5">
                      {inspectorTab === "basic" &&
                        (selectedNode ? (
                          <NodeTypeCard
                            nodeType={selectedNode}
                            consumption={
                              consumptionByNodeType[selectedNode.name] ?? null
                            }
                            onChange={handleUpdateSelectedNode}
                            onDelete={handleDeleteSelectedNode}
                            onConfirmNode={(name) =>
                              void handleConfirmNodeProposal(name)
                            }
                            onOpenLineage={(name) => {
                              setSelectedNodeName(name);
                              setLineageNodeName(name);
                            }}
                            onOpenAppPackaging={(name) => {
                              setAppDrawerFilterNode(name);
                              setAppDrawerOpen(true);
                            }}
                          />
                        ) : (
                          <EmptyState
                            title={tx("未选中节点类型", "No node type selected")}
                          />
                        ))}

                      {inspectorTab === "properties" &&
                        (selectedNode ? (
                          <PropertyTable
                            properties={selectedNode.properties}
                            onChange={(nextProps) =>
                              handleUpdateSelectedNode({
                                ...selectedNode,
                                properties: nextProps,
                              })
                            }
                          />
                        ) : (
                          <EmptyState
                            title={tx("未选中节点类型", "No node type selected")}
                          />
                        ))}

                      {inspectorTab === "rules" &&
                        (selectedNode ? (
                          <RuleEditor
                            nodeIndex={selectedNodeIndex}
                            nodeType={selectedNode}
                            allNodeTypes={availableNodeTypesForEdges}
                            edgeTypes={workingDoc.edge_types}
                            issues={issues}
                            subgraph={activeSubgraph}
                            onChangeAggRules={(nextAgg) =>
                              handleUpdateSelectedNode({
                                ...selectedNode,
                                agg_rules: nextAgg,
                              })
                            }
                            onChangeDerivedRules={(nextDerived) =>
                              handleUpdateSelectedNode({
                                ...selectedNode,
                                derived_rules: nextDerived,
                              })
                            }
                          />
                        ) : (
                          <EmptyState
                            title={tx("未选中节点类型", "No node type selected")}
                          />
                        ))}

                      {inspectorTab === "edge" && (
                        <div className="space-y-3">
                          <div className="flex items-center gap-2">
                            {workingDoc.edge_types.length > 0 ? (
                              <Select
                                value={effectiveEdgeIndex ?? 0}
                                onChange={(e) =>
                                  setSelectedEdgeIndex(Number(e.target.value))
                                }
                                className="h-8 flex-1 min-w-0 font-mono-tech"
                              >
                                {workingDoc.edge_types.map((et, idx) => (
                                  <option key={`${et.name}-${idx}`} value={idx}>
                                    {et.src_type_name} → {et.name} →{" "}
                                    {et.dst_type_name}
                                  </option>
                                ))}
                              </Select>
                            ) : (
                              <span className="text-xs text-muted-foreground flex-1">
                                {tx("当前全图尚无关系连线", "No relations defined")}
                              </span>
                            )}
                            {canEdit && (
                              <Button
                                variant="outline"
                                size="sm"
                                onClick={handleInlineAddEdge}
                                className="shrink-0"
                              >
                                <Plus className="h-3 w-3" />
                                {tx("新建关系", "New")}
                              </Button>
                            )}
                          </div>

                          {selectedEdge && effectiveEdgeIndex !== null ? (
                            <EdgeTypeEditor
                              edgeType={selectedEdge}
                              availableNodeTypes={availableNodeTypesForEdges}
                              onChange={(updatedEdge) =>
                                mutateDoc((prev) => ({
                                  ...prev,
                                  edge_types: prev.edge_types.map((et, i) =>
                                    i === effectiveEdgeIndex ? updatedEdge : et
                                  ),
                                }))
                              }
                              onDelete={() => {
                                const rmIdx = effectiveEdgeIndex;
                                const remaining =
                                  workingDoc.edge_types.length - 1;
                                mutateDoc((prev) => ({
                                  ...prev,
                                  edge_types: prev.edge_types.filter(
                                    (_, i) => i !== rmIdx
                                  ),
                                }));
                                setSelectedEdgeIndex(remaining > 0 ? 0 : null);
                              }}
                            />
                          ) : (
                            <EmptyState
                              title={tx("暂无关系连线定义", "No relation defined")}
                            />
                          )}
                        </div>
                      )}

                      {inspectorTab === "mappings" && (
                        <MappingEditor
                          activeSubgraph={activeSubgraph}
                          mappings={workingDoc.mappings}
                          persistedMappings={persistedMappings}
                          datasets={datasets}
                          nodeTypes={workingDoc.node_types}
                          edgeTypes={workingDoc.edge_types}
                          selectedMappingIndex={selectedMappingIndex}
                          onSelectMappingIndex={setSelectedMappingIndex}
                          onChangeMappings={(nextMappings: MappingSpec[]) =>
                            mutateDoc((prev) => ({
                              ...prev,
                              mappings: nextMappings,
                            }))
                          }
                          onRefreshPersistedMappings={async () => {
                            const latest = await xblueApi
                              .listMappings(activeSubgraph)
                              .catch(() => [] as MappingRecord[]);
                            setPersistedMappings(latest);
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

        {/* Right-Hand Collapsible Resident Copilot Sidebar */}
        <StudioCopilotPanel
          collapsed={copilotCollapsed}
          onToggleCollapsed={() => setCopilotCollapsed((v) => !v)}
          activeDraft={activeDraft}
          selectedNodeName={
            inspectorTab === "edge" || inspectorTab === "mappings"
              ? null
              : (selectedNode?.name ?? null)
          }
          selectedEdgeName={
            inspectorTab === "edge"
              ? (selectedEdge?.name ?? null)
              : selectedEdgeIndex !== null
                ? (workingDoc.edge_types[selectedEdgeIndex]?.name ?? null)
                : null
          }
          activeInspectorTab={inspectorTab}
          datasets={datasets}
          onEnsureDraft={ensureActiveDraft}
          onDraftUpdated={handleApplyUpdatedDraft}
          onDiagnosticsUpdated={(nextPass, nextIssues) => {
            setServerPassCount(nextPass);
            setIssues(nextIssues);
          }}
          onLocateTargetPath={handleLocateTargetPath}
        />
      </div>

      {/* Persistent Sticky Bottom IDE Status Bar & Collapsible Diagnostics / Snapshots Dock */}
      <Card
        className="shrink-0 overflow-hidden"
        data-testid="studio-bottom-dock"
      >
        <div className="flex items-center justify-between gap-2 bg-muted/25 px-3 py-1">
          <div className="flex flex-wrap items-center gap-2" role="tablist">
            <button
              type="button"
              role="tab"
              aria-selected={dockTab === "issues" && !dockCollapsed}
              onClick={() => {
                if (dockTab === "issues") {
                  setDockCollapsed((v) => !v);
                } else {
                  setDockTab("issues");
                  setDockCollapsed(false);
                }
              }}
              className={cn(
                "inline-flex items-center gap-1.5 rounded px-2 py-0.5 text-xs font-medium transition-colors cursor-pointer",
                dockTab === "issues" && !dockCollapsed
                  ? "bg-xblue-soft text-xblue"
                  : "text-muted-foreground hover:text-foreground"
              )}
            >
              <ShieldAlert className="h-3.5 w-3.5 text-xblue" />
              <span>{tx("校验诊断", "Diagnostics")}</span>
              <Badge
                variant="success"
                data-testid="dock-pass-count-badge"
                className="tabular-nums"
              >
                <CheckCircle2 className="h-3 w-3 mr-0.5" />
                {tx(`${passCount} 项通过`, `${passCount} Passed`)}
              </Badge>
              {issues.length > 0 && (
                <Badge variant={errorCount > 0 ? "danger" : "warning"}>
                  {issues.length}
                </Badge>
              )}
              {unconfirmedCount > 0 && (
                <Badge
                  variant="warning"
                  data-testid="dock-proposal-count-badge"
                  className="tabular-nums"
                >
                  {tx(`${unconfirmedCount} 待确认`, `${unconfirmedCount} Proposed`)}
                </Badge>
              )}
            </button>

            {unconfirmedCount > 0 && canEdit && dockCollapsed && (
              <Button
                type="button"
                variant="outline"
                size="sm"
                data-testid="dock-confirm-all-btn"
                disabled={busyOp !== null}
                onClick={() => void handleConfirmAllProposals()}
                className="h-6 px-2 text-[11px] border-amber-500/50 text-amber-700 dark:text-amber-300"
              >
                <CheckCheck className="h-3 w-3" />
                {tx("全部确认", "Confirm All")}
              </Button>
            )}

            <span className="text-border">|</span>

            <button
              type="button"
              role="tab"
              aria-selected={dockTab === "nodes" && !dockCollapsed}
              onClick={() => {
                if (dockTab === "nodes") {
                  setDockCollapsed((v) => !v);
                } else {
                  setDockTab("nodes");
                  setDockCollapsed(false);
                }
              }}
              className={cn(
                "inline-flex items-center gap-1.5 rounded px-2 py-0.5 text-xs font-medium transition-colors cursor-pointer",
                dockTab === "nodes" && !dockCollapsed
                  ? "bg-xblue-soft text-xblue"
                  : "text-muted-foreground hover:text-foreground"
              )}
            >
              <Database className="h-3.5 w-3.5 text-xblue" />
              <span>{tx("运行时节点快照", "Runtime Snapshots")}</span>
              <span className="font-mono-tech text-[11px] text-foreground font-semibold tabular-nums">
                {runtimeNodes.length}
              </span>
            </button>
          </div>

          <Button
            type="button"
            variant="ghost"
            size="sm"
            className="h-6 px-2 text-[11px]"
            onClick={() => setDockCollapsed((v) => !v)}
          >
            {dockCollapsed ? (
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

        {!dockCollapsed && (
          <div className="border-t border-border p-2.5 max-h-48 overflow-y-auto">
            {dockTab === "issues" ? (
              <IssueList
                issues={issues}
                validating={validating}
                passCount={passCount}
                unconfirmedCount={unconfirmedCount}
                confirmingAll={busyOp === "confirm-proposals"}
                embedded
                onLocateIssue={handleLocateIssue}
                onConfirmAllProposals={() => void handleConfirmAllProposals()}
              />
            ) : runtimeNodes.length === 0 ? (
              <div className="py-4 text-center text-xs text-muted-foreground">
                {tx("暂无已注册运行时节点实例", "No registered runtime nodes")}
              </div>
            ) : (
              <div className="grid grid-cols-1 md:grid-cols-2 xl:grid-cols-4 gap-2 text-xs">
                {runtimeNodes.slice(0, 16).map((node) => {
                  const metricPairs = Object.entries(node.metrics ?? {});
                  const propPairs = Object.entries(
                    node.properties ?? {}
                  ).filter(([k]) => !k.startsWith("__"));
                  return (
                    <div
                      key={node.id}
                      className="rounded border border-border bg-background p-2 space-y-1.5"
                    >
                      <div className="flex items-center justify-between gap-1.5">
                        <div className="min-w-0 flex items-center gap-1.5">
                          <span className="font-medium truncate">
                            {node.title}
                          </span>
                          <span className="font-mono-tech text-[11px] text-muted-foreground shrink-0">
                            ({node.node_key})
                          </span>
                        </div>
                        <Badge variant="xblue">{node.type_name}</Badge>
                      </div>

                      <div className="flex flex-wrap gap-1">
                        {metricPairs.map(([mk, mv]) => (
                          <Badge key={`m-${mk}`} variant="neutral">
                            {mk}: {fmtNum(mv, 2)}
                          </Badge>
                        ))}
                        {propPairs.slice(0, 3).map(([pk, pv]) => (
                          <Badge key={`p-${pk}`} variant="outline">
                            {pk}: {String(pv ?? "")}
                          </Badge>
                        ))}
                        {metricPairs.length === 0 && propPairs.length === 0 && (
                          <span className="text-[11px] text-muted-foreground">
                            —
                          </span>
                        )}
                      </div>
                    </div>
                  );
                })}
              </div>
            )}
          </div>
        )}
      </Card>

      {/* Publish Schema Release Modal (Replaces cluttered header summary input) */}
      <Modal
        open={publishModalOpen}
        onClose={() => setPublishModalOpen(false)}
        maxWidthClass="max-w-md"
        title={
          <>
            <UploadCloud className="h-4 w-4 text-xblue shrink-0" />
            <span>
              {tx("发布模型版本", "Publish Schema Release")} · {activeSubgraph}
            </span>
          </>
        }
        footer={
          <>
            <Button
              variant="outline"
              onClick={() => setPublishModalOpen(false)}
              disabled={busyOp === "publish"}
            >
              {tx("取消", "Cancel")}
            </Button>
            <Button
              variant="primary"
              data-testid="publish-schema-btn"
              disabled={busyOp !== null || errorCount > 0}
              onClick={() => void executePublish()}
            >
              <UploadCloud className="h-3.5 w-3.5" />
              {busyOp === "publish"
                ? tx("发布中…", "Publishing…")
                : tx("确认发布", "Publish")}
            </Button>
          </>
        }
      >
        <div className="space-y-3 text-xs">
          <div className="flex items-center justify-between rounded-md bg-muted/40 px-3 py-2">
            <span className="text-muted-foreground">
              {tx("目标子图与基线", "Subgraph & Base")}
            </span>
            <span className="font-mono-tech font-medium">
              {activeSubgraph} · v{baseVersion} → v{publishedVersion + 1}
            </span>
          </div>

          {unconfirmedCount > 0 && (
            <div className="flex items-center justify-between gap-2 rounded-md border border-amber-500/50 bg-amber-500/10 px-3 py-2">
              <span className="text-amber-800 dark:text-amber-300">
                {tx(
                  `含 ${unconfirmedCount} 项待确认提议`,
                  `${unconfirmedCount} proposed items pending sign-off`
                )}
              </span>
              {canEdit && (
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  disabled={busyOp !== null}
                  onClick={() => void handleConfirmAllProposals()}
                >
                  <CheckCheck className="h-3.5 w-3.5" />
                  {tx("全部确认", "Confirm All")}
                </Button>
              )}
            </div>
          )}

          <div>
            <label className="block font-medium mb-1">
              {tx("草案标题", "Draft Title")}
            </label>
            <Input
              value={draftTitle}
              onChange={(e) => setDraftTitle(e.target.value)}
              placeholder={tx("供应链建模演进草案", "Supply chain schema draft")}
              className="h-8 text-xs"
            />
          </div>

          <div>
            <label className="block font-medium mb-1">
              {tx("版本发布摘要", "Release Summary")}
            </label>
            <Textarea
              rows={3}
              data-testid="publish-summary-input"
              value={changeSummary}
              onChange={(e) => setChangeSummary(e.target.value)}
              placeholder={tx(
                "补充供应商节点与库存履约派生规则",
                "Add Supplier node type and fulfillment derived rules"
              )}
              className="text-xs"
            />
          </div>
        </div>
      </Modal>

      {/* Drawers & Modals */}
      <ModelSpecOverview
        open={specOverviewOpen}
        subgraphSlug={activeSubgraph}
        subgraphDisplayName={activeSubgraphInfo?.display_name ?? activeSubgraph}
        publishedVersion={publishedVersion}
        activeDraftId={activeDraft?.id ?? null}
        workingDoc={workingDoc}
        onClose={() => setSpecOverviewOpen(false)}
      />

      <VersionPanel
        open={versionPanelOpen}
        subgraphSlug={activeSubgraph}
        versions={versions}
        onClose={() => setVersionPanelOpen(false)}
        onRestoreVersion={handleRestoreVersion}
      />

      <DomainApiDrawer
        open={domainDrawerOpen}
        fallbackMainDoc={mainDoc}
        mainVersion={mainVersion}
        onClose={() => setDomainDrawerOpen(false)}
      />

      <StudioAppDrawer
        open={appDrawerOpen}
        subgraphSlug={activeSubgraph}
        isDevSandbox={Boolean(activeSubgraphInfo?.is_dev_sandbox)}
        nodeTypes={workingDoc.node_types}
        consumption={consumption}
        filterNodeType={appDrawerFilterNode}
        onChangeFilterNodeType={setAppDrawerFilterNode}
        onRefreshConsumption={refreshConsumption}
        onClose={() => setAppDrawerOpen(false)}
      />

      <AiDraftWizard
        open={aiWizardOpen}
        subgraphSlug={activeSubgraph}
        datasets={datasets}
        docNodes={docNodes}
        onClose={() => setAiWizardOpen(false)}
        onGenerate={handleGenerateAiDraft}
      />

      <SubgraphMergeDialog
        open={mergeDialogOpen}
        subgraphSlug={activeSubgraph}
        subgraphDisplayName={activeSubgraphInfo?.display_name ?? activeSubgraph}
        preview={mergePreview}
        loading={mergeLoading}
        merging={merging}
        error={mergeError}
        onClose={() => setMergeDialogOpen(false)}
        onRefreshPreview={refreshMergePreview}
        onExecuteMerge={executeSubgraphMerge}
      />

      <KnowledgeCandidatesModal
        open={knowledgeCandidatesOpen}
        mainNodeTypes={mainDoc.node_types}
        onClose={() => setKnowledgeCandidatesOpen(false)}
        onCandidateAccepted={(res, cand) => {
          setStatusBanner(
            tx(
              `已从超图实体「${cand.name}」确认建点：${res.node.title || cand.name} (${res.node.node_key})`,
              `Created node ${res.node.title || cand.name} (${res.node.node_key}) from entity "${cand.name}"`
            )
          );
          if (res.node.id || res.node.node_key) {
            setRuntimeNodes((prev) => {
              const filtered = prev.filter(
                (n) =>
                  !(
                    n.type_name === res.node.type_name &&
                    n.node_key === res.node.node_key
                  )
              );
              return [res.node, ...filtered];
            });
          }
          void xblueApi
            .listNodes({ subgraph: activeSubgraph })
            .then((snap) => {
              const resolved = Array.isArray(snap?.nodes)
                ? snap.nodes
                : Array.isArray(snap?.items)
                  ? snap.items
                  : [];
              if (resolved.length > 0) {
                setRuntimeNodes(resolved);
              }
            })
            .catch(() => {});
        }}
      />
        </>
      )}
    </div>
  );
}
