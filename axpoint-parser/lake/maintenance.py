from __future__ import annotations

from pathlib import Path

from lake.catalog import lake_connection
from schemas import LakeMaintenanceRequest, LakeMaintenanceResult
from security import validate_tenant_id


def run_maintenance(
    req: LakeMaintenanceRequest,
    *,
    data_root: Path | None = None,
    catalog_dsn: str | None = None,
) -> LakeMaintenanceResult:
    """Run DuckLake adjacent file compaction, snapshot expiration, and orphaned file cleanup."""
    validate_tenant_id(req.tenant_id)
    keep_days = max(int(req.keep_days), 0)

    with lake_connection(data_root=data_root, catalog_dsn=catalog_dsn) as con:
        merged_rows = con.execute("CALL ducklake_merge_adjacent_files('lake')").fetchall()
        expire_sql = (
            "CALL ducklake_expire_snapshots("
            f"'lake', older_than => now() - INTERVAL '{keep_days} days')"
        )
        expired_rows = con.execute(expire_sql).fetchall()
        cleaned_rows = con.execute(
            "CALL ducklake_cleanup_old_files('lake', cleanup_all => true)"
        ).fetchall()

    return LakeMaintenanceResult(
        merged_files=len(merged_rows),
        expired_snapshots=len(expired_rows),
        deleted_files=len(cleaned_rows),
    )
