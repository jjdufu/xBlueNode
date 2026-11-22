import { useCallback, useEffect, useMemo, useState } from "react";
import { useNavigate } from "react-router";
import {
  ArrowUpRight,
  BookOpen,
  CheckCircle2,
  Download,
  FileDown,
  Loader2,
} from "lucide-react";
import { useI18n } from "@/i18n";
import { usePrincipal } from "@/lib/permissions";
import type { ExportFormat } from "@/lib/api/enums";
import {
  computeLocalPassCount,
  countUnconfirmedProposals,
  xblueApi,
  type AcceptancePromptItem,
  type SchemaDoc,
  type SpecDocSection,
  type SubgraphSpecDoc,
} from "@/lib/api/xblue";
import { xblueviewApi, type ExportJobRecord } from "@/lib/api/xblueview";
import {
  Badge,
  Button,
  Card,
  Drawer,
  EmptyState,
  Select,
  StatusBadge,
} from "@/components/ui/primitives";
import { cn } from "@/lib/utils";

export interface ModelSpecOverviewProps {
  open: boolean;
  subgraphSlug: string;
  subgraphDisplayName?: string;
  publishedVersion: number;
  activeDraftId?: string | null;
  workingDoc: SchemaDoc;
  onClose: () => void;
}

const SECTION_TITLES: Readonly<Record<string, { zh: string; en: string }>> = {
  sec_01_overview: { zh: "1. 模型总览与健康度", en: "1. Overview & Health" },
  sec_02_semantic_layers: {
    zh: "2. 业务域与语义分层",
    en: "2. Domain & Semantic Layers",
  },
  sec_03_entity_dictionary: {
    zh: "3. 实体属性与指标字典",
    en: "3. Entity & Metric Dictionary",
  },
  sec_04_topology_matrix: {
    zh: "4. 拓扑关系与因果传导",
    en: "4. Topology & Propagation",
  },
  sec_05_compute_formulas: {
    zh: "5. 聚合与派生公式",
    en: "5. Rollups & Formulas",
  },
  sec_06_source_mappings: {
    zh: "6. 数据湖映射规则",
    en: "6. Source Mappings",
  },
  sec_07_acceptance_prompts: {
    zh: "7. 推荐验收问数集",
    en: "7. Acceptance Prompts",
  },
};

const EXPORT_FORMATS: readonly ExportFormat[] = ["PDF", "DOCX", "MARKDOWN"];

