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
