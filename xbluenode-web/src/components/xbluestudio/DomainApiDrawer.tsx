import { useEffect, useMemo, useState } from "react";
import { Code2, Globe, Play, RefreshCw, X } from "lucide-react";
import {
  xblueApi,
  type ObjectCatalogResponse,
  type SchemaDoc,
} from "@/lib/api/xblue";
import { Badge, Button, Textarea } from "@/components/ui/primitives";

export interface DomainApiDrawerProps {
  open: boolean;
  fallbackMainDoc: SchemaDoc;
  mainVersion: number;
  onClose: () => void;
}

const DEFAULT_SAMPLE_QUERY = `{
  warehouse(nodeKey: "W-1") {
    title
    inventory
    fulfillsOrders(limit: 5) {
      nodeKey
    }
  }
}`;

function toCamelCase(name: string): string {
  if (!name) return "item";
  return name.charAt(0).toLowerCase() + name.slice(1);
}

function buildFallbackSdl(doc: SchemaDoc): string {
  if (doc.node_types.length === 0) {
    return "# 企业主图尚未发布领域节点类型";
  }
  const lines: string[] = [];
  for (const nt of doc.node_types) {
    lines.push(`type ${nt.name} {`);
    lines.push("  id: ID!");
    lines.push("  nodeKey: String!");
    lines.push("  title: String!");
    lines.push("  updatedAt: String!");
    for (const p of nt.properties) {
      const gqlType =
        p.data_type === "INT64"
          ? "Int"
          : p.data_type === "FLOAT64" || p.data_type === "DECIMAL"
            ? "Float"
            : p.data_type === "BOOLEAN"
              ? "Boolean"
              : "String";
      lines.push(`  ${p.name}: ${gqlType}`);
    }
    for (const et of doc.edge_types) {
      if (et.src_type_name === nt.name) {
        const edgeCamel = et.name
          .toLowerCase()
          .replace(/_([a-z])/g, (_, c: string) => c.toUpperCase());
        lines.push(
          `  ${edgeCamel}${et.dst_type_name}s(limit: Int = 50): [${et.dst_type_name}!]!`
        );
      }
    }
    lines.push("}");
    lines.push("");
  }
  lines.push("type Query {");
  for (const nt of doc.node_types) {
    const camel = toCamelCase(nt.name);
    lines.push(`  ${camel}(nodeKey: String!): ${nt.name}`);
    lines.push(
      `  ${camel}List(q: String, limit: Int = 50, cursor: String): [${nt.name}!]!`
    );
  }
  lines.push("}");
  return lines.join("\n");
}

