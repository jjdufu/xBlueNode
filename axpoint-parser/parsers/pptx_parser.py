from __future__ import annotations

import io
import math
import posixpath
import zipfile
from pathlib import Path
from typing import Any

from pptx import Presentation

from parsers.base import ParseContext, ParseResult
from parsers.blocks import (
    extract_blocks_from_grid,
    normalize_column_names,
    render_markdown_table,
)
from parsers.office_convert import to_ooxml
from parsers.staging import write_rows_to_staging
from schemas import ExtractedTable
from security import SidecarError, check_zip, parse_xml_bytes_safe

_DGM_NS = "http://schemas.openxmlformats.org/drawingml/2006/diagram"
_A_NS = "http://schemas.openxmlformats.org/drawingml/2006/main"


def _extract_smartart_by_slide(pptx_bytes: bytes) -> dict[int, list[str]]:
    """Extract SmartArt diagram text nodes mapped by 1-based slide index."""
    smartart_texts: dict[int, list[str]] = {}
    with zipfile.ZipFile(io.BytesIO(pptx_bytes), "r") as zf:
        names = set(zf.namelist())
        for name in sorted(names):
            if not (name.startswith("ppt/slides/_rels/slide") and name.endswith(".xml.rels")):
                continue
            stem_num = name.removeprefix("ppt/slides/_rels/slide").removesuffix(".xml.rels")
            if not stem_num.isdigit():
                continue
            slide_idx = int(stem_num)
            rels_root = parse_xml_bytes_safe(zf.read(name))
            slide_nodes: list[str] = []
            for rel in rels_root:
                rel_type = rel.attrib.get("Type", "")
                target = rel.attrib.get("Target", "")
                if "diagramData" not in rel_type or not target:
                    continue
                resolved = posixpath.normpath(posixpath.join("ppt/slides", target)).lstrip("/")
                if resolved not in names:
                    continue
                dgm_root = parse_xml_bytes_safe(zf.read(resolved))
                for pt in dgm_root.findall(f".//{{{_DGM_NS}}}pt"):
                    if pt.attrib.get("type") == "doc":
                        continue
                    t_elems = pt.findall(f".//{{{_A_NS}}}t")
                    joined = "".join((el.text or "") for el in t_elems).strip()
                    if joined:
                        slide_nodes.append(joined)
            if slide_nodes:
                smartart_texts[slide_idx] = slide_nodes
    return smartart_texts


def _clean_chart_num(val: Any) -> Any:
    if isinstance(val, float) and math.isfinite(val) and val.is_integer():
        return int(val)
    return val


