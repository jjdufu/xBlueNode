from __future__ import annotations

import math
from abc import ABC, abstractmethod
from collections.abc import Iterator, Sequence
from dataclasses import dataclass, field
from datetime import UTC, date, datetime
from decimal import Decimal
from typing import Any

import orjson
import polars as pl
import pyarrow as pa
import sqlglot
from sqlglot import exp

from parsers.profiler import mask_pii_value, profile_polars_df
from schemas import (
    ColumnProfile,
    ConnectorSpecModel,
    DataType,
    DiscoveredObject,
    PreviewResponse,
)
from security import SidecarError, quote_sql_ident, validate_sql_ident

_FORBIDDEN_SQL_NODES: tuple[type[exp.Expression], ...] = (
    exp.Insert,
    exp.Update,
    exp.Delete,
    exp.Drop,
    exp.Alter,
    exp.Create,
    exp.Command,
    exp.Copy,
    exp.Merge,
    exp.Into,
)


@dataclass
class ReadBatch:
    """Single Arrow RecordBatch yielded by a Connector.read() generator."""

    batch: pa.RecordBatch
    cursor_to: str | None = None
    offsets: dict[str, str] = field(default_factory=dict)


class BaseConnector(ABC):
    """Base protocol/ABC for all 32 AxHub connectors."""

    spec: ConnectorSpecModel

    def __init__(
        self,
        config: dict[str, Any],
        secrets: dict[str, str],
        *,
        allow_private_network: bool = False,
        tenant_id: str = "default",
    ) -> None:
        self.config = dict(config)
        self.secrets = dict(secrets)
        self.allow_private_network = allow_private_network
        self.tenant_id = tenant_id

    @abstractmethod
    def test(self) -> tuple[bool, str]:
        """Verify upstream reachability and read-only credentials."""

    def discover(self) -> list[DiscoveredObject]:
        """Discover tables, collections, topics, or endpoints."""
        return []

    @abstractmethod
    def preview(self, ref: dict[str, Any], limit: int) -> pa.Table:
        """Fetch up to `limit` sample rows as an Arrow Table."""

    @abstractmethod
    def read(
        self,
        ref: dict[str, Any],
        mode: str,
        cursor_column: str | None,
        cursor_from: str | None,
        batch_rows: int = 50_000,
        stream_offsets: dict[str, str] | None = None,
    ) -> Iterator[ReadBatch]:
        """Stream source records in Arrow RecordBatches."""

    def close(self) -> None:
        """Release any open network connections or sessions."""
        return None


def validate_select_only_sql(query: str, dialect: str = "postgres") -> str:
    """Verify custom query is a single read-only SELECT statement via sqlglot AST."""
    cleaned = query.strip().rstrip(";").strip()
    if not cleaned:
        raise SidecarError(400, "SQL_REJECTED", "custom SQL query is empty")

    glot_dialect = {
        "postgres": "postgres",
        "kingbase": "postgres",
        "opengauss": "postgres",
        "mysql": "mysql",
        "tidb": "mysql",
        "oceanbase": "mysql",
        "doris": "doris",
        "starrocks": "starrocks",
        "mssql": "tsql",
        "oracle": "oracle",
        "dameng": "oracle",
        "sqlite": "sqlite",
        "clickhouse": "clickhouse",
        "hive": "hive",
    }.get(dialect.lower(), "postgres")

    try:
        statements = sqlglot.parse(cleaned, read=glot_dialect)
    except Exception as exc:
        raise SidecarError(400, "SQL_REJECTED", f"invalid SQL syntax: {exc}") from exc

    stmts = [s for s in statements if s is not None]
    if len(stmts) != 1:
        raise SidecarError(400, "SQL_REJECTED", "only a single SELECT statement is permitted")

    root = stmts[0]
    if not isinstance(root, (exp.Select, exp.Union)):
        raise SidecarError(400, "SQL_REJECTED", "custom SQL root must be SELECT")

    for node in root.walk():
        if isinstance(node, _FORBIDDEN_SQL_NODES):
            raise SidecarError(
                400,
                "SQL_REJECTED",
                f"forbidden SQL construct {type(node).__name__} in read-only connector",
            )

    return cleaned


