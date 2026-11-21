from __future__ import annotations

from collections.abc import Iterator
from typing import Any

import httpx
import pyarrow as pa

from connectors.base import (
    BaseConnector,
    ReadBatch,
    coerce_cursor_param,
    format_cursor_value,
    records_to_arrow_table,
    validate_select_only_sql,
)
from connectors.typemap import map_native_type
from schemas import ConnectorSpecModel, DiscoveredColumn, DiscoveredObject
from security import (
    SafeOutboundTransport,
    SidecarError,
    check_outbound_host,
    check_outbound_url,
    quote_sql_ident,
    validate_sql_ident,
)

INFLUXDB_SPEC = ConnectorSpecModel(
    connector_type="INFLUXDB",
    display_name="InfluxDB 2.x / 3.x",
    category="TIMESERIES",
    config_schema={
        "type": "object",
        "required": ["url"],
        "properties": {
            "url": {"type": "string"},
            "org": {"type": "string"},
            "bucket": {"type": "string"},
            "version": {"type": "string", "enum": ["2", "3"], "default": "2"},
        },
    },
    secret_fields=["token"],
    supports_modes=["SNAPSHOT", "APPEND"],
    supports_discover=True,
    image_tier="lite",
)

TDENGINE_SPEC = ConnectorSpecModel(
    connector_type="TDENGINE",
    display_name="TDengine",
    category="TIMESERIES",
    config_schema={
        "type": "object",
        "required": ["url", "database"],
        "properties": {
            "url": {"type": "string"},
            "database": {"type": "string"},
        },
    },
    secret_fields=["username", "password"],
    supports_modes=["SNAPSHOT", "APPEND"],
    supports_discover=True,
    image_tier="lite",
)

IOTDB_SPEC = ConnectorSpecModel(
    connector_type="IOTDB",
    display_name="Apache IoTDB",
    category="TIMESERIES",
    config_schema={
        "type": "object",
        "required": ["host"],
        "properties": {
            "host": {"type": "string"},
            "port": {"type": "integer", "default": 6667},
        },
    },
    secret_fields=["username", "password"],
    supports_modes=["SNAPSHOT", "APPEND"],
    supports_discover=True,
    image_tier="lite",
)


def _normalize_influx_v3_table(tbl: pa.Table) -> pa.Table:
    """Normalize InfluxDB 3 FlightSQL table (decode dictionary tags, rename time -> ts)."""
    if tbl.num_columns == 0:
        return tbl
    has_ts = "ts" in tbl.column_names
    arrays: list[pa.Array[Any] | pa.ChunkedArray[Any]] = []
    fields: list[pa.Field[Any]] = []
    for i, field in enumerate(tbl.schema):
        col = tbl.column(i)
        name = "ts" if (field.name == "time" and not has_ts) else field.name
        if pa.types.is_dictionary(field.type):
            col = col.cast(field.type.value_type)
            out_type = col.type
        elif pa.types.is_timestamp(field.type):
            tz = field.type.tz or "UTC"
            out_type = pa.timestamp("us", tz=tz)
            col = col.cast(out_type)
        else:
            out_type = field.type
        arrays.append(col)
        fields.append(pa.field(name, out_type, nullable=field.nullable))
    return pa.Table.from_arrays(arrays, schema=pa.schema(fields))


