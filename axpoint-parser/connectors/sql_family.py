from __future__ import annotations

import sqlite3
from collections.abc import Iterator
from typing import Any
from urllib.parse import quote_plus

import pyarrow as pa
from sqlalchemy import create_engine, inspect, text
from sqlalchemy.engine import Engine

from connectors.base import (
    BaseConnector,
    ReadBatch,
    build_select_sql,
    coerce_cursor_param,
    format_cursor_value,
    records_to_arrow_table,
    suggest_cursor_column,
)
from connectors.typemap import map_native_type
from schemas import ConnectorSpecModel, DiscoveredColumn, DiscoveredObject
from security import (
    SidecarError,
    check_outbound_host,
    resolve_path_under_root,
    validate_sql_ident,
)
from settings import get_data_root

POSTGRES_SPEC = ConnectorSpecModel(
    connector_type="POSTGRES",
    display_name="PostgreSQL",
    category="RELATIONAL",
    config_schema={
        "type": "object",
        "required": ["host", "database"],
        "properties": {
            "host": {"type": "string"},
            "port": {"type": "integer", "default": 5432},
            "database": {"type": "string"},
            "username": {"type": "string", "default": "postgres"},
            "schema": {"type": "string", "default": "public"},
            "sslmode": {"type": "string", "default": "prefer"},
        },
    },
    secret_fields=["password"],
    supports_modes=["SNAPSHOT", "APPEND", "UPSERT"],
    supports_discover=True,
    image_tier="lite",
)

MYSQL_SPEC = ConnectorSpecModel(
    connector_type="MYSQL",
    display_name="MySQL",
    category="RELATIONAL",
    config_schema={
        "type": "object",
        "required": ["host", "database"],
        "properties": {
            "host": {"type": "string"},
            "port": {"type": "integer", "default": 3306},
            "database": {"type": "string"},
            "username": {"type": "string", "default": "root"},
            "charset": {"type": "string", "default": "utf8mb4"},
            "ssl": {"type": "boolean", "default": False},
        },
    },
    secret_fields=["password"],
    supports_modes=["SNAPSHOT", "APPEND", "UPSERT"],
    supports_discover=True,
    image_tier="lite",
)

MSSQL_SPEC = ConnectorSpecModel(
    connector_type="MSSQL",
    display_name="SQL Server",
    category="RELATIONAL",
    config_schema={
        "type": "object",
        "required": ["host", "database"],
        "properties": {
            "host": {"type": "string"},
            "port": {"type": "integer", "default": 1433},
            "database": {"type": "string"},
            "username": {"type": "string", "default": "sa"},
            "schema": {"type": "string", "default": "dbo"},
            "encrypt": {"type": "boolean", "default": False},
        },
    },
    secret_fields=["password"],
    supports_modes=["SNAPSHOT", "APPEND", "UPSERT"],
    supports_discover=True,
    image_tier="full",
)

ORACLE_SPEC = ConnectorSpecModel(
    connector_type="ORACLE",
    display_name="Oracle Database",
    category="RELATIONAL",
    config_schema={
        "type": "object",
        "required": ["host", "service_name"],
        "properties": {
            "host": {"type": "string"},
            "port": {"type": "integer", "default": 1521},
            "service_name": {"type": "string"},
            "username": {"type": "string"},
            "schema": {"type": "string"},
        },
    },
    secret_fields=["password"],
    supports_modes=["SNAPSHOT", "APPEND", "UPSERT"],
    supports_discover=True,
    image_tier="full",
)

SQLITE_SPEC = ConnectorSpecModel(
    connector_type="SQLITE",
    display_name="SQLite",
    category="RELATIONAL",
    config_schema={
        "type": "object",
        "required": ["path"],
        "properties": {
            "path": {"type": "string"},
        },
    },
    secret_fields=[],
    supports_modes=["SNAPSHOT", "APPEND", "UPSERT"],
    supports_discover=True,
    image_tier="lite",
)


def extract_schema_and_table(
    ref: dict[str, Any], default_schema: str | None
) -> tuple[str | None, str | None, str | None]:
    """Extract (schema, table, custom_query) from source_ref flexibly."""
    query = ref.get("query")
    if isinstance(query, str) and query.strip():
        return None, None, query.strip()

    raw_table = str(ref.get("table") or ref.get("name") or "").strip()
    raw_schema = str(ref.get("schema") or "").strip()

    if "." in raw_table and not raw_schema:
        parts = raw_table.split(".", 1)
        raw_schema, raw_table = parts[0].strip(), parts[1].strip()

    schema = raw_schema or (default_schema.strip() if default_schema else None)
    return schema, (raw_table or None), None


