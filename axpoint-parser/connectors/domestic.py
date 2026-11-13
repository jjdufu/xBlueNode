from __future__ import annotations

from collections.abc import Iterator
from typing import Any

import pyarrow as pa

from connectors.base import (
    BaseConnector,
    ReadBatch,
    build_select_sql,
    coerce_cursor_param,
    format_cursor_value,
    records_to_arrow_table,
    suggest_cursor_column,
)
from connectors.sql_family import SqlAlchemyFamilyConnector, extract_schema_and_table
from connectors.typemap import map_native_type
from schemas import ConnectorSpecModel, DiscoveredColumn, DiscoveredObject
from security import SidecarError, check_outbound_host, validate_sql_ident

DAMENG_SPEC = ConnectorSpecModel(
    connector_type="DAMENG",
    display_name="达梦数据库 DM8",
    category="DOMESTIC",
    config_schema={
        "type": "object",
        "required": ["host"],
        "properties": {
            "host": {"type": "string"},
            "port": {"type": "integer", "default": 5236},
            "username": {"type": "string", "default": "SYSDBA"},
            "schema": {"type": "string"},
        },
    },
    secret_fields=["password"],
    supports_modes=["SNAPSHOT", "APPEND", "UPSERT"],
    supports_discover=True,
    image_tier="full",
)

KINGBASE_SPEC = ConnectorSpecModel(
    connector_type="KINGBASE",
    display_name="人大金仓 KingbaseES",
    category="DOMESTIC",
    config_schema={
        "type": "object",
        "required": ["host", "database"],
        "properties": {
            "host": {"type": "string"},
            "port": {"type": "integer", "default": 54321},
            "database": {"type": "string"},
            "username": {"type": "string", "default": "system"},
            "schema": {"type": "string", "default": "public"},
        },
    },
    secret_fields=["password"],
    supports_modes=["SNAPSHOT", "APPEND", "UPSERT"],
    supports_discover=True,
    image_tier="lite",
)

OPENGAUSS_SPEC = ConnectorSpecModel(
    connector_type="OPENGAUSS",
    display_name="openGauss",
    category="DOMESTIC",
    config_schema={
        "type": "object",
        "required": ["host", "database"],
        "properties": {
            "host": {"type": "string"},
            "port": {"type": "integer", "default": 5432},
            "database": {"type": "string"},
            "username": {"type": "string"},
            "schema": {"type": "string", "default": "public"},
        },
    },
    secret_fields=["password"],
    supports_modes=["SNAPSHOT", "APPEND", "UPSERT"],
    supports_discover=True,
    image_tier="lite",
)

OCEANBASE_SPEC = ConnectorSpecModel(
    connector_type="OCEANBASE",
    display_name="OceanBase (MySQL Mode)",
    category="DOMESTIC",
    config_schema={
        "type": "object",
        "required": ["host", "database"],
        "properties": {
            "host": {"type": "string"},
            "port": {"type": "integer", "default": 2881},
            "database": {"type": "string"},
            "tenant_user": {"type": "string"},
            "username": {"type": "string"},
        },
    },
    secret_fields=["password"],
    supports_modes=["SNAPSHOT", "APPEND", "UPSERT"],
    supports_discover=True,
    image_tier="lite",
)

TIDB_SPEC = ConnectorSpecModel(
    connector_type="TIDB",
    display_name="TiDB",
    category="DOMESTIC",
    config_schema={
        "type": "object",
        "required": ["host", "database"],
        "properties": {
            "host": {"type": "string"},
            "port": {"type": "integer", "default": 4000},
            "database": {"type": "string"},
            "username": {"type": "string", "default": "root"},
        },
    },
    secret_fields=["password"],
    supports_modes=["SNAPSHOT", "APPEND", "UPSERT"],
    supports_discover=True,
    image_tier="lite",
)


