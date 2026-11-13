from __future__ import annotations

import re
from collections.abc import Iterator
from typing import Any
from urllib.parse import urlparse

import orjson
import pyarrow as pa

from connectors.base import (
    BaseConnector,
    ReadBatch,
    coerce_cursor_param,
    format_cursor_value,
    records_to_arrow_table,
    suggest_cursor_column,
)
from connectors.typemap import map_native_type
from parsers.json_parser import flatten_json_records
from schemas import ConnectorSpecModel, DiscoveredColumn, DiscoveredObject
from security import SidecarError, check_outbound_host, check_outbound_url

_CYPHER_WRITE_RE = re.compile(
    r"\b(?:CREATE|MERGE|DELETE|DETACH|SET|REMOVE|DROP|CALL\s+dbms|LOAD\s+CSV)\b",
    re.IGNORECASE,
)

MONGODB_SPEC = ConnectorSpecModel(
    connector_type="MONGODB",
    display_name="MongoDB",
    category="NOSQL",
    config_schema={
        "type": "object",
        "required": ["database"],
        "properties": {
            "uri": {"type": "string"},
            "host": {"type": "string", "default": "127.0.0.1"},
            "port": {"type": "integer", "default": 27017},
            "database": {"type": "string"},
            "auth_source": {"type": "string", "default": "admin"},
        },
    },
    secret_fields=["username", "password"],
    supports_modes=["SNAPSHOT", "APPEND", "UPSERT"],
    supports_discover=True,
    image_tier="lite",
)

REDIS_SPEC = ConnectorSpecModel(
    connector_type="REDIS",
    display_name="Redis",
    category="NOSQL",
    config_schema={
        "type": "object",
        "required": ["host"],
        "properties": {
            "host": {"type": "string"},
            "port": {"type": "integer", "default": 6379},
            "db": {"type": "integer", "default": 0},
            "tls": {"type": "boolean", "default": False},
            "key_patterns": {"type": "array", "items": {"type": "string"}, "default": ["*"]},
        },
    },
    secret_fields=["username", "password"],
    supports_modes=["SNAPSHOT", "APPEND"],
    supports_discover=True,
    image_tier="lite",
)

ELASTICSEARCH_SPEC = ConnectorSpecModel(
    connector_type="ELASTICSEARCH",
    display_name="Elasticsearch",
    category="NOSQL",
    config_schema={
        "type": "object",
        "required": ["hosts"],
        "properties": {
            "hosts": {"type": "array", "items": {"type": "string"}},
            "verify_certs": {"type": "boolean", "default": True},
        },
    },
    secret_fields=["api_key", "username", "password"],
    supports_modes=["SNAPSHOT", "APPEND"],
    supports_discover=True,
    image_tier="lite",
)

CASSANDRA_SPEC = ConnectorSpecModel(
    connector_type="CASSANDRA",
    display_name="Apache Cassandra",
    category="NOSQL",
    config_schema={
        "type": "object",
        "required": ["contact_points", "keyspace"],
        "properties": {
            "contact_points": {"type": "array", "items": {"type": "string"}},
            "port": {"type": "integer", "default": 9042},
            "keyspace": {"type": "string"},
        },
    },
    secret_fields=["username", "password"],
    supports_modes=["SNAPSHOT", "APPEND"],
    supports_discover=True,
    image_tier="full",
)

HBASE_SPEC = ConnectorSpecModel(
    connector_type="HBASE",
    display_name="Apache HBase (Thrift)",
    category="NOSQL",
    config_schema={
        "type": "object",
        "required": ["host"],
        "properties": {
            "host": {"type": "string"},
            "thrift_port": {"type": "integer", "default": 9090},
            "table_prefix": {"type": "string", "default": ""},
        },
    },
    secret_fields=[],
    supports_modes=["SNAPSHOT", "APPEND"],
    supports_discover=True,
    image_tier="full",
)

NEO4J_SPEC = ConnectorSpecModel(
    connector_type="NEO4J",
    display_name="Neo4j",
    category="NOSQL",
    config_schema={
        "type": "object",
        "required": ["uri"],
        "properties": {
            "uri": {"type": "string"},
            "database": {"type": "string", "default": "neo4j"},
        },
    },
    secret_fields=["username", "password"],
    supports_modes=["SNAPSHOT", "APPEND"],
    supports_discover=True,
    image_tier="lite",
)


