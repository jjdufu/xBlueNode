import { useCallback, useEffect, useMemo, useState } from "react";
import { Cable, Database, FileUp, Search } from "lucide-react";
import {
  axhubApi,
  type Connection,
  type ConnectorSpec,
  type Dataset,
} from "@/lib/api/axhub";
import type { SourceKind } from "@/lib/api/enums";
import { Badge } from "@/components/ui/primitives";
import { CatalogSearch } from "@/components/axhub/CatalogSearch";
import { ConnectionForm } from "@/components/axhub/ConnectionForm";
import {
  type DatasetDetailTab,
  DatasetTable,
} from "@/components/axhub/DatasetTable";
import { LineageGraph } from "@/components/axhub/LineageGraph";
import { SyncHistory } from "@/components/axhub/SyncHistory";
import { UploadDropzone } from "@/components/axhub/UploadDropzone";

type HubWorkspaceView = "workspace" | "uploads" | "catalog";

export function AxHubPage() {
  const [activeView, setActiveView] = useState<HubWorkspaceView>("workspace");
  const [connectorSpecs, setConnectorSpecs] = useState<ConnectorSpec[]>([]);
  const [connections, setConnections] = useState<Connection[]>([]);
  const [datasets, setDatasets] = useState<Dataset[]>([]);
  const [selectedDatasetId, setSelectedDatasetId] = useState<string | null>(null);
  const [detailTab, setDetailTab] = useState<DatasetDetailTab>("preview");
  const [datasetFilters, setDatasetFilters] = useState<{
    q?: string;
    tag?: string;
    source_kind?: SourceKind;
  }>({});

  const loadConnectionsAndSpecs = useCallback(async () => {
    const [specsRes, connsRes] = await Promise.all([
      axhubApi.listConnectorTypes().catch(() => []),
      axhubApi.listConnections().catch(() => ({ items: [], next_cursor: null })),
    ]);
    setConnectorSpecs(Array.isArray(specsRes) ? specsRes : []);
    setConnections(Array.isArray(connsRes?.items) ? connsRes.items : []);
  }, []);

  const loadDatasets = useCallback(
    async (filters = datasetFilters) => {
      try {
        const dsRes = await axhubApi.listDatasets({
          ...filters,
          limit: 100,
        });
        const items = Array.isArray(dsRes?.items) ? dsRes.items : [];
        setDatasets(items);
        setSelectedDatasetId((prev) => {
          if (prev && items.some((item) => item.id === prev)) {
            return prev;
          }
          return items[0]?.id ?? null;
        });
      } catch {
        setDatasets([]);
      }
    },
    [datasetFilters]
  );

  useEffect(() => {
    void loadConnectionsAndSpecs();
    void loadDatasets();
  }, [loadConnectionsAndSpecs, loadDatasets]);

  const connectionNamesMap = useMemo(() => {
    const map: Record<string, string> = {};
    for (const c of connections) {
      map[c.id] = c.name;
    }
    return map;
  }, [connections]);

  const handleFilterChange = async (nextFilters: {
    q?: string;
    tag?: string;
    source_kind?: SourceKind;
  }) => {
    setDatasetFilters(nextFilters);
    await loadDatasets(nextFilters);
  };

  const handleJumpToDataset = (datasetId: string) => {
    setSelectedDatasetId(datasetId);
    setDetailTab("preview");
    setActiveView("workspace");
  };

  return (
    <div className="space-y-5" data-testid="axhub-page">
      {/* Page Header & Workspace Switcher */}
      <div className="flex flex-wrap items-center justify-between gap-3 border-b border-border pb-4">
        <div>
          <h1 className="text-lg font-semibold tracking-tight">
            AxHub · 数据接入与资产目录
          </h1>
          <p className="text-xs text-muted-foreground mt-0.5">
            多源连接器、DuckLake 同步、质量规则与全局资产血缘目录
          </p>
        </div>

        <div className="flex flex-wrap items-center gap-2">
          <Badge variant="neutral">连接 {connections.length}</Badge>
          <Badge variant="xblue">数据集 {datasets.length}</Badge>

          <div className="flex items-center rounded-md border border-border bg-muted/40 p-0.5">
            <button
              type="button"
              onClick={() => setActiveView("workspace")}
              data-testid="axhub-nav-workspace"
              className={`inline-flex items-center gap-1.5 rounded px-3 py-1.5 text-xs font-medium transition-colors cursor-pointer ${
                activeView === "workspace"
                  ? "bg-background text-foreground border border-border"
                  : "text-muted-foreground hover:text-foreground"
              }`}
            >
              <Cable className="h-3.5 w-3.5 text-xblue" />
              连接与数据集
            </button>
            <button
              type="button"
              onClick={() => setActiveView("uploads")}
              data-testid="axhub-nav-uploads"
              className={`inline-flex items-center gap-1.5 rounded px-3 py-1.5 text-xs font-medium transition-colors cursor-pointer ${
                activeView === "uploads"
                  ? "bg-background text-foreground border border-border"
                  : "text-muted-foreground hover:text-foreground"
              }`}
            >
              <FileUp className="h-3.5 w-3.5 text-xblue" />
              文件上传入湖
            </button>
            <button
              type="button"
              onClick={() => setActiveView("catalog")}
              data-testid="axhub-nav-catalog"
              className={`inline-flex items-center gap-1.5 rounded px-3 py-1.5 text-xs font-medium transition-colors cursor-pointer ${
                activeView === "catalog"
                  ? "bg-background text-foreground border border-border"
                  : "text-muted-foreground hover:text-foreground"
              }`}
            >
              <Search className="h-3.5 w-3.5 text-xblue" />
              资产目录与术语
            </button>
          </div>
        </div>
      </div>

      {activeView === "workspace" && (
        <div className="space-y-5">
          <ConnectionForm
            connections={connections}
            connectorSpecs={connectorSpecs}
            onChanged={loadConnectionsAndSpecs}
            onDatasetCreated={() => loadDatasets()}
          />

          <DatasetTable
            datasets={datasets}
            selectedDatasetId={selectedDatasetId}
            activeDetailTab={detailTab}
            onSelectDataset={(id) => setSelectedDatasetId(id)}
            onChangeDetailTab={(tab) => setDetailTab(tab)}
            onFilterChange={handleFilterChange}
            onRefreshDatasets={() => loadDatasets()}
            renderHistorySlot={(ds) => (
              <SyncHistory
                dataset={ds}
                onDatasetUpdated={() => loadDatasets()}
              />
            )}
            renderLineageSlot={(ds) => (
              <LineageGraph
                dataset={ds}
                connectionNames={connectionNamesMap}
              />
            )}
          />
        </div>
      )}

      {activeView === "uploads" && (
        <div className="space-y-4">
          <UploadDropzone
            onCommitted={() => loadDatasets()}
            onSelectDatasetId={(dsId) => handleJumpToDataset(dsId)}
          />
          <div className="flex items-center gap-2 text-xs text-muted-foreground">
            <Database className="h-3.5 w-3.5 text-xblue" />
            <span>
              提示：文件完成解析并提交为 COMMITTED 后，自动生成的表格轨数据集会同步出现在「连接与数据集」列表中。
            </span>
          </div>
        </div>
      )}

      {activeView === "catalog" && (
        <CatalogSearch
          onSelectDataset={(dsId) => handleJumpToDataset(dsId)}
        />
      )}
    </div>
  );
}
