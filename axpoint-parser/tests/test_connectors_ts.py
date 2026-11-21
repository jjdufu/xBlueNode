from __future__ import annotations

import httpx
import pytest

from connectors.registry import create_connector, list_connector_specs
from connectors.timeseries import TDengineConnector
from schemas import ConnectorType
from security import OutboundBlocked, SidecarError


def test_timeseries_connector_specs_registered() -> None:
    specs = {s.connector_type: s for s in list_connector_specs()}
    for ctype in ("INFLUXDB", "TDENGINE", "IOTDB"):
        assert ctype in specs
        assert specs[ctype].category == "TIMESERIES"


def test_tdengine_rest_discover_preview_and_append_cursor() -> None:
    def mock_handler(request: httpx.Request) -> httpx.Response:
        sql = request.content.decode("utf-8")
        if sql == "SHOW STABLES":
            return httpx.Response(
                200,
                json={"code": 0, "column_meta": [["stable_name", "VARCHAR"]], "data": [["meters"]]},
            )
        if sql == "SHOW TABLES":
            return httpx.Response(200, json={"code": 0, "column_meta": [], "data": []})
        if sql.startswith("DESCRIBE `meters`"):
            return httpx.Response(
                200,
                json={
                    "code": 0,
                    "column_meta": [["Field", "VARCHAR"], ["Type", "VARCHAR"]],
                    "data": [["ts", "TIMESTAMP"], ["current_amp", "DOUBLE"]],
                },
            )
        if "SELECT * FROM `meters`" in sql:
            return httpx.Response(
                200,
                json={
                    "code": 0,
                    "column_meta": [["ts", "TIMESTAMP"], ["current_amp", "DOUBLE"]],
                    "data": [
                        ["2026-01-01T00:00:00Z", 12.5],
                        ["2026-01-01T00:01:00Z", 13.0],
                    ],
                },
            )
        return httpx.Response(200, json={"code": 0, "column_meta": [], "data": [[1]]})

    transport = httpx.MockTransport(mock_handler)
    conn = TDengineConnector(
        config={
            "url": "https://93.184.216.34:6041",
            "database": "power",
            "_transport": transport,
        },
        secrets={"username": "root", "password": "secret"},
    )
    try:
        ok, _ = conn.test()
        assert ok is True
        disc = conn.discover()
        assert len(disc) == 1
        assert disc[0].display_name == "meters"

        batches = list(conn.read({"stable": "meters"}, "APPEND", "ts", "2026-01-01T00:00:00Z"))
        assert sum(b.batch.num_rows for b in batches) == 2
        assert batches[-1].cursor_to == "2026-01-01T00:01:00Z"

        with pytest.raises(SidecarError) as exc_info:
            conn.preview({"query": "DROP TABLE meters"}, limit=5)
        assert exc_info.value.error_code == "SQL_REJECTED"
    finally:
        conn.close()


@pytest.mark.parametrize(
    ("ctype", "cfg"),
    [
        ("INFLUXDB", {"url": "http://127.0.0.1:8086", "org": "acme", "bucket": "iot"}),
        ("TDENGINE", {"url": "http://127.0.0.1:6041", "database": "iot"}),
        ("IOTDB", {"host": "127.0.0.1", "port": 6667}),
    ],
)
def test_timeseries_blocks_private_network_by_default(
    ctype: ConnectorType, cfg: dict[str, object]
) -> None:
    with pytest.raises(OutboundBlocked):
        create_connector(ctype, dict(cfg), {}, allow_private_network=False)


def test_influxdb_v3_test_discover_preview_append_and_sql_guard(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from datetime import UTC, datetime
    from typing import Any

    import pyarrow as pa

    from connectors.timeseries import InfluxDbConnector

    executed: list[tuple[str, dict[str, Any] | None]] = []

    class _FakeInflux3Client:
        def __init__(self, **kwargs: Any) -> None:
            self.kwargs = kwargs

        def query(
            self, sql: str, query_parameters: dict[str, Any] | None = None
        ) -> pa.Table:
            executed.append((sql, query_parameters))
            if sql == "SELECT 1":
                return pa.table({"one": [1]})
            if "information_schema.tables" in sql:
                return pa.table({"table_name": ["meters"]})
            if "information_schema.columns" in sql:
                return pa.table(
                    {
                        "table_name": ["meters", "meters", "meters"],
                        "column_name": ["room", "time", "watts"],
                        "data_type": ["Dictionary(Int32, Utf8)", "Timestamp(ns)", "Float64"],
                        "is_nullable": ["YES", "NO", "YES"],
                    }
                )
            tag_col = pa.DictionaryArray.from_arrays(
                pa.array([0, 1], type=pa.int32()),
                pa.array(["A", "B"], type=pa.string()),
            )
            ts_vals = [
                datetime(2026, 1, 1, 0, 0, 0, tzinfo=UTC),
                datetime(2026, 1, 1, 0, 1, 0, tzinfo=UTC),
            ]
            if query_parameters and "cur" in query_parameters:
                tag_col = tag_col.slice(1)
                ts_vals = ts_vals[1:]
                watts_vals = [135.0]
            else:
                watts_vals = [120.5, 135.0]
            return pa.table(
                {
                    "room": tag_col,
                    "time": pa.array(ts_vals, type=pa.timestamp("ns")),
                    "watts": pa.array(watts_vals, type=pa.float64()),
                }
            )

        def close(self) -> None:
            return None

    import influxdb_client_3 as ic3_mod

    monkeypatch.setattr(ic3_mod, "InfluxDBClient3", _FakeInflux3Client)

    conn = InfluxDbConnector(
        config={
            "url": "https://93.184.216.34:8181",
            "bucket": "demo_iot",
            "org": "default",
            "version": "3",
        },
        secrets={"token": "v3-secret-token"},
    )
    ok, _ = conn.test()
    assert ok is True
    assert executed[-1][0] == "SELECT 1"

    disc = conn.discover()
    assert len(disc) == 1
    assert disc[0].display_name == "meters"
    assert disc[0].suggested_cursor == "ts"
    assert [c.name for c in disc[0].columns] == ["room", "ts", "watts"]

    prev = conn.preview({"measurement": "meters"}, limit=5)
    assert prev.num_rows == 2
    assert "ts" in prev.column_names
    assert pa.types.is_string(prev.schema.field("room").type)

    batches = list(
        conn.read({"measurement": "meters"}, "APPEND", "ts", "2026-01-01T00:00:00Z")
    )
    assert sum(b.batch.num_rows for b in batches) == 1
    assert batches[-1].cursor_to == "2026-01-01T00:01:00Z"
    last_sql, last_params = executed[-1]
    assert "$cur" in last_sql
    assert last_params == {"cur": "2026-01-01T00:00:00Z"}

    with pytest.raises(SidecarError) as exc_ident:
        conn.preview({"measurement": "meters; DROP TABLE meters"}, limit=5)
    assert exc_ident.value.status_code == 400

    with pytest.raises(SidecarError) as exc_sql:
        conn.preview({"sql": "DELETE FROM meters"}, limit=5)
    assert exc_sql.value.error_code == "SQL_REJECTED"