def _normalize_bson_value(val: Any) -> Any:
    try:
        from bson import Decimal128, ObjectId
    except ImportError:
        ObjectId = ()  # type: ignore[assignment,misc]
        Decimal128 = ()  # type: ignore[assignment,misc]

    if isinstance(val, ObjectId):
        return str(val)
    if isinstance(val, Decimal128):
        return val.to_decimal()
    if isinstance(val, dict):
        return {str(k): _normalize_bson_value(v) for k, v in val.items()}
    if isinstance(val, list):
        return [_normalize_bson_value(x) for x in val]
    return val


def _flatten_docs_to_arrow(docs: list[dict[str, Any]]) -> pa.Table:
    if not docs:
        return pa.table({})
    normalized = [_normalize_bson_value(d) for d in docs]
    flat = flatten_json_records(normalized, json_explode="NONE")
    row_dicts = [dict(zip(flat.main_columns, row, strict=False)) for row in flat.main_rows]
    return records_to_arrow_table(row_dicts, column_order=flat.main_columns)


class MongoDbConnector(BaseConnector):
    """MongoDB read-only connector via pymongo."""

    spec = MONGODB_SPEC

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
        uri = str(self.config.get("uri") or "").strip()
        if uri:
            parsed = urlparse(uri)
            host = parsed.hostname or "127.0.0.1"
            port = parsed.port or 27017
        else:
            host = str(self.config.get("host") or "").strip()
            port = int(self.config.get("port") or 27017)
        if not host:
            raise SidecarError(400, "INVALID_ARGUMENT", "MongoDB config missing host or uri")
        check_outbound_host(host, port, allow_private_network=allow_private_network)
        self._client: Any = None

    def _get_client(self) -> Any:
        import pymongo

        if self._client is None:
            uri = str(self.config.get("uri") or "").strip()
            user = self.secrets.get("username") or self.config.get("username")
            pw = self.secrets.get("password") or self.config.get("password")
            if uri:
                self._client = pymongo.MongoClient(
                    uri, serverSelectionTimeoutMS=5000, connectTimeoutMS=5000
                )
            else:
                kwargs: dict[str, Any] = {
                    "host": str(self.config["host"]),
                    "port": int(self.config.get("port") or 27017),
                    "serverSelectionTimeoutMS": 5000,
                    "connectTimeoutMS": 5000,
                }
                if user:
                    kwargs["username"] = str(user)
                    kwargs["password"] = str(pw or "")
                    kwargs["authSource"] = str(self.config.get("auth_source") or "admin")
                self._client = pymongo.MongoClient(**kwargs)
        return self._client

    def test(self) -> tuple[bool, str]:
        try:
            self._get_client().admin.command("ping")
            return True, "connected"
        except Exception as exc:
            return False, str(exc)

    def discover(self) -> list[DiscoveredObject]:
        db_name = str(self.config.get("database") or "test")
        db = self._get_client()[db_name]
        objects: list[DiscoveredObject] = []
        for coll_name in sorted(db.list_collection_names()):
            coll = db[coll_name]
            sample_docs = list(coll.find({}, limit=200))
            tbl = _flatten_docs_to_arrow(sample_docs)
            cols: list[DiscoveredColumn] = []
            typed_pairs: list[tuple[str, Any]] = []
            for field in tbl.schema:
                dt = map_native_type(str(field.type))
                cols.append(
                    DiscoveredColumn(
                        name=field.name,
                        native_type=str(field.type),
                        data_type=dt,
                        nullable=True,
                    )
                )
                typed_pairs.append((field.name, dt))
            est = coll.estimated_document_count()
            objects.append(
                DiscoveredObject(
                    ref={"collection": coll_name},
                    display_name=coll_name,
                    source_kind="NOSQL_COLLECTION",
                    columns=cols,
                    primary_key=["_id"] if any(c.name == "_id" for c in cols) else [],
                    suggested_cursor=suggest_cursor_column(typed_pairs),
                    estimated_rows=int(est),
                )
            )
        return objects

    def preview(self, ref: dict[str, Any], limit: int) -> pa.Table:
        db = self._get_client()[str(self.config.get("database") or "test")]
        coll_name = str(ref.get("collection") or ref.get("table") or "")
        if not coll_name:
            raise SidecarError(400, "INVALID_ARGUMENT", "MongoDB ref missing 'collection'")
        docs = list(db[coll_name].find(ref.get("filter") or {}, limit=min(max(limit, 1), 1000)))
        return _flatten_docs_to_arrow(docs)

    def read(
        self,
        ref: dict[str, Any],
        mode: str,
        cursor_column: str | None,
        cursor_from: str | None,
        batch_rows: int = 50_000,
        stream_offsets: dict[str, str] | None = None,
    ) -> Iterator[ReadBatch]:
        db = self._get_client()[str(self.config.get("database") or "test")]
        coll_name = str(ref.get("collection") or ref.get("table") or "")
        if not coll_name:
            raise SidecarError(400, "INVALID_ARGUMENT", "MongoDB ref missing 'collection'")
        flt: dict[str, Any] = dict(ref.get("filter") or {})
        if mode.upper() in {"APPEND", "UPSERT"} and cursor_column and cursor_from is not None:
            op = "$gte" if mode.upper() == "UPSERT" else "$gt"
            flt[cursor_column] = {op: coerce_cursor_param(cursor_from)}

        cursor = db[coll_name].find(flt).batch_size(max(batch_rows, 100))
        if cursor_column:
            cursor = cursor.sort(cursor_column, 1)

        buf: list[dict[str, Any]] = []
        step = max(batch_rows, 1)
        for doc in cursor:
            buf.append(doc)
            if len(buf) >= step:
                tbl = _flatten_docs_to_arrow(buf)
                max_c = format_cursor_value(buf[-1].get(cursor_column)) if cursor_column else None
                for rb in tbl.to_batches(max_chunksize=step):
                    yield ReadBatch(batch=rb, cursor_to=max_c)
                buf.clear()
        if buf:
            tbl = _flatten_docs_to_arrow(buf)
            max_c = format_cursor_value(buf[-1].get(cursor_column)) if cursor_column else None
            for rb in tbl.to_batches(max_chunksize=step):
                yield ReadBatch(batch=rb, cursor_to=max_c)

    def close(self) -> None:
        if self._client is not None:
            self._client.close()
            self._client = None