function buildFallbackSpecDoc(
  subgraphSlug: string,
  displayName: string,
  version: number,
  draftId: string | null | undefined,
  doc: SchemaDoc
): SubgraphSpecDoc {
  const passCount = computeLocalPassCount(doc);
  const unconfirmedCount = countUnconfirmedProposals(doc);
  const totalMetrics = doc.node_types.reduce(
    (acc, nt) => acc + nt.properties.filter((p) => p.kind === "METRIC").length,
    0
  );

  const primary = doc.node_types[0];
  const secondary = doc.node_types[1] ?? primary;
  const pName = primary?.name ?? "Entity";
  const pLabel = primary?.display_name || pName;
  const sName = secondary?.name ?? pName;
  const sLabel = secondary?.display_name || sName;
  const firstAttr =
    primary?.properties.find((p) => p.kind === "ATTRIBUTE")?.display_name ||
    primary?.properties.find((p) => p.kind === "ATTRIBUTE")?.name ||
    "状态";
  const m1Prop =
    primary?.properties.find((p) => p.kind === "METRIC") ??
    secondary?.properties.find((p) => p.kind === "METRIC");
  const m1Name = m1Prop?.name ?? "value";
  const m1Label = m1Prop?.display_name || m1Name;
  const edge = doc.edge_types[0];
  const edgeSrc = edge?.src_type_name ?? pName;
  const edgeDst = edge?.dst_type_name ?? sName;
  const edgeName = edge?.name ?? "REL";

  const prompts: AcceptancePromptItem[] = [
    {
      id: "q1",
      question: `按${firstAttr}筛选并列出${pLabel}的${m1Label}明细`,
      expected_grounding_mode: "DOMAIN_OBJECT",
      target_node_types: [pName],
      target_metrics: [m1Name],
    },
    {
      id: "q2",
      question: `按${firstAttr}汇总统计${pLabel}的${m1Label}`,
      expected_grounding_mode: "SEMANTIC_METRIC",
      target_node_types: [pName],
      target_metrics: [m1Name],
    },
    {
      id: "q3",
      question: `关联 ${edgeSrc} 与 ${edgeDst}（沿 ${edgeName}）对比${m1Label}`,
      expected_grounding_mode: "GUARDED_SQL",
      target_node_types: edgeSrc === edgeDst ? [edgeSrc] : [edgeSrc, edgeDst],
      target_metrics: [m1Name],
    },
    {
      id: "q4",
      question: `查询${sLabel}中${m1Label}排名前 10 的对象`,
      expected_grounding_mode: "DOMAIN_OBJECT",
      target_node_types: [sName],
      target_metrics: [m1Name],
    },
    {
      id: "q5",
      question: `排查${sLabel}中${m1Label}异常偏低或存在缺口的记录`,
      expected_grounding_mode: "GUARDED_SQL",
      target_node_types: [sName],
      target_metrics: [m1Name],
    },
    {
      id: "q6",
      question: `穿透分析 ${edgeSrc} 波动沿 ${edgeName} 对下游 ${edgeDst} 的影响范围`,
      expected_grounding_mode: "DOMAIN_OBJECT",
      target_node_types: edgeSrc === edgeDst ? [edgeSrc] : [edgeSrc, edgeDst],
      target_metrics: [m1Name],
    },
  ];

  const sections: SpecDocSection[] = [
    {
      key: "sec_01_overview",
      title: "模型总览与健康度",
      summary: "",
      rows: [
        {
          subgraph_slug: subgraphSlug,
          display_name: displayName,
          state: draftId ? "DRAFT" : "PUBLISHED",
          version,
          node_type_count: doc.node_types.length,
          edge_type_count: doc.edge_types.length,
          metric_count: totalMetrics,
          mapping_count: doc.mappings.length,
          pass_count: passCount,
          unconfirmed_count: unconfirmedCount,
        },
      ],
    },
    {
      key: "sec_02_semantic_layers",
      title: "业务域与语义分层矩阵",
      summary: "",
      rows: doc.node_types.map((nt) => ({
        node_type: nt.name,
        display_name: nt.display_name,
        domain_group: nt.domain_group ?? "默认域",
        semantic_layer_tier: nt.semantic_layer_tier ?? "UNCLASSIFIED",
        maturity: nt.maturity ?? "CONFIRMED",
      })),
    },
    {
      key: "sec_03_entity_dictionary",
      title: "实体、属性与指标字典",
      summary: "",
      rows: doc.node_types.flatMap((nt) =>
        nt.properties.map((prop) => ({
          node_type: nt.name,
          property: prop.name,
          display_name: prop.display_name,
          kind: prop.kind,
          data_type: prop.data_type,
          required: prop.required,
          unit: prop.unit ?? "-",
          pii_category: prop.pii_category,
          maturity: prop.maturity ?? "CONFIRMED",
        }))
      ),
    },
    {
      key: "sec_04_topology_matrix",
      title: "拓扑关系与因果传导",
      summary: "",
      rows: doc.edge_types.map((et) => ({
        edge_type: et.name,
        src_type_name: et.src_type_name,
        dst_type_name: et.dst_type_name,
        cardinality: et.cardinality,
        impact_weight: et.impact_weight,
        maturity: et.maturity ?? "CONFIRMED",
      })),
    },
    {
      key: "sec_05_compute_formulas",
      title: "跨边聚合与节点内派生公式",
      summary: "",
      rows: doc.node_types.flatMap((nt) => [
        ...nt.agg_rules.map((agg) => ({
          node_type: nt.name,
          rule_kind: "AGG",
          target_metric: agg.target_metric,
          formula: `${agg.op}(${agg.source_type_name}.${agg.source_metric} via ${agg.via_link_type})`,
          maturity: agg.maturity ?? "CONFIRMED",
        })),
        ...nt.derived_rules.map((der) => ({
          node_type: nt.name,
          rule_kind: "DERIVED",
          target_metric: der.target_metric,
          formula: der.expression,
          maturity: der.maturity ?? "CONFIRMED",
        })),
      ]),
    },
    {
      key: "sec_06_source_mappings",
      title: "数据湖映射与入图规则",
      summary: "",
      rows: doc.mappings.map((m) => ({
        mapping_name: m.name,
        dataset: m.dataset,
        node_type: m.node_type,
        key_column: m.key_column,
        link_count: m.links.length,
        last_status: "NEVER",
      })),
    },
    {
      key: "sec_07_acceptance_prompts",
      title: "推荐验收问数集",
      summary: "",
      rows: prompts.map((p) => ({
        id: p.id,
        question: p.question,
        expected_grounding_mode: p.expected_grounding_mode,
      })),
    },
  ];

  const mdLines = [
    `# ${displayName} (${subgraphSlug})`,
    "",
    ...prompts.map(
      (p) => `- **${p.id}** (\`${p.expected_grounding_mode}\`): ${p.question}`
    ),
  ];

  return {
    subgraph_slug: subgraphSlug,
    display_name: displayName,
    version,
    draft_id: draftId ?? null,
    pass_count: passCount,
    unconfirmed_count: unconfirmedCount,
    sections,
    acceptance_prompts: prompts,
    markdown: mdLines.join("\n"),
  };
}

