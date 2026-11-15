import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  AlertTriangle,
  Code2,
  FilePlus2,
  GitCompare,
  History,
  RefreshCw,
  Save,
  Sparkles,
  Trash2,
  UploadCloud,
} from "lucide-react";
import { ApiError } from "@/lib/api/client";
import { axhubApi, type Dataset } from "@/lib/api/axhub";
import { api, type TopologyNode } from "@/lib/api/rag";
import {
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
  type SubgraphInfo,
  type TypeMergeResolution,
  type ValidationIssue,
} from "@/lib/api/xblue";
import { Badge, Button, Card, Input } from "@/components/ui/primitives";
import { cn } from "@/lib/utils";
import { AiDraftWizard } from "@/components/xbluestudio/AiDraftWizard";
import { DomainApiDrawer } from "@/components/xbluestudio/DomainApiDrawer";
import { EdgeTypeEditor } from "@/components/xbluestudio/EdgeTypeEditor";
import { IssueList } from "@/components/xbluestudio/IssueList";
import { MappingEditor } from "@/components/xbluestudio/MappingEditor";
import { NodeTypeCard } from "@/components/xbluestudio/NodeTypeCard";
import { PropertyTable } from "@/components/xbluestudio/PropertyTable";
import { RuleEditor } from "@/components/xbluestudio/RuleEditor";
import { StudioCanvas } from "@/components/xbluestudio/StudioCanvas";
import { StudioSubgraphBar } from "@/components/xbluestudio/StudioSubgraphBar";
import { SubgraphMergeDialog } from "@/components/xbluestudio/SubgraphMergeDialog";
import { VersionPanel } from "@/components/xbluestudio/VersionPanel";

type InspectorTab = "basic" | "properties" | "rules" | "edge" | "mappings";

const EMPTY_DOC: SchemaDoc = {
  node_types: [],
  edge_types: [],
  mappings: [],
};