class RedisConnector(BaseConnector):
    """Redis read-only connector using SCAN (KEYS is strictly prohibited)."""

    spec = REDIS_SPEC

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
        port = int(self.config.get("port") or 6379)
        if not host:
            raise SidecarError(400, "INVALID_ARGUMENT", "Redis config missing 'host'")
        check_outbound_host(host, port, allow_private_network=allow_private_network)
        self._client: Any = None

    def _get_client(self) -> Any:
        import redis

        if self._client is None:
            self._client = redis.Redis(
                host=str(self.config["host"]),
                port=int(self.config.get("port") or 6379),
                db=int(self.config.get("db") or 0),
                username=self.secrets.get("username") or self.config.get("username"),
                password=self.secrets.get("password") or self.config.get("password"),
                ssl=bool(self.config.get("tls", False)),
                decode_responses=True,
                socket_connect_timeout=5,
                socket_timeout=10,
            )
        return self._client

    def test(self) -> tuple[bool, str]:
        try:
            self._get_client().ping()
            return True, "connected"
        except Exception as exc:
            return False, str(exc)

    def discover(self) -> list[DiscoveredObject]:
        cli = self._get_client()
        patterns = self.config.get("key_patterns") or ["*"]
        objects: list[DiscoveredObject] = []
        for pat in patterns:
            pat_s = str(pat)
            sampled_keys: list[str] = []
            for key in cli.scan_iter(match=pat_s, count=500):
                sampled_keys.append(str(key))
                if len(sampled_keys) >= 50:
                    break
            key_type = cli.type(sampled_keys[0]) if sampled_keys else "hash"
            objects.append(
                DiscoveredObject(
                    ref={"pattern": pat_s, "key_type": key_type},
                    display_name=f"{pat_s} ({key_type})",
                    source_kind="NOSQL_COLLECTION",
                    columns=[
                        DiscoveredColumn(
                            name="_key", native_type="string", data_type="STRING", nullable=False
                        )
                    ],
                    primary_key=["_key"],
                    suggested_cursor="_id" if key_type == "stream" else None,
                    estimated_rows=len(sampled_keys),
                )
            )
        return objects

    def _read_pattern(
        self, pattern: str, cursor_from: str | None, max_items: int | None = None
    ) -> list[dict[str, Any]]:
        cli = self._get_client()
        records: list[dict[str, Any]] = []
        for key in cli.scan_iter(match=pattern, count=1000):
            k_str = str(key)
            k_type = cli.type(k_str)
            if k_type == "hash":
                h = cli.hgetall(k_str)
                records.append({"_key": k_str, **h})
            elif k_type == "stream":
                min_id = f"({cursor_from}" if cursor_from else "-"
                entries = cli.xrange(k_str, min=min_id, max="+", count=max_items or 10_000)
                for entry_id, fields in entries:
                    records.append({"_key": k_str, "_id": str(entry_id), **fields})
            elif k_type == "string":
                raw = cli.get(k_str)
                if isinstance(raw, str) and raw.startswith("{"):
                    try:
                        parsed = orjson.loads(raw)
                        if isinstance(parsed, dict):
                            records.append({"_key": k_str, **parsed})
                            continue
                    except orjson.JSONDecodeError:
                        pass
                records.append({"_key": k_str, "value": raw})
            elif k_type == "list":
                vals = cli.lrange(k_str, 0, (max_items or 1000) - 1)
                for idx, v in enumerate(vals):
                    records.append({"_key": k_str, "index": idx, "value": v})
            elif k_type == "set":
                for m in cli.smembers(k_str):
                    records.append({"_key": k_str, "member": m})
            elif k_type == "zset":
                for member, score in cli.zrange(k_str, 0, -1, withscores=True):
                    records.append({"_key": k_str, "member": member, "score": float(score)})

            if max_items is not None and len(records) >= max_items:
                return records[:max_items]
        return records

    def preview(self, ref: dict[str, Any], limit: int) -> pa.Table:
        pattern = str(ref.get("pattern") or "*")
        recs = self._read_pattern(pattern, cursor_from=None, max_items=min(max(limit, 1), 1000))
        return _flatten_docs_to_arrow(recs)

    def read(
        self,
        ref: dict[str, Any],
        mode: str,
        cursor_column: str | None,
        cursor_from: str | None,
        batch_rows: int = 50_000,
        stream_offsets: dict[str, str] | None = None,
    ) -> Iterator[ReadBatch]:
        pattern = str(ref.get("pattern") or "*")
        recs = self._read_pattern(
            pattern, cursor_from=cursor_from if mode.upper() == "APPEND" else None
        )
        if not recs:
            return
        tbl = _flatten_docs_to_arrow(recs)
        max_c = (
            format_cursor_value(recs[-1].get(cursor_column))
            if cursor_column and recs[-1].get(cursor_column) is not None
            else None
        )
        for rb in tbl.to_batches(max_chunksize=max(batch_rows, 1)):
            yield ReadBatch(batch=rb, cursor_to=max_c)

    def close(self) -> None:
        if self._client is not None:
            self._client.close()
            self._client = None