export function DomainApiDrawer({
  open,
  fallbackMainDoc,
  mainVersion,
  onClose,
}: DomainApiDrawerProps) {
  const [sdlText, setSdlText] = useState("");
  const [sdlVersion, setSdlVersion] = useState(mainVersion);
  const [catalog, setCatalog] = useState<ObjectCatalogResponse>({
    version: mainVersion,
    items: [],
  });
  const [loading, setLoading] = useState(false);
  const [queryText, setQueryText] = useState(DEFAULT_SAMPLE_QUERY);
  const [variablesText, setVariablesText] = useState("{}");
  const [runningQuery, setRunningQuery] = useState(false);
  const [queryResponse, setQueryResponse] = useState<string>("");
  const [queryError, setQueryError] = useState<string | null>(null);

  const loadDomainMetadata = async () => {
    setLoading(true);
    try {
      const [sdlResp, objResp] = await Promise.all([
        xblueApi.getGraphqlSdl().catch(() => ({ version: 0, sdl: "" })),
        xblueApi
          .listDomainObjects()
          .catch(() => ({ version: 0, items: [] })),
      ]);
      setSdlText(sdlResp.sdl);
      setSdlVersion(sdlResp.version || objResp.version || mainVersion);
      setCatalog(objResp);
      if (objResp.items.length > 0 && objResp.items[0]?.sample_graphql) {
        setQueryText(objResp.items[0].sample_graphql);
      }
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    if (!open) return;
    void loadDomainMetadata();
  }, [open, mainVersion]);

  const displayedSdl = useMemo(() => {
    if (sdlText.trim().length > 0) return sdlText;
    return buildFallbackSdl(fallbackMainDoc);
  }, [sdlText, fallbackMainDoc]);

  const effectiveCatalogItems = useMemo(() => {
    if (catalog.items.length > 0) return catalog.items;
    return fallbackMainDoc.node_types.map((nt) => {
      const camel = toCamelCase(nt.name);
      return {
        name: nt.name,
        display_name: nt.display_name,
        description: nt.description,
        collection_path: `/api/xblue/objects/${nt.name}`,
        item_path: `/api/xblue/objects/${nt.name}/{node_key}`,
        graphql_field: camel,
        graphql_list_field: `${camel}List`,
        fields: nt.properties.map((p) => ({
          name: p.name,
          display_name: p.display_name,
          kind: p.kind,
          data_type: p.data_type,
          required: p.required,
          unit: p.unit,
          pii_category: p.pii_category,
        })),
        neighbors: [],
        sample_graphql: DEFAULT_SAMPLE_QUERY,
      };
    });
  }, [catalog.items, fallbackMainDoc.node_types]);

  if (!open) return null;

  const handleRunGraphql = async () => {
    setRunningQuery(true);
    setQueryError(null);
    try {
      let parsedVars: Record<string, unknown> | undefined;
      const trimmedVars = variablesText.trim();
      if (trimmedVars && trimmedVars !== "{}") {
        const rawVars: unknown = JSON.parse(trimmedVars);
        if (
          typeof rawVars === "object" &&
          rawVars !== null &&
          !Array.isArray(rawVars)
        ) {
          parsedVars = rawVars as Record<string, unknown>;
        } else {
          setQueryError("Variables 必须为 JSON 对象");
          setRunningQuery(false);
          return;
        }
      }
      const res = await xblueApi.executeGraphql(queryText, parsedVars);
      setQueryResponse(JSON.stringify(res, null, 2));
    } catch (err) {
      setQueryError(err instanceof Error ? err.message : "GraphQL 查询执行失败");
    } finally {
      setRunningQuery(false);
    }
  };

  return (
    <div
      className="fixed inset-0 z-50 flex justify-end bg-black/40"
      data-testid="domain-api-drawer"
    >
      <div className="h-full w-full max-w-3xl bg-card border-l border-border shadow-xl flex flex-col">
        <div className="flex items-center justify-between border-b border-border px-4 py-3">
          <div className="flex items-center gap-2">
            <Code2 className="h-4 w-4 text-xblue" />
            <div>
              <h2 className="text-sm font-semibold">
                领域对象与动态 GraphQL 接口 · 企业主图 v{sdlVersion || mainVersion}
              </h2>
              <p className="text-[11px] text-muted-foreground">
                主图 Schema 发布后零代码实时热更新强类型对象 REST 与 async-graphql 动态 Schema
              </p>
            </div>
          </div>

          <div className="flex items-center gap-1.5">
            <Button
              variant="outline"
              className="h-7 px-2.5 py-1 text-xs"
              onClick={() => void loadDomainMetadata()}
              disabled={loading}
            >
              <RefreshCw className="h-3 w-3" />
              刷新
            </Button>
            <button
              type="button"
              data-testid="close-domain-drawer-btn"
              onClick={onClose}
              className="rounded p-1 hover:bg-muted text-muted-foreground cursor-pointer"
            >
              <X className="h-4 w-4" />
            </button>
          </div>
        </div>

        <div className="flex-1 overflow-y-auto p-4 space-y-4 text-xs">
          {/* 1. REST Object Catalog */}
          <div className="space-y-2">
            <div className="flex items-center justify-between">
              <span className="font-semibold flex items-center gap-1.5">
                <Globe className="h-3.5 w-3.5 text-xblue" />
                强类型领域对象 REST 目录 (/api/xblue/objects)
              </span>
              <Badge variant="xblue">
                {effectiveCatalogItems.length} 个已发布实体类型
              </Badge>
            </div>

            <div className="grid grid-cols-1 md:grid-cols-2 gap-2">
              {effectiveCatalogItems.map((item) => (
                <div
                  key={item.name}
                  data-testid={`domain-object-card-${item.name}`}
                  className="rounded border border-border bg-muted/20 p-2.5 space-y-1.5"
                >
                  <div className="flex items-center justify-between">
                    <div>
                      <span className="font-semibold">{item.display_name}</span>
                      <span className="ml-1.5 font-mono-tech text-muted-foreground">
                        {item.name}
                      </span>
                    </div>
                    {item.sample_graphql && (
                      <button
                        type="button"
                        onClick={() => setQueryText(item.sample_graphql)}
                        className="text-[11px] text-xblue hover:underline cursor-pointer"
                      >
                        填入示例查询
                      </button>
                    )}
                  </div>
                  <div className="space-y-0.5 font-mono-tech text-[11px] text-muted-foreground">
                    <div>GET {item.collection_path}</div>
                    <div>GET {item.item_path}?include_neighbors=true</div>
                  </div>
                  <div className="flex flex-wrap gap-1 pt-0.5">
                    {item.fields.slice(0, 5).map((f) => (
                      <Badge key={f.name} variant="outline">
                        {f.name}:{f.data_type}
                      </Badge>
                    ))}
                    {item.neighbors.map((nb) => (
                      <Badge key={nb.field_name} variant="xblue">
                        → {nb.field_name}
                      </Badge>
                    ))}
                  </div>
                </div>
              ))}
            </div>
          </div>

          {/* 2. Dynamic GraphQL SDL */}
          <div className="space-y-1.5 border-t border-border pt-3">
            <div className="flex items-center justify-between">
              <span className="font-semibold">
                动态 GraphQL SDL (/api/xblue/graphql/sdl)
              </span>
              <span className="font-mono-tech text-[11px] text-muted-foreground">
                POST /api/xblue/graphql
              </span>
            </div>
            <pre
              data-testid="domain-graphql-sdl"
              className="max-h-56 overflow-auto rounded border border-border bg-muted/40 p-3 font-mono-tech text-[11px] leading-relaxed whitespace-pre"
            >
              {displayedSdl}
            </pre>
          </div>

          {/* 3. Interactive GraphQL Trial Runner */}
          <div className="space-y-2 border-t border-border pt-3">
            <div className="flex items-center justify-between">
              <span className="font-semibold">
                交互式 GraphQL 查询试运行面板
              </span>
              <Button
                data-testid="graphql-run-btn"
                className="h-7 px-3 py-1 text-xs"
                onClick={() => void handleRunGraphql()}
                disabled={runningQuery || !queryText.trim()}
              >
                <Play className="h-3.5 w-3.5" />
                {runningQuery ? "执行中..." : "执行示例查询"}
              </Button>
            </div>

            <div className="grid grid-cols-1 md:grid-cols-3 gap-2">
              <div className="md:col-span-2">
                <label className="block text-[11px] text-muted-foreground mb-1">
                  GraphQL Query
                </label>
                <Textarea
                  data-testid="graphql-query-input"
                  rows={6}
                  value={queryText}
                  onChange={(e) => setQueryText(e.target.value)}
                  className="font-mono-tech text-xs"
                />
              </div>
              <div>
                <label className="block text-[11px] text-muted-foreground mb-1">
                  Variables (JSON)
                </label>
                <Textarea
                  rows={6}
                  value={variablesText}
                  onChange={(e) => setVariablesText(e.target.value)}
                  className="font-mono-tech text-xs"
                />
              </div>
            </div>

            {queryError && (
              <div className="rounded border border-red-500/30 bg-red-500/10 p-2 text-red-600 dark:text-red-400">
                {queryError}
              </div>
            )}

            {queryResponse && (
              <div>
                <div className="text-[11px] text-muted-foreground mb-1">
                  响应结果 (JSON)
                </div>
                <pre
                  data-testid="graphql-response-output"
                  className="max-h-56 overflow-auto rounded border border-border bg-muted/40 p-3 font-mono-tech text-[11px] leading-relaxed"
                >
                  {queryResponse}
                </pre>
              </div>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
