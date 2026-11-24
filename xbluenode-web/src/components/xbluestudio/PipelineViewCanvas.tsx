import { useMemo, useState } from "react";
import {
  ArrowRight,
  Calculator,
  Database,
  GitMerge,
  Plus,
} from "lucide-react";
import { useI18n } from "@/i18n";
import { usePrincipal } from "@/lib/permissions";
import type { MappingSpec, SchemaDoc } from "@/lib/api/xblue";
import { Badge, Button, Card, Input, Select } from "@/components/ui/primitives";
import { cn } from "@/lib/utils";
import { renderNodeIcon } from "./NodeTypeCard";

export interface PipelineViewCanvasProps {
  workingDoc: SchemaDoc;
  datasetNames: readonly string[];
  selectedNodeName: string | null;
  selectedMappingIndex: number | null;
  onSelectNode: (name: string) => void;
  onSelectMapping: (index: number) => void;
  onCreateMapping: (mapping: MappingSpec) => void;
}

/**
 * Four-stage left-to-right `MAPPING_PIPELINE` view synchronized with `workingDoc`
 * (`数据集源表 → 字段映射规则 → 业务对象类型 → 聚合与派生规则`).
 */
export function PipelineViewCanvas({
  workingDoc,
  datasetNames,
  selectedNodeName,
  selectedMappingIndex,
  onSelectNode,
  onSelectMapping,
  onCreateMapping,
}: PipelineViewCanvasProps) {
  const { tx, tEnum } = useI18n();
  const { can } = usePrincipal();
  const canEdit = can("STUDIO_EDIT");

  const [quickAddOpen, setQuickAddOpen] = useState(false);
  const [draftMapName, setDraftMapName] = useState("");
  const [draftDataset, setDraftDataset] = useState("");
  const [draftNodeType, setDraftNodeType] = useState("");
  const [draftKeyCol, setDraftKeyCol] = useState("id");

  const uniqueDatasets = useMemo(() => {
    const set = new Set<string>();
    for (const m of workingDoc.mappings) {
      if (m.dataset.trim()) set.add(m.dataset.trim());
    }
    for (const ds of datasetNames) {
      if (ds.trim()) set.add(ds.trim());
    }
    return Array.from(set);
  }, [datasetNames, workingDoc.mappings]);

  const ruleEntries = useMemo(() => {
    const list: Array<{
      id: string;
      nodeType: string;
      metric: string;
      kind: "AGGREGATION" | "DERIVED";
      detail: string;
    }> = [];
    for (const nt of workingDoc.node_types) {
      for (const agg of nt.agg_rules) {
        list.push({
          id: `agg:${nt.name}:${agg.target_metric}`,
          nodeType: nt.name,
          metric: agg.target_metric,
          kind: "AGGREGATION",
          detail: `${tEnum(agg.op)}(${agg.source_type_name}.${agg.source_metric})`,
        });
      }
      for (const drv of nt.derived_rules) {
        list.push({
          id: `drv:${nt.name}:${drv.target_metric}`,
          nodeType: nt.name,
          metric: drv.target_metric,
          kind: "DERIVED",
          detail: drv.expression,
        });
      }
    }
    return list;
  }, [tEnum, workingDoc.node_types]);

  const submitQuickMapping = () => {
    const targetNode =
      draftNodeType || workingDoc.node_types[0]?.name || "";
    const dataset =
      draftDataset.trim() || uniqueDatasets[0] || "demo_orders";
    const name =
      draftMapName.trim() ||
      `${dataset}_to_${targetNode.toLowerCase()}_${workingDoc.mappings.length + 1}`;
    if (!targetNode || !dataset) return;
    onCreateMapping({
      name,
      dataset,
      node_type: targetNode,
      key_column: draftKeyCol.trim() || "id",
      title: `{${draftKeyCol.trim() || "id"}}`,
      attributes: {},
      metrics: {},
      geo: { lng_column: null, lat_column: null },
      links: [],
      on_source_delete: "KEEP",
      mode: "INCREMENTAL",
    });
    setDraftMapName("");
    setQuickAddOpen(false);
  };

  return (
    <div
      data-testid="studio-pipeline-canvas"
      className="flex-1 min-h-0 w-full overflow-auto p-3.5 bg-muted/10"
    >
      <div className="mb-3 flex flex-wrap items-center justify-between gap-2">
        <div className="flex items-center gap-2 text-xs font-medium text-foreground">
          <GitMerge className="h-3.5 w-3.5 text-xblue" />
          <span>{tx("同源映射流水线", "Mapping Pipeline")}</span>
          <Badge variant="neutral" className="tabular-nums">
            {workingDoc.mappings.length}
          </Badge>
        </div>
        {canEdit && workingDoc.node_types.length > 0 && (
          <Button
            type="button"
            variant="outline"
            size="sm"
            data-testid="pipeline-add-mapping-btn"
            onClick={() => {
              setDraftDataset(uniqueDatasets[0] ?? "demo_orders");
              setDraftNodeType(
                selectedNodeName ?? workingDoc.node_types[0]?.name ?? ""
              );
              setQuickAddOpen((v) => !v);
            }}
          >
            <Plus className="h-3.5 w-3.5" />
            {tx("新增映射", "Add Mapping")}
          </Button>
        )}
      </div>

      {quickAddOpen && canEdit && (
        <Card
          data-testid="pipeline-quick-mapping-form"
          className="mb-3 p-3 space-y-2 text-xs"
        >
          <div className="grid grid-cols-1 md:grid-cols-4 gap-2">
            <div>
              <label className="block text-[11px] text-muted-foreground mb-1">
                {tx("映射名称", "Mapping Name")}
              </label>
              <Input
                data-testid="pipeline-mapping-name-input"
                value={draftMapName}
                onChange={(e) => setDraftMapName(e.target.value)}
                placeholder="orders_to_order"
                className="h-8 text-xs font-mono-tech"
              />
            </div>
            <div>
              <label className="block text-[11px] text-muted-foreground mb-1">
                {tx("源数据集", "Dataset")}
              </label>
              <Input
                data-testid="pipeline-mapping-dataset-input"
                value={draftDataset}
                onChange={(e) => setDraftDataset(e.target.value)}
                placeholder="demo_orders"
                className="h-8 text-xs font-mono-tech"
              />
            </div>
            <div>
              <label className="block text-[11px] text-muted-foreground mb-1">
                {tx("目标对象", "Object Type")}
              </label>
              <Select
                data-testid="pipeline-mapping-node-select"
                value={draftNodeType}
                onChange={(e) => setDraftNodeType(e.target.value)}
                className="h-8 w-full font-mono-tech"
              >
                {workingDoc.node_types.map((nt) => (
                  <option key={nt.name} value={nt.name}>
                    {nt.display_name || nt.name}
                  </option>
                ))}
              </Select>
            </div>
            <div>
              <label className="block text-[11px] text-muted-foreground mb-1">
                {tx("主键列", "Key Column")}
              </label>
              <Input
                data-testid="pipeline-mapping-key-input"
                value={draftKeyCol}
                onChange={(e) => setDraftKeyCol(e.target.value)}
                placeholder="id"
                className="h-8 text-xs font-mono-tech"
              />
            </div>
          </div>
          <div className="flex justify-end gap-1.5">
            <Button
              type="button"
              variant="ghost"
              size="sm"
              onClick={() => setQuickAddOpen(false)}
            >
              {tx("取消", "Cancel")}
            </Button>
            <Button
              type="button"
              size="sm"
              data-testid="pipeline-mapping-submit-btn"
              onClick={submitQuickMapping}
            >
              {tx("创建", "Create")}
            </Button>
          </div>
        </Card>
      )}

      <div className="grid grid-cols-1 lg:grid-cols-4 gap-3 min-w-[760px]">
        {/* Stage 1: Source Datasets */}
        <div
          data-testid="pipeline-col-datasets"
          className="rounded-md border border-border bg-card/85 p-2.5 space-y-2"
        >
          <div className="flex items-center justify-between border-b border-border pb-1.5 text-xs font-semibold">
            <span className="inline-flex items-center gap-1.5">
              <Database className="h-3.5 w-3.5 text-xblue" />
              {tx("数据集源表", "Datasets")}
            </span>
            <Badge variant="neutral">{uniqueDatasets.length}</Badge>
          </div>
          <div className="space-y-1.5">
            {uniqueDatasets.length === 0 ? (
              <div className="py-4 text-center text-xs text-muted-foreground">
                {tx("暂无数据集", "No datasets")}
              </div>
            ) : (
              uniqueDatasets.map((ds) => {
                const linkedCount = workingDoc.mappings.filter(
                  (m) => m.dataset === ds
                ).length;
                return (
                  <div
                    key={ds}
                    data-testid={`pipeline-dataset-${ds}`}
                    className="rounded border border-border bg-background px-2.5 py-2 text-xs flex items-center justify-between gap-2"
                  >
                    <span className="truncate font-mono-tech font-medium">
                      {ds}
                    </span>
                    <Badge variant={linkedCount > 0 ? "xblue" : "outline"}>
                      {linkedCount}
                    </Badge>
                  </div>
                );
              })
            )}
          </div>
        </div>

        {/* Stage 2: Field Mapping Rules */}
        <div
          data-testid="pipeline-col-mappings"
          className="rounded-md border border-border bg-card/85 p-2.5 space-y-2"
        >
          <div className="flex items-center justify-between border-b border-border pb-1.5 text-xs font-semibold">
            <span className="inline-flex items-center gap-1.5">
              <ArrowRight className="h-3.5 w-3.5 text-xblue" />
              {tx("字段映射", "Mappings")}
            </span>
            <Badge variant="neutral">{workingDoc.mappings.length}</Badge>
          </div>
          <div className="space-y-1.5">
            {workingDoc.mappings.length === 0 ? (
              <div className="py-4 text-center text-xs text-muted-foreground">
                {tx("暂无映射规则", "No mappings")}
              </div>
            ) : (
              workingDoc.mappings.map((m, idx) => {
                const attrCount = Object.keys(m.attributes ?? {}).length;
                const metricCount = Object.keys(m.metrics ?? {}).length;
                const active = selectedMappingIndex === idx;
                return (
                  <button
                    key={`${m.name}-${idx}`}
                    type="button"
                    data-testid={`pipeline-mapping-${m.name}`}
                    onClick={() => onSelectMapping(idx)}
                    className={cn(
                      "w-full text-left rounded border px-2.5 py-2 text-xs space-y-1 transition-colors cursor-pointer",
                      active
                        ? "border-xblue bg-xblue-soft/40"
                        : "border-border bg-background hover:border-xblue/60"
                    )}
                  >
                    <div className="flex items-center justify-between gap-1">
                      <span className="truncate font-mono-tech font-semibold">
                        {m.name}
                      </span>
                      <Badge variant="outline" className="text-[10px]">
                        {m.key_column}
                      </Badge>
                    </div>
                    <div className="text-[11px] text-muted-foreground truncate">
                      {m.dataset} → {m.node_type}
                    </div>
                    <div className="flex items-center gap-1 text-[10px] text-muted-foreground">
                      <span>
                        {tx(`${attrCount} 属性`, `${attrCount} Attr`)}
                      </span>
                      <span>·</span>
                      <span>
                        {tx(`${metricCount} 指标`, `${metricCount} Metric`)}
                      </span>
                    </div>
                  </button>
                );
              })
            )}
          </div>
        </div>

        {/* Stage 3: Target Object Types */}
        <div
          data-testid="pipeline-col-objects"
          className="rounded-md border border-border bg-card/85 p-2.5 space-y-2"
        >
          <div className="flex items-center justify-between border-b border-border pb-1.5 text-xs font-semibold">
            <span>{tx("业务对象", "Object Types")}</span>
            <Badge variant="neutral">{workingDoc.node_types.length}</Badge>
          </div>
          <div className="space-y-1.5">
            {workingDoc.node_types.map((nt) => {
              const active = selectedNodeName === nt.name;
              return (
                <button
                  key={nt.name}
                  type="button"
                  data-testid={`pipeline-object-${nt.name}`}
                  onClick={() => onSelectNode(nt.name)}
                  className={cn(
                    "w-full text-left rounded border px-2.5 py-2 text-xs space-y-1 transition-colors cursor-pointer",
                    active
                      ? "border-xblue bg-xblue-soft/40"
                      : "border-border bg-background hover:border-xblue/60"
                  )}
                >
                  <div className="flex items-center justify-between gap-1.5">
                    <span className="inline-flex items-center gap-1.5 min-w-0 font-semibold truncate">
                      <span style={{ color: nt.color || "#2563eb" }}>
                        {renderNodeIcon(nt.icon)}
                      </span>
                      <span className="truncate">
                        {nt.display_name || nt.name}
                      </span>
                    </span>
                    {nt.semantic_layer_tier && (
                      <Badge variant="xblue" className="text-[10px]">
                        {tEnum(nt.semantic_layer_tier)}
                      </Badge>
                    )}
                  </div>
                  <div className="text-[10px] font-mono-tech text-muted-foreground">
                    {nt.name} · {nt.properties.length}{" "}
                    {tx("字段", "fields")}
                  </div>
                </button>
              );
            })}
          </div>
        </div>

        {/* Stage 4: Aggregation & Derived Rules */}
        <div
          data-testid="pipeline-col-rules"
          className="rounded-md border border-border bg-card/85 p-2.5 space-y-2"
        >
          <div className="flex items-center justify-between border-b border-border pb-1.5 text-xs font-semibold">
            <span className="inline-flex items-center gap-1.5">
              <Calculator className="h-3.5 w-3.5 text-xblue" />
              {tx("聚合与派生", "Compute Rules")}
            </span>
            <Badge variant="neutral">{ruleEntries.length}</Badge>
          </div>
          <div className="space-y-1.5">
            {ruleEntries.length === 0 ? (
              <div className="py-4 text-center text-xs text-muted-foreground">
                {tx("暂无计算规则", "No compute rules")}
              </div>
            ) : (
              ruleEntries.map((r) => (
                <button
                  key={r.id}
                  type="button"
                  data-testid={`pipeline-rule-${r.nodeType}-${r.metric}`}
                  onClick={() => onSelectNode(r.nodeType)}
                  className="w-full text-left rounded border border-border bg-background hover:border-xblue/60 px-2.5 py-2 text-xs space-y-1 cursor-pointer"
                >
                  <div className="flex items-center justify-between gap-1">
                    <span className="truncate font-mono-tech font-semibold">
                      {r.nodeType}.{r.metric}
                    </span>
                    <Badge variant="outline" className="text-[10px]">
                      {tEnum(r.kind)}
                    </Badge>
                  </div>
                  <div className="truncate font-mono-tech text-[10px] text-muted-foreground">
                    {r.detail}
                  </div>
                </button>
              ))
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
