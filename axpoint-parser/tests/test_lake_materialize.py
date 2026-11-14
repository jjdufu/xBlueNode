from __future__ import annotations

from collections.abc import Iterator
from pathlib import Path

import polars as pl
import pytest
from fastapi.testclient import TestClient

from lake.catalog import lake_connection
from lake.writer import _write_df_to_ducklake, preview_lake
from main import app
from schemas import LakePreviewRequest

TEST_INTERNAL_TOKEN = "axp-step04-internal-token-0123456789abcdef"


def _seed_tenant_tables(data_root: Path, catalog_dsn: str) -> None:
    t1_orders = pl.DataFrame(
        {
            "order_id": [1, 2, 3, 4, 5],
            "region": ["east", "east", "south", "north", "south"],
            "status": ["PAID", "PAID", "PENDING", "PAID", "CANCELLED"],
            "amount": [120.5, 80.0, 50.0, 200.0, 30.0],
        }
    )
    _write_df_to_ducklake(
        tenant_id="t1",
        dataset_id="ds-orders-t1",
        batch_id="seed-t1-001",
        lake_table="orders",
        sync_mode="SNAPSHOT",
        primary_key=[],
        cursor_column=None,
        delete_column=None,
        schema_policy="ADDITIVE",
        expected_schema=[],
        quality_rules=[],
        raw_df=t1_orders,
        rows_read=t1_orders.height,
        observed_cursor_to=None,
        stream_offsets={},
        data_root=data_root,
        catalog_dsn=catalog_dsn,
    )

    t2_secret = pl.DataFrame(
        {
            "order_id": [901, 902],
            "secret_note": ["classified-alpha", "classified-beta"],
            "amount": [9999.0, 8888.0],
        }
    )
    _write_df_to_ducklake(
        tenant_id="t2",
        dataset_id="ds-secret-t2",
        batch_id="seed-t2-001",
        lake_table="secret_orders",
        sync_mode="SNAPSHOT",
        primary_key=[],
        cursor_column=None,
        delete_column=None,
        schema_policy="ADDITIVE",
        expected_schema=[],
        quality_rules=[],
        raw_df=t2_secret,
        rows_read=t2_secret.height,
        observed_cursor_to=None,
        stream_offsets={},
        data_root=data_root,
        catalog_dsn=catalog_dsn,
    )


@pytest.fixture(scope="module")
def materialize_env(
    tmp_path_factory: pytest.TempPathFactory,
) -> Iterator[tuple[TestClient, Path, str, dict[str, str]]]:
    base_tmp = tmp_path_factory.mktemp("materialize_step04")
    data_root = base_tmp / "data"
    cat_dir = base_tmp / "catalog_meta"
    cat_dir.mkdir(parents=True, exist_ok=True)
    dsn = f"ducklake:{cat_dir / 'step04.ducklake'}"

    _seed_tenant_tables(data_root, dsn)

    with pytest.MonkeyPatch.context() as mp:
        mp.setenv("AXPOINT_INTERNAL_TOKEN", TEST_INTERNAL_TOKEN)
        mp.setenv("AXPOINT_DATA_ROOT", str(data_root))
        mp.setenv("AXPOINT_LAKE_CATALOG_DSN", dsn)

        client = TestClient(app)
        headers = {
            "x-axpoint-internal-token": TEST_INTERNAL_TOKEN,
            "x-axpoint-tenant": "t1",
        }
        yield client, data_root, dsn, headers


def test_lake_only_dirs_excludes_tenants_and_root(tmp_path: Path) -> None:
    data_root = tmp_path / "data_root"
    cat_dir = tmp_path / "isolated_cat"
    cat_dir.mkdir(parents=True, exist_ok=True)
    dsn = f"ducklake:{cat_dir / 'cat.ducklake'}"

    with lake_connection(data_root=data_root, catalog_dsn=dsn, lake_only_dirs=True) as con:
        row_dirs = con.execute("SELECT current_setting('allowed_directories')").fetchone()
        assert row_dirs is not None
        allowed = [str(p).rstrip("/") for p in row_dirs[0]]
        assert (data_root.resolve() / "lake").as_posix() in allowed
        assert (data_root.resolve() / "tenants").as_posix() not in allowed
        assert data_root.resolve().as_posix() not in allowed

        row_ext = con.execute("SELECT current_setting('enable_external_access')").fetchone()
        assert row_ext is not None
        assert row_ext[0] is False


