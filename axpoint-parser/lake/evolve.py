from __future__ import annotations

from collections.abc import Sequence

import duckdb

from lake.catalog import quote_lake_col
from schemas import ColumnProfile, DataType, SchemaChange, SchemaPolicy
from security import SidecarError

_LOSSLESS_WIDEN_PAIRS: frozenset[tuple[DataType, DataType]] = frozenset(
    {
        ("INT64", "DECIMAL"),
        ("INT64", "FLOAT64"),
        ("DECIMAL", "FLOAT64"),
        ("DATE", "TIMESTAMP"),
    }
)


class SchemaDriftBlockedError(SidecarError):
    """Raised when incoming batch schema violates dataset SchemaPolicy."""

    def __init__(self, message: str, schema_change: SchemaChange) -> None:
        super().__init__(
            status_code=422,
            error_code="SCHEMA_DRIFT_BLOCKED",
            message=message,
        )
        self.schema_change = schema_change


def is_lossless_widening(old_type: DataType, new_type: DataType) -> bool:
    """Return True if converting old_type -> new_type is lossless per Step 02 §02.9.3."""
    if old_type == new_type:
        return False
    if new_type == "STRING":
        return True
    return (old_type, new_type) in _LOSSLESS_WIDEN_PAIRS


def datatype_to_duckdb_sql(dt: str) -> str:
    """Map canonical AxPoint DataType to DuckLake SQL column type."""
    mapping: dict[str, str] = {
        "STRING": "VARCHAR",
        "INT64": "BIGINT",
        "FLOAT64": "DOUBLE",
        "DECIMAL": "DECIMAL(38, 6)",
        "BOOLEAN": "BOOLEAN",
        "DATE": "DATE",
        "TIMESTAMP": "TIMESTAMPTZ",
        "BINARY": "BLOB",
        "JSON": "VARCHAR",
        "LIST": "VARCHAR[]",
        "STRUCT": "VARCHAR",
    }
    return mapping.get(dt, "VARCHAR")


def duckdb_type_to_datatype(duck_type: str) -> DataType:
    """Map DuckDB information_schema data_type back to canonical DataType."""
    t = duck_type.strip().upper()
    if t.endswith("[]") or t.startswith("LIST"):
        return "LIST"
    if t.startswith("STRUCT"):
        return "STRUCT"
    if t == "BOOLEAN":
        return "BOOLEAN"
    if "INT" in t and not t.startswith("INTERVAL"):
        return "INT64"
    if t.startswith(("DECIMAL", "NUMERIC")):
        return "DECIMAL"
    if t in {"FLOAT", "REAL", "DOUBLE"}:
        return "FLOAT64"
    if t == "DATE":
        return "DATE"
    if t.startswith("TIMESTAMP"):
        return "TIMESTAMP"
    if t in {"BLOB", "BYTEA", "BINARY", "VARBINARY"}:
        return "BINARY"
    if t == "JSON":
        return "JSON"
    return "STRING"


def evaluate_schema_evolution(
    expected_schema: Sequence[ColumnProfile],
    incoming_schema: Sequence[ColumnProfile],
    policy: SchemaPolicy,
) -> tuple[SchemaChange, list[ColumnProfile]]:
    """Compare incoming column profiles against expected_schema under policy.

    Returns (schema_change, effective_target_schema).
    Raises SchemaDriftBlockedError (422 SCHEMA_DRIFT_BLOCKED) when policy forbids the drift.
    """
    user_expected = [c for c in expected_schema if not c.name.startswith("_ax_")]
    user_incoming = [c for c in incoming_schema if not c.name.startswith("_ax_")]

    if not user_expected:
        return SchemaChange(), list(user_incoming)

    exp_by_name = {c.name: c for c in user_expected}
    inc_by_name = {c.name: c for c in user_incoming}

    added: list[ColumnProfile] = [c for c in user_incoming if c.name not in exp_by_name]
    removed: list[str] = [c.name for c in user_expected if c.name not in inc_by_name]
    widened: list[tuple[str, str, str]] = []
    widened_typed: dict[str, DataType] = {}
    narrowed: list[tuple[str, str, str]] = []

    for col_name, exp_col in exp_by_name.items():
        inc_col = inc_by_name.get(col_name)
        if inc_col is None or exp_col.data_type == inc_col.data_type:
            continue
        if is_lossless_widening(exp_col.data_type, inc_col.data_type):
            widened.append((col_name, exp_col.data_type, inc_col.data_type))
            widened_typed[col_name] = inc_col.data_type
        else:
            narrowed.append((col_name, exp_col.data_type, inc_col.data_type))

    has_any_drift = bool(added or removed or widened or narrowed)
    breaking = False
    if policy == "STRICT" and has_any_drift:
        breaking = True
    elif policy == "ADDITIVE" and (removed or narrowed):
        breaking = True

    change = SchemaChange(
        added=added,
        removed=removed,
        widened=widened,
        narrowed=narrowed,
        breaking=breaking,
    )

    if breaking:
        reasons: list[str] = []
        if added:
            reasons.append(f"added={[c.name for c in added]}")
        if removed:
            reasons.append(f"removed={removed}")
        if widened:
            reasons.append(f"widened={widened}")
        if narrowed:
            reasons.append(f"narrowed={narrowed}")
        raise SchemaDriftBlockedError(
            f"schema drift blocked by {policy} policy: {', '.join(reasons)}",
            schema_change=change,
        )

    narrowed_set = {col for col, _old, _new in narrowed}
    resolved: list[ColumnProfile] = []

    for exp_col in user_expected:
        inc_col = inc_by_name.get(exp_col.name)
        if inc_col is None:
            resolved.append(exp_col.model_copy(update={"ordinal": len(resolved), "nullable": True}))
        elif exp_col.name in narrowed_set:
            resolved.append(
                inc_col.model_copy(
                    update={"ordinal": len(resolved), "data_type": "STRING", "nullable": True}
                )
            )
        elif exp_col.name in widened_typed:
            resolved.append(
                inc_col.model_copy(
                    update={
                        "ordinal": len(resolved),
                        "data_type": widened_typed[exp_col.name],
                        "nullable": exp_col.nullable or inc_col.nullable,
                    }
                )
            )
        else:
            resolved.append(
                inc_col.model_copy(
                    update={
                        "ordinal": len(resolved),
                        "nullable": exp_col.nullable or inc_col.nullable,
                    }
                )
            )

    for add_col in added:
        resolved.append(add_col.model_copy(update={"ordinal": len(resolved), "nullable": True}))

    return change, resolved


def apply_ddl_evolution_to_table(
    con: duckdb.DuckDBPyConnection,
    full_table_sql: str,
    change: SchemaChange,
    policy: SchemaPolicy,
) -> None:
    """Apply ADD COLUMN and ALTER COLUMN TYPE DDL on an existing DuckLake table."""
    for add_col in change.added:
        q_col = quote_lake_col(add_col.name)
        sql_t = datatype_to_duckdb_sql(add_col.data_type)
        con.execute(f"ALTER TABLE {full_table_sql} ADD COLUMN {q_col} {sql_t}")

    for col_name, _old_t, new_t in change.widened:
        q_col = quote_lake_col(col_name)
        sql_t = datatype_to_duckdb_sql(new_t)
        con.execute(f"ALTER TABLE {full_table_sql} ALTER COLUMN {q_col} TYPE {sql_t}")

    if policy == "EVOLVE":
        for col_name, _old_t, _new_t in change.narrowed:
            q_col = quote_lake_col(col_name)
            con.execute(f"ALTER TABLE {full_table_sql} ALTER COLUMN {q_col} TYPE VARCHAR")
