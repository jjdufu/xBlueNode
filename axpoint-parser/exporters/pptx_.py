from __future__ import annotations

import io
import math
import re
from pathlib import Path
from typing import Any

from pptx import Presentation
from pptx.chart.data import CategoryChartData
from pptx.dml.color import RGBColor
from pptx.enum.chart import XL_CHART_TYPE
from pptx.presentation import Presentation as PresentationType
from pptx.slide import Slide, SlideLayout
from pptx.util import Inches, Pt

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
    build_placeholder_map,
    format_display_cell,
    format_timestamp_tz,
    iter_table_rows,
    parse_decimal_safe,
    resolve_template_file,
)

PPTX_ROWS_PER_SLIDE = 15
PPTX_MAX_TABLE_ROWS = 10_050

_PLACEHOLDER_RE = re.compile(r"\{\{\s*([A-Za-z0-9_.-]+)\s*\}\}")
_XML_CTRL_CHARS_RE = re.compile(r"[\x00-\x08\x0b\x0c\x0e-\x1f]")

_NATIVE_CHART_MAP: dict[str, XL_CHART_TYPE] = {
    "BAR_CHART": XL_CHART_TYPE.COLUMN_CLUSTERED,
    "LINE_CHART": XL_CHART_TYPE.LINE,
    "PIE_CHART": XL_CHART_TYPE.PIE,
}


def _clean_text(value: str) -> str:
    return _XML_CTRL_CHARS_RE.sub("", value)


def _rgb(hex_code: str) -> RGBColor:
    rgb_cls: Any = RGBColor
    color: RGBColor = rgb_cls.from_string(hex_code)
    return color


def _find_layout(
    prs: PresentationType,
    preferred_names: tuple[str, ...],
    fallback_idx: int,
) -> SlideLayout:
    lowered_targets = {n.lower() for n in preferred_names}
    for layout in prs.slide_layouts:
        if layout.name.strip().lower() in lowered_targets:
            return layout
    idx = min(max(fallback_idx, 0), len(prs.slide_layouts) - 1)
    return prs.slide_layouts[idx]


def _replace_in_text_frame(text_frame: Any, mapping: dict[str, str]) -> None:
    for para in text_frame.paragraphs:
        raw = para.text
        if "{{" not in raw:
            continue
        runs = para.runs
        if not runs:
            para.text = _PLACEHOLDER_RE.sub(
                lambda m: _clean_text(mapping.get(m.group(1), "")), raw
            )
            continue
        coalesced = "".join(r.text for r in runs)
        replaced = _PLACEHOLDER_RE.sub(
            lambda m: _clean_text(mapping.get(m.group(1), "")), coalesced
        )
        if replaced != coalesced:
            runs[0].text = replaced
            for extra in runs[1:]:
                extra.text = ""


def _replace_in_shape(shape: Any, mapping: dict[str, str]) -> None:
    if getattr(shape, "has_text_frame", False):
        _replace_in_text_frame(shape.text_frame, mapping)
    if getattr(shape, "has_table", False):
        for row in shape.table.rows:
            for cell in row.cells:
                _replace_in_text_frame(cell.text_frame, mapping)
    if hasattr(shape, "shapes"):
        for child in shape.shapes:
            _replace_in_shape(child, mapping)


def _replace_template_placeholders(prs: PresentationType, mapping: dict[str, str]) -> None:
    for slide in prs.slides:
        for shape in slide.shapes:
            _replace_in_shape(shape, mapping)


def _extract_slides_text(prs: PresentationType) -> str:
    chunks: list[str] = []
    for slide in prs.slides:
        for shape in slide.shapes:
            if getattr(shape, "has_text_frame", False):
                chunks.append(shape.text_frame.text)
            if getattr(shape, "has_table", False):
                for row in shape.table.rows:
                    for cell in row.cells:
                        chunks.append(cell.text)
    return "\n".join(chunks)


