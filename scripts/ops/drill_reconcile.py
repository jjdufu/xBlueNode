#!/usr/bin/env python3
"""Reconcile source PostgreSQL + DuckLake snapshot counts against an isolated restore drill."""

from __future__ import annotations

import argparse
import os
import re
import sys
from datetime import datetime
from pathlib import Path
from urllib.parse import parse_qsl, unquote, urlparse

import psycopg

_PREFIX_RE = re.compile(r"^(sec_|ds_|xb_|ax_|xv_|an_|hg_)")
_SAFE_IDENT_RE = re.compile(r"^[a-zA-Z0-9_]+$")


def _quote_ident(ident: str) -> str:
    if not _SAFE_IDENT_RE.match(ident):
        raise ValueError(f"unsafe SQL identifier: {ident!r}")
    return f'"{ident}"'


def _libpq_quote(val: str) -> str:
    if val != "" and all(ch.isalnum() or ch in "_.-" for ch in val):
        return val
    escaped = val.replace("\\", "\\\\").replace("'", "\\'")
    return f"'{escaped}'"


def _postgres_dsn_to_libpq(dsn: str) -> str:
    trimmed = dsn.strip()
    if not trimmed.startswith(("postgres://", "postgresql://")):
        return trimmed
    parsed = urlparse(trimmed)
    parts: list[str] = []
    dbname = unquote(parsed.path.lstrip("/"))
    if dbname:
        parts.append(f"dbname={_libpq_quote(dbname)}")
    host = unquote(parsed.hostname) if parsed.hostname else "127.0.0.1"
    parts.append(f"host={_libpq_quote(host)}")
    parts.append(f"port={parsed.port if parsed.port is not None else 5432}")
    if parsed.username:
        parts.append(f"user={_libpq_quote(unquote(parsed.username))}")
    if parsed.password is not None:
        parts.append(f"password={_libpq_quote(unquote(parsed.password))}")
    for key, val in parse_qsl(parsed.query, keep_blank_values=True):
        if key:
            parts.append(f"{key}={_libpq_quote(val)}")
    return " ".join(parts)


def _lookup_backup_cutoff(
    src_conn: psycopg.Connection[tuple[object, ...]],
    backup_location: str | None,
) -> datetime | None:
    with src_conn.cursor() as cur:
        if backup_location:
            cur.execute(
                "SELECT started_at FROM ax_backups "
                "WHERE location = %s ORDER BY started_at DESC LIMIT 1",
                (backup_location,),
            )
            row = cur.fetchone()
            if row and isinstance(row[0], datetime):
                return row[0]
        cur.execute(
            "SELECT started_at FROM ax_backups ORDER BY started_at DESC LIMIT 1"
        )
        row = cur.fetchone()
        if row and isinstance(row[0], datetime):
            return row[0]
    return None


def _discover_governed_tables(
    conn: psycopg.Connection[tuple[object, ...]],
) -> dict[str, str | None]:
    """Map table_name to timestamp cutoff column ('created_at' | 'started_at' | None)."""
    tables: dict[str, str | None] = {}
    with conn.cursor() as cur:
        cur.execute(
            """
            SELECT t.table_name,
                   MAX(CASE WHEN c.column_name = 'created_at' THEN 'created_at'
                            WHEN c.column_name = 'started_at' THEN 'started_at'
                            ELSE NULL END) AS ts_col
              FROM information_schema.tables t
              LEFT JOIN information_schema.columns c
                ON c.table_schema = t.table_schema AND c.table_name = t.table_name
             WHERE t.table_schema = 'public'
               AND t.table_type = 'BASE TABLE'
             GROUP BY t.table_name
             ORDER BY t.table_name
            """
        )
        for tbl_obj, ts_obj in cur.fetchall():
            tbl = str(tbl_obj)
            if not _PREFIX_RE.match(tbl):
                continue
            # Partition children of xb_metric_series are counted via parent xb_metric_series
            if tbl.startswith("xb_metric_series_") and tbl != "xb_metric_series":
                continue
            tables[tbl] = str(ts_obj) if ts_obj is not None else None
    return tables


def _count_table(
    conn: psycopg.Connection[tuple[object, ...]],
    table_name: str,
    ts_col: str | None,
    cutoff: datetime | None,
) -> int:
    q_tbl = _quote_ident(table_name)
    with conn.cursor() as cur:
        if ts_col is not None and cutoff is not None:
            q_col = _quote_ident(ts_col)
            cur.execute(
                f"SELECT count(*) FROM public.{q_tbl} WHERE {q_col} IS NULL OR {q_col} <= %s",
                (cutoff,),
            )
        else:
            cur.execute(f"SELECT count(*) FROM public.{q_tbl}")
        row = cur.fetchone()
        return int(row[0]) if row and isinstance(row[0], int) else 0


