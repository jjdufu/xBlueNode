from __future__ import annotations

import asyncio
import struct
from collections.abc import AsyncIterator
from pathlib import Path
from typing import Any

import pyarrow as pa
import pytest

from connectors.cdc_mysql import (
    MYSQL_DELETE_ROWS_EVENT_V2,
    MYSQL_TABLE_MAP_EVENT,
    MYSQL_UPDATE_ROWS_EVENT_V2,
    MYSQL_WRITE_ROWS_EVENT_V2,
    decode_mysql_binlog_v4_packets,
    parse_mysql_cdc_records,
)
from connectors.cdc_pg import (
    CdcChangeEvent,
    compute_pk_digest_diff,
    decode_pgoutput_messages,
    parse_test_decoding_line,
    parse_wal2json_payload,
    validate_cdc_ident,
)
from connectors.registry import list_connector_specs
from connectors.stream_worker import AdaptiveStreamBuffer, run_stream_microbatch_loop
from lake.writer import apply_cdc_events, preview_lake
from schemas import LakePreviewRequest
from security import SidecarError


def test_postgres_and_mysql_advertise_log_cdc_mode() -> None:
    specs = {s.connector_type: s for s in list_connector_specs()}
    assert "LOG_CDC" in specs["POSTGRES"].supports_modes
    assert "LOG_CDC" in specs["MYSQL"].supports_modes


def test_adaptive_stream_buffer_flushes_at_2000_rows_and_1000ms_window() -> None:
    buf = AdaptiveStreamBuffer(max_batch_rows=2_000, flush_window_ms=1_000)
    t0 = 100.0

    for idx in range(1_999):
        ready = buf.push(
            {"seq": idx, "_ax_msg_id": f"m-{idx}"},
            {"0": str(idx + 1)},
            now_monotonic=t0,
        )
        assert ready is False

    # 2,000th message immediately triggers row_threshold flush
    assert (
        buf.push({"seq": 1_999, "_ax_msg_id": "m-1999"}, {"0": "2000"}, now_monotonic=t0) is True
    )
    flushed = buf.drain(now_monotonic=t0)
    assert flushed is not None
    assert len(flushed.records) == 2_000
    assert flushed.flushed_reason == "row_threshold"
    assert flushed.offsets == {"0": "2000"}

    # Time window trigger at >= 1,000ms with partial batch
    pushed_2k = buf.push(
        {"seq": 2_000, "_ax_msg_id": "m-2000"},
        {"0": "2001"},
        now_monotonic=t0 + 0.1,
    )
    assert pushed_2k is False
    assert buf.should_flush(now_monotonic=t0 + 0.8) is False
    assert buf.should_flush(now_monotonic=t0 + 1.15) is True
    win_batch = buf.drain(now_monotonic=t0 + 1.15)
    assert win_batch is not None
    assert len(win_batch.records) == 1
    assert win_batch.flushed_reason == "window_elapsed"
    assert win_batch.offsets == {"0": "2001"}


def test_async_stream_microbatch_loop_flushes_arrow_batches() -> None:
    flushed_counts: list[tuple[int, dict[str, str], str]] = []

    async def _fake_stream() -> AsyncIterator[tuple[dict[str, Any], dict[str, str]]]:
        for idx in range(5):
            yield ({"id": idx, "temp": 20.0 + idx}, {"0": str(idx + 1)})

    async def _on_flush(tbl: pa.Table, offsets: dict[str, str], reason: str) -> None:
        flushed_counts.append((tbl.num_rows, dict(offsets), reason))

    final_offsets = asyncio.run(
        run_stream_microbatch_loop(
            _fake_stream(),
            _on_flush,
            max_batch_rows=2,
            flush_window_ms=500,
        )
    )
    assert final_offsets == {"0": "5"}
    assert [c[0] for c in flushed_counts] == [2, 2, 1]
    assert flushed_counts[0][2] == "row_threshold"


