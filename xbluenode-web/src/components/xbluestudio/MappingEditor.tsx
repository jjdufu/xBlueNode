import { useEffect, useState } from "react";
import { Eye, Play, Plus, Trash2 } from "lucide-react";
import { useI18n } from "@/i18n";
import { usePrincipal } from "@/lib/permissions";
import { axhubApi, type Dataset } from "@/lib/api/axhub";
import {
  xblueApi,
  type EdgeTypeSpec,
  type LinkDirection,
  type MappingLinkSpec,
  type MappingMode,
  type MappingPreviewResponse,
  type MappingRecord,
  type MappingRunRecord,
  type MappingSpec,
  type NodeTypeSpec,
  type OnSourceDelete,
} from "@/lib/api/xblue";
import {
  Badge,
  Button,
  EmptyState,
  Input,
  Select,
  StatusBadge,
} from "@/components/ui/primitives";
import { cn } from "@/lib/utils";

export interface MappingEditorProps {
  activeSubgraph: string;
  mappings: MappingSpec[];
  persistedMappings: MappingRecord[];
  datasets: Dataset[];
  nodeTypes: NodeTypeSpec[];
  edgeTypes: EdgeTypeSpec[];
  selectedMappingIndex: number | null;
  onSelectMappingIndex: (index: number | null) => void;
  onChangeMappings: (next: MappingSpec[]) => void;
  onRefreshPersistedMappings: () => Promise<void>;
}