def test_materialize_cte_aggregation_overwrite_and_zero_row_schema(
    materialize_env: tuple[TestClient, Path, str, dict[str, str]],
) -> None:
    client, data_root, dsn, headers = materialize_env

    # 1. Materialize CTE + filter + aggregation query into derived_regional_paid
    cte_sql = """
    WITH paid AS (
        SELECT region, amount
        FROM lake."tenant_t1"."orders"
        WHERE status = 'PAID'
    )
    SELECT
        region,
        COUNT(*) AS paid_cnt,
        SUM(amount) AS paid_total
    FROM paid
    GROUP BY region
    ORDER BY region
    """
    resp1 = client.post(
        "/lake/materialize",
        headers=headers,
        json={
            "tenant_id": "t1",
            "dataset_id": "ds-derived-01",
            "lake_table": "derived_regional_paid",
            "sql": cte_sql,
            "batch_id": "mat-batch-001",
        },
    )
    assert resp1.status_code == 200, resp1.text
    body1 = resp1.json()
    assert body1["dataset_id"] == "ds-derived-01"
    assert body1["total_rows"] == 2
    assert body1["rows_read"] == 2
    assert body1["rows_written"] == 2
    assert body1["rows_deleted"] == 0
    snap1 = body1["snapshot_id"]
    assert snap1 > 0

    col_by_name = {c["name"]: c for c in body1["schema"]}
    assert list(col_by_name.keys()) == ["region", "paid_cnt", "paid_total"]
    assert col_by_name["region"]["data_type"] == "STRING"
    assert col_by_name["paid_cnt"]["data_type"] == "INT64"
    assert col_by_name["paid_total"]["data_type"] == "FLOAT64"

    # Verify preview_lake returns exact derived rows and hides _ax_* system columns
    prev1 = preview_lake(
        LakePreviewRequest(tenant_id="t1", lake_table="derived_regional_paid", limit=50),
        data_root=data_root,
        catalog_dsn=dsn,
    )
    assert [c.name for c in prev1.columns] == ["region", "paid_cnt", "paid_total"]
    assert all(not c.name.startswith("_ax_") for c in prev1.columns)
    assert prev1.rows == [
        {"region": "east", "paid_cnt": 2, "paid_total": 200.5},
        {"region": "north", "paid_cnt": 1, "paid_total": 200.0},
    ]

    # 2. Re-materialize the SAME lake_table referencing its own previous snapshot (self-reference)
    self_ref_sql = """
    SELECT region, paid_cnt * 10 AS scaled_cnt, paid_total
    FROM lake."tenant_t1"."derived_regional_paid"
    WHERE region = 'east'
    """
    resp_self = client.post(
        "/lake/materialize",
        headers=headers,
        json={
            "tenant_id": "t1",
            "dataset_id": "ds-derived-01",
            "lake_table": "derived_regional_paid",
            "sql": self_ref_sql,
            "batch_id": "mat-batch-002",
        },
    )
    assert resp_self.status_code == 200, resp_self.text
    body_self = resp_self.json()
    assert body_self["total_rows"] == 1
    assert body_self["snapshot_id"] > snap1

    # 3. Re-materialize the SAME lake_table with a 0-row filter; column schema must be preserved
    zero_sql = """
    SELECT
        region,
        COUNT(*) AS order_cnt,
        SUM(amount) AS total_amount
    FROM lake."tenant_t1"."orders"
    WHERE amount > 9999999
    GROUP BY region
    """
    resp_zero = client.post(
        "/lake/materialize",
        headers=headers,
        json={
            "tenant_id": "t1",
            "dataset_id": "ds-derived-01",
            "lake_table": "derived_regional_paid",
            "sql": zero_sql,
            "batch_id": "mat-batch-003",
        },
    )
    assert resp_zero.status_code == 200, resp_zero.text
    body_zero = resp_zero.json()
    assert body_zero["total_rows"] == 0
    assert body_zero["rows_read"] == 0
    assert body_zero["rows_written"] == 0
    assert body_zero["snapshot_id"] > body_self["snapshot_id"]

    zero_cols = {c["name"]: c["data_type"] for c in body_zero["schema"]}
    assert zero_cols == {
        "region": "STRING",
        "order_cnt": "INT64",
        "total_amount": "FLOAT64",
    }

    prev_zero = preview_lake(
        LakePreviewRequest(tenant_id="t1", lake_table="derived_regional_paid", limit=50),
        data_root=data_root,
        catalog_dsn=dsn,
    )
    assert prev_zero.rows == []
    assert [c.name for c in prev_zero.columns] == ["region", "order_cnt", "total_amount"]


