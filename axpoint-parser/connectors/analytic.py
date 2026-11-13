from __future__ import annotations

from collections.abc import Iterator
from typing import Any

import clickhouse_connect
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

CLICKHOUSE_SPEC = ConnectorSpecModel(
    connector_type="CLICKHOUSE",
    display_name="ClickHouse",
    category="ANALYTIC",
    config_schema={
        "type": "object",
        "required": ["host"],
        "properties": {
            "host": {"type": "string"},
            "port": {"type": "integer", "default": 8123},
            "database": {"type": "string", "default": "default"},
            "username": {"type": "string", "default": "default"},
            "secure": {"type": "boolean", "default": False},
        },
    },
    secret_fields=["password"],
    supports_modes=["SNAPSHOT", "APPEND", "UPSERT"],
    supports_discover=True,
    image_tier="lite",
)

DORIS_SPEC = ConnectorSpecModel(
    connector_type="DORIS",
    display_name="Apache Doris",
    category="ANALYTIC",
    config_schema={
        "type": "object",
        "required": ["host", "database"],
        "properties": {
            "host": {"type": "string"},
            "query_port": {"type": "integer", "default": 9030},
            "port": {"type": "integer", "default": 9030},
            "database": {"type": "string"},
            "username": {"type": "string", "default": "root"},
            "flight_port": {"type": "integer"},
        },
    },
    secret_fields=["password"],
    supports_modes=["SNAPSHOT", "APPEND", "UPSERT"],
    supports_discover=True,
    image_tier="lite",
)

STARROCKS_SPEC = ConnectorSpecModel(
    connector_type="STARROCKS",
    display_name="StarRocks",
    category="ANALYTIC",
    config_schema={
        "type": "object",
        "required": ["host", "database"],
        "properties": {
            "host": {"type": "string"},
            "query_port": {"type": "integer", "default": 9030},
            "port": {"type": "integer", "default": 9030},
            "database": {"type": "string"},
            "username": {"type": "string", "default": "root"},
            "flight_port": {"type": "integer"},
        },
    },
    secret_fields=["password"],
    supports_modes=["SNAPSHOT", "APPEND", "UPSERT"],
    supports_discover=True,
    image_tier="lite",
)

HIVE_SPEC = ConnectorSpecModel(
    connector_type="HIVE",
    display_name="Apache Hive",
    category="ANALYTIC",
    config_schema={
        "type": "object",
        "required": ["host"],
        "properties": {
            "host": {"type": "string"},
            "port": {"type": "integer", "default": 10000},
            "database": {"type": "string", "default": "default"},
            "username": {"type": "string"},
            "auth": {"type": "string", "default": "NONE"},
            "kerberos_service_name": {"type": "string"},
        },
    },
    secret_fields=["password", "keytab_b64"],
    supports_modes=["SNAPSHOT", "APPEND"],
    supports_discover=True,
    image_tier="full",
)