def _slide_size_inches(prs: PresentationType) -> tuple[float, float]:
    one_inch = float(int(Inches(1.0)))
    raw_w = prs.slide_width
    raw_h = prs.slide_height
    w_in = float(int(raw_w)) / one_inch if raw_w is not None else 13.333
    h_in = float(int(raw_h)) / one_inch if raw_h is not None else 7.5
    return w_in, h_in


def _add_clean_content_slide(prs: PresentationType, title_text: str) -> Slide:
    layout = _find_layout(
        prs,
        ("标题和内容", "仅标题", "Title Only", "Title and Content", "Blank", "空白"),
        fallback_idx=1,
    )
    slide: Slide = prs.slides.add_slide(layout)
    title_shape = slide.shapes.title

    # Remove non-title empty placeholder boxes so custom tables/charts never overlap them.
    for ph in list(slide.placeholders):
        if title_shape is not None and ph == title_shape:
            continue
        sp = ph._element
        parent = sp.getparent()
        if parent is not None:
            parent.remove(sp)

    cleaned_title = _clean_text(title_text)
    if title_shape is not None:
        title_shape.text = cleaned_title
    else:
        tb = slide.shapes.add_textbox(Inches(0.6), Inches(0.35), Inches(11.5), Inches(0.7))
        p = tb.text_frame.paragraphs[0]
        run = p.add_run()
        run.text = cleaned_title
        run.font.size = Pt(22)
        run.font.bold = True
        run.font.color.rgb = _rgb("0F172A")
    return slide


def _add_cover_slide(prs: PresentationType, doc: ExportDocument) -> None:
    layout = _find_layout(prs, ("标题", "Title Slide", "Title"), fallback_idx=0)
    slide: Slide = prs.slides.add_slide(layout)
    ts_str = format_timestamp_tz(doc.generated_at, doc.timezone)
    meta_line = f"{doc.generated_by} | {ts_str} ({doc.timezone})"
    sub_text = f"{doc.subtitle}\n{meta_line}".strip() if doc.subtitle else meta_line

    if slide.shapes.title is not None:
        slide.shapes.title.text = _clean_text(doc.title)
    else:
        tb = slide.shapes.add_textbox(Inches(0.8), Inches(2.2), Inches(11.0), Inches(1.2))
        r = tb.text_frame.paragraphs[0].add_run()
        r.text = _clean_text(doc.title)
        r.font.size = Pt(32)
        r.font.bold = True

    subtitle_placed = False
    for ph in slide.placeholders:
        if slide.shapes.title is not None and ph == slide.shapes.title:
            continue
        if ph.has_text_frame:
            ph.text_frame.text = _clean_text(sub_text)
            subtitle_placed = True
            break

    if not subtitle_placed:
        sub_box = slide.shapes.add_textbox(Inches(0.8), Inches(3.7), Inches(11.0), Inches(1.2))
        r_sub = sub_box.text_frame.paragraphs[0].add_run()
        r_sub.text = _clean_text(sub_text)
        r_sub.font.size = Pt(16)
        r_sub.font.color.rgb = _rgb("475569")