def coerce_cursor_param(raw_cursor: str | None, cursor_type: DataType | None = None) -> Any:
    """Convert stringified cursor value back into typed bind parameter."""
    if raw_cursor is None:
        return None
    s = raw_cursor.strip()
    if cursor_type == "INT64" or (cursor_type is None and s.lstrip("-").isdigit()):
        try:
            return int(s)
        except ValueError:
            pass
    if cursor_type == "FLOAT64":
        try:
            return float(s)
        except ValueError:
            pass
    if cursor_type == "TIMESTAMP":
        try:
            norm = s[:-1] + "+00:00" if s.endswith("Z") else s
            return datetime.fromisoformat(norm)
        except ValueError:
            return s
    if cursor_type == "DATE":
        try:
            return date.fromisoformat(s[:10])
        except ValueError:
            return s
    return s


def format_cursor_value(val: Any) -> str | None:
    """Format observed max cursor value into canonical string representation."""
    if val is None:
        return None
    if isinstance(val, datetime):
        dt_utc = val.astimezone(UTC) if val.tzinfo is not None else val.replace(tzinfo=UTC)
        return dt_utc.isoformat().replace("+00:00", "Z")
    if isinstance(val, date):
        return val.isoformat()
    return str(val)


def build_select_sql(
    *,
    schema: str | None,
    table: str | None,
    custom_query: str | None,
    columns: Sequence[str] | None,
    mode: str,
    cursor_column: str | None,
    has_cursor_from: bool,
    dialect: str = "postgres",
    param_placeholder: str = ":cursor_from",
    limit: int | None = None,
) -> str:
    """Build parameterized read-only SELECT statement per Step 02 §02.4."""
    if custom_query:
        safe_sub = validate_select_only_sql(custom_query, dialect=dialect)
        from_clause = f"({safe_sub}) AS _ax_subq"
    else:
        if not table:
            raise SidecarError(400, "INVALID_ARGUMENT", "source_ref missing table or query")
        q_table = quote_sql_ident(table, dialect=dialect)
        if schema:
            q_schema = quote_sql_ident(schema, dialect=dialect)
            from_clause = f"{q_schema}.{q_table}"
        else:
            from_clause = q_table

    if columns:
        col_Projection = ", ".join(quote_sql_ident(c, dialect=dialect) for c in columns)
    else:
        col_Projection = "*"

    sql = f"SELECT {col_Projection} FROM {from_clause}"

    upper_mode = mode.upper()
    if upper_mode in {"APPEND", "UPSERT"} and cursor_column:
        validate_sql_ident(cursor_column)
        q_cur = quote_sql_ident(cursor_column, dialect=dialect)
        if has_cursor_from:
            op = ">=" if upper_mode == "UPSERT" else ">"
            sql += f" WHERE {q_cur} {op} {param_placeholder}"
        sql += f" ORDER BY {q_cur} ASC"

    if limit is not None and limit > 0:
        d_low = dialect.lower()
        if d_low in {"mssql", "tsql"}:
            sql = sql.replace("SELECT ", f"SELECT TOP {int(limit)} ", 1)
        elif d_low in {"oracle", "dameng"}:
            sql = f"SELECT * FROM ({sql}) WHERE ROWNUM <= {int(limit)}"
        else:
            sql += f" LIMIT {int(limit)}"

    return sql


def suggest_cursor_column(columns: Sequence[tuple[str, DataType]]) -> str | None:
    """Suggest an incremental cursor column based on timestamp/update naming heuristics."""
    preferred_names = (
        "updated_at",
        "modified_at",
        "gmt_modified",
        "update_time",
        "ts",
        "timestamp",
        "event_time",
        "created_at",
        "create_time",
    )
    lower_map = {name.lower(): name for name, _dt in columns}
    for cand in preferred_names:
        if cand in lower_map:
            return lower_map[cand]
    for name, dt in columns:
        if dt in {"TIMESTAMP", "DATE"}:
            return name
    return None


