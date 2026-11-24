from __future__ import annotations

import hashlib
import re
import struct
from collections.abc import Sequence
from dataclasses import dataclass
from datetime import UTC, date, datetime
from decimal import Decimal
from typing import Any, Literal

import orjson
from sqlalchemy import text
from sqlalchemy.engine import Engine

from connectors.base import normalize_py_value_for_arrow
from security import SidecarError, quote_sql_ident

CdcOp = Literal["I", "U", "D"]

STRICT_ASCII_IDENT_RE = re.compile(r"^[A-Za-z_][A-Za-z0-9_$]*$")
_TEST_DECODING_RE = re.compile(
    r"^table\s+(?:(?P<schema>[A-Za-z_][A-Za-z0-9_$]*)\.)?(?P<table>[A-Za-z_][A-Za-z0-9_$]*):\s+"
    r"(?P<verb>INSERT|UPDATE|DELETE):\s*(?P<cols>.*)$"
)
_TD_COL_RE = re.compile(
    r"(?P<name>[A-Za-z_][A-Za-z0-9_$]*)\[(?P<typ>[^\]]+)\]:"
    r"(?P<val>'(?:''|[^'])*'|null|-?\d+(?:\.\d+)?|true|false|\S+)"
)


@dataclass(frozen=True)
class CdcChangeEvent:
    """Normalized row-level CDC event emitted by PostgreSQL or MySQL log readers."""

    op: CdcOp
    pk: dict[str, Any]
    row_after: dict[str, Any] | None
    lsn: str | None = None


def validate_cdc_ident(ident: str) -> str:
    """Validate identifier against strict ASCII SQL whitelist ^[A-Za-z_][A-Za-z0-9_$]*$."""
    cleaned = ident.strip()
    if not cleaned or not STRICT_ASCII_IDENT_RE.fullmatch(cleaned):
        raise SidecarError(
            status_code=400,
            error_code="SQL_REJECTED",
            message=f"invalid CDC SQL identifier: {ident!r}",
        )
    return cleaned


def canonical_pk_key(pk_cols: Sequence[str], row_or_pk: dict[str, Any]) -> str:
    """Serialize primary key values into a canonical deterministic JSON key."""
    ordered = [normalize_cdc_scalar(row_or_pk.get(col)) for col in pk_cols]
    return orjson.dumps(ordered, default=str).decode("utf-8")


def normalize_cdc_scalar(val: Any) -> Any:
    """Normalize scalar values for deterministic hashing and JSON persistence."""
    norm = normalize_py_value_for_arrow(val)
    if isinstance(norm, Decimal):
        return str(norm)
    if isinstance(norm, (datetime, date)):
        return norm.isoformat()
    if isinstance(norm, bytes):
        return norm.hex()
    return norm


def compute_row_digest(row: dict[str, Any]) -> str:
    """Compute SHA-256 hex digest of a normalized source row."""
    clean = {
        k: normalize_cdc_scalar(row[k])
        for k in sorted(row.keys())
        if not k.startswith("_ax_")
    }
    payload = orjson.dumps(clean, option=orjson.OPT_SORT_KEYS, default=str)
    return hashlib.sha256(payload).hexdigest()


def compute_pk_digest_diff(
    rows: Sequence[dict[str, Any]],
    primary_key: Sequence[str],
    prev_meta: dict[str, Any],
    *,
    mode_tag: str,
    extra_meta: dict[str, Any] | None = None,
) -> tuple[list[CdcChangeEvent], dict[str, Any]]:
    """Compute exact (I, U, D) change events by comparing read-only PK row digests."""
    pk_cols = [validate_cdc_ident(pk) for pk in primary_key]
    raw_prev_digests = prev_meta.get("pk_digests")
    raw_prev_pks = prev_meta.get("pk_values")
    prev_digests: dict[str, str] = (
        {str(k): str(v) for k, v in raw_prev_digests.items()}
        if isinstance(raw_prev_digests, dict)
        else {}
    )
    prev_pks: dict[str, dict[str, Any]] = (
        {
            str(k): dict(v)
            for k, v in raw_prev_pks.items()
            if isinstance(v, dict)
        }
        if isinstance(raw_prev_pks, dict)
        else {}
    )

    next_digests: dict[str, str] = {}
    next_pks: dict[str, dict[str, Any]] = {}
    events: list[CdcChangeEvent] = []

    for row in rows:
        missing = [pk for pk in pk_cols if pk not in row or row[pk] is None]
        if missing:
            raise SidecarError(
                400,
                "INVALID_ARGUMENT",
                f"LOG_CDC source row missing primary_key columns {missing}",
            )
        pk_map = {pk: row[pk] for pk in pk_cols}
        pk_key = canonical_pk_key(pk_cols, pk_map)
        digest = compute_row_digest(row)
        next_digests[pk_key] = digest
        next_pks[pk_key] = {pk: normalize_cdc_scalar(row[pk]) for pk in pk_cols}

        old_digest = prev_digests.get(pk_key)
        if old_digest is None:
            events.append(CdcChangeEvent(op="I", pk=pk_map, row_after=dict(row)))
        elif old_digest != digest:
            events.append(CdcChangeEvent(op="U", pk=pk_map, row_after=dict(row)))

    for old_key in prev_digests:
        if old_key not in next_digests:
            pk_dict = prev_pks.get(old_key)
            if pk_dict is None:
                decoded = orjson.loads(old_key)
                if isinstance(decoded, list) and len(decoded) == len(pk_cols):
                    pk_dict = dict(zip(pk_cols, decoded, strict=False))
                else:
                    continue
            events.append(CdcChangeEvent(op="D", pk=pk_dict, row_after=None))

    next_meta: dict[str, Any] = {
        "mode": mode_tag,
        "pk_digests": next_digests,
        "pk_values": next_pks,
        "synced_at": datetime.now(UTC).isoformat(),
    }
    if extra_meta:
        next_meta.update(extra_meta)
    return events, next_meta


