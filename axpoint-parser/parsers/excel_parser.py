from __future__ import annotations

import io
import math
from pathlib import Path
from typing import Any

import openpyxl
from python_calamine import CalamineWorkbook

from parsers.base import ParseContext, ParseResult
from parsers.blocks import (
    MergeSpan,
    extract_blocks_from_grid,
    render_markdown_table,
)
from parsers.office_convert import recalculate_xlsx
from parsers.staging import write_rows_to_staging
from schemas import ExtractedTable
from security import SidecarError, check_zip


def _normalize_calamine_cell(val: Any) -> Any:
    """Convert whole-number floats returned by calamine into exact Python ints."""
    if isinstance(val, bool):
        return val
    if isinstance(val, float):
        if math.isnan(val) or math.isinf(val):
            return None
        if val.is_integer() and -(1 << 53) <= val <= (1 << 53):
            return int(val)
        return val
    if isinstance(val, str):
        s = val.strip()
        return s if s != "" else None
    return val


def _inspect_ooxml_metadata(
    data: bytes,
) -> tuple[dict[str, list[MergeSpan]], set[str], list[tuple[str, int, int]]]:
    """Use openpyxl on .xlsx/.xlsm to read merge spans, hidden sheets, and formula locations."""
    merges_by_sheet: dict[str, list[MergeSpan]] = {}
    hidden_sheets: set[str] = set()
    formula_cells: list[tuple[str, int, int]] = []

    wb = openpyxl.load_workbook(io.BytesIO(data), read_only=False, data_only=False)
    try:
        for sheet_name in wb.sheetnames:
            ws = wb[sheet_name]
            if ws.sheet_state != "visible":
                hidden_sheets.add(sheet_name)

            spans: list[MergeSpan] = []
            for mr in ws.merged_cells.ranges:
                spans.append(
                    MergeSpan(
                        min_row=mr.min_row - 1,
                        min_col=mr.min_col - 1,
                        max_row=mr.max_row - 1,
                        max_col=mr.max_col - 1,
                    )
                )
            merges_by_sheet[sheet_name] = spans

            for r_idx, row in enumerate(ws.iter_rows(values_only=True)):
                for c_idx, cell_val in enumerate(row):
                    if isinstance(cell_val, str) and cell_val.startswith("="):
                        formula_cells.append((sheet_name, r_idx, c_idx))
    finally:
        wb.close()

    return merges_by_sheet, hidden_sheets, formula_cells


class ExcelParser:
    """Parser for .xlsx, .xlsm, .xls, .xlsb, and .ods spreadsheets (Step 01 §01.5)."""

    suffixes: tuple[str, ...] = (".xlsx", ".xlsm", ".xls", ".xlsb", ".ods")

    def parse(self, data: bytes, ctx: ParseContext) -> ParseResult:
        suffix = Path(ctx.filename).suffix.lower()
        stem = Path(ctx.filename).stem or "workbook"
        source_format = suffix.lstrip(".") or "xlsx"

        if data.startswith(b"PK\x03\x04"):
            check_zip(data)

        is_ooxml = suffix in {".xlsx", ".xlsm"} and data.startswith(b"PK\x03\x04")
        merges_by_sheet: dict[str, list[MergeSpan]] = {}
        hidden_sheets: set[str] = set()
        formula_cells: list[tuple[str, int, int]] = []

        if is_ooxml:
            try:
                merges_by_sheet, hidden_sheets, formula_cells = _inspect_ooxml_metadata(data)
            except Exception as exc:
                raise SidecarError(
                    422, "PARSE_MALFORMED", f"failed to inspect OOXML workbook: {exc}"
                ) from exc

        grids = self._read_calamine_grids(data)

        # Check whether any formula cell has an uncached empty value
        if formula_cells:
            uncached = self._count_uncached_formulas(grids, formula_cells)
            if uncached > 0:
                recalc = recalculate_xlsx(data)
                if recalc is not None:
                    grids = self._read_calamine_grids(recalc)
                    uncached = self._count_uncached_formulas(grids, formula_cells)
                if uncached > 0:
                    ctx.add_warning(
                        "FORMULA_NOT_CACHED",
                        f"{uncached} formula cells have no cached value in {ctx.filename}",
                    )

        md_sections: list[str] = [f"# {stem}"]
        tables: list[ExtractedTable] = []

        for sheet_name, grid in grids.items():
            merges = merges_by_sheet.get(sheet_name, [])
            blocks = extract_blocks_from_grid(grid, merges)
            if not blocks:
                continue

            md_sections.append(f"## {sheet_name}")
            multi_block = len(blocks) > 1
            is_hidden = sheet_name in hidden_sheets

            for b_idx, blk in enumerate(blocks, start=1):
                for orig, new in blk.renamed_columns:
                    ctx.add_warning(
                        "COLUMN_RENAMED",
                        f"sheet {sheet_name!r} column {orig!r} normalized to {new!r}",
                    )
                raw_tbl_name = f"{sheet_name}_{b_idx}" if multi_block else sheet_name
                origin = f"sheet={sheet_name};block={b_idx}"
                if is_hidden:
                    origin += ";hidden=true"

                extracted = write_rows_to_staging(
                    ctx,
                    raw_tbl_name,
                    blk.headers,
                    blk.data_rows,
                    origin=origin,
                    header_rows=blk.header_rows,
                    source_comments=blk.source_comments,
                    already_normalized_cols=True,
                )
                tables.append(extracted)
                md_sections.append(render_markdown_table(blk.markdown_rows, max_rows=50))

        engine = "calamine+openpyxl" if is_ooxml else "calamine"
        return ParseResult(
            title=stem,
            source_format=source_format,
            engine=engine,
            markdown="\n\n".join(md_sections),
            tables=tables,
        )

    def _read_calamine_grids(self, data: bytes) -> dict[str, list[list[Any]]]:
        try:
            wb = CalamineWorkbook.from_filelike(io.BytesIO(data))
        except Exception as exc:
            raise SidecarError(
                422, "PARSE_MALFORMED", f"failed to read spreadsheet via calamine: {exc}"
            ) from exc

        out: dict[str, list[list[Any]]] = {}
        for sheet_name in wb.sheet_names:
            raw_rows = wb.get_sheet_by_name(sheet_name).to_python()
            out[sheet_name] = [[_normalize_calamine_cell(c) for c in row] for row in raw_rows]
        return out

    @staticmethod
    def _count_uncached_formulas(
        grids: dict[str, list[list[Any]]],
        formula_cells: list[tuple[str, int, int]],
    ) -> int:
        missing = 0
        for sheet_name, r_idx, c_idx in formula_cells:
            sheet_grid = grids.get(sheet_name)
            if not sheet_grid or r_idx >= len(sheet_grid):
                missing += 1
                continue
            row = sheet_grid[r_idx]
            if c_idx >= len(row) or row[c_idx] is None:
                missing += 1
        return missing