function cellStr(val: unknown): string {
  if (val === null || val === undefined || val === "") return "-";
  if (typeof val === "boolean") return val ? "Yes" : "No";
  if (typeof val === "number") return String(val);
  if (typeof val === "string") return val;
  if (Array.isArray(val)) return val.map(String).join(", ");
  return JSON.stringify(val);
}

export function ModelSpecOverview({
  open,
  subgraphSlug,
  subgraphDisplayName,
  publishedVersion,
  activeDraftId,
  workingDoc,
  onClose,
}: ModelSpecOverviewProps) {
  const navigate = useNavigate();
  const { isZh, tx, tEnum } = useI18n();
  const { can } = usePrincipal();
  const canExport = can("EXPORT_DATA");

  const [specDoc, setSpecDoc] = useState<SubgraphSpecDoc | null>(null);
  const [loading, setLoading] = useState<boolean>(false);
  const [activeSectionKey, setActiveSectionKey] =
    useState<string>("sec_01_overview");

  const [exportFormat, setExportFormat] = useState<ExportFormat>("PDF");
  const [exporting, setExporting] = useState<boolean>(false);
  const [exportJob, setExportJob] = useState<ExportJobRecord | null>(null);
  const [exportError, setExportError] = useState<string | null>(null);

  const loadSpec = useCallback(async () => {
    setLoading(true);
    try {
      const res = await xblueApi.getSubgraphSpecDoc(subgraphSlug, {
        draftId: activeDraftId ?? undefined,
        format: "json",
      });
      if (res && Array.isArray(res.sections) && res.sections.length > 0) {
        setSpecDoc(res);
      } else {
        setSpecDoc(
          buildFallbackSpecDoc(
            subgraphSlug,
            subgraphDisplayName || subgraphSlug,
            publishedVersion,
            activeDraftId,
            workingDoc
          )
        );
      }
    } catch {
      setSpecDoc(
        buildFallbackSpecDoc(
          subgraphSlug,
          subgraphDisplayName || subgraphSlug,
          publishedVersion,
          activeDraftId,
          workingDoc
        )
      );
    } finally {
      setLoading(false);
    }
  }, [
    subgraphSlug,
    subgraphDisplayName,
    publishedVersion,
    activeDraftId,
    workingDoc,
  ]);

  useEffect(() => {
    if (!open) return;
    setExportJob(null);
    setExportError(null);
    setActiveSectionKey("sec_01_overview");
    void loadSpec();
  }, [open, loadSpec]);

  const effectiveSpec = useMemo(
    () =>
      specDoc ??
      buildFallbackSpecDoc(
        subgraphSlug,
        subgraphDisplayName || subgraphSlug,
        publishedVersion,
        activeDraftId,
        workingDoc
      ),
    [
      specDoc,
      subgraphSlug,
      subgraphDisplayName,
      publishedVersion,
      activeDraftId,
      workingDoc,
    ]
  );

  const handleExport = async () => {
    if (exporting) return;
    setExporting(true);
    setExportError(null);
    try {
      const docTitle =
        effectiveSpec.title ||
        `${effectiveSpec.display_name || effectiveSpec.subgraph_slug} ${tx("模型全景规格书", "Model Specification")}`;
      const job = await xblueviewApi.createExport({
        export_format: exportFormat,
        source: {
          kind: "MARKDOWN",
          title: docTitle,
          markdown: effectiveSpec.markdown,
        },
        options: {
          title: docTitle,
          watermark: true,
        },
      });
      setExportJob(job);
    } catch (err) {
      setExportError(
        err instanceof Error
          ? err.message
          : tx("导出失败", "Export failed")
      );
    } finally {
      setExporting(false);
    }
  };

  const scrollToSection = (key: string) => {
    setActiveSectionKey(key);
    if (typeof document !== "undefined") {
      const el = document.getElementById(`spec-section-${key}`);
      el?.scrollIntoView({ behavior: "smooth", block: "start" });
    }
  };

  const sectionHeading = (sec: SpecDocSection): string => {
    const localized = SECTION_TITLES[sec.key];
    if (localized) {
      return isZh ? localized.zh : localized.en;
    }
    return sec.title;
  };

  const renderSectionRows = (sec: SpecDocSection) => {
    const rows = Array.isArray(sec.rows) ? sec.rows : [];
    if (rows.length === 0) {
      return <EmptyState title={tx("暂无记录", "No records")} className="py-5" />;
    }

    if (sec.key === "sec_01_overview") {
      const row = rows[0] ?? {};
      const metrics: readonly { label: string; value: string }[] = [
        {
          label: tx("子图标识", "Subgraph"),
          value: cellStr(row.subgraph_slug ?? effectiveSpec.subgraph_slug),
        },
        {
          label: tx("状态", "State"),
          value: tEnum(cellStr(row.state)),
        },
        {
          label: tx("基线版本", "Version"),
          value: `v${cellStr(row.version ?? effectiveSpec.version)}`,
        },
        {
          label: tx("节点类型数", "Node Types"),
          value: cellStr(row.node_type_count),
        },
        {
          label: tx("关系边数", "Relations"),
          value: cellStr(row.edge_type_count),
        },
        {
          label: tx("指标总数", "Metrics"),
          value: cellStr(row.metric_count),
        },
        {
          label: tx("通过项", "Passed"),
          value: cellStr(row.pass_count ?? effectiveSpec.pass_count),
        },
        {
          label: tx("待确认", "Proposed"),
          value: cellStr(
            row.unconfirmed_count ?? effectiveSpec.unconfirmed_count
          ),
        },
      ];
      return (
        <div className="grid grid-cols-2 sm:grid-cols-4 gap-2">
          {metrics.map((m) => (
            <div
              key={m.label}
              className="rounded border border-border bg-muted/20 px-3 py-2 space-y-0.5"
            >
              <div className="text-[11px] text-muted-foreground">{m.label}</div>
              <div className="text-sm font-semibold font-mono-tech tabular-nums">
                {m.value}
              </div>
            </div>
          ))}
        </div>
      );
    }

    const firstRow = rows[0] ?? {};
    const columns = Object.keys(firstRow);

    return (
      <div className="overflow-x-auto rounded-md border border-border">
        <table className="w-full border-collapse text-left text-xs">
          <thead className="bg-muted/50 text-[11px] text-muted-foreground">
            <tr>
              {columns.map((col) => (
                <th key={col} className="px-2.5 py-2 font-mono-tech font-medium">
                  {col}
                </th>
              ))}
            </tr>
          </thead>
          <tbody className="divide-y divide-border">
            {rows.map((r, rIdx) => (
              <tr key={rIdx} className="hover:bg-muted/20">
                {columns.map((col) => {
                  const val = r[col];
                  const isEnumCol =
                    col === "maturity" ||
                    col === "semantic_layer_tier" ||
                    col === "pii_category" ||
                    col === "last_status";
                  return (
                    <td
                      key={col}
                      className="px-2.5 py-1.5 font-mono-tech align-middle"
                    >
                      {isEnumCol && typeof val === "string" ? (
                        <Badge
                          variant={
                            val === "PROPOSED"
                              ? "warning"
                              : val === "STATIC_FACT" ||
                                  val === "EVENT_DYNAMIC" ||
                                  val === "CUBE_ROLLUP"
                                ? "xblue"
                                : "outline"
                          }
                        >
                          {val === "UNCLASSIFIED"
                            ? tx("未分层", "Unassigned")
                            : val === "NEVER"
                              ? tx("未同步", "Never")
                              : tEnum(val)}
                        </Badge>
                      ) : (
                        <span>{cellStr(val)}</span>
                      )}
                    </td>
                  );
                })}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    );
  };

  return (
    <Drawer
      open={open}
      onClose={onClose}
      widthClass="max-w-6xl"
      testId="model-spec-overview"
      closeTestId="close-spec-overview-btn"
      title={
        <div className="flex flex-wrap items-center gap-2">
          <BookOpen className="h-4 w-4 text-xblue shrink-0" />
          <span>{tx("模型规格书", "Spec Doc")}</span>
          <Badge variant="outline" className="font-mono-tech">
            {effectiveSpec.subgraph_slug} · v{effectiveSpec.version}
          </Badge>
          <Badge variant="success" className="tabular-nums">
            <CheckCircle2 className="h-3 w-3 mr-1" />
            {tx(
              `${effectiveSpec.pass_count} 项通过`,
              `${effectiveSpec.pass_count} Passed`
            )}
          </Badge>
          {effectiveSpec.unconfirmed_count > 0 && (
            <Badge variant="warning" className="tabular-nums">
              {tx(
                `${effectiveSpec.unconfirmed_count} 待确认`,
                `${effectiveSpec.unconfirmed_count} Proposed`
              )}
            </Badge>
          )}
        </div>
      }
    >
      {/* Top Export Action Bar */}
      <div className="flex flex-wrap items-center justify-between gap-2 rounded-md border border-border bg-muted/20 px-3 py-2">
        <div className="flex flex-wrap items-center gap-2 min-w-0">
          <span className="font-semibold text-sm truncate">
            {effectiveSpec.display_name || effectiveSpec.subgraph_slug}
          </span>
          {loading && (
            <Loader2 className="h-3.5 w-3.5 animate-spin text-xblue" />
          )}
          {exportJob && (
            <div
              className="inline-flex items-center gap-1.5"
              data-testid="spec-export-job-status"
            >
              <StatusBadge status={exportJob.status} />
              {exportJob.status === "SUCCEEDED" && (
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  data-testid="spec-export-download-btn"
                  onClick={() => void xblueviewApi.downloadExport(exportJob.id)}
                  className="h-7 px-2"
                >
                  <Download className="h-3 w-3 text-xblue" />
                  {tx("下载", "Download")}
                </Button>
              )}
            </div>
          )}
          {exportError && (
            <span className="text-red-600 dark:text-red-400">{exportError}</span>
          )}
        </div>

        <div className="flex items-center gap-2 shrink-0">
          <Select
            data-testid="spec-export-format-select"
            value={exportFormat}
            aria-label={tx("导出格式", "Export Format")}
            onChange={(e) => setExportFormat(e.target.value as ExportFormat)}
            className="h-8 w-32 font-mono-tech"
          >
            {EXPORT_FORMATS.map((fmt) => (
              <option key={fmt} value={fmt}>
                {fmt}
              </option>
            ))}
          </Select>
          <Button
            type="button"
            variant="primary"
            size="sm"
            data-testid="spec-export-btn"
            disabled={exporting || !canExport}
            onClick={() => void handleExport()}
          >
            {exporting ? (
              <Loader2 className="h-3.5 w-3.5 animate-spin" />
            ) : (
              <FileDown className="h-3.5 w-3.5" />
            )}
            {exporting ? tx("导出中…", "Exporting…") : tx("导出文档", "Export")}
          </Button>
        </div>
      </div>

      {/* Left 7-Section Anchor TOC + Right Specification Content */}
      <div className="grid grid-cols-1 md:grid-cols-[220px_minmax(0,1fr)] gap-4 pt-1">
        <nav
          data-testid="spec-doc-toc"
          aria-label={tx("章节目录", "Table of Contents")}
          className="md:sticky md:top-0 self-start rounded-md border border-border bg-card p-2 space-y-1"
        >
          {effectiveSpec.sections.map((sec) => {
            const active = activeSectionKey === sec.key;
            return (
              <button
                key={sec.key}
                type="button"
                data-testid={`spec-toc-${sec.key}`}
                onClick={() => scrollToSection(sec.key)}
                className={cn(
                  "w-full flex items-center justify-between gap-2 rounded px-2.5 py-1.5 text-left text-xs transition-colors cursor-pointer",
                  active
                    ? "bg-xblue-soft text-xblue font-semibold"
                    : "text-muted-foreground hover:text-foreground hover:bg-muted/60"
                )}
              >
                <span className="truncate">{sectionHeading(sec)}</span>
                <span className="font-mono-tech text-[10px] tabular-nums opacity-80">
                  {Array.isArray(sec.rows) ? sec.rows.length : 0}
                </span>
              </button>
            );
          })}
        </nav>

        <div className="space-y-4 min-w-0" data-testid="spec-doc-sections">
          {effectiveSpec.sections.map((sec) => (
            <Card
              key={sec.key}
              id={`spec-section-${sec.key}`}
              data-testid={`spec-section-${sec.key}`}
              className="p-3.5 space-y-2.5"
            >
              <div className="flex flex-wrap items-center justify-between gap-2 border-b border-border pb-2">
                <h3 className="text-sm font-semibold">{sectionHeading(sec)}</h3>
                {sec.summary && (
                  <span className="text-[11px] text-muted-foreground font-mono-tech">
                    {sec.summary}
                  </span>
                )}
              </div>

              {sec.key === "sec_07_acceptance_prompts" ? (
                <div
                  className="grid grid-cols-1 lg:grid-cols-2 gap-2.5"
                  data-testid="spec-acceptance-prompts-grid"
                >
                  {effectiveSpec.acceptance_prompts.map((item) => (
                    <div
                      key={item.id}
                      data-testid={`spec-prompt-card-${item.id}`}
                      className="rounded-md border border-border bg-muted/15 p-3 flex flex-col justify-between gap-2.5"
                    >
                      <div className="space-y-1.5">
                        <div className="flex items-center justify-between gap-2">
                          <div className="flex items-center gap-1.5">
                            <Badge
                              variant="outline"
                              className="font-mono-tech font-semibold uppercase"
                            >
                              {item.id}
                            </Badge>
                            <Badge
                              variant="xblue"
                              data-testid={`spec-prompt-mode-${item.id}`}
                            >
                              {tEnum(item.expected_grounding_mode)}
                            </Badge>
                          </div>
                          <Button
                            type="button"
                            variant="outline"
                            size="sm"
                            data-testid={`spec-prompt-ask-${item.id}`}
                            onClick={() => {
                              onClose();
                              navigate(
                                `/query?q=${encodeURIComponent(item.question)}`
                              );
                            }}
                            className="h-7 px-2.5"
                          >
                            <span>{tx("去问数", "Ask")}</span>
                            <ArrowUpRight className="h-3 w-3 text-xblue" />
                          </Button>
                        </div>

                        <div className="text-xs font-medium text-foreground leading-relaxed">
                          {item.question}
                        </div>
                      </div>

                      <div className="flex flex-wrap items-center gap-1">
                        {item.target_node_types.map((nt) => (
                          <Badge key={`nt-${nt}`} variant="neutral">
                            {nt}
                          </Badge>
                        ))}
                        {item.target_metrics.map((m) => (
                          <Badge key={`m-${m}`} variant="outline">
                            {m}
                          </Badge>
                        ))}
                      </div>
                    </div>
                  ))}
                </div>
              ) : (
                renderSectionRows(sec)
              )}
            </Card>
          ))}
        </div>
      </div>
    </Drawer>
  );
}