class ClickHouseConnector(BaseConnector):
    """ClickHouse analytical connector using clickhouse-connect Arrow streaming."""

    spec = CLICKHOUSE_SPEC

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
        port = int(self.config.get("port") or 8123)
        if not host:
            raise SidecarError(400, "INVALID_ARGUMENT", "ClickHouse config missing 'host'")
        check_outbound_host(host, port, allow_private_network=allow_private_network)
        self._client: Any = None

    def _get_client(self) -> Any:
        if self._client is None:
            self._client = clickhouse_connect.get_client(
                host=str(self.config["host"]),
                port=int(self.config.get("port") or 8123),
                username=str(self.config.get("username") or "default"),
                password=str(self.secrets.get("password") or ""),
                database=str(self.config.get("database") or "default"),
                secure=bool(self.config.get("secure", False)),
                connect_timeout=10,
                send_receive_timeout=min(max(int(self.config.get("timeout_s") or 60), 1), 600),
            )
        return self._client

    def test(self) -> tuple[bool, str]:
        try:
            cli = self._get_client()
            cli.command("SELECT 1")
            return True, "connected"
        except Exception as exc:
            return False, str(exc)

    def discover(self) -> list[DiscoveredObject]:
        cli = self._get_client()
        db = str(self.config.get("database") or "default")
        tbl_res = cli.query(
            "SELECT name, total_rows, primary_key FROM system.tables "
            "WHERE database = {db:String} AND not is_temporary ORDER BY name",
            parameters={"db": db},
        )
        col_res = cli.query(
            "SELECT table, name, type FROM system.columns "
            "WHERE database = {db:String} ORDER BY table, position",
            parameters={"db": db},
        )
        cols_by_tbl: dict[str, list[tuple[str, str]]] = {}
        for t_name, c_name, c_type in col_res.result_rows:
            cols_by_tbl.setdefault(str(t_name), []).append((str(c_name), str(c_type)))

        objects: list[DiscoveredObject] = []
        for t_name, total_rows, pk_expr in tbl_res.result_rows:
            t_str = str(t_name)
            try:
                validate_sql_ident(t_str)
            except SidecarError:
                continue
            disc_cols: list[DiscoveredColumn] = []
            typed_pairs: list[tuple[str, Any]] = []
            for c_name, c_type in cols_by_tbl.get(t_str, []):
                dt = map_native_type(c_type, dialect="clickhouse")
                disc_cols.append(
                    DiscoveredColumn(
                        name=c_name,
                        native_type=c_type,
                        data_type=dt,
                        nullable=c_type.lower().startswith("nullable("),
                    )
                )
                typed_pairs.append((c_name, dt))
            pks = [p.strip() for p in str(pk_expr or "").split(",") if p.strip()]
            objects.append(
                DiscoveredObject(
                    ref={"schema": db, "table": t_str},
                    display_name=f"{db}.{t_str}",
                    source_kind="SQL_TABLE",
                    columns=disc_cols,
                    primary_key=pks,
                    suggested_cursor=suggest_cursor_column(typed_pairs),
                    estimated_rows=int(total_rows) if total_rows is not None else None,
                )
            )
        return objects

    def preview(self, ref: dict[str, Any], limit: int) -> pa.Table:
        cli = self._get_client()
        schema, table, query = extract_schema_and_table(
            ref, str(self.config.get("database") or "default")
        )
        sql = build_select_sql(
            schema=schema,
            table=table,
            custom_query=query,
            columns=None,
            mode="SNAPSHOT",
            cursor_column=None,
            has_cursor_from=False,
            dialect="clickhouse",
            limit=min(max(limit, 1), 1000),
        )
        return cli.query_arrow(sql)

    def read(
        self,
        ref: dict[str, Any],
        mode: str,
        cursor_column: str | None,
        cursor_from: str | None,
        batch_rows: int = 50_000,
        stream_offsets: dict[str, str] | None = None,
    ) -> Iterator[ReadBatch]:
        cli = self._get_client()
        schema, table, query = extract_schema_and_table(
            ref, str(self.config.get("database") or "default")
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
            dialect="clickhouse",
            param_placeholder="%(cursor_from)s",
        )
        params = {"cursor_from": coerce_cursor_param(cursor_from)} if has_cur else None
        step = max(batch_rows, 1)
        with cli.query_arrow_stream(sql, parameters=params) as stream:
            for arrow_chunk in stream:
                tbl = (
                    arrow_chunk
                    if isinstance(arrow_chunk, pa.Table)
                    else pa.Table.from_batches([arrow_chunk])
                )
                max_c: str | None = None
                if cursor_column and cursor_column in tbl.column_names and tbl.num_rows > 0:
                    col_vals = [v for v in tbl[cursor_column].to_pylist() if v is not None]
                    if col_vals:
                        max_c = format_cursor_value(col_vals[-1])
                for rb in tbl.to_batches(max_chunksize=step):
                    yield ReadBatch(batch=rb, cursor_to=max_c)

    def close(self) -> None:
        if self._client is not None:
            self._client.close()
            self._client = None


class DorisConnector(SqlAlchemyFamilyConnector):
    spec = DORIS_SPEC
    dialect_name = "mysql"
    default_port = 9030
    default_schema_name = None

    def __init__(
        self,
        config: dict[str, Any],
        secrets: dict[str, str],
        *,
        allow_private_network: bool = False,
        tenant_id: str = "default",
    ) -> None:
        merged = dict(config)
        if "query_port" in merged and "port" not in merged:
            merged["port"] = merged["query_port"]
        super().__init__(
            merged,
            secrets,
            allow_private_network=allow_private_network,
            tenant_id=tenant_id,
        )


