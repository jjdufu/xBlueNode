from __future__ import annotations

from pathlib import Path

from parsers.engine import parse_unified

FIXTURES_DIR = Path(__file__).resolve().parent / "fixtures"


def test_orders_utf8_and_gbk_profiles_match(tmp_path: Path) -> None:
    utf8_res = parse_unified(
        "orders.csv", (FIXTURES_DIR / "orders.csv").read_bytes(), data_root=tmp_path
    )
    gbk_res = parse_unified(
        "orders_gbk.csv", (FIXTURES_DIR / "orders_gbk.csv").read_bytes(), data_root=tmp_path
    )

    assert len(utf8_res.tables) == 1
    assert len(gbk_res.tables) == 1
    t_utf8 = utf8_res.tables[0]
    t_gbk = gbk_res.tables[0]

    assert t_utf8.row_count == 2000
    assert t_gbk.row_count == 2000
    assert any(w.code == "ENCODING_FALLBACK" for w in gbk_res.warnings)

    utf8_cols = {c.name: (c.data_type, c.pii_category) for c in t_utf8.columns}
    gbk_cols = {c.name: (c.data_type, c.pii_category) for c in t_gbk.columns}
    assert utf8_cols == gbk_cols
    assert utf8_cols["amount"][0] == "DECIMAL"
    assert utf8_cols["order_date"][0] == "DATE"
    assert utf8_cols["phone"][1] == "PHONE"
    assert t_utf8.staging_path.startswith("tenants/default/staging/")
    assert (tmp_path / t_utf8.staging_path).is_file()


def test_sensors_tsv_and_broken_csv(tmp_path: Path) -> None:
    tsv_res = parse_unified(
        "sensors.tsv", (FIXTURES_DIR / "sensors.tsv").read_bytes(), data_root=tmp_path
    )
    assert tsv_res.source_format == "tsv"
    assert tsv_res.tables[0].row_count == 500

    broken_res = parse_unified(
        "broken.csv", (FIXTURES_DIR / "broken.csv").read_bytes(), data_root=tmp_path
    )
    assert broken_res.tables[0].row_count == 17
    assert any(w.code == "ROW_MALFORMED" for w in broken_res.warnings)
