from __future__ import annotations

import io
import re
from pathlib import Path
from typing import Any

from docx import Document as DocxDocument
from docx.document import Document as DocxDocumentType
from docx.oxml import OxmlElement
from docx.oxml.ns import qn
from docx.shared import Inches, Pt, RGBColor
from docx.table import Table as DocxTable
from docx.text.paragraph import Paragraph as DocxParagraph

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
    resolve_template_file,
)

DOCX_MAX_TABLE_ROWS = 10_000

_PLACEHOLDER_RE = re.compile(r"\{\{\s*([A-Za-z0-9_.-]+)\s*\}\}")
_XML_CTRL_CHARS_RE = re.compile(r"[\x00-\x08\x0b\x0c\x0e-\x1f]")
_SECTIONS_TOKEN = "{{sections}}"


def _clean_xml_text(value: str) -> str:
    return _XML_CTRL_CHARS_RE.sub("", value)


def _replace_in_paragraph(
    paragraph: DocxParagraph,
    mapping: dict[str, str],
    *,
    preserve_sections_token: bool = True,
) -> None:
    raw_text = paragraph.text
    if "{{" not in raw_text:
        return

    def _repl(match: re.Match[str]) -> str:
        key = match.group(1)
        if preserve_sections_token and key == "sections":
            return _SECTIONS_TOKEN
        return _clean_xml_text(mapping.get(key, ""))

    runs = paragraph.runs
    if not runs:
        replaced = _PLACEHOLDER_RE.sub(_repl, raw_text)
        if replaced != raw_text:
            paragraph.text = replaced
        return

    coalesced = "".join(r.text for r in runs)
    replaced = _PLACEHOLDER_RE.sub(_repl, coalesced)
    if replaced == coalesced:
        return
    runs[0].text = replaced
    for extra_run in runs[1:]:
        extra_run.text = ""


def _replace_in_table(
    table: DocxTable,
    mapping: dict[str, str],
    *,
    preserve_sections_token: bool = True,
) -> None:
    for row in table.rows:
        for cell in row.cells:
            for para in cell.paragraphs:
                _replace_in_paragraph(
                    para, mapping, preserve_sections_token=preserve_sections_token
                )
            for nested in cell.tables:
                _replace_in_table(
                    nested, mapping, preserve_sections_token=preserve_sections_token
                )


def _replace_all_placeholders(doc_obj: DocxDocumentType, mapping: dict[str, str]) -> None:
    for para in doc_obj.paragraphs:
        _replace_in_paragraph(para, mapping, preserve_sections_token=True)
    for tbl in doc_obj.tables:
        _replace_in_table(tbl, mapping, preserve_sections_token=True)

    for section in doc_obj.sections:
        hf_containers = (
            section.header,
            section.footer,
            section.first_page_header,
            section.first_page_footer,
            section.even_page_header,
            section.even_page_footer,
        )
        for container in hf_containers:
            if container is None:
                continue
            for para in container.paragraphs:
                _replace_in_paragraph(para, mapping, preserve_sections_token=False)
            for tbl in container.tables:
                _replace_in_table(tbl, mapping, preserve_sections_token=False)


def _resolve_table_style_name(doc_obj: DocxDocumentType) -> str | None:
    style_names = {s.name for s in doc_obj.styles}
    if "AxTable" in style_names:
        return "AxTable"
    if "Table Grid" in style_names:
        return "Table Grid"
    return None


def _build_oxml_row(values: list[str], *, bold: bool = False) -> Any:
    tr = OxmlElement("w:tr")
    for text_val in values:
        tc = OxmlElement("w:tc")
        p = OxmlElement("w:p")
        r = OxmlElement("w:r")
        if bold:
            r_pr = OxmlElement("w:rPr")
            r_pr.append(OxmlElement("w:b"))
            r.append(r_pr)
        t = OxmlElement("w:t")
        cleaned = _clean_xml_text(text_val)
        if cleaned != cleaned.strip():
            t.set(qn("xml:space"), "preserve")
        t.text = cleaned
        r.append(t)
        p.append(r)
        tc.append(p)
        tr.append(tc)
    return tr


