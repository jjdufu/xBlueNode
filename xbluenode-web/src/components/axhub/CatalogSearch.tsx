import React, { useCallback, useEffect, useState } from "react";
import { BookOpen, Database, Plus, Search, Trash2 } from "lucide-react";
import { useI18n } from "@/i18n";
import {
  axhubApi,
  type CatalogSearchHit,
  type GlossaryTerm,
} from "@/lib/api/axhub";
import type { SourceKind } from "@/lib/api/enums";
import { usePrincipal } from "@/lib/permissions";
import {
  Badge,
  Button,
  Card,
  ConfirmDialog,
  EmptyState,
  Input,
} from "@/components/ui/primitives";

export interface CatalogSearchProps {
  onSelectDataset?: (datasetId: string) => void;
}

export function CatalogSearch({ onSelectDataset }: CatalogSearchProps) {
  const { tx, tEnum, fmtNum } = useI18n();
  const { canAny } = usePrincipal();
  const canEditGlossary = canAny(["DATA_ADMIN", "DATA_SYNC"]);

  const [query, setQuery] = useState("");
  const [hits, setHits] = useState<CatalogSearchHit[]>([]);
  const [searchedOnce, setSearchedOnce] = useState(false);
  const [searching, setSearching] = useState(false);

  const [glossary, setGlossary] = useState<GlossaryTerm[]>([]);
  const [termName, setTermName] = useState("");
  const [termDef, setTermDef] = useState("");
  const [termSynonyms, setTermSynonyms] = useState("");
  const [pendingDeleteTerm, setPendingDeleteTerm] =
    useState<GlossaryTerm | null>(null);
  const [deletingTerm, setDeletingTerm] = useState(false);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  const formatSourceKind = (kind: SourceKind): string => {
    switch (kind) {
      case "NOSQL_COLLECTION":
        return tx("文档集合", "NoSQL Collection");
      case "TIMESERIES":
        return tx("时序表", "Time-Series");
      case "STREAM":
        return tx("流数据", "Stream");
      case "API_RESOURCE":
        return tx("接口资源", "API Resource");
      default:
        return tEnum(kind);
    }
  };

  const loadGlossary = useCallback(async () => {
    try {
      const res = await axhubApi.listGlossary();
      setGlossary(Array.isArray(res?.items) ? res.items : []);
    } catch {
      // Non-blocking
    }
  }, []);

  const executeSearch = useCallback(async (q: string) => {
    setSearching(true);
    setErrorMsg(null);
    setSearchedOnce(true);
    try {
      const res = await axhubApi.searchCatalog(q.trim(), 50);
      setHits(Array.isArray(res?.items) ? res.items : []);
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setSearching(false);
    }
  }, []);

  useEffect(() => {
    void loadGlossary();
    void executeSearch("");
  }, [executeSearch, loadGlossary]);

  const handleSearchSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    void executeSearch(query);
  };

  const handleCreateTerm = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!canEditGlossary || !termName.trim()) return;
    setErrorMsg(null);
    try {
      const synonyms = termSynonyms
        .split(",")
        .map((s) => s.trim())
        .filter((s) => s.length > 0);
      await axhubApi.createGlossaryTerm({
        term: termName.trim(),
        definition: termDef.trim(),
        synonyms,
      });
      setTermName("");
      setTermDef("");
      setTermSynonyms("");
      await loadGlossary();
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    }
  };

  const handleConfirmDeleteTerm = async () => {
    if (!pendingDeleteTerm || !canEditGlossary) return;
    setDeletingTerm(true);
    setErrorMsg(null);
    try {
      await axhubApi.deleteGlossaryTerm(pendingDeleteTerm.id);
      setPendingDeleteTerm(null);
      await loadGlossary();
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setDeletingTerm(false);
    }
  };

  return (
    <div className="space-y-4" data-testid="catalog-search-panel">
      {errorMsg && (
        <div
          className="rounded border border-red-500/30 bg-red-500/5 px-3 py-2 text-xs text-red-600 dark:text-red-400"
          data-testid="catalog-error-banner"
        >
          {errorMsg}
        </div>
      )}

      {/* Catalog Full-Text Search */}
      <Card className="p-4 space-y-3">
        <div className="flex items-center justify-between border-b border-border pb-2.5">
          <h2 className="text-sm font-semibold">
            {tx("数据资产目录检索", "Data Asset Catalog")}
          </h2>
          {searchedOnce && (
            <Badge variant="neutral">
              {fmtNum(hits.length, 0)} {tx("项", "hits")}
            </Badge>
          )}
        </div>

        <form onSubmit={handleSearchSubmit} className="flex items-center gap-2">
          <Input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="demo_orders / warehouse_code / supply_chain"
            data-testid="catalog-search-input"
          />
          <Button
            type="submit"
            disabled={searching}
            data-testid="catalog-search-btn"
          >
            <Search className="h-3.5 w-3.5" />
            {searching ? tx("检索中…", "Searching…") : tx("搜索", "Search")}
          </Button>
        </form>

        {searchedOnce && (
          <div className="space-y-2 pt-1" data-testid="catalog-search-results">
            {hits.length === 0 ? (
              <EmptyState title={tx("无匹配资产", "No matching assets")} />
            ) : (
              <div className="space-y-2 max-h-[420px] overflow-y-auto pr-1">
                {hits.map((hit) => (
                  <div
                    key={hit.id}
                    className="rounded border border-border p-3 space-y-2 text-xs bg-background"
                    data-testid={`catalog-hit-${hit.name}`}
                  >
                    <div className="flex flex-wrap items-center justify-between gap-2">
                      <div className="flex flex-wrap items-center gap-2">
                        <Database className="h-3.5 w-3.5 text-xblue" />
                        <span className="font-mono-tech font-semibold">
                          {hit.name}
                        </span>
                        {hit.display_name && hit.display_name !== hit.name && (
                          <span className="text-muted-foreground">
                            {hit.display_name}
                          </span>
                        )}
                        <Badge variant="outline">
                          {formatSourceKind(hit.source_kind)}
                        </Badge>
                        <span className="font-mono-tech text-muted-foreground">
                          {fmtNum(hit.row_count, 0)} {tx("行", "rows")}
                        </span>
                      </div>

                      {onSelectDataset && (
                        <Button
                          type="button"
                          variant="outline"
                          size="sm"
                          onClick={() => onSelectDataset(hit.id)}
                          data-testid={`catalog-open-${hit.name}`}
                        >
                          {tx("查看", "Inspect")}
                        </Button>
                      )}
                    </div>

                    {hit.description && (
                      <p className="text-muted-foreground">{hit.description}</p>
                    )}

                    <div className="flex flex-wrap items-center gap-2">
                      {hit.matched_columns.length > 0 && (
                        <div className="flex flex-wrap items-center gap-1">
                          <span className="text-[11px] text-muted-foreground">
                            {tx("命中列", "Columns")}:
                          </span>
                          {hit.matched_columns.map((col) => (
                            <Badge
                              key={col}
                              variant="xblue"
                              data-testid={`matched-col-${hit.name}-${col}`}
                            >
                              {col}
                            </Badge>
                          ))}
                        </div>
                      )}

                      {hit.tags.length > 0 && (
                        <div className="flex flex-wrap items-center gap-1">
                          <span className="text-[11px] text-muted-foreground">
                            {tx("标签", "Tags")}:
                          </span>
                          {hit.tags.map((tag) => (
                            <Badge key={tag} variant="neutral">
                              {tag}
                            </Badge>
                          ))}
                        </div>
                      )}
                    </div>
                  </div>
                ))}
              </div>
            )}
          </div>
        )}
      </Card>

      {/* Business Glossary Management */}
      <Card className="p-4 space-y-3" data-testid="glossary-section">
        <div className="flex items-center justify-between border-b border-border pb-2.5">
          <div className="flex items-center gap-2">
            <BookOpen className="h-4 w-4 text-xblue" />
            <h2 className="text-sm font-semibold">
              {tx("业务术语表", "Business Glossary")}
            </h2>
          </div>
          <Badge variant="neutral">{fmtNum(glossary.length, 0)}</Badge>
        </div>

        <form
          onSubmit={(e) => void handleCreateTerm(e)}
          className="grid grid-cols-1 md:grid-cols-[180px_minmax(220px,1fr)_200px_auto] gap-2.5 items-end"
        >
          <div className="space-y-1">
            <label className="text-[11px] text-muted-foreground block">
              {tx("业务术语名称", "Term Name")}
            </label>
            <Input
              value={termName}
              disabled={!canEditGlossary}
              onChange={(e) => setTermName(e.target.value)}
              placeholder={tx("履约订单", "Fulfilled Order")}
              data-testid="glossary-term-input"
            />
          </div>
          <div className="space-y-1">
            <label className="text-[11px] text-muted-foreground block">
              {tx("业务定义释义", "Definition")}
            </label>
            <Input
              value={termDef}
              disabled={!canEditGlossary}
              onChange={(e) => setTermDef(e.target.value)}
              placeholder={tx(
                "已完成支付并分配仓库的订单",
                "Paid order assigned to a warehouse"
              )}
              data-testid="glossary-definition-input"
            />
          </div>
          <div className="space-y-1">
            <label className="text-[11px] text-muted-foreground block">
              {tx("同义词 / 表映射", "Synonyms")}
            </label>
            <Input
              value={termSynonyms}
              disabled={!canEditGlossary}
              onChange={(e) => setTermSynonyms(e.target.value)}
              placeholder="demo_orders, orders"
              data-testid="glossary-synonyms-input"
            />
          </div>
          <Button
            type="submit"
            variant="outline"
            disabled={!canEditGlossary || !termName.trim()}
            data-testid="glossary-add-btn"
          >
            <Plus className="h-3.5 w-3.5" />
            {tx("新增术语", "Add Term")}
          </Button>
        </form>

        {glossary.length === 0 ? (
          <EmptyState title={tx("暂无业务术语", "No glossary terms")} />
        ) : (
          <div className="grid grid-cols-1 md:grid-cols-2 gap-2">
            {glossary.map((item) => (
              <div
                key={item.id}
                className="rounded border border-border p-2.5 text-xs flex items-start justify-between gap-2 bg-background"
                data-testid={`glossary-item-${item.term}`}
              >
                <div className="space-y-1 min-w-0">
                  <div className="font-semibold flex flex-wrap items-center gap-1.5">
                    <span>{item.term}</span>
                    {item.synonyms.map((syn) => (
                      <Badge key={syn} variant="neutral">
                        {syn}
                      </Badge>
                    ))}
                  </div>
                  {item.definition && (
                    <div className="text-muted-foreground">
                      {item.definition}
                    </div>
                  )}
                </div>
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  disabled={!canEditGlossary}
                  onClick={() => setPendingDeleteTerm(item)}
                  aria-label={tx("删除", "Delete")}
                >
                  <Trash2 className="h-3.5 w-3.5" />
                </Button>
              </div>
            ))}
          </div>
        )}
      </Card>

      <ConfirmDialog
        open={Boolean(pendingDeleteTerm)}
        onClose={() => setPendingDeleteTerm(null)}
        onConfirm={() => void handleConfirmDeleteTerm()}
        loading={deletingTerm}
        title={tx("删除业务术语", "Delete Term")}
        description={tx(
          `确定删除业务术语「${pendingDeleteTerm?.term ?? ""}」？`,
          `Delete glossary term "${pendingDeleteTerm?.term ?? ""}"?`
        )}
      />
    </div>
  );
}
