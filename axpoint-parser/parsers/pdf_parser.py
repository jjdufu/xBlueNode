from __future__ import annotations

import io
import tempfile
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from pypdf import PdfReader

from parsers.base import ParseContext, ParseResult
from parsers.blocks import (
    extract_blocks_from_grid,
    render_markdown_table,
)
from parsers.staging import write_rows_to_staging
from schemas import ExtractedTable
from security import SidecarError
from settings import get_model_path, get_pdf_max_pages, is_ocr_enabled

_IMAGE_SUFFIXES = frozenset({".png", ".jpg", ".jpeg", ".tiff", ".tif"})


@dataclass
class _PdfSpan:
    x: float
    y: float
    font_size: float
    text: str


def _group_spans_into_lines(spans: list[_PdfSpan], y_tol: float = 3.5) -> list[list[_PdfSpan]]:
    """Cluster PDF text spans into horizontal lines sorted top-to-bottom, left-to-right."""
    if not spans:
        return []
    sorted_by_y = sorted(spans, key=lambda s: (-s.y, s.x))
    lines: list[list[_PdfSpan]] = []
    current_line: list[_PdfSpan] = [sorted_by_y[0]]
    current_y = sorted_by_y[0].y

    for sp in sorted_by_y[1:]:
        if abs(sp.y - current_y) <= y_tol:
            current_line.append(sp)
        else:
            current_line.sort(key=lambda s: s.x)
            lines.append(current_line)
            current_line = [sp]
            current_y = sp.y
    current_line.sort(key=lambda s: s.x)
    lines.append(current_line)
    return lines


def _cluster_x_anchors(multi_lines: list[list[_PdfSpan]], x_tol: float = 24.0) -> list[float]:
    all_xs = sorted(sp.x for ln in multi_lines for sp in ln)
    anchors: list[float] = []
    for x in all_xs:
        if not anchors or abs(x - anchors[-1]) > x_tol:
            anchors.append(x)
        else:
            anchors[-1] = (anchors[-1] + x) / 2.0
    return anchors


def _extract_pypdf_page_layout(
    page: Any,
) -> tuple[list[str], list[list[list[str]]]]:
    """Extract ordered Markdown paragraphs/headings and multi-column tables from a pypdf page."""
    spans: list[_PdfSpan] = []

    def _visitor(
        text: str,
        _cm: list[float],
        tm: list[float],
        _font_dict: dict[str, Any] | None,
        font_size: float,
    ) -> None:
        cleaned = (text or "").strip()
        if cleaned:
            x = float(tm[4]) if len(tm) >= 6 else 0.0
            y = float(tm[5]) if len(tm) >= 6 else 0.0
            spans.append(_PdfSpan(x=x, y=y, font_size=float(font_size or 11.0), text=cleaned))

    try:
        page.extract_text(visitor_text=_visitor)
    except Exception:
        fallback_txt = (page.extract_text() or "").strip()
        return ([fallback_txt] if fallback_txt else []), []

    if not spans:
        return [], []

    lines = _group_spans_into_lines(spans)
    md_items: list[str] = []
    page_tables: list[list[list[str]]] = []

    idx = 0
    n_lines = len(lines)
    para_buf: list[str] = []

    def _flush_para() -> None:
        if para_buf:
            # Join CJK wrapped lines seamlessly, ASCII with space
            combined = para_buf[0]
            for nxt in para_buf[1:]:
                if (
                    combined
                    and nxt
                    and (
                        "\u4e00" <= combined[-1] <= "\u9fff"
                        or "\u4e00" <= nxt[0] <= "\u9fff"
                        or nxt[0] in "。，、；：？！）"
                    )
                ):
                    combined += nxt
                else:
                    combined += " " + nxt
            md_items.append(combined)
            para_buf.clear()

    while idx < n_lines:
        ln = lines[idx]
        # Check if a run of >= 3 consecutive multi-column lines starts here
        if len(ln) >= 2:
            run_end = idx
            while run_end < n_lines and len(lines[run_end]) >= 2:
                run_end += 1
            if run_end - idx >= 3:
                _flush_para()
                tbl_lines = lines[idx:run_end]
                anchors = _cluster_x_anchors(tbl_lines)
                if len(anchors) >= 2:
                    grid: list[list[str]] = []
                    for t_ln in tbl_lines:
                        row_cells = [""] * len(anchors)
                        for sp in t_ln:
                            best_c = min(
                                range(len(anchors)),
                                key=lambda ci: abs(sp.x - anchors[ci]),
                            )
                            row_cells[best_c] = (
                                f"{row_cells[best_c]} {sp.text}".strip()
                                if row_cells[best_c]
                                else sp.text
                            )
                        grid.append(row_cells)
                    page_tables.append(grid)
                    idx = run_end
                    continue

        line_text = " ".join(sp.text for sp in ln).strip()
        max_fs = max(sp.font_size for sp in ln)
        if max_fs >= 15.0:
            _flush_para()
            lvl = "##" if max_fs >= 16.0 else "###"
            md_items.append(f"{lvl} {line_text}")
        else:
            para_buf.append(line_text)
        idx += 1

    _flush_para()
    return md_items, page_tables


