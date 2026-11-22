import {
  forwardRef,
  useCallback,
  useEffect,
  useImperativeHandle,
  useMemo,
  useRef,
  useState,
} from "react";
import {
  AtSign,
  Database,
  FileText,
  GitBranch,
  Search,
  Wrench,
  X,
} from "lucide-react";
import { useI18n } from "@/i18n";
import { axhubApi, type Dataset } from "@/lib/api/axhub";
import { api, type DocumentSummary } from "@/lib/api/rag";
import { xblueApi, type SubgraphInfo } from "@/lib/api/xblue";
import { listEndpoints, type EndpointSpec } from "@/lib/api/axagent";
import type { ChatQueryScope } from "@/lib/api/analytics";
import { Badge, Button, Input, Tabs } from "@/components/ui/primitives";
import { cn } from "@/lib/utils";
import { hasActiveChatScope } from "./logic";

export type ScopeTabKind = "datasets" | "docs" | "subgraphs" | "skills";

export interface ScopeCandidateItem {
  readonly kind: ScopeTabKind;
  readonly id: string;
  readonly label: string;
  readonly sublabel: string;
}

export interface QueryScopeBarHandle {
  handleMentionKeyDown: (e: React.KeyboardEvent) => boolean;
}

export interface QueryScopeBarProps {
  scope: ChatQueryScope;
  onChange: (next: ChatQueryScope) => void;
  mentionKeyword: string | null;
  onDismissMention: () => void;
  onCommitMentionSelection: (nextScope: ChatQueryScope) => void;
  popoverPlacement?: "top" | "bottom";
}

export const QueryScopeBar = forwardRef<
  QueryScopeBarHandle,
  QueryScopeBarProps