class InfluxDbConnector(BaseConnector):
    """InfluxDB 2.x / 3.x time-series connector."""

    spec = INFLUXDB_SPEC

    def __init__(
        self,
        config: dict[str, Any],
        secrets: dict[str, str],
        *,
        allow_private_network: bool = False,
        tenant_id: str = "default",
    ) -> None:
        super().__init__(
            config, secrets, allow_private_network=allow_private_network, tenant_id=tenant_id
        )
        url = str(self.config.get("url") or "").strip()
        if not url:
            raise SidecarError(400, "INVALID_ARGUMENT", "InfluxDB config missing 'url'")
        check_outbound_url(url, allow_private_network=allow_private_network)

    def _is_v3(self) -> bool:
        return str(self.config.get("version") or "2").strip() == "3"

    def _open_v3_client(self) -> Any:
        import influxdb_client_3 as ic3_mod

        cls3: Any = ic3_mod.InfluxDBClient3
        db_name = str(self.config.get("bucket") or self.config.get("database") or "").strip()
        org_name = str(self.config.get("org") or "").strip()
        return cls3(
            host=str(self.config["url"]),
            token=str(self.secrets.get("token") or ""),
            org=org_name or None,
            database=db_name or None,
        )

    def test(self) -> tuple[bool, str]:
        try:
            if self._is_v3():
                cli3 = self._open_v3_client()
                try:
                    cli3.query("SELECT 1")
                finally:
                    cli3.close()
                return True, "connected (InfluxDB 3)"

            from influxdb_client.client.influxdb_client import InfluxDBClient

            with InfluxDBClient(
                url=str(self.config["url"]),
                token=str(self.secrets.get("token") or ""),
                org=str(self.config.get("org") or ""),
                timeout=5000,
            ) as cli2:
                ok = cli2.ping()
                return (True, "connected") if ok else (False, "InfluxDB ping returned false")
        except Exception as exc:
            return False, str(exc)

    def _discover_v3(self) -> list[DiscoveredObject]:
        cli3 = self._open_v3_client()
        try:
            tbl_res: pa.Table = cli3.query(
                "SELECT table_name FROM information_schema.tables "
                "WHERE table_schema = 'iox' ORDER BY table_name"
            )
            col_res: pa.Table = cli3.query(
                "SELECT table_name, column_name, data_type, is_nullable "
                "FROM information_schema.columns "
                "WHERE table_schema = 'iox' ORDER BY table_name, ordinal_position"
            )
        finally:
            cli3.close()

        cols_by_tbl: dict[str, list[DiscoveredColumn]] = {}
        for row in col_res.to_pylist():
            t_str = str(row.get("table_name") or "")
            raw_col = str(row.get("column_name") or "")
            raw_type = str(row.get("data_type") or "")
            is_null = str(row.get("is_nullable") or "YES").upper() == "YES"
            c_name = "ts" if raw_col == "time" else raw_col
            dt = "TIMESTAMP" if raw_col == "time" else map_native_type(raw_type)
            cols_by_tbl.setdefault(t_str, []).append(
                DiscoveredColumn(
                    name=c_name,
                    native_type=raw_type,
                    data_type=dt,
                    nullable=False if raw_col == "time" else is_null,
                )
            )

        out: list[DiscoveredObject] = []
        for row in tbl_res.to_pylist():
            m_name = str(row.get("table_name") or "")
            try:
                validate_sql_ident(m_name)
            except SidecarError:
                continue
            cols = cols_by_tbl.get(m_name) or [
                DiscoveredColumn(
                    name="ts",
                    native_type="Timestamp(ns)",
                    data_type="TIMESTAMP",
                    nullable=False,
                )
            ]
            out.append(
                DiscoveredObject(
                    ref={"measurement": m_name},
                    display_name=m_name,
                    source_kind="TIMESERIES",
                    columns=cols,
                    primary_key=[],
                    suggested_cursor="ts",
                )
            )
        return out

    def discover(self) -> list[DiscoveredObject]:
        if self._is_v3():
            return self._discover_v3()

        bucket = str(self.config.get("bucket") or "")
        from influxdb_client.client.influxdb_client import InfluxDBClient

        out: list[DiscoveredObject] = []
        with InfluxDBClient(
            url=str(self.config["url"]),
            token=str(self.secrets.get("token") or ""),
            org=str(self.config.get("org") or ""),
        ) as cli:
            q_api = cli.query_api()
            flux = f'import "influxdata/influxdb/schema"\nschema.measurements(bucket: "{bucket}")'
            tables = q_api.query(flux)
            for t in tables:
                for rec in t.records:
                    m_name = str(rec.get_value())
                    out.append(
                        DiscoveredObject(
                            ref={"measurement": m_name},
                            display_name=m_name,
                            source_kind="TIMESERIES",
                            columns=[
                                DiscoveredColumn(
                                    name="ts",
                                    native_type="dateTime:RFC3339",
                                    data_type="TIMESTAMP",
                                    nullable=False,
                                )
                            ],
                            primary_key=[],
                            suggested_cursor="ts",
                        )
                    )
        return out

    def _query_v3_table(
        self,
        ref: dict[str, Any],
        *,
        cursor_column: str | None,
        cursor_from: str | None,
        limit: int | None,
    ) -> pa.Table:
        raw_sql = str(ref.get("sql") or ref.get("query") or "").strip()
        params: dict[str, Any] | None = None
        if raw_sql:
            sql = validate_select_only_sql(raw_sql, dialect="postgres")
            if limit is not None:
                sql = f"SELECT * FROM ({sql}) AS _ax_q LIMIT {int(limit)}"
        else:
            meas = str(ref.get("measurement") or ref.get("table") or "").strip()
            if not meas:
                raise SidecarError(
                    400, "INVALID_ARGUMENT", "InfluxDB v3 ref missing 'measurement' or 'sql'"
                )
            q_meas = quote_sql_ident(meas, dialect="postgres")
            sql_cur = "time" if (not cursor_column or cursor_column == "ts") else cursor_column
            q_cur = quote_sql_ident(sql_cur, dialect="postgres")
            where_sql = ""
            if cursor_from:
                where_sql = f" WHERE {q_cur} > $cur"
                params = {
                    "cur": cursor_from if sql_cur == "time" else coerce_cursor_param(cursor_from)
                }
            lim_sql = f" LIMIT {int(limit)}" if limit is not None else ""
            sql = f"SELECT * FROM {q_meas}{where_sql} ORDER BY {q_cur} ASC{lim_sql}"

        cli3 = self._open_v3_client()
        try:
            tbl: pa.Table = (
                cli3.query(sql, query_parameters=params)
                if params is not None
                else cli3.query(sql)
            )
        finally:
            cli3.close()
        return _normalize_influx_v3_table(tbl)

    def _query_records(
        self, ref: dict[str, Any], start_expr: str, limit: int | None
    ) -> list[dict[str, Any]]:
        bucket = str(self.config.get("bucket") or "")
        if ref.get("flux"):
            flux = str(ref["flux"])
        else:
            meas = str(ref.get("measurement") or "")
            if not meas:
                raise SidecarError(
                    400, "INVALID_ARGUMENT", "InfluxDB ref missing 'measurement' or 'flux'"
                )
            lim_pipe = f" |> limit(n: {int(limit)})" if limit else ""
            flux = (
                f'from(bucket: "{bucket}")'
                f" |> range(start: {start_expr})"
                f' |> filter(fn: (r) => r._measurement == "{meas}")'
                f' |> pivot(rowKey:["_time"], columnKey: ["_field"], valueColumn: "_value")'
                f"{lim_pipe}"
            )
        from influxdb_client.client.influxdb_client import InfluxDBClient

        rows: list[dict[str, Any]] = []
        with InfluxDBClient(
            url=str(self.config["url"]),
            token=str(self.secrets.get("token") or ""),
            org=str(self.config.get("org") or ""),
        ) as cli:
            for tbl in cli.query_api().query(flux):
                for rec in tbl.records:
                    vals = {
                        k: v
                        for k, v in rec.values.items()
                        if k not in {"result", "table", "_start", "_stop"}
                    }
                    if "_time" in vals:
                        vals["ts"] = vals.pop("_time")
                    rows.append(vals)
        return rows

    def preview(self, ref: dict[str, Any], limit: int) -> pa.Table:
        bounded = min(max(limit, 1), 1000)
        if self._is_v3():
            return self._query_v3_table(
                ref, cursor_column=None, cursor_from=None, limit=bounded
            )
        rows = self._query_records(ref, "-30d", bounded)
        return records_to_arrow_table(rows)

    def read(
        self,
        ref: dict[str, Any],
        mode: str,
        cursor_column: str | None,
        cursor_from: str | None,
        batch_rows: int = 50_000,
        stream_offsets: dict[str, str] | None = None,
    ) -> Iterator[ReadBatch]:
        cur_key = cursor_column or "ts"
        if self._is_v3():
            eff_from = cursor_from if (mode.upper() == "APPEND" and cursor_from) else None
            tbl3 = self._query_v3_table(
                ref, cursor_column=cur_key, cursor_from=eff_from, limit=None
            )
            if tbl3.num_rows == 0:
                return
            max_c3: str | None = None
            if cur_key in tbl3.column_names:
                last_val = tbl3.column(cur_key)[-1].as_py()
                max_c3 = format_cursor_value(last_val)
            for rb in tbl3.to_batches(max_chunksize=max(batch_rows, 1)):
                yield ReadBatch(batch=rb, cursor_to=max_c3)
            return

        start_expr = (
            cursor_from if (mode.upper() == "APPEND" and cursor_from) else "1970-01-01T00:00:00Z"
        )
        rows = self._query_records(ref, start_expr, None)
        if not rows:
            return
        max_c = format_cursor_value(rows[-1].get(cur_key)) if rows[-1].get(cur_key) else None
        tbl = records_to_arrow_table(rows)
        for rb in tbl.to_batches(max_chunksize=max(batch_rows, 1)):
            yield ReadBatch(batch=rb, cursor_to=max_c)