class StarRocksConnector(DorisConnector):
    spec = STARROCKS_SPEC


class HiveConnector(BaseConnector):
    """Apache HiveServer2 connector via pyhive (full image tier)."""

    spec = HIVE_SPEC

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
        port = int(self.config.get("port") or 10000)
        if not host:
            raise SidecarError(400, "INVALID_ARGUMENT", "Hive config missing 'host'")
        check_outbound_host(host, port, allow_private_network=allow_private_network)

    def _connect(self) -> Any:
        try:
            from pyhive import hive
        except ImportError as exc:
            raise SidecarError(
                422,
                "CONNECTOR_DRIVER_UNAVAILABLE",
                "pyhive driver is not available in lite image",
            ) from exc

        auth = str(self.config.get("auth") or "NONE").upper()
        kwargs: dict[str, Any] = {
            "host": str(self.config["host"]),
            "port": int(self.config.get("port") or 10000),
            "database": str(self.config.get("database") or "default"),
            "auth": auth,
        }
        if self.config.get("username"):
            kwargs["username"] = str(self.config["username"])
        if auth == "LDAP" and self.secrets.get("password"):
            kwargs["password"] = str(self.secrets["password"])
        if auth == "KERBEROS" and self.config.get("kerberos_service_name"):
            kwargs["kerberos_service_name"] = str(self.config["kerberos_service_name"])
        return hive.connect(**kwargs)

    def test(self) -> tuple[bool, str]:
        try:
            conn = self._connect()
            try:
                cur = conn.cursor()
                cur.execute("SELECT 1")
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
            cur.execute("SHOW TABLES")
            tables = [str(r[0]) for r in cur.fetchall()]
            out: list[DiscoveredObject] = []
            for t_name in tables:
                try:
                    validate_sql_ident(t_name)
                except SidecarError:
                    continue
                cur.execute(f"DESCRIBE `{t_name}`")
                cols: list[DiscoveredColumn] = []
                typed_pairs: list[tuple[str, Any]] = []
                for row in cur.fetchall():
                    c_name = str(row[0] or "").strip()
                    c_type = str(row[1] or "").strip()
                    if not c_name or c_name.startswith("#"):
                        break
                    dt = map_native_type(c_type, dialect="hive")
                    cols.append(
                        DiscoveredColumn(
                            name=c_name,
                            native_type=c_type,
                            data_type=dt,
                            nullable=True,
                        )
                    )
                    typed_pairs.append((c_name, dt))
                out.append(
                    DiscoveredObject(
                        ref={"table": t_name},
                        display_name=t_name,
                        source_kind="SQL_TABLE",
                        columns=cols,
                        primary_key=[],
                        suggested_cursor=suggest_cursor_column(typed_pairs),
                        estimated_rows=None,
                    )
                )
            return out
        finally:
            conn.close()

    def preview(self, ref: dict[str, Any], limit: int) -> pa.Table:
        _schema, table, query = extract_schema_and_table(ref, None)
        sql = build_select_sql(
            schema=None,
            table=table,
            custom_query=query,
            columns=None,
            mode="SNAPSHOT",
            cursor_column=None,
            has_cursor_from=False,
            dialect="hive",
            limit=min(max(limit, 1), 1000),
        )
        conn = self._connect()
        try:
            cur = conn.cursor()
            cur.execute(sql)
            cols = [str(d[0]).split(".")[-1] for d in (cur.description or [])]
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
        _schema, table, query = extract_schema_and_table(ref, None)
        has_cur = cursor_from is not None and cursor_column is not None
        sql = build_select_sql(
            schema=None,
            table=table,
            custom_query=query,
            columns=None,
            mode=mode,
            cursor_column=cursor_column,
            has_cursor_from=has_cur,
            dialect="hive",
            param_placeholder="%(cursor_from)s",
        )
        conn = self._connect()
        try:
            cur = conn.cursor()
            if has_cur:
                cur.execute(sql, {"cursor_from": coerce_cursor_param(cursor_from)})
            else:
                cur.execute(sql)
            cols = [str(d[0]).split(".")[-1] for d in (cur.description or [])]
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
