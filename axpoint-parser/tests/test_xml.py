from __future__ import annotations

from pathlib import Path

from parsers.engine import parse_unified

FIXTURES_DIR = Path(__file__).resolve().parent / "fixtures"


def test_catalog_xml_attributes_nested_elements_and_namespaces(tmp_path: Path) -> None:
    raw = (FIXTURES_DIR / "catalog.xml").read_bytes()
    res = parse_unified("catalog.xml", raw, data_root=tmp_path)

    assert len(res.tables) == 1
    tbl = res.tables[0]
    assert tbl.row_count == 100
    assert tbl.origin == "record_tag=item"

    col_names = {c.name for c in tbl.columns}
    assert "@id" in col_names
    assert "price.amount" in col_names
    assert "price.@currency" in col_names
    assert "命名空间映射" in res.markdown
    assert "urn:axpoint:catalog:v1" in res.markdown


def test_xml_explicit_record_tag_option(tmp_path: Path) -> None:
    xml_bytes = (
        b'<?xml version="1.0" encoding="UTF-8"?>'
        b"<batch><header><id>B1</id></header>"
        b'<entry code="E1"><qty>10</qty></entry>'
        b'<entry code="E2"><qty>20</qty></entry></batch>'
    )
    res = parse_unified(
        "custom.xml",
        xml_bytes,
        options={"record_tag": "entry"},
        data_root=tmp_path,
    )
    assert len(res.tables) == 1
    assert res.tables[0].row_count == 2
    assert "@code" in {c.name for c in res.tables[0].columns}