class SqlAlchemyFamilyConnector(BaseConnector):
    """Shared read-only SQLAlchemy connector for POSTGRES, MYSQL, MSSQL, ORACLE."""

    dialect_name: str = "postgres"
    default_port: int = 5432
    default_schema_name: str | None = "public"

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
        port = int(self.config.get("port") or self.default_port)
        if not host:
            raise SidecarError(400, "INVALID_ARGUMENT", "missing database host")
        check_outbound_host(host, port, allow_private_network=allow_private_network)
        self._engine: Engine | None = None

    def _build_sqlalchemy_url(self) -> str:
        host = str(self.config["host"]).strip()
        port = int(self.config.get("port") or self.default_port)
        user = str(
            self.config.get("username")
            or self.config.get("user")
            or self.secrets.get("username")
            or ""
        )
        pw = str(self.secrets.get("password") or self.config.get("password") or "")
        auth = f"{quote_plus(user)}:{quote_plus(pw)}@" if user else ""

        if self.dialect_name == "postgres":
            db = quote_plus(str(self.config.get("database") or "postgres"))
            sslmode = quote_plus(str(self.config.get("sslmode") or "prefer"))
            return f"postgresql+psycopg://{auth}{host}:{port}/{db}?sslmode={sslmode}"

        if self.dialect_name == "mysql":
            db = quote_plus(str(self.config.get("database") or ""))
            charset = quote_plus(str(self.config.get("charset") or "utf8mb4"))
            return f"mysql+pymysql://{auth}{host}:{port}/{db}?charset={charset}"

        if self.dialect_name == "mssql":
            db = quote_plus(str(self.config.get("database") or "master"))
            return f"mssql+pymssql://{auth}{host}:{port}/{db}"

        if self.dialect_name == "oracle":
            svc = quote_plus(
                str(self.config.get("service_name") or self.config.get("database") or "FREEPDB1")
            )
            return f"oracle+oracledb://{auth}{host}:{port}/?service_name={svc}"

        raise SidecarError(400, "INVALID_ARGUMENT", f"unsupported SQL dialect {self.dialect_name}")

    def _get_engine(self) -> Engine:
        if self._engine is None:
            timeout_s = min(max(int(self.config.get("timeout_s") or 10), 1), 600)
            connect_args: dict[str, Any] = {}
            if self.dialect_name == "postgres":
                connect_args["connect_timeout"] = min(timeout_s, 10)
                connect_args["options"] = "-c default_transaction_read_only=on"
            elif self.dialect_name == "mysql":
                connect_args["connect_timeout"] = min(timeout_s, 10)
                connect_args["read_timeout"] = timeout_s
            self._engine = create_engine(
                self._build_sqlalchemy_url(),
                pool_pre_ping=True,
                connect_args=connect_args,
            )
        return self._engine

    def test(self) -> tuple[bool, str]:
        probe = "SELECT 1 FROM DUAL" if self.dialect_name == "oracle" else "SELECT 1"
        try:
            eng = self._get_engine()
            with eng.connect() as conn:
                conn.execute(text(probe))
            return True, "connected"
        except Exception as exc:
            return False, str(exc)

    def discover(self) -> list[DiscoveredObject]:
        eng = self._get_engine()
        insp = inspect(eng)
        cfg_schema = str(self.config.get("schema") or "").strip() or self.default_schema_name
        if self.dialect_name == "mysql":
            cfg_schema = str(self.config.get("database") or "").strip() or None

        row_estimates: dict[str, int] = {}
        try:
            with eng.connect() as conn:
                if self.dialect_name == "postgres" and cfg_schema:
                    rows = conn.execute(
                        text(
                            "SELECT c.relname, GREATEST(c.reltuples::bigint, 0) "
                            "FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace "
                            "WHERE n.nspname = :s AND c.relkind IN ('r', 'v', 'm', 'p')"
                        ),
                        {"s": cfg_schema},
                    ).fetchall()
                    row_estimates = {str(r[0]): int(r[1]) for r in rows}
                elif self.dialect_name == "mysql" and cfg_schema:
                    rows = conn.execute(
                        text(
                            "SELECT TABLE_NAME, COALESCE(TABLE_ROWS, 0) "
                            "FROM information_schema.TABLES WHERE TABLE_SCHEMA = :s"
                        ),
                        {"s": cfg_schema},
                    ).fetchall()
                    row_estimates = {str(r[0]): int(r[1]) for r in rows}
        except Exception:
            row_estimates = {}

        names: list[str] = list(insp.get_table_names(schema=cfg_schema))
        try:
            names.extend(insp.get_view_names(schema=cfg_schema))
        except Exception:
            pass

        objects: list[DiscoveredObject] = []
        for tbl_name in names:
            try:
                validate_sql_ident(tbl_name)
            except SidecarError:
                continue
            raw_cols = insp.get_columns(tbl_name, schema=cfg_schema)
            pk_info = insp.get_pk_constraint(tbl_name, schema=cfg_schema)
            pks = [str(c) for c in (pk_info.get("constrained_columns") or [])]

            disc_cols: list[DiscoveredColumn] = []
            typed_pairs: list[tuple[str, Any]] = []
            for rc in raw_cols:
                c_name = str(rc["name"])
                native_t = str(rc["type"])
                mapped_dt = map_native_type(native_t, dialect=self.dialect_name)
                disc_cols.append(
                    DiscoveredColumn(
                        name=c_name,
                        native_type=native_t,
                        data_type=mapped_dt,
                        nullable=bool(rc.get("nullable", True)),
                    )
                )
                typed_pairs.append((c_name, mapped_dt))

            ref_obj: dict[str, Any] = {"table": tbl_name}
            if cfg_schema and self.dialect_name != "mysql":
                ref_obj["schema"] = cfg_schema
            disp = f"{cfg_schema}.{tbl_name}" if cfg_schema else tbl_name

            objects.append(
                DiscoveredObject(
                    ref=ref_obj,
                    display_name=disp,
                    source_kind="SQL_TABLE",
                    columns=disc_cols,
                    primary_key=pks,
                    suggested_cursor=suggest_cursor_column(typed_pairs),
                    estimated_rows=row_estimates.get(tbl_name),
                )
            )
        return objects

    def preview(self, ref: dict[str, Any], limit: int) -> pa.Table:
        capped = min(max(limit, 1), 1000)
        default_s = (
            None
            if self.dialect_name == "mysql"
            else (str(self.config.get("schema") or "").strip() or self.default_schema_name)
        )
        schema, table, query = extract_schema_and_table(ref, default_s)
        sql = build_select_sql(
            schema=schema,
            table=table,
            custom_query=query,
            columns=None,
            mode="SNAPSHOT",
            cursor_column=None,
            has_cursor_from=False,
            dialect=self.dialect_name,
            limit=capped,
        )
        eng = self._get_engine()
        with eng.connect() as conn:
            res = conn.execute(text(sql))
            col_names = list(res.keys())
            rows = [dict(zip(col_names, r, strict=False)) for r in res.fetchall()]
        return records_to_arrow_table(rows, column_order=col_names)

    def read(
        self,
        ref: dict[str, Any],
        mode: str,
        cursor_column: str | None,
        cursor_from: str | None,
        batch_rows: int = 50_000,
        stream_offsets: dict[str, str] | None = None,
    ) -> Iterator[ReadBatch]:
        default_s = (
            None
            if self.dialect_name == "mysql"
            else (str(self.config.get("schema") or "").strip() or self.default_schema_name)
        )
        schema, table, query = extract_schema_and_table(ref, default_s)
        has_cur = cursor_from is not None and cursor_column is not None
        sql = build_select_sql(
            schema=schema,
            table=table,
            custom_query=query,
            columns=None,
            mode=mode,
            cursor_column=cursor_column,
            has_cursor_from=has_cur,
            dialect=self.dialect_name,
            param_placeholder=":cursor_from",
        )
        params: dict[str, Any] = {}
        if has_cur:
            params["cursor_from"] = coerce_cursor_param(cursor_from)

        eng = self._get_engine()
        step = max(batch_rows, 1)
        with eng.connect().execution_options(stream_results=True) as conn:
            result = conn.execute(text(sql), params)
            col_names = list(result.keys())
            while True:
                chunk = result.fetchmany(step)
                if not chunk:
                    break
                dicts = [dict(zip(col_names, r, strict=False)) for r in chunk]
                max_cur: str | None = None
                if cursor_column:
                    cur_vals = [
                        d.get(cursor_column) for d in dicts if d.get(cursor_column) is not None
                    ]
                    if cur_vals:
                        max_cur = format_cursor_value(cur_vals[-1])
                pa_tbl = records_to_arrow_table(dicts, column_order=col_names)
                for rb in pa_tbl.to_batches(max_chunksize=step):
                    yield ReadBatch(batch=rb, cursor_to=max_cur)

    def close(self) -> None:
        if self._engine is not None:
            self._engine.dispose()
            self._engine = None