def test_materialize_allows_whitelisted_table_functions_and_strips_ax_cols(
    materialize_env: tuple[TestClient, Path, str, dict[str, str]],
) -> None:
    client, data_root, dsn, headers = materialize_env

    # SELECT * on DuckLake source table includes _ax_batch_id/_ax_synced_at internally;
    # materialize must strip upstream _ax_* and attach fresh system columns cleanly.
    resp = client.post(
        "/lake/materialize",
        headers=headers,
        json={
            "tenant_id": "t1",
            "dataset_id": "ds-star",
            "lake_table": "orders_copy",
            "sql": 'SELECT * FROM lake."tenant_t1"."orders" WHERE order_id <= 2;',
            "batch_id": "mat-star-001",
        },
    )
    assert resp.status_code == 200, resp.text
    body = resp.json()
    assert body["total_rows"] == 2
    assert [c["name"] for c in body["schema"]] == ["order_id", "region", "status", "amount"]

    # Allowed table functions: range, generate_series, unnest
    tf_resp = client.post(
        "/lake/materialize",
        headers=headers,
        json={
            "tenant_id": "t1",
            "dataset_id": "ds-tf",
            "lake_table": "seq_nums",
            "sql": (
                "SELECT r.i AS seq_id, u.tag AS tag "
                "FROM range(3) AS r(i) "
                "CROSS JOIN unnest(['a', 'b']) AS u(tag)"
            ),
            "batch_id": "mat-tf-001",
        },
    )
    assert tf_resp.status_code == 200, tf_resp.text
    assert tf_resp.json()["total_rows"] == 6

    gs_resp = client.post(
        "/lake/materialize",
        headers=headers,
        json={
            "tenant_id": "t1",
            "dataset_id": "ds-gs",
            "lake_table": "series_nums",
            "sql": "SELECT s.v AS val FROM generate_series(1, 4) AS s(v)",
            "batch_id": "mat-gs-001",
        },
    )
    assert gs_resp.status_code == 200, gs_resp.text
    assert gs_resp.json()["total_rows"] == 4


def test_materialize_rejects_mismatched_tenant_header(
    materialize_env: tuple[TestClient, Path, str, dict[str, str]],
) -> None:
    client, _data_root, _dsn, _headers = materialize_env
    resp = client.post(
        "/lake/materialize",
        headers={
            "x-axpoint-internal-token": TEST_INTERNAL_TOKEN,
            "x-axpoint-tenant": "t2",
        },
        json={
            "tenant_id": "t1",
            "lake_table": "forbidden_cross_hdr",
            "sql": 'SELECT * FROM lake."tenant_t1"."orders"',
            "batch_id": "b-mismatch",
        },
    )
    assert resp.status_code == 403
    assert resp.json()["error_code"] == "PERMISSION_DENIED"