def _add_kpi_slides(prs: PresentationType, kpis: list[ExportKpi]) -> None:
    slide_w, _ = _slide_size_inches(prs)
    usable_w = max(8.0, slide_w - 1.2)

    for start in range(0, len(kpis), 6):
        chunk = kpis[start : start + 6]
        slide = _add_clean_content_slide(prs, "核心指标概览")
        cols_per_row = min(3, len(chunk))
        card_w = (usable_w - 0.3 * (cols_per_row - 1)) / cols_per_row
        card_h = 1.8

        for idx, kpi in enumerate(chunk):
            r_idx = idx // cols_per_row
            c_idx = idx % cols_per_row
            left = Inches(0.6 + c_idx * (card_w + 0.3))
            top = Inches(1.5 + r_idx * (card_h + 0.35))
            box = slide.shapes.add_textbox(left, top, Inches(card_w), Inches(card_h))
            tf = box.text_frame
            tf.word_wrap = True

            p_lbl = tf.paragraphs[0]
            r_lbl = p_lbl.add_run()
            r_lbl.text = _clean_text(kpi.label)
            r_lbl.font.size = Pt(13)
            r_lbl.font.color.rgb = _rgb("475569")

            p_val = tf.add_paragraph()
            r_val = p_val.add_run()
            r_val.text = _clean_text(kpi.value)
            r_val.font.size = Pt(26)
            r_val.font.bold = True
            r_val.font.color.rgb = _rgb("0F172A")

            if kpi.compare:
                p_cmp = tf.add_paragraph()
                r_cmp = p_cmp.add_run()
                r_cmp.text = _clean_text(kpi.compare)
                r_cmp.font.size = Pt(11)
                r_cmp.font.color.rgb = _rgb("2563EB")


def _try_build_native_chart_data(
    chart: ExportChart,
    *,
    data_root: Path | None = None,
) -> tuple[XL_CHART_TYPE, Any] | None:
    xl_type = _NATIVE_CHART_MAP.get(chart.widget_kind.upper())
    if xl_type is None or chart.data_table is None:
        return None

    columns = chart.data_table.columns
    if len(columns) < 2:
        return None

    numeric_types = {"INT64", "FLOAT64", "DECIMAL"}
    cat_idx = next(
        (idx for idx, col in enumerate(columns) if col.data_type not in numeric_types),
        0,
    )
    series_indices = [
        idx
        for idx, col in enumerate(columns)
        if idx != cat_idx and col.data_type in numeric_types
    ]
    if not series_indices:
        return None
    if xl_type == XL_CHART_TYPE.PIE:
        series_indices = series_indices[:1]

    sampled_rows: list[list[Any]] = []
    for raw_row in iter_table_rows(chart.data_table, data_root=data_root):
        sampled_rows.append(raw_row)
        if len(sampled_rows) >= 200:
            break
    if not sampled_rows:
        return None

    categories = [
        _clean_text(str(row[cat_idx])) if cat_idx < len(row) and row[cat_idx] is not None else ""
        for row in sampled_rows
    ]

    chart_data_cls: Any = CategoryChartData
    chart_data: Any = chart_data_cls()
    chart_data.categories = categories
    for s_idx in series_indices:
        vals: list[float] = []
        for row in sampled_rows:
            raw_v = row[s_idx] if s_idx < len(row) else None
            dec = parse_decimal_safe(raw_v)
            vals.append(float(dec) if dec is not None else 0.0)
        chart_data.add_series(_clean_text(columns[s_idx].name), tuple(vals))
    return xl_type, chart_data


def _add_chart_slide(
    prs: PresentationType,
    chart: ExportChart,
    *,
    fallback_title: str,
    data_root: Path | None = None,
) -> None:
    slide_title = chart.title.strip() or fallback_title
    slide = _add_clean_content_slide(prs, slide_title)

    slide_w_in, slide_h_in = _slide_size_inches(prs)
    left = Inches(0.7)
    top = Inches(1.35)
    box_w = Inches(max(6.0, slide_w_in - 1.4))
    box_h = Inches(max(3.8, slide_h_in - 2.0))

    native = _try_build_native_chart_data(chart, data_root=data_root)
    if native is not None:
        xl_type, chart_data = native
        shapes_any: Any = slide.shapes
        gf: Any = shapes_any.add_chart(xl_type, left, top, box_w, box_h, chart_data)
        gf.chart.has_title = True
        gf.chart.chart_title.text_frame.text = _clean_text(slide_title)
        gf.chart.has_legend = True
        return

    png_bytes, _ = render_chart_bytes(
        RenderChartRequest(
            option=chart.echarts_option,
            width=chart.width_px,
            height=chart.height_px,
            format="png",
        )
    )
    aspect = chart.width_px / max(chart.height_px, 1)
    avail_w = max(6.0, slide_w_in - 1.4)
    avail_h = max(3.8, slide_h_in - 2.0)
    if avail_w / avail_h > aspect:
        pic_h = avail_h
        pic_w = avail_h * aspect
    else:
        pic_w = avail_w
        pic_h = avail_w / aspect
    pic_left = Inches((slide_w_in - pic_w) / 2.0)
    slide.shapes.add_picture(
        io.BytesIO(png_bytes),
        pic_left,
        top,
        width=Inches(pic_w),
        height=Inches(pic_h),
    )