class PptxParser:
    """Parser for .pptx and .ppt presentations (Step 01 §01.6)."""

    suffixes: tuple[str, ...] = (".pptx", ".ppt")

    def parse(self, data: bytes, ctx: ParseContext) -> ParseResult:
        suffix = Path(ctx.filename).suffix.lower()
        stem = Path(ctx.filename).stem or "presentation"
        source_format = "ppt" if suffix == ".ppt" else "pptx"

        if suffix == ".ppt" or data.startswith(b"\xd0\xcf\x11\xe0"):
            data = to_ooxml(data, ".ppt", "pptx")
            ctx.add_warning(
                "EARLY_OFFICE_CONVERTED",
                f"converted legacy {ctx.filename} from .ppt to .pptx via LibreOffice",
            )

        check_zip(data)

        try:
            prs = Presentation(io.BytesIO(data))
        except Exception as exc:
            raise SidecarError(
                422, "PARSE_MALFORMED", f"failed to open PPTX presentation: {exc}"
            ) from exc

        smartart_map = _extract_smartart_by_slide(data)

        md_sections: list[str] = [f"# {stem}"]
        tables: list[ExtractedTable] = []

        for slide_idx, slide in enumerate(prs.slides, start=1):
            title_shape = slide.shapes.title
            slide_title = (
                title_shape.text.strip()
                if title_shape is not None and title_shape.has_text_frame
                else ""
            )
            heading = (
                f"## 第 {slide_idx} 页：{slide_title}" if slide_title else f"## 第 {slide_idx} 页"
            )
            slide_parts: list[str] = [heading]

            tbl_counter = 0
            chart_counter = 0

            def _visit_shapes(
                shapes_iter: Any,
                slide_num: int,
                cur_title_shape: Any,
                out_parts: list[str],
            ) -> None:
                nonlocal tbl_counter, chart_counter
                ordered = sorted(
                    list(shapes_iter),
                    key=lambda sh: (
                        int(getattr(sh, "top", 0) or 0),
                        int(getattr(sh, "left", 0) or 0),
                    ),
                )
                for shape in ordered:
                    if cur_title_shape is not None and shape == cur_title_shape:
                        continue
                    # GroupShape recursion
                    if hasattr(shape, "shapes"):
                        _visit_shapes(shape.shapes, slide_num, cur_title_shape, out_parts)
                        continue

                    if getattr(shape, "has_table", False):
                        tbl_counter += 1
                        tbl_obj = shape.table
                        grid = [[cell.text.strip() for cell in row.cells] for row in tbl_obj.rows]
                        blocks = extract_blocks_from_grid(grid)
                        for blk in blocks:
                            for orig, new in blk.renamed_columns:
                                ctx.add_warning(
                                    "COLUMN_RENAMED",
                                    f"slide {slide_num} column {orig!r} normalized to {new!r}",
                                )
                            ext = write_rows_to_staging(
                                ctx,
                                f"{stem}_s{slide_num}_t{tbl_counter}",
                                blk.headers,
                                blk.data_rows,
                                origin=f"slide={slide_num};table={tbl_counter}",
                                header_rows=blk.header_rows,
                                source_comments=blk.source_comments,
                                already_normalized_cols=True,
                            )
                            tables.append(ext)
                            out_parts.append(render_markdown_table(blk.markdown_rows, max_rows=50))
                        continue

                    if getattr(shape, "has_chart", False):
                        chart_counter += 1
                        chart = shape.chart
                        cats: list[str] = []
                        if chart.plots and chart.plots[0].categories:
                            cats = [str(c) for c in chart.plots[0].categories]
                        series_list = list(chart.series)
                        raw_hdrs = [
                            "category",
                            *[str(s.name or f"series_{i + 1}") for i, s in enumerate(series_list)],
                        ]
                        norm_hdrs, comments, renamed = normalize_column_names(raw_hdrs)
                        for orig, new in renamed:
                            ctx.add_warning(
                                "COLUMN_RENAMED",
                                f"slide {slide_num} chart column {orig!r} normalized to {new!r}",
                            )
                        n_rows = max(
                            len(cats),
                            max((len(s.values) for s in series_list), default=0),
                        )
                        chart_rows: list[list[Any]] = []
                        for r_i in range(n_rows):
                            cat_label = cats[r_i] if r_i < len(cats) else f"item_{r_i + 1}"
                            row_vals: list[Any] = [cat_label]
                            for s in series_list:
                                v = s.values[r_i] if r_i < len(s.values) else None
                                row_vals.append(_clean_chart_num(v))
                            chart_rows.append(row_vals)

                        ext_chart = write_rows_to_staging(
                            ctx,
                            f"{stem}_s{slide_num}_c{chart_counter}",
                            norm_hdrs,
                            chart_rows,
                            origin=f"slide={slide_num};chart={chart_counter}",
                            source_comments=comments,
                            already_normalized_cols=True,
                        )
                        tables.append(ext_chart)
                        out_parts.append(
                            render_markdown_table([norm_hdrs, *chart_rows], max_rows=50)
                        )
                        continue

                    if getattr(shape, "has_text_frame", False):
                        paras = [
                            p.text.strip()
                            for p in shape.text_frame.paragraphs
                            if p.text and p.text.strip()
                        ]
                        if paras:
                            out_parts.append("\n".join(paras))

            _visit_shapes(slide.shapes, slide_idx, title_shape, slide_parts)

            if slide_idx in smartart_map:
                smart_items = "\n".join(f"- {item}" for item in smartart_map[slide_idx])
                slide_parts.append(smart_items)

            if slide.has_notes_slide and slide.notes_slide.notes_text_frame is not None:
                raw_note = slide.notes_slide.notes_text_frame.text.strip()
                if raw_note:
                    cleaned_note = raw_note
                    for prefix in ("备注：", "备注:"):
                        if cleaned_note.startswith(prefix):
                            cleaned_note = cleaned_note[len(prefix) :].strip()
                            break
                    slide_parts.append(f"> 备注：{cleaned_note}")

            md_sections.append("\n\n".join(slide_parts))

        return ParseResult(
            title=stem,
            source_format=source_format,
            engine="python-pptx",
            markdown="\n\n".join(md_sections),
            tables=tables,
        )
