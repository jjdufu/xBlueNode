import { useEffect, useState } from "react";
import { Eye, Play, Plus, Trash2 } from "lucide-react";
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
import { Badge, Button, Input } from "@/components/ui/primitives";
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
  const effectiveIndex =
    selectedMappingIndex !== null && mappings[selectedMappingIndex]
      ? selectedMappingIndex
      : mappings.length > 0
        ? 0
        : null;

  const currentMapping =
    effectiveIndex !== null ? mappings[effectiveIndex] ?? null : null;

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
    if (effectiveIndex === null) return;
    onChangeMappings(
      mappings.map((m, i) => (i === effectiveIndex ? { ...m, ...patch } : m))
    );
  };

  const deleteCurrent = () => {
    if (effectiveIndex === null) return;
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
    if (!currentMapping) return;
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
        `已预览 ${resp.sampled_rows} 行源数据：成功 ${resp.items.length} 行，拒绝 ${resp.rows_rejected} 行`
      );
    } catch (err) {
      setActionMsg(err instanceof Error ? err.message : "预览执行失败");
    } finally {
      setBusyAction(null);
    }
  };

  const handleRun = async () => {
    if (!matchedRecord?.id || !currentMapping) return;
    setBusyAction("run");
    setActionMsg(null);
    try {
      const resp = await xblueApi.runMapping(
        matchedRecord.id,
        currentMapping.mode
      );
      setActionMsg(
        `已提交物化任务 (${resp.mode}) · Job ${resp.job_id.slice(0, 8)}`
      );
      await onRefreshPersistedMappings();
      const latestRuns = await xblueApi.listMappingRuns(matchedRecord.id);
      setRuns(latestRuns);
    } catch (err) {
      setActionMsg(err instanceof Error ? err.message : "触发物化任务失败");
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
        <select
          value={value}
          onChange={(e) => onSelect(e.target.value)}
          className="h-7 w-full rounded border border-input bg-background px-1.5 text-xs font-mono-tech"
        >
          <option value="">-- 未绑定 --</option>
          {datasetColumns.map((col) => (
            <option key={col} value={col}>
              {col}
            </option>
          ))}
        </select>
      );
    }
    return (
      <Input
        value={value}
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
        <Button
          variant="outline"
          className="h-7 px-2 py-1 text-xs"
          data-testid="add-mapping-btn"
          onClick={addMapping}
        >
          <Plus className="h-3 w-3" />
          新建数据映射
        </Button>
      </div>

      {!currentMapping ? (
        <div className="rounded border border-dashed border-border p-4 text-center text-muted-foreground">
          当前子图尚未配置 AxHub 数据集到图节点的映射规则。
        </div>
      ) : (
        <div className="space-y-3 rounded border border-border p-3">
          <div className="grid grid-cols-2 gap-2">
            <div>
              <label className="block font-medium mb-1">映射名称 (name)</label>
              <Input
                value={currentMapping.name}
                onChange={(e) => updateCurrent({ name: e.target.value })}
                placeholder="orders_to_order_nodes"
                className="h-8 text-xs font-mono-tech"
              />
            </div>
            <div>
              <label className="block font-medium mb-1">
                源 AxHub 数据集 (dataset)
              </label>
              {datasets.length > 0 ? (
                <select
                  value={currentMapping.dataset}
                  onChange={(e) => updateCurrent({ dataset: e.target.value })}
                  className="h-8 w-full rounded border border-input bg-background px-2 text-xs font-mono-tech"
                >
                  <option value="">选择数据集...</option>
                  {datasets.map((ds) => (
                    <option key={ds.id} value={ds.name}>
                      {ds.name} ({ds.display_name} · {ds.row_count}行)
                    </option>
                  ))}
                </select>
              ) : (
                <Input
                  value={currentMapping.dataset}
                  onChange={(e) => updateCurrent({ dataset: e.target.value })}
                  placeholder="demo_orders"
                  className="h-8 text-xs font-mono-tech"
                />
              )}
            </div>
          </div>

          <div className="grid grid-cols-3 gap-2">
            <div>
              <label className="block font-medium mb-1">目标节点类型</label>
              <select
                value={currentMapping.node_type}
                onChange={(e) => updateCurrent({ node_type: e.target.value })}
                className="h-7 w-full rounded border border-input bg-background px-1.5 text-xs font-mono-tech"
              >
                <option value="">选择类型...</option>
                {nodeTypes.map((nt) => (
                  <option key={nt.name} value={nt.name}>
                    {nt.name}
                  </option>
                ))}
              </select>
            </div>
            <div>
              <label className="block font-medium mb-1">
                主键列 (key_column)
              </label>
              {renderColumnControl(
                currentMapping.key_column,
                (val) => updateCurrent({ key_column: val }),
                "id"
              )}
            </div>
            <div>
              <label className="block font-medium mb-1">标题模板 (title)</label>
              <Input
                value={currentMapping.title}
                onChange={(e) => updateCurrent({ title: e.target.value })}
                placeholder="订单 {id}"
                className="h-7 px-2 text-xs font-mono-tech"
              />
            </div>
          </div>

          <div className="grid grid-cols-3 gap-2">
            <div>
              <label className="block font-medium mb-1">
                指标时间列 (metric_time_column)
              </label>
              {renderColumnControl(
                currentMapping.metric_time_column ?? "",
                (val) => updateCurrent({ metric_time_column: val || null }),
                "updated_at"
              )}
            </div>
            <div>
              <label className="block font-medium mb-1">物化模式 (mode)</label>
              <select
                value={currentMapping.mode}
                onChange={(e) =>
                  updateCurrent({ mode: e.target.value as MappingMode })
                }
                className="h-7 w-full rounded border border-input bg-background px-1.5 text-xs font-mono-tech"
              >
                <option value="INCREMENTAL">INCREMENTAL (增量)</option>
                <option value="FULL">FULL (全量)</option>
              </select>
            </div>
            <div>
              <label className="block font-medium mb-1">
                源行删除策略 (on_source_delete)
              </label>
              <select
                value={currentMapping.on_source_delete}
                onChange={(e) =>
                  updateCurrent({
                    on_source_delete: e.target.value as OnSourceDelete,
                  })
                }
                className="h-7 w-full rounded border border-input bg-background px-1.5 text-xs font-mono-tech"
              >
                <option value="KEEP">KEEP (保留节点)</option>
                <option value="DELETE">DELETE (同步删除)</option>
              </select>
            </div>
          </div>

          <div>
            <label className="block font-medium mb-1">
              源行过滤条件 (filter，可选)
            </label>
            <Input
              value={currentMapping.filter ?? ""}
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
                  静态属性列映射 (attributes)
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
                      "源表列名"
                    )}
                  </div>
                ))}
              </div>
              <div className="space-y-1.5">
                <div className="font-semibold text-[11px]">
                  数值指标列映射 (metrics)
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
                      "源表数值列"
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
                外键关系映射 (links)
              </span>
              <Button
                variant="ghost"
                className="h-6 px-2 py-0.5 text-[11px]"
                onClick={addLinkRule}
              >
                <Plus className="h-3 w-3" />
                添加外键边
              </Button>
            </div>
            {currentMapping.links.map((lk, lIdx) => (
              <div
                key={lIdx}
                className="grid grid-cols-12 items-center gap-1 rounded border border-border bg-muted/20 p-1.5"
              >
                <Input
                  value={lk.link_type}
                  onChange={(e) =>
                    updateLinkRule(lIdx, { link_type: e.target.value })
                  }
                  placeholder="FULFILLS"
                  className="col-span-3 h-7 px-1.5 text-[11px] font-mono-tech"
                />
                <select
                  value={lk.direction}
                  onChange={(e) =>
                    updateLinkRule(lIdx, {
                      direction: e.target.value as LinkDirection,
                    })
                  }
                  className="col-span-2 h-7 rounded border border-input bg-background px-1 text-[11px] font-mono-tech"
                >
                  <option value="OUT">OUT</option>
                  <option value="IN">IN</option>
                </select>
                <Input
                  value={lk.target_type}
                  onChange={(e) =>
                    updateLinkRule(lIdx, { target_type: e.target.value })
                  }
                  placeholder="Warehouse"
                  className="col-span-3 h-7 px-1.5 text-[11px] font-mono-tech"
                />
                <Input
                  value={lk.target_key_column}
                  onChange={(e) =>
                    updateLinkRule(lIdx, {
                      target_key_column: e.target.value,
                    })
                  }
                  placeholder="warehouse_code"
                  className="col-span-3 h-7 px-1.5 text-[11px] font-mono-tech"
                />
                <button
                  type="button"
                  onClick={() => removeLinkRule(lIdx)}
                  className="col-span-1 flex justify-center text-muted-foreground hover:text-red-600 cursor-pointer"
                >
                  <Trash2 className="h-3.5 w-3.5" />
                </button>
              </div>
            ))}
          </div>

          {/* Preview 50 rows & Manual Materialize Run */}
          <div className="flex flex-wrap items-center justify-between gap-2 border-t border-border pt-2.5">
            <div className="flex items-center gap-1.5">
              <Button
                variant="outline"
                className="h-7 px-2.5 py-1 text-xs"
                data-testid="mapping-preview-btn"
                disabled={!matchedRecord || busyAction !== null}
                onClick={() => void handlePreview()}
                title={
                  matchedRecord
                    ? "干运行预览前 50 行映射结果与拒绝样本（不写库）"
                    : "请先发布草案以持久化该映射后再执行在线预览"
                }
              >
                <Eye className="h-3.5 w-3.5" />
                {busyAction === "preview" ? "预览中..." : "预览前 50 行"}
              </Button>
              <Button
                variant="primary"
                className="h-7 px-2.5 py-1 text-xs"
                data-testid="mapping-run-btn"
                disabled={
                  !matchedRecord ||
                  activeSubgraph !== "main" ||
                  busyAction !== null
                }
                onClick={() => void handleRun()}
                title={
                  activeSubgraph !== "main"
                    ? "部门子图仅支持映射预览，合并至主图后可执行物化"
                    : "立即入队执行 MAP_MATERIALIZE 物化任务"
                }
              >
                <Play className="h-3.5 w-3.5" />
                {busyAction === "run" ? "提交中..." : "立即运行物化"}
              </Button>
            </div>

            <button
              type="button"
              onClick={deleteCurrent}
              className="inline-flex items-center gap-1 rounded px-2 py-1 text-red-600 dark:text-red-400 hover:bg-red-500/10 cursor-pointer"
            >
              <Trash2 className="h-3.5 w-3.5" />
              删除映射
            </button>
          </div>

          {!matchedRecord && (
            <div className="text-[11px] text-muted-foreground">
              提示：当前映射尚处于未发布草案状态，发布版本后即可执行前 50 行干运行预览与物化调度。
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
                  前 50 行干运行预览 ({previewData.items.length} 条有效节点)
                </span>
                <Badge
                  variant={previewData.rows_rejected > 0 ? "neutral" : "xblue"}
                >
                  拒绝 {previewData.rows_rejected} 行
                </Badge>
              </div>
              {previewData.items.length > 0 && (
                <div className="max-h-40 overflow-auto rounded border border-border bg-background">
                  <table className="w-full text-left border-collapse text-[11px] font-mono-tech">
                    <thead className="bg-muted/60 text-muted-foreground">
                      <tr>
                        <th className="px-2 py-1">node_key</th>
                        <th className="px-2 py-1">title</th>
                        <th className="px-2 py-1">attributes</th>
                        <th className="px-2 py-1">metrics</th>
                      </tr>
                    </thead>
                    <tbody className="divide-y divide-border">
                      {previewData.items.slice(0, 10).map((item, idx) => (
                        <tr key={`${item.node_key}-${idx}`}>
                          <td className="px-2 py-1 font-semibold">
                            {item.node_key}
                          </td>
                          <td className="px-2 py-1">{item.title}</td>
                          <td className="px-2 py-1 truncate max-w-[160px]">
                            {JSON.stringify(item.attributes)}
                          </td>
                          <td className="px-2 py-1 text-xblue">
                            {JSON.stringify(item.metrics)}
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
                    拒绝样本 ({previewData.reject_samples.length})
                  </div>
                  {previewData.reject_samples.slice(0, 5).map((rej, idx) => (
                    <div
                      key={idx}
                      className="rounded border border-amber-500/30 bg-amber-500/10 px-2 py-1 font-mono-tech text-[10px]"
                    >
                      原因: {rej.reason} · 行: {JSON.stringify(rej.row)}
                    </div>
                  ))}
                </div>
              )}
            </div>
          )}

          {runs.length > 0 && (
            <div className="space-y-1 border-t border-border pt-2">
              <div className="font-semibold text-[11px]">最近物化运行记录</div>
              {runs.slice(0, 3).map((r) => (
                <div
                  key={r.id}
                  className="flex items-center justify-between rounded border border-border px-2 py-1 font-mono-tech text-[11px]"
                >
                  <span>
                    {r.status} · {r.mode}
                  </span>
                  <span className="text-muted-foreground">
                    +{r.nodes_upserted} 节点 / +{r.links_upserted} 边 / 拒{" "}
                    {r.rows_rejected}
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
