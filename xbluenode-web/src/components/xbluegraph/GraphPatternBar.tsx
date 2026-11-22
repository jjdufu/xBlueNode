import { useEffect, useMemo, useState } from "react";
import { Code2, Filter, RotateCcw, SlidersHorizontal } from "lucide-react";
import { useI18n } from "@/i18n";
import {
  xblueApi,
  type BlueEdgeTypeDef,
  type BlueNodeTypeDef,
  type SubgraphInfo,
} from "@/lib/api/xblue";
import {
  Button,
  Card,
  Input,
  SegmentedControl,
  Select,
  type SelectOptionItem,
} from "@/components/ui/primitives";
import {
  formatGraphPatternExpression,
  parseGraphPatternExpression,
  prioritizeDstTypes,
  prioritizeEdgeTypes,
} from "./logic";

export interface GraphPatternFilterValues {
  subgraph: string;
  srcType: string;
  edgeType: string;
  dstType: string;
}

export interface GraphPatternBarProps {
  subgraph: string;
  onSubgraphChange: (nextSubgraph: string) => void;
  fallbackNodeTypes: readonly string[];
  fallbackEdgeTypes: readonly string[];
  loading?: boolean;
  onApplyPattern: (pattern: GraphPatternFilterValues) => void;
  onResetPattern: () => void;
}

type PatternInputMode = "slots" | "expr";

