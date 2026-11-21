import { useCallback, useEffect, useMemo, useState } from "react";
import {
  Download,
  FileSpreadsheet,
  FileText,
  Presentation,
  Trash2,
  Upload,
  X,
} from "lucide-react";
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
import { Badge, Button, Input } from "@/components/ui/primitives";
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
  desc: string;
}[] = [
  { format: "PDF", label: "PDF", desc: "A4/A3 矢量排版报告" },
  { format: "XLSX", label: "XLSX", desc: "多工作表原生工作簿" },
  { format: "CSV", label: "CSV", desc: "UTF-8 明细表文件" },
  { format: "DOCX", label: "DOCX", desc: "Word 报告（支持模板）" },
  { format: "PPTX", label: "PPTX", desc: "演示文稿（原生图表/模板）" },
  { format: "MARKDOWN", label: "MARKDOWN", desc: "GFM 文本与 SVG 归档" },
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

  if (!open || !target) return null;

  const activeFilterEntries = Object.entries(
    sanitizeExportFilters(target.filters)
  );

  const handleUploadTemplate = async () => {
    if (exportFormat !== "DOCX" && exportFormat !== "PPTX") return;
    if (!uploadFile) {
      setErrorMsg("请先选择要上传的模板文件");
      return;
    }
    const resolvedName =
      uploadName.trim() ||
      uploadFile.name.replace(/\.(docx|pptx)$/i, "").trim() ||
      `模板-${Date.now().toString(36).slice(-4)}`;

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
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/45 p-4"
      data-testid="export-dialog-backdrop"
      onClick={onClose}
    >
      <div
        className="w-full max-w-xl max-h-[90vh] rounded-md border border-border bg-background text-foreground shadow-xl flex flex-col overflow-hidden"
        data-testid="export-dialog"
        onClick={(e) => e.stopPropagation()}
      >
        {/* Header */}
        <div className="px-4 py-3 border-b border-border flex items-center justify-between gap-2 shrink-0">
          <div className="flex items-center gap-2 min-w-0">
            <Download className="h-4 w-4 text-xblue shrink-0" />
            <h3 className="text-sm font-semibold truncate">
              {target.kind === "WIDGET"
                ? `导出组件 · ${target.widgetTitle || "可视化组件"}`
                : `出品导出 · ${target.dashboardName}`}
            </h3>
            <Badge variant="xblue">{target.kind}</Badge>
          </div>
          <button
            type="button"
            onClick={onClose}
            className="rounded p-1 text-muted-foreground hover:text-foreground hover:bg-muted cursor-pointer"
            aria-label="关闭导出窗口"
          >
            <X className="h-4 w-4" />
          </button>
        </div>

        {/* Body */}
        <form
          onSubmit={(e) => void handleSubmit(e)}
          className="flex-1 overflow-y-auto p-4 space-y-4 text-xs"
        >
          {errorMsg && (
            <div
              className="rounded border border-foreground/30 bg-muted px-3 py-2 text-xs"
              data-testid="export-dialog-error"
            >
              {errorMsg}
            </div>
          )}

          {/* Active Context Summary */}
          <div className="rounded-md border border-border bg-muted/25 px-3 py-2 flex flex-wrap items-center justify-between gap-2">
            <span className="text-muted-foreground">
              数据快照将按当前创建者行列策略执行服务端渲染
            </span>
            <div className="flex items-center gap-1.5 font-mono-tech text-[11px] tabular-nums">
              <Badge variant="outline">
                筛选条件 {activeFilterEntries.length}
              </Badge>
              {target.kind === "WIDGET" &&
                (target.drillPath?.length ?? 0) > 0 && (
                  <Badge variant="outline">
                    下钻层级 {target.drillPath?.length ?? 0}
                  </Badge>
                )}
            </div>
          </div>

          {/* 6 Format Selector */}
          <div className="space-y-1.5">
            <label className="block font-medium text-foreground">
              选择出品格式
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
                      {item.desc}
                    </div>
                  </button>
                );
              })}
            </div>
          </div>

          {/* Title & Subtitle */}
          <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
            <div className="space-y-1">
              <label className="block text-muted-foreground">报告主标题</label>
              <Input
                value={title}
                onChange={(e) => setTitle(e.target.value)}
                placeholder="输入报告标题"
                className="h-8 text-xs"
                data-testid="export-title-input"
              />
            </div>
            <div className="space-y-1">
              <label className="block text-muted-foreground">
                副标题（可选）
              </label>
              <Input
                value={subtitle}
                onChange={(e) => setSubtitle(e.target.value)}
                placeholder="例如：2026 Q1 经营复盘"
                className="h-8 text-xs"
                data-testid="export-subtitle-input"
              />
            </div>
          </div>

          {/* Watermark */}
          <div className="rounded-md border border-border p-3 space-y-2.5">
            <label className="flex items-center justify-between gap-2 cursor-pointer">
              <div>
                <div className="font-medium text-foreground">安全溯源水印</div>
                <div className="text-[11px] text-muted-foreground">
                  开启后默认写入“当前账号 + 生成时间”，也可指定自定义文本
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
                placeholder="留空自动生成用户名与时间戳，或输入自定义水印文本"
                className="h-8 text-xs"
                data-testid="export-watermark-text-input"
              />
            )}
          </div>

          {/* PDF Specific Options */}
          {exportFormat === "PDF" && (
            <div className="grid grid-cols-2 gap-3 rounded-md border border-border p-3">
              <div className="space-y-1">
                <label className="block text-muted-foreground">PDF 纸张尺寸</label>
                <select
                  value={pageSize}
                  onChange={(e) =>
                    setPageSize(e.target.value as ExportPageSize)
                  }
                  className="h-8 w-full rounded-md border border-input bg-background px-2 text-xs font-mono-tech"
                  data-testid="export-pdf-page-size"
                >
                  <option value="A4">A4 (210 × 297 mm)</option>
                  <option value="A3">A3 (297 × 420 mm)</option>
                </select>
              </div>
              <div className="space-y-1">
                <label className="block text-muted-foreground">排版方向</label>
                <select
                  value={orientation}
                  onChange={(e) =>
                    setOrientation(e.target.value as ExportOrientation)
                  }
                  className="h-8 w-full rounded-md border border-input bg-background px-2 text-xs"
                  data-testid="export-pdf-orientation"
                >
                  <option value="portrait">纵向 (Portrait)</option>
                  <option value="landscape">横向 (Landscape)</option>
                </select>
              </div>
            </div>
          )}

          {/* CSV Specific Options */}
          {exportFormat === "CSV" && (
            <div className="rounded-md border border-border p-3">
              <label className="flex items-center justify-between gap-2 cursor-pointer">
                <div>
                  <div className="font-medium text-foreground">
                    写入 UTF-8 BOM 头
                  </div>
                  <div className="text-[11px] text-muted-foreground">
                    开启后桌面电子表格软件可直接正确识别中文字符编码
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

          {/* DOCX / PPTX Template Selection, Upload & Placeholders */}
          {supportsTemplates && (
            <div
              className="rounded-md border border-border p-3 space-y-3"
              data-testid="export-template-section"
            >
              <div className="flex items-center justify-between gap-2">
                <span className="font-medium text-foreground">
                  {exportFormat} 报告母版模板
                </span>
                {templatesLoading && (
                  <span className="text-[11px] text-muted-foreground">
                    加载模板中...
                  </span>
                )}
              </div>

              <div className="flex items-center gap-2">
                <select
                  value={selectedTemplateId}
                  onChange={(e) => setSelectedTemplateId(e.target.value)}
                  className="h-8 flex-1 rounded-md border border-input bg-background px-2 text-xs"
                  data-testid="export-template-select"
                >
                  <option value="">默认内置样式（不使用母版）</option>
                  {matchingTemplates.map((tpl) => (
                    <option key={tpl.id} value={tpl.id}>
                      {tpl.name}
                      {tpl.placeholders.length > 0
                        ? ` (${tpl.placeholders.length} 个占位符)`
                        : ""}
                    </option>
                  ))}
                </select>
                {selectedTemplate && (
                  <Button
                    type="button"
                    variant="outline"
                    disabled={deletingTemplateId === selectedTemplate.id}
                    onClick={() =>
                      void handleDeleteTemplate(selectedTemplate.id)
                    }
                    className="h-8 px-2.5 text-xs"
                    title="删除当前选中模板"
                    data-testid={`delete-export-template-${selectedTemplate.id}`}
                  >
                    <Trash2 className="h-3.5 w-3.5" />
                  </Button>
                )}
              </div>

              {/* Discovered {{key}} Placeholders */}
              {selectedTemplate && selectedTemplate.placeholders.length > 0 && (
                <div
                  className="space-y-2 rounded border border-border bg-muted/20 p-2.5"
                  data-testid="export-placeholders-panel"
                >
                  <div className="text-[11px] font-medium text-muted-foreground">
                    填写模板占位符变量
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
                          placeholder={`输入 ${phKey} 的替换内容`}
                          className="h-7 text-xs"
                          data-testid={`export-placeholder-${phKey}`}
                        />
                      </div>
                    ))}
                  </div>
                </div>
              )}

              {/* Upload New Template */}
              <div className="pt-2 border-t border-border/70 space-y-2">
                <div className="text-[11px] text-muted-foreground">
                  上传新 {exportFormat === "DOCX" ? ".docx" : ".pptx"} 母版（支持{" "}
                  <span className="font-mono-tech">{"{{key}}"}</span> 占位符自动识别）
                </div>
                <div className="flex flex-wrap items-center gap-2">
                  <Input
                    value={uploadName}
                    onChange={(e) => setUploadName(e.target.value)}
                    placeholder="模板名称（可选）"
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
                    disabled={!uploadFile || uploadingTemplate}
                    onClick={() => void handleUploadTemplate()}
                    className="h-8 px-2.5 text-xs"
                    data-testid="upload-export-template-btn"
                  >
                    <Upload className="h-3.5 w-3.5 text-xblue" />
                    {uploadingTemplate ? "上传中..." : "上传模板"}
                  </Button>
                </div>
              </div>
            </div>
          )}

          {/* Footer */}
          <div className="pt-2 border-t border-border flex items-center justify-end gap-2">
            <Button type="button" variant="outline" onClick={onClose}>
              取消
            </Button>
            <Button
              type="submit"
              disabled={submitting}
              data-testid="submit-export-btn"
            >
              <Download className="h-3.5 w-3.5" />
              {submitting ? "提交出品任务中..." : `开始导出 ${exportFormat}`}
            </Button>
          </div>
        </form>
      </div>
    </div>
  );
}
