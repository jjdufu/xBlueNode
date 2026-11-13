from __future__ import annotations

from collections.abc import Sequence
from typing import Any

import polars as pl

from parsers.base import ParseContext
from parsers.blocks import normalize_column_names, normalize_table_name
from parsers.profiler import infer_and_convert_column, profile_polars_df
from schemas import ColumnProfile, DataType, ExtractedTable


def write_rows_to_staging(
    ctx: ParseContext,
    raw_table_name: str,
    headers: Sequence[str],
    rows: Sequence[Sequence[Any]],
    *,
    origin: str,
    header_rows: int = 1,
    source_comments: Sequence[str] | None = None,
    already_normalized_cols: bool = False,
) -> ExtractedTable:
    """Profile row-major tabular data, write zstd Parquet to staging, and return ExtractedTable."""
    table_name = normalize_table_name(raw_table_name, ctx.used_table_names)

    if already_normalized_cols:
        col_names = list(headers)
        comments = list(source_comments) if source_comments is not None else [""] * len(col_names)
    else:
        col_names, comments, renamed = normalize_column_names(headers)
        for orig, new in renamed:
            ctx.add_warning(
                "COLUMN_RENAMED",
                f"column {orig!r} normalized to {new!r} in table {table_name}",
            )

    n_cols = len(col_names)
    series_list: list[pl.Series] = []
    profiles: list[ColumnProfile] = []
    total_malformed = 0

    for c_idx in range(n_cols):
        col_vals = [r[c_idx] if c_idx < len(r) else None for r in rows]
        c_comment = comments[c_idx] if c_idx < len(comments) else ""
        s, prof, malformed = infer_and_convert_column(
            col_names[c_idx],
            c_idx,
            col_vals,
            timezone_str=ctx.timezone,
            source_comment=c_comment,
        )
        series_list.append(s)
        profiles.append(prof)
        total_malformed += malformed

    if total_malformed > 0:
        ctx.add_warning(
            "ROW_MALFORMED",
            f"{total_malformed} cell values coerced to NULL during type inference in {table_name}",
        )

    df = pl.DataFrame(series_list) if series_list else pl.DataFrame()
    rel_staging_path = f"tenants/{ctx.tenant_id}/staging/{ctx.parse_id}/{table_name}.parquet"

    if ctx.write_staging:
        ctx.staging_dir.mkdir(parents=True, exist_ok=True)
        out_file = ctx.staging_dir / f"{table_name}.parquet"
        df.write_parquet(out_file, compression="zstd", row_group_size=131_072)

    return ExtractedTable(
        table_name=table_name,
        origin=origin,
        row_count=len(rows),
        staging_path=rel_staging_path,
        columns=profiles,
        header_rows=header_rows,
    )


def write_polars_to_staging(
    ctx: ParseContext,
    raw_table_name: str,
    df: pl.DataFrame,
    *,
    origin: str,
    header_rows: int = 1,
    type_overrides: dict[str, DataType] | None = None,
) -> ExtractedTable:
    """Write an already-typed Polars DataFrame to staging Parquet and build ExtractedTable."""
    table_name = normalize_table_name(raw_table_name, ctx.used_table_names)
    profiles = profile_polars_df(df, timezone_str=ctx.timezone, type_overrides=type_overrides)
    rel_staging_path = f"tenants/{ctx.tenant_id}/staging/{ctx.parse_id}/{table_name}.parquet"

    if ctx.write_staging:
        ctx.staging_dir.mkdir(parents=True, exist_ok=True)
        out_file = ctx.staging_dir / f"{table_name}.parquet"
        df.write_parquet(out_file, compression="zstd", row_group_size=131_072)

    return ExtractedTable(
        table_name=table_name,
        origin=origin,
        row_count=df.height,
        staging_path=rel_staging_path,
        columns=profiles,
        header_rows=header_rows,
    )
