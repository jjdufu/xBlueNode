import React, {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import {
  AlertTriangle,
  GripVertical,
  Play,
  Plus,
  Save,
  TableProperties,
  X,
} from "lucide-react";
import { useI18n } from "@/i18n";
import {
  analyticsApi,
  type AnalyticsCatalogResponse,
  type FieldRef,
  type FilterOp,
  type FilterSpec,
  type PivotResult,
  type PivotSpec,
  type TimeBucket,
  type ValueSpec,
} from "@/lib/api/analytics";
import type { AggFn, DataType, RollupGrain } from "@/lib/api/enums";
import {
  Badge,
  Button,
  Card,
  Input,
  Select,
} from "@/components/ui/primitives";

const AGG_OPTIONS: readonly AggFn[] = [
  "SUM",
  "AVG",
  "COUNT",
  "MIN",
  "MAX",
  "MEDIAN",
  "COUNT_DISTINCT",
];

const FILTER_OPS: ReadonlyArray<{ op: FilterOp; label: string }> = [
  { op: "Eq", label: "=" },
  { op: "Ne", label: "!=" },
  { op: "Gt", label: ">" },
  { op: "Gte", label: ">=" },
  { op: "Lt", label: "<" },
  { op: "Lte", label: "<=" },
  { op: "In", label: "IN" },
  { op: "Like", label: "LIKE" },
  { op: "IsNull", label: "IS NULL" },
  { op: "IsNotNull", label: "IS NOT NULL" },
];

const NUMERIC_TYPES: ReadonlySet<DataType> = new Set([
  "DECIMAL",
  "INT64",
  "FLOAT64",
]);

const TEMPORAL_TYPES: ReadonlySet<DataType> = new Set(["DATE", "TIMESTAMP"]);

type DropZoneKey = "rows" | "columns" | "values" | "filters";

interface SourceFieldInfo {
  name: string;
  dataType: DataType;
}

function parseFilterInput(op: FilterOp, raw: string): unknown {
  if (op === "IsNull" || op === "IsNotNull") return null;
  if (op === "In") {
    return raw
      .split(",")
      .map((s) => s.trim())
      .filter((s) => s.length > 0);
  }
  const trimmed = raw.trim();
  if (/^-?\d+(\.\d+)?$/.test(trimmed)) {
    const n = Number(trimmed);
    if (Number.isFinite(n)) return n;
  }
  if (trimmed === "true") return true;
  if (trimmed === "false") return false;
  return raw;
}

export interface PivotBuilderProps {
  catalog: AnalyticsCatalogResponse;
  selectedDataset: string;
  onSelectDataset: (datasetOrView: string) => void;
  onSavePivotQuery?: (name: string, spec: PivotSpec) => Promise<void>;
}

export function PivotBuilder({
  catalog,
  selectedDataset,
  onSelectDataset,
  onSavePivotQuery,
}: PivotBuilderProps) {
  const { tx, fmtNum } = useI18n();
  const draggedFieldRef = useRef<string | null>(null);
  const reqSeqRef = useRef(0);
  const dragDepthRef = useRef<Record<DropZoneKey, number>>({
    rows: 0,
    columns: 0,
    values: 0,
    filters: 0,
  });

  const [rows, setRows] = useState<FieldRef[]>([]);
  const [columns, setColumns] = useState<FieldRef[]>([]);
  const [values, setValues] = useState<ValueSpec[]>([]);
  const [filters, setFilters] = useState<
    Array<{ field: string; op: FilterOp; rawValue: string }>
  >([]);
  const [timeBucketField, setTimeBucketField] = useState<string>("");
  const [timeBucketGrain, setTimeBucketGrain] = useState<RollupGrain>("MONTH");
  const [subtotals, setSubtotals] = useState<boolean>(true);
  const [grandTotal, setGrandTotal] = useState<boolean>(true);
  const [rowLimit, setRowLimit] = useState<number>(100);
  const [columnLimit, setColumnLimit] = useState<number>(50);

  const [activeDropZone, setActiveDropZone] = useState<DropZoneKey | null>(
    null
  );
  const [pivotResult, setPivotResult] = useState<PivotResult | null>(null);
  const [loading, setLoading] = useState(false);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  const [saveOpen, setSaveOpen] = useState(false);
  const [saveName, setSaveName] = useState("");
  const [saving, setSaving] = useState(false);

  const grainLabel = (g: RollupGrain): string => {
    switch (g) {
      case "MINUTE":
        return tx("分钟", "Minute");
      case "HOUR":
        return tx("小时", "Hour");
      case "DAY":
        return tx("日", "Day");
      case "MONTH":
        return tx("月", "Month");
      default:
        return g;
    }
  };

  const resetDragDepths = () => {
    dragDepthRef.current = { rows: 0, columns: 0, values: 0, filters: 0 };
    setActiveDropZone(null);
  };

  const sourceOptions = useMemo(() => {
    const opts: Array<{
      value: string;
      label: string;
      kind: "DATASET" | "VIEW";
    }> = [];
    for (const ds of catalog.datasets ?? []) {
      opts.push({
        value: ds.name,
        label:
          ds.display_name && ds.display_name !== ds.name
            ? `${ds.name} · ${ds.display_name}`
            : ds.name,
        kind: "DATASET",
      });
    }
    for (const vw of catalog.platform_views ?? []) {
      opts.push({
        value: vw.name,
        label: `${vw.name} · ${tx("联邦视图", "Platform View")}`,
        kind: "VIEW",
      });
    }
    return opts;
  }, [catalog.datasets, catalog.platform_views, tx]);

  useEffect(() => {
    if (!selectedDataset && sourceOptions[0]) {
      onSelectDataset(sourceOptions[0].value);
    }
  }, [onSelectDataset, selectedDataset, sourceOptions]);

  const availableFields = useMemo<SourceFieldInfo[]>(() => {
    const ds = (catalog.datasets ?? []).find((d) => d.name === selectedDataset);
    if (ds) {
      return ds.columns.map((c) => ({
        name: c.column_name,
        dataType: c.data_type,
      }));
    }
    const vw = (catalog.platform_views ?? []).find(
      (v) => v.name === selectedDataset
    );
    if (vw) {
      return vw.columns.map((c) => ({
        name: c.name,
        dataType: c.data_type,
      }));
    }
    return [];
  }, [catalog.datasets, catalog.platform_views, selectedDataset]);

  const temporalFields = useMemo(
    () => availableFields.filter((f) => TEMPORAL_TYPES.has(f.dataType)),
    [availableFields]
  );

  const handleDatasetChange = (nextDs: string) => {
    onSelectDataset(nextDs);
    setRows([]);
    setColumns([]);
    setValues([]);
    setFilters([]);
    setTimeBucketField("");
    setPivotResult(null);
    setErrorMsg(null);
  };

  const addRowField = useCallback((fieldName: string) => {
    if (!fieldName) return;
    setRows((prev) => {
      if (prev.length >= 5 || prev.some((r) => r.field === fieldName)) {
        return prev;
      }
      return [...prev, { field: fieldName }];
    });
  }, []);

  const addColField = useCallback((fieldName: string) => {
    if (!fieldName) return;
    setColumns((prev) => {
      if (prev.length >= 3 || prev.some((c) => c.field === fieldName)) {
        return prev;
      }
      return [...prev, { field: fieldName }];
    });
  }, []);

  const addValueField = useCallback(
    (fieldName: string) => {
      if (!fieldName) return;
      const meta = availableFields.find((f) => f.name === fieldName);
      const defaultAgg: AggFn =
        meta && NUMERIC_TYPES.has(meta.dataType) ? "SUM" : "COUNT";
      setValues((prev) => {
        if (prev.length >= 10) return prev;
        return [
          ...prev,
          {
            field: fieldName,
            agg: defaultAgg,
            alias: `${defaultAgg.toLowerCase()}_${fieldName}`,
          },
        ];
      });
    },
    [availableFields]
  );

  const addFilterField = useCallback((fieldName: string) => {
    if (!fieldName) return;
    setFilters((prev) => [
      ...prev,
      { field: fieldName, op: "Eq", rawValue: "" },
    ]);
  }, []);

  const buildSpec = useCallback((): PivotSpec => {
    const compiledFilters: FilterSpec[] = filters
      .filter(
        (f) =>
          f.op === "IsNull" ||
          f.op === "IsNotNull" ||
          f.rawValue.trim().length > 0
      )
      .map((f) => ({
        field: f.field,
        op: f.op,
        value: parseFilterInput(f.op, f.rawValue),
      }));

    const timeBucket: TimeBucket | undefined = timeBucketField
      ? { field: timeBucketField, grain: timeBucketGrain }
      : undefined;

    return {
      dataset: selectedDataset,
      rows,
      columns,
      values,
      filters: compiledFilters,
      ...(timeBucket ? { time_bucket: timeBucket } : {}),
      sort: [],
      subtotals,
      grand_total: grandTotal,
      row_limit: rowLimit,
      column_limit: columnLimit,
    };
  }, [
    columnLimit,
    columns,
    filters,
    grandTotal,
    rowLimit,
    rows,
    selectedDataset,
    subtotals,
    timeBucketField,
    timeBucketGrain,
    values,
  ]);

  const executePivot = useCallback(async (spec: PivotSpec) => {
    if (!spec.dataset || spec.values.length === 0) {
      setPivotResult(null);
      return;
    }
    const seq = ++reqSeqRef.current;
    setLoading(true);
    setErrorMsg(null);
    try {
      const res = await analyticsApi.runPivot(spec);
      if (seq !== reqSeqRef.current) return;
      setPivotResult(res);
    } catch (err) {
      if (seq !== reqSeqRef.current) return;
      setErrorMsg(err instanceof Error ? err.message : String(err));
    } finally {
      if (seq === reqSeqRef.current) {
        setLoading(false);
      }
    }
  }, []);

  useEffect(() => {
    const spec = buildSpec();
    if (!spec.dataset || spec.values.length === 0) {
      setPivotResult(null);
      return;
    }
    void executePivot(spec);
  }, [buildSpec, executePivot]);

  const handleZoneDragEnter = (
    e: React.DragEvent<HTMLDivElement>,
    zone: DropZoneKey
  ) => {
    e.preventDefault();
    dragDepthRef.current[zone] += 1;
    setActiveDropZone(zone);
  };

  const handleZoneDragOver = (
    e: React.DragEvent<HTMLDivElement>,
    zone: DropZoneKey
  ) => {
    e.preventDefault();
    e.dataTransfer.dropEffect = "copy";
    if (activeDropZone !== zone) {
      setActiveDropZone(zone);
    }
  };

  const handleZoneDragLeave = (
    e: React.DragEvent<HTMLDivElement>,
    zone: DropZoneKey
  ) => {
    e.preventDefault();
    dragDepthRef.current[zone] = Math.max(0, dragDepthRef.current[zone] - 1);
    const leftContainer =
      e.relatedTarget instanceof Node
        ? !e.currentTarget.contains(e.relatedTarget)
        : dragDepthRef.current[zone] === 0;
    if (dragDepthRef.current[zone] === 0 || leftContainer) {
      dragDepthRef.current[zone] = 0;
      setActiveDropZone((prev) => (prev === zone ? null : prev));
    }
  };

  const handleZoneDrop = (
    e: React.DragEvent<HTMLDivElement>,
    zone: DropZoneKey
  ) => {
    e.preventDefault();
    resetDragDepths();
    const transferred = e.dataTransfer.getData("text/plain").trim();
    const fieldName = transferred || draggedFieldRef.current || "";
    draggedFieldRef.current = null;
    if (!fieldName) return;

    if (zone === "rows") addRowField(fieldName);
    else if (zone === "columns") addColField(fieldName);
    else if (zone === "values") addValueField(fieldName);
    else if (zone === "filters") addFilterField(fieldName);
  };

  const rowDimSlotCount = useMemo(() => {
    if (!pivotResult) return Math.max(1, rows.length);
    const maxHeaderLen = pivotResult.row_headers.reduce(
      (acc, h) => Math.max(acc, h.length),
      0
    );
    return Math.max(1, rows.length, maxHeaderLen);
  }, [pivotResult, rows.length]);

  const handleSaveConfirm = async () => {
    if (!onSavePivotQuery || !saveName.trim()) return;
    setSaving(true);
    try {
      await onSavePivotQuery(saveName.trim(), buildSpec());
      setSaveName("");
      setSaveOpen(false);
    } finally {
      setSaving(false);
    }
  };

  const renderDropPlaceholder = (zone: DropZoneKey) => {
    if (activeDropZone !== zone) return null;
    return (
      <span
        data-testid={`pivot-drop-placeholder-${zone}`}
        className="inline-flex items-center rounded border border-dashed border-xblue bg-xblue-soft/40 px-2.5 py-1 text-[11px] font-medium text-xblue pointer-events-none"
      >
        {tx("松开以放入此维度区", "Drop field here")}
      </span>
    );
  };

  const dropZoneClass = (zone: DropZoneKey) =>
    `min-h-[82px] rounded-md p-2.5 transition-colors ${
      activeDropZone === zone
        ? "border-2 border-dashed border-xblue bg-xblue-soft/30"
        : "border border-border bg-card"
    }`;

  return (
    <div className="space-y-3" data-testid="analytics-pivot-builder">
      {/* Top Bar: Dataset Selector + Toggles + Run */}
      <div className="flex flex-wrap items-center justify-between gap-2 rounded-md border border-border bg-muted/30 px-3 py-2 text-xs">
        <div className="flex flex-wrap items-center gap-2">
          <span className="font-medium text-muted-foreground">
            {tx("数据源", "Dataset")}:
          </span>
          <Select
            value={selectedDataset}
            onChange={(e) => handleDatasetChange(e.target.value)}
            className="h-8 font-mono-tech"
            data-testid="pivot-dataset-select"
          >
            {sourceOptions.length === 0 && (
              <option value="">{tx("暂无数据集", "No datasets")}</option>
            )}
            {sourceOptions.map((opt) => (
              <option key={opt.value} value={opt.value}>
                {opt.label}
              </option>
            ))}
          </Select>

          {temporalFields.length > 0 && (
            <div className="flex items-center gap-1">
              <span className="text-muted-foreground">
                {tx("时间分桶", "Time Bucket")}:
              </span>
              <Select
                value={timeBucketField}
                onChange={(e) => setTimeBucketField(e.target.value)}
                className="h-8 font-mono-tech"
                data-testid="pivot-time-field-select"
              >
                <option value="">{tx("关闭", "Off")}</option>
                {temporalFields.map((tf) => (
                  <option key={tf.name} value={tf.name}>
                    {tf.name}
                  </option>
                ))}
              </Select>
              {timeBucketField && (
                <Select
                  value={timeBucketGrain}
                  onChange={(e) =>
                    setTimeBucketGrain(e.target.value as RollupGrain)
                  }
                  className="h-8"
                >
                  {(["MINUTE", "HOUR", "DAY", "MONTH"] as const).map((g) => (
                    <option key={g} value={g}>
                      {grainLabel(g)}
                    </option>
                  ))}
                </Select>
              )}
            </div>
          )}
        </div>

        <div className="flex flex-wrap items-center gap-2">
          <label className="inline-flex items-center gap-1.5 rounded border border-border bg-background px-2.5 py-1.5 cursor-pointer">
            <input
              type="checkbox"
              checked={subtotals}
              onChange={(e) => setSubtotals(e.target.checked)}
              className="rounded border-input accent-xblue"
              data-testid="pivot-subtotals-toggle"
            />
            <span>{tx("显示小计", "Subtotals")}</span>
          </label>

          <label className="inline-flex items-center gap-1.5 rounded border border-border bg-background px-2.5 py-1.5 cursor-pointer">
            <input
              type="checkbox"
              checked={grandTotal}
              onChange={(e) => setGrandTotal(e.target.checked)}
              className="rounded border-input accent-xblue"
              data-testid="pivot-grand-total-toggle"
            />
            <span>{tx("显示总计", "Grand Total")}</span>
          </label>

          <div className="flex items-center gap-1">
            <span className="text-muted-foreground">
              {tx("行限", "Row Cap")}
            </span>
            <Select
              value={rowLimit}
              onChange={(e) => setRowLimit(Number(e.target.value))}
              className="h-7 font-mono-tech text-[11px]"
            >
              <option value={100}>{fmtNum(100, 0)}</option>
              <option value={500}>{fmtNum(500, 0)}</option>
              <option value={1000}>{fmtNum(1000, 0)}</option>
            </Select>
            <span className="text-muted-foreground">
              {tx("列限", "Col Cap")}
            </span>
            <Select
              value={columnLimit}
              onChange={(e) => setColumnLimit(Number(e.target.value))}
              className="h-7 font-mono-tech text-[11px]"
            >
              <option value={50}>{fmtNum(50, 0)}</option>
              <option value={100}>{fmtNum(100, 0)}</option>
              <option value={200}>{fmtNum(200, 0)}</option>
            </Select>
          </div>

          <Button
            type="button"
            variant="primary"
            onClick={() => void executePivot(buildSpec())}
            disabled={loading || !selectedDataset || values.length === 0}
            data-testid="pivot-run-btn"
          >
            <Play className="h-3.5 w-3.5" />
            {loading ? tx("计算中…", "Running…") : tx("刷新透视", "Refresh")}
          </Button>

          {onSavePivotQuery && (
            <Button
              type="button"
              variant="outline"
              disabled={values.length === 0}
              onClick={() => setSaveOpen((prev) => !prev)}
              data-testid="pivot-save-open-btn"
            >
              <Save className="h-3.5 w-3.5 text-xblue" />
              {tx("保存透视", "Save Pivot")}
            </Button>
          )}
        </div>
      </div>

      {saveOpen && (
        <Card className="flex flex-wrap items-center gap-2 bg-muted/20 p-3">
          <Input
            value={saveName}
            onChange={(e) => setSaveName(e.target.value)}
            placeholder="orders_pivot_summary"
            className="h-8 max-w-xs text-xs font-mono-tech"
            data-testid="pivot-save-name-input"
          />
          <Button
            type="button"
            variant="primary"
            size="sm"
            disabled={saving || !saveName.trim()}
            onClick={() => void handleSaveConfirm()}
            data-testid="pivot-save-confirm-btn"
          >
            {tx("保存", "Save")}
          </Button>
          <Button
            type="button"
            variant="ghost"
            size="sm"
            onClick={() => setSaveOpen(false)}
          >
            {tx("取消", "Cancel")}
          </Button>
        </Card>
      )}

      {/* Draggable Field Pool */}
      <Card className="p-3 space-y-2">
        <div className="flex items-center justify-between text-xs">
          <span className="font-medium text-muted-foreground">
            {tx("可用字段", "Available Fields")}
          </span>
          <span className="font-mono-tech text-[11px] text-muted-foreground">
            {fmtNum(availableFields.length, 0)}
          </span>
        </div>

        {availableFields.length === 0 ? (
          <div className="py-2 text-xs text-muted-foreground">
            {tx("暂无字段元数据", "No field metadata")}
          </div>
        ) : (
          <div className="flex flex-wrap gap-2" data-testid="pivot-field-pool">
            {availableFields.map((f) => (
              <div
                key={f.name}
                className="inline-flex items-center rounded border border-border bg-muted/30 text-xs overflow-hidden"
              >
                <div
                  draggable={true}
                  data-testid={`pivot-field-${f.name}`}
                  onDragStart={(e) => {
                    draggedFieldRef.current = f.name;
                    e.dataTransfer.effectAllowed = "copyMove";
                    e.dataTransfer.setData("text/plain", f.name);
                  }}
                  onDragEnd={() => {
                    draggedFieldRef.current = null;
                    resetDragDepths();
                  }}
                  className="inline-flex items-center gap-1.5 px-2.5 py-1.5 cursor-grab active:cursor-grabbing select-none hover:bg-muted"
                >
                  <GripVertical className="h-3 w-3 text-muted-foreground" />
                  <span className="font-mono-tech font-medium text-foreground">
                    {f.name}
                  </span>
                  <span className="font-mono-tech text-[10px] text-muted-foreground">
                    {f.dataType}
                  </span>
                </div>

                <div className="flex items-center border-l border-border bg-background px-1 py-1 gap-0.5">
                  <button
                    type="button"
                    onClick={() => addRowField(f.name)}
                    title={tx(`行维度: ${f.name}`, `Add row: ${f.name}`)}
                    data-testid={`pivot-quick-row-${f.name}`}
                    className="rounded px-1 py-0.5 text-[10px] text-muted-foreground hover:bg-muted hover:text-foreground cursor-pointer"
                  >
                    +{tx("行", "Row")}
                  </button>
                  <button
                    type="button"
                    onClick={() => addColField(f.name)}
                    title={tx(`列维度: ${f.name}`, `Add col: ${f.name}`)}
                    data-testid={`pivot-quick-col-${f.name}`}
                    className="rounded px-1 py-0.5 text-[10px] text-muted-foreground hover:bg-muted hover:text-foreground cursor-pointer"
                  >
                    +{tx("列", "Col")}
                  </button>
                  <button
                    type="button"
                    onClick={() => addValueField(f.name)}
                    title={tx(`指标: ${f.name}`, `Add value: ${f.name}`)}
                    data-testid={`pivot-quick-val-${f.name}`}
                    className="rounded px-1 py-0.5 text-[10px] text-xblue hover:bg-xblue-soft cursor-pointer"
                  >
                    +{tx("值", "Val")}
                  </button>
                </div>
              </div>
            ))}
          </div>
        )}
      </Card>

      {/* Four Drop Zones: Rows, Columns, Values, Filters */}
      <div className="grid grid-cols-1 md:grid-cols-2 gap-3">
        {/* 1. Rows Zone */}
        <div
          data-testid="pivot-drop-rows"
          onDragEnter={(e) => handleZoneDragEnter(e, "rows")}
          onDragOver={(e) => handleZoneDragOver(e, "rows")}
          onDragLeave={(e) => handleZoneDragLeave(e, "rows")}
          onDrop={(e) => handleZoneDrop(e, "rows")}
          className={dropZoneClass("rows")}
        >
          <div className="mb-1.5 flex items-center justify-between text-xs pointer-events-none">
            <span className="font-semibold text-foreground">
              {tx("行维度", "Rows")}
            </span>
            <span className="font-mono-tech text-[10px] text-muted-foreground">
              {rows.length}/5
            </span>
          </div>
          <div className="flex flex-wrap items-center gap-1.5">
            {rows.map((r) => (
              <span
                key={r.field}
                className="inline-flex items-center gap-1 rounded border border-border bg-muted px-2 py-1 font-mono-tech text-xs"
                data-testid={`pivot-row-chip-${r.field}`}
              >
                {r.field}
                <button
                  type="button"
                  onClick={() =>
                    setRows((prev) =>
                      prev.filter((item) => item.field !== r.field)
                    )
                  }
                  className="text-muted-foreground hover:text-foreground cursor-pointer"
                >
                  <X className="h-3 w-3" />
                </button>
              </span>
            ))}
            {renderDropPlaceholder("rows")}
            {rows.length === 0 && activeDropZone !== "rows" && (
              <span className="py-1 text-[11px] text-muted-foreground pointer-events-none">
                {tx("拖入字段作为分组行", "Drag fields here for row grouping")}
              </span>
            )}
          </div>
        </div>

        {/* 2. Columns Zone */}
        <div
          data-testid="pivot-drop-columns"
          onDragEnter={(e) => handleZoneDragEnter(e, "columns")}
          onDragOver={(e) => handleZoneDragOver(e, "columns")}
          onDragLeave={(e) => handleZoneDragLeave(e, "columns")}
          onDrop={(e) => handleZoneDrop(e, "columns")}
          className={dropZoneClass("columns")}
        >
          <div className="mb-1.5 flex items-center justify-between text-xs pointer-events-none">
            <span className="font-semibold text-foreground">
              {tx("列维度", "Columns")}
            </span>
            <span className="font-mono-tech text-[10px] text-muted-foreground">
              {columns.length}/3
            </span>
          </div>
          <div className="flex flex-wrap items-center gap-1.5">
            {columns.map((c) => (
              <span
                key={c.field}
                className="inline-flex items-center gap-1 rounded border border-border bg-muted px-2 py-1 font-mono-tech text-xs"
                data-testid={`pivot-col-chip-${c.field}`}
              >
                {c.field}
                <button
                  type="button"
                  onClick={() =>
                    setColumns((prev) =>
                      prev.filter((item) => item.field !== c.field)
                    )
                  }
                  className="text-muted-foreground hover:text-foreground cursor-pointer"
                >
                  <X className="h-3 w-3" />
                </button>
              </span>
            ))}
            {renderDropPlaceholder("columns")}
            {columns.length === 0 && activeDropZone !== "columns" && (
              <span className="py-1 text-[11px] text-muted-foreground pointer-events-none">
                {tx("拖入字段作为交叉列头", "Drag fields here for column headers")}
              </span>
            )}
          </div>
        </div>

        {/* 3. Values Zone */}
        <div
          data-testid="pivot-drop-values"
          onDragEnter={(e) => handleZoneDragEnter(e, "values")}
          onDragOver={(e) => handleZoneDragOver(e, "values")}
          onDragLeave={(e) => handleZoneDragLeave(e, "values")}
          onDrop={(e) => handleZoneDrop(e, "values")}
          className={dropZoneClass("values")}
        >
          <div className="mb-1.5 flex items-center justify-between text-xs pointer-events-none">
            <span className="font-semibold text-foreground">
              {tx("数值指标", "Values")}
            </span>
            <span className="font-mono-tech text-[10px] text-muted-foreground">
              {values.length}/10
            </span>
          </div>
          <div className="flex flex-wrap items-center gap-1.5">
            {values.map((v, idx) => (
              <div
                key={`${v.field}-${idx}`}
                className="inline-flex items-center gap-1.5 rounded border border-xblue/35 bg-xblue-soft/50 px-2 py-1 text-xs"
                data-testid={`pivot-val-chip-${v.field}`}
              >
                <select
                  value={v.agg}
                  onChange={(e) => {
                    const nextAgg = e.target.value as AggFn;
                    setValues((prev) =>
                      prev.map((item, i) =>
                        i === idx
                          ? {
                              ...item,
                              agg: nextAgg,
                              alias: `${nextAgg.toLowerCase()}_${item.field}`,
                            }
                          : item
                      )
                    );
                  }}
                  className="rounded border border-border bg-background px-1 py-0.5 font-mono-tech text-[11px] text-xblue"
                  data-testid={`pivot-val-agg-${v.field}-${idx}`}
                >
                  {AGG_OPTIONS.map((agg) => (
                    <option key={agg} value={agg}>
                      {agg}
                    </option>
                  ))}
                </select>
                <span className="font-mono-tech font-medium text-foreground">
                  ({v.field})
                </span>
                <button
                  type="button"
                  onClick={() =>
                    setValues((prev) => prev.filter((_, i) => i !== idx))
                  }
                  className="text-muted-foreground hover:text-foreground cursor-pointer"
                >
                  <X className="h-3 w-3" />
                </button>
              </div>
            ))}
            {renderDropPlaceholder("values")}
            {values.length === 0 && activeDropZone !== "values" && (
              <span className="py-1 text-[11px] text-muted-foreground pointer-events-none">
                {tx("拖入度量字段生成聚合值", "Drag measure fields here to aggregate")}
              </span>
            )}
          </div>
        </div>

        {/* 4. Filters Zone */}
        <div
          data-testid="pivot-drop-filters"
          onDragEnter={(e) => handleZoneDragEnter(e, "filters")}
          onDragOver={(e) => handleZoneDragOver(e, "filters")}
          onDragLeave={(e) => handleZoneDragLeave(e, "filters")}
          onDrop={(e) => handleZoneDrop(e, "filters")}
          className={dropZoneClass("filters")}
        >
          <div className="mb-1.5 flex items-center justify-between text-xs">
            <span className="font-semibold text-foreground pointer-events-none">
              {tx("筛选条件", "Filters")}
            </span>
            {availableFields[0] && (
              <button
                type="button"
                onClick={() => addFilterField(availableFields[0]?.name ?? "")}
                className="inline-flex items-center gap-0.5 text-[11px] text-xblue hover:underline cursor-pointer"
              >
                <Plus className="h-3 w-3" />
                {tx("新增筛选", "Add Filter")}
              </button>
            )}
          </div>
          <div className="space-y-1.5">
            {filters.map((flt, idx) => (
              <div
                key={`${flt.field}-${idx}`}
                className="flex flex-wrap items-center gap-1.5 text-xs"
              >
                <Badge variant="neutral">{flt.field}</Badge>
                <select
                  value={flt.op}
                  onChange={(e) => {
                    const nextOp = e.target.value as FilterOp;
                    setFilters((prev) =>
                      prev.map((item, i) =>
                        i === idx ? { ...item, op: nextOp } : item
                      )
                    );
                  }}
                  className="h-7 rounded border border-input bg-background px-1.5 font-mono-tech text-[11px]"
                >
                  {FILTER_OPS.map((o) => (
                    <option key={o.op} value={o.op}>
                      {o.label}
                    </option>
                  ))}
                </select>
                {flt.op !== "IsNull" && flt.op !== "IsNotNull" && (
                  <Input
                    value={flt.rawValue}
                    onChange={(e) => {
                      const val = e.target.value;
                      setFilters((prev) =>
                        prev.map((item, i) =>
                          i === idx ? { ...item, rawValue: val } : item
                        )
                      );
                    }}
                    placeholder="PAID"
                    className="h-7 w-28 text-xs font-mono-tech"
                  />
                )}
                <button
                  type="button"
                  onClick={() =>
                    setFilters((prev) => prev.filter((_, i) => i !== idx))
                  }
                  className="text-muted-foreground hover:text-foreground cursor-pointer"
                >
                  <X className="h-3.5 w-3.5" />
                </button>
              </div>
            ))}
            {renderDropPlaceholder("filters")}
            {filters.length === 0 && activeDropZone !== "filters" && (
              <span className="block py-1 text-[11px] text-muted-foreground pointer-events-none">
                {tx("拖入字段添加过滤条件", "Drag fields here to filter")}
              </span>
            )}
          </div>
        </div>
      </div>

      {/* Error Message */}
      {errorMsg && (
        <Card className="border-red-500/30 bg-red-500/5 p-3 text-xs text-red-600 dark:text-red-400">
          {errorMsg}
        </Card>
      )}

      {/* 2D Pivot Matrix Preview */}
      <Card className="overflow-hidden" data-testid="pivot-matrix-card">
        <div className="flex flex-wrap items-center justify-between gap-2 border-b border-border bg-muted/30 px-3 py-2 text-xs">
          <div className="flex items-center gap-2">
            <TableProperties className="h-3.5 w-3.5 text-xblue" />
            <span className="font-medium">
              {tx("交叉透视结果", "Pivot Matrix")}
            </span>
            {pivotResult && (
              <>
                <Badge variant="neutral">
                  {fmtNum(pivotResult.cells.length, 0)} ×{" "}
                  {fmtNum(pivotResult.column_headers.length, 0)}
                </Badge>
                <span className="font-mono-tech text-[11px] text-muted-foreground">
                  {fmtNum(pivotResult.duration_ms, 0)} ms
                </span>
              </>
            )}
          </div>
          {pivotResult?.columns_truncated && (
            <Badge variant="warning">
              <AlertTriangle className="mr-1 h-3 w-3" />
              {tx("列数超限已截断", "Columns truncated")}
            </Badge>
          )}
        </div>

        {!pivotResult ? (
          <div className="flex h-36 items-center justify-center px-4 text-xs text-muted-foreground">
            {loading
              ? tx("计算中…", "Computing pivot…")
              : tx(
                  "拖入行维度与数值指标以生成透视表",
                  "Add row dimensions and value measures to build pivot table"
                )}
          </div>
        ) : (
          <div className="max-h-96 overflow-auto">
            <table
              className="w-full border-collapse text-left text-xs"
              data-testid="pivot-result-table"
            >
              <thead className="sticky top-0 z-10 bg-muted border-b border-border">
                <tr>
                  {Array.from({ length: rowDimSlotCount }, (_, idx) => (
                    <th
                      key={`rdim-${idx}`}
                      className="border-r border-border px-3 py-2 font-mono-tech font-semibold text-foreground whitespace-nowrap"
                    >
                      {rows[idx]?.label ||
                        rows[idx]?.field ||
                        tx("汇总维度", "Dimension")}
                    </th>
                  ))}
                  {pivotResult.column_headers.map((colHdr, cIdx) => (
                    <th
                      key={`chdr-${cIdx}`}
                      className="border-r border-border last:border-r-0 px-3 py-2 text-right font-mono-tech font-semibold text-foreground whitespace-nowrap"
                    >
                      {colHdr.filter(Boolean).join(" / ") ||
                        `${tx("指标", "Measure")} ${cIdx + 1}`}
                    </th>
                  ))}
                </tr>
              </thead>
              <tbody>
                {pivotResult.cells.map((cellRow, rIdx) => {
                  const kind = pivotResult.row_kinds[rIdx] ?? "DATA";
                  const rawHeaders = pivotResult.row_headers[rIdx] ?? [];
                  const grandLabel = tx("总计", "Grand Total");
                  const subLabel = tx("小计", "Subtotal");
                  const hasGrandLabel = rawHeaders.some(
                    (h) => h.includes("总计") || h.toLowerCase().includes("total")
                  );
                  const hasSubLabel = rawHeaders.some(
                    (h) => h.includes("小计") || h.toLowerCase().includes("subtotal")
                  );

                  const displayHeaders = Array.from(
                    { length: rowDimSlotCount },
                    (_, dIdx) => {
                      const val = rawHeaders[dIdx] ?? "";
                      if (kind === "GRAND_TOTAL" && !hasGrandLabel) {
                        return dIdx === 0 ? grandLabel : "";
                      }
                      if (kind === "SUBTOTAL" && !hasSubLabel) {
                        if (dIdx === rowDimSlotCount - 1) {
                          return val ? `${val} · ${subLabel}` : subLabel;
                        }
                      }
                      return val;
                    }
                  );

                  const rowClass =
                    kind === "GRAND_TOTAL"
                      ? "bg-xblue-soft/60 font-semibold border-t-2 border-border"
                      : kind === "SUBTOTAL"
                        ? "bg-muted/60 font-medium border-t border-border"
                        : "border-b border-border hover:bg-muted/30";

                  return (
                    <tr
                      key={rIdx}
                      className={rowClass}
                      data-testid={
                        kind === "GRAND_TOTAL"
                          ? "pivot-row-grand-total"
                          : kind === "SUBTOTAL"
                            ? "pivot-row-subtotal"
                            : `pivot-row-data-${rIdx}`
                      }
                    >
                      {displayHeaders.map((hdrText, dIdx) => (
                        <td
                          key={`rh-${dIdx}`}
                          className="border-r border-border px-3 py-1.5 font-mono-tech whitespace-nowrap"
                        >
                          {hdrText || (kind === "DATA" ? "-" : "")}
                        </td>
                      ))}
                      {cellRow.map((cellVal, cIdx) => (
                        <td
                          key={`cell-${cIdx}`}
                          className="border-r border-border last:border-r-0 px-3 py-1.5 text-right font-mono-tech whitespace-nowrap"
                        >
                          {cellVal === "" ? "-" : cellVal}
                        </td>
                      ))}
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        )}
      </Card>
    </div>
  );
}
