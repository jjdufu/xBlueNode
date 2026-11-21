from __future__ import annotations

from pathlib import Path

from exporters import csv_, docx_, markdown, pdf, pptx_, xlsx
from exporters.document import (
    ExportChart,
    ExportColumnSpec,
    ExportDocument,
    ExportFormatLiteral,
    ExportHeading,
    ExportKpi,
    ExportMarkdown,
    ExportParagraph,
    ExportResult,
    ExportSection,
    ExportTable,
    ExportText,
    SidecarExportRequest,
    SidecarExportResponse,
    cleanup_document_parquet_files,
    resolve_export_target_path,
)
from security import SidecarError

SUPPORTED_EXPORT_FORMATS: frozenset[str] = frozenset(
    {"XLSX", "CSV", "PDF", "DOCX", "PPTX", "MARKDOWN"}
)


def normalize_export_format(raw_format: str) -> ExportFormatLiteral:
    """Validate and normalize path parameter `{export_format}` case-insensitively."""
    upper = raw_format.strip().upper()
    if upper == "XLSX":
        return "XLSX"
    if upper == "CSV":
        return "CSV"
    if upper == "PDF":
        return "PDF"
    if upper == "DOCX":
        return "DOCX"
    if upper == "PPTX":
        return "PPTX"
    if upper == "MARKDOWN":
        return "MARKDOWN"
    raise SidecarError(
        400,
        "INVALID_ARGUMENT",
        f"unsupported export format: {raw_format!r}",
    )


def _resolve_output_extension(fmt: ExportFormatLiteral, doc: ExportDocument) -> str:
    if fmt == "XLSX":
        return "xlsx"
    if fmt == "CSV":
        return "zip" if csv_.will_package_as_zip(doc) else "csv"
    if fmt == "PDF":
        return "pdf"
    if fmt == "DOCX":
        return "docx"
    if fmt == "PPTX":
        return "pptx"
    return "zip" if markdown.has_charts(doc) else "md"


def export_document(
    export_format: str,
    req: SidecarExportRequest,
    *,
    data_root: Path | None = None,
) -> SidecarExportResponse:
    """Dispatch `SidecarExportRequest` to the matching format exporter and clean temp Parquet."""
    fmt = normalize_export_format(export_format)
    doc = req.document
    ext = _resolve_output_extension(fmt, doc)
    out_path = resolve_export_target_path(
        req.tenant_id,
        req.job_id,
        ext,
        data_root=data_root,
    )

    try:
        if fmt == "XLSX":
            res = xlsx.export(doc, out_path, data_root=data_root)
        elif fmt == "CSV":
            res = csv_.export(doc, out_path, data_root=data_root)
        elif fmt == "PDF":
            res = pdf.export(doc, out_path, data_root=data_root)
        elif fmt == "DOCX":
            res = docx_.export(doc, out_path, data_root=data_root)
        elif fmt == "PPTX":
            res = pptx_.export(doc, out_path, data_root=data_root)
        else:
            res = markdown.export(doc, out_path, data_root=data_root)
    finally:
        cleanup_document_parquet_files(doc, data_root=data_root)

    return SidecarExportResponse(
        file_path=str(res.file_path),
        file_size=res.file_size,
        sha256=res.sha256,
        file_name=res.file_name,
    )


__all__ = [
    "ExportChart",
    "ExportColumnSpec",
    "ExportDocument",
    "ExportFormatLiteral",
    "ExportHeading",
    "ExportKpi",
    "ExportMarkdown",
    "ExportParagraph",
    "ExportResult",
    "ExportSection",
    "ExportTable",
    "ExportText",
    "SUPPORTED_EXPORT_FORMATS",
    "SidecarExportRequest",
    "SidecarExportResponse",
    "csv_",
    "docx_",
    "export_document",
    "markdown",
    "normalize_export_format",
    "pdf",
    "pptx_",
    "xlsx",
]
