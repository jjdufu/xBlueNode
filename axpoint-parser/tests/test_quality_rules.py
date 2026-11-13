from __future__ import annotations

from datetime import UTC, datetime, timedelta
from pathlib import Path

import polars as pl
import pytest

from lake.quality import evaluate_quality
from lake.writer import preview_lake, sync_dataset
from schemas import LakePreviewRequest, QualityRule, SyncRequest
from security import SidecarError


def test_all_7_quality_rule_kinds() -> None:
    now = datetime(2026, 5, 1, 12, 0, 0, tzinfo=UTC)
    stale = (now - timedelta(minutes=180)).isoformat()
    df = pl.DataFrame(
        {
            "id": [1, 1, 3],
            "name": ["Alice", None, "Charlie"],
            "score": [50.0, 95.0, 150.0],
            "code": ["ORD-01", "BAD", "ORD-03"],
            "status": ["OPEN", "CLOSED", "UNKNOWN"],
            "ts": [stale, stale, stale],
        }
    )

    rules = [
        QualityRule(id="r1", column_name="name", rule_kind="NOT_NULL", on_fail="WARN"),
        QualityRule(id="r2", column_name="id", rule_kind="UNIQUE", on_fail="WARN"),
        QualityRule(
            id="r3",
            column_name="score",
            rule_kind="RANGE",
            params={"min": 0, "max": 100},
            on_fail="WARN",
        ),
        QualityRule(
            id="r4",
            column_name="code",
            rule_kind="REGEX",
            params={"pattern": r"^ORD-\d+$"},
            on_fail="WARN",
        ),
        QualityRule(
            id="r5",
            column_name="status",
            rule_kind="ENUM",
            params={"values": ["OPEN", "CLOSED"]},
            on_fail="WARN",
        ),
        QualityRule(
            id="r6",
            column_name="ts",
            rule_kind="FRESHNESS",
            params={"max_lag_minutes": 60},
            on_fail="WARN",
        ),
        QualityRule(
            id="r7",
            rule_kind="ROW_COUNT_MIN",
            params={"min_rows": 5},
            on_fail="BLOCK",
        ),
    ]

    rep = evaluate_quality(df, rules, now_utc=now)
    assert rep.passed is False
    assert rep.blocked is True
    assert all(not r.passed for r in rep.rules)


def test_block_rule_prevents_ducklake_commit_while_warn_commits(tmp_path: Path) -> None:
    dsn = f"ducklake:{tmp_path / 'qual_cat.ducklake'}"

    # Seed valid initial snapshot
    r_ok = sync_dataset(
        SyncRequest(
            tenant_id="tq",
            dataset_id="ds-q",
            batch_id="b-ok",
            connector_type="WEBHOOK",
            config={"record_path": "$[*]"},
            source_ref={"items": [{"delivery_id": "1", "payload": [{"sku": "A", "qty": 10}]}]},
            lake_table="inventory_q",
            sync_mode="SNAPSHOT",
        ),
        data_root=tmp_path,
        catalog_dsn=dsn,
        raise_on_quality_block=True,
    )
    assert r_ok.rows_written == 1

    # WARN failure still commits
    r_warn = sync_dataset(
        SyncRequest(
            tenant_id="tq",
            dataset_id="ds-q",
            batch_id="b-warn",
            connector_type="WEBHOOK",
            config={"record_path": "$[*]"},
            source_ref={"items": [{"delivery_id": "2", "payload": [{"sku": "B", "qty": -5}]}]},
            lake_table="inventory_q",
            sync_mode="APPEND",
            quality_rules=[
                QualityRule(
                    id="q-pos",
                    column_name="qty",
                    rule_kind="RANGE",
                    params={"min": 0},
                    on_fail="WARN",
                )
            ],
        ),
        data_root=tmp_path,
        catalog_dsn=dsn,
        raise_on_quality_block=True,
    )
    assert r_warn.rows_written == 1
    assert r_warn.quality.passed is False
    assert r_warn.quality.blocked is False

    # BLOCK failure raises 422 QUALITY_BLOCKED and leaves existing 2 lake rows untouched
    with pytest.raises(SidecarError) as exc_info:
        sync_dataset(
            SyncRequest(
                tenant_id="tq",
                dataset_id="ds-q",
                batch_id="b-block",
                connector_type="WEBHOOK",
                config={"record_path": "$[*]"},
                source_ref={"items": [{"delivery_id": "3", "payload": [{"sku": None, "qty": 5}]}]},
                lake_table="inventory_q",
                sync_mode="APPEND",
                quality_rules=[
                    QualityRule(id="q-nn", column_name="sku", rule_kind="NOT_NULL", on_fail="BLOCK")
                ],
            ),
            data_root=tmp_path,
            catalog_dsn=dsn,
            raise_on_quality_block=True,
        )
    assert exc_info.value.status_code == 422
    assert exc_info.value.error_code == "QUALITY_BLOCKED"

    prev = preview_lake(
        LakePreviewRequest(tenant_id="tq", lake_table="inventory_q", limit=10),
        data_root=tmp_path,
        catalog_dsn=dsn,
    )
    assert len(prev.rows) == 2
