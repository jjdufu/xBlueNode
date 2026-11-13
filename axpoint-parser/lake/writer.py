from __future__ import annotations

from collections.abc import Sequence
from datetime import UTC, datetime
from pathlib import Path

import duckdb
import polars as pl
import pyarrow as pa
import pyarrow.parquet as pq

from connectors.base import arrow_table_to_preview, format_cursor_value
from connectors.registry import create_connector
from lake.catalog import (
    lake_connection,
    quote_lake_col,
    quote_lake_table,
    tenant_schema,
    tenant_schema_name,
)
from lake.evolve import (
    apply_ddl_evolution_to_table,
    datatype_to_duckdb_sql,
    duckdb_type_to_datatype,
    evaluate_schema_evolution,
)
from lake.quality import evaluate_quality
from parsers.profiler import profile_polars_df
from schemas import (
    ColumnProfile,
    CommitStagingRequest,
    LakePreviewRequest,
    LakeSnapshotInfo,
    PreviewResponse,
    QualityRule,
    SchemaChange,
    SchemaPolicy,
    SyncMode,
    SyncRequest,
    SyncResult,
)
from security import (
    SidecarError,
    resolve_path_under_root,
    safe_tenant_dir,
    validate_lake_table,
    validate_tenant_id,
)
from settings import get_data_root


def _table_exists(con: duckdb.DuckDBPyConnection, schema_name: str, table_name: str) -> bool:
    row = con.execute(
        "SELECT 1 FROM information_schema.tables "
        "WHERE table_catalog = 'lake' AND table_schema = ? AND table_name = ? LIMIT 1",
        [schema_name, table_name],
    ).fetchone()
    return row is not None


def _get_existing_lake_columns(
    con: duckdb.DuckDBPyConnection, schema_name: str, table_name: str
) -> list[tuple[str, str]]:
    rows = con.execute(
        "SELECT column_name, data_type FROM information_schema.columns "
        "WHERE table_catalog = 'lake' AND table_schema = ? AND table_name = ? "
        "ORDER BY ordinal_position",
        [schema_name, table_name],
    ).fetchall()
    return [(str(r[0]), str(r[1])) for r in rows]


def _current_snapshot_id(con: duckdb.DuckDBPyConnection) -> int:
    row = con.execute("SELECT * FROM ducklake_current_snapshot('lake')").fetchone()
    return int(row[0]) if row and row[0] is not None else 0


def _build_projection_select(
    source_view: str,
    target_user_schema: Sequence[ColumnProfile],
    incoming_cols: set[str],
    *,
    include_msg_id: bool,
    batch_id: str,
) -> tuple[list[str], str]:
    """Build (quoted_cols, SELECT_sql) casting incoming columns and NULL-padding missing cols."""
    target_col_idents: list[str] = []
    select_exprs: list[str] = []

    for col_prof in target_user_schema:
        q_c = quote_lake_col(col_prof.name)
        sql_t = datatype_to_duckdb_sql(col_prof.data_type)
        target_col_idents.append(q_c)
        if col_prof.name in incoming_cols:
            select_exprs.append(f"CAST({q_c} AS {sql_t}) AS {q_c}")
        else:
            select_exprs.append(f"CAST(NULL AS {sql_t}) AS {q_c}")

    if include_msg_id:
        q_mid = quote_lake_col("_ax_msg_id")
        target_col_idents.append(q_mid)
        if "_ax_msg_id" in incoming_cols:
            select_exprs.append(f"CAST({q_mid} AS VARCHAR) AS {q_mid}")
        else:
            select_exprs.append(f"CAST(NULL AS VARCHAR) AS {q_mid}")

    safe_bid = "'" + batch_id.replace("'", "''") + "'"
    target_col_idents.append(quote_lake_col("_ax_batch_id"))
    select_exprs.append(f'{safe_bid}::VARCHAR AS "_ax_batch_id"')

    target_col_idents.append(quote_lake_col("_ax_synced_at"))
    select_exprs.append('now()::TIMESTAMPTZ AS "_ax_synced_at"')

    select_sql = f"SELECT {', '.join(select_exprs)} FROM {source_view}"
    return target_col_idents, select_sql


