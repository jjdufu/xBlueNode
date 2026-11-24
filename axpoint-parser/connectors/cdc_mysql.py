from __future__ import annotations

import struct
from collections.abc import Iterable, Sequence
from typing import Any

from sqlalchemy import text
from sqlalchemy.engine import Engine

from connectors.cdc_pg import (
    CdcChangeEvent,
    compute_pk_digest_diff,
    validate_cdc_ident,
)
from security import SidecarError, quote_sql_ident

MYSQL_TABLE_MAP_EVENT = 0x13
MYSQL_WRITE_ROWS_EVENT_V2 = 0x1E
MYSQL_UPDATE_ROWS_EVENT_V2 = 0x1F
MYSQL_DELETE_ROWS_EVENT_V2 = 0x20


def parse_mysql_cdc_records(
    raw_events: Iterable[dict[str, Any]],
    primary_key: Sequence[str],
    target_schema: str | None,
    target_table: str,
    *,
    initial_meta: dict[str, Any] | None = None,
) -> tuple[list[CdcChangeEvent], dict[str, Any]]:
    """Normalize structured MySQL ROW binlog event dicts into CdcChangeEvent items."""
    pk_cols = [validate_cdc_ident(pk) for pk in primary_key]
    meta: dict[str, Any] = dict(initial_meta or {})
    events: list[CdcChangeEvent] = []

    for ev in raw_events:
        ev_schema = str(ev.get("schema") or ev.get("database") or "")
        ev_table = str(ev.get("table") or "")
        if target_schema and ev_schema and ev_schema != target_schema:
            continue
        if ev_table and ev_table != target_table:
            continue

        bfile = ev.get("binlog_file") or ev.get("log_file")
        bpos = ev.get("binlog_pos") or ev.get("log_pos")
        if bfile:
            meta["binlog_file"] = str(bfile)
        if bpos is not None:
            meta["binlog_pos"] = int(bpos)

        raw_type = str(ev.get("event_type") or ev.get("op") or "").upper()
        if raw_type in {"WRITE_ROWS", "INSERT", "I"}:
            row_after = dict(ev.get("values") or ev.get("after_values") or ev.get("row") or {})
            if all(pk in row_after for pk in pk_cols):
                events.append(
                    CdcChangeEvent(
                        op="I",
                        pk={pk: row_after[pk] for pk in pk_cols},
                        row_after=row_after,
                    )
                )
        elif raw_type in {"UPDATE_ROWS", "UPDATE", "U"}:
            row_after = dict(ev.get("after_values") or ev.get("values") or ev.get("row") or {})
            row_before = dict(ev.get("before_values") or {})
            if row_before and all(pk in row_before and pk in row_after for pk in pk_cols):
                old_pk = {pk: row_before[pk] for pk in pk_cols}
                new_pk = {pk: row_after[pk] for pk in pk_cols}
                if old_pk != new_pk:
                    events.append(CdcChangeEvent(op="D", pk=old_pk, row_after=None))
                    events.append(CdcChangeEvent(op="I", pk=new_pk, row_after=row_after))
                    continue
            if all(pk in row_after for pk in pk_cols):
                events.append(
                    CdcChangeEvent(
                        op="U",
                        pk={pk: row_after[pk] for pk in pk_cols},
                        row_after=row_after,
                    )
                )
        elif raw_type in {"DELETE_ROWS", "DELETE", "D"}:
            row_before = dict(ev.get("values") or ev.get("before_values") or ev.get("row") or {})
            if all(pk in row_before for pk in pk_cols):
                events.append(
                    CdcChangeEvent(
                        op="D",
                        pk={pk: row_before[pk] for pk in pk_cols},
                        row_after=None,
                    )
                )

    meta["mode"] = "mysql_binlog_row"
    return events, meta