export function XBlueStudioPage() {
  const [subgraphs, setSubgraphs] = useState<SubgraphInfo[]>([]);
  const [activeSubgraph, setActiveSubgraph] = useState<string>("main");
  const [mainDoc, setMainDoc] = useState<SchemaDoc>(EMPTY_DOC);
  const [mainVersion, setMainVersion] = useState<number>(1);

  const [publishedVersion, setPublishedVersion] = useState<number>(1);
  const [baseVersion, setBaseVersion] = useState<number>(1);
  const [workingDoc, setWorkingDoc] = useState<SchemaDoc>(EMPTY_DOC);
  const [isDirty, setIsDirty] = useState<boolean>(false);

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
  const [validating, setValidating] = useState<boolean>(false);

  const [selectedNodeName, setSelectedNodeName] = useState<string | null>(null);
  const [selectedEdgeIndex, setSelectedEdgeIndex] = useState<number | null>(
    null
  );
  const [selectedMappingIndex, setSelectedMappingIndex] = useState<
    number | null
  >(null);
  const [inspectorTab, setInspectorTab] = useState<InspectorTab>("basic");

  const [statusBanner, setStatusBanner] = useState<string | null>(null);
  const [conflictBanner, setConflictBanner] = useState<string | null>(null);
  const [breakingBanner, setBreakingBanner] = useState<string | null>(null);
  const [forcePublish, setForcePublish] = useState<boolean>(false);
  const [cascadeDelete, setCascadeDelete] = useState<boolean>(false);
  const [busyOp, setBusyOp] = useState<string | null>(null);

  const [versionPanelOpen, setVersionPanelOpen] = useState<boolean>(false);
  const [domainDrawerOpen, setDomainDrawerOpen] = useState<boolean>(false);
  const [aiWizardOpen, setAiWizardOpen] = useState<boolean>(false);

  const [mergeDialogOpen, setMergeDialogOpen] = useState<boolean>(false);
  const [mergePreview, setMergePreview] = useState<MergePreview | null>(null);
  const [mergeLoading, setMergeLoading] = useState<boolean>(false);
  const [merging, setMerging] = useState<boolean>(false);
  const [mergeError, setMergeError] = useState<string | null>(null);

  const skipNextValidationRef = useRef<boolean>(true);

  const loadSubgraphWorkspace = useCallback(async (slug: string) => {
    skipNextValidationRef.current = true;
    const [
      sgList,
      pubSchema,
      verList,
      draftList,
      mapList,
      mainSchemaOpt,
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
      slug === "main"
        ? Promise.resolve(null)
        : xblueApi.getPublishedSchema("main").catch(() => null),
    ]);

    setSubgraphs(sgList);
    setVersions(verList);
    setPersistedMappings(mapList);
    setPublishedVersion(pubSchema.version);

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

    if (openDrafts.length > 0 && openDrafts[0]) {
      const latestDraft = openDrafts[0];
      setActiveDraft(latestDraft);
      setDraftTitle(latestDraft.title);
      setBaseVersion(latestDraft.base_version);
      setWorkingDoc(latestDraft.schema_doc);
      setIssues(latestDraft.issues);
      setIsDirty(false);
      setSelectedNodeName(latestDraft.schema_doc.node_types[0]?.name ?? null);
    } else {
      setActiveDraft(null);
      setDraftTitle(`${slug} 模型演进草案`);
      setBaseVersion(pubSchema.version);
      setWorkingDoc(schemaDoc);
      setIssues([]);
      setIsDirty(false);
      setSelectedNodeName(schemaDoc.node_types[0]?.name ?? null);
    }

    setSelectedEdgeIndex(null);
    setSelectedMappingIndex(null);
    setConflictBanner(null);
    setBreakingBanner(null);
  }, []);

  useEffect(() => {
    let cancelled = false;
    const bootstrap = async () => {
      await loadSubgraphWorkspace("main");
      const [nodeSnap, dsResp, topoResp] = await Promise.all([
        xblueApi
          .getNodes()
          .catch(() => ({ nodes: [] as BlueNode[], links: [], items: [] as BlueNode[], next_cursor: null })),
        axhubApi
          .listDatasets({ limit: 100 })
          .catch(() => ({ items: [] as Dataset[], next_cursor: null })),
        api
          .topology()
          .catch(() => ({ nodes: [] as TopologyNode[], edges: [] })),
      ]);
      if (cancelled) return;
      const resolvedNodes = Array.isArray(nodeSnap?.nodes)
        ? nodeSnap.nodes
        : Array.isArray(nodeSnap?.items)
          ? nodeSnap.items
          : [];
      setRuntimeNodes(resolvedNodes);
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
    setWorkingDoc((prev) => updater(prev));
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

  const selectedNodeIndex = useMemo(
    () =>
      selectedNodeName
        ? workingDoc.node_types.findIndex((nt) => nt.name === selectedNodeName)
        : -1,
    [workingDoc.node_types, selectedNodeName]
  );

  const selectedNode =
    selectedNodeIndex >= 0
      ? workingDoc.node_types[selectedNodeIndex] ?? null
      : null;

  const selectedEdge =
    selectedEdgeIndex !== null
      ? workingDoc.edge_types[selectedEdgeIndex] ?? null
      : null;

  const handleSelectSubgraph = async (slug: string) => {
    if (slug === activeSubgraph) return;
    setActiveSubgraph(slug);
    setStatusBanner(null);
    await loadSubgraphWorkspace(slug);
  };

  const handleCreateSubgraph = async (req: CreateSubgraphRequest) => {
    const created = await xblueApi.createSubgraph(req);
    setActiveSubgraph(created.slug);
    setStatusBanner(`已创建部门隔离建模子图 ${created.display_name} (${created.slug})`);
    await loadSubgraphWorkspace(created.slug);
  };

  const handleDeleteSubgraph = async (slug: string) => {
    if (slug === "main") return;
    try {
      await xblueApi.deleteSubgraph(slug);
      setActiveSubgraph("main");
      setStatusBanner(`已删除部门子图 ${slug}`);
      await loadSubgraphWorkspace("main");
    } catch (err) {
      setStatusBanner(err instanceof Error ? err.message : "删除子图失败");
    }
  };

  const handleCreateNodeType = (name: string, displayName: string) => {
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
      pos_x: 120 + (idx % 3) * 280,
      pos_y: 120 + Math.floor(idx / 3) * 180,
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
    const nextIdx = workingDoc.edge_types.length;
    mutateDoc((prev) => ({
      ...prev,
      edge_types: [...prev.edge_types, edge],
    }));
    setSelectedNodeName(null);
    setSelectedEdgeIndex(nextIdx);
    setInspectorTab("edge");
  };

  const handleUpdateSelectedNode = (updated: NodeTypeSpec) => {
    if (selectedNodeIndex < 0) return;
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
    if (!selectedNode) return;
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
            title: draftTitle || `${activeSubgraph} 草案`,
            schema_doc: workingDoc,
            base_version: baseVersion,
          },
          activeSubgraph
        );
      }
      setActiveDraft(saved);
      setIssues(saved.issues);
      setIsDirty(false);
      const latestDrafts = await xblueApi
        .listDrafts(activeSubgraph)
        .catch(() => [saved]);
      setDrafts(latestDrafts.filter((d) => d.status === "DRAFT"));
      setStatusBanner(`草案「${saved.title}」已保存 (基于版本 v${saved.base_version})`);
    } catch (err) {
      setStatusBanner(err instanceof Error ? err.message : "保存草案失败");
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
    setIssues(draft.issues);
    setIsDirty(false);
    setSelectedNodeName(draft.schema_doc.node_types[0]?.name ?? null);
    setSelectedEdgeIndex(null);
  };

  const handleDiscardActiveDraft = async () => {
    if (!activeDraft?.id) return;
    setBusyOp("discard-draft");
    try {
      await xblueApi.discardDraft(activeDraft.id);
      setStatusBanner(`已丢弃草案「${activeDraft.title}」`);
      await loadSubgraphWorkspace(activeSubgraph);
    } catch (err) {
      setStatusBanner(err instanceof Error ? err.message : "丢弃草案失败");
    } finally {
      setBusyOp(null);
    }
  };

  const executePublish = async (forceOverride?: boolean) => {
    const errorCount = issues.filter((i) => i.severity === "ERROR").length;
    if (errorCount > 0) {
      setStatusBanner(
        `当前模型存在 ${errorCount} 个 ERROR 级校验问题，请修复后再发布`
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
            title: draftTitle || `${activeSubgraph} 发布草案`,
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
            changeSummary.trim() || `发布 ${activeSubgraph} 模型更新`,
          force: useForce,
          cascade: useForce && cascadeDelete,
        });
        setStatusBanner(
          `子图 ${pubRes.subgraph_slug} 已成功发布新版本 v${pubRes.version}`
        );
      } else {
        const directRes = await xblueApi.saveSchemaDirect(
          {
            ...workingDoc,
            base_version: baseVersion,
            change_summary:
              changeSummary.trim() || `发布 ${activeSubgraph} 模型更新`,
            force: useForce,
            cascade: useForce && cascadeDelete,
          },
          activeSubgraph
        );
        setStatusBanner(
          `子图 ${directRes.subgraph_slug} 已发布版本 v${directRes.version}`
        );
      }

      setChangeSummary("");
      setForcePublish(false);
      setCascadeDelete(false);
      await loadSubgraphWorkspace(activeSubgraph);
    } catch (err) {
      if (err instanceof ApiError) {
        if (err.status === 409 && err.errorCode === "VERSION_CONFLICT") {
          setConflictBanner(
            err.message ||
              "并发版本冲突 (VERSION_CONFLICT)：当前子图已有更新的已发布版本，请查看差异或基于最新版本重新加载。"
          );
          return;
        }
        if (
          err.status === 409 &&
          err.errorCode === "SCHEMA_BREAKING_CHANGE"
        ) {
          setBreakingBanner(
            err.message ||
              "检测到破坏性模型变更 (SCHEMA_BREAKING_CHANGE)：删除已有数据的节点/关系类型或修改不兼容字段类型需勾选强制发布 (force=true)。"
          );
          return;
        }
      }
      setStatusBanner(err instanceof Error ? err.message : "发布模型版本失败");
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
    setIssues(restored.issues);
    setIsDirty(false);
    setStatusBanner(`已从历史版本 v${v} 恢复为待审阅草案「${restored.title}」`);
  };

  const handleGenerateAiDraft = async (req: AiDraftRequest) => {
    const generated = await xblueApi.createAiDraft(req, activeSubgraph);
    skipNextValidationRef.current = true;
    setActiveDraft(generated);
    setDraftTitle(generated.title);
    setBaseVersion(generated.base_version);
    setWorkingDoc(generated.schema_doc);
    setIssues(generated.issues);
    setIsDirty(false);
    setSelectedNodeName(generated.schema_doc.node_types[0]?.name ?? null);
    const latestDrafts = await xblueApi
      .listDrafts(activeSubgraph)
      .catch(() => [generated]);
    setDrafts(latestDrafts.filter((d) => d.status === "DRAFT"));
    setStatusBanner(
      `AI 草案「${generated.title}」已生成并载入画布，请审阅节点、关系与校验诊断后发布`
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
      setMergeError(err instanceof Error ? err.message : "加载合并预演失败");
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
      setMergeError(err instanceof Error ? err.message : "刷新合并预演失败");
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
        `部门子图 ${activeSubgraph} 已成功对齐合并入企业主图 main (最新主图版本 v${res.version})`
      );
    } catch (err) {
      setMergeError(err instanceof Error ? err.message : "执行子图合并失败");
    } finally {
      setMerging(false);
    }
  };

  const handleLocateIssue = (issue: ValidationIssue) => {
    const nodeMatch = /^node_types\[(\d+)\](?:\.([a-z_]+))?/.exec(issue.path);
    if (nodeMatch) {
      const idx = Number(nodeMatch[1]);
      const subField = nodeMatch[2] ?? "";
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

    const edgeMatch = /^edge_types\[(\d+)\]/.exec(issue.path);
    if (edgeMatch) {
      const eIdx = Number(edgeMatch[1]);
      if (workingDoc.edge_types[eIdx]) {
        setSelectedNodeName(null);
        setSelectedEdgeIndex(eIdx);
        setInspectorTab("edge");
        return;
      }
    }

    const mapMatch = /^mappings\[(\d+)\]/.exec(issue.path);
    if (mapMatch) {
      const mIdx = Number(mapMatch[1]);
      if (workingDoc.mappings[mIdx]) {
        setSelectedMappingIndex(mIdx);
        setInspectorTab("mappings");
      }
    }
  };

  const activeSubgraphInfo = subgraphs.find((s) => s.slug === activeSubgraph);

  return (
    <div className="space-y-4">
      {/* Page Header — h1 strictly preserves smoke.spec.ts assertion */}
      <div className="flex flex-wrap items-center justify-between gap-3 border-b border-border pb-3">
        <div>
          <h1 className="text-lg font-semibold tracking-tight">
            xBlueStudio · 节点与关系建模
          </h1>
          <p className="text-xs text-muted-foreground mt-0.5">
            企业多子图隔离建模、AI 草案推导、动态聚合/派生指标与领域 GraphQL 自动发布工作台
          </p>
        </div>

        <div className="flex flex-wrap items-center gap-1.5">
          <Button
            variant="outline"
            className="h-8 px-2.5 text-xs"
            data-testid="open-ai-wizard-btn"
            onClick={() => setAiWizardOpen(true)}
          >
            <Sparkles className="h-3.5 w-3.5 text-xblue" />
            AI 草案向导
          </Button>

          <Button
            variant="outline"
            className="h-8 px-2.5 text-xs"
            data-testid="open-version-panel-btn"
            onClick={() => setVersionPanelOpen(true)}
          >
            <History className="h-3.5 w-3.5" />
            版本历史
          </Button>

          <Button
            variant="outline"
            className="h-8 px-2.5 text-xs"
            data-testid="open-domain-drawer-btn"
            onClick={() => setDomainDrawerOpen(true)}
          >
            <Code2 className="h-3.5 w-3.5 text-xblue" />
            领域接口
          </Button>

          <Button
            variant="outline"
            className="h-8 px-2.5 text-xs"
            data-testid="save-draft-btn"
            disabled={busyOp !== null}
            onClick={() => void handleSaveDraft()}
          >
            <Save className="h-3.5 w-3.5" />
            {busyOp === "save-draft" ? "保存中..." : "保存草案"}
          </Button>

          <Input
            data-testid="publish-summary-input"
            value={changeSummary}
            onChange={(e) => setChangeSummary(e.target.value)}
            placeholder="输入版本发布摘要..."
            className="h-8 w-44 text-xs"
          />

          <Button
            className="h-8 px-3 text-xs"
            data-testid="publish-schema-btn"
            disabled={
              busyOp !== null ||
              issues.some((iss) => iss.severity === "ERROR")
            }
            onClick={() => void executePublish()}
          >
            <UploadCloud className="h-3.5 w-3.5" />
            {busyOp === "publish" ? "发布中..." : "发布模型版本"}
          </Button>
        </div>
      </div>

      {/* Subgraph Switcher & Always-On Context Bar */}
      <StudioSubgraphBar
        subgraphs={subgraphs}
        activeSubgraph={activeSubgraph}
        activeDraft={activeDraft}
        draftTitle={draftTitle}
        baseVersion={baseVersion}
        publishedVersion={publishedVersion}
        isDirty={isDirty}
        mainNodeTypes={mainDoc.node_types}
        onSelectSubgraph={(slug) => void handleSelectSubgraph(slug)}
        onCreateSubgraph={handleCreateSubgraph}
        onDeleteSubgraph={handleDeleteSubgraph}
        onOpenMergeDialog={() => void openMergeDialog()}
      />

      {/* Draft Switcher Bar when open drafts exist */}
      {drafts.length > 0 && (
        <div className="flex flex-wrap items-center justify-between gap-2 rounded-md border border-border bg-muted/30 px-3 py-1.5 text-xs">
          <div className="flex flex-wrap items-center gap-1.5">
            <FilePlus2 className="h-3.5 w-3.5 text-muted-foreground" />
            <span className="text-muted-foreground">待发布草案列表:</span>
            {drafts.map((d) => (
              <button
                key={d.id || d.title}
                type="button"
                onClick={() => handleSelectDraft(d)}
                className={cn(
                  "rounded border px-2 py-0.5 text-[11px] cursor-pointer",
                  activeDraft?.id === d.id
                    ? "border-xblue bg-xblue-soft text-xblue font-medium"
                    : "border-border bg-background text-muted-foreground hover:text-foreground"
                )}
              >
                {d.title} (v{d.base_version} · {d.source})
              </button>
            ))}
          </div>

          <div className="flex items-center gap-1.5">
            <Input
              value={draftTitle}
              onChange={(e) => setDraftTitle(e.target.value)}
              placeholder="编辑当前草案标题"
              className="h-7 w-44 text-xs"
            />
            {activeDraft?.id && (
              <Button
                variant="ghost"
                className="h-7 px-2 py-1 text-xs text-red-600 dark:text-red-400"
                disabled={busyOp !== null}
                onClick={() => void handleDiscardActiveDraft()}
              >
                <Trash2 className="h-3 w-3" />
                丢弃草案
              </Button>
            )}
          </div>
        </div>
      )}

      {/* 409 VERSION_CONFLICT Alert */}
      {conflictBanner && (
        <div
          data-testid="version-conflict-banner"
          className="rounded-md border border-amber-500/50 bg-amber-500/10 p-3 flex flex-wrap items-center justify-between gap-2 text-xs"
        >
          <div className="flex items-center gap-2 text-amber-800 dark:text-amber-300">
            <AlertTriangle className="h-4 w-4 shrink-0" />
            <span>{conflictBanner}</span>
          </div>
          <div className="flex items-center gap-2">
            <Button
              variant="outline"
              className="h-7 px-2.5 py-1 text-xs"
              onClick={() => setVersionPanelOpen(true)}
            >
              <GitCompare className="h-3.5 w-3.5" />
              查看差异
            </Button>
            <Button
              className="h-7 px-2.5 py-1 text-xs"
              onClick={() => void loadSubgraphWorkspace(activeSubgraph)}
            >
              <RefreshCw className="h-3.5 w-3.5" />
              基于最新版本重新加载
            </Button>
          </div>
        </div>
      )}

      {/* 409 SCHEMA_BREAKING_CHANGE Alert */}
      {breakingBanner && (
        <div
          data-testid="breaking-change-banner"
          className="rounded-md border border-red-500/50 bg-red-500/10 p-3 space-y-2 text-xs"
        >
          <div className="flex items-center gap-2 text-red-700 dark:text-red-300 font-medium">
            <AlertTriangle className="h-4 w-4 shrink-0" />
            <span>{breakingBanner}</span>
          </div>
          <div className="flex flex-wrap items-center justify-between gap-2">
            <div className="flex flex-wrap items-center gap-4">
              <label className="flex items-center gap-1.5 cursor-pointer">
                <input
                  type="checkbox"
                  data-testid="force-publish-checkbox"
                  checked={forcePublish}
                  onChange={(e) => setForcePublish(e.target.checked)}
                />
                <span>确认强制发布破坏性变更 (force=true)</span>
              </label>
              {activeSubgraph === "main" && (
                <label className="flex items-center gap-1.5 cursor-pointer">
                  <input
                    type="checkbox"
                    checked={cascadeDelete}
                    onChange={(e) => setCascadeDelete(e.target.checked)}
                  />
                  <span>级联删除已移除类型的运行时实例 (cascade=true)</span>
                </label>
              )}
            </div>
            <Button
              className="h-7 px-3 py-1 text-xs"
              data-testid="confirm-force-publish-btn"
              disabled={!forcePublish || busyOp !== null}
              onClick={() => void executePublish(true)}
            >
              确认强制发布
            </Button>
          </div>
        </div>
      )}

      {/* Operational Status Banner */}
      {statusBanner && (
        <Card
          className="px-3 py-2 text-xs font-mono-tech flex items-center justify-between"
          data-testid="studio-status-toast"
        >
          <span>{statusBanner}</span>
          <button
            type="button"
            onClick={() => setStatusBanner(null)}
            className="text-muted-foreground hover:text-foreground cursor-pointer"
          >
            关闭
          </button>
        </Card>
      )}

      {/* Main Split Workbench: Canvas + IssueList on Left, Multi-Tab Inspector on Right */}
      <div className="grid grid-cols-1 xl:grid-cols-12 gap-4">
        <div className="xl:col-span-7 space-y-3">
          <StudioCanvas
            nodeTypes={workingDoc.node_types}
            edgeTypes={workingDoc.edge_types}
            issues={issues}
            selectedNodeName={selectedNodeName}
            selectedEdgeIndex={selectedEdgeIndex}
            onSelectNode={(name) => {
              setSelectedNodeName(name);
              if (name && inspectorTab === "edge") {
                setInspectorTab("basic");
              }
            }}
            onSelectEdge={(edgeIdx) => {
              setSelectedEdgeIndex(edgeIdx);
              if (edgeIdx !== null) {
                setInspectorTab("edge");
              }
            }}
            onUpdateNodePosition={(name, posX, posY) => {
              mutateDoc((prev) => ({
                ...prev,
                node_types: prev.node_types.map((nt) =>
                  nt.name === name ? { ...nt, pos_x: posX, pos_y: posY } : nt
                ),
              }));
            }}
            onApplyLayoutPositions={(positions) => {
              mutateDoc((prev) => ({
                ...prev,
                node_types: prev.node_types.map((nt) => {
                  const pos = positions[nt.name];
                  return pos ? { ...nt, pos_x: pos.x, pos_y: pos.y } : nt;
                }),
              }));
            }}
            onCreateNodeType={handleCreateNodeType}
            onCreateEdgeType={handleCreateEdgeType}
          />

          <IssueList
            issues={issues}
            validating={validating}
            onLocateIssue={handleLocateIssue}
          />
        </div>

        {/* Right Inspector Workbench */}
        <div className="xl:col-span-5 space-y-3">
          <Card className="p-3 space-y-3">
            {/* Inspector Tabs */}
            <div className="flex flex-wrap items-center gap-1 border-b border-border pb-2">
              {(
                [
                  { id: "basic", label: "节点基础" },
                  { id: "properties", label: "属性与指标" },
                  { id: "rules", label: "聚合与派生" },
                  {
                    id: "edge",
                    label: `关系连线 (${workingDoc.edge_types.length})`,
                  },
                  {
                    id: "mappings",
                    label: `数据映射 (${workingDoc.mappings.length})`,
                  },
                ] as const
              ).map((tab) => (
                <button
                  key={tab.id}
                  type="button"
                  data-testid={`inspector-tab-${tab.id}`}
                  onClick={() => setInspectorTab(tab.id)}
                  className={cn(
                    "rounded px-2.5 py-1 text-xs font-medium transition-colors cursor-pointer",
                    inspectorTab === tab.id
                      ? "bg-primary text-primary-foreground"
                      : "text-muted-foreground hover:text-foreground hover:bg-muted"
                  )}
                >
                  {tab.label}
                </button>
              ))}
            </div>

            {/* Node Selector Pill Strip for node-scoped tabs */}
            {(inspectorTab === "basic" ||
              inspectorTab === "properties" ||
              inspectorTab === "rules") &&
              workingDoc.node_types.length > 0 && (
                <div className="flex flex-wrap items-center gap-1">
                  <span className="text-[11px] text-muted-foreground mr-1">
                    当前节点:
                  </span>
                  {workingDoc.node_types.map((nt) => (
                    <button
                      key={nt.name}
                      type="button"
                      onClick={() => setSelectedNodeName(nt.name)}
                      className={cn(
                        "rounded border px-2 py-0.5 text-[11px] font-mono-tech cursor-pointer",
                        selectedNode?.name === nt.name
                          ? "border-xblue bg-xblue-soft text-xblue font-semibold"
                          : "border-border bg-background text-muted-foreground hover:text-foreground"
                      )}
                    >
                      {nt.display_name} ({nt.name})
                    </button>
                  ))}
                </div>
              )}

            {/* Tab Panels */}
            {inspectorTab === "basic" &&
              (selectedNode ? (
                <NodeTypeCard
                  nodeType={selectedNode}
                  onChange={handleUpdateSelectedNode}
                  onDelete={handleDeleteSelectedNode}
                />
              ) : (
                <div className="py-8 text-center text-xs text-muted-foreground">
                  请在左侧画布点击选择一个节点类型卡片，或点击“新增节点类型”。
                </div>
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
                <div className="py-8 text-center text-xs text-muted-foreground">
                  请先选择一个业务节点类型以编辑其静态属性与时序指标。
                </div>
              ))}

            {inspectorTab === "rules" &&
              (selectedNode ? (
                <RuleEditor
                  nodeIndex={selectedNodeIndex}
                  nodeType={selectedNode}
                  allNodeTypes={availableNodeTypesForEdges}
                  edgeTypes={workingDoc.edge_types}
                  issues={issues}
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
                <div className="py-8 text-center text-xs text-muted-foreground">
                  请先选择一个业务节点类型以配置跨边聚合或节点内派生公式。
                </div>
              ))}

            {inspectorTab === "edge" && (
              <div className="space-y-3">
                {workingDoc.edge_types.length > 0 && (
                  <div className="flex flex-wrap items-center gap-1">
                    {workingDoc.edge_types.map((et, idx) => (
                      <button
                        key={`${et.name}-${idx}`}
                        type="button"
                        onClick={() => setSelectedEdgeIndex(idx)}
                        className={cn(
                          "rounded border px-2 py-0.5 font-mono-tech text-[11px] cursor-pointer",
                          selectedEdgeIndex === idx
                            ? "border-xblue bg-xblue-soft text-xblue font-semibold"
                            : "border-border bg-background text-muted-foreground hover:text-foreground"
                        )}
                      >
                        {et.src_type_name} → {et.name} → {et.dst_type_name}
                      </button>
                    ))}
                  </div>
                )}

                {selectedEdge && selectedEdgeIndex !== null ? (
                  <EdgeTypeEditor
                    edgeType={selectedEdge}
                    availableNodeTypes={availableNodeTypesForEdges}
                    onChange={(updatedEdge) =>
                      mutateDoc((prev) => ({
                        ...prev,
                        edge_types: prev.edge_types.map((et, i) =>
                          i === selectedEdgeIndex ? updatedEdge : et
                        ),
                      }))
                    }
                    onDelete={() => {
                      const rmIdx = selectedEdgeIndex;
                      mutateDoc((prev) => ({
                        ...prev,
                        edge_types: prev.edge_types.filter(
                          (_, i) => i !== rmIdx
                        ),
                      }));
                      setSelectedEdgeIndex(null);
                    }}
                  />
                ) : (
                  <div className="py-6 text-center text-xs text-muted-foreground">
                    在画布上点击关系连线标签，或从节点右侧蓝点拖拽至目标节点即可创建新关系。
                  </div>
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
                  mutateDoc((prev) => ({ ...prev, mappings: nextMappings }))
                }
                onRefreshPersistedMappings={async () => {
                  const latest = await xblueApi
                    .listMappings(activeSubgraph)
                    .catch(() => [] as MappingRecord[]);
                  setPersistedMappings(latest);
                }}
              />
            )}
          </Card>

          {/* Compact Live Runtime Nodes Summary */}
          <Card className="p-3 space-y-2">
            <div className="flex items-center justify-between">
              <h2 className="text-xs font-semibold">
                企业主图已注册运行时节点快照 ({runtimeNodes.length})
              </h2>
              <Badge variant="outline">subgraph=main</Badge>
            </div>
            {runtimeNodes.length === 0 ? (
              <div className="text-xs text-muted-foreground">
                暂无运行时实例，发布主图映射后将自动从 DuckLake 物化写入。
              </div>
            ) : (
              <div className="grid grid-cols-1 md:grid-cols-2 gap-1.5 text-xs max-h-36 overflow-y-auto">
                {runtimeNodes.slice(0, 8).map((node) => (
                  <div
                    key={node.id}
                    className="rounded border border-border px-2.5 py-1.5"
                  >
                    <div className="flex items-center justify-between gap-1">
                      <span className="font-medium truncate">{node.title}</span>
                      <Badge variant="outline">{node.type_name}</Badge>
                    </div>
                    <div className="font-mono-tech text-[10px] text-muted-foreground truncate mt-0.5">
                      {node.node_key} · {JSON.stringify(node.metrics)}
                    </div>
                  </div>
                ))}
              </div>
            )}
          </Card>
        </div>
      </div>

      {/* Drawers & Modals */}
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
    </div>
  );
}
