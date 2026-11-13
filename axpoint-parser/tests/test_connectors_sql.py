from __future__ import annotations

import sqlite3
from pathlib import Path

import pytest
from fastapi.testclient import TestClient

from main import app

TEST_TOKEN = "step02-sql-conn-token-0123456789abcdef"


def _seed_sqlite_db(db_file: Path) -> None:
    with sqlite3.connect(db_file) as conn:
        conn.execute(
            """
            CREATE TABLE orders (
                order_id INTEGER PRIMARY KEY,
                customer_phone TEXT NOT NULL,
                amount REAL NOT NULL,
                updated_at TEXT NOT NULL,
                is_deleted INTEGER NOT NULL DEFAULT 0
            )
            """
        )
        conn.executemany(
            "INSERT INTO orders (order_id, customer_phone, amount, updated_at, is_deleted) "
            "VALUES (?, ?, ?, ?, ?)",
            [
                (1, "13812345678", 100.0, "2026-01-01T00:00:00Z", 0),
                (2, "13987654321", 250.5, "2026-01-02T00:00:00Z", 0),
                (3, "13700001111", 80.0, "2026-01-03T00:00:00Z", 0),
            ],
        )
        conn.commit()


def test_sqlite_discover_preview_and_sync_lifecycle(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    monkeypatch.setenv("AXPOINT_INTERNAL_TOKEN", TEST_TOKEN)
    monkeypatch.setenv("AXPOINT_DATA_ROOT", str(tmp_path))
    monkeypatch.setenv("AXPOINT_LAKE_CATALOG_DSN", f"ducklake:{tmp_path / 'cat.ducklake'}")

    db_path = tmp_path / "source_erp.sqlite"
    _seed_sqlite_db(db_path)

    client = TestClient(app)
    hdrs = {"X-AxPoint-Internal-Token": TEST_TOKEN, "X-AxPoint-Tenant": "acme"}

    # 1. Test connection
    t_resp = client.post(
        "/connectors/test",
        headers=hdrs,
        json={"connector_type": "SQLITE", "config": {"path": "source_erp.sqlite"}},
    )
    assert t_resp.status_code == 200
    assert t_resp.json()["ok"] is True

    # 2. Discover tables, PKs, and suggested cursor
    d_resp = client.post(
        "/connectors/discover",
        headers=hdrs,
        json={"connector_type": "SQLITE", "config": {"path": "source_erp.sqlite"}},
    )
    assert d_resp.status_code == 200
    objs = d_resp.json()
    assert len(objs) == 1
    assert objs[0]["display_name"] == "orders"
    assert objs[0]["primary_key"] == ["order_id"]
    assert objs[0]["suggested_cursor"] == "updated_at"
    assert objs[0]["estimated_rows"] == 3

    # 3. Preview masks PII phone numbers
    p_resp = client.post(
        "/connectors/preview",
        headers=hdrs,
        json={
            "connector_type": "SQLITE",
            "config": {"path": "source_erp.sqlite"},
            "ref": {"table": "orders"},
            "limit": 10,
        },
    )
    assert p_resp.status_code == 200
    p_body = p_resp.json()
    assert p_body["rows"][0]["customer_phone"] == "138****5678"

    # 4. Initial SNAPSHOT sync -> 3 rows
    s1 = client.post(
        "/connectors/sync",
        headers=hdrs,
        json={
            "tenant_id": "acme",
            "dataset_id": "ds-orders",
            "batch_id": "b-1",
            "connector_type": "SQLITE",
            "config": {"path": "source_erp.sqlite"},
            "source_ref": {"table": "orders"},
            "lake_table": "erp_orders",
            "sync_mode": "UPSERT",
            "primary_key": ["order_id"],
            "cursor_column": "updated_at",
            "delete_column": "is_deleted",
        },
    )
    assert s1.status_code == 200
    s1_body = s1.json()
    assert s1_body["rows_written"] == 3
    assert s1_body["cursor_to"] == "2026-01-03T00:00:00Z"
    assert "schema" in s1_body

    # Mutate upstream SQLite: update order 2, soft-delete order 3, insert order 4
    with sqlite3.connect(db_path) as conn:
        conn.execute(
            "UPDATE orders SET amount = 999.0, "
            "updated_at = '2026-01-04T00:00:00Z' WHERE order_id = 2"
        )
        conn.execute(
            "UPDATE orders SET is_deleted = 1, "
            "updated_at = '2026-01-04T00:00:00Z' WHERE order_id = 3"
        )
        conn.execute(
            "INSERT INTO orders VALUES (4, '13611112222', 500.0, '2026-01-05T00:00:00Z', 0)"
        )
        conn.commit()

    # 5. Incremental UPSERT from previous cursor
    s2 = client.post(
        "/connectors/sync",
        headers=hdrs,
        json={
            "tenant_id": "acme",
            "dataset_id": "ds-orders",
            "batch_id": "b-2",
            "connector_type": "SQLITE",
            "config": {"path": "source_erp.sqlite"},
            "source_ref": {"table": "orders"},
            "lake_table": "erp_orders",
            "sync_mode": "UPSERT",
            "primary_key": ["order_id"],
            "cursor_column": "updated_at",
            "cursor_from": s1_body["cursor_to"],
            "delete_column": "is_deleted",
        },
    )
    assert s2.status_code == 200
    s2_body = s2.json()
    assert s2_body["rows_written"] == 2
    assert s2_body["rows_deleted"] == 1

    # Verify lake preview has order_id {1, 2, 4} with updated amount 999.0 and order 3 deleted
    lp = client.post(
        "/lake/preview",
        headers=hdrs,
        json={"tenant_id": "acme", "lake_table": "erp_orders", "limit": 20},
    )
    assert lp.status_code == 200
    lake_rows = {r["order_id"]: r["amount"] for r in lp.json()["rows"]}
    assert lake_rows == {1: 100.0, 2: 999.0, 4: 500.0}


@pytest.mark.parametrize(
    "bad_sql",
    [
        "DROP TABLE orders",
        "DELETE FROM orders WHERE order_id = 1",
        "INSERT INTO orders VALUES (9, '13800138000', 1.0, '2026-01-01', 0)",
        "SELECT * FROM orders; DROP TABLE orders",
        "SELECT * INTO evil_copy FROM orders",
    ],
)
def test_custom_sql_rejects_mutating_and_multi_statements(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path, bad_sql: str
) -> None:
    monkeypatch.setenv("AXPOINT_INTERNAL_TOKEN", TEST_TOKEN)
    monkeypatch.setenv("AXPOINT_DATA_ROOT", str(tmp_path))
    db_path = tmp_path / "ro.sqlite"
    _seed_sqlite_db(db_path)

    client = TestClient(app)
    resp = client.post(
        "/connectors/preview",
        headers={"X-AxPoint-Internal-Token": TEST_TOKEN},
        json={
            "connector_type": "SQLITE",
            "config": {"path": "ro.sqlite"},
            "ref": {"query": bad_sql},
            "limit": 10,
        },
    )
    assert resp.status_code == 400
    assert resp.json()["error_code"] == "SQL_REJECTED"
