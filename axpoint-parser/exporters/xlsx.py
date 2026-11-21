from __future__ import annotations

import io
import re
from decimal import Decimal
from pathlib import Path
from typing import Any

import openpyxl
from openpyxl.cell import WriteOnlyCell
from openpyxl.drawing.image import Image as OpenpyxlImage
from openpyxl.styles import Alignment, Font, PatternFill
from openpyxl.utils import get_column_letter
from openpyxl.worksheet._write_only import WriteOnlyWorksheet

from charts import render_chart_bytes
from exporters.document import (
    ExportChart,
    ExportColumnSpec,
    ExportDocument,
    ExportKpi,
    ExportResult,
    ExportTable,
    ExportText,
    RenderChartRequest,
    atomic_write_export,
    build_download_filename,
    coerce_excel_cell_value,
    format_timestamp_tz,
    iter_table_rows,
    parse_kpi_numeric,
)

# Maximum data rows per worksheet (Excel cap is 1,048,576 rows including 1 header row).
XLSX_MAX_DATA_ROWS = 1_048_575

_SHEET_INVALID_CHARS_RE = re.compile(r"[\[\]:*?/\\\x00-\x1f]+")

_TITLE_FONT = Font(name="Noto Sans CJK SC", size=16, bold=True, color="0F172A")
_SUBTITLE_FONT = Font(name="Noto Sans CJK SC", size=11, italic=False, color="475569")
_SECTION_FONT = Font(name="Noto Sans CJK SC", size=12, bold=True, color="1E293B")
_HEADER_FONT = Font(name="Noto Sans CJK SC", size=10, bold=True, color="FFFFFF")
_HEADER_FILL = PatternFill(fill_type="solid", start_color="1E3A8A", end_color="1E3A8A")
_SUBTOTAL_FONT = Font(name="Noto Sans CJK SC", size=10, bold=True, color="0F172A")
_SUBTOTAL_FILL = PatternFill(fill_type="solid", start_color="F1F5F9", end_color="F1F5F9")
_TOTAL_FONT = Font(name="Noto Sans CJK SC", size=10, bold=True, color="0F172A")
_TOTAL_FILL = PatternFill(fill_type="solid", start_color="E2E8F0", end_color="E2E8F0")
_BODY_FONT = Font(name="Noto Sans CJK SC", size=10, color="0F172A")
_META_FONT = Font(name="Noto Sans CJK SC", size=9, color="64748B")


def sanitize_sheet_title(raw_title: str, used_lower: set[str], fallback: str = "数据表") -> str:
    """Sanitize Excel worksheet title (`<= 31` chars, no `[]:*?/\\`, case-insensitively unique)."""
    cleaned = _SHEET_INVALID_CHARS_RE.sub("_", raw_title.strip()).strip(" '")
    if not cleaned:
        cleaned = fallback

    candidate = cleaned[:31]
    if candidate.lower() not in used_lower:
        used_lower.add(candidate.lower())
        return candidate

    idx = 2
    while True:
        suffix = f" ({idx})"
        base = cleaned[: max(1, 31 - len(suffix))]
        deduped = f"{base}{suffix}"
        if deduped.lower() not in used_lower:
            used_lower.add(deduped.lower())
            return deduped
        idx += 1


def _default_number_format(col: ExportColumnSpec) -> str | None:
    if col.format:
        return col.format
    if col.data_type == "DECIMAL":
        return "#,##0.00"
    if col.data_type == "FLOAT64":
        return "#,##0.00"
    if col.data_type == "INT64":
        return "#,##0"
    if col.data_type == "DATE":
        return "yyyy-mm-dd"
    if col.data_type == "TIMESTAMP":
        return "yyyy-mm-dd hh:mm:ss"
    return None