export function GraphPatternBar({
  subgraph,
  onSubgraphChange,
  fallbackNodeTypes,
  fallbackEdgeTypes,
  loading = false,
  onApplyPattern,
  onResetPattern,
}: GraphPatternBarProps) {
  const { isZh, tx } = useI18n();
  const [mode, setMode] = useState<PatternInputMode>("slots");
  const [subgraphs, setSubgraphs] = useState<SubgraphInfo[]>([]);
  const [schemaNodes, setSchemaNodes] = useState<BlueNodeTypeDef[]>([]);
  const [schemaEdges, setSchemaEdges] = useState<BlueEdgeTypeDef[]>([]);

  const [srcType, setSrcType] = useState("");
  const [edgeType, setEdgeType] = useState("");
  const [dstType, setDstType] = useState("");
  const [exprText, setExprText] = useState("");
  const [validationMsg, setValidationMsg] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      const list = await xblueApi.listSubgraphs().catch(() => []);
      if (!cancelled && Array.isArray(list)) {
        setSubgraphs(list);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      const schema = await xblueApi.getSchema(subgraph).catch(() => null);
      if (cancelled) return;
      setSchemaNodes(
        Array.isArray(schema?.node_types) ? schema.node_types : []
      );
      setSchemaEdges(
        Array.isArray(schema?.edge_types) ? schema.edge_types : []
      );
    })();
    return () => {
      cancelled = true;
    };
  }, [subgraph]);

  const nodeDisplayMap = useMemo(() => {
    const m = new Map<string, string>();
    for (const nt of schemaNodes) {
      if (nt.name) {
        m.set(
          nt.name,
          isZh && nt.display_name && nt.display_name !== nt.name
            ? `${nt.name} · ${nt.display_name}`
            : nt.name
        );
      }
    }
    return m;
  }, [isZh, schemaNodes]);

  const allNodeNames = useMemo(() => {
    const set = new Set<string>();
    for (const nt of schemaNodes) {
      if (nt.name) set.add(nt.name);
    }
    for (const t of fallbackNodeTypes) {
      if (t) set.add(t);
    }
    return Array.from(set).sort();
  }, [fallbackNodeTypes, schemaNodes]);

  const knownNodeSet = useMemo(() => new Set(allNodeNames), [allNodeNames]);

  const knownEdgeSet = useMemo(() => {
    const s = new Set<string>();
    for (const et of schemaEdges) {
      if (et.name) s.add(et.name);
    }
    for (const e of fallbackEdgeTypes) {
      if (e) s.add(e);
    }
    return s;
  }, [fallbackEdgeTypes, schemaEdges]);

  const srcOptions = useMemo<SelectOptionItem[]>(() => {
    const opts: SelectOptionItem[] = [
      {
        value: "",
        label: tx("源类型", "Source"),
        text: tx("源类型", "Source"),
      },
    ];
    for (const name of allNodeNames) {
      const label = nodeDisplayMap.get(name) ?? name;
      opts.push({ value: name, label, text: label });
    }
    return opts;
  }, [allNodeNames, nodeDisplayMap, tx]);

  const edgeOptions = useMemo<SelectOptionItem[]>(() => {
    const { valid, others } = prioritizeEdgeTypes(
      schemaEdges,
      fallbackEdgeTypes,
      srcType
    );
    const opts: SelectOptionItem[] = [
      {
        value: "",
        label: tx("关系类型", "Relation"),
        text: tx("关系类型", "Relation"),
      },
    ];
    const validGroup =
      srcType && valid.length > 0
        ? tx("匹配关系", "Valid Relations")
        : undefined;
    const otherGroup =
      srcType && valid.length > 0 && others.length > 0
        ? tx("其他关系", "Other Relations")
        : undefined;

    for (const name of valid) {
      opts.push({ value: name, label: name, text: name, group: validGroup });
    }
    for (const name of others) {
      opts.push({ value: name, label: name, text: name, group: otherGroup });
    }
    return opts;
  }, [fallbackEdgeTypes, schemaEdges, srcType, tx]);

  const dstPrioritized = useMemo(
    () => prioritizeDstTypes(allNodeNames, schemaEdges, srcType, edgeType),
    [allNodeNames, edgeType, schemaEdges, srcType]
  );

  const dstOptions = useMemo<SelectOptionItem[]>(() => {
    const { valid, others } = dstPrioritized;
    const opts: SelectOptionItem[] = [
      {
        value: "",
        label: tx("目标类型", "Target"),
        text: tx("目标类型", "Target"),
      },
    ];
    const validGroup =
      valid.length > 0 ? tx("匹配目标", "Valid Targets") : undefined;
    const otherGroup =
      valid.length > 0 && others.length > 0
        ? tx("其他目标", "Other Targets")
        : undefined;

    for (const name of valid) {
      const label = nodeDisplayMap.get(name) ?? name;
      opts.push({ value: name, label, text: label, group: validGroup });
    }
    for (const name of others) {
      const label = nodeDisplayMap.get(name) ?? name;
      opts.push({ value: name, label, text: label, group: otherGroup });
    }
    return opts;
  }, [dstPrioritized, nodeDisplayMap, tx]);

  const handleSrcChange = (nextSrc: string) => {
    setSrcType(nextSrc);
    setValidationMsg(null);
  };

  const handleEdgeChange = (nextEdge: string) => {
    setEdgeType(nextEdge);
    setValidationMsg(null);
    if (nextEdge) {
      const { autoLockDst } = prioritizeDstTypes(
        allNodeNames,
        schemaEdges,
        srcType,
        nextEdge
      );
      if (autoLockDst && !dstType) {
        setDstType(autoLockDst);
      }
    }
  };

  const handleModeSwitch = (nextMode: PatternInputMode) => {
    setValidationMsg(null);
    if (nextMode === "expr") {
      setExprText(formatGraphPatternExpression(srcType, edgeType, dstType));
    }
    setMode(nextMode);
  };

  const handleApply = (e: React.FormEvent) => {
    e.preventDefault();
    setValidationMsg(null);
    if (mode === "expr") {
      const parsed = parseGraphPatternExpression(
        exprText,
        knownNodeSet,
        knownEdgeSet
      );
      if (!parsed.ok) {
        if (parsed.errorCode === "SYNTAX_ERROR") {
          setValidationMsg(
            tx(
              `格式示例: ${parsed.message}`,
              `Format example: ${parsed.message}`
            )
          );
        } else {
          setValidationMsg(
            tx(
              `未定义类型: ${parsed.message}`,
              `Unknown schema type: ${parsed.message}`
            )
          );
        }
        return;
      }
      setSrcType(parsed.srcType);
      setEdgeType(parsed.edgeType);
      setDstType(parsed.dstType);
      onApplyPattern({
        subgraph,
        srcType: parsed.srcType,
        edgeType: parsed.edgeType,
        dstType: parsed.dstType,
      });
      return;
    }

    onApplyPattern({
      subgraph,
      srcType,
      edgeType,
      dstType,
    });
  };

  const handleReset = () => {
    setSrcType("");
    setEdgeType("");
    setDstType("");
    setExprText("");
    setValidationMsg(null);
    onResetPattern();
  };

  const subgraphOptions = useMemo<SelectOptionItem[]>(() => {
    const items: SelectOptionItem[] = [
      {
        value: "main",
        label: "main",
        text: "main",
      },
    ];
    for (const sg of subgraphs) {
      if (sg.slug && sg.slug !== "main") {
        items.push({
          value: sg.slug,
          label: sg.slug,
          text: `${sg.slug} ${sg.display_name}`,
        });
      }
    }
    return items;
  }, [subgraphs]);

  return (
    <Card
      className="shrink-0 px-3 py-2 space-y-1.5"
      data-testid="graph-pattern-bar"
    >
      <form
        onSubmit={handleApply}
        className="flex flex-wrap items-center justify-between gap-2 text-xs"
      >
        <div className="flex flex-wrap items-center gap-1.5 min-w-0 flex-1">
          <SegmentedControl<PatternInputMode>
            value={mode}
            onChange={handleModeSwitch}
            items={[
              {
                key: "slots",
                label: tx("槽位", "Slots"),
                icon: <SlidersHorizontal className="h-3 w-3" />,
                testId: "pattern-mode-slots",
              },
              {
                key: "expr",
                label: tx("表达式", "Expr"),
                icon: <Code2 className="h-3 w-3" />,
                testId: "pattern-mode-expr",
              },
            ]}
          />

          {subgraphOptions.length > 1 && (
            <Select
              value={subgraph}
              onChange={(e) => onSubgraphChange(e.target.value)}
              options={subgraphOptions}
              aria-label={tx("子图", "Subgraph")}
              className="h-7 w-28 font-mono-tech text-[11px]"
              data-testid="pattern-subgraph-select"
            />
          )}

          {mode === "slots" ? (
            <div className="flex flex-wrap items-center gap-1 font-mono-tech">
              <span className="text-muted-foreground">(</span>
              <Select
                value={srcType}
                onChange={(e) => handleSrcChange(e.target.value)}
                options={srcOptions}
                aria-label={tx("源类型", "Source Type")}
                className="h-7 min-w-[116px] text-xs"
                data-testid="pattern-src-select"
              />
              <span className="text-muted-foreground">)-[</span>
              <Select
                value={edgeType}
                onChange={(e) => handleEdgeChange(e.target.value)}
                options={edgeOptions}
                aria-label={tx("关系类型", "Relation Type")}
                className="h-7 min-w-[116px] text-xs"
                data-testid="pattern-edge-select"
              />
              <span className="text-muted-foreground">]-&gt;(</span>
              <Select
                value={dstType}
                onChange={(e) => {
                  setDstType(e.target.value);
                  setValidationMsg(null);
                }}
                options={dstOptions}
                aria-label={tx("目标类型", "Target Type")}
                className="h-7 min-w-[116px] text-xs"
                data-testid="pattern-dst-select"
              />
              <span className="text-muted-foreground">)</span>
            </div>
          ) : (
            <div className="flex-1 min-w-[240px]">
              <Input
                value={exprText}
                onChange={(e) => {
                  setExprText(e.target.value);
                  setValidationMsg(null);
                }}
                placeholder="(Supplier)-[SUPPLIES]->(Factory)"
                className="h-7 font-mono-tech text-xs"
                data-testid="graph-pattern-expr-input"
              />
            </div>
          )}
        </div>

        <div className="flex items-center gap-1.5 shrink-0">
          <Button
            type="submit"
            size="sm"
            disabled={loading}
            data-testid="graph-pattern-apply-btn"
          >
            <Filter className="h-3 w-3" />
            {tx("筛选", "Filter")}
          </Button>
          <Button
            type="button"
            variant="outline"
            size="sm"
            disabled={loading}
            onClick={handleReset}
            data-testid="graph-pattern-reset-btn"
          >
            <RotateCcw className="h-3 w-3" />
            {tx("重置", "Reset")}
          </Button>
        </div>
      </form>

      {validationMsg && (
        <div
          className="text-[11px] font-mono-tech text-amber-600 dark:text-amber-400"
          data-testid="graph-pattern-validation-msg"
        >
          {validationMsg}
        </div>
      )}
    </Card>
  );
}
