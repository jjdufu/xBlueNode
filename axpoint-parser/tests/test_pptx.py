from __future__ import annotations

from pathlib import Path

import polars as pl
import pytest

from parsers.engine import parse_unified
from parsers.office_convert import has_soffice
from security import SidecarError

FIXTURES_DIR = Path(__file__).resolve().parent / "fixtures"


def test_briefing_pptx_notes_groups_smartart_and_chart_series(tmp_path: Path) -> None:
    res = parse_unified(
        "briefing.pptx",
        (FIXTURES_DIR / "briefing.pptx").read_bytes(),
        data_root=tmp_path,
    )
    for page_num in range(1, 6):
        assert f"## 第 {page_num} 页" in res.markdown

    assert "备注" in res.markdown
    assert "组合节点 A" in res.markdown
    assert "组合节点 B" in res.markdown
    assert "采购入库" in res.markdown
    assert "出库发运" in res.markdown

    assert len(res.tables) == 2
    chart_tables = [t for t in res.tables if "chart=" in t.origin]
    slide_tables = [t for t in res.tables if "table=" in t.origin]
    assert len(chart_tables) == 1
    assert len(slide_tables) == 1

    chart_df = pl.read_parquet(tmp_path / chart_tables[0].staging_path)
    assert chart_df["category"].to_list() == ["W-1", "W-2", "W-3", "W-4"]
    assert chart_df["计划出库"].to_list() == [1200, 950, 820, 600]
    assert chart_df["实际出库"].to_list() == [1185, 930, 805, 590]


def test_legacy_ppt_without_soffice_returns_422(tmp_path: Path) -> None:
    if has_soffice():
        pytest.skip("soffice installed in full image")
    with pytest.raises(SidecarError) as exc_info:
        parse_unified("legacy.ppt", b"\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1dummy", data_root=tmp_path)
    assert exc_info.value.status_code == 422
    assert exc_info.value.error_code == "UNSUPPORTED_EARLY_OFFICE_FORMAT"