def _reconcile_tables(
    src_dsn: str,
    tgt_dsn: str,
    backup_location: str | None,
) -> list[str]:
    mismatches: list[str] = []
    with psycopg.connect(src_dsn) as src_conn, psycopg.connect(tgt_dsn) as tgt_conn:
        cutoff = _lookup_backup_cutoff(src_conn, backup_location)
        src_tables = _discover_governed_tables(src_conn)
        tgt_tables = _discover_governed_tables(tgt_conn)

        missing_in_target = sorted(set(src_tables) - set(tgt_tables))
        for tbl in missing_in_target:
            mismatches.append(f"table public.{tbl} missing in restored database")

        for tbl, ts_col in src_tables.items():
            if tbl not in tgt_tables:
                continue
            src_cnt = _count_table(src_conn, tbl, ts_col, cutoff)
            tgt_cnt = _count_table(tgt_conn, tbl, ts_col, cutoff)
            if src_cnt != tgt_cnt:
                mismatches.append(
                    f"table public.{tbl}: source_count={src_cnt} != restored_count={tgt_cnt}"
                )
    return mismatches


def _reconcile_ducklake(tgt_dsn: str, lake_root: Path) -> list[str]:
    mismatches: list[str] = []
    with psycopg.connect(tgt_dsn) as tgt_conn, tgt_conn.cursor() as cur:
        cur.execute(
            """
            SELECT count(*)
              FROM information_schema.tables
             WHERE table_schema = 'lake_catalog'
               AND table_name = 'ducklake_snapshot'
            """
        )
        cat_row = cur.fetchone()
        has_catalog = bool(cat_row and cat_row[0])
        if not has_catalog:
            return mismatches

        cur.execute(
            """
            SELECT d.id, d.tenant_id, d.lake_schema, d.lake_table, d.row_count,
                   (SELECT sj.rows_written
                      FROM ds_sync_jobs sj
                     WHERE sj.dataset_id = d.id AND sj.status = 'SUCCEEDED'
                     ORDER BY sj.finished_at DESC NULLS LAST, sj.started_at DESC
                     LIMIT 1) AS latest_sync_rows,
                   d.sync_mode
              FROM ds_datasets d
             WHERE d.status = 'READY'
               AND d.sync_mode <> 'DIRECT'
               AND d.lake_table <> ''
               AND EXISTS (
                   SELECT 1
                     FROM lake_catalog.ducklake_table dt
                     JOIN lake_catalog.ducklake_schema ds ON ds.schema_id = dt.schema_id
                    WHERE ds.schema_name = d.lake_schema
                      AND dt.table_name = d.lake_table
                      AND dt.end_snapshot IS NULL
                      AND ds.end_snapshot IS NULL
               )
            """
        )
        datasets = cur.fetchall()

    if not datasets:
        return mismatches

    import duckdb

    lake_data_dir = (lake_root / "lake").resolve()
    lake_data_dir.mkdir(parents=True, exist_ok=True)
    pg_libpq = _postgres_dsn_to_libpq(tgt_dsn)

    con = duckdb.connect()
    try:
        con.execute("LOAD postgres; LOAD ducklake;")
    except duckdb.Error:
        con.execute("INSTALL postgres; INSTALL ducklake; LOAD postgres; LOAD ducklake;")

    escaped_path = str(lake_data_dir).replace("'", "''") + os.sep
    escaped_dsn = f"ducklake:postgres:{pg_libpq}".replace("'", "''")
    con.execute(
        f"ATTACH '{escaped_dsn}' AS lake "
        f"(DATA_PATH '{escaped_path}', METADATA_SCHEMA 'lake_catalog', "
        "OVERRIDE_DATA_PATH true, READ_ONLY)"
    )

    for (
        ds_id,
        tenant_id,
        lake_schema,
        lake_table,
        ds_row_count,
        latest_sync_rows,
        sync_mode,
    ) in datasets:
        schema_ident = _quote_ident(str(lake_schema))
        table_ident = _quote_ident(str(lake_table))
        try:
            res = con.execute(f"SELECT count(*) FROM lake.{schema_ident}.{table_ident}").fetchone()
            actual_rows = int(res[0]) if res is not None else 0
        except duckdb.Error as exc:
            mismatches.append(
                f"dataset {ds_id} ({tenant_id}.{lake_table}) unreadable in DuckLake: {exc}"
            )
            continue

        expected = int(ds_row_count) if isinstance(ds_row_count, int) else None
        if str(sync_mode) == "SNAPSHOT" and isinstance(latest_sync_rows, int):
            expected = int(latest_sync_rows)
        if expected is not None and actual_rows != expected:
            mismatches.append(
                f"dataset {ds_id} ({tenant_id}.{lake_table}): "
                f"ducklake_rows={actual_rows} != expected={expected}"
            )

    con.close()
    return mismatches


def main() -> int:
    parser = argparse.ArgumentParser(description="Reconcile restored xBlueNode backup.")
    parser.add_argument("--source", required=True, help="Source MIGRATION_DATABASE_URL")
    parser.add_argument("--target", required=True, help="Restored drill PostgreSQL DSN")
    parser.add_argument("--lake-root", required=True, help="Restored files root containing lake/")
    parser.add_argument(
        "--backup-location", default=None, help="Backup directory path in ax_backups"
    )
    args = parser.parse_args()

    errors = _reconcile_tables(args.source, args.target, args.backup_location)
    errors.extend(_reconcile_ducklake(args.target, Path(args.lake_root)))

    if errors:
        for err in errors:
            print(f"drill_reconcile MISMATCH: {err}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
