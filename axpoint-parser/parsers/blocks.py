from __future__ import annotations

import math
import re
from collections.abc import Sequence
from dataclasses import dataclass
from typing import Any

from pypinyin import Style, lazy_pinyin

_SUMMARY_LABELS = frozenset({"合计", "总计", "小计", "total", "subtotal", "grand total"})
_NON_IDENT_RE = re.compile(r"[^a-z0-9_]+")
_MULTI_UNDERSCORE_RE = re.compile(r"_{3,}")


@dataclass
class MergeSpan:
    """1-free 0-based inclusive cell merge region (min_row, min_col, max_row, max_col)."""

    min_row: int
    min_col: int
    max_row: int
    max_col: int


@dataclass
class DetectedBlock:
    """Extracted rectangular table block with separated header, data, and summary rows."""

    row_offset: int
    col_offset: int
    headers: list[str]
    source_comments: list[str]
    renamed_columns: list[tuple[str, str]]
    header_rows: int
    data_rows: list[list[Any]]
    markdown_rows: list[list[str]]


def normalize_table_name(raw_name: str, used_names: set[str] | None = None) -> str:
    """Convert raw sheet/file/block name into ^[a-z][a-z0-9_]{0,62}$ using pypinyin.

    Preserves double-underscore '__' child-table separators (e.g. api_orders__lines).
    """
    segments = raw_name.split("__")
    norm_segments: list[str] = []
    for seg in segments:
        tokens: list[str] = []
        buf = ""
        for ch in seg:
            if "\u4e00" <= ch <= "\u9fff":
                if buf:
                    tokens.append(buf)
                    buf = ""
                py = lazy_pinyin(ch, style=Style.NORMAL)
                if py and py[0]:
                    tokens.append(py[0].lower())
            elif ch.isascii() and ch.isalnum():
                buf += ch.lower()
            else:
                if buf:
                    tokens.append(buf)
                    buf = ""
        if buf:
            tokens.append(buf)
        joined = "_".join(t for t in tokens if t)
        joined = _NON_IDENT_RE.sub("_", joined).strip("_")
        norm_segments.append(joined)

    candidate = "__".join(s for s in norm_segments if s)
    candidate = _MULTI_UNDERSCORE_RE.sub("__", candidate).strip("_")
    if not candidate:
        candidate = "table"
    if not ("a" <= candidate[0] <= "z"):
        candidate = f"t_{candidate}"
    candidate = candidate[:63].rstrip("_")
    if not candidate:
        candidate = "t_table"

    if used_names is None:
        return candidate

    if candidate not in used_names:
        used_names.add(candidate)
        return candidate

    suffix_num = 2
    while True:
        sfx = f"_{suffix_num}"
        base = candidate[: 63 - len(sfx)].rstrip("_")
        deduped = f"{base}{sfx}"
        if deduped not in used_names:
            used_names.add(deduped)
            return deduped
        suffix_num += 1


def normalize_column_names(
    raw_headers: Sequence[str],
) -> tuple[list[str], list[str], list[tuple[str, str]]]:
    """Normalize column headers per §01.2.4: preserve original printable chars, fix blanks/dups.

    Returns (normalized_names, source_comments, renamed_pairs).
    """
    names: list[str] = []
    comments: list[str] = []
    renamed: list[tuple[str, str]] = []
    seen: dict[str, int] = {}

    for idx, raw in enumerate(raw_headers, start=1):
        orig = (raw or "").strip()
        cleaned = re.sub(r"[\x00-\x1f\x7f]+", " ", orig).strip()
        cleaned = cleaned.replace('"', "").strip()

        if not cleaned:
            final = f"col_{idx}"
        else:
            final = cleaned[:128]

        if final in seen:
            seen[final] += 1
            final = f"{final}_{seen[final]}"
        else:
            seen[final] = 1

        names.append(final)
        if orig and orig != final:
            comments.append(orig)
            renamed.append((orig, final))
        elif not orig:
            comments.append("")
            renamed.append(("", final))
        else:
            comments.append("")

    return names, comments, renamed


def _is_cell_empty(val: Any) -> bool:
    if val is None:
        return True
    if isinstance(val, str) and val.strip() == "":
        return True
    if isinstance(val, float) and math.isnan(val):
        return True
    return False