def _write_df_to_ducklake(
    *,
    tenant_id: str,
    dataset_id: str,
    batch_id: str,
    lake_table: str,
    sync_mode: SyncMode,
    primary_key: Sequence[str],
    cursor_column: str | None,
    delete_column: str | None,
    schema_policy: SchemaPolicy,
    expected_schema: Sequence[ColumnProfile],
    quality_rules: Sequence[QualityRule],
    raw_df: pl.DataFrame,
    rows_read: int,
    observed_cursor_to: str | None,
    stream_offsets: dict[str, str],
    timezone_str: str = "Asia/Shanghai",
    data_root: Path | None = None,
    catalog_dsn: str | None = None,
    raise_on_quality_block: bool = False,
) -> SyncResult:
    """Core transactional DuckLake writer shared by /connectors/sync and /lake/commit_staging."""
    valid_tenant = validate_tenant_id(tenant_id)
    valid_table = validate_lake_table(lake_table)
    s_raw = tenant_schema_name(valid_tenant)
    s_q = tenant_schema(valid_tenant)
    t_q = quote_lake_table(valid_table)
    full_tbl = f"lake.{s_q}.{t_q}"

    if sync_mode == "UPSERT" and not primary_key:
        raise SidecarError(400, "INVALID_ARGUMENT", "UPSERT mode requires non-empty primary_key")

    has_msg_id = "_ax_msg_id" in raw_df.columns
    work_df = raw_df

    # 1. Deduplicate _ax_msg_id within incoming batch for stream/webhook idempotency
    if has_msg_id and work_df.height > 0:
        work_df = work_df.unique(subset=["_ax_msg_id"], keep="first", maintain_order=True)

    # 2. Deduplicate UPSERT batch by primary_key (keep latest row)
    if sync_mode == "UPSERT" and work_df.height > 0:
        missing_pks = [pk for pk in primary_key if pk not in work_df.columns]
        if missing_pks:
            raise SidecarError(
                400,
                "INVALID_ARGUMENT",
                f"primary_key columns {missing_pks} missing from incoming data",
            )
        if cursor_column and cursor_column in work_df.columns:
            work_df = work_df.sort(cursor_column)
        work_df = work_df.unique(subset=list(primary_key), keep="last", maintain_order=True)

    # Separate user columns from _ax_* system columns
    user_cols = [c for c in work_df.columns if not c.startswith("_ax_")]
    user_df = work_df.select(user_cols) if user_cols else pl.DataFrame()
    incoming_profiles = (
        profile_polars_df(user_df, timezone_str=timezone_str)
        if user_cols
        else list(expected_schema)
    )

    with lake_connection(data_root=data_root, catalog_dsn=catalog_dsn) as con:
        con.execute(f"CREATE SCHEMA IF NOT EXISTS lake.{s_q}")
        exists = _table_exists(con, s_raw, valid_table)

        # Resolve baseline schema from DuckLake catalog if caller passed empty on existing table
        effective_expected = list(expected_schema)
        if not effective_expected and exists:
            existing_cols = _get_existing_lake_columns(con, s_raw, valid_table)
            effective_expected = [
                ColumnProfile(
                    name=c_name,
                    ordinal=idx,
                    data_type=duckdb_type_to_datatype(c_type),
                    nullable=True,
                )
                for idx, (c_name, c_type) in enumerate(existing_cols)
                if not c_name.startswith("_ax_")
            ]

        # If APPEND / UPSERT produced 0 rows from source, evaluate ROW_COUNT_MIN if any, else no-op
        if work_df.height == 0 and sync_mode in {"APPEND", "UPSERT"} and exists:
            q_rep = evaluate_quality(user_df, quality_rules)
            if q_rep.blocked:
                if raise_on_quality_block:
                    raise SidecarError(
                        422, "QUALITY_BLOCKED", "quality block rule failed on empty batch"
                    )
                return SyncResult(
                    dataset_id=dataset_id,
                    rows_read=rows_read,
                    rows_written=0,
                    rows_deleted=0,
                    cursor_to=observed_cursor_to,
                    stream_offsets=stream_offsets,
                    snapshot_id=_current_snapshot_id(con),
                    schema=effective_expected,
                    schema_change=SchemaChange(),
                    quality=q_rep,
                )
            return SyncResult(
                dataset_id=dataset_id,
                rows_read=rows_read,
                rows_written=0,
                rows_deleted=0,
                cursor_to=observed_cursor_to,
                stream_offsets=stream_offsets,
                snapshot_id=_current_snapshot_id(con),
                schema=effective_expected,
                schema_change=SchemaChange(),
                quality=q_rep,
            )

        # Evaluate Schema Evolution (raises 422 SCHEMA_DRIFT_BLOCKED on policy violation)
        schema_change, resolved_schema = evaluate_schema_evolution(
            effective_expected, incoming_profiles, schema_policy
        )

        # Split UPSERT soft-deletes before evaluating quality on active rows
        soft_del_df: pl.DataFrame | None = None
        active_work_df = work_df
        if (
            sync_mode == "UPSERT"
            and delete_column
            and delete_column in work_df.columns
            and work_df.height > 0
        ):
            del_mask = work_df[delete_column].cast(pl.Boolean).fill_null(False)
            soft_del_df = work_df.filter(del_mask).select(list(primary_key))
            active_work_df = work_df.filter(~del_mask)

        eval_user_df = (
            active_work_df.select([c for c in active_work_df.columns if not c.startswith("_ax_")])
            if user_cols
            else pl.DataFrame()
        )
        quality_report = evaluate_quality(eval_user_df, quality_rules)
        if quality_report.blocked:
            if raise_on_quality_block:
                failed_details = "; ".join(
                    f"{r.rule_kind}({r.column_name or '*'}): {r.detail}"
                    for r in quality_report.rules
                    if not r.passed and r.on_fail == "BLOCK"
                )
                raise SidecarError(
                    422,
                    "QUALITY_BLOCKED",
                    f"dataset quality block rule failed: {failed_details}",
                )
            return SyncResult(
                dataset_id=dataset_id,
                rows_read=rows_read,
                rows_written=0,
                rows_deleted=0,
                cursor_to=None,
                stream_offsets={},
                snapshot_id=_current_snapshot_id(con),
                schema=effective_expected or resolved_schema,
                schema_change=schema_change,
                quality=quality_report,
            )

        rows_written = 0
        rows_deleted = 0
        existing_has_msg_id = False
        if exists:
            existing_col_names = {c[0] for c in _get_existing_lake_columns(con, s_raw, valid_table)}
            existing_has_msg_id = "_ax_msg_id" in existing_col_names

        include_msg_id = has_msg_id or existing_has_msg_id

        con.execute("BEGIN")
        try:
            if sync_mode == "SNAPSHOT" or not exists:
                if exists:
                    con.execute(f"DROP TABLE {full_tbl}")
                if active_work_df.height == 0 and not resolved_schema:
                    con.execute(
                        f"CREATE TABLE {full_tbl} "
                        '("_ax_batch_id" VARCHAR, "_ax_synced_at" TIMESTAMPTZ)'
                    )
                else:
                    arrow_in = active_work_df.to_arrow()
                    con.register("_ax_src_batch", arrow_in)
                    try:
                        _cols_q, sel_sql = _build_projection_select(
                            "_ax_src_batch",
                            resolved_schema,
                            set(active_work_df.columns),
                            include_msg_id=include_msg_id,
                            batch_id=batch_id,
                        )
                        con.execute(f"CREATE TABLE {full_tbl} AS {sel_sql}")
                    finally:
                        con.unregister("_ax_src_batch")
                rows_written = active_work_df.height

            elif sync_mode == "APPEND":
                # 1) Retry idempotency: purge any rows previously written with same batch_id
                con.execute(f'DELETE FROM {full_tbl} WHERE "_ax_batch_id" = ?', [batch_id])

                # Apply DDL evolution if needed (plus add _ax_msg_id if newly introduced)
                apply_ddl_evolution_to_table(con, full_tbl, schema_change, schema_policy)
                if has_msg_id and not existing_has_msg_id:
                    con.execute(f'ALTER TABLE {full_tbl} ADD COLUMN "_ax_msg_id" VARCHAR')
                    existing_has_msg_id = True

                arrow_in = active_work_df.to_arrow()
                con.register("_ax_raw_append", arrow_in)
                try:
                    where_clauses: list[str] = []
                    # 2) Cursor crash-recovery deduplication against lake max(cursor)
                    if cursor_column and cursor_column in active_work_df.columns:
                        q_cur = quote_lake_col(cursor_column)
                        max_in_lake_row = con.execute(
                            f"SELECT max({q_cur}) FROM {full_tbl}"
                        ).fetchone()
                        if max_in_lake_row and max_in_lake_row[0] is not None:
                            where_clauses.append(
                                f"r.{q_cur} > (SELECT max({q_cur}) FROM {full_tbl})"
                            )

                    if has_msg_id and existing_has_msg_id:
                        where_clauses.append(
                            'r."_ax_msg_id" NOT IN '
                            f'(SELECT "_ax_msg_id" FROM {full_tbl} WHERE "_ax_msg_id" IS NOT NULL)'
                        )

                    flt_sql = (
                        f"SELECT r.* FROM _ax_raw_append r WHERE {' AND '.join(where_clauses)}"
                        if where_clauses
                        else "SELECT * FROM _ax_raw_append"
                    )
                    con.execute(f"CREATE TEMP TABLE _ax_filtered_append AS {flt_sql}")
                    try:
                        cnt_row = con.execute("SELECT count(*) FROM _ax_filtered_append").fetchone()
                        rows_written = int(cnt_row[0]) if cnt_row else 0
                        if rows_written > 0:
                            cols_q, sel_sql = _build_projection_select(
                                "_ax_filtered_append",
                                resolved_schema,
                                set(active_work_df.columns),
                                include_msg_id=include_msg_id,
                                batch_id=batch_id,
                            )
                            con.execute(f"INSERT INTO {full_tbl} ({', '.join(cols_q)}) {sel_sql}")
                    finally:
                        con.execute("DROP TABLE IF EXISTS _ax_filtered_append")
                finally:
                    con.unregister("_ax_raw_append")

            elif sync_mode == "UPSERT":
                apply_ddl_evolution_to_table(con, full_tbl, schema_change, schema_policy)
                pk_join = " AND ".join(
                    f"t.{quote_lake_col(pk)} IS NOT DISTINCT FROM s.{quote_lake_col(pk)}"
                    for pk in primary_key
                )

                # Handle soft-deleted keys
                if soft_del_df is not None and soft_del_df.height > 0:
                    con.register("_ax_del_keys", soft_del_df.to_arrow())
                    try:
                        del_cnt_row = con.execute(
                            f"SELECT count(*) FROM {full_tbl} t "
                            f"WHERE EXISTS (SELECT 1 FROM _ax_del_keys s WHERE {pk_join})"
                        ).fetchone()
                        rows_deleted = int(del_cnt_row[0]) if del_cnt_row else 0
                        if rows_deleted > 0:
                            con.execute(
                                f"DELETE FROM {full_tbl} t USING _ax_del_keys s WHERE {pk_join}"
                            )
                    finally:
                        con.unregister("_ax_del_keys")

                # Upsert active rows (transactional DELETE matching PKs + INSERT)
                if active_work_df.height > 0:
                    con.register("_ax_upsert_src", active_work_df.to_arrow())
                    try:
                        con.execute(
                            f"DELETE FROM {full_tbl} t USING _ax_upsert_src s WHERE {pk_join}"
                        )
                        cols_q, sel_sql = _build_projection_select(
                            "_ax_upsert_src",
                            resolved_schema,
                            set(active_work_df.columns),
                            include_msg_id=include_msg_id,
                            batch_id=batch_id,
                        )
                        con.execute(f"INSERT INTO {full_tbl} ({', '.join(cols_q)}) {sel_sql}")
                        rows_written = active_work_df.height
                    finally:
                        con.unregister("_ax_upsert_src")

            con.execute("COMMIT")
        except Exception:
            try:
                con.execute("ROLLBACK")
            except Exception:
                pass
            raise

        snap_id = _current_snapshot_id(con)
        total_cnt_row = con.execute(f"SELECT count(*) FROM {full_tbl}").fetchone()
        total_rows = int(total_cnt_row[0]) if total_cnt_row else 0

        # Re-compute user column profiles on latest committed data (up to 100k sample)
        if resolved_schema:
            sel_user = ", ".join(quote_lake_col(c.name) for c in resolved_schema)
            committed_arrow = (
                con.cursor().sql(f"SELECT {sel_user} FROM {full_tbl} LIMIT 100000").to_arrow_table()
            )
            committed_pl = pl.from_arrow(committed_arrow)
            assert isinstance(committed_pl, pl.DataFrame)
            type_map = {c.name: c.data_type for c in resolved_schema}
            final_profiles = profile_polars_df(
                committed_pl, timezone_str=timezone_str, type_overrides=type_map
            )
        else:
            final_profiles = []

        # Determine final cursor_to
        final_cursor_to = observed_cursor_to
        if (
            final_cursor_to is None
            and cursor_column
            and cursor_column in active_work_df.columns
            and active_work_df.height > 0
        ):
            non_null_c = active_work_df[cursor_column].drop_nulls()
            if non_null_c.len() > 0:
                final_cursor_to = format_cursor_value(non_null_c.max())

        return SyncResult(
            dataset_id=dataset_id,
            rows_read=rows_read,
            rows_written=rows_written,
            rows_deleted=rows_deleted,
            total_rows=total_rows,
            cursor_to=final_cursor_to,
            stream_offsets=stream_offsets,
            snapshot_id=snap_id,
            schema=final_profiles,
            schema_change=schema_change,
            quality=quality_report,
        )