def _decode_td_literal(raw_val: str, pg_type: str) -> Any:
    if raw_val == "null":
        return None
    if raw_val.startswith("'") and raw_val.endswith("'"):
        inner = raw_val[1:-1].replace("''", "'")
        return inner
    low_t = pg_type.lower()
    if "int" in low_t:
        try:
            return int(raw_val)
        except ValueError:
            return raw_val
    if any(tok in low_t for tok in ("numeric", "float", "double", "real", "decimal")):
        try:
            return float(raw_val)
        except ValueError:
            return raw_val
    if "bool" in low_t:
        return raw_val.lower() in {"true", "t", "1"}
    return raw_val


def parse_test_decoding_line(
    data: str,
    primary_key: Sequence[str],
    target_schema: str | None,
    target_table: str,
    *,
    lsn: str | None = None,
) -> CdcChangeEvent | None:
    """Parse a PostgreSQL test_decoding logical replication slot line into CdcChangeEvent."""
    m = _TEST_DECODING_RE.match(data.strip())
    if not m:
        return None
    line_schema = m.group("schema") or "public"
    line_table = m.group("table")
    if target_schema and line_schema != target_schema:
        return None
    if line_table != target_table:
        return None

    verb = m.group("verb")
    cols_part = m.group("cols")
    if verb == "UPDATE" and " new-tuple: " in cols_part:
        cols_part = cols_part.split(" new-tuple: ", 1)[1]

    parsed_cols: dict[str, Any] = {}
    for cm in _TD_COL_RE.finditer(cols_part):
        c_name = cm.group("name")
        c_type = cm.group("typ")
        c_val = _decode_td_literal(cm.group("val"), c_type)
        parsed_cols[c_name] = c_val

    if any(pk not in parsed_cols for pk in primary_key):
        return None
    pk_map = {pk: parsed_cols[pk] for pk in primary_key}

    if verb == "DELETE":
        return CdcChangeEvent(op="D", pk=pk_map, row_after=None, lsn=lsn)
    if verb == "UPDATE":
        return CdcChangeEvent(op="U", pk=pk_map, row_after=parsed_cols, lsn=lsn)
    return CdcChangeEvent(op="I", pk=pk_map, row_after=parsed_cols, lsn=lsn)


