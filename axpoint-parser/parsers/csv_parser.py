from __future__ import annotations

import csv
import tempfile
from pathlib import Path

import duckdb
import polars as pl

from parsers.base import ParseContext, ParseResult
from parsers.blocks import (
    extract_blocks_from_grid,
    normalize_column_names,
    normalize_table_name,
    render_markdown_table,
)
from parsers.encoding import detect_and_decode
from parsers.profiler import profile_polars_df
from parsers.staging import write_rows_to_staging
from schemas import ExtractedTable

_CANDIDATE_DELIMITERS = (",", ";", "\t", "|", "^")
_LARGE_CSV_THRESHOLD_BYTES = 256 * 1024 * 1024


def sniff_delimiter(sample_text: str, *, default: str = ",") -> str:
    """Sniff the most consistent delimiter among (',', ';', '\\t', '|', '^') on sample lines."""
    lines = [ln for ln in sample_text.splitlines() if ln.strip()][:50]
    if not lines:
        return default

    best_delim = default
    best_score = -1.0
    for delim in _CANDIDATE_DELIMITERS:
        col_counts: list[int] = []
        for ln in lines:
            try:
                row = next(csv.reader([ln], delimiter=delim, quotechar='"', strict=True))
                col_counts.append(len(row))
            except csv.Error:
                continue
        if not col_counts:
            continue
        dominant = max(set(col_counts), key=col_counts.count)
        if dominant < 2:
            continue
        consistency = col_counts.count(dominant) / len(lines)
        score = consistency * 100.0 + min(dominant, 20)
        if consistency >= 0.80 and score > best_score:
            best_score = score
            best_delim = delim
    return best_delim


def _parse_csv_records_strict(text: str, delimiter: str) -> tuple[list[list[str]], int]:
    """Parse CSV text with strict RFC 4180 quote validation and column consistency checks.

    Supports valid multi-line quoted values while rejecting lines with unclosed/corrupt quotes
    or inconsistent field counts.
    """
    raw_lines = text.splitlines(keepends=True)
    good_rows: list[list[str]] = []
    malformed_rows = 0

    idx = 0
    n_lines = len(raw_lines)
    while idx < n_lines:
        line = raw_lines[idx]
        if not line.strip():
            idx += 1
            continue

        # Fast-path single line strict parse
        try:
            parsed = list(csv.reader([line], delimiter=delimiter, quotechar='"', strict=True))
            if parsed:
                good_rows.append(parsed[0])
            idx += 1
            continue
        except csv.Error:
            pass

        # Could be a legitimate multiline quoted record OR a single-line malformed quote.
        # Only accumulate up to 8 lines if quote count is odd and quotes aren't followed by garbage.
        combined = line
        merged = False
        if combined.count('"') % 2 == 1:
            for lookahead in range(1, min(8, n_lines - idx)):
                combined += raw_lines[idx + lookahead]
                if combined.count('"') % 2 == 0:
                    try:
                        parsed_multi = list(
                            csv.reader([combined], delimiter=delimiter, quotechar='"', strict=True)
                        )
                        if len(parsed_multi) == 1:
                            good_rows.append(parsed_multi[0])
                            idx += lookahead + 1
                            merged = True
                    except csv.Error:
                        pass
                    break
        if not merged:
            malformed_rows += 1
            idx += 1

    if not good_rows:
        return [], malformed_rows

    expected_cols = len(good_rows[0])
    filtered_rows: list[list[str]] = [good_rows[0]]
    for row in good_rows[1:]:
        if len(row) != expected_cols:
            malformed_rows += 1
        else:
            filtered_rows.append(row)

    return filtered_rows, malformed_rows


def parse_delimited_text(
    text: str,
    ctx: ParseContext,
    *,
    delimiter: str,
    source_format: str,
    engine: str,
) -> ParseResult:
    """Shared CSV/TSV/delimited-TXT parser producing Markdown summary + Parquet table."""
    stem = Path(ctx.filename).stem or "table"
    rows, rejected = _parse_csv_records_strict(text, delimiter)
    if rejected > 0:
        ctx.add_warning("ROW_MALFORMED", f"rejected {rejected} malformed CSV/TSV rows")

    if not rows:
        return ParseResult(
            title=stem,
            source_format=source_format,
            engine=engine,
            markdown=f"# {stem}\n\n（空表格文件）",
            tables=[],
        )

    blocks = extract_blocks_from_grid(rows)
    if blocks:
        blk = blocks[0]
        for orig, new in blk.renamed_columns:
            ctx.add_warning(
                "COLUMN_RENAMED",
                f"column {orig!r} normalized to {new!r}",
            )
        headers = blk.headers
        data_rows = blk.data_rows
        comments = blk.source_comments
        hdr_count = blk.header_rows
        md_preview = render_markdown_table(blk.markdown_rows, max_rows=20)
    else:
        headers, comments, renamed = normalize_column_names(rows[0])
        for orig, new in renamed:
            ctx.add_warning("COLUMN_RENAMED", f"column {orig!r} normalized to {new!r}")
        data_rows = rows[1:]
        hdr_count = 1
        md_preview = render_markdown_table([headers, *data_rows], max_rows=20)

    tbl = write_rows_to_staging(
        ctx,
        stem,
        headers,
        data_rows,
        origin="block=1",
        header_rows=hdr_count,
        source_comments=comments,
        already_normalized_cols=True,
    )

    col_summary = ", ".join(f"`{c.name}` ({c.data_type})" for c in tbl.columns)
    md = (
        f"# {stem}\n\n"
        f"- **文件名**：`{ctx.filename}`\n"
        f"- **行数**：{tbl.row_count}\n"
        f"- **列清单**：{col_summary}\n\n"
        f"{md_preview}"
    )
    return ParseResult(
        title=stem,
        source_format=source_format,
        engine=engine,
        markdown=md,
        tables=[tbl],
    )