def sync_dataset(
    req: SyncRequest,
    *,
    timezone_str: str = "Asia/Shanghai",
    data_root: Path | None = None,
    catalog_dsn: str | None = None,
    raise_on_quality_block: bool = False,
) -> SyncResult:
    """Execute full source read -> schema evolve -> quality check -> DuckLake atomic commit."""
    valid_tenant = validate_tenant_id(req.tenant_id)
    connector = create_connector(
        req.connector_type,
        req.config,
        req.secrets,
        allow_private_network=req.allow_private_network,
        tenant_id=valid_tenant,
    )
    batches: list[pa.RecordBatch] = []
    max_cursor_to: str | None = None
    merged_offsets: dict[str, str] = dict(req.stream_offsets)
    rows_read = 0

    try:
        for rb_item in connector.read(
            req.source_ref,
            req.sync_mode,
            req.cursor_column,
            req.cursor_from,
            batch_rows=req.batch_rows,
            stream_offsets=req.stream_offsets,
        ):
            if rb_item.batch.num_rows > 0:
                batches.append(rb_item.batch)
                rows_read += rb_item.batch.num_rows
            if rb_item.cursor_to is not None:
                max_cursor_to = rb_item.cursor_to
            if rb_item.offsets:
                merged_offsets.update(rb_item.offsets)
    finally:
        connector.close()

    if batches:
        arrow_tbl = pa.Table.from_batches(batches)
        raw_df = pl.from_arrow(arrow_tbl)
        assert isinstance(raw_df, pl.DataFrame)
    else:
        raw_df = pl.DataFrame()

    return _write_df_to_ducklake(
        tenant_id=valid_tenant,
        dataset_id=req.dataset_id,
        batch_id=req.batch_id,
        lake_table=req.lake_table,
        sync_mode=req.sync_mode,
        primary_key=req.primary_key,
        cursor_column=req.cursor_column,
        delete_column=req.delete_column,
        schema_policy=req.schema_policy,
        expected_schema=req.expected_schema,
        quality_rules=req.quality_rules,
        raw_df=raw_df,
        rows_read=rows_read,
        observed_cursor_to=max_cursor_to,
        stream_offsets=merged_offsets,
        timezone_str=timezone_str,
        data_root=data_root,
        catalog_dsn=catalog_dsn,
        raise_on_quality_block=raise_on_quality_block,
    )


