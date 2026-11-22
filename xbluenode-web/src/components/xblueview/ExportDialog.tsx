import { useCallback, useEffect, useMemo, useState } from "react";
import {
  Download,
  FileSpreadsheet,
  FileText,
  Presentation,
  Trash2,
  Upload,
} from "lucide-react";
import { useI18n } from "@/i18n";
import type { ExportFormat } from "@/lib/api/enums";
import {
  xblueviewApi,
  type CreateExportRequest,
  type DrillPathItem,
  type ExportJobRecord,
  type ExportOrientation,
  type ExportPageSize,
  type ExportSource,
  type ExportTemplateRecord,
} from "@/lib/api/xblueview";
import { usePrincipal } from "@/lib/permissions";
import {
  Badge,
  Button,
  IconButton,
  Input,
  Modal,
  Select,
} from "@/components/ui/primitives";
import { sanitizeExportFilters } from "./logic";

export interface ExportDialogTarget {
  kind: "DASHBOARD" | "WIDGET";
  dashboardId: string;
  dashboardName: string;
  widgetId?: string;
  widgetTitle?: string;
  filters: Record<string, unknown>;
  drillPath?: DrillPathItem[];
}

export interface ExportDialogProps {
  open: boolean;
  target: ExportDialogTarget | null;
  onClose: () => void;
  onExportCreated: (job: ExportJobRecord) => void;
}

const EXPORT_FORMAT_ITEMS: readonly {
  format: ExportFormat;
  label: string;
  zhDesc: string;
  enDesc: string;
}[] = [
  {
    format: "PDF",
    label: "PDF",
    zhDesc: "A4/A3 矢量排版报告",
    enDesc: "A4/A3 vector report",
  },
  {
    format: "XLSX",
    label: "XLSX",
    zhDesc: "多工作表原生工作簿",
    enDesc: "Multi-sheet workbook",
  },
  {
    format: "CSV",
    label: "CSV",
    zhDesc: "UTF-8 明细表文件",
    enDesc: "UTF-8 tabular data",
  },
  {
    format: "DOCX",
    label: "DOCX",
    zhDesc: "Word 报告与母版替换",
    enDesc: "Word report with template",
  },
  {
    format: "PPTX",
    label: "PPTX",
    zhDesc: "演示文稿与母版图表",
    enDesc: "Slide deck with template",
  },
  {
    format: "MARKDOWN",
    label: "MARKDOWN",
    zhDesc: "GFM 文本与 SVG 归档",
    enDesc: "GFM text & SVG archive",
  },
];

function resolveBrowserTimezone(): string {
  try {
    return Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC";
  } catch {
    return "UTC";
  }
}