class TDengineConnector(BaseConnector):
    """TDengine REST SQL connector."""

    spec = TDENGINE_SPEC

    def __init__(
        self,
        config: dict[str, Any],
        secrets: dict[str, str],
        *,
        allow_private_network: bool = False,
        tenant_id: str = "default",
    ) -> None:
        super().__init__(
            config, secrets, allow_private_network=allow_private_network, tenant_id=tenant_id
        )
        url = str(self.config.get("url") or "").strip().rstrip("/")
        if not url:
            raise SidecarError(400, "INVALID_ARGUMENT", "TDengine config missing 'url'")
        check_outbound_url(url, allow_private_network=allow_private_network)
        self._base_url = url

    def _exec_rest_sql(self, sql: str) -> tuple[list[tuple[str, str]], list[list[Any]]]:
        db = str(self.config.get("database") or "")
        endpoint = f"{self._base_url}/rest/sql/{db}" if db else f"{self._base_url}/rest/sql"
        user = str(self.secrets.get("username") or self.config.get("username") or "root")
        pw = str(self.secrets.get("password") or "taosdata")
        transport = self.config.get("_transport") or SafeOutboundTransport(
            allow_private_network=self.allow_private_network
        )
        with httpx.Client(transport=transport, timeout=15.0, trust_env=False) as client:
            resp = client.post(endpoint, content=sql.encode("utf-8"), auth=(user, pw))
            resp.raise_for_status()
            payload = resp.json()
        if payload.get("code", 0) != 0:
            raise SidecarError(
                502, "UPSTREAM_CONNECTOR", str(payload.get("desc") or "TDengine error")
            )
        meta = [(str(m[0]), str(m[1])) for m in (payload.get("column_meta") or [])]
        data = list(payload.get("data") or [])
        return meta, data

    def test(self) -> tuple[bool, str]:
        try:
            self._exec_rest_sql("SELECT SERVER_VERSION()")
            return True, "connected"
        except Exception as exc:
            return False, str(exc)

    def discover(self) -> list[DiscoveredObject]:
        out: list[DiscoveredObject] = []
        for show_cmd, ref_key in (("SHOW STABLES", "stable"), ("SHOW TABLES", "table")):
            _meta, rows = self._exec_rest_sql(show_cmd)
            for r in rows:
                t_name = str(r[0])
                try:
                    validate_sql_ident(t_name)
                except SidecarError:
                    continue
                _cm, desc_rows = self._exec_rest_sql(f"DESCRIBE `{t_name}`")
                cols = [
                    DiscoveredColumn(
                        name=str(dr[0]),
                        native_type=str(dr[1]),
                        data_type=map_native_type(str(dr[1]), dialect="tdengine"),
                        nullable=True,
                    )
                    for dr in desc_rows
                ]
                out.append(
                    DiscoveredObject(
                        ref={ref_key: t_name},
                        display_name=t_name,
                        source_kind="TIMESERIES",
                        columns=cols,
                        primary_key=[],
                        suggested_cursor=cols[0].name if cols else "ts",
                    )
                )
        return out

    def _build_query(
        self,
        ref: dict[str, Any],
        cursor_col: str | None,
        cursor_from: str | None,
        limit: int | None,
    ) -> str:
        if ref.get("query"):
            base_q = validate_select_only_sql(str(ref["query"]), dialect="mysql")
            return f"{base_q} LIMIT {int(limit)}" if limit else base_q
        target = str(ref.get("stable") or ref.get("table") or "")
        validate_sql_ident(target)
        sql = f"SELECT * FROM `{target}`"
        if cursor_col and cursor_from:
            validate_sql_ident(cursor_col)
            safe_ts = str(cursor_from).replace("'", "''")
            sql += f" WHERE `{cursor_col}` > '{safe_ts}'"
        if cursor_col:
            sql += f" ORDER BY `{cursor_col}` ASC"
        if limit:
            sql += f" LIMIT {int(limit)}"
        return sql

    def preview(self, ref: dict[str, Any], limit: int) -> pa.Table:
        sql = self._build_query(ref, None, None, min(max(limit, 1), 1000))
        meta, rows = self._exec_rest_sql(sql)
        cols = [m[0] for m in meta]
        dicts = [dict(zip(cols, r, strict=False)) for r in rows]
        return records_to_arrow_table(dicts, column_order=cols)

    def read(
        self,
        ref: dict[str, Any],
        mode: str,
        cursor_column: str | None,
        cursor_from: str | None,
        batch_rows: int = 50_000,
        stream_offsets: dict[str, str] | None = None,
    ) -> Iterator[ReadBatch]:
        c_from = cursor_from if mode.upper() == "APPEND" else None
        sql = self._build_query(ref, cursor_column, c_from, None)
        meta, rows = self._exec_rest_sql(sql)
        if not rows:
            return
        cols = [m[0] for m in meta]
        dicts = [dict(zip(cols, r, strict=False)) for r in rows]
        cur_k = cursor_column or cols[0]
        max_c = format_cursor_value(dicts[-1].get(cur_k))
        tbl = records_to_arrow_table(dicts, column_order=cols)
        for rb in tbl.to_batches(max_chunksize=max(batch_rows, 1)):
            yield ReadBatch(batch=rb, cursor_to=max_c)