def _make_cell(
    ws: WriteOnlyWorksheet,
    value: Any,
    *,
    font: Font | None = None,
    fill: PatternFill | None = None,
    number_format: str | None = None,
    alignment: Alignment | None = None,
) -> WriteOnlyCell:
    cell = WriteOnlyCell(ws, value=value)
    if font is not None:
        cell.font = font
    if fill is not None:
        cell.fill = fill
    if number_format is not None:
        cell.number_format = number_format
    if alignment is not None:
        cell.alignment = alignment
    return cell


def _init_table_sheet(
    wb: openpyxl.Workbook,
    sheet_title: str,
    columns: list[ExportColumnSpec],
) -> WriteOnlyWorksheet:
    ws: WriteOnlyWorksheet = wb.create_sheet(title=sheet_title)
    ws.freeze_panes = "A2"
    for col_idx, col in enumerate(columns, start=1):
        col_letter = get_column_letter(col_idx)
        ws.column_dimensions[col_letter].width = max(14, min(36, len(col.name) * 2 + 6))
    header_cells = [
        _make_cell(
            ws,
            col.name,
            font=_HEADER_FONT,
            fill=_HEADER_FILL,
            alignment=Alignment(horizontal="center", vertical="center"),
        )
        for col in columns
    ]
    if header_cells:
        ws.append(header_cells)
    return ws


