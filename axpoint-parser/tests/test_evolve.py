from __future__ import annotations

from pathlib import Path

import pytest

from lake.evolve import SchemaDriftBlockedError, evaluate_schema_evolution
from lake.writer import preview_lake, sync_dataset
from schemas import ColumnProfile, LakePreviewRequest, SchemaPolicy, SyncRequest


def _col(name: str, dt: str, ordinal: int = 0) -> ColumnProfile:
    return ColumnProfile(name=name, ordinal=ordinal, data_type=dt, nullable=True)  # type: ignore[arg-type]


@pytest.mark.parametrize(
    ("policy", "incoming", "should_block"),
    [
        # Added column: blocked in STRICT, allowed in ADDITIVE and EVOLVE
        ("STRICT", [_col("id", "INT64", 0), _col("extra", "STRING", 1)], True),
        ("ADDITIVE", [_col("id", "INT64", 0), _col("extra", "STRING", 1)], False),
        ("EVOLVE", [_col("id", "INT64", 0), _col("extra", "STRING", 1)], False),
        # Removed column: blocked in STRICT and ADDITIVE, allowed in EVOLVE
        ("STRICT", [], True),
        ("ADDITIVE", [], True),
        ("EVOLVE", [], False),
        # Lossless widening (INT64 -> FLOAT64): blocked in STRICT, allowed in ADDITIVE & EVOLVE
        ("STRICT", [_col("id", "FLOAT64", 0)], True),
        ("ADDITIVE", [_col("id", "FLOAT64", 0)], False),
        ("EVOLVE", [_col("id", "FLOAT64", 0)], False),
    ],
)
def test_schema_policy_matrix_on_add_remove_widen(
    policy: SchemaPolicy, incoming: list[ColumnProfile], should_block: bool
) -> None:
    expected = [_col("id", "INT64", 0)]
    if should_block:
        with pytest.raises(SchemaDriftBlockedError) as exc_info:
            evaluate_schema_evolution(expected, incoming, policy)
        assert exc_info.value.status_code == 422
        assert exc_info.value.error_code == "SCHEMA_DRIFT_BLOCKED"
    else:
        change, resolved = evaluate_schema_evolution(expected, incoming, policy)
        assert change.breaking is False
        assert len(resolved) >= 1


def test_narrowing_blocks_strict_and_additive_but_upgrades_to_string_in_evolve(
    tmp_path: Path,
) -> None:
    expected = [_col("val", "STRING", 0)]
    narrowed_inc = [_col("val", "INT64", 0)]

    strict_policies: tuple[SchemaPolicy, ...] = ("STRICT", "ADDITIVE")
    for pol in strict_policies:
        with pytest.raises(SchemaDriftBlockedError):
            evaluate_schema_evolution(expected, narrowed_inc, pol)

    change, resolved = evaluate_schema_evolution(expected, narrowed_inc, "EVOLVE")
    assert change.narrowed == [("val", "STRING", "INT64")]
    assert resolved[0].data_type == "STRING"

    # End-to-end DuckLake DDL evolution check (ADDITIVE add + EVOLVE NULL-padding)
    dsn = f"ducklake:{tmp_path / 'evolve_cat.ducklake'}"
    r1 = sync_dataset(
        SyncRequest(
            tenant_id="t1",
            dataset_id="ds-ev",
            batch_id="b1",
            connector_type="WEBHOOK",
            config={"record_path": "$[*]"},
            source_ref={"items": [{"delivery_id": "1", "payload": [{"a": 10, "b": "hello"}]}]},
            lake_table="ev_tbl",
            sync_mode="APPEND",
            schema_policy="EVOLVE",
        ),
        data_root=tmp_path,
        catalog_dsn=dsn,
    )
    assert r1.rows_written == 1

    # Batch 2 drops 'b' and adds 'c' under EVOLVE -> succeeds, old 'b' NULL-padded, new 'c' added
    r2 = sync_dataset(
        SyncRequest(
            tenant_id="t1",
            dataset_id="ds-ev",
            batch_id="b2",
            connector_type="WEBHOOK",
            config={"record_path": "$[*]"},
            source_ref={"items": [{"delivery_id": "2", "payload": [{"a": 20, "c": 3.5}]}]},
            lake_table="ev_tbl",
            sync_mode="APPEND",
            schema_policy="EVOLVE",
            expected_schema=r1.columns_schema,
        ),
        data_root=tmp_path,
        catalog_dsn=dsn,
    )
    assert r2.rows_written == 1
    assert r2.schema_change.removed == ["b"]
    assert [c.name for c in r2.schema_change.added] == ["c"]

    prev = preview_lake(
        LakePreviewRequest(tenant_id="t1", lake_table="ev_tbl", limit=10),
        data_root=tmp_path,
        catalog_dsn=dsn,
    )
    assert len(prev.rows) == 2
    assert {c.name for c in prev.columns} == {"a", "b", "c"}