def parse_wal2json_payload(
    raw_json: str,
    primary_key: Sequence[str],
    target_schema: str | None,
    target_table: str,
    *,
    lsn: str | None = None,
) -> list[CdcChangeEvent]:
    """Parse wal2json v1/v2 JSON change payloads into CdcChangeEvent items."""
    try:
        doc = orjson.loads(raw_json)
    except orjson.JSONDecodeError:
        return []
    if not isinstance(doc, dict):
        return []

    out: list[CdcChangeEvent] = []
    changes = doc.get("change")
    if isinstance(changes, list):
        for item in changes:
            if not isinstance(item, dict):
                continue
            sch = str(item.get("schema") or "public")
            tbl = str(item.get("table") or "")
            if (target_schema and sch != target_schema) or tbl != target_table:
                continue
            kind = str(item.get("kind") or "").lower()
            names = item.get("columnnames")
            vals = item.get("columnvalues")
            oldkeys = item.get("oldkeys")
            row_after = (
                dict(zip([str(n) for n in names], vals, strict=False))
                if isinstance(names, list) and isinstance(vals, list)
                else {}
            )
            old_map: dict[str, Any] = {}
            if isinstance(oldkeys, dict):
                ok_names = oldkeys.get("keynames")
                ok_vals = oldkeys.get("keyvalues")
                if isinstance(ok_names, list) and isinstance(ok_vals, list):
                    old_map = dict(zip([str(n) for n in ok_names], ok_vals, strict=False))
            pk_src = old_map if kind == "delete" and old_map else row_after
            if any(pk not in pk_src for pk in primary_key):
                continue
            pk_map = {pk: pk_src[pk] for pk in primary_key}
            if kind == "delete":
                out.append(CdcChangeEvent(op="D", pk=pk_map, row_after=None, lsn=lsn))
            elif kind == "update":
                out.append(CdcChangeEvent(op="U", pk=pk_map, row_after=row_after, lsn=lsn))
            elif kind == "insert":
                out.append(CdcChangeEvent(op="I", pk=pk_map, row_after=row_after, lsn=lsn))
    return out


def decode_pgoutput_messages(
    frames: Sequence[bytes],
    primary_key: Sequence[str],
    target_schema: str | None,
    target_table: str,
) -> list[CdcChangeEvent]:
    """Decode pgoutput binary protocol frames ('R' Relation + 'I'/'U'/'D' TupleData)."""
    relations: dict[int, tuple[str, str, list[str]]] = {}
    events: list[CdcChangeEvent] = []

    for frame in frames:
        if not frame:
            continue
        tag = chr(frame[0])
        if tag == "R" and len(frame) >= 7:
            rel_id = struct.unpack(">I", frame[1:5])[0]
            pos = 5
            ns_end = frame.index(b"\x00", pos)
            ns = frame[pos:ns_end].decode("utf-8")
            pos = ns_end + 1
            rel_end = frame.index(b"\x00", pos)
            rel_name = frame[pos:rel_end].decode("utf-8")
            pos = rel_end + 2  # skip replica_identity byte
            ncols = struct.unpack(">H", frame[pos : pos + 2])[0]
            pos += 2
            col_names: list[str] = []
            for _ in range(ncols):
                pos += 1  # flags byte
                c_end = frame.index(b"\x00", pos)
                col_names.append(frame[pos:c_end].decode("utf-8"))
                pos = c_end + 1 + 8  # oid (4) + atttypmod (4)
            relations[rel_id] = (ns, rel_name, col_names)
        elif tag in {"I", "U", "D"} and len(frame) >= 6:
            rel_id = struct.unpack(">I", frame[1:5])[0]
            rel_meta = relations.get(rel_id)
            if rel_meta is None:
                continue
            ns, rel_name, col_names = rel_meta
            if (target_schema and ns != target_schema) or rel_name != target_table:
                continue
            pos = 5
            sub_tag = chr(frame[pos])
            if tag == "U" and sub_tag in {"K", "O"}:
                pos, _old_vals = _read_pgoutput_tuple(frame, pos + 1, col_names)
                sub_tag = chr(frame[pos])
            if sub_tag not in {"N", "K", "O"}:
                continue
            _end_pos, tup_dict = _read_pgoutput_tuple(frame, pos + 1, col_names)
            if any(pk not in tup_dict for pk in primary_key):
                continue
            pk_map = {pk: tup_dict[pk] for pk in primary_key}
            if tag == "D":
                events.append(CdcChangeEvent(op="D", pk=pk_map, row_after=None))
            elif tag == "U":
                events.append(CdcChangeEvent(op="U", pk=pk_map, row_after=tup_dict))
            else:
                events.append(CdcChangeEvent(op="I", pk=pk_map, row_after=tup_dict))
    return events


def _read_pgoutput_tuple(
    buf: bytes, start: int, col_names: Sequence[str]
) -> tuple[int, dict[str, Any]]:
    ncols = struct.unpack(">H", buf[start : start + 2])[0]
    pos = start + 2
    out: dict[str, Any] = {}
    for idx in range(ncols):
        kind = chr(buf[pos])
        pos += 1
        c_name = col_names[idx] if idx < len(col_names) else f"col_{idx}"
        if kind == "n":
            out[c_name] = None
        elif kind == "u":
            continue
        elif kind in {"t", "b"}:
            vlen = struct.unpack(">I", buf[pos : pos + 4])[0]
            pos += 4
            raw = buf[pos : pos + vlen]
            pos += vlen
            if kind == "t":
                s_val = raw.decode("utf-8")
                out[c_name] = int(s_val) if s_val.lstrip("-").isdigit() else s_val
            else:
                out[c_name] = raw
    return pos, out


