import { useCallback, useEffect, useMemo, useState } from "react";
import { Cable, FileUp, Search } from "lucide-react";
import { useI18n } from "@/i18n";
import { MODULE_META } from "@/modules/meta";
import {
  axhubApi,
  type Connection,
  type ConnectorSpec,
  type Dataset,
} from "@/lib/api/axhub";
import type { SourceKind } from "@/lib/api/enums";
import {
  Badge,
  PageHeader,
  SegmentedControl,
} from "@/components/ui/primitives";
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
  const { tx, fmtNum } = useI18n();

  const [activeView, setActiveView] = useState<HubWorkspaceView>("workspace");
  const [connectorSpecs, setConnectorSpecs] = useState<ConnectorSpec[]>([]);
  const [connections, setConnections] = useState<Connection[]>([]);
  const [datasets, setDatasets] = useState<Dataset[]>([]);
  const [selectedDatasetId, setSelectedDatasetId] = useState<string | null>(
    null
  );
  const [detailTab, setDetailTab] = useState<DatasetDetailTab>("preview");
  const [datasetFilters, setDatasetFilters] = useState<{
    q?: string;
    tag?: string;
    source_kind?: SourceKind;
  }>({});

  const loadConnectionsAndSpecs = useCallback(async () => {
    const [specsRes, connsRes] = await Promise.all([
      axhubApi.listConnectorTypes().catch(() => []),
      axhubApi
        .listConnections()
        .catch(() => ({ items: [], next_cursor: null })),
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
      <PageHeader
        title={tx(MODULE_META.axhub.label, MODULE_META.axhub.enLabel)}
        badges={
          <>
            <Badge variant="neutral">
              {tx("连接", "Connections")} {fmtNum(connections.length, 0)}
            </Badge>
            <Badge variant="xblue">
              {tx("数据集", "Datasets")} {fmtNum(datasets.length, 0)}
            </Badge>
          </>
        }
        actions={
          <SegmentedControl
            value={activeView}
            onChange={(next) => setActiveView(next)}
            items={[
              {
                key: "workspace",
                label: tx("连接与数据集", "Workspace"),
                icon: <Cable className="h-3.5 w-3.5 text-xblue" />,
                testId: "axhub-nav-workspace",
              },
              {
                key: "uploads",
                label: tx("文件入湖", "File Upload"),
                icon: <FileUp className="h-3.5 w-3.5 text-xblue" />,
                testId: "axhub-nav-uploads",
              },
              {
                key: "catalog",
                label: tx("资产目录", "Catalog"),
                icon: <Search className="h-3.5 w-3.5 text-xblue" />,
                testId: "axhub-nav-catalog",
              },
            ]}
          />
        }
      />

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
        <UploadDropzone
          onCommitted={() => loadDatasets()}
          onSelectDatasetId={(dsId) => handleJumpToDataset(dsId)}
        />
      )}

      {activeView === "catalog" && (
        <CatalogSearch onSelectDataset={(dsId) => handleJumpToDataset(dsId)} />
      )}
    </div>
  );
}