class CsvParser:
    """Parser for .csv and .tsv files (Step 01 §01.3)."""

    suffixes: tuple[str, ...] = (".csv", ".tsv")

    def parse(self, data: bytes, ctx: ParseContext) -> ParseResult:
        suffix = Path(ctx.filename).suffix.lower()
        source_format = "tsv" if suffix == ".tsv" else "csv"

        if len(data) > _LARGE_CSV_THRESHOLD_BYTES:
            return self._parse_large_duckdb(data, ctx, source_format=source_format)

        text, encoding, used_fallback = detect_and_decode(data)
        if used_fallback:
            ctx.add_warning(
                "ENCODING_FALLBACK",
                f"decoded {ctx.filename} using fallback encoding {encoding}",
            )

        delimiter = "\t" if suffix == ".tsv" else sniff_delimiter(text[:65_536])
        return parse_delimited_text(
            text,
            ctx,
            delimiter=delimiter,
            source_format=source_format,
            engine="duckdb+polars-csv",
        )

    def _parse_large_duckdb(
        self, data: bytes, ctx: ParseContext, *, source_format: str
    ) -> ParseResult:
        stem = Path(ctx.filename).stem or "table"
        table_name = normalize_table_name(stem, ctx.used_table_names)
        ctx.staging_dir.mkdir(parents=True, exist_ok=True)
        out_pq = ctx.staging_dir / f"{table_name}.parquet"

        with tempfile.NamedTemporaryFile(suffix=f".{source_format}", delete=True) as tmp:
            tmp.write(data)
            tmp.flush()
            con = duckdb.connect()
            try:
                src_q = "'" + tmp.name.replace("'", "''") + "'"
                dst_q = "'" + str(out_pq).replace("'", "''") + "'"
                delim_opt = ", delim='\\t'" if source_format == "tsv" else ""
                con.execute(
                    f"COPY (SELECT * FROM read_csv({src_q}, header=true{delim_opt}, "
                    f"ignore_errors=true, store_rejects=true)) "
                    f"TO {dst_q} (FORMAT PARQUET, COMPRESSION ZSTD)"
                )
                rej_row = con.execute("SELECT count(*) FROM reject_errors").fetchone()
                if rej_row and int(rej_row[0]) > 0:
                    ctx.add_warning(
                        "ROW_MALFORMED", f"rejected {int(rej_row[0])} malformed CSV rows"
                    )
            finally:
                con.close()

        sample_df = pl.read_parquet(out_pq, n_rows=100_000)
        total_rows = int(pl.scan_parquet(out_pq).select(pl.len()).collect().item())
        profiles = profile_polars_df(sample_df, timezone_str=ctx.timezone)
        preview_rows = [sample_df.columns, *[list(r) for r in sample_df.head(20).iter_rows()]]
        md_preview = render_markdown_table(preview_rows, max_rows=20)
        rel_path = f"tenants/{ctx.tenant_id}/staging/{ctx.parse_id}/{table_name}.parquet"

        if not ctx.write_staging and out_pq.exists():
            out_pq.unlink(missing_ok=True)

        extracted = ExtractedTable(
            table_name=table_name,
            origin="block=1",
            row_count=total_rows,
            staging_path=rel_path,
            columns=profiles,
            header_rows=1,
        )
        col_summary = ", ".join(f"`{c.name}` ({c.data_type})" for c in profiles)
        md = (
            f"# {stem}\n\n"
            f"- **文件名**：`{ctx.filename}`\n"
            f"- **行数**：{total_rows}\n"
            f"- **列清单**：{col_summary}\n\n"
            f"{md_preview}"
        )
        return ParseResult(
            title=stem,
            source_format=source_format,
            engine="duckdb-copy-csv",
            markdown=md,
            tables=[extracted],
        )