class IoTDbConnector(BaseConnector):
    """Apache IoTDB RPC Session connector."""

    spec = IOTDB_SPEC

    def __init__(
        self,
        config: dict[str, Any],
        secrets: dict[str, str],
        *,
        allow_private_network: bool = False,
        tenant_id: str = "default",
    ) -> None:
        super().__init__(
            config, secrets, allow_private_network=allow_private_network, tenant_id=tenant_id
        )
        host = str(self.config.get("host") or "").strip()
        port = int(self.config.get("port") or 6667)
        if not host:
            raise SidecarError(400, "INVALID_ARGUMENT", "IoTDB config missing 'host'")
        check_outbound_host(host, port, allow_private_network=allow_private_network)

    def _open_session(self) -> Any:
        from iotdb.Session import Session

        user = str(self.secrets.get("username") or self.config.get("username") or "root")
        pw = str(self.secrets.get("password") or "root")
        session = Session(str(self.config["host"]), int(self.config.get("port") or 6667), user, pw)
        session.open(False)
        return session

    def test(self) -> tuple[bool, str]:
        try:
            session = self._open_session()
            try:
                ds = session.execute_query_statement("SHOW VERSION")
                ds.close_operation_handle()
            finally:
                session.close()
            return True, "connected"
        except Exception as exc:
            return False, str(exc)

    def discover(self) -> list[DiscoveredObject]:
        session = self._open_session()
        try:
            ds = session.execute_query_statement("SHOW TIMESERIES")
            paths: list[str] = []
            while ds.has_next():
                row = ds.next()
                fields = row.get_fields()
                if fields:
                    paths.append(str(fields[0]))
            ds.close_operation_handle()
            return [
                DiscoveredObject(
                    ref={"path": p},
                    display_name=p,
                    source_kind="TIMESERIES",
                    columns=[
                        DiscoveredColumn(
                            name="Time", native_type="INT64", data_type="TIMESTAMP", nullable=False
                        ),
                        DiscoveredColumn(
                            name="value", native_type="DOUBLE", data_type="FLOAT64", nullable=True
                        ),
                    ],
                    primary_key=[],
                    suggested_cursor="Time",
                )
                for p in paths
            ]
        finally:
            session.close()

    def preview(self, ref: dict[str, Any], limit: int) -> pa.Table:
        path = str(ref.get("path") or "root.**")
        session = self._open_session()
        try:
            ds = session.execute_query_statement(
                f"SELECT * FROM {path} LIMIT {min(max(limit, 1), 1000)}"
            )
            cols = [str(c) for c in ds.get_column_names()]
            rows: list[dict[str, Any]] = []
            while ds.has_next():
                rec = ds.next()
                vals = [
                    rec.get_timestamp(),
                    *[f.get_object_value(f.get_data_type()) for f in rec.get_fields()],
                ]
                rows.append(dict(zip(cols, vals, strict=False)))
            ds.close_operation_handle()
            return records_to_arrow_table(rows, column_order=cols)
        finally:
            session.close()

    def read(
        self,
        ref: dict[str, Any],
        mode: str,
        cursor_column: str | None,
        cursor_from: str | None,
        batch_rows: int = 50_000,
        stream_offsets: dict[str, str] | None = None,
    ) -> Iterator[ReadBatch]:
        path = str(ref.get("path") or "root.**")
        where = (
            f" WHERE Time > {int(cursor_from)}"
            if (mode.upper() == "APPEND" and cursor_from and cursor_from.isdigit())
            else ""
        )
        session = self._open_session()
        try:
            ds = session.execute_query_statement(f"SELECT * FROM {path}{where}")
            cols = [str(c) for c in ds.get_column_names()]
            rows: list[dict[str, Any]] = []
            while ds.has_next():
                rec = ds.next()
                vals = [
                    rec.get_timestamp(),
                    *[f.get_object_value(f.get_data_type()) for f in rec.get_fields()],
                ]
                rows.append(dict(zip(cols, vals, strict=False)))
            ds.close_operation_handle()
            if not rows:
                return
            max_c = str(rows[-1].get("Time")) if rows[-1].get("Time") is not None else None
            tbl = records_to_arrow_table(rows, column_order=cols)
            for rb in tbl.to_batches(max_chunksize=max(batch_rows, 1)):
                yield ReadBatch(batch=rb, cursor_to=max_c)
        finally:
            session.close()
