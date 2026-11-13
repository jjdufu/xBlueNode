from __future__ import annotations

import re

from schemas import DataType

_WRAPPER_RE = re.compile(r"^(?:nullable|lowcardinality)\s*\((.*)\)$", re.IGNORECASE)
_NAME_ARGS_RE = re.compile(r"^([a-z0-9_ ]+?)\s*\(\s*([^)]*)\s*\)", re.IGNORECASE)


def _unwrap_clickhouse(raw: str) -> str:
    cur = raw.strip()
    while True:
        m = _WRAPPER_RE.match(cur)
        if not m:
            return cur
        cur = m.group(1).strip()


def map_native_type(native_type: str, dialect: str = "") -> DataType:
    """Map database/source native type string into canonical AxPoint DataType."""
    if not native_type:
        return "STRING"

    d = dialect.strip().lower()
    t = _unwrap_clickhouse(native_type.strip())
    low = t.lower()

    # ClickHouse / Hive / PG array, tuple, struct, map containers
    if low.startswith(("array(", "array<", "list<", "_")) or low.endswith("[]"):
        if low.startswith("_") and len(low) > 1:
            return "LIST"
        if not low.startswith("_"):
            return "LIST"

    if low.startswith(("tuple(", "struct<", "row(")):
        return "STRUCT"

    if low.startswith(("map(", "map<")):
        return "JSON"

    # Extract base type name and optional (precision, scale)
    base = low
    args: list[str] = []
    m_args = _NAME_ARGS_RE.match(low)
    if m_args:
        base = m_args.group(1).strip()
        args = [a.strip() for a in m_args.group(2).split(",") if a.strip()]

    # Strip unsigned / zerofill modifiers (MySQL / ClickHouse)
    for suffix in (" unsigned", " signed", " zerofill"):
        if base.endswith(suffix):
            base = base[: -len(suffix)].strip()

    # Dialect-specific overrides
    if d in {"mysql", "tidb", "oceanbase", "doris", "starrocks"}:
        if base == "tinyint" and args == ["1"]:
            return "BOOLEAN"
        if base == "largeint":
            return "STRING"

    if d in {"oracle", "dameng"}:
        if base == "date":
            return "TIMESTAMP"
        if base in {"number", "dec", "numeric", "decimal"}:
            if not args:
                return "DECIMAL"
            try:
                prec = int(args[0])
                scale = int(args[1]) if len(args) > 1 else 0
            except ValueError:
                return "DECIMAL"
            if prec > 38:
                return "STRING"
            if scale <= 0 and prec <= 18:
                return "INT64"
            return "DECIMAL"

    if d in {"clickhouse"}:
        if base in {"int128", "uint128", "int256", "uint256"}:
            return "STRING"
        if base.startswith("datetime"):
            return "TIMESTAMP"
        if base in {"date", "date32"}:
            return "DATE"
        if base.startswith("decimal"):
            return "DECIMAL"
        if base.startswith("fixedstring") or base.startswith("enum"):
            return "STRING"

    if d in {"tdengine"} and base in {"nchar", "binary"}:
        return "STRING"

    # Generic mappings across SQL & NoSQL families
    if base in {"bool", "boolean", "bit"}:
        if base == "bit" and args and args[0] != "1":
            return "BINARY"
        return "BOOLEAN"

    if base in {
        "tinyint",
        "smallint",
        "mediumint",
        "int",
        "integer",
        "bigint",
        "int1",
        "int2",
        "int4",
        "int8",
        "int16",
        "int32",
        "int64",
        "uint8",
        "uint16",
        "uint32",
        "uint64",
        "serial",
        "smallserial",
        "bigserial",
        "long",
        "short",
        "byte",
    }:
        return "INT64"

    if base in {"numeric", "decimal", "dec", "money", "smallmoney", "decimal128", "number"}:
        if args:
            try:
                p = int(args[0])
                if p > 38:
                    return "STRING"
            except ValueError:
                pass
        return "DECIMAL"

    if base in {
        "real",
        "float",
        "double",
        "double precision",
        "float4",
        "float8",
        "float32",
        "float64",
        "binary_float",
        "binary_double",
        "half_float",
        "scaled_float",
    }:
        return "FLOAT64"

    if base == "date":
        return "DATE"

    if (
        base
        in {
            "timestamp",
            "timestamptz",
            "timestamp with time zone",
            "timestamp without time zone",
            "timestamp with local time zone",
            "datetime",
            "datetime2",
            "smalldatetime",
            "datetimeoffset",
        }
        or base.startswith("timestamp")
        or base.startswith("datetime")
    ):
        return "TIMESTAMP"

    if base in {
        "json",
        "jsonb",
        "object",
        "document",
        "bson",
        "variant",
        "super",
    }:
        return "JSON"

    if base in {
        "bytea",
        "blob",
        "tinyblob",
        "mediumblob",
        "longblob",
        "binary",
        "varbinary",
        "raw",
        "long raw",
        "image",
        "bytes",
    }:
        return "BINARY"

    if base in {"array", "list", "set"}:
        return "LIST"

    if base in {"struct", "tuple", "row"}:
        return "STRUCT"

    # SQLite affinity fallback when dialect == "sqlite"
    if d == "sqlite":
        if "int" in base:
            return "INT64"
        if any(k in base for k in ("char", "clob", "text")):
            return "STRING"
        if "blob" in base:
            return "BINARY"
        if any(k in base for k in ("real", "floa", "doub")):
            return "FLOAT64"

    return "STRING"
