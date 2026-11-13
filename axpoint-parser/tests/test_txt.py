from __future__ import annotations

from pathlib import Path

from parsers.engine import parse_unified

FIXTURES_DIR = Path(__file__).resolve().parent / "fixtures"


def test_plain_txt_paragraphs_only(tmp_path: Path) -> None:
    res = parse_unified("plain.txt", (FIXTURES_DIR / "plain.txt").read_bytes(), data_root=tmp_path)
    assert res.tables == []
    assert "第1段" in res.markdown
    assert "第30段" in res.markdown


def test_fixed_width_txt(tmp_path: Path) -> None:
    res = parse_unified(
        "fixed_width.txt", (FIXTURES_DIR / "fixed_width.txt").read_bytes(), data_root=tmp_path
    )
    assert len(res.tables) == 1
    tbl = res.tables[0]
    assert tbl.row_count == 200
    assert [c.name for c in tbl.columns] == [
        "CODE",
        "WAREHOUSE",
        "REGION",
        "CAPACITY",
        "STOCK",
        "STATUS",
    ]


def test_key_value_txt(tmp_path: Path) -> None:
    res = parse_unified(
        "key_value.txt", (FIXTURES_DIR / "key_value.txt").read_bytes(), data_root=tmp_path
    )
    assert len(res.tables) == 1
    tbl = res.tables[0]
    assert tbl.row_count == 10
    assert len(tbl.columns) == 5
    assert tbl.columns[0].name == "node_key"


def test_delimited_txt(tmp_path: Path) -> None:
    res = parse_unified(
        "delimited.txt", (FIXTURES_DIR / "delimited.txt").read_bytes(), data_root=tmp_path
    )
    assert len(res.tables) == 1
    tbl = res.tables[0]
    assert tbl.row_count == 300
    assert len(tbl.columns) == 5