def decode_mysql_binlog_v4_packets(
    packets: Sequence[bytes],
    column_names: Sequence[str],
    primary_key: Sequence[str],
    target_schema: str,
    target_table: str,
    *,
    binlog_file: str = "mysql-bin.000001",
) -> tuple[list[CdcChangeEvent], dict[str, Any]]:
    """Decode compact MySQL v4 ROW binlog packets (19-byte header + TABLE_MAP / ROWS_V2)."""
    pk_cols = [validate_cdc_ident(pk) for pk in primary_key]
    table_map: dict[int, tuple[str, str]] = {}
    events: list[CdcChangeEvent] = []
    last_pos = 4

    for pkt in packets:
        if len(pkt) < 19:
            continue
        _ts, event_type, _srv_id, _ev_len, next_pos, _flags = struct.unpack("<IBIIIH", pkt[:19])
        last_pos = int(next_pos)
        body = pkt[19:]

        if event_type == MYSQL_TABLE_MAP_EVENT and len(body) >= 10:
            table_id = int.from_bytes(body[:6], "little")
            pos = 8  # 6-byte table_id + 2-byte flags
            db_len = body[pos]
            pos += 1
            db_name = body[pos : pos + db_len].decode("utf-8")
            pos += db_len + 1  # null terminator
            tbl_len = body[pos]
            pos += 1
            tbl_name = body[pos : pos + tbl_len].decode("utf-8")
            table_map[table_id] = (db_name, tbl_name)

        elif event_type in {
            MYSQL_WRITE_ROWS_EVENT_V2,
            MYSQL_UPDATE_ROWS_EVENT_V2,
            MYSQL_DELETE_ROWS_EVENT_V2,
        } and len(body) >= 10:
            table_id = int.from_bytes(body[:6], "little")
            mapped = table_map.get(table_id)
            if mapped is None or mapped != (target_schema, target_table):
                continue
            extra_len = struct.unpack("<H", body[8:10])[0]
            pos = 8 + max(extra_len, 2)
            ncols = body[pos]
            pos += 1
            bitmap_len = (ncols + 7) // 8
            pos += bitmap_len  # columns-present bitmap 1
            if event_type == MYSQL_UPDATE_ROWS_EVENT_V2:
                pos += bitmap_len  # columns-present bitmap 2

            while pos < len(body):
                pos, row1 = _decode_compact_row(body, pos, column_names[:ncols])
                if event_type == MYSQL_WRITE_ROWS_EVENT_V2:
                    if all(pk in row1 for pk in pk_cols):
                        events.append(
                            CdcChangeEvent(
                                op="I",
                                pk={pk: row1[pk] for pk in pk_cols},
                                row_after=row1,
                            )
                        )
                elif event_type == MYSQL_DELETE_ROWS_EVENT_V2:
                    if all(pk in row1 for pk in pk_cols):
                        events.append(
                            CdcChangeEvent(
                                op="D",
                                pk={pk: row1[pk] for pk in pk_cols},
                                row_after=None,
                            )
                        )
                else:
                    pos, row2 = _decode_compact_row(body, pos, column_names[:ncols])
                    if all(pk in row2 for pk in pk_cols):
                        events.append(
                            CdcChangeEvent(
                                op="U",
                                pk={pk: row2[pk] for pk in pk_cols},
                                row_after=row2,
                            )
                        )

    return events, {
        "mode": "mysql_binlog_row",
        "binlog_file": binlog_file,
        "binlog_pos": last_pos,
    }


def _decode_compact_row(
    buf: bytes, start: int, col_names: Sequence[str]
) -> tuple[int, dict[str, Any]]:
    ncols = len(col_names)
    null_len = (ncols + 7) // 8
    null_bits = buf[start : start + null_len]
    pos = start + null_len
    out: dict[str, Any] = {}
    for idx, c_name in enumerate(col_names):
        is_null = bool(null_bits[idx // 8] & (1 << (idx % 8)))
        if is_null:
            out[c_name] = None
            continue
        tag = buf[pos]
        pos += 1
        if tag == 0x01:  # signed int64
            out[c_name] = struct.unpack("<q", buf[pos : pos + 8])[0]
            pos += 8
        elif tag == 0x02:  # float64
            out[c_name] = struct.unpack("<d", buf[pos : pos + 8])[0]
            pos += 8
        else:  # length-prefixed utf-8 string (0x03)
            slen = struct.unpack("<H", buf[pos : pos + 2])[0]
            pos += 2
            out[c_name] = buf[pos : pos + slen].decode("utf-8")
            pos += slen
    return pos, out


def read_mysql_cdc(
    engine: Engine,
    *,
    database: str | None,
    table: str,
    primary_key: Sequence[str],
    cdc_slot_meta: dict[str, Any],
    batch_rows: int = 50_000,
) -> tuple[list[CdcChangeEvent], dict[str, Any]]:
    """Read bounded MySQL LOG_CDC changes with binlog position metadata and read-only PK diff."""
    if not primary_key:
        raise SidecarError(400, "INVALID_ARGUMENT", "LOG_CDC requires non-empty primary_key")
    valid_table = validate_cdc_ident(table)
    valid_db = validate_cdc_ident(database) if database else None
    pk_cols = [validate_cdc_ident(pk) for pk in primary_key]
    capped = max(min(batch_rows, 500_000), 1)

    binlog_file = str(cdc_slot_meta.get("binlog_file") or "")
    binlog_pos = int(cdc_slot_meta.get("binlog_pos") or 4)

    with engine.connect() as conn:
        for status_sql in ("SHOW BINARY LOG STATUS", "SHOW MASTER STATUS"):
            try:
                st_row = conn.execute(text(status_sql)).fetchone()
                if st_row and len(st_row) >= 2:
                    binlog_file = str(st_row[0])
                    binlog_pos = int(st_row[1])
                    break
            except Exception:
                continue

        q_tbl = quote_sql_ident(valid_table, dialect="mysql")
        from_expr = (
            f"{quote_sql_ident(valid_db, dialect='mysql')}.{q_tbl}" if valid_db else q_tbl
        )
        order_pks = ", ".join(quote_sql_ident(pk, dialect="mysql") for pk in pk_cols)
        res = conn.execute(
            text(f"SELECT * FROM {from_expr} ORDER BY {order_pks} LIMIT :lim"),
            {"lim": capped},
        )
        col_names = list(res.keys())
        rows = [dict(zip(col_names, r, strict=False)) for r in res.fetchall()]

    extra: dict[str, Any] = {
        "binlog_file": binlog_file or "mysql-bin.000001",
        "binlog_pos": binlog_pos,
    }
    return compute_pk_digest_diff(
        rows,
        pk_cols,
        cdc_slot_meta,
        mode_tag="mysql_binlog_pk_diff",
        extra_meta=extra,
    )