def _add_table_slides(
    prs: PresentationType,
    table: ExportTable,
    *,
    fallback_title: str,
    tz_name: str,
    data_root: Path | None = None,
) -> None:
    columns = table.columns
    num_cols = len(columns)
    if num_cols == 0:
        return

    formatted_rows: list[list[str]] = []
    total_seen = 0
    for raw_row in iter_table_rows(table, data_root=data_root):
        if total_seen < PPTX_MAX_TABLE_ROWS:
            formatted_rows.append(
                [
                    _clean_text(
                        format_display_cell(
                            raw_row[c_idx] if c_idx < len(raw_row) else None,
                            columns[c_idx],
                            tz_name=tz_name,
                        )
                    )
                    for c_idx in range(num_cols)
                ]
            )
        total_seen += 1

    if table.totals is not None and total_seen <= PPTX_MAX_TABLE_ROWS:
        formatted_rows.append(
            [
                _clean_text(
                    format_display_cell(
                        table.totals[c_idx] if c_idx < len(table.totals) else None,
                        columns[c_idx],
                        tz_name=tz_name,
                    )
                )
                for c_idx in range(num_cols)
            ]
        )

    base_title = table.title.strip() or fallback_title
    slide_w_in, slide_h_in = _slide_size_inches(prs)
    tbl_w = Inches(max(6.5, slide_w_in - 1.2))
    tbl_h = Inches(max(4.0, slide_h_in - 2.1))

    if not formatted_rows:
        formatted_rows = [["" for _ in range(num_cols)]]

    total_pages = max(1, math.ceil(len(formatted_rows) / PPTX_ROWS_PER_SLIDE))
    headers = [_clean_text(col.name) for col in columns]

    for page_idx in range(total_pages):
        page_rows = formatted_rows[
            page_idx * PPTX_ROWS_PER_SLIDE : (page_idx + 1) * PPTX_ROWS_PER_SLIDE
        ]
        page_title = (
            f"{base_title} ({page_idx + 1}/{total_pages})"
            if total_pages > 1
            else base_title
        )
        slide = _add_clean_content_slide(prs, page_title)
        shape = slide.shapes.add_table(
            len(page_rows) + 1,
            num_cols,
            Inches(0.6),
            Inches(1.35),
            tbl_w,
            tbl_h,
        )
        ppt_tbl = shape.table

        for c_idx, hdr_name in enumerate(headers):
            cell = ppt_tbl.cell(0, c_idx)
            cell.text = hdr_name
            for p in cell.text_frame.paragraphs:
                for r in p.runs:
                    r.font.size = Pt(10)
                    r.font.bold = True

        for r_offset, row_vals in enumerate(page_rows, start=1):
            for c_idx, val_str in enumerate(row_vals):
                cell = ppt_tbl.cell(r_offset, c_idx)
                cell.text = val_str
                for p in cell.text_frame.paragraphs:
                    for r in p.runs:
                        r.font.size = Pt(9.5)

    if total_seen > PPTX_MAX_TABLE_ROWS:
        note_slide = _add_clean_content_slide(prs, f"{base_title}（截断说明）")
        tb = note_slide.shapes.add_textbox(
            Inches(0.8), Inches(2.2), Inches(max(6.0, slide_w_in - 1.6)), Inches(1.5)
        )
        run = tb.text_frame.paragraphs[0].add_run()
        run.text = (
            f"共 {total_seen:,} 行数据，PPTX 演示文稿仅分页展示前 "
            f"{PPTX_MAX_TABLE_ROWS:,} 行，完整数据请导出 XLSX 或 CSV。"
        )
        run.font.size = Pt(14)