def normalize_py_value_for_arrow(val: Any) -> Any:
    """Normalize heterogeneous Python/DB-API values so PyArrow builds clean typed columns."""
    if val is None:
        return None
    if isinstance(val, bool):
        return val
    if isinstance(val, (int, str, bytes)):
        return val
    if isinstance(val, bytearray | memoryview):
        return bytes(val)
    if isinstance(val, float):
        return val if math.isfinite(val) else None
    if isinstance(val, Decimal):
        return val if val.is_finite() else None
    if isinstance(val, datetime):
        if val.tzinfo is None:
            return val.replace(tzinfo=UTC)
        return val.astimezone(UTC)
    if isinstance(val, date):
        return val
    if isinstance(val, (dict, list)):
        return orjson.dumps(val, default=str).decode("utf-8")
    return str(val)


def records_to_arrow_table(
    records: Sequence[dict[str, Any]],
    column_order: Sequence[str] | None = None,
) -> pa.Table:
    """Convert row dicts into a PyArrow Table with stable column ordering."""
    if not records and not column_order:
        return pa.table({})

    cols: list[str] = list(column_order) if column_order else []
    for rec in records:
        for k in rec:
            if k not in cols:
                cols.append(k)

    if not records:
        return pa.table({c: pa.array([], type=pa.string()) for c in cols})

    col_arrays: dict[str, pa.Array[Any]] = {}
    for col in cols:
        raw_vals = [normalize_py_value_for_arrow(r.get(col)) for r in records]
        try:
            col_arrays[col] = pa.array(raw_vals)
        except (pa.ArrowInvalid, pa.ArrowTypeError, OverflowError):
            str_vals = [None if v is None else str(v) for v in raw_vals]
            col_arrays[col] = pa.array(str_vals, type=pa.string())
    return pa.table(col_arrays)


def arrow_table_to_preview(
    table: pa.Table,
    *,
    limit: int = 100,
    timezone_str: str = "Asia/Shanghai",
) -> PreviewResponse:
    """Convert an Arrow Table into PreviewResponse with PII-masked rows and ColumnProfiles."""
    capped = table.slice(0, min(max(limit, 1), 1000))
    if capped.num_columns == 0:
        return PreviewResponse(columns=[], rows=[])

    pl_df = pl.from_arrow(capped)
    assert isinstance(pl_df, pl.DataFrame)
    profiles: list[ColumnProfile] = profile_polars_df(pl_df, timezone_str=timezone_str)
    pii_by_col = {p.name: p.pii_category for p in profiles}

    py_rows = capped.to_pylist()
    out_rows: list[dict[str, Any]] = []
    for raw_r in py_rows:
        serialized: dict[str, Any] = {}
        for col, val in raw_r.items():
            if val is None:
                serialized[col] = None
                continue
            pii_cat = pii_by_col.get(col, "NONE")
            if pii_cat != "NONE":
                serialized[col] = mask_pii_value(str(val), pii_cat)
            elif isinstance(val, datetime):
                dt_u = val.astimezone(UTC) if val.tzinfo else val.replace(tzinfo=UTC)
                serialized[col] = dt_u.isoformat().replace("+00:00", "Z")
            elif isinstance(val, date):
                serialized[col] = val.isoformat()
            elif isinstance(val, Decimal):
                serialized[col] = str(val)
            elif isinstance(val, (bytes, bytearray, memoryview)):
                serialized[col] = f"<binary:{len(val)}B>"
            elif isinstance(val, int) and abs(val) > (1 << 53) - 1:
                serialized[col] = str(val)
            else:
                serialized[col] = val
        out_rows.append(serialized)

    return PreviewResponse(columns=profiles, rows=out_rows)