def _write_overview_sheet(
    wb: openpyxl.Workbook,
    doc: ExportDocument,
    used_lower: set[str],
    image_streams: list[io.BytesIO],
) -> None:
    title = sanitize_sheet_title("概览", used_lower, fallback="概览")
    ws: WriteOnlyWorksheet = wb.create_sheet(title=title)
    ws.column_dimensions["A"].width = 28
    ws.column_dimensions["B"].width = 24
    ws.column_dimensions["C"].width = 24
    ws.column_dimensions["D"].width = 28

    cur_row = 1
    ws.append([_make_cell(ws, doc.title, font=_TITLE_FONT)])
    cur_row += 1

    if doc.subtitle:
        ws.append([_make_cell(ws, doc.subtitle, font=_SUBTITLE_FONT)])
        cur_row += 1

    ts_str = format_timestamp_tz(doc.generated_at, doc.timezone)
    meta_cells = [
        _make_cell(ws, "生成人", font=_META_FONT),
        _make_cell(ws, doc.generated_by, font=_META_FONT),
        _make_cell(ws, "生成时间", font=_META_FONT),
        _make_cell(ws, f"{ts_str} ({doc.timezone})", font=_META_FONT),
    ]
    ws.append(meta_cells)
    cur_row += 1

    if doc.watermark:
        ws.append(
            [
                _make_cell(ws, "水印", font=_META_FONT),
                _make_cell(ws, doc.watermark, font=_META_FONT),
            ]
        )
        cur_row += 1

    ws.append([])
    cur_row += 1

    kpis = [sec for sec in doc.sections if isinstance(sec, ExportKpi)]
    if kpis:
        ws.append([_make_cell(ws, "核心指标", font=_SECTION_FONT)])
        cur_row += 1
        ws.append(
            [
                _make_cell(ws, "指标名称", font=_HEADER_FONT, fill=_HEADER_FILL),
                _make_cell(ws, "指标值", font=_HEADER_FONT, fill=_HEADER_FILL),
                _make_cell(ws, "对比 / 说明", font=_HEADER_FONT, fill=_HEADER_FILL),
            ]
        )
        cur_row += 1
        for kpi in kpis:
            kpi_dec = parse_kpi_numeric(kpi.value)
            if kpi_dec is not None:
                has_fraction = "." in kpi.value
                num_fmt = "#,##0.00" if has_fraction else "#,##0"
                val_cell = _make_cell(ws, kpi_dec, font=_BODY_FONT, number_format=num_fmt)
            else:
                val_cell = _make_cell(ws, kpi.value, font=_BODY_FONT)
            ws.append(
                [
                    _make_cell(ws, kpi.label, font=_BODY_FONT),
                    val_cell,
                    _make_cell(ws, kpi.compare or "", font=_BODY_FONT),
                ]
            )
            cur_row += 1
        ws.append([])
        cur_row += 1

    text_sections = [sec for sec in doc.sections if isinstance(sec, ExportText)]
    for txt_sec in text_sections:
        font = _SECTION_FONT if txt_sec.kind == "HEADING" else _BODY_FONT
        ws.append([_make_cell(ws, txt_sec.text, font=font)])
        cur_row += 1
    if text_sections:
        ws.append([])
        cur_row += 1

    charts = [sec for sec in doc.sections if isinstance(sec, ExportChart)]
    for idx, chart in enumerate(charts, start=1):
        chart_label = chart.title or f"图表 {idx}"
        ws.append([_make_cell(ws, chart_label, font=_SECTION_FONT)])
        cur_row += 1

        png_bytes, _ = render_chart_bytes(
            RenderChartRequest(
                option=chart.echarts_option,
                width=chart.width_px,
                height=chart.height_px,
                format="png",
            )
        )
        bio = io.BytesIO(png_bytes)
        image_streams.append(bio)
        xl_img = OpenpyxlImage(bio)
        display_w = 640
        display_h = max(200, int(display_w * chart.height_px / max(chart.width_px, 1)))
        xl_img.width = display_w
        xl_img.height = display_h
        ws.add_image(xl_img, f"A{cur_row}")

        spacer_rows = max(14, (display_h // 20) + 2)
        for _ in range(spacer_rows):
            ws.append([])
            cur_row += 1


def _write_table_sheets(
    wb: openpyxl.Workbook,
    table: ExportTable,
    *,
    fallback_title: str,
    tz_name: str,
    used_lower: set[str],
    data_root: Path | None = None,
) -> None:
    columns = table.columns
    col_formats = [_default_number_format(c) for c in columns]
    num_cols = len(columns)

    base_raw = table.title.strip() or fallback_title
    first_title = sanitize_sheet_title(base_raw, used_lower, fallback=fallback_title)
    ws = _init_table_sheet(wb, first_title, columns)

    sheet_data_rows = 0
    global_row_idx = 0
    row_kinds = table.row_kinds
    has_pivot_kinds = bool(row_kinds)

    # Track contiguous group label spans on column 1 for PIVOT detail rows to merge safely.
    group_val: str | None = None
    group_start_sheet_row: int | None = None

    def _flush_merge(end_sheet_row: int) -> None:
        nonlocal group_val, group_start_sheet_row
        if (
            group_val is not None
            and group_start_sheet_row is not None
            and end_sheet_row > group_start_sheet_row
        ):
            ws.merged_cells.ranges.add(f"A{group_start_sheet_row}:A{end_sheet_row}")
        group_val = None
        group_start_sheet_row = None

    for raw_row in iter_table_rows(table, data_root=data_root):
        if sheet_data_rows >= XLSX_MAX_DATA_ROWS:
            _flush_merge(sheet_data_rows + 1)
            next_title = sanitize_sheet_title(base_raw, used_lower, fallback=fallback_title)
            ws = _init_table_sheet(wb, next_title, columns)
            sheet_data_rows = 0

        kind = (
            row_kinds[global_row_idx].upper()
            if global_row_idx < len(row_kinds)
            else "DETAIL"
        )
        row_font: Font | None = None
        row_fill: PatternFill | None = None
        if kind == "SUBTOTAL":
            row_font = _SUBTOTAL_FONT
            row_fill = _SUBTOTAL_FILL
        elif kind == "TOTAL":
            row_font = _TOTAL_FONT
            row_fill = _TOTAL_FILL

        excel_row_num = sheet_data_rows + 2  # Row 1 is header
        cells: list[Any] = []
        for col_idx in range(num_cols):
            raw_val = raw_row[col_idx] if col_idx < len(raw_row) else None
            coerced = coerce_excel_cell_value(raw_val, columns[col_idx], tz_name=tz_name)
            fmt = col_formats[col_idx]
            if fmt is None and isinstance(coerced, Decimal):
                fmt = "#,##0.00"
            if fmt is not None or row_font is not None or row_fill is not None:
                cells.append(
                    _make_cell(
                        ws,
                        coerced,
                        font=row_font,
                        fill=row_fill,
                        number_format=fmt,
                    )
                )
            else:
                cells.append(coerced)

        ws.append(cells)

        if has_pivot_kinds and num_cols >= 2 and columns[0].data_type == "STRING":
            first_str = (
                str(raw_row[0]) if len(raw_row) > 0 and raw_row[0] is not None else ""
            )
            if kind == "DETAIL" and first_str:
                if first_str == group_val:
                    pass
                else:
                    _flush_merge(excel_row_num - 1)
                    group_val = first_str
                    group_start_sheet_row = excel_row_num
            else:
                _flush_merge(excel_row_num - 1)

        sheet_data_rows += 1
        global_row_idx += 1

    _flush_merge(sheet_data_rows + 1)

    if table.totals is not None:
        if sheet_data_rows >= XLSX_MAX_DATA_ROWS:
            next_title = sanitize_sheet_title(base_raw, used_lower, fallback=fallback_title)
            ws = _init_table_sheet(wb, next_title, columns)
            sheet_data_rows = 0

        total_cells: list[Any] = []
        for col_idx in range(num_cols):
            raw_val = table.totals[col_idx] if col_idx < len(table.totals) else None
            coerced = coerce_excel_cell_value(raw_val, columns[col_idx], tz_name=tz_name)
            fmt = col_formats[col_idx]
            if fmt is None and isinstance(coerced, Decimal):
                fmt = "#,##0.00"
            total_cells.append(
                _make_cell(
                    ws,
                    coerced,
                    font=_TOTAL_FONT,
                    fill=_TOTAL_FILL,
                    number_format=fmt,
                )
            )
        ws.append(total_cells)


def export(
    doc: ExportDocument,
    out_path: Path,
    *,
    data_root: Path | None = None,
) -> ExportResult:
    """Export `ExportDocument` to an `.xlsx` workbook via openpyxl `write_only=True`."""

    def _write_xlsx(part_path: Path) -> None:
        wb = openpyxl.Workbook(write_only=True)
        used_lower: set[str] = set()
        image_streams: list[io.BytesIO] = []
        try:
            _write_overview_sheet(wb, doc, used_lower, image_streams)

            explicit_titles: set[str] = set()
            table_idx = 0
            for section in doc.sections:
                if isinstance(section, ExportTable):
                    table_idx += 1
                    if section.title.strip():
                        explicit_titles.add(section.title.strip())
                    _write_table_sheets(
                        wb,
                        section,
                        fallback_title=f"数据表_{table_idx}",
                        tz_name=doc.timezone,
                        used_lower=used_lower,
                        data_root=data_root,
                    )

            chart_idx = 0
            for section in doc.sections:
                if isinstance(section, ExportChart) and section.data_table is not None:
                    chart_idx += 1
                    dt = section.data_table
                    if not dt.columns:
                        continue
                    candidate_title = dt.title.strip() or section.title.strip()
                    if candidate_title and candidate_title in explicit_titles:
                        continue
                    backing_title = (
                        f"{candidate_title[:26]}_数据"
                        if candidate_title
                        else f"图表数据_{chart_idx}"
                    )
                    backing_table = dt.model_copy(update={"title": backing_title})
                    _write_table_sheets(
                        wb,
                        backing_table,
                        fallback_title=f"图表数据_{chart_idx}",
                        tz_name=doc.timezone,
                        used_lower=used_lower,
                        data_root=data_root,
                    )

            wb.save(str(part_path))
        finally:
            wb.close()
            for bio in image_streams:
                bio.close()

    file_size, sha256_hex = atomic_write_export(out_path, _write_xlsx)
    file_name = build_download_filename(doc.title, "xlsx")
    return ExportResult(
        file_path=out_path,
        file_size=file_size,
        sha256=sha256_hex,
        file_name=file_name,
    )