export function ExportDialog({
  open,
  target,
  onClose,
  onExportCreated,
}: ExportDialogProps) {
  const { tx, tEnum, isZh, fmtNum } = useI18n();
  const { can } = usePrincipal();
  const canExport = can("EXPORT_DATA");

  const [exportFormat, setExportFormat] = useState<ExportFormat>("PDF");
  const [title, setTitle] = useState("");
  const [subtitle, setSubtitle] = useState("");
  const [watermarkEnabled, setWatermarkEnabled] = useState(true);
  const [watermarkCustomText, setWatermarkCustomText] = useState("");
  const [pageSize, setPageSize] = useState<ExportPageSize>("A4");
  const [orientation, setOrientation] = useState<ExportOrientation>("portrait");
  const [csvBom, setCsvBom] = useState(true);

  const [templates, setTemplates] = useState<ExportTemplateRecord[]>([]);
  const [templatesLoading, setTemplatesLoading] = useState(false);
  const [selectedTemplateId, setSelectedTemplateId] = useState<string>("");
  const [placeholderValues, setPlaceholderValues] = useState<
    Record<string, string>
  >({});

  const [uploadName, setUploadName] = useState("");
  const [uploadFile, setUploadFile] = useState<File | null>(null);
  const [uploadingTemplate, setUploadingTemplate] = useState(false);
  const [deletingTemplateId, setDeletingTemplateId] = useState<string | null>(
    null
  );

  const [submitting, setSubmitting] = useState(false);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  useEffect(() => {
    if (!open || !target) return;
    const defaultTitle =
      target.kind === "WIDGET"
        ? target.widgetTitle?.trim() || target.dashboardName
        : target.dashboardName;
    setTitle(defaultTitle);
    setSubtitle("");
    setWatermarkEnabled(true);
    setWatermarkCustomText("");
    setErrorMsg(null);
  }, [open, target]);

  const supportsTemplates =
    exportFormat === "DOCX" || exportFormat === "PPTX";

  const loadTemplates = useCallback(async () => {
    setTemplatesLoading(true);
    try {
      const res = await xblueviewApi.listExportTemplates({ limit: 100 });
      setTemplates(Array.isArray(res?.items) ? res.items : []);
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setTemplatesLoading(false);
    }
  }, []);

  useEffect(() => {
    if (open && supportsTemplates) {
      void loadTemplates();
    }
  }, [loadTemplates, open, supportsTemplates]);

  const matchingTemplates = useMemo(
    () =>
      supportsTemplates
        ? templates.filter((t) => t.export_format === exportFormat)
        : [],
    [exportFormat, supportsTemplates, templates]
  );

  useEffect(() => {
    if (!supportsTemplates) {
      setSelectedTemplateId("");
      return;
    }
    if (
      selectedTemplateId &&
      !matchingTemplates.some((t) => t.id === selectedTemplateId)
    ) {
      setSelectedTemplateId("");
    }
  }, [matchingTemplates, selectedTemplateId, supportsTemplates]);

  const selectedTemplate = useMemo(
    () => matchingTemplates.find((t) => t.id === selectedTemplateId) ?? null,
    [matchingTemplates, selectedTemplateId]
  );

  useEffect(() => {
    if (!selectedTemplate) {
      setPlaceholderValues({});
      return;
    }
    setPlaceholderValues((prev) => {
      const next: Record<string, string> = {};
      for (const key of selectedTemplate.placeholders ?? []) {
        next[key] = prev[key] ?? "";
      }
      return next;
    });
  }, [selectedTemplate]);

  const activeFilterEntries = useMemo(
    () =>
      target ? Object.entries(sanitizeExportFilters(target.filters)) : [],
    [target]
  );

  const handleUploadTemplate = async () => {
    if (!canExport) return;
    if (exportFormat !== "DOCX" && exportFormat !== "PPTX") return;
    if (!uploadFile) {
      setErrorMsg(tx("未附加母版文件", "Template file is required"));
      return;
    }
    const resolvedName =
      uploadName.trim() ||
      uploadFile.name.replace(/\.(docx|pptx)$/i, "").trim() ||
      `tpl-${Date.now().toString(36).slice(-4)}`;

    setUploadingTemplate(true);
    setErrorMsg(null);
    try {
      const created = await xblueviewApi.uploadExportTemplate(
        uploadFile,
        exportFormat,
        resolvedName
      );
      setTemplates((prev) => [
        created,
        ...prev.filter((item) => item.id !== created.id),
      ]);
      setSelectedTemplateId(created.id);
      setUploadName("");
      setUploadFile(null);
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setUploadingTemplate(false);
    }
  };

  const handleDeleteTemplate = async (templateId: string) => {
    if (!canExport) return;
    setDeletingTemplateId(templateId);
    setErrorMsg(null);
    try {
      await xblueviewApi.deleteExportTemplate(templateId);
      setTemplates((prev) => prev.filter((item) => item.id !== templateId));
      if (selectedTemplateId === templateId) {
        setSelectedTemplateId("");
      }
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setDeletingTemplateId(null);
    }
  };

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!target || !canExport) return;
    const cleanedFilters = sanitizeExportFilters(target.filters);
    const source: ExportSource =
      target.kind === "WIDGET" && target.widgetId
        ? {
            kind: "WIDGET",
            widget_id: target.widgetId,
            filters: cleanedFilters,
            drill_path: target.drillPath ?? [],
          }
        : {
            kind: "DASHBOARD",
            dashboard_id: target.dashboardId,
            filters: cleanedFilters,
          };

    const resolvedWatermark: boolean | string = !watermarkEnabled
      ? false
      : watermarkCustomText.trim().length > 0
        ? watermarkCustomText.trim()
        : true;

    const cleanPlaceholders: Record<string, string> = {};
    if (supportsTemplates && selectedTemplate) {
      for (const key of selectedTemplate.placeholders ?? []) {
        cleanPlaceholders[key] = placeholderValues[key] ?? "";
      }
    }

    const req: CreateExportRequest = {
      export_format: exportFormat,
      source,
      options: {
        title: title.trim() || undefined,
        subtitle: subtitle.trim() || undefined,
        watermark: resolvedWatermark,
        csv_bom: exportFormat === "CSV" ? csvBom : undefined,
        page_size: exportFormat === "PDF" ? pageSize : undefined,
        orientation: exportFormat === "PDF" ? orientation : undefined,
        placeholders:
          Object.keys(cleanPlaceholders).length > 0
            ? cleanPlaceholders
            : undefined,
        timezone: resolveBrowserTimezone(),
      },
      template_id:
        supportsTemplates && selectedTemplateId ? selectedTemplateId : null,
    };

    setSubmitting(true);
    setErrorMsg(null);
    try {
      const createdJob = await xblueviewApi.createExport(req);
      onExportCreated(createdJob);
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <Modal
      open={open && Boolean(target)}
      onClose={onClose}
      testId="export-dialog"
      maxWidthClass="max-w-xl"
      title={
        target ? (
          <>
            <Download className="h-4 w-4 text-xblue shrink-0" />
            <span className="truncate">
              {target.kind === "WIDGET"
                ? `${tx("导出组件", "Export Widget")} · ${
                    target.widgetTitle || tx("组件", "Widget")
                  }`
                : `${tx("出品导出", "Export")} · ${target.dashboardName}`}
            </span>
            <Badge variant="xblue">{tEnum(target.kind)}</Badge>
          </>
        ) : (
          tx("出品导出", "Export")
        )
      }
      footer={
        <>
          <Button
            type="button"
            variant="outline"
            onClick={onClose}
            disabled={submitting}
          >
            {tx("取消", "Cancel")}
          </Button>
          <Button
            type="submit"
            form="export-dialog-form"
            disabled={submitting || !canExport}
            data-testid="submit-export-btn"
          >
            <Download className="h-3.5 w-3.5" />
            {submitting
              ? tx("提交中…", "Submitting…")
              : `${tx("导出", "Export")} ${exportFormat}`}
          </Button>
        </>
      }
    >
      {target && (
        <form
          id="export-dialog-form"
          onSubmit={(e) => void handleSubmit(e)}
          className="space-y-4 text-xs"
        >
          {errorMsg && (
            <div
              className="rounded border border-red-500/40 bg-red-500/10 px-3 py-2 text-red-700 dark:text-red-300"
              data-testid="export-dialog-error"
            >
              {errorMsg}
            </div>
          )}

          <div className="rounded-md border border-border bg-muted/25 px-3 py-2 flex flex-wrap items-center justify-between gap-2">
            <span className="text-muted-foreground">
              {tx(
                "服务端快照受当前账号行列安全策略约束",
                "Server snapshot respects active row/column security policies"
              )}
            </span>
            <div className="flex items-center gap-1.5 font-mono-tech text-[11px] tabular-nums">
              <Badge variant="outline">
                {tx(
                  `筛选 ${fmtNum(activeFilterEntries.length, 0)}`,
                  `Filters ${fmtNum(activeFilterEntries.length, 0)}`
                )}
              </Badge>
              {target.kind === "WIDGET" &&
                (target.drillPath?.length ?? 0) > 0 && (
                  <Badge variant="outline">
                    {tx(
                      `下钻 ${fmtNum(target.drillPath?.length ?? 0, 0)}`,
                      `Drill ${fmtNum(target.drillPath?.length ?? 0, 0)}`
                    )}
                  </Badge>
                )}
            </div>
          </div>

          <div className="space-y-1.5">
            <label className="block font-medium text-foreground">
              {tx("出品格式", "Format")}
            </label>
            <div className="grid grid-cols-2 sm:grid-cols-3 gap-2">
              {EXPORT_FORMAT_ITEMS.map((item) => {
                const selected = exportFormat === item.format;
                const Icon =
                  item.format === "XLSX" || item.format === "CSV"
                    ? FileSpreadsheet
                    : item.format === "PPTX"
                      ? Presentation
                      : FileText;
                return (
                  <button
                    key={item.format}
                    type="button"
                    onClick={() => setExportFormat(item.format)}
                    data-testid={`export-format-${item.format}`}
                    className={`rounded-md border p-2.5 text-left transition-colors cursor-pointer ${
                      selected
                        ? "border-xblue bg-xblue-soft/60 text-foreground"
                        : "border-border bg-card hover:bg-muted/50 text-muted-foreground hover:text-foreground"
                    }`}
                  >
                    <div className="flex items-center justify-between gap-1">
                      <span className="font-mono-tech font-semibold text-xs text-foreground">
                        {item.label}
                      </span>
                      <Icon
                        className={`h-3.5 w-3.5 ${
                          selected ? "text-xblue" : "text-muted-foreground"
                        }`}
                      />
                    </div>
                    <div className="text-[11px] text-muted-foreground mt-0.5 truncate">
                      {isZh ? item.zhDesc : item.enDesc}
                    </div>
                  </button>
                );
              })}
            </div>
          </div>

          <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
            <div className="space-y-1">
              <label className="block text-muted-foreground">
                {tx("报告主标题", "Title")}
              </label>
              <Input
                value={title}
                onChange={(e) => setTitle(e.target.value)}
                placeholder={tx("供应链全景周报", "Supply Chain Weekly Report")}
                className="h-8 text-xs"
                data-testid="export-title-input"
              />
            </div>
            <div className="space-y-1">
              <label className="block text-muted-foreground">
                {tx("副标题", "Subtitle")}
              </label>
              <Input
                value={subtitle}
                onChange={(e) => setSubtitle(e.target.value)}
                placeholder={tx("2026 Q1 经营复盘", "2026 Q1 Review")}
                className="h-8 text-xs"
                data-testid="export-subtitle-input"
              />
            </div>
          </div>

          <div className="rounded-md border border-border p-3 space-y-2.5">
            <label className="flex items-center justify-between gap-2 cursor-pointer">
              <div>
                <div className="font-medium text-foreground">
                  {tx("安全溯源水印", "Audit Watermark")}
                </div>
                <div className="text-[11px] text-muted-foreground">
                  {tx(
                    "默认嵌入当前账号与生成时间戳",
                    "Embeds username and timestamp by default"
                  )}
                </div>
              </div>
              <input
                type="checkbox"
                checked={watermarkEnabled}
                onChange={(e) => setWatermarkEnabled(e.target.checked)}
                className="h-4 w-4 accent-[#2563eb] cursor-pointer"
                data-testid="export-watermark-toggle"
              />
            </label>
            {watermarkEnabled && (
              <Input
                value={watermarkCustomText}
                onChange={(e) => setWatermarkCustomText(e.target.value)}
                placeholder={tx(
                  "内部机密 · 华东供应链中心",
                  "CONFIDENTIAL · East China Ops"
                )}
                className="h-8 text-xs"
                data-testid="export-watermark-text-input"
              />
            )}
          </div>

          {exportFormat === "PDF" && (
            <div className="grid grid-cols-2 gap-3 rounded-md border border-border p-3">
              <div className="space-y-1">
                <label className="block text-muted-foreground">
                  {tx("纸张尺寸", "Page Size")}
                </label>
                <Select
                  value={pageSize}
                  onChange={(e) =>
                    setPageSize(e.target.value as ExportPageSize)
                  }
                  className="h-8 w-full text-xs font-mono-tech"
                  data-testid="export-pdf-page-size"
                >
                  <option value="A4">A4 (210 × 297 mm)</option>
                  <option value="A3">A3 (297 × 420 mm)</option>
                </Select>
              </div>
              <div className="space-y-1">
                <label className="block text-muted-foreground">
                  {tx("排版方向", "Orientation")}
                </label>
                <Select
                  value={orientation}
                  onChange={(e) =>
                    setOrientation(e.target.value as ExportOrientation)
                  }
                  className="h-8 w-full text-xs"
                  data-testid="export-pdf-orientation"
                >
                  <option value="portrait">{tx("纵向", "Portrait")}</option>
                  <option value="landscape">{tx("横向", "Landscape")}</option>
                </Select>
              </div>
            </div>
          )}

          {exportFormat === "CSV" && (
            <div className="rounded-md border border-border p-3">
              <label className="flex items-center justify-between gap-2 cursor-pointer">
                <div>
                  <div className="font-medium text-foreground">
                    {tx("UTF-8 BOM 头", "UTF-8 BOM Header")}
                  </div>
                  <div className="text-[11px] text-muted-foreground">
                    {tx(
                      "桌面表格软件直接识别多语言字符编码",
                      "Ensures desktop spreadsheets detect UTF-8 encoding"
                    )}
                  </div>
                </div>
                <input
                  type="checkbox"
                  checked={csvBom}
                  onChange={(e) => setCsvBom(e.target.checked)}
                  className="h-4 w-4 accent-[#2563eb] cursor-pointer"
                  data-testid="export-csv-bom-toggle"
                />
              </label>
            </div>
          )}

          {supportsTemplates && (
            <div
              className="rounded-md border border-border p-3 space-y-3"
              data-testid="export-template-section"
            >
              <div className="flex items-center justify-between gap-2">
                <span className="font-medium text-foreground">
                  {tx(
                    `${exportFormat} 母版模板`,
                    `${exportFormat} Master Template`
                  )}
                </span>
                {templatesLoading && (
                  <span className="text-[11px] text-muted-foreground">
                    {tx("加载中…", "Loading…")}
                  </span>
                )}
              </div>

              <div className="flex items-center gap-2">
                <Select
                  value={selectedTemplateId}
                  onChange={(e) => setSelectedTemplateId(e.target.value)}
                  className="h-8 flex-1 text-xs"
                  data-testid="export-template-select"
                >
                  <option value="">
                    {tx("内置默认样式", "Built-in Default Style")}
                  </option>
                  {matchingTemplates.map((tpl) => (
                    <option key={tpl.id} value={tpl.id}>
                      {tpl.name}
                      {tpl.placeholders.length > 0
                        ? ` (${tpl.placeholders.length})`
                        : ""}
                    </option>
                  ))}
                </Select>
                {selectedTemplate && canExport && (
                  <IconButton
                    size="sm"
                    disabled={deletingTemplateId === selectedTemplate.id}
                    onClick={() =>
                      void handleDeleteTemplate(selectedTemplate.id)
                    }
                    aria-label={tx("删除模板", "Delete template")}
                    title={tx("删除模板", "Delete template")}
                    data-testid={`delete-export-template-${selectedTemplate.id}`}
                  >
                    <Trash2 className="h-3.5 w-3.5" />
                  </IconButton>
                )}
              </div>

              {selectedTemplate && selectedTemplate.placeholders.length > 0 && (
                <div
                  className="space-y-2 rounded border border-border bg-muted/20 p-2.5"
                  data-testid="export-placeholders-panel"
                >
                  <div className="text-[11px] font-medium text-muted-foreground">
                    {tx("模板占位符变量", "Template Variables")}
                  </div>
                  <div className="grid grid-cols-1 sm:grid-cols-2 gap-2">
                    {selectedTemplate.placeholders.map((phKey) => (
                      <div key={phKey} className="space-y-1">
                        <label className="block font-mono-tech text-[11px] text-foreground">
                          {`{{${phKey}}}`}
                        </label>
                        <Input
                          value={placeholderValues[phKey] ?? ""}
                          onChange={(e) =>
                            setPlaceholderValues((prev) => ({
                              ...prev,
                              [phKey]: e.target.value,
                            }))
                          }
                          placeholder={phKey}
                          className="h-7 text-xs"
                          data-testid={`export-placeholder-${phKey}`}
                        />
                      </div>
                    ))}
                  </div>
                </div>
              )}

              {canExport && (
                <div className="pt-2 border-t border-border/70 space-y-2">
                  <div className="text-[11px] text-muted-foreground">
                    {tx("上传新母版文件", "Upload New Template")} (
                    {exportFormat === "DOCX" ? ".docx" : ".pptx"} ·{" "}
                    <span className="font-mono-tech">{"{{key}}"}</span>)
                  </div>
                  <div className="flex flex-wrap items-center gap-2">
                    <Input
                      value={uploadName}
                      onChange={(e) => setUploadName(e.target.value)}
                      placeholder={tx("集团周报母版", "Weekly Report Template")}
                      className="h-8 text-xs flex-1 min-w-[130px]"
                      data-testid="export-template-name-input"
                    />
                    <input
                      type="file"
                      accept={exportFormat === "DOCX" ? ".docx" : ".pptx"}
                      onChange={(e) =>
                        setUploadFile(e.target.files?.[0] ?? null)
                      }
                      className="text-[11px] text-muted-foreground file:mr-2 file:rounded file:border file:border-border file:bg-muted file:px-2 file:py-1 file:text-[11px] file:text-foreground cursor-pointer"
                      data-testid="export-template-file-input"
                    />
                    <Button
                      type="button"
                      variant="outline"
                      size="sm"
                      disabled={!uploadFile || uploadingTemplate}
                      onClick={() => void handleUploadTemplate()}
                      className="h-8 px-2.5"
                      data-testid="upload-export-template-btn"
                    >
                      <Upload className="h-3.5 w-3.5 text-xblue" />
                      {uploadingTemplate
                        ? tx("上传中…", "Uploading…")
                        : tx("上传模板", "Upload")}
                    </Button>
                  </div>
                </div>
              )}
            </div>
          )}
        </form>
      )}
    </Modal>
  );
}