class ElasticsearchConnector(BaseConnector):
    """Elasticsearch read-only connector."""

    spec = ELASTICSEARCH_SPEC

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
        hosts = self.config.get("hosts") or []
        if isinstance(hosts, str):
            hosts = [hosts]
        if not hosts:
            raise SidecarError(400, "INVALID_ARGUMENT", "Elasticsearch config missing 'hosts'")
        for h_url in hosts:
            check_outbound_url(str(h_url), allow_private_network=allow_private_network)
        self._hosts = [str(h) for h in hosts]
        self._client: Any = None

    def _get_client(self) -> Any:
        from elasticsearch import Elasticsearch

        if self._client is None:
            kwargs: dict[str, Any] = {
                "hosts": self._hosts,
                "verify_certs": bool(self.config.get("verify_certs", True)),
                "request_timeout": 15,
            }
            if self.secrets.get("api_key"):
                kwargs["api_key"] = self.secrets["api_key"]
            elif self.secrets.get("username") or self.config.get("username"):
                kwargs["basic_auth"] = (
                    str(self.secrets.get("username") or self.config.get("username") or ""),
                    str(self.secrets.get("password") or ""),
                )
            self._client = Elasticsearch(**kwargs)
        return self._client

    def test(self) -> tuple[bool, str]:
        try:
            info = self._get_client().info()
            ver = info.get("version", {}).get("number", "ok")
            return True, f"connected (ES {ver})"
        except Exception as exc:
            return False, str(exc)

    def discover(self) -> list[DiscoveredObject]:
        cli = self._get_client()
        mappings = cli.indices.get_mapping(index="*")
        out: list[DiscoveredObject] = []
        for idx_name, idx_meta in sorted(mappings.items()):
            if str(idx_name).startswith("."):
                continue
            props = idx_meta.get("mappings", {}).get("properties", {})
            cols: list[DiscoveredColumn] = [
                DiscoveredColumn(
                    name="_id", native_type="keyword", data_type="STRING", nullable=False
                )
            ]
            typed_pairs: list[tuple[str, Any]] = [("_id", "STRING")]
            for field_name, field_spec in props.items():
                es_type = str(field_spec.get("type") or "object")
                dt = map_native_type(es_type, dialect="elasticsearch")
                cols.append(
                    DiscoveredColumn(
                        name=str(field_name),
                        native_type=es_type,
                        data_type=dt,
                        nullable=True,
                    )
                )
                typed_pairs.append((str(field_name), dt))
            out.append(
                DiscoveredObject(
                    ref={"index": str(idx_name)},
                    display_name=str(idx_name),
                    source_kind="NOSQL_COLLECTION",
                    columns=cols,
                    primary_key=["_id"],
                    suggested_cursor=suggest_cursor_column(typed_pairs),
                )
            )
        return out

    def preview(self, ref: dict[str, Any], limit: int) -> pa.Table:
        idx = str(ref.get("index") or "")
        if not idx:
            raise SidecarError(400, "INVALID_ARGUMENT", "Elasticsearch ref missing 'index'")
        resp = self._get_client().search(
            index=idx,
            query=ref.get("query") or {"match_all": {}},
            size=min(max(limit, 1), 1000),
        )
        hits = resp.get("hits", {}).get("hits", [])
        docs = [{"_id": h.get("_id"), **(h.get("_source") or {})} for h in hits]
        return _flatten_docs_to_arrow(docs)

    def read(
        self,
        ref: dict[str, Any],
        mode: str,
        cursor_column: str | None,
        cursor_from: str | None,
        batch_rows: int = 50_000,
        stream_offsets: dict[str, str] | None = None,
    ) -> Iterator[ReadBatch]:
        idx = str(ref.get("index") or "")
        if not idx:
            raise SidecarError(400, "INVALID_ARGUMENT", "Elasticsearch ref missing 'index'")
        cli = self._get_client()
        query: dict[str, Any] = ref.get("query") or {"match_all": {}}
        if mode.upper() == "APPEND" and cursor_column and cursor_from is not None:
            query = {
                "bool": {
                    "must": [query],
                    "filter": [{"range": {cursor_column: {"gt": cursor_from}}}],
                }
            }

        sort_field = cursor_column or "_shard_doc"
        page_size = min(max(batch_rows, 100), 10_000)
        search_after: list[Any] | None = None

        while True:
            kwargs: dict[str, Any] = {
                "index": idx,
                "query": query,
                "size": page_size,
                "sort": [{sort_field: "asc"}],
            }
            if search_after is not None:
                kwargs["search_after"] = search_after
            resp = cli.search(**kwargs)
            hits = resp.get("hits", {}).get("hits", [])
            if not hits:
                break
            docs = [{"_id": h.get("_id"), **(h.get("_source") or {})} for h in hits]
            search_after = hits[-1].get("sort")
            tbl = _flatten_docs_to_arrow(docs)
            max_c = format_cursor_value(docs[-1].get(cursor_column)) if cursor_column else None
            for rb in tbl.to_batches(max_chunksize=max(batch_rows, 1)):
                yield ReadBatch(batch=rb, cursor_to=max_c)
            if len(hits) < page_size:
                break

    def close(self) -> None:
        if self._client is not None:
            self._client.close()
            self._client = None


