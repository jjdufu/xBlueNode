from __future__ import annotations

from pathlib import Path

import pytest

from lake.maintenance import run_maintenance
from lake.writer import (
    commit_staging,
    drop_lake_table,
    list_snapshots,
    preview_lake,
    sync_dataset,
)
from parsers.engine import parse_unified
from schemas import (
    CommitItem,
    CommitStagingRequest,
    LakeMaintenanceRequest,
    LakePreviewRequest,
    SyncRequest,
)
from security import SidecarError

FIXTURES_DIR = Path(__file__).resolve().parent / "fixtures"


def test_webhook_append_idempotency_time_travel_and_maintenance(tmp_path: Path) -> None:
    dsn = f"ducklake:{tmp_path / 'lake_cat.ducklake'}"

    # Batch 1: 2 webhook events
    req1 = SyncRequest(
        tenant_id="t_demo",
        dataset_id="ds-wh",
        batch_id="batch-001",
        connector_type="WEBHOOK",
        config={"record_path": "$[*]"},
        source_ref={
            "items": [
                {"delivery_id": "del-1", "payload": [{"device": "D-1", "temp": 21.5}]},
                {"delivery_id": "del-2", "payload": [{"device": "D-2", "temp": 22.0}]},
            ]
        },
        lake_table="iot_telemetry",
        sync_mode="APPEND",
    )
    r1 = sync_dataset(req1, data_root=tmp_path, catalog_dsn=dsn)
    assert r1.rows_written == 2
    snap1 = r1.snapshot_id
    assert snap1 >= 1

    # Re-sending identical delivery_id in a NEW batch_id writes 0 duplicate rows (_ax_msg_id dedupe)
    req_dup = req1.model_copy(update={"batch_id": "batch-002"})
    r_dup = sync_dataset(req_dup, data_root=tmp_path, catalog_dsn=dsn)
    assert r_dup.rows_written == 0

    # Batch 3: 1 new webhook event
    req3 = SyncRequest(
        tenant_id="t_demo",
        dataset_id="ds-wh",
        batch_id="batch-003",
        connector_type="WEBHOOK",
        config={"record_path": "$[*]"},
        source_ref={
            "items": [
                {"delivery_id": "del-3", "payload": [{"device": "D-3", "temp": 25.0}]},
            ]
        },
        lake_table="iot_telemetry",
        sync_mode="APPEND",
    )
    r3 = sync_dataset(req3, data_root=tmp_path, catalog_dsn=dsn)
    assert r3.rows_written == 1
    assert r3.snapshot_id > snap1

    # Latest preview sees 3 rows; AT (VERSION => snap1) time-travel preview sees 2 rows
    latest = preview_lake(
        LakePreviewRequest(tenant_id="t_demo", lake_table="iot_telemetry", limit=10),
        data_root=tmp_path,
        catalog_dsn=dsn,
    )
    assert len(latest.rows) == 3

    historical = preview_lake(
        LakePreviewRequest(
            tenant_id="t_demo", lake_table="iot_telemetry", snapshot_id=snap1, limit=10
        ),
        data_root=tmp_path,
        catalog_dsn=dsn,
    )
    assert len(historical.rows) == 2

    snaps = list_snapshots("t_demo", "iot_telemetry", data_root=tmp_path, catalog_dsn=dsn)
    assert len(snaps) >= 2

    maint = run_maintenance(
        LakeMaintenanceRequest(tenant_id="t_demo", keep_days=7),
        data_root=tmp_path,
        catalog_dsn=dsn,
    )
    assert maint.merged_files >= 0

    assert drop_lake_table("t_demo", "iot_telemetry", data_root=tmp_path, catalog_dsn=dsn) is True
    assert drop_lake_table("t_demo", "iot_telemetry", data_root=tmp_path, catalog_dsn=dsn) is False


def test_commit_staging_commits_parquet_and_blocks_cross_tenant_escape(tmp_path: Path) -> None:
    dsn = f"ducklake:{tmp_path / 'commit_cat.ducklake'}"
    parsed = parse_unified(
        "supply_chain.xlsx",
        (FIXTURES_DIR / "supply_chain.xlsx").read_bytes(),
        tenant_id="tenant_a",
        data_root=tmp_path,
    )
    assert len(parsed.tables) == 4

    commit_req = CommitStagingRequest(
        tenant_id="tenant_a",
        parse_id=parsed.parse_id,
        items=[
            CommitItem(
                dataset_id=f"ds-{idx}",
                batch_id=f"b-{idx}",
                staging_path=t.staging_path,
                lake_table=t.table_name,
            )
            for idx, t in enumerate(parsed.tables)
        ],
    )
    committed = commit_staging(commit_req, data_root=tmp_path, catalog_dsn=dsn)
    assert len(committed) == 4
    assert sum(c.rows_written for c in committed) == 3 + 40 + 10 + 10

    # Cross-tenant staging_path escape attempt must fail with 403 PERMISSION_DENIED
    evil_req = CommitStagingRequest(
        tenant_id="tenant_b",
        parse_id=parsed.parse_id,
        items=[
            CommitItem(
                dataset_id="ds-steal",
                batch_id="b-steal",
                staging_path=parsed.tables[0].staging_path,
                lake_table="stolen_tbl",
            )
        ],
    )
    with pytest.raises(SidecarError) as exc_info:
        commit_staging(evil_req, data_root=tmp_path, catalog_dsn=dsn)
    assert exc_info.value.status_code == 403
    assert exc_info.value.error_code == "PERMISSION_DENIED"