def commit_staging(
    req: CommitStagingRequest,
    *,
    timezone_str: str = "Asia/Shanghai",
    data_root: Path | None = None,
    catalog_dsn: str | None = None,
) -> list[SyncResult]:
    """Commit staged Parquet tables produced by /parse/unified into DuckLake."""
    root = (data_root or get_data_root()).resolve()
    valid_tenant = validate_tenant_id(req.tenant_id)
    staging_dir = safe_tenant_dir(
        valid_tenant, "staging", req.parse_id, data_root=root, create=False
    )

    results: list[SyncResult] = []
    for item in req.items:
        pq_path = resolve_path_under_root(
            item.staging_path,
            data_root=root,
            required_prefix=staging_dir,
        )
        if not pq_path.is_file():
            raise SidecarError(
                404,
                "DATASET_NOT_FOUND",
                f"staging parquet file not found: {item.staging_path}",
            )
        arrow_tbl = pq.read_table(pq_path)
        pl_df = pl.from_arrow(arrow_tbl)
        assert isinstance(pl_df, pl.DataFrame)

        res = _write_df_to_ducklake(
            tenant_id=valid_tenant,
            dataset_id=item.dataset_id,
            batch_id=item.batch_id,
            lake_table=item.lake_table,
            sync_mode="SNAPSHOT",
            primary_key=[],
            cursor_column=None,
            delete_column=None,
            schema_policy=item.schema_policy,
            expected_schema=item.expected_schema,
            quality_rules=[],
            raw_df=pl_df,
            rows_read=pl_df.height,
            observed_cursor_to=None,
            stream_offsets={},
            timezone_str=timezone_str,
            data_root=root,
            catalog_dsn=catalog_dsn,
        )
        results.append(res)
    return results


