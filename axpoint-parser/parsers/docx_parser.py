from __future__ import annotations

import io
import re
from pathlib import Path
from typing import Any

from docx import Document as DocxDocument
from docx.oxml.table import CT_Tbl
from docx.oxml.text.paragraph import CT_P
from docx.table import Table
from docx.text.paragraph import Paragraph

from parsers.base import ParseContext, ParseResult
from parsers.blocks import (
    MergeSpan,
    extract_blocks_from_grid,
    render_markdown_table,
)
from parsers.office_convert import to_ooxml
from parsers.staging import write_rows_to_staging
from schemas import ExtractedTable
from security import SidecarError, check_zip

_HEADING_RE = re.compile(r"heading\s*(\d+)", re.IGNORECASE)


def _extract_direct_cell_text(cell: Any) -> str:
    """Extract direct cell paragraph text (excluding nested tables) and dedupe vMerge."""
    lines: list[str] = []
    for p in cell.paragraphs:
        txt = p.text.strip()
        if not txt:
            continue
        if not lines or lines[-1] != txt:
            lines.append(txt)
    return " ".join(lines)


class DocxParser:
    """Parser for .docx and .doc documents (Step 01 §01.7)."""

    suffixes: tuple[str, ...] = (".docx", ".doc")

    def parse(self, data: bytes, ctx: ParseContext) -> ParseResult:
        suffix = Path(ctx.filename).suffix.lower()
        stem = Path(ctx.filename).stem or "document"
        source_format = "doc" if suffix == ".doc" else "docx"

        if suffix == ".doc" or data.startswith(b"\xd0\xcf\x11\xe0"):
            data = to_ooxml(data, ".doc", "docx")
            ctx.add_warning(
                "EARLY_OFFICE_CONVERTED",
                f"converted legacy {ctx.filename} from .doc to .docx via LibreOffice",
            )

        check_zip(data)

        try:
            doc = DocxDocument(io.BytesIO(data))
        except Exception as exc:
            raise SidecarError(
                422, "PARSE_MALFORMED", f"failed to open DOCX document: {exc}"
            ) from exc

        md_parts: list[str] = []
        tables: list[ExtractedTable] = []
        doc_title = stem
        first_h1_seen = False
        top_tbl_counter = 0

        def _process_table(
            tbl: Table,
            *,
            origin_prefix: str,
            raw_name_prefix: str,
        ) -> str:
            n_rows = len(tbl.rows)
            if n_rows == 0:
                return ""

            grid: list[list[str]] = []
            md_override_grid: list[list[str]] = []
            merges: list[MergeSpan] = []
            deferred_nested: list[tuple[Table, str, str]] = []
            nested_counter = 0
            seen_nested_tcs: set[int] = set()

            for r_idx, row in enumerate(tbl.rows):
                cells = row.cells
                row_vals: list[str] = []
                md_vals: list[str] = []
                c_idx = 0
                n_cols = len(cells)
                while c_idx < n_cols:
                    cell = cells[c_idx]
                    tc = cell._tc
                    tc_id = id(tc)
                    txt = _extract_direct_cell_text(cell)

                    # Detect horizontal merge run where adjacent row.cells share identical _tc
                    span_end = c_idx
                    while span_end + 1 < n_cols and cells[span_end + 1]._tc is tc:
                        span_end += 1
                    if span_end > c_idx:
                        merges.append(
                            MergeSpan(
                                min_row=r_idx,
                                min_col=c_idx,
                                max_row=r_idx,
                                max_col=span_end,
                            )
                        )

                    md_txt = txt
                    if cell.tables and tc_id not in seen_nested_tcs:
                        seen_nested_tcs.add(tc_id)
                        for sub_tbl in cell.tables:
                            nested_counter += 1
                            placeholder = f"[嵌套表 {nested_counter}]"
                            md_txt = f"{md_txt} {placeholder}".strip() if md_txt else placeholder
                            deferred_nested.append(
                                (
                                    sub_tbl,
                                    f"{origin_prefix};nested={nested_counter}",
                                    f"{raw_name_prefix}_nested_{nested_counter}",
                                )
                            )

                    for k in range(c_idx, span_end + 1):
                        row_vals.append(txt if k == c_idx else "")
                        md_vals.append(md_txt if k == c_idx else "")
                    c_idx = span_end + 1

                grid.append(row_vals)
                md_override_grid.append(md_vals)

            # Tables with < 3 rows stay in Markdown text track only
            if n_rows < 3:
                small_md = render_markdown_table(md_override_grid, max_rows=50)
                nested_md_blocks = [
                    _process_table(st, origin_prefix=op, raw_name_prefix=rp)
                    for st, op, rp in deferred_nested
                ]
                nested_md_blocks = [b for b in nested_md_blocks if b]
                return "\n\n".join([small_md, *nested_md_blocks]) if nested_md_blocks else small_md

            blocks = extract_blocks_from_grid(grid, merges)
            rendered_sections: list[str] = []
            for b_idx, blk in enumerate(blocks, start=1):
                for orig, new in blk.renamed_columns:
                    ctx.add_warning(
                        "COLUMN_RENAMED",
                        f"{origin_prefix} column {orig!r} normalized to {new!r}",
                    )
                t_name = raw_name_prefix if len(blocks) == 1 else f"{raw_name_prefix}_{b_idx}"
                ext = write_rows_to_staging(
                    ctx,
                    t_name,
                    blk.headers,
                    blk.data_rows,
                    origin=origin_prefix,
                    header_rows=blk.header_rows,
                    source_comments=blk.source_comments,
                    already_normalized_cols=True,
                )
                tables.append(ext)

                # Build Markdown table preserving [嵌套表 N] placeholders
                hdr_rows = blk.header_rows
                body_md_rows = md_override_grid[blk.row_offset + hdr_rows :]
                # Fill horizontal/vertical merges while keeping nested placeholders
                merged_md = [blk.headers]
                for r_i, base_md_r in enumerate(blk.markdown_rows[1:]):
                    if r_i < len(body_md_rows):
                        combined_r = [
                            body_md_rows[r_i][c_i]
                            if c_i < len(body_md_rows[r_i]) and "[嵌套表" in body_md_rows[r_i][c_i]
                            else base_md_r[c_i]
                            for c_i in range(len(base_md_r))
                        ]
                        merged_md.append(combined_r)
                    else:
                        merged_md.append(base_md_r)
                rendered_sections.append(render_markdown_table(merged_md, max_rows=50))

            for st, op, rp in deferred_nested:
                sub_md = _process_table(st, origin_prefix=op, raw_name_prefix=rp)
                if sub_md:
                    rendered_sections.append(sub_md)
            return "\n\n".join(s for s in rendered_sections if s)

        for child in doc.element.body.iterchildren():
            if isinstance(child, CT_P):
                para = Paragraph(child, doc)
                txt = para.text.strip()
                if not txt:
                    continue
                style_name = para.style.name if para.style is not None and para.style.name else ""
                m_h = _HEADING_RE.search(style_name)
                if m_h:
                    lvl = min(max(int(m_h.group(1)), 1), 6)
                    if lvl == 1 and not first_h1_seen:
                        doc_title = txt
                        first_h1_seen = True
                    md_parts.append(f"{'#' * lvl} {txt}")
                elif style_name.lower().startswith("title") and not first_h1_seen:
                    doc_title = txt
                    first_h1_seen = True
                    md_parts.append(f"# {txt}")
                elif "list" in style_name.lower():
                    md_parts.append(f"- {txt}")
                else:
                    md_parts.append(txt)
            elif isinstance(child, CT_Tbl):
                top_tbl_counter += 1
                tbl_obj = Table(child, doc)
                tbl_md = _process_table(
                    tbl_obj,
                    origin_prefix=f"table={top_tbl_counter}",
                    raw_name_prefix=f"{stem}_t{top_tbl_counter}",
                )
                if tbl_md:
                    md_parts.append(tbl_md)

        return ParseResult(
            title=doc_title,
            source_format=source_format,
            engine="python-docx",
            markdown="\n\n".join(md_parts),
            tables=tables,
        )