def _is_header_like_row(row: Sequence[Any]) -> bool:
    """Rule 1: non-empty ratio >= 60% AND string cell ratio among non-empty >= 80%."""
    n_cols = len(row)
    if n_cols == 0:
        return False
    non_empty = [c for c in row if not _is_cell_empty(c)]
    if len(non_empty) / n_cols < 0.60:
        return False
    str_cells = 0
    for c in non_empty:
        if isinstance(c, str):
            txt = c.strip()
            # Pure numeric strings don't count as descriptive header labels
            if not re.fullmatch(r"[+-]?\d+(?:\.\d+)?%?", txt):
                str_cells += 1
    return (str_cells / len(non_empty)) >= 0.80


def split_sheet_into_raw_regions(
    grid: Sequence[Sequence[Any]],
) -> list[tuple[int, int, int, int]]:
    """Split a 2D sheet grid into (r0, c0, r1, c1) blocks separated by >=2 empty rows/cols."""
    n_rows = len(grid)
    if n_rows == 0:
        return []
    n_cols = max((len(r) for r in grid), default=0)
    if n_cols == 0:
        return []

    def _split_by_empty_rows(
        r_start: int, r_end: int, c_start: int, c_end: int
    ) -> list[tuple[int, int]]:
        row_non_empty = [
            any(
                c_idx < len(grid[r_idx]) and not _is_cell_empty(grid[r_idx][c_idx])
                for c_idx in range(c_start, c_end)
            )
            for r_idx in range(r_start, r_end)
        ]
        bands: list[tuple[int, int]] = []
        in_band = False
        band_start = r_start
        empty_streak = 0
        for offset, has_val in enumerate(row_non_empty):
            abs_r = r_start + offset
            if has_val:
                if not in_band:
                    band_start = abs_r
                    in_band = True
                empty_streak = 0
            elif in_band:
                empty_streak += 1
                if empty_streak >= 2:
                    bands.append((band_start, abs_r - empty_streak + 1))
                    in_band = False
                    empty_streak = 0
        if in_band:
            bands.append((band_start, r_end - empty_streak))
        return bands

    def _split_by_empty_cols(
        r_start: int, r_end: int, c_start: int, c_end: int
    ) -> list[tuple[int, int]]:
        col_non_empty = [
            any(
                c_idx < len(grid[r_idx]) and not _is_cell_empty(grid[r_idx][c_idx])
                for r_idx in range(r_start, r_end)
            )
            for c_idx in range(c_start, c_end)
        ]
        bands: list[tuple[int, int]] = []
        in_band = False
        band_start = c_start
        empty_streak = 0
        for offset, has_val in enumerate(col_non_empty):
            abs_c = c_start + offset
            if has_val:
                if not in_band:
                    band_start = abs_c
                    in_band = True
                empty_streak = 0
            elif in_band:
                empty_streak += 1
                if empty_streak >= 2:
                    bands.append((band_start, abs_c - empty_streak + 1))
                    in_band = False
                    empty_streak = 0
        if in_band:
            bands.append((band_start, c_end - empty_streak))
        return bands

    regions: list[tuple[int, int, int, int]] = []
    for rb0, rb1 in _split_by_empty_rows(0, n_rows, 0, n_cols):
        for cb0, cb1 in _split_by_empty_cols(rb0, rb1, 0, n_cols):
            # Refine row bounds inside this column slice
            for sub_r0, sub_r1 in _split_by_empty_rows(rb0, rb1, cb0, cb1):
                regions.append((sub_r0, cb0, sub_r1, cb1))
    return regions