def _insert_element_at_anchor(elem: Any, anchor_el: Any | None) -> None:
    if anchor_el is not None:
        anchor_el.addprevious(elem)


def _render_sections_into_docx(
    doc_obj: DocxDocumentType,
    doc: ExportDocument,
    *,
    anchor_el: Any | None,
    data_root: Path | None = None,
) -> None:
    table_style = _resolve_table_style_name(doc_obj)

    kpis = [sec for sec in doc.sections if isinstance(sec, ExportKpi)]
    if kpis:
        kpi_hdr = doc_obj.add_heading("核心指标", level=2)
        _insert_element_at_anchor(kpi_hdr._p, anchor_el)

        kpi_tbl = doc_obj.add_table(rows=1, cols=3)
        if table_style is not None:
            kpi_tbl.style = table_style
        hdr_cells = kpi_tbl.rows[0].cells
        hdr_cells[0].text = "指标"
        hdr_cells[1].text = "数值"
        hdr_cells[2].text = "对比"
        for kpi in kpis:
            kpi_tbl._tbl.append(
                _build_oxml_row([kpi.label, kpi.value, kpi.compare or ""], bold=False)
            )
        _insert_element_at_anchor(kpi_tbl._tbl, anchor_el)

    for section in doc.sections:
        if isinstance(section, ExportKpi):
            continue

        if isinstance(section, ExportText):
            if section.kind == "HEADING":
                lvl = min(max(section.level, 1), 4)
                h_para = doc_obj.add_heading(_clean_xml_text(section.text), level=lvl)
                _insert_element_at_anchor(h_para._p, anchor_el)
            else:
                p_para = doc_obj.add_paragraph(_clean_xml_text(section.text))
                _insert_element_at_anchor(p_para._p, anchor_el)

        elif isinstance(section, ExportChart):
            if section.title:
                ch_hdr = doc_obj.add_heading(_clean_xml_text(section.title), level=2)
                _insert_element_at_anchor(ch_hdr._p, anchor_el)

            png_bytes, _ = render_chart_bytes(
                RenderChartRequest(
                    option=section.echarts_option,
                    width=section.width_px,
                    height=section.height_px,
                    format="png",
                )
            )
            pic_para = doc_obj.add_paragraph()
            pic_para.add_run().add_picture(io.BytesIO(png_bytes), width=Inches(6.0))
            _insert_element_at_anchor(pic_para._p, anchor_el)

        elif isinstance(section, ExportTable):
            if section.title:
                t_hdr = doc_obj.add_heading(_clean_xml_text(section.title), level=2)
                _insert_element_at_anchor(t_hdr._p, anchor_el)

            columns = section.columns
            num_cols = len(columns)
            if num_cols == 0:
                continue

            tbl = doc_obj.add_table(rows=0, cols=num_cols)
            if table_style is not None:
                tbl.style = table_style
            tbl._tbl.append(_build_oxml_row([col.name for col in columns], bold=True))

            total_rows = 0
            row_kinds = section.row_kinds
            for raw_row in iter_table_rows(section, data_root=data_root):
                if total_rows < DOCX_MAX_TABLE_ROWS:
                    kind = (
                        row_kinds[total_rows].upper()
                        if total_rows < len(row_kinds)
                        else "DETAIL"
                    )
                    is_bold = kind in {"SUBTOTAL", "TOTAL"}
                    formatted = [
                        format_display_cell(
                            raw_row[c_idx] if c_idx < len(raw_row) else None,
                            columns[c_idx],
                            tz_name=doc.timezone,
                        )
                        for c_idx in range(num_cols)
                    ]
                    tbl._tbl.append(_build_oxml_row(formatted, bold=is_bold))
                total_rows += 1

            if section.totals is not None and total_rows <= DOCX_MAX_TABLE_ROWS:
                totals_formatted = [
                    format_display_cell(
                        section.totals[c_idx] if c_idx < len(section.totals) else None,
                        columns[c_idx],
                        tz_name=doc.timezone,
                    )
                    for c_idx in range(num_cols)
                ]
                tbl._tbl.append(_build_oxml_row(totals_formatted, bold=True))

            _insert_element_at_anchor(tbl._tbl, anchor_el)

            if total_rows > DOCX_MAX_TABLE_ROWS:
                note_para = doc_obj.add_paragraph(
                    f"（共 {total_rows:,} 行，DOCX 仅展示前 "
                    f"{DOCX_MAX_TABLE_ROWS:,} 行，完整数据请导出 XLSX 或 CSV）"
                )
                _insert_element_at_anchor(note_para._p, anchor_el)