export function MappingEditor({
  activeSubgraph,
  mappings,
  persistedMappings,
  datasets,
  nodeTypes,
  edgeTypes,
  selectedMappingIndex,
  onSelectMappingIndex,
  onChangeMappings,
  onRefreshPersistedMappings,
}: MappingEditorProps) {
  const { tx, fmtNum } = useI18n();
  const { can } = usePrincipal();
  const canEdit = can("STUDIO_EDIT");

  const effectiveIndex =
    selectedMappingIndex !== null && mappings[selectedMappingIndex]
      ? selectedMappingIndex
      : mappings.length > 0
        ? 0
        : null;

  const currentMapping =
    effectiveIndex !== null ? (mappings[effectiveIndex] ?? null) : null;

  const [datasetColumns, setDatasetColumns] = useState<string[]>([]);
  const [previewData, setPreviewData] =
    useState<MappingPreviewResponse | null>(null);
  const [runs, setRuns] = useState<MappingRunRecord[]>([]);
  const [busyAction, setBusyAction] = useState<string | null>(null);
  const [actionMsg, setActionMsg] = useState<string | null>(null);

  const matchedDataset = datasets.find(
    (ds) =>
      ds.name === currentMapping?.dataset || ds.id === currentMapping?.dataset
  );

  const matchedRecord = currentMapping
    ? persistedMappings.find((r) => r.name === currentMapping.name)
    : undefined;

  useEffect(() => {
    let cancelled = false;
    if (!matchedDataset) {
      setDatasetColumns([]);
      return;
    }
    const schemaCols = Array.isArray(matchedDataset.schema_json)
      ? matchedDataset.schema_json.map((c) => c.name).filter(Boolean)
      : [];
    if (schemaCols.length > 0) {
      setDatasetColumns(schemaCols);
      return;
    }
    void axhubApi
      .listColumns(matchedDataset.id)
      .then((cols) => {
        if (!cancelled && Array.isArray(cols)) {
          setDatasetColumns(cols.map((c) => c.column_name).filter(Boolean));
        }
      })
      .catch(() => {
        if (!cancelled) setDatasetColumns([]);
      });
    return () => {
      cancelled = true;
    };
  }, [matchedDataset]);

  useEffect(() => {
    let cancelled = false;
    setPreviewData(null);
    setActionMsg(null);
    if (!matchedRecord?.id) {
      setRuns([]);
      return;
    }
    void xblueApi
      .listMappingRuns(matchedRecord.id)
      .then((items) => {
        if (!cancelled) setRuns(items);
      })
      .catch(() => {
        if (!cancelled) setRuns([]);
      });
    return () => {
      cancelled = true;
    };
  }, [matchedRecord?.id]);

  const addMapping = () => {
    if (!canEdit) return;
    const idx = mappings.length + 1;
    const defaultDs = datasets[0]?.name ?? "demo_orders";
    const defaultNt = nodeTypes[0]?.name ?? "Order";
    const next: MappingSpec = {
      name: `mapping_${idx}`,
      dataset: defaultDs,
      node_type: defaultNt,
      key_column: "id",
      title: "{id}",
      attributes: {},
      metrics: {},
      geo: { lng_column: null, lat_column: null },
      links: [],
      on_source_delete: "KEEP",
      mode: "INCREMENTAL",
    };
    const updated = [...mappings, next];
    onChangeMappings(updated);
    onSelectMappingIndex(updated.length - 1);
  };

  const updateCurrent = (patch: Partial<MappingSpec>) => {
    if (!canEdit || effectiveIndex === null) return;
    onChangeMappings(
      mappings.map((m, i) => (i === effectiveIndex ? { ...m, ...patch } : m))
    );
  };

  const deleteCurrent = () => {
    if (!canEdit || effectiveIndex === null) return;
    const next = mappings.filter((_, i) => i !== effectiveIndex);
    onChangeMappings(next);
    onSelectMappingIndex(next.length > 0 ? 0 : null);
  };

  const targetNodeSpec = nodeTypes.find(
    (nt) => nt.name === currentMapping?.node_type
  );
  const attrProps =
    targetNodeSpec?.properties.filter((p) => p.kind === "ATTRIBUTE") ?? [];
  const metricProps =
    targetNodeSpec?.properties.filter((p) => p.kind === "METRIC") ?? [];

  const setAttrColumn = (propName: string, colName: string) => {
    if (!currentMapping) return;
    const nextAttrs = { ...currentMapping.attributes };
    if (colName.trim()) {
      nextAttrs[propName] = colName.trim();
    } else {
      delete nextAttrs[propName];
    }
    updateCurrent({ attributes: nextAttrs });
  };

  const setMetricColumn = (propName: string, colName: string) => {
    if (!currentMapping) return;
    const nextMetrics = { ...currentMapping.metrics };
    if (colName.trim()) {
      nextMetrics[propName] = colName.trim();
    } else {
      delete nextMetrics[propName];
    }
    updateCurrent({ metrics: nextMetrics });
  };

  const addLinkRule = () => {
    if (!currentMapping || !canEdit) return;
    const firstEdge = edgeTypes[0];
    const nextLink: MappingLinkSpec = {
      link_type: firstEdge?.name ?? "FULFILLS",
      direction: "IN",
      target_type: firstEdge?.src_type_name ?? nodeTypes[0]?.name ?? "",
      target_key_column: "",
      create_missing_target: false,
    };
    updateCurrent({ links: [...currentMapping.links, nextLink] });
  };

  const updateLinkRule = (lIdx: number, patch: Partial<MappingLinkSpec>) => {
    if (!currentMapping) return;
    updateCurrent({
      links: currentMapping.links.map((lk, i) =>
        i === lIdx ? { ...lk, ...patch } : lk
      ),
    });
  };

  const removeLinkRule = (lIdx: number) => {
    if (!currentMapping) return;
    updateCurrent({
      links: currentMapping.links.filter((_, i) => i !== lIdx),
    });
  };

  const handlePreview = async () => {
    if (!matchedRecord?.id) return;
    setBusyAction("preview");
    setActionMsg(null);
    try {
      const resp = await xblueApi.previewMapping(matchedRecord.id);
      setPreviewData(resp);
      setActionMsg(
        tx(
          `抽样 ${resp.sampled_rows} 行：通过 ${resp.items.length} · 拒绝 ${resp.rows_rejected}`,
          `Sampled ${resp.sampled_rows} rows: ${resp.items.length} valid · ${resp.rows_rejected} rejected`
        )
      );
    } catch (err) {
      setActionMsg(
        err instanceof Error ? err.message : tx("预览失败", "Preview failed")
      );
    } finally {
      setBusyAction(null);
    }
  };

  const handleRun = async () => {
    if (!canEdit || !matchedRecord?.id || !currentMapping) return;
    setBusyAction("run");
    setActionMsg(null);
    try {
      const resp = await xblueApi.runMapping(
        matchedRecord.id,
        currentMapping.mode
      );
      setActionMsg(
        tx(
          `已提交物化任务 (${resp.mode}) · Job ${resp.job_id.slice(0, 8)}`,
          `Materialization queued (${resp.mode}) · Job ${resp.job_id.slice(0, 8)}`
        )
      );
      await onRefreshPersistedMappings();
      const latestRuns = await xblueApi.listMappingRuns(matchedRecord.id);
      setRuns(latestRuns);
    } catch (err) {
      setActionMsg(
        err instanceof Error
          ? err.message
          : tx("提交物化失败", "Failed to run mapping")
      );
    } finally {
      setBusyAction(null);
    }
  };

  const renderColumnControl = (
    value: string,
    onSelect: (val: string) => void,
    placeholder: string
  ) => {
    if (datasetColumns.length > 0) {
      return (
        <Select
          value={value}
          disabled={!canEdit}
          onChange={(e) => onSelect(e.target.value)}
          className="h-7 w-full px-1.5 font-mono-tech"
        >
          <option value="">{tx("未绑定", "Unbound")}</option>
          {datasetColumns.map((col) => (
            <option key={col} value={col}>
              {col}
            </option>
          ))}
        </Select>
      );
    }
    return (
      <Input
        value={value}
        disabled={!canEdit}
        onChange={(e) => onSelect(e.target.value)}
        placeholder={placeholder}
        className="h-7 px-2 text-xs font-mono-tech"
      />
    );
  };

  return (
    <div className="space-y-3 text-xs" data-testid="mapping-editor">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div className="flex flex-wrap items-center gap-1">
          {mappings.map((m, idx) => (
            <button
              key={`${m.name}-${idx}`}
              type="button"
              onClick={() => onSelectMappingIndex(idx)}
              className={cn(
                "rounded border px-2 py-1 font-mono-tech text-[11px] cursor-pointer",
                effectiveIndex === idx
                  ? "border-xblue bg-xblue-soft text-xblue"
                  : "border-border bg-background text-muted-foreground hover:text-foreground"
              )}
            >
              {m.name || `mapping_${idx + 1}`}
            </button>
          ))}
        </div>
        {canEdit && (
          <Button
            variant="outline"
            size="sm"
            data-testid="add-mapping-btn"
            onClick={addMapping}
          >
            <Plus className="h-3 w-3" />
            {tx("新建映射", "New Mapping")}
          </Button>
        )}
      </div>

      {!currentMapping ? (
        <EmptyState title={tx("暂无数据映射规则", "No data mappings")} />
      ) : (
        <div className="space-y-3 rounded border border-border p-3">
          <div className="grid grid-cols-2 gap-2">
            <div>
              <label className="block font-medium mb-1">
                {tx("映射名称", "Mapping Name")}
              </label>
              <Input
                value={currentMapping.name}
                disabled={!canEdit}
                onChange={(e) => updateCurrent({ name: e.target.value })}
                placeholder="orders_to_order_nodes"
                className="h-8 text-xs font-mono-tech"
              />
            </div>
            <div>
              <label className="block font-medium mb-1">
                {tx("源数据集", "Source Dataset")}
              </label>
              {datasets.length > 0 ? (
                <Select
                  value={currentMapping.dataset}
                  disabled={!canEdit}
                  onChange={(e) => updateCurrent({ dataset: e.target.value })}
                  className="h-8 w-full font-mono-tech"
                >
                  <option value="">{tx("未指定", "None")}</option>
                  {datasets.map((ds) => (
                    <option key={ds.id} value={ds.name}>
                      {ds.name} ({ds.display_name})
                    </option>
                  ))}
                </Select>
              ) : (
                <Input
                  value={currentMapping.dataset}
                  disabled={!canEdit}
                  onChange={(e) => updateCurrent({ dataset: e.target.value })}
                  placeholder="demo_orders"
                  className="h-8 text-xs font-mono-tech"
                />
              )}
            </div>
          </div>

          <div className="grid grid-cols-3 gap-2">
            <div>
              <label className="block font-medium mb-1">
                {tx("目标类型", "Target Type")}
              </label>
              <Select
                value={currentMapping.node_type}
                disabled={!canEdit}
                onChange={(e) => updateCurrent({ node_type: e.target.value })}
                className="h-7 w-full px-1.5 font-mono-tech"
              >
                <option value="">{tx("未指定", "None")}</option>
                {nodeTypes.map((nt) => (
                  <option key={nt.name} value={nt.name}>
                    {nt.name}
                  </option>
                ))}
              </Select>
            </div>
            <div>
              <label className="block font-medium mb-1">
                {tx("主键列", "Key Column")}
              </label>
              {renderColumnControl(
                currentMapping.key_column,
                (val) => updateCurrent({ key_column: val }),
                "id"
              )}
            </div>
            <div>
              <label className="block font-medium mb-1">
                {tx("标题模板", "Title Template")}
              </label>
              <Input
                value={currentMapping.title}
                disabled={!canEdit}
                onChange={(e) => updateCurrent({ title: e.target.value })}
                placeholder="{id}"
                className="h-7 px-2 text-xs font-mono-tech"
              />
            </div>
          </div>

          <div className="grid grid-cols-3 gap-2">
            <div>
              <label className="block font-medium mb-1">
                {tx("时间列", "Time Column")}
              </label>
              {renderColumnControl(
                currentMapping.metric_time_column ?? "",
                (val) => updateCurrent({ metric_time_column: val || null }),
                "updated_at"
              )}
            </div>
            <div>
              <label className="block font-medium mb-1">
                {tx("物化模式", "Sync Mode")}
              </label>
              <Select
                value={currentMapping.mode}
                disabled={!canEdit}
                onChange={(e) =>
                  updateCurrent({ mode: e.target.value as MappingMode })
                }
                className="h-7 w-full px-1.5 font-mono-tech"
              >
                <option value="INCREMENTAL">
                  {tx("增量更新", "Incremental")}
                </option>
                <option value="FULL">{tx("全量刷新", "Full Refresh")}</option>
              </Select>
            </div>
            <div>
              <label className="block font-medium mb-1">
                {tx("源行删除策略", "On Delete")}
              </label>
              <Select
                value={currentMapping.on_source_delete}
                disabled={!canEdit}
                onChange={(e) =>
                  updateCurrent({
                    on_source_delete: e.target.value as OnSourceDelete,
                  })
                }
                className="h-7 w-full px-1.5 font-mono-tech"
              >
                <option value="KEEP">{tx("保留节点", "Keep Node")}</option>
                <option value="DELETE">{tx("同步删除", "Delete Node")}</option>
              </Select>
            </div>
          </div>

          <div>
            <label className="block font-medium mb-1">
              {tx("源行过滤条件", "Row Filter")}
            </label>
            <Input
              value={currentMapping.filter ?? ""}
              disabled={!canEdit}
              onChange={(e) =>
                updateCurrent({ filter: e.target.value || null })
              }
              placeholder="status != 'TEST'"
              className="h-7 px-2 text-xs font-mono-tech"
            />
          </div>

          {/* Attribute & Metric Column Bindings */}
          {(attrProps.length > 0 || metricProps.length > 0) && (
            <div className="grid grid-cols-1 md:grid-cols-2 gap-2 border-t border-border pt-2">
              <div className="space-y-1.5">
                <div className="font-semibold text-[11px]">
                  {tx("静态属性列映射", "Attribute Bindings")}
                </div>
                {attrProps.map((p) => (
                  <div
                    key={p.name}
                    className="grid grid-cols-2 items-center gap-1.5"
                  >
                    <span className="font-mono-tech truncate">{p.name}</span>
                    {renderColumnControl(
                      currentMapping.attributes[p.name] ?? "",
                      (val) => setAttrColumn(p.name, val),
                      "column"
                    )}
                  </div>
                ))}
              </div>
              <div className="space-y-1.5">
                <div className="font-semibold text-[11px]">
                  {tx("时序指标列映射", "Metric Bindings")}
                </div>
                {metricProps.map((p) => (
                  <div
                    key={p.name}
                    className="grid grid-cols-2 items-center gap-1.5"
                  >
                    <span className="font-mono-tech text-xblue truncate">
                      {p.name}
                    </span>
                    {renderColumnControl(
                      currentMapping.metrics[p.name] ?? "",
                      (val) => setMetricColumn(p.name, val),
                      "metric_col"
                    )}
                  </div>
                ))}
              </div>
            </div>
          )}

          {/* Foreign-Key Relationship Links */}
          <div className="space-y-1.5 border-t border-border pt-2">
            <div className="flex items-center justify-between">
              <span className="font-semibold text-[11px]">
                {tx("外键关系映射", "Foreign-Key Relations")}
              </span>
              {canEdit && (
                <Button variant="ghost" size="sm" onClick={addLinkRule}>
                  <Plus className="h-3 w-3" />
                  {tx("添加外键边", "Add Link")}
                </Button>
              )}
            </div>
            {currentMapping.links.map((lk, lIdx) => (
              <div
                key={lIdx}
                className="grid grid-cols-12 items-center gap-1 rounded border border-border bg-muted/20 p-1.5"
              >
                <Input
                  value={lk.link_type}
                  disabled={!canEdit}
                  onChange={(e) =>
                    updateLinkRule(lIdx, { link_type: e.target.value })
                  }
                  placeholder="FULFILLS"
                  className="col-span-3 h-7 px-1.5 text-[11px] font-mono-tech"
                />
                <Select
                  value={lk.direction}
                  disabled={!canEdit}
                  onChange={(e) =>
                    updateLinkRule(lIdx, {
                      direction: e.target.value as LinkDirection,
                    })
                  }
                  className="col-span-2 h-7 px-1 text-[11px] font-mono-tech"
                >
                  <option value="OUT">OUT</option>
                  <option value="IN">IN</option>
                </Select>
                <Input
                  value={lk.target_type}
                  disabled={!canEdit}
                  onChange={(e) =>
                    updateLinkRule(lIdx, { target_type: e.target.value })
                  }
                  placeholder="Warehouse"
                  className="col-span-3 h-7 px-1.5 text-[11px] font-mono-tech"
                />
                <Input
                  value={lk.target_key_column}
                  disabled={!canEdit}
                  onChange={(e) =>
                    updateLinkRule(lIdx, {
                      target_key_column: e.target.value,
                    })
                  }
                  placeholder="warehouse_code"
                  className="col-span-3 h-7 px-1.5 text-[11px] font-mono-tech"
                />
                {canEdit && (
                  <button
                    type="button"
                    onClick={() => removeLinkRule(lIdx)}
                    className="col-span-1 flex justify-center text-muted-foreground hover:text-red-600 cursor-pointer"
                  >
                    <Trash2 className="h-3.5 w-3.5" />
                  </button>
                )}
              </div>
            ))}
          </div>

          {/* Preview 50 rows & Manual Materialize Run */}
          <div className="flex flex-wrap items-center justify-between gap-2 border-t border-border pt-2.5">
            <div className="flex items-center gap-1.5">
              <Button
                variant="outline"
                size="sm"
                data-testid="mapping-preview-btn"
                disabled={!matchedRecord || busyAction !== null}
                onClick={() => void handlePreview()}
              >
                <Eye className="h-3.5 w-3.5" />
                {busyAction === "preview"
                  ? tx("预览中…", "Previewing…")
                  : tx("预览 50 行", "Preview 50 Rows")}
              </Button>
              {canEdit && (
                <Button
                  variant="primary"
                  size="sm"
                  data-testid="mapping-run-btn"
                  disabled={
                    !matchedRecord ||
                    activeSubgraph !== "main" ||
                    busyAction !== null
                  }
                  onClick={() => void handleRun()}
                >
                  <Play className="h-3.5 w-3.5" />
                  {busyAction === "run"
                    ? tx("提交中…", "Submitting…")
                    : tx("运行物化", "Materialize")}
                </Button>
              )}
            </div>

            {canEdit && (
              <Button variant="danger" size="sm" onClick={deleteCurrent}>
                <Trash2 className="h-3.5 w-3.5" />
                {tx("删除映射", "Delete")}
              </Button>
            )}
          </div>

          {!matchedRecord && (
            <div className="text-[11px] text-muted-foreground">
              {tx(
                "未发布映射草案：发布版本后支持预览与物化",
                "Unpublished mapping draft: publish schema to preview or materialize"
              )}
            </div>
          )}

          {actionMsg && (
            <div className="rounded border border-border bg-muted/40 px-2.5 py-1.5 font-mono-tech text-[11px]">
              {actionMsg}
            </div>
          )}

          {previewData && (
            <div
              className="space-y-2 rounded border border-border bg-muted/20 p-2.5"
              data-testid="mapping-preview-result"
            >
              <div className="flex items-center justify-between">
                <span className="font-semibold">
                  {tx(
                    `预览结果 (${previewData.items.length} 条有效节点)`,
                    `Preview (${previewData.items.length} valid nodes)`
                  )}
                </span>
                <Badge
                  variant={previewData.rows_rejected > 0 ? "warning" : "xblue"}
                >
                  {tx(
                    `拒绝 ${previewData.rows_rejected} 行`,
                    `${previewData.rows_rejected} rejected`
                  )}
                </Badge>
              </div>
              {previewData.items.length > 0 && (
                <div className="max-h-40 overflow-auto rounded border border-border bg-background">
                  <table className="w-full text-left border-collapse text-[11px] font-mono-tech">
                    <thead className="bg-muted/60 text-muted-foreground">
                      <tr>
                        <th className="px-2 py-1">{tx("主键", "Key")}</th>
                        <th className="px-2 py-1">{tx("标题", "Title")}</th>
                        <th className="px-2 py-1">{tx("属性", "Attributes")}</th>
                        <th className="px-2 py-1">{tx("指标", "Metrics")}</th>
                      </tr>
                    </thead>
                    <tbody className="divide-y divide-border">
                      {previewData.items.slice(0, 10).map((item, idx) => (
                        <tr key={`${item.node_key}-${idx}`}>
                          <td className="px-2 py-1 font-semibold">
                            {item.node_key}
                          </td>
                          <td className="px-2 py-1">{item.title}</td>
                          <td className="px-2 py-1">
                            <div className="flex flex-wrap gap-1">
                              {Object.entries(item.attributes ?? {}).map(
                                ([ak, av]) => (
                                  <Badge key={ak} variant="outline">
                                    {ak}: {String(av ?? "")}
                                  </Badge>
                                )
                              )}
                            </div>
                          </td>
                          <td className="px-2 py-1">
                            <div className="flex flex-wrap gap-1">
                              {Object.entries(item.metrics ?? {}).map(
                                ([mk, mv]) => (
                                  <Badge key={mk} variant="xblue">
                                    {mk}:{" "}
                                    {typeof mv === "number"
                                      ? fmtNum(mv, 2)
                                      : String(mv ?? "")}
                                  </Badge>
                                )
                              )}
                            </div>
                          </td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              )}
              {previewData.reject_samples.length > 0 && (
                <div className="space-y-1">
                  <div className="font-medium text-amber-600 dark:text-amber-400">
                    {tx("拒绝样本", "Rejected Samples")} (
                    {previewData.reject_samples.length})
                  </div>
                  {previewData.reject_samples.slice(0, 5).map((rej, idx) => (
                    <div
                      key={idx}
                      className="rounded border border-amber-500/30 bg-amber-500/10 px-2 py-1 font-mono-tech text-[10px]"
                    >
                      {rej.reason}
                    </div>
                  ))}
                </div>
              )}
            </div>
          )}

          {runs.length > 0 && (
            <div className="space-y-1 border-t border-border pt-2">
              <div className="font-semibold text-[11px]">
                {tx("最近物化记录", "Recent Runs")}
              </div>
              {runs.slice(0, 3).map((r) => (
                <div
                  key={r.id}
                  className="flex items-center justify-between rounded border border-border px-2 py-1 font-mono-tech text-[11px]"
                >
                  <span className="inline-flex items-center gap-1.5">
                    <StatusBadge status={r.status} />
                    <span>{r.mode}</span>
                  </span>
                  <span className="text-muted-foreground">
                    +{r.nodes_upserted} {tx("节点", "nodes")} / +
                    {r.links_upserted} {tx("边", "edges")}
                  </span>
                </div>
              ))}
            </div>
          )}
        </div>
      )}
    </div>
  );
}
