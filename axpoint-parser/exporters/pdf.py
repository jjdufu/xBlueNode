from __future__ import annotations

import re
from dataclasses import dataclass
from pathlib import Path

import weasyprint
from jinja2 import Environment, FileSystemLoader, select_autoescape
from markupsafe import Markup

from charts import render_chart_bytes
from exporters.document import (
    ExportChart,
    ExportDocument,
    ExportKpi,
    ExportResult,
    ExportTable,
    ExportText,
    RenderChartRequest,
    atomic_write_export,
    build_download_filename,
    format_display_cell,
    format_timestamp_tz,
    iter_table_rows,
)
from security import SidecarError

PDF_MAX_TABLE_ROWS = 5_000

TEMPLATES_DIR = Path(__file__).resolve().parent / "templates"
_XML_DECL_RE = re.compile(r"^\s*<\?xml[^>]*\?>\s*", re.IGNORECASE)

_JINJA_ENV = Environment(
    loader=FileSystemLoader(str(TEMPLATES_DIR)),
    autoescape=select_autoescape(["html", "j2", "xml"]),
)


@dataclass(frozen=True, slots=True)
class _PdfTableRow:
    cells: list[str]
    css_class: str


@dataclass(frozen=True, slots=True)
class _PdfRenderBlock:
    kind: str
    title: str = ""
    text: str = ""
    heading_class_level: int = 1
    svg_markup: Markup = Markup("")
    headers: list[str] | None = None
    rows: list[_PdfTableRow] | None = None
    truncated: bool = False
    total_rows: int = 0
    max_rows: int = PDF_MAX_TABLE_ROWS


def _escape_css_string(raw: str) -> str:
    return (
        raw.replace("\\", "\\\\")
        .replace('"', '\\"')
        .replace("\r", " ")
        .replace("\n", " ")
    )


def _prepare_inline_svg(svg_bytes: bytes) -> Markup:
    svg_str = svg_bytes.decode("utf-8", errors="replace")
    svg_body = _XML_DECL_RE.sub("", svg_str).strip()
    return Markup(svg_body)


def _build_table_block(
    table: ExportTable,
    *,
    tz_name: str,
    data_root: Path | None = None,
) -> _PdfRenderBlock:
    columns = table.columns
    num_cols = len(columns)
    headers = [col.name for col in columns]
    rows: list[_PdfTableRow] = []
    total_rows = 0
    row_kinds = table.row_kinds

    for raw_row in iter_table_rows(table, data_root=data_root):
        if total_rows < PDF_MAX_TABLE_ROWS:
            kind = (
                row_kinds[total_rows].upper()
                if total_rows < len(row_kinds)
                else "DETAIL"
            )
            css_class = ""
            if kind == "SUBTOTAL":
                css_class = "row-subtotal"
            elif kind == "TOTAL":
                css_class = "row-total"

            formatted = [
                format_display_cell(
                    raw_row[c_idx] if c_idx < len(raw_row) else None,
                    columns[c_idx],
                    tz_name=tz_name,
                )
                for c_idx in range(num_cols)
            ]
            rows.append(_PdfTableRow(cells=formatted, css_class=css_class))
        total_rows += 1

    if table.totals is not None and total_rows <= PDF_MAX_TABLE_ROWS:
        totals_formatted = [
            format_display_cell(
                table.totals[c_idx] if c_idx < len(table.totals) else None,
                columns[c_idx],
                tz_name=tz_name,
            )
            for c_idx in range(num_cols)
        ]
        rows.append(_PdfTableRow(cells=totals_formatted, css_class="row-total"))

    return _PdfRenderBlock(
        kind="TABLE",
        title=table.title,
        headers=headers,
        rows=rows,
        truncated=total_rows > PDF_MAX_TABLE_ROWS,
        total_rows=total_rows,
        max_rows=PDF_MAX_TABLE_ROWS,
    )


def render_report_html(
    doc: ExportDocument,
    *,
    data_root: Path | None = None,
) -> str:
    """Render `ExportDocument` into standalone HTML string ready for WeasyPrint."""
    if doc.page_size not in {"A4", "A3"}:
        raise SidecarError(400, "INVALID_ARGUMENT", f"unsupported page_size: {doc.page_size!r}")
    if doc.orientation not in {"portrait", "landscape"}:
        raise SidecarError(
            400, "INVALID_ARGUMENT", f"unsupported orientation: {doc.orientation!r}"
        )

    base_css = (TEMPLATES_DIR / "report.css").read_text(encoding="utf-8")
    kpis = [sec for sec in doc.sections if isinstance(sec, ExportKpi)]
    blocks: list[_PdfRenderBlock] = []

    for section in doc.sections:
        if isinstance(section, ExportKpi):
            continue
        if isinstance(section, ExportText):
            blocks.append(
                _PdfRenderBlock(
                    kind=section.kind,
                    text=section.text,
                    heading_class_level=min(max(section.level, 1), 3),
                )
            )
        elif isinstance(section, ExportChart):
            svg_bytes, _ = render_chart_bytes(
                RenderChartRequest(
                    option=section.echarts_option,
                    width=section.width_px,
                    height=section.height_px,
                    format="svg",
                )
            )
            blocks.append(
                _PdfRenderBlock(
                    kind="CHART",
                    title=section.title,
                    svg_markup=_prepare_inline_svg(svg_bytes),
                )
            )
        elif isinstance(section, ExportTable):
            blocks.append(
                _build_table_block(section, tz_name=doc.timezone, data_root=data_root)
            )

    template = _JINJA_ENV.get_template("report.html.j2")
    ts_str = format_timestamp_tz(doc.generated_at, doc.timezone)
    return template.render(
        title=doc.title,
        subtitle=doc.subtitle,
        generated_by=doc.generated_by,
        generated_at=ts_str,
        timezone=doc.timezone,
        watermark=doc.watermark,
        page_size=doc.page_size,
        orientation=doc.orientation,
        page_header_css=_escape_css_string(doc.title),
        watermark_css=_escape_css_string(doc.watermark or ""),
        base_css=base_css,
        kpis=kpis,
        blocks=blocks,
    )


def export(
    doc: ExportDocument,
    out_path: Path,
    *,
    data_root: Path | None = None,
) -> ExportResult:
    """Export `ExportDocument` to a `.pdf` file via Jinja2 + WeasyPrint."""
    html_str = render_report_html(doc, data_root=data_root)

    def _write_pdf(part_path: Path) -> None:
        weasyprint.HTML(string=html_str).write_pdf(str(part_path))

    file_size, sha256_hex = atomic_write_export(out_path, _write_pdf)
    return ExportResult(
        file_path=out_path,
        file_size=file_size,
        sha256=sha256_hex,
        file_name=build_download_filename(doc.title, "pdf"),
    )