def test_pg_test_decoding_wal2json_and_pgoutput_parsers() -> None:
    ins = parse_test_decoding_line(
        "table public.orders: INSERT: order_id[integer]:101 "
        "status[text]:'PAID' amount[numeric]:88.5",
        ["order_id"],
        "public",
        "orders",
        lsn="0/16B3748",
    )
    assert ins == CdcChangeEvent(
        op="I",
        pk={"order_id": 101},
        row_after={"order_id": 101, "status": "PAID", "amount": 88.5},
        lsn="0/16B3748",
    )

    upd = parse_test_decoding_line(
        "table public.orders: UPDATE: old-key: order_id[integer]:101 "
        "new-tuple: order_id[integer]:101 status[text]:'SHIPPED' amount[numeric]:95.0",
        ["order_id"],
        "public",
        "orders",
    )
    assert upd is not None and upd.op == "U" and upd.row_after is not None
    assert upd.row_after["status"] == "SHIPPED"

    dele = parse_test_decoding_line(
        "table public.orders: DELETE: order_id[integer]:101",
        ["order_id"],
        "public",
        "orders",
    )
    assert dele == CdcChangeEvent(op="D", pk={"order_id": 101}, row_after=None)

    w2j = parse_wal2json_payload(
        '{"change":['
        '{"kind":"insert","schema":"public","table":"orders",'
        '"columnnames":["order_id","amount"],"columnvalues":[1,50.0]},'
        '{"kind":"delete","schema":"public","table":"orders",'
        '"oldkeys":{"keynames":["order_id"],"keyvalues":[2]}}'
        "]}",
        ["order_id"],
        "public",
        "orders",
        lsn="0/16B3900",
    )
    assert len(w2j) == 2
    assert w2j[0].op == "I" and w2j[0].pk == {"order_id": 1}
    assert w2j[1].op == "D" and w2j[1].pk == {"order_id": 2}

    # Build binary pgoutput Relation ('R') + Insert ('I') + Delete ('D') frames
    rel_frame = (
        b"R"
        + struct.pack(">I", 42)
        + b"public\x00orders\x00d"
        + struct.pack(">H", 2)
        + b"\x01id\x00"
        + struct.pack(">II", 23, 0)
        + b"\x00title\x00"
        + struct.pack(">II", 25, 0)
    )
    ins_frame = (
        b"I"
        + struct.pack(">I", 42)
        + b"N"
        + struct.pack(">H", 2)
        + b"t"
        + struct.pack(">I", 2)
        + b"10"
        + b"t"
        + struct.pack(">I", 4)
        + b"item"
    )
    del_frame = (
        b"D"
        + struct.pack(">I", 42)
        + b"K"
        + struct.pack(">H", 2)
        + b"t"
        + struct.pack(">I", 2)
        + b"10"
        + b"n"
    )
    pgout_events = decode_pgoutput_messages(
        [rel_frame, ins_frame, del_frame], ["id"], "public", "orders"
    )
    assert len(pgout_events) == 2
    assert pgout_events[0].op == "I" and pgout_events[0].row_after == {"id": 10, "title": "item"}
    assert pgout_events[1].op == "D" and pgout_events[1].pk == {"id": 10}


def _make_v4_pkt(event_type: int, next_pos: int, body: bytes) -> bytes:
    header = struct.pack("<IBIIIH", 1767225600, event_type, 1, 19 + len(body), next_pos, 0)
    return header + body


def _encode_compact_row(id_val: int, name_val: str) -> bytes:
    encoded_name = name_val.encode("utf-8")
    return (
        b"\x00"  # null bitmap (0 nulls)
        + b"\x01"
        + struct.pack("<q", id_val)
        + b"\x03"
        + struct.pack("<H", len(encoded_name))
        + encoded_name
    )


def test_mysql_binlog_packets_and_record_parsers() -> None:
    tmap_body = (
        (77).to_bytes(6, "little")
        + struct.pack("<H", 0)
        + bytes([3])
        + b"erp\x00"
        + bytes([6])
        + b"orders\x00"
    )
    row_hdr = (77).to_bytes(6, "little") + struct.pack("<HH", 0, 2) + bytes([2, 0x03])
    write_body = row_hdr + _encode_compact_row(1, "Alpha") + _encode_compact_row(2, "Beta")
    upd_hdr = (77).to_bytes(6, "little") + struct.pack("<HH", 0, 2) + bytes([2, 0x03, 0x03])
    upd_body = upd_hdr + _encode_compact_row(2, "Beta") + _encode_compact_row(2, "Beta-V2")
    del_body = row_hdr + _encode_compact_row(1, "Alpha")

    packets = [
        _make_v4_pkt(MYSQL_TABLE_MAP_EVENT, 120, tmap_body),
        _make_v4_pkt(MYSQL_WRITE_ROWS_EVENT_V2, 240, write_body),
        _make_v4_pkt(MYSQL_UPDATE_ROWS_EVENT_V2, 360, upd_body),
        _make_v4_pkt(MYSQL_DELETE_ROWS_EVENT_V2, 420, del_body),
    ]
    events, meta = decode_mysql_binlog_v4_packets(
        packets,
        ["id", "name"],
        ["id"],
        "erp",
        "orders",
        binlog_file="mysql-bin.000003",
    )
    assert [(e.op, e.pk["id"]) for e in events] == [("I", 1), ("I", 2), ("U", 2), ("D", 1)]
    assert meta["binlog_file"] == "mysql-bin.000003"
    assert meta["binlog_pos"] == 420

    rec_events, rec_meta = parse_mysql_cdc_records(
        [
            {
                "schema": "erp",
                "table": "orders",
                "event_type": "WRITE_ROWS",
                "values": {"id": 9, "name": "N9"},
                "binlog_file": "mysql-bin.000004",
                "binlog_pos": 512,
            },
            {
                "schema": "erp",
                "table": "orders",
                "event_type": "DELETE_ROWS",
                "values": {"id": 9, "name": "N9"},
                "binlog_file": "mysql-bin.000004",
                "binlog_pos": 600,
            },
        ],
        ["id"],
        "erp",
        "orders",
    )
    assert [e.op for e in rec_events] == ["I", "D"]
    assert rec_meta["binlog_pos"] == 600


