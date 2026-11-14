from __future__ import annotations

import os
import threading
from collections.abc import Iterator
from contextlib import contextmanager
from pathlib import Path
from urllib.parse import unquote, urlparse

import duckdb

from security import SidecarError, validate_lake_table, validate_tenant_id
from settings import get_data_root, settings

_LAKE_LOCK = threading.RLock()


def _uri_to_libpq(uri: str) -> str:
    u = urlparse(uri)
    parts = [
        f"dbname={u.path.lstrip('/')}",
        f"host={u.hostname or '127.0.0.1'}",
        f"port={u.port or 5432}",
    ]
    if u.username:
        parts.append(f"user={unquote(u.username)}")
    if u.password:
        parts.append(f"password={unquote(u.password)}")
    return " ".join(parts)


def _resolve_catalog_dsn() -> str:
    try:
        dsn = settings.axpoint_lake_catalog_dsn
        if dsn:
            return dsn
    except Exception:
        pass
    return os.getenv("AXPOINT_LAKE_CATALOG_DSN", "").strip()


def tenant_schema_name(tenant_id: str) -> str:
    """Return unquoted DuckLake schema name for tenant (`tenant_<id>`)."""
    valid = validate_tenant_id(tenant_id)
    return f"tenant_{valid}"


def tenant_schema(tenant_id: str) -> str:
    """Return double-quoted DuckLake schema identifier (`\"tenant_<id>\"`)."""
    name = tenant_schema_name(tenant_id)
    return '"' + name.replace('"', '""') + '"'


def quote_lake_table(table_name: str) -> str:
    """Validate and double-quote a DuckLake table name."""
    valid = validate_lake_table(table_name)
    return '"' + valid.replace('"', '""') + '"'


def quote_lake_col(col_name: str) -> str:
    """Safely double-quote a DuckLake column identifier (supporting CJK, ., @, #)."""
    if not col_name or "\x00" in col_name or any(ord(c) < 32 for c in col_name):
        raise SidecarError(400, "INVALID_ARGUMENT", f"invalid column name: {col_name!r}")
    return '"' + col_name.replace('"', '""') + '"'


def _sql_quote_str(val: str) -> str:
    return "'" + val.replace("'", "''") + "'"


@contextmanager
def lake_connection(
    *,
    data_root: Path | None = None,
    catalog_dsn: str | None = None,
    lake_only_dirs: bool = False,
) -> Iterator[duckdb.DuckDBPyConnection]:
    """Open an isolated, hardened DuckDB session attached to DuckLake under process RLock."""
    root = (data_root or get_data_root()).resolve()
    lake_dir = root / "lake"
    tenants_dir = root / "tenants"
    lake_dir.mkdir(parents=True, exist_ok=True)
    tenants_dir.mkdir(parents=True, exist_ok=True)

    raw_dsn = (catalog_dsn if catalog_dsn is not None else _resolve_catalog_dsn()).strip()

    with _LAKE_LOCK:
        con = duckdb.connect(
            config={
                "allow_unsigned_extensions": "false",
                "autoinstall_known_extensions": "false",
            }
        )
        try:
            con.execute("LOAD ducklake; LOAD postgres;")

            allowed_dirs: list[Path] = (
                [lake_dir] if lake_only_dirs else [lake_dir, tenants_dir, root]
            )
            lake_path_sql = _sql_quote_str(f"{lake_dir.as_posix()}/")

            if raw_dsn.startswith(("postgres://", "postgresql://")):
                attach_target = _sql_quote_str(f"ducklake:postgres:{_uri_to_libpq(raw_dsn)}")
                con.execute(
                    f"ATTACH {attach_target} AS lake "
                    f"(DATA_PATH {lake_path_sql}, METADATA_SCHEMA 'lake_catalog');"
                )
            elif raw_dsn.startswith("ducklake:postgres:"):
                attach_target = _sql_quote_str(raw_dsn)
                con.execute(
                    f"ATTACH {attach_target} AS lake "
                    f"(DATA_PATH {lake_path_sql}, METADATA_SCHEMA 'lake_catalog');"
                )
            elif raw_dsn.startswith("ducklake:"):
                sub_path = raw_dsn.removeprefix("ducklake:")
                if sub_path and "://" not in sub_path and not sub_path.startswith("sqlite:"):
                    p = Path(sub_path).resolve()
                    p.parent.mkdir(parents=True, exist_ok=True)
                    allowed_dirs.append(p.parent)
                    attach_target = _sql_quote_str(f"ducklake:{p.as_posix()}")
                else:
                    attach_target = _sql_quote_str(raw_dsn)
                con.execute(f"ATTACH {attach_target} AS lake (DATA_PATH {lake_path_sql});")
            else:
                cat_file = (
                    Path(raw_dsn).resolve()
                    if raw_dsn
                    else (root / "lake_catalog.ducklake").resolve()
                )
                cat_file.parent.mkdir(parents=True, exist_ok=True)
                allowed_dirs.append(cat_file.parent)
                attach_target = _sql_quote_str(f"ducklake:{cat_file.as_posix()}")
                con.execute(f"ATTACH {attach_target} AS lake (DATA_PATH {lake_path_sql});")

            unique_dirs = list(dict.fromkeys(d.resolve().as_posix() for d in allowed_dirs))
            dirs_sql = ", ".join(_sql_quote_str(d) for d in unique_dirs)
            con.execute(
                f"SET allowed_directories = [{dirs_sql}]; "
                "SET threads = 4; "
                "SET memory_limit = '2GB'; "
                "SET enable_external_access = false; "
                "SET lock_configuration = true;"
            )
            yield con
        finally:
            con.close()