class CassandraConnector(BaseConnector):
    """Apache Cassandra connector (full image tier)."""

    spec = CASSANDRA_SPEC

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
        pts = self.config.get("contact_points") or []
        port = int(self.config.get("port") or 9042)
        if not pts:
            raise SidecarError(400, "INVALID_ARGUMENT", "Cassandra missing 'contact_points'")
        for pt in pts:
            check_outbound_host(str(pt), port, allow_private_network=allow_private_network)

    def _ensure_driver(self) -> Any:
        try:
            from cassandra.cluster import Cluster
        except ImportError as exc:
            raise SidecarError(
                422,
                "CONNECTOR_DRIVER_UNAVAILABLE",
                "cassandra-driver is not available in lite image",
            ) from exc
        return Cluster

    def test(self) -> tuple[bool, str]:
        cluster_cls = self._ensure_driver()
        try:
            cluster = cluster_cls(
                contact_points=[str(p) for p in self.config["contact_points"]],
                port=int(self.config.get("port") or 9042),
            )
            session = cluster.connect(str(self.config.get("keyspace") or "system"))
            session.execute("SELECT release_version FROM system.local")
            cluster.shutdown()
            return True, "connected"
        except Exception as exc:
            return False, str(exc)

    def preview(self, ref: dict[str, Any], limit: int) -> pa.Table:
        self._ensure_driver()
        return pa.table({})

    def read(
        self,
        ref: dict[str, Any],
        mode: str,
        cursor_column: str | None,
        cursor_from: str | None,
        batch_rows: int = 50_000,
        stream_offsets: dict[str, str] | None = None,
    ) -> Iterator[ReadBatch]:
        self._ensure_driver()
        yield from ()