def _apply_watermark_footer(doc_obj: DocxDocumentType, watermark: str) -> None:
    cleaned_wm = _clean_xml_text(watermark)
    for section in doc_obj.sections:
        footer = section.footer
        footer.is_linked_to_previous = False
        existing = "\n".join(p.text for p in footer.paragraphs if p.text.strip())
        if cleaned_wm in existing:
            continue
        if footer.paragraphs and not footer.paragraphs[0].text.strip():
            target_p = footer.paragraphs[0]
        else:
            target_p = footer.add_paragraph()
        run = target_p.add_run(cleaned_wm)
        run.font.size = Pt(8.5)
        run.font.color.rgb = RGBColor(0x64, 0x74, 0x8B)


def export(
    doc: ExportDocument,
    out_path: Path,
    *,
    data_root: Path | None = None,
) -> ExportResult:
    """Export `ExportDocument` to `.docx` with optional template placeholder coalescing."""
    placeholder_map = build_placeholder_map(doc)

    if doc.template_path:
        tmpl_path = resolve_template_file(doc.template_path, expected_suffix=".docx")
        doc_obj = DocxDocument(str(tmpl_path))
        _replace_all_placeholders(doc_obj, placeholder_map)

        anchor_para: DocxParagraph | None = None
        for para in doc_obj.paragraphs:
            if _SECTIONS_TOKEN in para.text:
                anchor_para = para
                break

        existing_text = "\n".join(p.text for p in doc_obj.paragraphs)
        for sec_hdr in doc_obj.sections:
            existing_text += "\n" + "\n".join(p.text for p in sec_hdr.header.paragraphs)

        if doc.title and doc.title not in existing_text:
            title_h = doc_obj.add_heading(_clean_xml_text(doc.title), level=1)
            _insert_element_at_anchor(
                title_h._p, anchor_para._p if anchor_para is not None else None
            )

        _render_sections_into_docx(
            doc_obj,
            doc,
            anchor_el=anchor_para._p if anchor_para is not None else None,
            data_root=data_root,
        )

        if anchor_para is not None:
            if anchor_para.text.strip() == _SECTIONS_TOKEN:
                parent = anchor_para._p.getparent()
                if parent is not None:
                    parent.remove(anchor_para._p)
            else:
                _replace_in_paragraph(
                    anchor_para, {"sections": ""}, preserve_sections_token=False
                )
    else:
        doc_obj = DocxDocument()
        doc_obj.add_heading(_clean_xml_text(doc.title), level=1)
        if doc.subtitle:
            sub_p = doc_obj.add_paragraph(_clean_xml_text(doc.subtitle))
            for r in sub_p.runs:
                r.font.color.rgb = RGBColor(0x47, 0x55, 0x69)

        ts_str = format_timestamp_tz(doc.generated_at, doc.timezone)
        meta_parts = [
            f"生成人：{doc.generated_by}",
            f"生成时间：{ts_str} ({doc.timezone})",
        ]
        if doc.watermark:
            meta_parts.append(f"水印：{doc.watermark}")
        meta_p = doc_obj.add_paragraph(" | ".join(_clean_xml_text(m) for m in meta_parts))
        for r in meta_p.runs:
            r.font.size = Pt(9)
            r.font.color.rgb = RGBColor(0x64, 0x74, 0x8B)

        _render_sections_into_docx(
            doc_obj,
            doc,
            anchor_el=None,
            data_root=data_root,
        )

    if doc.watermark:
        _apply_watermark_footer(doc_obj, doc.watermark)

    def _write_docx(part_path: Path) -> None:
        doc_obj.save(str(part_path))

    file_size, sha256_hex = atomic_write_export(out_path, _write_docx)
    return ExportResult(
        file_path=out_path,
        file_size=file_size,
        sha256=sha256_hex,
        file_name=build_download_filename(doc.title, "docx"),
    )