def extract_blocks_from_grid(
    grid: Sequence[Sequence[Any]],
    merges: Sequence[MergeSpan] = (),
) -> list[DetectedBlock]:
    """Detect rectangular table blocks, resolve multi-level headers & merges, and strip totals."""
    if not grid:
        return []

    # Build mutable rectangular copy so merge filling does not mutate caller
    max_cols = max((len(r) for r in grid), default=0)
    if max_cols == 0:
        return []
    work: list[list[Any]] = [[r[c] if c < len(r) else None for c in range(max_cols)] for r in grid]

    # Pre-populate merged top-left presence so region splitting respects merged spans
    regions = split_sheet_into_raw_regions(work)
    results: list[DetectedBlock] = []

    for r0, c0, r1, c1 in regions:
        sub_h = r1 - r0
        sub_w = c1 - c0
        if sub_h < 2 or sub_w < 1:
            continue

        block_merges = [
            MergeSpan(
                min_row=m.min_row - r0,
                min_col=m.min_col - c0,
                max_row=m.max_row - r0,
                max_col=m.max_col - c0,
            )
            for m in merges
            if m.min_row >= r0 and m.max_row < r1 and m.min_col >= c0 and m.max_col < c1
        ]

        sub_grid: list[list[Any]] = [[work[r][c] for c in range(c0, c1)] for r in range(r0, r1)]

        # Horizontal fill on rows that have horizontal merges so header detection sees full width
        horiz_merge_rows: set[int] = set()
        for m in block_merges:
            if m.max_col > m.min_col:
                for mr in range(m.min_row, m.max_row + 1):
                    horiz_merge_rows.add(mr)
                    val = sub_grid[m.min_row][m.min_col]
                    for mc in range(m.min_col, m.max_col + 1):
                        if _is_cell_empty(sub_grid[mr][mc]):
                            sub_grid[mr][mc] = val

        # Rule 1: find header start in first 20 rows
        hdr_start = 0
        for cand_r in range(min(20, sub_h)):
            if _is_header_like_row(sub_grid[cand_r]):
                hdr_start = cand_r
                break

        # Rule 2: multi-level headers (up to 3 rows when upper row has horizontal merge)
        hdr_count = 1
        while (
            hdr_count < 3
            and (hdr_start + hdr_count) < sub_h - 1
            and (hdr_start + hdr_count - 1) in horiz_merge_rows
            and _is_header_like_row(sub_grid[hdr_start + hdr_count])
        ):
            hdr_count += 1

        # Build combined raw headers
        raw_headers: list[str] = []
        for col_idx in range(sub_w):
            parts: list[str] = []
            for hr in range(hdr_start, hdr_start + hdr_count):
                cell_v = sub_grid[hr][col_idx]
                txt = "" if _is_cell_empty(cell_v) else str(cell_v).strip()
                if txt and (not parts or parts[-1] != txt):
                    parts.append(txt)
            raw_headers.append("_".join(parts))

        headers, comments, renamed = normalize_column_names(raw_headers)

        # Rule 3: vertical merge fill in data region
        data_start = hdr_start + hdr_count
        for m in block_merges:
            if m.max_row > m.min_row and m.max_row >= data_start:
                top_val = sub_grid[m.min_row][m.min_col]
                for mr in range(max(m.min_row, data_start), m.max_row + 1):
                    for mc in range(m.min_col, m.max_col + 1):
                        if _is_cell_empty(sub_grid[mr][mc]):
                            sub_grid[mr][mc] = top_val

        all_body_rows = [
            row for row in sub_grid[data_start:] if any(not _is_cell_empty(c) for c in row)
        ]

        # Rule 5: strip trailing summary/total rows from table track, keep in markdown track
        md_rows: list[list[str]] = [headers]
        for row in all_body_rows:
            md_rows.append([_cell_to_md(c) for c in row])

        clean_data_rows = list(all_body_rows)
        while clean_data_rows:
            last_row = clean_data_rows[-1]
            first_cell = ""
            for c in last_row:
                if not _is_cell_empty(c):
                    first_cell = str(c).strip().lower()
                    break
            if first_cell in _SUMMARY_LABELS:
                clean_data_rows.pop()
            else:
                break

        results.append(
            DetectedBlock(
                row_offset=r0,
                col_offset=c0,
                headers=headers,
                source_comments=comments,
                renamed_columns=renamed,
                header_rows=hdr_count,
                data_rows=clean_data_rows,
                markdown_rows=md_rows,
            )
        )

    return results


def _cell_to_md(val: Any) -> str:
    if _is_cell_empty(val):
        return ""
    if isinstance(val, float) and math.isfinite(val) and val.is_integer():
        s = str(int(val))
    else:
        s = str(val)
    return s.replace("\r", " ").replace("\n", " ").replace("|", "\\|").strip()


def render_markdown_table(rows: Sequence[Sequence[Any]], max_rows: int = 50) -> str:
    """Render rows (row 0 = header) into a GitHub-flavored Markdown table."""
    if not rows:
        return ""
    header = [_cell_to_md(c) for c in rows[0]]
    n_cols = len(header)
    if n_cols == 0:
        return ""
    sep = ["---"] * n_cols
    out = [
        "| " + " | ".join(header) + " |",
        "| " + " | ".join(sep) + " |",
    ]
    body = rows[1 : 1 + max_rows]
    for r in body:
        padded = [_cell_to_md(r[i]) if i < len(r) else "" for i in range(n_cols)]
        out.append("| " + " | ".join(padded) + " |")
    return "\n".join(out)
