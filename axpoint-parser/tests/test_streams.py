from __future__ import annotations

from datetime import UTC, datetime
from pathlib import Path

import pytest

from connectors.registry import create_connector, list_connector_specs
from connectors.streams import parse_stream_payload
from connectors.webhook import WebhookConnector, extract_records_by_jsonpath
from lake.writer import sync_dataset
from schemas import ConnectorType, SyncRequest
from security import OutboundBlocked


def test_stream_and_webhook_specs_registered() -> None:
    specs = {s.connector_type: s for s in list_connector_specs()}
    for ctype in ("KAFKA", "MQTT", "PULSAR", "WEBHOOK"):
        assert ctype in specs


def test_parse_stream_payload_flattens_json_and_falls_back_to_raw() -> None:
    now = datetime(2026, 1, 2, 3, 4, 5, tzinfo=UTC)
    rec = parse_stream_payload(b'{"device":"D-1","metrics":{"temp":21.5}}', "orders:0:42", now)
    assert rec["_ax_msg_id"] == "orders:0:42"
    assert rec["device"] == "D-1"
    assert any(k.endswith("temp") and v == 21.5 for k, v in rec.items())

    raw_rec = parse_stream_payload(b"plain-text-frame", "raw:0:1", now)
    assert raw_rec["_ax_msg_id"] == "raw:0:1"
    assert raw_rec["raw"] == "plain-text-frame"


def test_webhook_jsonpath_and_msg_id_deduplication_in_ducklake(tmp_path: Path) -> None:
    extracted = extract_records_by_jsonpath(
        {"data": {"events": [{"id": "e1", "v": 10}, {"id": "e2", "v": 20}]}},
        "$.data.events[*]",
    )
    assert len(extracted) == 2

    conn = WebhookConnector(config={"record_path": "$[*]"}, secrets={})
    tbl = conn.preview(
        {"items": [{"delivery_id": "d-1", "payload": [{"k": 1}, {"k": 2}]}]},
        limit=10,
    )
    assert tbl.num_rows == 2
    assert tbl.column("_ax_msg_id").to_pylist() == ["d-1:0", "d-1:1"]

    dsn = f"ducklake:{tmp_path / 'streams.ducklake'}"
    req = SyncRequest(
        tenant_id="default",
        dataset_id="ds-stream",
        batch_id="b-1",
        connector_type="WEBHOOK",
        config={"record_path": "$[*]"},
        source_ref={
            "items": [
                {"delivery_id": "d-100", "payload": [{"sensor": "S-1", "val": 1.5}]},
                {"delivery_id": "d-101", "payload": [{"sensor": "S-2", "val": 2.5}]},
            ]
        },
        lake_table="stream_events",
        sync_mode="APPEND",
    )
    r1 = sync_dataset(req, data_root=tmp_path, catalog_dsn=dsn)
    assert r1.rows_written == 2

    # Re-delivering d-100 + 1 new delivery d-102 in a subsequent sync only appends d-102
    req2 = req.model_copy(
        update={
            "batch_id": "b-2",
            "source_ref": {
                "items": [
                    {"delivery_id": "d-100", "payload": [{"sensor": "S-1", "val": 1.5}]},
                    {"delivery_id": "d-102", "payload": [{"sensor": "S-3", "val": 3.5}]},
                ]
            },
        }
    )
    r2 = sync_dataset(req2, data_root=tmp_path, catalog_dsn=dsn)
    assert r2.rows_written == 1


@pytest.mark.parametrize(
    ("ctype", "cfg"),
    [
        ("KAFKA", {"bootstrap_servers": "127.0.0.1:9092"}),
        ("MQTT", {"host": "127.0.0.1", "port": 1883}),
        ("PULSAR", {"service_url": "pulsar://127.0.0.1:6650"}),
    ],
)
def test_stream_connectors_block_private_brokers_by_default(
    ctype: ConnectorType, cfg: dict[str, object]
) -> None:
    with pytest.raises(OutboundBlocked):
        create_connector(ctype, dict(cfg), {}, allow_private_network=False)