@pytest.mark.parametrize(
    ("case_name", "bad_sql"),
    [
        ("cross_tenant", 'SELECT * FROM lake."tenant_t2"."secret_orders"'),
        (
            "cross_tenant_subquery",
            'SELECT * FROM (SELECT * FROM lake."tenant_t2"."secret_orders") s',
        ),
        ("bare_table", "SELECT * FROM orders"),
        ("platform_view_ax", "SELECT * FROM ax.nodes"),
        ("external_ax_pg", "SELECT * FROM ax_pg.public.sec_users"),
        ("live_conn", "SELECT * FROM live_erp.public.orders"),
        ("info_schema", "SELECT * FROM information_schema.tables"),
        ("system_catalog", "SELECT * FROM system.main.duckdb_tables"),
        ("main_schema", "SELECT * FROM main.orders"),
        ("path_replacement_scan", "SELECT * FROM '/etc/passwd'"),
        ("cte_lexical_leak", "SELECT * FROM c WHERE EXISTS (WITH c AS (SELECT 1) SELECT * FROM c)"),
        ("cte_forward_ref", "WITH c1 AS (SELECT * FROM c2), c2 AS (SELECT 1) SELECT * FROM c1"),
        ("cte_nonrecursive_self", "WITH orders AS (SELECT * FROM orders) SELECT * FROM orders"),
        ("read_csv", "SELECT * FROM read_csv('/etc/passwd')"),
        ("read_csv_auto", "SELECT * FROM read_csv_auto('/etc/passwd')"),
        ("read_parquet", "SELECT * FROM read_parquet('/tmp/evil.parquet')"),
        ("read_json", "SELECT * FROM read_json_auto('/tmp/evil.json')"),
        ("parquet_scan", "SELECT * FROM parquet_scan('/tmp/evil.parquet')"),
        ("glob_fn", "SELECT * FROM glob('/etc/*')"),
        ("query_fn", "SELECT * FROM query('SELECT 1')"),
        ("query_table_fn", "SELECT * FROM query_table('orders')"),
        ("ducklake_fn", "SELECT * FROM ducklake_snapshots('lake')"),
        ("postgres_query_fn", "SELECT * FROM postgres_query('ax_pg', 'SELECT 1')"),
        ("iceberg_fn", "SELECT * FROM iceberg_scan('/tmp/ice')"),
        ("delta_fn", "SELECT * FROM delta_scan('/tmp/delta')"),
        ("getenv_scalar", "SELECT getenv('HOME') AS h"),
        ("current_setting_scalar", "SELECT current_setting('memory_limit') AS m"),
        ("read_text_scalar", "SELECT read_text('/etc/passwd') AS txt"),
        ("read_blob_scalar", "SELECT read_blob('/etc/passwd') AS b"),
        ("load_extension_scalar", "SELECT load_extension('httpfs') AS ext"),
        ("sql_auto_complete_fn", "SELECT * FROM sql_auto_complete('SEL')"),
        ("pg_prefix_fn", "SELECT pg_sleep(1) AS s"),
        ("duckdb_prefix_fn", "SELECT * FROM duckdb_settings()"),
        ("pragma_prefix_fn", "SELECT * FROM pragma_database_list()"),
        ("internal_prefix_fn", "SELECT __internal_decompress('x') AS d"),
        ("qualified_table_fn", "SELECT * FROM main.range(5)"),
        ("select_into", 'SELECT * INTO stolen FROM lake."tenant_t1"."orders"'),
        ("copy_to", "COPY (SELECT 1) TO '/tmp/out.csv'"),
        ("attach_db", "ATTACH ':memory:' AS extra_db"),
        ("pragma_stmt", "PRAGMA version"),
        ("set_stmt", "SET enable_external_access = true"),
        ("use_stmt", "USE memory"),
        ("multi_statement", 'SELECT 1; DROP TABLE lake."tenant_t1"."orders"'),
        ("empty_sql", "   "),
        ("syntax_error", "SELECT FROM WHERE"),
        ("only_ax_columns", "SELECT 'spoof' AS _ax_batch_id, '2026-01-01' AS _ax_synced_at"),
        ("missing_column_binder", 'SELECT nonexistent_col FROM lake."tenant_t1"."orders"'),
        ("missing_lake_table", 'SELECT * FROM lake."tenant_t1"."missing_tbl"'),
    ],
)
def test_materialize_rejects_unsafe_or_invalid_sql(
    materialize_env: tuple[TestClient, Path, str, dict[str, str]],
    case_name: str,
    bad_sql: str,
) -> None:
    client, _data_root, _dsn, headers = materialize_env
    resp = client.post(
        "/lake/materialize",
        headers=headers,
        json={
            "tenant_id": "t1",
            "dataset_id": f"ds-bad-{case_name}",
            "lake_table": "rejected_target",
            "sql": bad_sql,
            "batch_id": f"batch-bad-{case_name}",
        },
    )
    assert resp.status_code == 400, f"{case_name} returned {resp.status_code}: {resp.text}"
    err = resp.json()
    assert err["error_code"] == "SQL_REJECTED", f"{case_name} returned {err}"