class HBaseConnector(BaseConnector):
    """Apache HBase Thrift connector via happybase (full image tier)."""

    spec = HBASE_SPEC

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
        port = int(self.config.get("thrift_port") or 9090)
        if not host:
            raise SidecarError(400, "INVALID_ARGUMENT", "HBase config missing 'host'")
        check_outbound_host(host, port, allow_private_network=allow_private_network)

    def _ensure_driver(self) -> Any:
        try:
            import happybase
        except ImportError as exc:
            raise SidecarError(
                422,
                "CONNECTOR_DRIVER_UNAVAILABLE",
                "happybase driver is not available in lite image",
            ) from exc
        return happybase

    def test(self) -> tuple[bool, str]:
        hb = self._ensure_driver()
        try:
            conn = hb.Connection(
                host=str(self.config["host"]),
                port=int(self.config.get("thrift_port") or 9090),
            )
            conn.tables()
            conn.close()
            return True, "connected"
        except Exception as exc:
            return False, str(exc)

    def preview(self, ref: dict[str, Any], limit: int) -> pa.Table:
        self._ensure_driver()
        return pa.table({})

    def read(
        self,
        ref: dict[str, Any],
        mode: str,
        cursor_column: str | None,
        cursor_from: str | None,
        batch_rows: int = 50_000,
        stream_offsets: dict[str, str] | None = None,
    ) -> Iterator[ReadBatch]:
        self._ensure_driver()
        yield from ()