class DamengConnector(BaseConnector):
    """Dameng DM8 connector using dmPython DB-API driver (full image tier)."""

    spec = DAMENG_SPEC

    def __init__(
        self,
        config: dict[str, Any],
        secrets: dict[str, str],
        *,
        allow_private_network: bool = False,
        tenant_id: str = "default",
    ) -> None:
        super().__init__(
            config,
            secrets,
            allow_private_network=allow_private_network,
            tenant_id=tenant_id,
        )
        host = str(self.config.get("host") or "").strip()
        port = int(self.config.get("port") or 5236)
        if not host:
            raise SidecarError(400, "INVALID_ARGUMENT", "Dameng config missing 'host'")
        check_outbound_host(host, port, allow_private_network=allow_private_network)

    def _connect(self) -> Any:
        try:
            import dmPython
        except ImportError as exc:
            raise SidecarError(
                422,
                "CONNECTOR_DRIVER_UNAVAILABLE",
                "dmPython driver is not available in lite image",
            ) from exc

        return dmPython.connect(
            user=str(self.config.get("username") or "SYSDBA"),
            password=str(self.secrets.get("password") or ""),
            server=str(self.config["host"]),
            port=int(self.config.get("port") or 5236),
        )

    def test(self) -> tuple[bool, str]:
        try:
            conn = self._connect()
            try:
                cur = conn.cursor()
                cur.execute("SELECT 1 FROM DUAL")
                cur.fetchone()
            finally:
                conn.close()
            return True, "connected"
        except SidecarError:
            raise
        except Exception as exc:
            return False, str(exc)

    def discover(self) -> list[DiscoveredObject]:
        conn = self._connect()
        try:
            cur = conn.cursor()
            owner = str(
                self.config.get("schema") or self.config.get("username") or "SYSDBA"
            ).upper()
            cur.execute(
                "SELECT TABLE_NAME, NUM_ROWS FROM ALL_TABLES WHERE OWNER = ? ORDER BY TABLE_NAME",
                (owner,),
            )
            tables = cur.fetchall()
            out: list[DiscoveredObject] = []
            for tbl_name, num_rows in tables:
                t_str = str(tbl_name)
                try:
                    validate_sql_ident(t_str)
                except SidecarError:
                    continue
                cur.execute(
                    "SELECT COLUMN_NAME, DATA_TYPE, DATA_PRECISION, DATA_SCALE, NULLABLE "
                    "FROM ALL_TAB_COLUMNS WHERE OWNER = ? AND TABLE_NAME = ? ORDER BY COLUMN_ID",
                    (owner, t_str),
                )
                cols: list[DiscoveredColumn] = []
                typed_pairs: list[tuple[str, Any]] = []
                for c_name, d_type, prec, scale, nullable in cur.fetchall():
                    native = f"{d_type}({prec},{scale or 0})" if prec is not None else str(d_type)
                    mapped = map_native_type(native, dialect="dameng")
                    cols.append(
                        DiscoveredColumn(
                            name=str(c_name),
                            native_type=native,
                            data_type=mapped,
                            nullable=(str(nullable) == "Y"),
                        )
                    )
                    typed_pairs.append((str(c_name), mapped))
                out.append(
                    DiscoveredObject(
                        ref={"schema": owner, "table": t_str},
                        display_name=f"{owner}.{t_str}",
                        source_kind="SQL_TABLE",
                        columns=cols,
                        primary_key=[],
                        suggested_cursor=suggest_cursor_column(typed_pairs),
                        estimated_rows=int(num_rows) if num_rows is not None else None,
                    )
                )
            return out
        finally:
            conn.close()

    def preview(self, ref: dict[str, Any], limit: int) -> pa.Table:
        schema, table, query = extract_schema_and_table(
            ref, str(self.config.get("schema") or "") or None
        )
        sql = build_select_sql(
            schema=schema,
            table=table,
            custom_query=query,
            columns=None,
            mode="SNAPSHOT",
            cursor_column=None,
            has_cursor_from=False,
            dialect="dameng",
            limit=min(max(limit, 1), 1000),
        )
        conn = self._connect()
        try:
            cur = conn.cursor()
            cur.execute(sql)
            cols = [d[0] for d in (cur.description or [])]
            rows = [dict(zip(cols, r, strict=False)) for r in cur.fetchall()]
            return records_to_arrow_table(rows, column_order=cols)
        finally:
            conn.close()

    def read(
        self,
        ref: dict[str, Any],
        mode: str,
        cursor_column: str | None,
        cursor_from: str | None,
        batch_rows: int = 50_000,
        stream_offsets: dict[str, str] | None = None,
    ) -> Iterator[ReadBatch]:
        schema, table, query = extract_schema_and_table(
            ref, str(self.config.get("schema") or "") or None
        )
        has_cur = cursor_from is not None and cursor_column is not None
        sql = build_select_sql(
            schema=schema,
            table=table,
            custom_query=query,
            columns=None,
            mode=mode,
            cursor_column=cursor_column,
            has_cursor_from=has_cur,
            dialect="dameng",
            param_placeholder="?",
        )
        conn = self._connect()
        try:
            cur = conn.cursor()
            if has_cur:
                cur.execute(sql, (coerce_cursor_param(cursor_from),))
            else:
                cur.execute(sql)
            cols = [d[0] for d in (cur.description or [])]
            step = max(batch_rows, 1)
            while True:
                chunk = cur.fetchmany(step)
                if not chunk:
                    break
                dicts = [dict(zip(cols, r, strict=False)) for r in chunk]
                max_c: str | None = None
                if cursor_column:
                    vals = [d.get(cursor_column) for d in dicts if d.get(cursor_column) is not None]
                    if vals:
                        max_c = format_cursor_value(vals[-1])
                pa_tbl = records_to_arrow_table(dicts, column_order=cols)
                for rb in pa_tbl.to_batches(max_chunksize=step):
                    yield ReadBatch(batch=rb, cursor_to=max_c)
        finally:
            conn.close()


class KingbaseConnector(SqlAlchemyFamilyConnector):
    spec = KINGBASE_SPEC
    dialect_name = "postgres"
    default_port = 54321
    default_schema_name = "public"


class OpenGaussConnector(SqlAlchemyFamilyConnector):
    spec = OPENGAUSS_SPEC
    dialect_name = "postgres"
    default_port = 5432
    default_schema_name = "public"


class OceanBaseConnector(SqlAlchemyFamilyConnector):
    spec = OCEANBASE_SPEC
    dialect_name = "mysql"
    default_port = 2881
    default_schema_name = None

    def __init__(
        self,
        config: dict[str, Any],
        secrets: dict[str, str],
        *,
        allow_private_network: bool = False,
        tenant_id: str = "default",
    ) -> None:
        merged_cfg = dict(config)
        if "tenant_user" in merged_cfg and "username" not in merged_cfg:
            merged_cfg["username"] = merged_cfg["tenant_user"]
        super().__init__(
            merged_cfg,
            secrets,
            allow_private_network=allow_private_network,
            tenant_id=tenant_id,
        )


class TiDbConnector(SqlAlchemyFamilyConnector):
    spec = TIDB_SPEC
    dialect_name = "mysql"
    default_port = 4000
    default_schema_name = None