def _apply_watermark_on_all_slides(prs: PresentationType, watermark: str) -> None:
    cleaned_wm = _clean_text(watermark)
    slide_w_in, slide_h_in = _slide_size_inches(prs)
    wm_w = min(5.0, max(3.0, slide_w_in * 0.45))

    for slide in prs.slides:
        box = slide.shapes.add_textbox(
            Inches(max(0.4, slide_w_in - wm_w - 0.4)),
            Inches(max(0.4, slide_h_in - 0.45)),
            Inches(wm_w),
            Inches(0.35),
        )
        p = box.text_frame.paragraphs[0]
        run = p.add_run()
        run.text = cleaned_wm
        run.font.size = Pt(8.5)
        run.font.color.rgb = _rgb("94A3B8")


def export(
    doc: ExportDocument,
    out_path: Path,
    *,
    data_root: Path | None = None,
) -> ExportResult:
    """Export `ExportDocument` to `.pptx` with native CategoryChartData & 15-row pagination."""
    placeholder_map = build_placeholder_map(doc)

    if doc.template_path:
        tmpl_path = resolve_template_file(doc.template_path, expected_suffix=".pptx")
        prs = Presentation(str(tmpl_path))
        _replace_template_placeholders(prs, placeholder_map)
        existing_text = _extract_slides_text(prs)
        if len(prs.slides) == 0 or (doc.title and doc.title not in existing_text):
            _add_cover_slide(prs, doc)
    else:
        prs = Presentation()
        prs.slide_width = Inches(13.333)
        prs.slide_height = Inches(7.5)
        _add_cover_slide(prs, doc)

    kpis = [sec for sec in doc.sections if isinstance(sec, ExportKpi)]
    if kpis:
        _add_kpi_slides(prs, kpis)

    chart_idx = 0
    table_idx = 0
    slide_w_in, _ = _slide_size_inches(prs)

    for section in doc.sections:
        if isinstance(section, ExportKpi):
            continue
        if isinstance(section, ExportText):
            heading_title = section.text[:60] if section.kind == "HEADING" else doc.title
            txt_slide = _add_clean_content_slide(prs, heading_title)
            if section.kind != "HEADING":
                tb = txt_slide.shapes.add_textbox(
                    Inches(0.7),
                    Inches(1.45),
                    Inches(max(6.0, slide_w_in - 1.4)),
                    Inches(4.8),
                )
                tf = tb.text_frame
                tf.word_wrap = True
                r = tf.paragraphs[0].add_run()
                r.text = _clean_text(section.text)
                r.font.size = Pt(14)
        elif isinstance(section, ExportChart):
            chart_idx += 1
            _add_chart_slide(
                prs,
                section,
                fallback_title=f"图表 {chart_idx}",
                data_root=data_root,
            )
        elif isinstance(section, ExportTable):
            table_idx += 1
            _add_table_slides(
                prs,
                section,
                fallback_title=f"数据表 {table_idx}",
                tz_name=doc.timezone,
                data_root=data_root,
            )

    if doc.watermark:
        _apply_watermark_on_all_slides(prs, doc.watermark)

    def _write_pptx(part_path: Path) -> None:
        prs.save(str(part_path))

    file_size, sha256_hex = atomic_write_export(out_path, _write_pptx)
    return ExportResult(
        file_path=out_path,
        file_size=file_size,
        sha256=sha256_hex,
        file_name=build_download_filename(doc.title, "pptx"),
    )