def preview_lake(
    req: LakePreviewRequest,
    *,
    timezone_str: str = "Asia/Shanghai",
    data_root: Path | None = None,
    catalog_dsn: str | None = None,
) -> PreviewResponse:
    """Preview rows from a DuckLake table (supporting AT VERSION snapshot time-travel)."""
    valid_tenant = validate_tenant_id(req.tenant_id)
    valid_table = validate_lake_table(req.lake_table)
    s_raw = tenant_schema_name(valid_tenant)
    s_q = tenant_schema(valid_tenant)
    t_q = quote_lake_table(valid_table)
    capped = min(max(req.limit, 1), 1000)

    with lake_connection(data_root=data_root, catalog_dsn=catalog_dsn) as con:
        if not _table_exists(con, s_raw, valid_table) and req.snapshot_id is None:
            raise SidecarError(404, "DATASET_NOT_FOUND", f"lake table {valid_table!r} not found")

        at_clause = (
            f" AT (VERSION => {int(req.snapshot_id)})" if req.snapshot_id is not None else ""
        )
        try:
            arrow_all = (
                con.cursor()
                .sql(f"SELECT * FROM lake.{s_q}.{t_q}{at_clause} LIMIT {capped}")
                .to_arrow_table()
            )
        except Exception as exc:
            raise SidecarError(
                404, "DATASET_NOT_FOUND", f"unable to read lake snapshot: {exc}"
            ) from exc

    assert isinstance(arrow_all, pa.Table)
    visible_cols = [c for c in arrow_all.column_names if not c.startswith("_ax_")]
    visible_tbl = arrow_all.select(visible_cols) if visible_cols else pa.table({})
    return arrow_table_to_preview(visible_tbl, limit=capped, timezone_str=timezone_str)