class PostgresConnector(SqlAlchemyFamilyConnector):
    spec = POSTGRES_SPEC
    dialect_name = "postgres"
    default_port = 5432
    default_schema_name = "public"


class MySqlConnector(SqlAlchemyFamilyConnector):
    spec = MYSQL_SPEC
    dialect_name = "mysql"
    default_port = 3306
    default_schema_name = None


class MsSqlConnector(SqlAlchemyFamilyConnector):
    spec = MSSQL_SPEC
    dialect_name = "mssql"
    default_port = 1433
    default_schema_name = "dbo"


class OracleConnector(SqlAlchemyFamilyConnector):
    spec = ORACLE_SPEC
    dialect_name = "oracle"
    default_port = 1521
    default_schema_name = None


class SqliteConnector(BaseConnector):
    """Read-only SQLite connector constrained under AXPOINT_DATA_ROOT."""

    spec = SQLITE_SPEC

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
        raw_path = str(self.config.get("path") or "").strip()
        if not raw_path:
            raise SidecarError(400, "INVALID_ARGUMENT", "SQLite config missing 'path'")
        root = get_data_root().resolve()
        self._db_path = resolve_path_under_root(raw_path, data_root=root)

    def _connect_ro(self) -> sqlite3.Connection:
        if not self._db_path.is_file():
            raise SidecarError(
                404, "DATASET_NOT_FOUND", f"SQLite file not found: {self._db_path.name}"
            )
        uri = f"file:{self._db_path.as_posix()}?mode=ro"
        conn = sqlite3.connect(uri, uri=True, timeout=10.0)
        conn.execute("PRAGMA query_only = ON;")
        return conn

    def test(self) -> tuple[bool, str]:
        try:
            with self._connect_ro() as conn:
                conn.execute("SELECT 1").fetchone()
            return True, "connected"
        except Exception as exc:
            return False, str(exc)

    def discover(self) -> list[DiscoveredObject]:
        objects: list[DiscoveredObject] = []
        with self._connect_ro() as conn:
            tbl_rows = conn.execute(
                "SELECT name FROM sqlite_master WHERE type IN ('table', 'view') "
                "AND name NOT LIKE 'sqlite_%' ORDER BY name"
            ).fetchall()
            for (tbl_name,) in tbl_rows:
                try:
                    validate_sql_ident(tbl_name)
                except SidecarError:
                    continue
                q_tbl = '"' + str(tbl_name).replace('"', '""') + '"'
                info_rows = conn.execute(f"PRAGMA table_info({q_tbl})").fetchall()
                disc_cols: list[DiscoveredColumn] = []
                pks: list[str] = []
                typed_pairs: list[tuple[str, Any]] = []
                for _cid, c_name, c_type, notnull, _dflt, pk_idx in info_rows:
                    dt = map_native_type(str(c_type or "TEXT"), dialect="sqlite")
                    disc_cols.append(
                        DiscoveredColumn(
                            name=str(c_name),
                            native_type=str(c_type or "TEXT"),
                            data_type=dt,
                            nullable=not bool(notnull) and not bool(pk_idx),
                        )
                    )
                    if pk_idx:
                        pks.append(str(c_name))
                    typed_pairs.append((str(c_name), dt))

                cnt_row = conn.execute(f"SELECT count(*) FROM {q_tbl}").fetchone()
                est = int(cnt_row[0]) if cnt_row else 0
                objects.append(
                    DiscoveredObject(
                        ref={"table": str(tbl_name)},
                        display_name=str(tbl_name),
                        source_kind="SQL_TABLE",
                        columns=disc_cols,
                        primary_key=pks,
                        suggested_cursor=suggest_cursor_column(typed_pairs),
                        estimated_rows=est,
                    )
                )
        return objects

    def preview(self, ref: dict[str, Any], limit: int) -> pa.Table:
        capped = min(max(limit, 1), 1000)
        _schema, table, query = extract_schema_and_table(ref, None)
        sql = build_select_sql(
            schema=None,
            table=table,
            custom_query=query,
            columns=None,
            mode="SNAPSHOT",
            cursor_column=None,
            has_cursor_from=False,
            dialect="sqlite",
            limit=capped,
        )
        with self._connect_ro() as conn:
            cur = conn.execute(sql)
            cols = [d[0] for d in (cur.description or [])]
            rows = [dict(zip(cols, r, strict=False)) for r in cur.fetchall()]
        return records_to_arrow_table(rows, column_order=cols)

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
            dialect="sqlite",
            param_placeholder="?",
        )
        bind_args: tuple[Any, ...] = (coerce_cursor_param(cursor_from),) if has_cur else ()
        step = max(batch_rows, 1)

        with self._connect_ro() as conn:
            cur = conn.execute(sql, bind_args)
            cols = [d[0] for d in (cur.description or [])]
            while True:
                chunk = cur.fetchmany(step)
                if not chunk:
                    break
                dicts = [dict(zip(cols, r, strict=False)) for r in chunk]
                max_cur: str | None = None
                if cursor_column:
                    vals = [d.get(cursor_column) for d in dicts if d.get(cursor_column) is not None]
                    if vals:
                        max_cur = format_cursor_value(vals[-1])
                pa_tbl = records_to_arrow_table(dicts, column_order=cols)
                for rb in pa_tbl.to_batches(max_chunksize=step):
                    yield ReadBatch(batch=rb, cursor_to=max_cur)
