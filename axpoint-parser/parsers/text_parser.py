from __future__ import annotations

import csv
import re
from pathlib import Path

from lxml import html as lxml_html

from parsers.base import ParseContext, ParseResult
from parsers.blocks import (
    extract_blocks_from_grid,
    normalize_column_names,
    render_markdown_table,
)
from parsers.csv_parser import parse_delimited_text
from parsers.encoding import detect_and_decode
from parsers.staging import write_rows_to_staging
from schemas import ExtractedTable

_CANDIDATE_DELIMS = ("|", "\t", ";", ",", "^")
_KV_LINE_RE = re.compile(r"^\s*([^:=：\r\n]{1,64}?)\s*[:=：]\s*(.*?)\s*$")


def _try_detect_delimited_txt(lines: list[str]) -> str | None:
    """Rule 1: first 50 non-empty lines yield a consistent delimiter with >= 2 columns."""
    sample = [ln for ln in lines if ln.strip()][:50]
    if len(sample) < 2:
        return None

    for delim in _CANDIDATE_DELIMS:
        counts: list[int] = []
        max_cell_len = 0
        valid = True
        for ln in sample:
            try:
                row = next(csv.reader([ln], delimiter=delim, quotechar='"', strict=True))
            except csv.Error:
                valid = False
                break
            counts.append(len(row))
            for cell in row:
                if len(cell) > max_cell_len:
                    max_cell_len = len(cell)
        if not valid or not counts:
            continue
        dominant = max(set(counts), key=counts.count)
        if dominant >= 2 and (counts.count(dominant) / len(sample)) >= 0.95 and max_cell_len <= 120:
            return delim
    return None


def _try_parse_fixed_width(
    lines: list[str],
) -> tuple[list[str], list[list[str]]] | None:
    """Rule 2: >=90% of first 50 lines share exact length and >=2 space separator columns."""
    non_empty = [ln.rstrip("\r\n") for ln in lines if ln.strip()]
    sample = non_empty[:50]
    if len(sample) < 3:
        return None

    lengths = [len(ln) for ln in sample]
    target_len = max(set(lengths), key=lengths.count)
    if target_len < 10 or (lengths.count(target_len) / len(sample)) < 0.90:
        return None

    uniform_sample = [ln for ln in sample if len(ln) == target_len]
    space_positions = [
        col_i for col_i in range(target_len) if all(ln[col_i] == " " for ln in uniform_sample)
    ]
    if len(space_positions) < 2:
        return None

    # Group contiguous all-space positions into separator intervals
    gaps: list[tuple[int, int]] = []
    g_start = space_positions[0]
    g_prev = space_positions[0]
    for pos in space_positions[1:]:
        if pos == g_prev + 1:
            g_prev = pos
        else:
            gaps.append((g_start, g_prev + 1))
            g_start = pos
            g_prev = pos
    gaps.append((g_start, g_prev + 1))

    # Build non-space column slices between gaps
    col_slices: list[tuple[int, int]] = []
    cursor = 0
    for gs, ge in gaps:
        if gs > cursor:
            col_slices.append((cursor, gs))
        cursor = ge
    if cursor < target_len:
        col_slices.append((cursor, target_len))

    # Filter out any empty slice and require >= 3 columns (>= 2 internal gaps)
    valid_slices = [(s, e) for s, e in col_slices if any(ln[s:e].strip() for ln in uniform_sample)]
    if len(valid_slices) < 3:
        return None

    extracted_rows: list[list[str]] = []
    for ln in non_empty:
        padded = ln.ljust(target_len) if len(ln) < target_len else ln
        row = [padded[s:e].strip() for s, e in valid_slices]
        if any(row):
            extracted_rows.append(row)

    if len(extracted_rows) < 2:
        return None
    return extracted_rows[0], extracted_rows[1:]


def _try_parse_key_value(
    text: str,
) -> tuple[list[str], list[list[str]]] | None:
    """Rule 3: >=80% of non-empty lines match key: value; blank lines separate records."""
    all_lines = text.splitlines()
    non_empty = [ln for ln in all_lines if ln.strip()]
    if len(non_empty) < 2:
        return None

    kv_hits = sum(1 for ln in non_empty if _KV_LINE_RE.match(ln) is not None)
    if kv_hits / len(non_empty) < 0.80:
        return None

    records: list[dict[str, str]] = []
    key_order: list[str] = []
    current: dict[str, str] = {}

    for ln in all_lines:
        if not ln.strip():
            if current:
                records.append(current)
                current = {}
            continue
        m = _KV_LINE_RE.match(ln)
        if m:
            k = m.group(1).strip()
            v = m.group(2).strip()
            if k in current and records or (k in current and len(current) >= 2):
                # Repeated key without blank line also starts a new record
                records.append(current)
                current = {}
            if k not in key_order:
                key_order.append(k)
            current[k] = v

    if current:
        records.append(current)

    if not records or not key_order:
        return None

    rows = [[rec.get(k, "") for k in key_order] for rec in records]
    return key_order, rows


def _format_plain_markdown(stem: str, text: str) -> str:
    chapters = re.split(r"(?:\r?\n\s*){3,}", text.strip())
    if len(chapters) <= 1:
        paras = [p.strip() for p in re.split(r"(?:\r?\n\s*){2,}", text.strip()) if p.strip()]
        body = "\n\n".join(paras)
        return f"# {stem}\n\n{body}".strip()

    sections: list[str] = [f"# {stem}"]
    for idx, ch in enumerate(chapters, start=1):
        paras = [p.strip() for p in re.split(r"(?:\r?\n\s*){2,}", ch.strip()) if p.strip()]
        if paras:
            sections.append(f"## 第 {idx} 节\n\n" + "\n\n".join(paras))
    return "\n\n".join(sections)


