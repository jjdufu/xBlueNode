from __future__ import annotations

from pathlib import Path

import polars as pl
import pytest

from parsers.engine import parse_unified
from parsers.office_convert import has_soffice
from security import SidecarError

FIXTURES_DIR = Path(__file__).resolve().parent / "fixtures"


def test_contract_docx_headings_merged_and_nested_tables(tmp_path: Path) -> None:
    res = parse_unified(
        "contract.docx",
        (FIXTURES_DIR / "contract.docx").read_bytes(),
        data_root=tmp_path,
    )
    assert "# 供应链仓储与干线履约服务合同" in res.markdown
    assert "## 第一章 服务范围与仓库节点" in res.markdown
    assert "### 1.1 计费标准与 SLA 阶梯附表" in res.markdown
    assert "[嵌套表 1]" in res.markdown

    # 2x2 signature table (< 3 rows) stays in Markdown only, not in tables
    assert "委托方签章" in res.markdown
    assert len(res.tables) == 2

    parent_tbl, nested_tbl = res.tables[0], res.tables[1]
    assert parent_tbl.origin == "table=1"
    assert nested_tbl.origin == "table=1;nested=1"
    assert parent_tbl.row_count == 3
    assert nested_tbl.row_count == 3

    p_df = pl.read_parquet(tmp_path / parent_tbl.staging_path)
    assert p_df["区域_大区"].to_list() == ["华东", "华东", "华南"]


def test_legacy_doc_without_soffice_returns_422(tmp_path: Path) -> None:
    if has_soffice():
        pytest.skip("soffice installed in full image")
    with pytest.raises(SidecarError) as exc_info:
        parse_unified("legacy.doc", b"\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1dummy", data_root=tmp_path)
    assert exc_info.value.status_code == 422
    assert exc_info.value.error_code == "UNSUPPORTED_EARLY_OFFICE_FORMAT"