>(function QueryScopeBar(
  {
    scope,
    onChange,
    mentionKeyword,
    onDismissMention,
    onCommitMentionSelection,
    popoverPlacement = "top",
  },
  ref
) {
  const { tx } = useI18n();
  const [manualOpen, setManualOpen] = useState(false);
  const [activeTab, setActiveTab] = useState<ScopeTabKind>("datasets");
  const [manualSearch, setManualSearch] = useState("");
  const [activeIndex, setActiveIndex] = useState(0);

  const [datasets, setDatasets] = useState<Dataset[]>([]);
  const [docs, setDocs] = useState<DocumentSummary[]>([]);
  const [subgraphs, setSubgraphs] = useState<SubgraphInfo[]>([]);
  const [endpoints, setEndpoints] = useState<EndpointSpec[]>([]);
  const [catalogLoaded, setCatalogLoaded] = useState(false);

  const containerRef = useRef<HTMLDivElement | null>(null);

  const isMentionActive = mentionKeyword !== null;
  const popoverOpen = manualOpen || isMentionActive;
  const activeScopeExists = hasActiveChatScope(scope);

  useEffect(() => {
    if ((!popoverOpen && !activeScopeExists) || catalogLoaded) return;
    let cancelled = false;
    void (async () => {
      const [dsRes, docRes, sgRes, epRes] = await Promise.all([
        axhubApi
          .listDatasets({ limit: 100 })
          .catch(() => ({ items: [] as Dataset[], next_cursor: null })),
        api.listDocuments({ limit: 100 }).catch(() => ({
          items: [] as DocumentSummary[],
          total: 0,
          next_cursor: null,
        })),
        xblueApi.listSubgraphs().catch(() => [] as SubgraphInfo[]),
        listEndpoints({ environment: "PROD" }).catch(
          () => [] as EndpointSpec[]
        ),
      ]);
      if (cancelled) return;
      setDatasets(Array.isArray(dsRes?.items) ? dsRes.items : []);
      setDocs(Array.isArray(docRes?.items) ? docRes.items : []);
      setSubgraphs(Array.isArray(sgRes) ? sgRes : []);
      setEndpoints(Array.isArray(epRes) ? epRes : []);
      setCatalogLoaded(true);
    })();
    return () => {
      cancelled = true;
    };
  }, [activeScopeExists, catalogLoaded, popoverOpen]);

  useEffect(() => {
    if (!manualOpen) return;
    const onMouseDown = (ev: MouseEvent) => {
      const target = ev.target as Node | null;
      if (target && !containerRef.current?.contains(target)) {
        setManualOpen(false);
      }
    };
    window.addEventListener("mousedown", onMouseDown);
    return () => window.removeEventListener("mousedown", onMouseDown);
  }, [manualOpen]);

  const effectiveFilter = (
    isMentionActive ? mentionKeyword : manualSearch
  )
    .trim()
    .toLowerCase();

  const allSubgraphItems = useMemo(() => {
    const base: Array<{ slug: string; displayName: string }> = [
      { slug: "main", displayName: tx("企业主图", "Enterprise Graph") },
    ];
    for (const sg of subgraphs) {
      if (sg.slug && sg.slug !== "main") {
        base.push({
          slug: sg.slug,
          displayName: sg.display_name || sg.slug,
        });
      }
    }
    return base;
  }, [subgraphs, tx]);

  const candidatesByTab = useMemo<
    Record<ScopeTabKind, ScopeCandidateItem[]>
  >(() => {
    const matchText = (a: string, b: string) =>
      !effectiveFilter ||
      a.toLowerCase().includes(effectiveFilter) ||
      b.toLowerCase().includes(effectiveFilter);

    return {
      datasets: datasets
        .filter((d) => matchText(d.display_name || d.name, d.name))
        .map((d) => ({
          kind: "datasets",
          id: d.id,
          label: d.display_name || d.name,
          sublabel: d.name,
        })),
      docs: docs
        .filter((d) => matchText(d.title, d.source_name))
        .map((d) => ({
          kind: "docs",
          id: d.id,
          label: d.title,
          sublabel: d.source_name,
        })),
      subgraphs: allSubgraphItems
        .filter((sg) => matchText(sg.slug, sg.displayName))
        .map((sg) => ({
          kind: "subgraphs",
          id: sg.slug,
          label: sg.displayName,
          sublabel: sg.slug,
        })),
      skills: endpoints
        .filter((ep) => matchText(ep.title || ep.slug, ep.slug))
        .map((ep) => ({
          kind: "skills",
          id: ep.slug,
          label: ep.title || ep.slug,
          sublabel: ep.slug,
        })),
    };
  }, [allSubgraphItems, datasets, docs, effectiveFilter, endpoints]);

  // Auto-switch tab during @keyword typing if current tab has 0 matches and another tab matches
  useEffect(() => {
    if (!isMentionActive || !effectiveFilter) return;
    if (candidatesByTab[activeTab].length > 0) return;
    const order: ScopeTabKind[] = ["datasets", "docs", "subgraphs", "skills"];
    const hit = order.find((k) => candidatesByTab[k].length > 0);
    if (hit && hit !== activeTab) {
      setActiveTab(hit);
    }
  }, [activeTab, candidatesByTab, effectiveFilter, isMentionActive]);

  const activeCandidates = candidatesByTab[activeTab];

  useEffect(() => {
    setActiveIndex(0);
  }, [activeTab, effectiveFilter]);

  const applyCandidateToScope = useCallback(
    (item: ScopeCandidateItem): ChatQueryScope => {
      const next: ChatQueryScope = { ...scope };
      if (item.kind === "datasets") {
        const prev = scope.dataset_ids ?? [];
        next.dataset_ids = prev.includes(item.id) ? prev : [...prev, item.id];
      } else if (item.kind === "docs") {
        const prev = scope.doc_ids ?? [];
        next.doc_ids = prev.includes(item.id) ? prev : [...prev, item.id];
      } else if (item.kind === "subgraphs") {
        next.subgraph_slug = item.id;
      } else if (item.kind === "skills") {
        const prev = scope.endpoint_slugs ?? [];
        next.endpoint_slugs = prev.includes(item.id)
          ? prev
          : [...prev, item.id];
      }
      return next;
    },
    [scope]
  );

  const handleSelectCandidate = useCallback(
    (item: ScopeCandidateItem) => {
      const nextScope = applyCandidateToScope(item);
      if (isMentionActive) {
        onCommitMentionSelection(nextScope);
      } else {
        onChange(nextScope);
      }
    },
    [applyCandidateToScope, isMentionActive, onChange, onCommitMentionSelection]
  );

  useImperativeHandle(
    ref,
    () => ({
      handleMentionKeyDown: (e: React.KeyboardEvent): boolean => {
        if (!popoverOpen) return false;
        if (e.key === "ArrowDown") {
          e.preventDefault();
          setActiveIndex((prev) =>
            activeCandidates.length === 0
              ? 0
              : (prev + 1) % activeCandidates.length
          );
          return true;
        }
        if (e.key === "ArrowUp") {
          e.preventDefault();
          setActiveIndex((prev) =>
            activeCandidates.length === 0
              ? 0
              : (prev - 1 + activeCandidates.length) % activeCandidates.length
          );
          return true;
        }
        if (e.key === "Enter" && !e.shiftKey && isMentionActive) {
          if (activeCandidates.length > 0) {
            e.preventDefault();
            const chosen =
              activeCandidates[
                Math.min(activeIndex, activeCandidates.length - 1)
              ];
            if (chosen) {
              handleSelectCandidate(chosen);
            }
            return true;
          }
        }
        if (e.key === "Escape") {
          e.preventDefault();
          setManualOpen(false);
          onDismissMention();
          return true;
        }
        return false;
      },
    }),
    [
      activeCandidates,
      activeIndex,
      handleSelectCandidate,
      isMentionActive,
      onDismissMention,
      popoverOpen,
    ]
  );

  const datasetLabelById = useMemo(() => {
    const m = new Map<string, string>();
    for (const d of datasets) {
      m.set(d.id, d.display_name || d.name);
    }
    return m;
  }, [datasets]);

  const docLabelById = useMemo(() => {
    const m = new Map<string, string>();
    for (const d of docs) {
      m.set(d.id, d.title);
    }
    return m;
  }, [docs]);

  const endpointLabelBySlug = useMemo(() => {
    const m = new Map<string, string>();
    for (const ep of endpoints) {
      m.set(ep.slug, ep.title || ep.slug);
    }
    return m;
  }, [endpoints]);

  const removeDatasetId = (id: string) => {
    onChange({
      ...scope,
      dataset_ids: (scope.dataset_ids ?? []).filter((x) => x !== id),
    });
  };

  const removeDocId = (id: string) => {
    onChange({
      ...scope,
      doc_ids: (scope.doc_ids ?? []).filter((x) => x !== id),
    });
  };

  const removeEndpointSlug = (slugVal: string) => {
    onChange({
      ...scope,
      endpoint_slugs: (scope.endpoint_slugs ?? []).filter((x) => x !== slugVal),
    });
  };

  const isCandidatePinned = (item: ScopeCandidateItem): boolean => {
    if (item.kind === "datasets") {
      return (scope.dataset_ids ?? []).includes(item.id);
    }
    if (item.kind === "docs") {
      return (scope.doc_ids ?? []).includes(item.id);
    }
    if (item.kind === "subgraphs") {
      return scope.subgraph_slug === item.id;
    }
    return (scope.endpoint_slugs ?? []).includes(item.id);
  };

  return (
    <div
      ref={containerRef}
      data-testid="query-scope-bar"
      className="relative flex flex-wrap items-center justify-between gap-2 border-b border-border/70 px-3 py-1.5 text-xs"
    >
      {/* Left: Open button + Doc Prefix Input + Active Blue Scope Pills */}
      <div className="flex flex-wrap items-center gap-1.5 min-w-0 flex-1">
        <Button
          type="button"
          variant="outline"
          size="sm"
          className="h-6 px-2 text-[11px]"
          onClick={() => setManualOpen((v) => !v)}
          data-testid="query-scope-open-btn"
        >
          <AtSign className="h-3 w-3 text-xblue" />
          <span>{tx("挂载范围", "Pin Scope")}</span>
        </Button>

        <div className="inline-flex items-center gap-1 rounded border border-border/80 bg-background px-1.5 h-6">
          <span className="text-[11px] text-muted-foreground shrink-0">
            {tx("前缀", "Prefix")}
          </span>
          <input
            type="text"
            value={scope.doc_title_prefix ?? ""}
            onChange={(e) =>
              onChange({
                ...scope,
                doc_title_prefix: e.target.value || null,
              })
            }
            placeholder="SOP-2026-"
            aria-label={tx("文档标题前缀", "Document title prefix")}
            data-testid="query-scope-doc-prefix-input"
            className="w-24 bg-transparent text-[11px] font-mono-tech text-foreground placeholder:text-muted-foreground focus:outline-none"
          />
        </div>

        {(scope.dataset_ids ?? []).map((id) => (
          <Badge
            key={`ds-${id}`}
            variant="xblue"
            className="gap-1"
            data-testid={`scope-pill-dataset-${id}`}
          >
            <Database className="h-2.5 w-2.5 shrink-0" />
            <span className="truncate max-w-36">
              {datasetLabelById.get(id) ?? id.slice(0, 8)}
            </span>
            <button
              type="button"
              onClick={() => removeDatasetId(id)}
              aria-label={tx("移除", "Remove")}
              className="hover:opacity-75 cursor-pointer"
            >
              <X className="h-2.5 w-2.5" />
            </button>
          </Badge>
        ))}

        {(scope.doc_ids ?? []).map((id) => (
          <Badge
            key={`doc-${id}`}
            variant="xblue"
            className="gap-1"
            data-testid={`scope-pill-doc-${id}`}
          >
            <FileText className="h-2.5 w-2.5 shrink-0" />
            <span className="truncate max-w-36">
              {docLabelById.get(id) ?? id.slice(0, 8)}
            </span>
            <button
              type="button"
              onClick={() => removeDocId(id)}
              aria-label={tx("移除", "Remove")}
              className="hover:opacity-75 cursor-pointer"
            >
              <X className="h-2.5 w-2.5" />
            </button>
          </Badge>
        ))}

        {scope.doc_title_prefix && scope.doc_title_prefix.trim().length > 0 && (
          <Badge
            variant="xblue"
            className="gap-1"
            data-testid="scope-pill-doc-prefix"
          >
            <FileText className="h-2.5 w-2.5 shrink-0" />
            <span>
              {tx(
                `前缀: ${scope.doc_title_prefix.trim()}`,
                `Prefix: ${scope.doc_title_prefix.trim()}`
              )}
            </span>
            <button
              type="button"
              onClick={() => onChange({ ...scope, doc_title_prefix: null })}
              aria-label={tx("移除", "Remove")}
              className="hover:opacity-75 cursor-pointer"
            >
              <X className="h-2.5 w-2.5" />
            </button>
          </Badge>
        )}

        {scope.subgraph_slug && scope.subgraph_slug.trim().length > 0 && (
          <Badge
            variant="xblue"
            className="gap-1"
            data-testid={`scope-pill-subgraph-${scope.subgraph_slug}`}
          >
            <GitBranch className="h-2.5 w-2.5 shrink-0" />
            <span>{scope.subgraph_slug}</span>
            <button
              type="button"
              onClick={() => onChange({ ...scope, subgraph_slug: null })}
              aria-label={tx("移除", "Remove")}
              className="hover:opacity-75 cursor-pointer"
            >
              <X className="h-2.5 w-2.5" />
            </button>
          </Badge>
        )}

        {(scope.endpoint_slugs ?? []).map((epSlug) => (
          <Badge
            key={`ep-${epSlug}`}
            variant="xblue"
            className="gap-1"
            data-testid={`scope-pill-endpoint-${epSlug}`}
          >
            <Wrench className="h-2.5 w-2.5 shrink-0" />
            <span className="truncate max-w-36">
              {endpointLabelBySlug.get(epSlug) ?? epSlug}
            </span>
            <button
              type="button"
              onClick={() => removeEndpointSlug(epSlug)}
              aria-label={tx("移除", "Remove")}
              className="hover:opacity-75 cursor-pointer"
            >
              <X className="h-2.5 w-2.5" />
            </button>
          </Badge>
        ))}
      </div>

      {/* Right: Clear button */}
      <Button
        type="button"
        variant="ghost"
        size="sm"
        disabled={!activeScopeExists}
        onClick={() => onChange({})}
        className="h-6 px-2 text-[11px]"
        data-testid="query-scope-clear-btn"
      >
        {tx("清空", "Clear")}
      </Button>

      {/* Categorized Search Popover (triggered by @mention or 挂载范围) */}
      {popoverOpen && (
        <div
          data-testid="query-scope-popover"
          className={cn(
            "absolute left-2 z-40 w-[min(420px,calc(100vw-2rem))] rounded-md border border-border bg-card text-card-foreground shadow-lg overflow-hidden",
            popoverPlacement === "bottom"
              ? "top-full mt-1.5"
              : "bottom-full mb-1.5"
          )}
        >
          <div className="px-2.5 pt-1.5 bg-muted/20">
            <Tabs<ScopeTabKind>
              value={activeTab}
              onChange={setActiveTab}
              items={[
                {
                  key: "datasets",
                  label: tx("数据集", "Datasets"),
                  count: candidatesByTab.datasets.length,
                  testId: "query-scope-tab-datasets",
                },
                {
                  key: "docs",
                  label: tx("文档", "Docs"),
                  count: candidatesByTab.docs.length,
                  testId: "query-scope-tab-docs",
                },
                {
                  key: "subgraphs",
                  label: tx("子图", "Subgraphs"),
                  count: candidatesByTab.subgraphs.length,
                  testId: "query-scope-tab-subgraphs",
                },
                {
                  key: "skills",
                  label: tx("技能", "Skills"),
                  count: candidatesByTab.skills.length,
                  testId: "query-scope-tab-skills",
                },
              ]}
            />
          </div>

          {!isMentionActive && (
            <div className="p-2 border-b border-border flex items-center gap-1.5">
              <Search className="h-3.5 w-3.5 text-muted-foreground shrink-0" />
              <Input
                value={manualSearch}
                onChange={(e) => setManualSearch(e.target.value)}
                placeholder="demo_orders"
                className="h-7 text-xs"
                data-testid="query-scope-search-input"
              />
            </div>
          )}

          <div className="max-h-52 overflow-y-auto p-1.5 space-y-0.5">
            {activeCandidates.length === 0 ? (
              <div className="py-4 text-center text-xs text-muted-foreground">
                {tx("暂无匹配项", "No items")}
              </div>
            ) : (
              activeCandidates.map((item, idx) => {
                const highlighted = idx === activeIndex;
                const pinned = isCandidatePinned(item);
                return (
                  <button
                    key={`${item.kind}-${item.id}`}
                    type="button"
                    onMouseEnter={() => setActiveIndex(idx)}
                    onClick={() => handleSelectCandidate(item)}
                    data-testid={`query-scope-option-${item.kind}-${item.id}`}
                    className={cn(
                      "w-full flex items-center justify-between gap-2 rounded px-2.5 py-1.5 text-left text-xs transition-colors cursor-pointer",
                      highlighted
                        ? "bg-xblue-soft text-xblue font-medium"
                        : "hover:bg-muted/60 text-foreground"
                    )}
                  >
                    <div className="min-w-0 flex-1 truncate">
                      <span className="truncate">{item.label}</span>
                      {item.sublabel && item.sublabel !== item.label && (
                        <span className="ml-1.5 font-mono-tech text-[11px] text-muted-foreground">
                          {item.sublabel}
                        </span>
                      )}
                    </div>
                    {pinned && (
                      <Badge variant="xblue">{tx("已选", "Pinned")}</Badge>
                    )}
                  </button>
                );
              })
            )}
          </div>
        </div>
      )}
    </div>
  );
});