def _lsn_to_int(lsn: str | None) -> int:
    if not lsn or "/" not in lsn:
        return -1
    hi_s, lo_s = lsn.strip().split("/", 1)
    try:
        return (int(hi_s, 16) << 32) | int(lo_s, 16)
    except ValueError:
        return -1


def read_postgres_cdc(
    engine: Engine,
    *,
    schema: str | None,
    table: str,
    primary_key: Sequence[str],
    cdc_slot_meta: dict[str, Any],
    batch_rows: int = 50_000,
) -> tuple[list[CdcChangeEvent], dict[str, Any]]:
    """Read bounded PostgreSQL LOG_CDC changes via logical slot peek or read-only PK digest diff."""
    if not primary_key:
        raise SidecarError(400, "INVALID_ARGUMENT", "LOG_CDC requires non-empty primary_key")
    valid_schema = validate_cdc_ident(schema or "public")
    valid_table = validate_cdc_ident(table)
    pk_cols = [validate_cdc_ident(pk) for pk in primary_key]
    capped = max(min(batch_rows, 500_000), 1)

    with engine.connect() as conn:
        wal_row = conn.execute(text("SHOW wal_level")).fetchone()
        wal_level = str(wal_row[0]).strip().lower() if wal_row and wal_row[0] else "replica"
        slot_name = str(cdc_slot_meta.get("slot_name") or "").strip()

        if wal_level == "logical" and slot_name and STRICT_ASCII_IDENT_RE.fullmatch(slot_name):
            slot_exists = conn.execute(
                text(
                    "SELECT plugin FROM pg_replication_slots "
                    "WHERE slot_name = :slot AND slot_type = 'logical' LIMIT 1"
                ),
                {"slot": slot_name},
            ).fetchone()
            if slot_exists is not None:
                last_lsn = str(cdc_slot_meta.get("last_lsn") or "").strip() or None
                last_lsn_num = _lsn_to_int(last_lsn)
                slot_rows = conn.execute(
                    text(
                        "SELECT lsn::text, data "
                        "FROM pg_logical_slot_peek_changes(:slot, NULL, :lim)"
                    ),
                    {"slot": slot_name, "lim": capped},
                ).fetchall()

                events: list[CdcChangeEvent] = []
                max_lsn = last_lsn
                for lsn_str_raw, data_raw in slot_rows:
                    lsn_str = str(lsn_str_raw)
                    if last_lsn_num >= 0 and _lsn_to_int(lsn_str) <= last_lsn_num:
                        continue
                    max_lsn = lsn_str
                    text_payload = str(data_raw or "")
                    if text_payload.lstrip().startswith("{"):
                        events.extend(
                            parse_wal2json_payload(
                                text_payload, pk_cols, valid_schema, valid_table, lsn=lsn_str
                            )
                        )
                    else:
                        parsed = parse_test_decoding_line(
                            text_payload, pk_cols, valid_schema, valid_table, lsn=lsn_str
                        )
                        if parsed is not None:
                            events.append(parsed)

                next_meta = dict(cdc_slot_meta)
                next_meta.update(
                    {
                        "mode": "pg_logical_slot",
                        "wal_level": wal_level,
                        "slot_name": slot_name,
                        "last_lsn": max_lsn,
                    }
                )
                return events, next_meta

        q_schema = quote_sql_ident(valid_schema, dialect="postgres")
        q_table = quote_sql_ident(valid_table, dialect="postgres")
        order_pks = ", ".join(quote_sql_ident(pk, dialect="postgres") for pk in pk_cols)
        res = conn.execute(
            text(f"SELECT * FROM {q_schema}.{q_table} ORDER BY {order_pks} LIMIT :lim"),
            {"lim": capped},
        )
        col_names = list(res.keys())
        rows = [dict(zip(col_names, r, strict=False)) for r in res.fetchall()]
        cur_lsn_row = conn.execute(
            text(
                "SELECT CASE WHEN pg_is_in_recovery() "
                "THEN pg_last_wal_replay_lsn()::text "
                "ELSE pg_current_wal_lsn()::text END"
            )
        ).fetchone()
        cur_lsn = str(cur_lsn_row[0]) if cur_lsn_row and cur_lsn_row[0] else "0/0"

    return compute_pk_digest_diff(
        rows,
        pk_cols,
        cdc_slot_meta,
        mode_tag="pg_wal_pk_diff",
        extra_meta={"wal_level": wal_level, "last_lsn": cur_lsn},
    )