class Neo4jConnector(BaseConnector):
    """Neo4j read-only Bolt connector enforcing READ_ACCESS and write-Cypher rejection."""

    spec = NEO4J_SPEC

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
        uri = str(self.config.get("uri") or "").strip()
        if not uri:
            raise SidecarError(400, "INVALID_ARGUMENT", "Neo4j config missing 'uri'")
        parsed = urlparse(uri)
        if not parsed.hostname:
            raise SidecarError(400, "INVALID_ARGUMENT", "Neo4j URI missing hostname")
        check_outbound_host(
            parsed.hostname, parsed.port or 7687, allow_private_network=allow_private_network
        )
        self._driver: Any = None

    @staticmethod
    def validate_cypher_readonly(cypher: str) -> str:
        cleaned = cypher.strip()
        if not cleaned or _CYPHER_WRITE_RE.search(cleaned):
            raise SidecarError(
                400,
                "SQL_REJECTED",
                "mutating Cypher clauses (CREATE/MERGE/SET/DELETE/DROP) are forbidden",
            )
        return cleaned

    def _get_driver(self) -> Any:
        from neo4j import GraphDatabase

        if self._driver is None:
            user = str(self.secrets.get("username") or self.config.get("username") or "neo4j")
            pw = str(self.secrets.get("password") or "")
            self._driver = GraphDatabase.driver(
                str(self.config["uri"]),
                auth=(user, pw),
                connection_timeout=5.0,
            )
        return self._driver

    def test(self) -> tuple[bool, str]:
        import neo4j

        try:
            drv = self._get_driver()
            with drv.session(
                database=str(self.config.get("database") or "neo4j"),
                default_access_mode=neo4j.READ_ACCESS,
            ) as session:
                session.run("RETURN 1 AS ok").consume()
            return True, "connected"
        except Exception as exc:
            return False, str(exc)

    def discover(self) -> list[DiscoveredObject]:
        import neo4j

        drv = self._get_driver()
        out: list[DiscoveredObject] = []
        with drv.session(
            database=str(self.config.get("database") or "neo4j"),
            default_access_mode=neo4j.READ_ACCESS,
        ) as session:
            labels = [str(r[0]) for r in session.run("CALL db.labels()")]
            for lbl in labels:
                out.append(
                    DiscoveredObject(
                        ref={"label": lbl},
                        display_name=f":{lbl}",
                        source_kind="NOSQL_COLLECTION",
                        columns=[
                            DiscoveredColumn(
                                name="_id", native_type="string", data_type="STRING", nullable=False
                            )
                        ],
                        primary_key=["_id"],
                    )
                )
        return out

    def _build_cypher(self, ref: dict[str, Any], limit: int | None = None) -> str:
        if ref.get("cypher"):
            cyp = self.validate_cypher_readonly(str(ref["cypher"]))
            return f"{cyp} LIMIT {limit}" if limit and "limit" not in cyp.lower() else cyp
        lbl = str(ref.get("label") or "").strip()
        if not lbl or not re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", lbl):
            raise SidecarError(400, "INVALID_ARGUMENT", "invalid Neo4j label or cypher ref")
        cyp = f"MATCH (n:`{lbl}`) RETURN elementId(n) AS _id, properties(n) AS props"
        if limit:
            cyp += f" LIMIT {int(limit)}"
        return cyp

    def preview(self, ref: dict[str, Any], limit: int) -> pa.Table:
        import neo4j

        cyp = self._build_cypher(ref, limit=min(max(limit, 1), 1000))
        drv = self._get_driver()
        docs: list[dict[str, Any]] = []
        with drv.session(
            database=str(self.config.get("database") or "neo4j"),
            default_access_mode=neo4j.READ_ACCESS,
        ) as session:
            for rec in session.run(cyp):
                d = dict(rec.data())
                props = d.pop("props", None)
                if isinstance(props, dict):
                    d.update(props)
                docs.append(d)
        return _flatten_docs_to_arrow(docs)

    def read(
        self,
        ref: dict[str, Any],
        mode: str,
        cursor_column: str | None,
        cursor_from: str | None,
        batch_rows: int = 50_000,
        stream_offsets: dict[str, str] | None = None,
    ) -> Iterator[ReadBatch]:
        import neo4j

        cyp = self._build_cypher(ref)
        drv = self._get_driver()
        buf: list[dict[str, Any]] = []
        step = max(batch_rows, 1)
        with drv.session(
            database=str(self.config.get("database") or "neo4j"),
            default_access_mode=neo4j.READ_ACCESS,
        ) as session:
            for rec in session.run(cyp):
                d = dict(rec.data())
                props = d.pop("props", None)
                if isinstance(props, dict):
                    d.update(props)
                buf.append(d)
                if len(buf) >= step:
                    tbl = _flatten_docs_to_arrow(buf)
                    for rb in tbl.to_batches(max_chunksize=step):
                        yield ReadBatch(batch=rb)
                    buf.clear()
        if buf:
            tbl = _flatten_docs_to_arrow(buf)
            for rb in tbl.to_batches(max_chunksize=step):
                yield ReadBatch(batch=rb)

    def close(self) -> None:
        if self._driver is not None:
            self._driver.close()
            self._driver = None
