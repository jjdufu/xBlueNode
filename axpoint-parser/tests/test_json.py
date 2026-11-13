from __future__ import annotations

from pathlib import Path

import orjson

from parsers.engine import parse_unified

FIXTURES_DIR = Path(__file__).resolve().parent / "fixtures"


def test_api_orders_json_explode_modes_and_schema_archive(tmp_path: Path) -> None:
    raw = (FIXTURES_DIR / "api_orders.json").read_bytes()

    # AUTO mode: main table + exploded child table with _parent_row
    res_auto = parse_unified("api_orders.json", raw, data_root=tmp_path)
    by_name = {t.table_name: t for t in res_auto.tables}
    assert "api_orders" in by_name
    assert "api_orders__lines" in by_name

    main_tbl = by_name["api_orders"]
    child_tbl = by_name["api_orders__lines"]
    assert main_tbl.row_count == 200
    assert "customer.name" in {c.name for c in main_tbl.columns}
    assert "_parent_row" in {c.name for c in child_tbl.columns}

    # Verify genson .schema.json archive was created alongside staging parquet
    schema_path = (
        tmp_path / "tenants" / "default" / "staging" / res_auto.parse_id / "api_orders.schema.json"
    )
    assert schema_path.is_file()
    schema_obj = orjson.loads(schema_path.read_bytes())
    assert schema_obj["type"] == "object"

    # NONE mode: single table with lines kept as JSON column
    res_none = parse_unified(
        "api_orders.json", raw, options={"json_explode": "NONE"}, data_root=tmp_path
    )
    assert len(res_none.tables) == 1
    col_map = {c.name: c.data_type for c in res_none.tables[0].columns}
    assert col_map["lines"] == "JSON"

    # ALL mode: Cartesian expansion into main table
    res_all = parse_unified(
        "api_orders.json", raw, options={"json_explode": "ALL"}, data_root=tmp_path
    )
    assert len(res_all.tables) == 1
    assert res_all.tables[0].row_count == child_tbl.row_count


def test_events_jsonl_5000_rows(tmp_path: Path) -> None:
    res = parse_unified(
        "events.jsonl", (FIXTURES_DIR / "events.jsonl").read_bytes(), data_root=tmp_path
    )
    assert res.source_format == "jsonl"
    assert len(res.tables) == 1
    assert res.tables[0].row_count == 5000