class PdfAndImageParser:
    """Parser for .pdf and raster images (.png/.jpg/.jpeg/.tiff) (Step 01 §01.8)."""

    suffixes: tuple[str, ...] = (".pdf", ".png", ".jpg", ".jpeg", ".tiff", ".tif")

    def parse(self, data: bytes, ctx: ParseContext) -> ParseResult:
        suffix = Path(ctx.filename).suffix.lower()
        stem = Path(ctx.filename).stem or "document"
        is_img = suffix in _IMAGE_SUFFIXES

        ocr_opt = ctx.options.get("ocr", "AUTO").upper()
        ocr_allowed = is_ocr_enabled() and ocr_opt != "OFF"

        # 1. Try Docling in full image tier when available
        if ocr_allowed or not is_img:
            docling_res = self._try_docling(data, suffix, stem, ctx, do_ocr=ocr_allowed)
            if docling_res is not None:
                return docling_res

        # 2. Image fallback in lite tier
        if is_img:
            if ocr_allowed:
                easy_res = self._try_easyocr_image(data, stem, ctx)
                if easy_res is not None:
                    return easy_res
            raise SidecarError(
                status_code=415,
                error_code="UNSUPPORTED_FORMAT",
                message="image OCR requires optional docling/easyocr models",
            )

        # 3. PDF pypdf layout & table extraction
        try:
            reader = PdfReader(io.BytesIO(data))
        except Exception as exc:
            raise SidecarError(422, "PARSE_MALFORMED", f"failed to parse PDF file: {exc}") from exc

        max_pages = get_pdf_max_pages()
        total_pages = len(reader.pages)
        pages_to_read = min(total_pages, max_pages)
        if total_pages > max_pages:
            ctx.add_warning(
                "TRUNCATED_PAGES",
                f"PDF has {total_pages} pages; truncated to first {max_pages} pages",
            )

        meta_title = ""
        if reader.metadata and getattr(reader.metadata, "title", None):
            meta_title = str(reader.metadata.title).strip()
        doc_title = meta_title or stem

        md_sections: list[str] = [f"# {doc_title}"]
        tables: list[ExtractedTable] = []
        any_text = False

        for p_idx in range(1, pages_to_read + 1):
            page = reader.pages[p_idx - 1]
            p_md_items, p_tables = _extract_pypdf_page_layout(page)
            if p_md_items or p_tables:
                any_text = True
            md_sections.extend(p_md_items)

            for t_idx, grid in enumerate(p_tables, start=1):
                total_cells = sum(len(r) for r in grid)
                empty_cells = sum(1 for r in grid for c in r if not c.strip())
                empty_ratio = (empty_cells / total_cells) if total_cells > 0 else 1.0
                if len(grid) < 3 or empty_ratio > 0.40:
                    ctx.add_warning(
                        "TABLE_LOW_QUALITY",
                        f"page {p_idx} table {t_idx} excluded from table track "
                        f"(rows={len(grid)}, empty_ratio={empty_ratio:.2f})",
                    )
                    md_sections.append(render_markdown_table(grid, max_rows=50))
                    continue

                blocks = extract_blocks_from_grid(grid)
                for blk in blocks:
                    for orig, new in blk.renamed_columns:
                        ctx.add_warning(
                            "COLUMN_RENAMED",
                            f"page {p_idx} table {t_idx} column {orig!r} normalized to {new!r}",
                        )
                    ext = write_rows_to_staging(
                        ctx,
                        f"{stem}_p{p_idx}_t{t_idx}",
                        blk.headers,
                        blk.data_rows,
                        origin=f"page={p_idx};table={t_idx}",
                        header_rows=blk.header_rows,
                        source_comments=blk.source_comments,
                        already_normalized_cols=True,
                    )
                    tables.append(ext)
                    md_sections.append(render_markdown_table(blk.markdown_rows, max_rows=50))

        if not any_text and ocr_opt != "OFF":
            ctx.add_warning(
                "OCR_LOW_CONFIDENCE",
                "no extractable text layer found in PDF and OCR engine is disabled or unavailable",
            )

        return ParseResult(
            title=doc_title,
            source_format="pdf",
            engine="pypdf",
            markdown="\n\n".join(md_sections),
            tables=tables,
        )

    def _try_docling(
        self,
        data: bytes,
        suffix: str,
        stem: str,
        ctx: ParseContext,
        *,
        do_ocr: bool,
    ) -> ParseResult | None:
        try:
            from docling.datamodel.base_models import InputFormat
            from docling.datamodel.pipeline_options import EasyOcrOptions, PdfPipelineOptions
            from docling.document_converter import (
                DocumentConverter,
                ImageFormatOption,
                PdfFormatOption,
            )
        except ImportError:
            return None

        try:
            model_root = get_model_path()
            easyocr_dir = model_root / "easyocr"
            doc2md_dir = model_root / "doc2md"
            easyocr_opts = EasyOcrOptions()
            if easyocr_dir.is_dir():
                easyocr_opts.model_storage_directory = str(easyocr_dir)

            pdf_opts = PdfPipelineOptions(
                artifacts_path=str(doc2md_dir) if doc2md_dir.is_dir() else None
            )
            pdf_opts.ocr_options = easyocr_opts
            pdf_opts.do_ocr = do_ocr
            pdf_opts.do_table_structure = True

            converter = DocumentConverter(
                format_options={
                    InputFormat.PDF: PdfFormatOption(pipeline_options=pdf_opts),
                    InputFormat.IMAGE: ImageFormatOption(pipeline_options=pdf_opts),
                }
            )
            with tempfile.NamedTemporaryFile(suffix=suffix, delete=True) as tmp:
                tmp.write(data)
                tmp.flush()
                conv = converter.convert(tmp.name)
                md_text = str(conv.document.export_to_markdown())

            tables: list[ExtractedTable] = []
            for t_idx, doc_tbl in enumerate(getattr(conv.document, "tables", []), start=1):
                try:
                    pd_df = doc_tbl.export_to_dataframe()
                    grid = [list(pd_df.columns), *pd_df.values.tolist()]
                except Exception:
                    continue
                total_cells = sum(len(r) for r in grid)
                empty_cells = sum(1 for r in grid for c in r if c is None or str(c).strip() == "")
                empty_ratio = (empty_cells / total_cells) if total_cells > 0 else 1.0
                if len(grid) < 3 or empty_ratio > 0.40:
                    ctx.add_warning(
                        "TABLE_LOW_QUALITY",
                        f"table {t_idx} excluded from table track",
                    )
                    continue
                blocks = extract_blocks_from_grid(grid)
                for blk in blocks:
                    ext = write_rows_to_staging(
                        ctx,
                        f"{stem}_t{t_idx}",
                        blk.headers,
                        blk.data_rows,
                        origin=f"page=1;table={t_idx}",
                        header_rows=blk.header_rows,
                        source_comments=blk.source_comments,
                        already_normalized_cols=True,
                    )
                    tables.append(ext)

            return ParseResult(
                title=stem,
                source_format="image" if suffix in _IMAGE_SUFFIXES else "pdf",
                engine="docling",
                markdown=md_text,
                tables=tables,
            )
        except Exception:
            return None

    def _try_easyocr_image(self, data: bytes, stem: str, ctx: ParseContext) -> ParseResult | None:
        try:
            import easyocr
        except ImportError:
            return None

        try:
            model_root = get_model_path() / "easyocr"
            kwargs: dict[str, Any] = {"gpu": False, "verbose": False}
            if model_root.is_dir():
                kwargs["model_storage_directory"] = str(model_root)
                kwargs["download_enabled"] = False
            reader = easyocr.Reader(["ch_sim", "en"], **kwargs)
            results = reader.readtext(data)
            texts: list[str] = []
            confs: list[float] = []
            for _bbox, txt, conf in results:
                if txt and str(txt).strip():
                    texts.append(str(txt).strip())
                    confs.append(float(conf))
            if confs and (sum(confs) / len(confs)) < 0.60:
                ctx.add_warning(
                    "OCR_LOW_CONFIDENCE",
                    f"average OCR confidence {sum(confs) / len(confs):.2f} < 0.60",
                )
            md = f"# {stem}\n\n" + "\n".join(texts)
            return ParseResult(
                title=stem,
                source_format="image",
                engine="easyocr",
                markdown=md,
                tables=[],
            )
        except Exception:
            return None
