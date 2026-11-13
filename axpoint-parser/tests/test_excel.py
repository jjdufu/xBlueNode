from __future__ import annotations

from pathlib import Path

import polars as pl

from parsers.engine import parse_unified

FIXTURES_DIR = Path(__file__).resolve().parent / "fixtures"


def test_supply_chain_xlsx_multi_sheet_merges_and_blocks(tmp_path: Path) -> None:
    res = parse_unified(
        "supply_chain.xlsx",
        (FIXTURES_DIR / "supply_chain.xlsx").read_bytes(),
        data_root=tmp_path,
    )
    assert len(res.tables) == 4
    by_name = {t.table_name: t for t in res.tables}
    assert set(by_name.keys()) == {"gong_chang", "ding_dan", "ku_cun_1", "ku_cun_2"}

    ding_dan = by_name["ding_dan"]
    assert ding_dan.origin.startswith("sheet=订单")
    col_types = {c.name: c.data_type for c in ding_dan.columns}
    assert "数量_计划" in col_types
    assert col_types["数量_实际"] == "INT64"

    # Verify vertical merged cells in 归属_仓库 were filled across all 40 rows
    dd_df = pl.read_parquet(tmp_path / ding_dan.staging_path)
    assert dd_df.height == 40
    assert dd_df["归属_仓库"].null_count() == 0

    # Summary "合计" rows stripped from table track (10 rows each) but preserved in Markdown
    assert by_name["ku_cun_1"].row_count == 10
    assert by_name["ku_cun_2"].row_count == 10
    assert "合计" in res.markdown


def test_formulas_uncached_warning_and_ods(tmp_path: Path) -> None:
    f_res = parse_unified(
        "formulas_uncached.xlsx",
        (FIXTURES_DIR / "formulas_uncached.xlsx").read_bytes(),
        data_root=tmp_path,
    )
    assert len(f_res.tables) == 1
    assert any(w.code == "FORMULA_NOT_CACHED" for w in f_res.warnings)

    ods_res = parse_unified(
        "plan.ods",
        (FIXTURES_DIR / "plan.ods").read_bytes(),
        data_root=tmp_path,
    )
    assert ods_res.source_format == "ods"
    assert len(ods_res.tables) == 1
    assert ods_res.tables[0].row_count == 50