class TextParser:
    """Parser for .txt, .md, .markdown, .rst, .html, and .htm files (Step 01 §01.4)."""

    suffixes: tuple[str, ...] = (".txt", ".md", ".markdown", ".rst", ".html", ".htm")

    def parse(self, data: bytes, ctx: ParseContext) -> ParseResult:
        suffix = Path(ctx.filename).suffix.lower()
        stem = Path(ctx.filename).stem or "document"
        text, encoding, used_fallback = detect_and_decode(data)
        if used_fallback:
            ctx.add_warning(
                "ENCODING_FALLBACK",
                f"decoded {ctx.filename} using fallback encoding {encoding}",
            )

        if suffix in {".md", ".markdown", ".rst"}:
            fmt = "md" if suffix in {".md", ".markdown"} else "rst"
            return ParseResult(
                title=stem,
                source_format=fmt,
                engine="text-markdown",
                markdown=text.strip(),
                tables=[],
            )

        if suffix in {".html", ".htm"}:
            return self._parse_html(text, stem, ctx)

        lines = text.splitlines()

        # 1. Delimited text
        delim = _try_detect_delimited_txt(lines)
        if delim is not None:
            res = parse_delimited_text(
                text,
                ctx,
                delimiter=delim,
                source_format="txt",
                engine="text-delimited",
            )
            return res

        # 2. Fixed-width text
        fw = _try_parse_fixed_width(lines)
        if fw is not None:
            raw_headers, data_rows = fw
            norm_cols, comments, renamed = normalize_column_names(raw_headers)
            for orig, new in renamed:
                ctx.add_warning("COLUMN_RENAMED", f"column {orig!r} normalized to {new!r}")
            tbl = write_rows_to_staging(
                ctx,
                stem,
                norm_cols,
                data_rows,
                origin="fixed_width=true",
                source_comments=comments,
                already_normalized_cols=True,
            )
            md_tbl = render_markdown_table([norm_cols, *data_rows], max_rows=20)
            return ParseResult(
                title=stem,
                source_format="txt",
                engine="text-fixed-width",
                markdown=f"# {stem}\n\n{md_tbl}",
                tables=[tbl],
            )

        # 3. Key-value text
        kv = _try_parse_key_value(text)
        if kv is not None:
            raw_keys, data_rows = kv
            norm_cols, comments, renamed = normalize_column_names(raw_keys)
            for orig, new in renamed:
                ctx.add_warning("COLUMN_RENAMED", f"column {orig!r} normalized to {new!r}")
            tbl = write_rows_to_staging(
                ctx,
                stem,
                norm_cols,
                data_rows,
                origin="key_value=true",
                source_comments=comments,
                already_normalized_cols=True,
            )
            md_tbl = render_markdown_table([norm_cols, *data_rows], max_rows=20)
            return ParseResult(
                title=stem,
                source_format="txt",
                engine="text-key-value",
                markdown=f"# {stem}\n\n{md_tbl}",
                tables=[tbl],
            )

        # 4. Plain text
        return ParseResult(
            title=stem,
            source_format="txt",
            engine="text-plain",
            markdown=_format_plain_markdown(stem, text),
            tables=[],
        )

    def _parse_html(self, text: str, stem: str, ctx: ParseContext) -> ParseResult:
        doc = lxml_html.fromstring(text or "<html></html>")
        md_parts: list[str] = []
        tables: list[ExtractedTable] = []

        title_el = doc.xpath("//title/text()")
        title = str(title_el[0]).strip() if title_el and str(title_el[0]).strip() else stem

        for idx, tbl_el in enumerate(doc.xpath("//table"), start=1):
            grid: list[list[str]] = []
            for tr in tbl_el.xpath(".//tr"):
                cells = [" ".join(td.itertext()).strip() for td in tr.xpath("./th|./td")]
                if cells:
                    grid.append(cells)
            if len(grid) >= 2:
                blocks = extract_blocks_from_grid(grid)
                for blk in blocks:
                    extracted = write_rows_to_staging(
                        ctx,
                        f"{stem}_{idx}",
                        blk.headers,
                        blk.data_rows,
                        origin=f"html_table={idx}",
                        header_rows=blk.header_rows,
                        source_comments=blk.source_comments,
                        already_normalized_cols=True,
                    )
                    tables.append(extracted)
                    md_parts.append(render_markdown_table(blk.markdown_rows, max_rows=30))

        body_nodes = doc.xpath("//body//*[self::h1 or self::h2 or self::h3 or self::p or self::li]")
        if body_nodes:
            text_blocks: list[str] = []
            for node in body_nodes:
                content = " ".join(node.itertext()).strip()
                if not content:
                    continue
                tag = str(node.tag).lower()
                if tag == "h1":
                    text_blocks.append(f"# {content}")
                elif tag == "h2":
                    text_blocks.append(f"## {content}")
                elif tag == "h3":
                    text_blocks.append(f"### {content}")
                elif tag == "li":
                    text_blocks.append(f"- {content}")
                else:
                    text_blocks.append(content)
            md_full = "\n\n".join([*text_blocks, *md_parts])
        else:
            raw_txt = " ".join(doc.itertext()).strip()
            md_full = "\n\n".join(filter(None, [f"# {title}", raw_txt, *md_parts]))

        return ParseResult(
            title=title,
            source_format="html",
            engine="lxml-html",
            markdown=md_full,
            tables=tables,
        )