def test_ducklake_cdc_hard_delete_and_upsert_single_tx_atomicity(tmp_path: Path) -> None:
    dsn = f"ducklake:{tmp_path / 'step29_cdc.ducklake'}"

    # Batch 1: initial 3 rows via PK digest diff
    rows_b1 = [
        {"order_id": "ORD-1", "amount": 100.0, "region": "EAST"},
        {"order_id": "ORD-2", "amount": 200.0, "region": "WEST"},
        {"order_id": "ORD-3", "amount": 300.0, "region": "SOUTH"},
    ]
    ev1, meta1 = compute_pk_digest_diff(rows_b1, ["order_id"], {}, mode_tag="pg_wal_pk_diff")
    res1 = apply_cdc_events(
        tenant_id="default",
        dataset_id="ds-cdc-1",
        batch_id="b-1",
        lake_table="cdc_orders",
        primary_key=["order_id"],
        schema_policy="ADDITIVE",
        expected_schema=[],
        quality_rules=[],
        events=ev1,
        cdc_slot_meta=meta1,
        data_root=tmp_path,
        catalog_dsn=dsn,
    )
    assert res1.rows_written == 3
    assert res1.rows_deleted == 0
    assert res1.total_rows == 3

    # Batch 2: hard-delete ORD-2, update ORD-1 -> 150.0, insert ORD-4
    rows_b2 = [
        {"order_id": "ORD-1", "amount": 150.0, "region": "EAST"},
        {"order_id": "ORD-3", "amount": 300.0, "region": "SOUTH"},
        {"order_id": "ORD-4", "amount": 420.0, "region": "NORTH"},
    ]
    ev2, meta2 = compute_pk_digest_diff(
        rows_b2, ["order_id"], res1.cdc_slot_meta, mode_tag="pg_wal_pk_diff"
    )
    assert [(e.op, e.pk["order_id"]) for e in ev2] == [
        ("U", "ORD-1"),
        ("I", "ORD-4"),
        ("D", "ORD-2"),
    ]

    res2 = apply_cdc_events(
        tenant_id="default",
        dataset_id="ds-cdc-1",
        batch_id="b-2",
        lake_table="cdc_orders",
        primary_key=["order_id"],
        schema_policy="ADDITIVE",
        expected_schema=res1.columns_schema,
        quality_rules=[],
        events=ev2,
        cdc_slot_meta=meta2,
        data_root=tmp_path,
        catalog_dsn=dsn,
    )
    assert res2.rows_deleted == 1
    assert res2.rows_written == 2
    assert res2.total_rows == 3
    assert res2.deleted_primary_keys == [{"order_id": "ORD-2"}]

    prev = preview_lake(
        LakePreviewRequest(tenant_id="default", lake_table="cdc_orders", limit=20),
        data_root=tmp_path,
        catalog_dsn=dsn,
    )
    by_id = {r["order_id"]: r["amount"] for r in prev.rows}
    assert by_id == {"ORD-1": 150.0, "ORD-3": 300.0, "ORD-4": 420.0}

    with pytest.raises(SidecarError) as exc_info:
        validate_cdc_ident("order_id; DROP TABLE x")
    assert exc_info.value.error_code == "SQL_REJECTED"