def list_snapshots(
    tenant_id: str,
    lake_table: str,
    *,
    data_root: Path | None = None,
    catalog_dsn: str | None = None,
) -> list[LakeSnapshotInfo]:
    """List DuckLake snapshots relevant to a tenant table in reverse chronological order."""
    valid_tenant = validate_tenant_id(tenant_id)
    valid_table = validate_lake_table(lake_table)
    s_raw = tenant_schema_name(valid_tenant)
    qualified_marker = f"{s_raw}.{valid_table}"

    with lake_connection(data_root=data_root, catalog_dsn=catalog_dsn) as con:
        rows = con.execute(
            "SELECT snapshot_id, snapshot_time, changes FROM ducklake_snapshots('lake') "
            "ORDER BY snapshot_id DESC"
        ).fetchall()

    snapshots: list[LakeSnapshotInfo] = []
    for snap_id, snap_time, changes in rows:
        sid = int(snap_id)
        if sid == 0:
            continue
        changes_dict = changes if isinstance(changes, dict) else {}
        changes_str = str(changes_dict)
        # Filter snapshots touching this table when markers exist
        if changes_dict:
            tables_touched = (
                changes_dict.get("tables_created", [])
                + changes_dict.get("tables_inserted_into", [])
                + changes_dict.get("tables_deleted_from", [])
                + changes_dict.get("tables_altered", [])
            )
            if (
                tables_touched
                and qualified_marker not in [str(x) for x in tables_touched]
                and valid_table not in [str(x) for x in tables_touched]
            ):
                continue
            if (
                not tables_touched
                and "schemas_created" in changes_dict
                and "inlined" not in changes_str
            ):
                continue

        if isinstance(snap_time, datetime):
            ts_iso = snap_time.astimezone(UTC).isoformat().replace("+00:00", "Z")
        else:
            ts_iso = str(snap_time)

        snapshots.append(
            LakeSnapshotInfo(
                snapshot_id=sid,
                committed_at=ts_iso,
                rows_added=0,
                rows_deleted=0,
            )
        )
    return snapshots


def drop_lake_table(
    tenant_id: str,
    lake_table: str,
    *,
    data_root: Path | None = None,
    catalog_dsn: str | None = None,
) -> bool:
    """Drop a tenant's DuckLake table if it exists."""
    valid_tenant = validate_tenant_id(tenant_id)
    valid_table = validate_lake_table(lake_table)
    s_raw = tenant_schema_name(valid_tenant)
    s_q = tenant_schema(valid_tenant)
    t_q = quote_lake_table(valid_table)

    with lake_connection(data_root=data_root, catalog_dsn=catalog_dsn) as con:
        if not _table_exists(con, s_raw, valid_table):
            return False
        con.execute(f"DROP TABLE IF EXISTS lake.{s_q}.{t_q}")
        return True
