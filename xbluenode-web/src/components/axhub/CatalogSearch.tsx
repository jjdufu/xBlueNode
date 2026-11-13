import { useCallback, useEffect, useState } from "react";
import { BookOpen, Database, Plus, Search, Trash2 } from "lucide-react";
import {
  axhubApi,
  type CatalogSearchHit,
  type GlossaryTerm,
} from "@/lib/api/axhub";
import { Badge, Button, Card, Input } from "@/components/ui/primitives";

export interface CatalogSearchProps {
  onSelectDataset?: (datasetId: string) => void;
}

export function CatalogSearch({ onSelectDataset }: CatalogSearchProps) {
  const [query, setQuery] = useState("");
  const [hits, setHits] = useState<CatalogSearchHit[]>([]);
  const [searchedOnce, setSearchedOnce] = useState(false);
  const [searching, setSearching] = useState(false);

  const [glossary, setGlossary] = useState<GlossaryTerm[]>([]);
  const [termName, setTermName] = useState("");
  const [termDef, setTermDef] = useState("");
  const [termSynonyms, setTermSynonyms] = useState("");
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  const loadGlossary = useCallback(async () => {
    try {
      const res = await axhubApi.listGlossary();
      setGlossary(Array.isArray(res?.items) ? res.items : []);
    } catch {
      // Ignore empty initial glossary error
    }
  }, []);

  useEffect(() => {
    void loadGlossary();
  }, [loadGlossary]);

  const executeSearch = async (q: string) => {
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
  };

  const handleSearchSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    void executeSearch(query);
  };

  const handleCreateTerm = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!termName.trim()) return;
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

  const handleDeleteTerm = async (id: string) => {
    setErrorMsg(null);
    try {
      await axhubApi.deleteGlossaryTerm(id);
      await loadGlossary();
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    }
  };

  return (
    <div className="space-y-4" data-testid="catalog-search-panel">
      {errorMsg && (
        <div className="rounded border border-foreground/30 bg-muted px-3 py-2 text-xs">
          {errorMsg}
        </div>
      )}

      {/* Catalog Full-Text Search */}
      <Card className="p-4 space-y-3">
        <div className="border-b border-border pb-2.5">
          <h2 className="text-sm font-semibold">全域数据资产目录检索</h2>
          <p className="text-xs text-muted-foreground">
            联合检索数据集名称、描述、标签、列级元数据与业务术语同义词扩展
          </p>
        </div>

        <form onSubmit={handleSearchSubmit} className="flex gap-2">
          <Input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="输入表名、列名、业务术语或标签（如 orders）..."
            data-testid="catalog-search-input"
          />
          <Button
            type="submit"
            disabled={searching}
            data-testid="catalog-search-btn"
          >
            <Search className="h-3.5 w-3.5" />
            {searching ? "检索中..." : "搜索目录"}
          </Button>
        </form>

        {searchedOnce && (
          <div className="space-y-2 pt-1" data-testid="catalog-search-results">
            <div className="text-xs text-muted-foreground">
              检索命中 {hits.length} 个数据集资产
            </div>
            {hits.length === 0 ? (
              <div className="rounded border border-dashed border-border py-6 text-center text-xs text-muted-foreground">
                未找到匹配的目录资产。
              </div>
            ) : (
              <div className="space-y-2">
                {hits.map((hit) => (
                  <div
                    key={hit.id}
                    className="rounded border border-border p-3 space-y-2 text-xs bg-background"
                    data-testid={`catalog-hit-${hit.name}`}
                  >
                    <div className="flex flex-wrap items-center justify-between gap-2">
                      <div className="flex items-center gap-2">
                        <Database className="h-3.5 w-3.5 text-xblue" />
                        <span className="font-mono-tech font-semibold">
                          {hit.name}
                        </span>
                        {hit.display_name && (
                          <span className="text-muted-foreground">
                            ({hit.display_name})
                          </span>
                        )}
                        <Badge variant="outline">{hit.source_kind}</Badge>
                        <span className="font-mono-tech text-muted-foreground">
                          {hit.row_count} 行
                        </span>
                      </div>

                      {onSelectDataset && (
                        <Button
                          type="button"
                          variant="outline"
                          className="px-2.5 py-1 text-[11px]"
                          onClick={() => onSelectDataset(hit.id)}
                          data-testid={`catalog-open-${hit.name}`}
                        >
                          查看数据集
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
                            命中列:
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
                            标签:
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
        <div className="flex items-center gap-2 border-b border-border pb-2.5">
          <BookOpen className="h-4 w-4 text-xblue" />
          <div>
            <h2 className="text-sm font-semibold">业务术语表（Glossary）</h2>
            <p className="text-xs text-muted-foreground">
              定义标准业务术语与同义词，自动增强目录检索与智能问数语义对齐
            </p>
          </div>
        </div>

        <form
          onSubmit={(e) => void handleCreateTerm(e)}
          className="grid grid-cols-1 md:grid-cols-4 gap-2"
        >
          <Input
            value={termName}
            onChange={(e) => setTermName(e.target.value)}
            placeholder="标准术语名称 *"
            data-testid="glossary-term-input"
          />
          <Input
            value={termDef}
            onChange={(e) => setTermDef(e.target.value)}
            placeholder="业务口径定义"
            data-testid="glossary-definition-input"
          />
          <Input
            value={termSynonyms}
            onChange={(e) => setTermSynonyms(e.target.value)}
            placeholder="同义词（逗号分隔，如 履约单,订单）"
            data-testid="glossary-synonyms-input"
          />
          <Button type="submit" variant="outline" data-testid="glossary-add-btn">
            <Plus className="h-3.5 w-3.5" />
            添加术语
          </Button>
        </form>

        {glossary.length === 0 ? (
          <div className="text-xs text-muted-foreground py-2">
            暂无已登记的业务术语。
          </div>
        ) : (
          <div className="grid grid-cols-1 md:grid-cols-2 gap-2">
            {glossary.map((item) => (
              <div
                key={item.id}
                className="rounded border border-border p-2.5 text-xs flex items-start justify-between gap-2 bg-background"
                data-testid={`glossary-item-${item.term}`}
              >
                <div className="space-y-1">
                  <div className="font-semibold flex items-center gap-2">
                    <span>{item.term}</span>
                    {item.synonyms.map((syn) => (
                      <Badge key={syn} variant="neutral">
                        {syn}
                      </Badge>
                    ))}
                  </div>
                  {item.definition && (
                    <div className="text-muted-foreground">{item.definition}</div>
                  )}
                </div>
                <Button
                  type="button"
                  variant="ghost"
                  className="px-2 py-1 text-xs"
                  onClick={() => void handleDeleteTerm(item.id)}
                >
                  <Trash2 className="h-3.5 w-3.5" />
                </Button>
              </div>
            ))}
          </div>
        )}
      </Card>
    </div>
  );
}
