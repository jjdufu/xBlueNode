#!/usr/bin/env python3
"""Vectorized deterministic world data generator for Step 34 (`semi_supply` & `med_dispatch`).

Fixed seed `202605`, zero external network calls, idempotent DuckLake Parquet + PostgreSQL
`UNNEST` batch ingestion completing each world in < 3s.
"""
from __future__ import annotations

import argparse
import json
import math
import os
import random
import time
import uuid
from collections.abc import Sequence
from dataclasses import dataclass
from datetime import UTC, datetime, timedelta
from typing import Any

import duckdb
import psycopg
import pyarrow as pa

from lake.catalog import lake_connection, quote_lake_table, tenant_schema, tenant_schema_name
from security import validate_lake_table, validate_tenant_id

BASE_SEED: int = 202605
WORLD_NS = uuid.UUID("7f34b8a0-2026-4d34-9a34-5e31c0ffee34")


@dataclass(frozen=True)
class ColumnMetaSpec:
    name: str
    data_type: str
    description: str
    business_term: str
    unit: str = ""
    pii_category: str = "NONE"


@dataclass(frozen=True)
class NodeRow:
    node_id: uuid.UUID
    subgraph_slug: str
    type_name: str
    node_key: str
    title: str
    properties_json: str
    metrics_json: str
    geo_lng: float | None
    geo_lat: float | None


@dataclass(frozen=True)
class LinkRow:
    link_id: uuid.UUID
    subgraph_slug: str
    link_type: str
    src_node_id: uuid.UUID
    dst_node_id: uuid.UUID
    weight: float


@dataclass(frozen=True)
class MetricPointRow:
    node_id: uuid.UUID
    metric: str
    ts: datetime
    value: float
    source_tag: str


@dataclass(frozen=True)
class AlertSeedSpec:
    rule_name: str
    description: str
    type_name: str
    metric: str
    op: str
    threshold: float
    severity: str
    title_template: str
    body_template: str
    alert_node_id: uuid.UUID
    alert_title: str
    alert_body: str
    observed_value: float


def _det_uuid(tenant_id: str, scope: str, key: str) -> uuid.UUID:
    return uuid.uuid5(WORLD_NS, f"{tenant_id}:{scope}:{key}")


def _lake_table_exists(con: duckdb.DuckDBPyConnection, schema_name: str, table_name: str) -> bool:
    row = con.execute(
        "SELECT 1 FROM information_schema.tables "
        "WHERE table_catalog = 'lake' AND table_schema = ? AND table_name = ? LIMIT 1",
        [schema_name, table_name],
    ).fetchone()
    return row is not None


def _write_arrow_to_ducklake(
    *,
    tenant_id: str,
    table_name: str,
    arrow_tbl: pa.Table,
    batch_id: uuid.UUID,
) -> int:
    valid_tenant = validate_tenant_id(tenant_id)
    valid_table = validate_lake_table(table_name)
    s_raw = tenant_schema_name(valid_tenant)
    s_q = tenant_schema(valid_tenant)
    t_q = quote_lake_table(valid_table)
    full_tbl = f"lake.{s_q}.{t_q}"

    n_rows = int(arrow_tbl.num_rows)
    synced_ts = datetime.now(UTC)
    augmented = arrow_tbl.append_column(
        "_ax_batch_id",
        pa.array([str(batch_id)] * n_rows, type=pa.string()),
    ).append_column(
        "_ax_synced_at",
        pa.array([synced_ts] * n_rows, type=pa.timestamp("us", tz="UTC")),
    )

    with lake_connection() as con:
        con.execute(f"CREATE SCHEMA IF NOT EXISTS lake.{s_q}")
        exists = _lake_table_exists(con, s_raw, valid_table)
        con.execute("BEGIN")
        try:
            if exists:
                con.execute(f"DROP TABLE {full_tbl}")
            con.register("_ax_world_src", augmented)
            try:
                con.execute(f"CREATE TABLE {full_tbl} AS SELECT * FROM _ax_world_src")
            finally:
                con.unregister("_ax_world_src")
            con.execute("COMMIT")
        except Exception:
            con.execute("ROLLBACK")
            raise
        snap_row = con.execute("SELECT * FROM ducklake_current_snapshot('lake')").fetchone()
        return int(snap_row[0]) if snap_row and snap_row[0] is not None else 1


def _upsert_lake_dataset_metadata(
    cur: psycopg.Cursor[Any],
    *,
    tenant_id: str,
    dataset_name: str,
    display_name: str,
    description: str,
    columns: Sequence[ColumnMetaSpec],
    row_count: int,
    snapshot_id: int,
    tags: list[str],
) -> uuid.UUID:
    valid_tenant = validate_tenant_id(tenant_id)
    valid_name = validate_lake_table(dataset_name)
    lake_schema_str = tenant_schema_name(valid_tenant)
    ds_det_id = _det_uuid(valid_tenant, "dataset", valid_name)

    schema_profiles: list[dict[str, Any]] = [
        {
            "name": col.name,
            "ordinal": idx,
            "data_type": col.data_type,
            "nullable": False,
            "null_rate": 0.0,
            "distinct_count": min(row_count, 1000),
            "sample_values": [],
            "pii_category": col.pii_category,
            "pii_confidence": 0.0,
        }
        for idx, col in enumerate(columns)
    ]

    cur.execute(
        """
        INSERT INTO ds_datasets (
            id, tenant_id, connection_id, name, display_name, description,
            source_kind, source_ref, sync_mode, primary_key, schema_policy,
            lake_schema, lake_table, schema_json, schema_version,
            row_count, last_snapshot_id, last_synced_at, tags, status, updated_at
        )
        VALUES (
            %s, %s, NULL, %s, %s, %s,
            'FILE_TABLE', '{}'::jsonb, 'SNAPSHOT', '{}'::text[], 'ADDITIVE',
            %s, %s, %s::jsonb, 1,
            %s, %s, NOW(), %s::text[], 'READY', NOW()
        )
        ON CONFLICT (tenant_id, name) DO UPDATE SET
            display_name = EXCLUDED.display_name,
            description = EXCLUDED.description,
            lake_schema = EXCLUDED.lake_schema,
            lake_table = EXCLUDED.lake_table,
            schema_json = EXCLUDED.schema_json,
            schema_version = GREATEST(ds_datasets.schema_version, 1),
            row_count = EXCLUDED.row_count,
            last_snapshot_id = EXCLUDED.last_snapshot_id,
            last_synced_at = NOW(),
            tags = EXCLUDED.tags,
            status = 'READY',
            updated_at = NOW()
        RETURNING id
        """,
        (
            ds_det_id,
            valid_tenant,
            valid_name,
            display_name,
            description,
            lake_schema_str,
            valid_name,
            json.dumps(schema_profiles, ensure_ascii=False),
            row_count,
            snapshot_id,
            tags,
        ),
    )
    fetched = cur.fetchone()
    if fetched is None:
        raise RuntimeError(f"failed to upsert ds_datasets row for {valid_name}")
    ds_id = uuid.UUID(str(fetched[0]))

    cur.execute(
        "DELETE FROM ds_column_meta WHERE tenant_id = %s AND dataset_id = %s",
        (valid_tenant, ds_id),
    )
    cur.execute(
        """
        INSERT INTO ds_column_meta (
            tenant_id, dataset_id, column_name, data_type, pii_category,
            pii_confirmed, description, business_term, unit, ordinal
        )
        SELECT
            %s, %s, u.col_name, u.dt, u.pii,
            TRUE, u.descr, u.bterm, u.unit_lbl, u.ord
        FROM UNNEST(
            %s::text[], %s::text[], %s::text[], %s::text[], %s::text[], %s::text[], %s::int4[]
        ) AS u(col_name, dt, pii, descr, bterm, unit_lbl, ord)
        """,
        (
            valid_tenant,
            ds_id,
            [c.name for c in columns],
            [c.data_type for c in columns],
            [c.pii_category for c in columns],
            [c.description for c in columns],
            [c.business_term for c in columns],
            [c.unit for c in columns],
            list(range(len(columns))),
        ),
    )

    sync_job_id = _det_uuid(valid_tenant, "sync_job", valid_name)
    ax_job_id = _det_uuid(valid_tenant, "ax_job_sync", valid_name)
    batch_id = _det_uuid(valid_tenant, "lake_batch", valid_name)
    cur.execute(
        """
        INSERT INTO ds_sync_jobs (
            id, tenant_id, dataset_id, job_id, batch_id, sync_mode,
            status, rows_read, rows_written, rows_deleted, snapshot_id,
            started_at, finished_at, created_at
        )
        VALUES (
            %s, %s, %s, %s, %s, 'SNAPSHOT',
            'SUCCEEDED', %s, %s, 0, %s,
            NOW(), NOW(), NOW()
        )
        ON CONFLICT (id) DO UPDATE SET
            rows_read = EXCLUDED.rows_read,
            rows_written = EXCLUDED.rows_written,
            snapshot_id = EXCLUDED.snapshot_id,
            status = 'SUCCEEDED',
            started_at = NOW(),
            finished_at = NOW()
        """,
        (
            sync_job_id,
            valid_tenant,
            ds_id,
            ax_job_id,
            batch_id,
            row_count,
            row_count,
            snapshot_id,
        ),
    )

    cur.execute(
        """
        INSERT INTO ax_outbox (tenant_id, topic, payload)
        VALUES (%s, 'dataset.synced', %s::jsonb)
        """,
        (valid_tenant, json.dumps({"dataset_id": str(ds_id)})),
    )
    cur.execute("NOTIFY ax_outbox, 'dataset.synced'")
    return ds_id


def _batch_replace_subgraph_topology(
    cur: psycopg.Cursor[Any],
    *,
    tenant_id: str,
    subgraph_slug: str,
    nodes: Sequence[NodeRow],
    links: Sequence[LinkRow],
) -> None:
    keep_node_ids = [n.node_id for n in nodes]
    cur.execute(
        "DELETE FROM xb_links WHERE tenant_id = %s AND subgraph_slug = %s",
        (tenant_id, subgraph_slug),
    )
    cur.execute(
        """
        DELETE FROM xb_nodes
        WHERE tenant_id = %s AND subgraph_slug = %s AND NOT (id = ANY(%s::uuid[]))
        """,
        (tenant_id, subgraph_slug, keep_node_ids),
    )
    _batch_upsert_nodes_and_links(cur, tenant_id=tenant_id, nodes=nodes, links=links)


def _batch_upsert_nodes_and_links(
    cur: psycopg.Cursor[Any],
    *,
    tenant_id: str,
    nodes: Sequence[NodeRow],
    links: Sequence[LinkRow],
) -> None:
    if nodes:
        cur.execute(
            """
            INSERT INTO xb_nodes (
                id, tenant_id, subgraph_slug, type_name, node_key, title,
                properties, metrics, geo_lng, geo_lat, updated_at
            )
            SELECT
                u.nid, %s, u.sg, u.tname, u.nkey, u.ttl,
                u.props::jsonb, u.mets::jsonb, u.lng, u.lat, NOW()
            FROM UNNEST(
                %s::uuid[], %s::text[], %s::text[], %s::text[], %s::text[],
                %s::text[], %s::text[], %s::float8[], %s::float8[]
            ) AS u(nid, sg, tname, nkey, ttl, props, mets, lng, lat)
            ON CONFLICT (tenant_id, subgraph_slug, type_name, node_key) DO UPDATE SET
                title = EXCLUDED.title,
                properties = EXCLUDED.properties,
                metrics = EXCLUDED.metrics,
                geo_lng = EXCLUDED.geo_lng,
                geo_lat = EXCLUDED.geo_lat,
                updated_at = NOW()
            """,
            (
                tenant_id,
                [n.node_id for n in nodes],
                [n.subgraph_slug for n in nodes],
                [n.type_name for n in nodes],
                [n.node_key for n in nodes],
                [n.title for n in nodes],
                [n.properties_json for n in nodes],
                [n.metrics_json for n in nodes],
                [n.geo_lng for n in nodes],
                [n.geo_lat for n in nodes],
            ),
        )

    # Resolve persisted node IDs if an existing `main` row owned `(type_name, node_key)`
    if links:
        cur.execute(
            """
            INSERT INTO xb_links (
                id, tenant_id, subgraph_slug, link_type,
                src_node_id, dst_node_id, weight, updated_at
            )
            SELECT
                u.lid, %s, u.sg, u.ltype, u.src_id, u.dst_id, u.w, NOW()
            FROM UNNEST(
                %s::uuid[], %s::text[], %s::text[], %s::uuid[], %s::uuid[], %s::float8[]
            ) AS u(lid, sg, ltype, src_id, dst_id, w)
            ON CONFLICT (tenant_id, subgraph_slug, link_type, src_node_id, dst_node_id)
            DO UPDATE SET
                weight = EXCLUDED.weight,
                updated_at = NOW()
            """,
            (
                tenant_id,
                [lnk.link_id for lnk in links],
                [lnk.subgraph_slug for lnk in links],
                [lnk.link_type for lnk in links],
                [lnk.src_node_id for lnk in links],
                [lnk.dst_node_id for lnk in links],
                [lnk.weight for lnk in links],
            ),
        )


def _lookup_node_id_map(
    cur: psycopg.Cursor[Any],
    *,
    tenant_id: str,
    subgraph_slug: str,
    keys: Sequence[tuple[str, str]],
) -> dict[tuple[str, str], uuid.UUID]:
    if not keys:
        return {}
    t_list = [k[0] for k in keys]
    k_list = [k[1] for k in keys]
    cur.execute(
        """
        SELECT n.type_name, n.node_key, n.id
        FROM xb_nodes n
        JOIN UNNEST(%s::text[], %s::text[]) AS q(tname, nkey)
          ON n.type_name = q.tname AND n.node_key = q.nkey
        WHERE n.tenant_id = %s AND n.subgraph_slug = %s
        """,
        (t_list, k_list, tenant_id, subgraph_slug),
    )
    return {(str(r[0]), str(r[1])): uuid.UUID(str(r[2])) for r in cur.fetchall()}


def _batch_replace_metric_series(
    cur: psycopg.Cursor[Any],
    *,
    tenant_id: str,
    batch_id: uuid.UUID,
    points: Sequence[MetricPointRow],
) -> int:
    cur.execute("SELECT ax_ensure_metric_partitions(2, 2)")
    cur.execute(
        "DELETE FROM xb_metric_series WHERE tenant_id = %s AND batch_id = %s",
        (tenant_id, batch_id),
    )
    chunk_size = 10_000
    written = 0
    for offset in range(0, len(points), chunk_size):
        chunk = points[offset : offset + chunk_size]
        cur.execute(
            """
            INSERT INTO xb_metric_series (
                tenant_id, node_id, metric, ts, value, value_text, source_tag, batch_id
            )
            SELECT
                %s, u.nid, u.mkey, u.ts_val, u.val, NULL, u.stag, %s
            FROM UNNEST(
                %s::uuid[], %s::text[], %s::timestamptz[], %s::float8[], %s::text[]
            ) AS u(nid, mkey, ts_val, val, stag)
            ON CONFLICT (tenant_id, node_id, metric, ts, source_tag) DO UPDATE SET
                value = EXCLUDED.value,
                batch_id = EXCLUDED.batch_id
            """,
            (
                tenant_id,
                batch_id,
                [p.node_id for p in chunk],
                [p.metric for p in chunk],
                [p.ts for p in chunk],
                [p.value for p in chunk],
                [p.source_tag for p in chunk],
            ),
        )
        written += len(chunk)
    return written


def _seed_rules_and_alerts(
    cur: psycopg.Cursor[Any],
    *,
    tenant_id: str,
    specs: Sequence[AlertSeedSpec],
) -> None:
    for spec in specs:
        rule_det_id = _det_uuid(tenant_id, "ax_rule", spec.rule_name)
        target_json = json.dumps({"type_name": spec.type_name, "metric": spec.metric})
        cond_json = json.dumps(
            {"kind": "THRESHOLD", "op": spec.op, "value": spec.threshold, "for_minutes": 0}
        )
        cur.execute(
            """
            INSERT INTO ax_rules (
                id, tenant_id, name, description, scope_kind, target, condition,
                severity, title_template, body_template, cooldown_minutes,
                resolve_after_minutes, enabled, updated_at
            )
            VALUES (
                %s, %s, %s, %s, 'METRIC', %s::jsonb, %s::jsonb,
                %s, %s, %s, 20,
                10, TRUE, NOW()
            )
            ON CONFLICT (tenant_id, name) DO UPDATE SET
                description = EXCLUDED.description,
                target = EXCLUDED.target,
                condition = EXCLUDED.condition,
                severity = EXCLUDED.severity,
                title_template = EXCLUDED.title_template,
                body_template = EXCLUDED.body_template,
                enabled = TRUE,
                updated_at = NOW()
            RETURNING id
            """,
            (
                rule_det_id,
                tenant_id,
                spec.rule_name,
                spec.description,
                target_json,
                cond_json,
                spec.severity,
                spec.title_template,
                spec.body_template,
            ),
        )
        row = cur.fetchone()
        if row is None:
            continue
        rule_id = uuid.UUID(str(row[0]))
        dedup_key = f"{rule_id}:{spec.alert_node_id}"
        alert_id = _det_uuid(tenant_id, "ax_alert", spec.rule_name)
        cur.execute(
            "DELETE FROM ax_alerts WHERE tenant_id = %s AND dedup_key = %s AND id <> %s",
            (tenant_id, dedup_key, alert_id),
        )
        cur.execute(
            """
            INSERT INTO ax_alerts (
                id, tenant_id, rule_id, dedup_key, node_id, severity, status,
                title, body, last_value, occurrences,
                first_seen_at, last_seen_at
            )
            VALUES (
                %s, %s, %s, %s, %s, %s, 'OPEN',
                %s, %s, %s, 2,
                NOW() - INTERVAL '25 minutes', NOW()
            )
            ON CONFLICT (id) DO UPDATE SET
                rule_id = EXCLUDED.rule_id,
                dedup_key = EXCLUDED.dedup_key,
                node_id = EXCLUDED.node_id,
                severity = EXCLUDED.severity,
                status = 'OPEN',
                title = EXCLUDED.title,
                body = EXCLUDED.body,
                last_value = EXCLUDED.last_value,
                last_seen_at = NOW()
            """,
            (
                alert_id,
                tenant_id,
                rule_id,
                dedup_key,
                spec.alert_node_id,
                spec.severity,
                spec.alert_title,
                spec.alert_body,
                spec.observed_value,
            ),
        )


def seed_semi_supply(conn: psycopg.Connection[Any], tenant_id: str) -> dict[str, Any]:
    t0 = time.perf_counter()
    rng = random.Random(BASE_SEED)
    sub = "semi_supply"

    countries = ["中国", "日本", "德国", "韩国", "新加坡", "马来西亚"]
    grades = ["11N", "9N", "6N"]
    vendor_prefixes = [
        "东海特气",
        "九州硅片",
        "莱茵光刻胶",
        "汉拿电子氟",
        "长三角靶材",
        "南洋高纯硅",
    ]
    fab_cities = [
        ("张江", 121.6012, 31.2045),
        ("临港", 121.8940, 30.9082),
        ("无锡", 120.3690, 31.5285),
        ("合肥", 117.1850, 31.8256),
        ("苏州", 120.7125, 31.3180),
        ("武汉", 114.4320, 30.4820),
        ("深圳", 114.0859, 22.5470),
        ("北京亦庄", 116.5128, 39.7950),
    ]
    process_nodes = [14, 22, 28, 40, 55, 65]
    tool_vendors = ["华芯光刻装备", "北方微纳刻蚀", "中微等离子装备", "盛美精密清洗"]
    recipes = [
        "DUV_28NM_M1",
        "ICP_GATE_ETCH",
        "ARFI_14NM_FIN",
        "IGBT_40NM_PASS",
        "MCU_28NM_FLASH",
        "CVD_OXIDE_DEP",
    ]
    port_defs = [
        ("浦东空港恒温货站", "PVG", "恒温宽体货机", 121.8052, 31.1434),
        ("苏南硕放保税航站", "WUX", "恒温包机快线", 120.4290, 31.4944),
        ("仁川保税转运空港", "ICN", "跨国冷链货机", 126.4407, 37.4602),
        ("成田精密电子货站", "NRT", "跨国冷链货机", 140.3929, 35.7720),
        ("法兰克福航空保税港", "FRA", "洲际恒温货机", 8.5622, 50.0379),
        ("樟宜半导体转运港", "SIN", "恒温宽体货机", 103.9915, 1.3644),
    ]
    regions = ["华东大区", "华南大区", "西南大区", "华北大区", "华中大区", "东北大区"]
    oem_customers = [
        "星途新能源汽车",
        "岭南智造车企",
        "山城智能汽车",
        "长三角智驾集团",
        "北国汽车集团",
        "江淮电控总成",
    ]
    priorities = ["P0_STRATEGIC", "P1_KEY", "P2_STANDARD"]

    nodes: list[NodeRow] = []
    links: list[LinkRow] = []

    # 1) 42 SpecialtyVendor
    vendor_ids: list[uuid.UUID] = []
    for i in range(1, 43):
        key = f"SV-{i:02d}"
        nid = _det_uuid(tenant_id, sub, key)
        vendor_ids.append(nid)
        v_name = f"{vendor_prefixes[(i - 1) % len(vendor_prefixes)]}{i:02d}号基地"
        supply_idx = round(84.0 + rng.random() * 14.5, 2)
        lead_days = round(9.0 + rng.random() * 20.0, 1)
        nodes.append(
            NodeRow(
                node_id=nid,
                subgraph_slug=sub,
                type_name="SpecialtyVendor",
                node_key=key,
                title=v_name,
                properties_json=json.dumps(
                    {
                        "vendor_name": v_name,
                        "country": countries[(i - 1) % len(countries)],
                        "material_grade": grades[(i - 1) % len(grades)],
                    },
                    ensure_ascii=False,
                ),
                metrics_json=json.dumps(
                    {"supply_index": supply_idx, "lead_time_days": lead_days}
                ),
                geo_lng=round(114.0 + (i % 12) * 1.6, 4),
                geo_lat=round(24.0 + (i // 6) * 2.1, 4),
            )
        )

    # 2) 16 WaferFab + 112 LithoTool + 160 WaferLotEvent + 16 YieldRollupCube
    fab_ids: list[uuid.UUID] = []
    fab_keys: list[str] = []
    fab_yields: list[float] = []
    tool_ids: list[uuid.UUID] = []
    tool_codes: list[str] = []

    for f_idx in range(1, 17):
        f_key = f"FAB-{f_idx:02d}"
        f_id = _det_uuid(tenant_id, sub, f_key)
        fab_ids.append(f_id)
        fab_keys.append(f_key)
        city_label, base_lng, base_lat = fab_cities[(f_idx - 1) % len(fab_cities)]
        f_name = f"{city_label}车规晶圆{f_idx:02d}厂"
        p_nm = process_nodes[(f_idx - 1) % len(process_nodes)]
        wpm = float(32000 + (f_idx % 7) * 4500)
        f_yield = round(90.5 + (f_idx % 6) * 1.15, 2)
        fab_yields.append(f_yield)
        safety_stock = float(5200 + (f_idx % 5) * 900)

        # Generate 7 LithoTool children first to compute exact avg_tool_oee
        child_oees: list[float] = []
        for t_local in range(1, 8):
            t_seq = (f_idx - 1) * 7 + t_local
            t_key = f"LT-{t_seq:03d}"
            t_code = f"LITHO-{f_idx:02d}-{t_local:02d}"
            t_id = _det_uuid(tenant_id, sub, t_key)
            tool_ids.append(t_id)
            tool_codes.append(t_code)
            oee = round(85.0 + rng.random() * 11.5, 2)
            temp_c = round(22.0 + rng.random() * 3.8, 2)
            defect_ppm = round(0.25 + rng.random() * 1.6, 3)
            drift_risk = round(max(0.0, (temp_c - 22.0) * 3.5 + defect_ppm * 8.0), 2)
            child_oees.append(oee)
            nodes.append(
                NodeRow(
                    node_id=t_id,
                    subgraph_slug=sub,
                    type_name="LithoTool",
                    node_key=t_key,
                    title=t_code,
                    properties_json=json.dumps(
                        {
                            "tool_code": t_code,
                            "tool_vendor": tool_vendors[(t_seq - 1) % len(tool_vendors)],
                            "chamber_id": f"CH-{f_idx:02d}{t_local:02d}",
                        },
                        ensure_ascii=False,
                    ),
                    metrics_json=json.dumps(
                        {
                            "oee_pct": oee,
                            "chamber_temp_c": temp_c,
                            "particle_defect_rate": defect_ppm,
                            "thermal_drift_risk": drift_risk,
                        }
                    ),
                    geo_lng=None,
                    geo_lat=None,
                )
            )
            links.append(
                LinkRow(
                    link_id=_det_uuid(tenant_id, sub, f"EQUIPPED_WITH:{f_key}->{t_key}"),
                    subgraph_slug=sub,
                    link_type="EQUIPPED_WITH",
                    src_node_id=f_id,
                    dst_node_id=t_id,
                    weight=0.75,
                )
            )

        avg_oee = round(sum(child_oees) / len(child_oees), 2)
        eff_good = round(wpm * (f_yield / 100.0) * (avg_oee / 100.0), 2)
        nodes.append(
            NodeRow(
                node_id=f_id,
                subgraph_slug=sub,
                type_name="WaferFab",
                node_key=f_key,
                title=f_name,
                properties_json=json.dumps(
                    {
                        "fab_name": f_name,
                        "process_nm": p_nm,
                        "cleanroom_class": "ISO-1" if p_nm <= 28 else "ISO-2",
                    },
                    ensure_ascii=False,
                ),
                metrics_json=json.dumps(
                    {
                        "wafer_starts_wpm": wpm,
                        "fab_yield_pct": f_yield,
                        "safety_wafer_stock": safety_stock,
                        "avg_tool_oee": avg_oee,
                        "effective_good_wpm": eff_good,
                    }
                ),
                geo_lng=round(base_lng + (f_idx % 3) * 0.04, 4),
                geo_lat=round(base_lat + (f_idx // 4) * 0.03, 4),
            )
        )

        # 10 WaferLotEvent per fab -> 160 total
        for l_local in range(1, 11):
            l_seq = (f_idx - 1) * 10 + l_local
            l_key = f"LOT-{l_seq:04d}"
            l_code = f"W2605-{f_idx:02d}{l_local:02d}"
            l_id = _det_uuid(tenant_id, sub, l_key)
            die_good = round(11800.0 + rng.random() * 6800.0, 0)
            nodes.append(
                NodeRow(
                    node_id=l_id,
                    subgraph_slug=sub,
                    type_name="WaferLotEvent",
                    node_key=l_key,
                    title=l_code,
                    properties_json=json.dumps(
                        {
                            "lot_code": l_code,
                            "step_recipe": recipes[(l_seq - 1) % len(recipes)],
                        },
                        ensure_ascii=False,
                    ),
                    metrics_json=json.dumps({"die_good_cnt": die_good}),
                    geo_lng=None,
                    geo_lat=None,
                )
            )
            links.append(
                LinkRow(
                    link_id=_det_uuid(tenant_id, sub, f"RUNS_LOT:{f_key}->{l_key}"),
                    subgraph_slug=sub,
                    link_type="RUNS_LOT",
                    src_node_id=f_id,
                    dst_node_id=l_id,
                    weight=0.7,
                )
            )

        # 1 YieldRollupCube per fab -> 16 total
        c_key = f"YCUBE-{f_idx:02d}"
        c_id = _det_uuid(tenant_id, sub, c_key)
        period_lbl = f"2026-M05-{f_key}"
        scrap_usd = round((100.0 - f_yield) * 14500.0, 2)
        nodes.append(
            NodeRow(
                node_id=c_id,
                subgraph_slug=sub,
                type_name="YieldRollupCube",
                node_key=c_key,
                title=period_lbl,
                properties_json=json.dumps({"cube_period": period_lbl}, ensure_ascii=False),
                metrics_json=json.dumps(
                    {"monthly_avg_yield": f_yield, "scrap_cost_usd": scrap_usd}
                ),
                geo_lng=None,
                geo_lat=None,
            )
        )
        links.append(
            LinkRow(
                link_id=_det_uuid(tenant_id, sub, f"SUMMARIZES_YIELD:{f_key}->{c_key}"),
                subgraph_slug=sub,
                link_type="SUMMARIZES_YIELD",
                src_node_id=f_id,
                dst_node_id=c_id,
                weight=0.65,
            )
        )

    # 3) 18 CustomsPort
    port_ids: list[uuid.UUID] = []
    for p_idx in range(1, 19):
        p_key = f"PORT-{p_idx:02d}"
        p_id = _det_uuid(tenant_id, sub, p_key)
        port_ids.append(p_id)
        base_pname, base_pcode, t_mode, p_lng, p_lat = port_defs[(p_idx - 1) % len(port_defs)]
        p_title = f"{base_pname}-{p_idx:02d}监管区"
        c_hours = round(10.0 + (p_idx % 7) * 3.8 + rng.random() * 2.5, 1)
        backlog = round(8.0 + (p_idx % 8) * 7.0, 0)
        nodes.append(
            NodeRow(
                node_id=p_id,
                subgraph_slug=sub,
                type_name="CustomsPort",
                node_key=p_key,
                title=p_title,
                properties_json=json.dumps(
                    {
                        "port_name": p_title,
                        "port_code": f"{base_pcode}{p_idx:02d}",
                        "transport_mode": t_mode,
                    },
                    ensure_ascii=False,
                ),
                metrics_json=json.dumps(
                    {"clearance_hours": c_hours, "backlog_lots": backlog}
                ),
                geo_lng=round(p_lng + (p_idx % 3) * 0.05, 4),
                geo_lat=round(p_lat + (p_idx // 6) * 0.04, 4),
            )
        )

    # 4) 32 RegionalHub
    hub_ids: list[uuid.UUID] = []
    for h_idx in range(1, 33):
        h_key = f"HUB-{h_idx:02d}"
        h_id = _det_uuid(tenant_id, sub, h_key)
        hub_ids.append(h_id)
        reg = regions[(h_idx - 1) % len(regions)]
        h_name = f"{reg}恒温芯片{h_idx:02d}仓"
        inv_k = round(180.0 + rng.random() * 620.0, 1)
        out_k = round(24.0 + rng.random() * 28.0, 1)
        dos = round(inv_k / out_k, 2)
        nodes.append(
            NodeRow(
                node_id=h_id,
                subgraph_slug=sub,
                type_name="RegionalHub",
                node_key=h_key,
                title=h_name,
                properties_json=json.dumps(
                    {"hub_name": h_name, "region": reg}, ensure_ascii=False
                ),
                metrics_json=json.dumps(
                    {
                        "chip_inventory_k": inv_k,
                        "outbound_rate_k": out_k,
                        "days_of_supply": dos,
                    }
                ),
                geo_lng=round(106.5 + (h_idx % 8) * 2.1, 4),
                geo_lat=round(23.0 + (h_idx // 8) * 4.2, 4),
            )
        )

    # 5) 130 AutoOemOrder (strict ONE_TO_MANY from RegionalHub -> AutoOemOrder)
    for o_idx in range(1, 131):
        o_key = f"OEM-{o_idx:03d}"
        o_id = _det_uuid(tenant_id, sub, o_key)
        cust = oem_customers[(o_idx - 1) % len(oem_customers)]
        tier = priorities[(o_idx - 1) % len(priorities)]
        o_title = f"{cust}车规芯片保供单-{o_idx:03d}"
        ord_qty = round(95.0 + rng.random() * 340.0, 1)
        ot_prob = round(0.76 + rng.random() * 0.22, 3)
        parent_hub_idx = (o_idx - 1) % len(hub_ids)
        parent_hub_id = hub_ids[parent_hub_idx]
        nodes.append(
            NodeRow(
                node_id=o_id,
                subgraph_slug=sub,
                type_name="AutoOemOrder",
                node_key=o_key,
                title=o_title,
                properties_json=json.dumps(
                    {
                        "order_title": o_title,
                        "oem_customer": cust,
                        "priority_tier": tier,
                    },
                    ensure_ascii=False,
                ),
                metrics_json=json.dumps(
                    {"ordered_qty_k": ord_qty, "on_time_prob": ot_prob}
                ),
                geo_lng=None,
                geo_lat=None,
            )
        )
        links.append(
            LinkRow(
                link_id=_det_uuid(
                    tenant_id, sub, f"DELIVERS_ORDER:HUB-{parent_hub_idx + 1:02d}->{o_key}"
                ),
                subgraph_slug=sub,
                link_type="DELIVERS_ORDER",
                src_node_id=parent_hub_id,
                dst_node_id=o_id,
                weight=0.85,
            )
        )

    # MANY_TO_MANY supply/logistics edges (42*10=420, 16*14=224, 18*16=288 -> total 1,350 edges)
    for v_i, v_id in enumerate(vendor_ids):
        for offset in range(10):
            f_i = (v_i * 3 + offset) % len(fab_ids)
            links.append(
                LinkRow(
                    link_id=_det_uuid(tenant_id, sub, f"SUPPLIES:{v_i}->{f_i}"),
                    subgraph_slug=sub,
                    link_type="SUPPLIES_MATERIAL",
                    src_node_id=v_id,
                    dst_node_id=fab_ids[f_i],
                    weight=0.8,
                )
            )
    for f_i, f_id in enumerate(fab_ids):
        for offset in range(14):
            p_i = (f_i * 2 + offset) % len(port_ids)
            links.append(
                LinkRow(
                    link_id=_det_uuid(tenant_id, sub, f"SHIPS:{f_i}->{p_i}"),
                    subgraph_slug=sub,
                    link_type="SHIPS_VIA_PORT",
                    src_node_id=f_id,
                    dst_node_id=port_ids[p_i],
                    weight=0.85,
                )
            )
    for p_i, p_id in enumerate(port_ids):
        for offset in range(16):
            h_i = (p_i * 2 + offset) % len(hub_ids)
            links.append(
                LinkRow(
                    link_id=_det_uuid(tenant_id, sub, f"DISPATCHES:{p_i}->{h_i}"),
                    subgraph_slug=sub,
                    link_type="DISPATCHES_TO_HUB",
                    src_node_id=p_id,
                    dst_node_id=hub_ids[h_i],
                    weight=0.85,
                )
            )

    # Anchor nodes in `main` (geo=None avoids duplicate map plotting; supports AxQL/View/AxSim)
    main_nodes: list[NodeRow] = [
        NodeRow(
            node_id=_det_uuid(tenant_id, "main", "WaferFab:FAB-01"),
            subgraph_slug="main",
            type_name="WaferFab",
            node_key="FAB-01",
            title="张江车规晶圆一厂",
            properties_json=json.dumps(
                {"fab_name": "张江车规晶圆一厂", "process_nm": 28, "cleanroom_class": "ISO-1"},
                ensure_ascii=False,
            ),
            metrics_json=json.dumps(
                {
                    "wafer_starts_wpm": 45000.0,
                    "fab_yield_pct": 94.6,
                    "safety_wafer_stock": 8200.0,
                    "avg_tool_oee": 91.5,
                    "effective_good_wpm": 38963.3,
                }
            ),
            geo_lng=None,
            geo_lat=None,
        ),
        NodeRow(
            node_id=_det_uuid(tenant_id, "main", "WaferFab:FAB-02"),
            subgraph_slug="main",
            type_name="WaferFab",
            node_key="FAB-02",
            title="临港先进制程二厂",
            properties_json=json.dumps(
                {"fab_name": "临港先进制程二厂", "process_nm": 14, "cleanroom_class": "ISO-1"},
                ensure_ascii=False,
            ),
            metrics_json=json.dumps(
                {
                    "wafer_starts_wpm": 35000.0,
                    "fab_yield_pct": 92.4,
                    "safety_wafer_stock": 6400.0,
                    "avg_tool_oee": 89.2,
                    "effective_good_wpm": 28847.3,
                }
            ),
            geo_lng=None,
            geo_lat=None,
        ),
        NodeRow(
            node_id=_det_uuid(tenant_id, "main", "LithoTool:LT-001"),
            subgraph_slug="main",
            type_name="LithoTool",
            node_key="LT-001",
            title="LITHO-01-01",
            properties_json=json.dumps(
                {
                    "tool_code": "LITHO-01-01",
                    "tool_vendor": "华芯光刻装备",
                    "chamber_id": "CH-0101",
                },
                ensure_ascii=False,
            ),
            metrics_json=json.dumps(
                {
                    "oee_pct": 91.5,
                    "chamber_temp_c": 22.4,
                    "particle_defect_rate": 0.42,
                    "thermal_drift_risk": 4.76,
                }
            ),
            geo_lng=None,
            geo_lat=None,
        ),
        NodeRow(
            node_id=_det_uuid(tenant_id, "main", "CustomsPort:PORT-01"),
            subgraph_slug="main",
            type_name="CustomsPort",
            node_key="PORT-01",
            title="浦东空港恒温货站",
            properties_json=json.dumps(
                {
                    "port_name": "浦东空港恒温货站",
                    "port_code": "PVG",
                    "transport_mode": "恒温宽体货机",
                },
                ensure_ascii=False,
            ),
            metrics_json=json.dumps({"clearance_hours": 28.0, "backlog_lots": 42.0}),
            geo_lng=None,
            geo_lat=None,
        ),
        NodeRow(
            node_id=_det_uuid(tenant_id, "main", "RegionalHub:HUB-01"),
            subgraph_slug="main",
            type_name="RegionalHub",
            node_key="HUB-01",
            title="华东大区恒温芯片01仓",
            properties_json=json.dumps(
                {"hub_name": "华东大区恒温芯片01仓", "region": "华东大区"},
                ensure_ascii=False,
            ),
            metrics_json=json.dumps(
                {"chip_inventory_k": 320.0, "outbound_rate_k": 40.0, "days_of_supply": 8.0}
            ),
            geo_lng=None,
            geo_lat=None,
        ),
        NodeRow(
            node_id=_det_uuid(tenant_id, "main", "AutoOemOrder:OEM-001"),
            subgraph_slug="main",
            type_name="AutoOemOrder",
            node_key="OEM-001",
            title="星途新能源汽车车规芯片保供单",
            properties_json=json.dumps(
                {
                    "order_title": "星途新能源汽车车规芯片保供单",
                    "oem_customer": "星途新能源汽车",
                    "priority_tier": "P0_STRATEGIC",
                },
                ensure_ascii=False,
            ),
            metrics_json=json.dumps({"ordered_qty_k": 320.0, "on_time_prob": 0.92}),
            geo_lng=None,
            geo_lat=None,
        ),
    ]
    for lot_i in range(1, 6):
        l_key = f"LOT-000{lot_i}"
        main_nodes.append(
            NodeRow(
                node_id=_det_uuid(tenant_id, "main", f"WaferLotEvent:{l_key}"),
                subgraph_slug="main",
                type_name="WaferLotEvent",
                node_key=l_key,
                title=f"W2605-M0{lot_i}",
                properties_json=json.dumps(
                    {"lot_code": f"W2605-M0{lot_i}", "step_recipe": recipes[lot_i - 1]},
                    ensure_ascii=False,
                ),
                metrics_json=json.dumps({"die_good_cnt": float(14200 + lot_i * 250)}),
                geo_lng=None,
                geo_lat=None,
            )
        )

    # 30,000-row DuckLake table `semi_lot_telemetry`
    lake_rows = 30_000
    now_utc = datetime.now(UTC).replace(microsecond=0)
    base_ts = now_utc - timedelta(hours=24)
    lot_ids_col = [f"SL-{idx:06d}" for idx in range(1, lake_rows + 1)]
    fab_key_col = [fab_keys[(idx - 1) % 16] for idx in range(1, lake_rows + 1)]
    tool_code_col = [tool_codes[(idx - 1) % 112] for idx in range(1, lake_rows + 1)]
    proc_nm_col = [process_nodes[(idx - 1) % len(process_nodes)] for idx in range(1, lake_rows + 1)]
    recipe_col = [recipes[(idx - 1) % len(recipes)] for idx in range(1, lake_rows + 1)]
    wafer_cnt_col = [25 if idx % 10 != 0 else 24 for idx in range(1, lake_rows + 1)]
    die_good_col = [float(12400 + (idx * 37) % 6200) for idx in range(1, lake_rows + 1)]
    yield_col = [round(89.5 + ((idx * 19) % 95) * 0.1, 2) for idx in range(1, lake_rows + 1)]
    temp_col = [round(21.9 + ((idx * 13) % 36) * 0.1, 2) for idx in range(1, lake_rows + 1)]
    part_col = [round(0.20 + ((idx * 11) % 140) * 0.01, 2) for idx in range(1, lake_rows + 1)]
    rec_ts_col = [
        base_ts + timedelta(seconds=(idx * 86400) // lake_rows) for idx in range(lake_rows)
    ]

    semi_arrow = pa.Table.from_pydict(
        {
            "lot_id": pa.array(lot_ids_col, type=pa.string()),
            "fab_key": pa.array(fab_key_col, type=pa.string()),
            "tool_code": pa.array(tool_code_col, type=pa.string()),
            "process_nm": pa.array(proc_nm_col, type=pa.int64()),
            "step_recipe": pa.array(recipe_col, type=pa.string()),
            "wafer_count": pa.array(wafer_cnt_col, type=pa.int64()),
            "die_good_cnt": pa.array(die_good_col, type=pa.float64()),
            "yield_pct": pa.array(yield_col, type=pa.float64()),
            "chamber_temp_c": pa.array(temp_col, type=pa.float64()),
            "particle_ppm": pa.array(part_col, type=pa.float64()),
            "recorded_at": pa.array(rec_ts_col, type=pa.timestamp("us", tz="UTC")),
        }
    )
    lake_batch_id = _det_uuid(tenant_id, "lake_batch", "semi_lot_telemetry")
    snap_id = _write_arrow_to_ducklake(
        tenant_id=tenant_id,
        table_name="semi_lot_telemetry",
        arrow_tbl=semi_arrow,
        batch_id=lake_batch_id,
    )

    semi_cols = [
        ColumnMetaSpec("lot_id", "STRING", "晶圆传感流水号", "批次流水标识"),
        ColumnMetaSpec("fab_key", "STRING", "所属晶圆厂编码", "晶圆厂编码"),
        ColumnMetaSpec("tool_code", "STRING", "曝光或刻蚀机台号", "机台编号"),
        ColumnMetaSpec("process_nm", "INT64", "工艺制程节点", "制程节点", "nm"),
        ColumnMetaSpec("step_recipe", "STRING", "制程工艺配方", "工艺配方"),
        ColumnMetaSpec("wafer_count", "INT64", "FOUP盒内晶圆片数", "批次片数", "片"),
        ColumnMetaSpec("die_good_cnt", "FLOAT64", "CP探针良品裸片数", "良品裸片", "颗"),
        ColumnMetaSpec("yield_pct", "FLOAT64", "单批流片良率", "批次良率", "%"),
        ColumnMetaSpec("chamber_temp_c", "FLOAT64", "反应腔实时温度", "腔体温度", "℃"),
        ColumnMetaSpec("particle_ppm", "FLOAT64", "腔体微粒缺陷率", "颗粒缺陷率", "ppm"),
        ColumnMetaSpec("recorded_at", "TIMESTAMP", "传感采样UTC时间戳", "采样时间"),
    ]

    with conn.cursor() as cur:
        cur.execute("SELECT set_config('app.tenant_id', %s, true)", (tenant_id,))
        ds_id = _upsert_lake_dataset_metadata(
            cur,
            tenant_id=tenant_id,
            dataset_name="semi_lot_telemetry",
            display_name="晶圆批次与腔体传感明细",
            description=(
                "半导体前道流片批次良率、光刻/刻蚀腔体温度与颗粒缺陷传感明细表（30,000 行）"
            ),
            columns=semi_cols,
            row_count=lake_rows,
            snapshot_id=snap_id,
            tags=["showcase", "semi_supply", "telemetry"],
        )

        _batch_replace_subgraph_topology(
            cur,
            tenant_id=tenant_id,
            subgraph_slug=sub,
            nodes=nodes,
            links=links,
        )
        # Clean only our own deterministic main anchor links before re-upserting main anchors
        _batch_upsert_nodes_and_links(cur, tenant_id=tenant_id, nodes=main_nodes, links=[])
        main_id_map = _lookup_node_id_map(
            cur,
            tenant_id=tenant_id,
            subgraph_slug="main",
            keys=[(n.type_name, n.node_key) for n in main_nodes],
        )
        m_fab1 = main_id_map[("WaferFab", "FAB-01")]
        m_fab2 = main_id_map[("WaferFab", "FAB-02")]
        m_lt1 = main_id_map[("LithoTool", "LT-001")]
        m_port1 = main_id_map[("CustomsPort", "PORT-01")]
        m_hub1 = main_id_map[("RegionalHub", "HUB-01")]
        m_oem1 = main_id_map[("AutoOemOrder", "OEM-001")]

        cur.execute(
            """
            DELETE FROM xb_links
            WHERE tenant_id = %s AND subgraph_slug = 'main'
              AND link_type IN (
                  'EQUIPPED_WITH', 'RUNS_LOT', 'SHIPS_VIA_PORT',
                  'DISPATCHES_TO_HUB', 'DELIVERS_ORDER'
              )
            """,
            (tenant_id,),
        )
        main_links: list[LinkRow] = [
            LinkRow(
                _det_uuid(tenant_id, "main", "L:FAB1->LT1"),
                "main",
                "EQUIPPED_WITH",
                m_fab1,
                m_lt1,
                0.75,
            ),
            LinkRow(
                _det_uuid(tenant_id, "main", "L:FAB1->PORT1"),
                "main",
                "SHIPS_VIA_PORT",
                m_fab1,
                m_port1,
                0.85,
            ),
            LinkRow(
                _det_uuid(tenant_id, "main", "L:PORT1->HUB1"),
                "main",
                "DISPATCHES_TO_HUB",
                m_port1,
                m_hub1,
                0.85,
            ),
            LinkRow(
                _det_uuid(tenant_id, "main", "L:HUB1->OEM1"),
                "main",
                "DELIVERS_ORDER",
                m_hub1,
                m_oem1,
                0.85,
            ),
        ]
        for lot_i in range(1, 6):
            l_id_m = main_id_map[("WaferLotEvent", f"LOT-000{lot_i}")]
            src_fab = m_fab1 if lot_i <= 3 else m_fab2
            main_links.append(
                LinkRow(
                    _det_uuid(tenant_id, "main", f"L:RUNS_LOT:{lot_i}"),
                    "main",
                    "RUNS_LOT",
                    src_fab,
                    l_id_m,
                    0.7,
                )
            )
        _batch_upsert_nodes_and_links(cur, tenant_id=tenant_id, nodes=[], links=main_links)

        # Build exact 16,800 time-series points over trailing 24 hours
        series_pts: list[MetricPointRow] = []
        # 110 LithoTool nodes in semi_supply * 75 timestamps * 2 metrics = 16,500
        for t_idx in range(110):
            tid = tool_ids[t_idx]
            for step in range(75):
                ts = now_utc - timedelta(minutes=step * 19)
                wave = math.sin((step + t_idx) * 0.25)
                series_pts.append(
                    MetricPointRow(tid, "oee_pct", ts, round(89.5 + wave * 3.8, 2), "SYNC")
                )
                series_pts.append(
                    MetricPointRow(tid, "chamber_temp_c", ts, round(22.6 + wave * 0.9, 2), "SYNC")
                )
        # 300 main anchor points so cockpit KPIs & charts render 24h series -> total 16,800
        for step in range(75):
            ts = now_utc - timedelta(minutes=step * 19)
            wave = math.sin(step * 0.2)
            series_pts.append(
                MetricPointRow(m_lt1, "oee_pct", ts, round(91.2 + wave * 2.4, 2), "SYNC")
            )
            series_pts.append(
                MetricPointRow(m_lt1, "chamber_temp_c", ts, round(22.4 + wave * 0.6, 2), "SYNC")
            )
        for step in range(50):
            ts = now_utc - timedelta(minutes=step * 28)
            wpm_pt = 45000.0 + (step % 5) * 120.0
            yld_pt = round(94.2 + (step % 4) * 0.15, 2)
            series_pts.append(MetricPointRow(m_fab1, "wafer_starts_wpm", ts, wpm_pt, "SYNC"))
            series_pts.append(MetricPointRow(m_fab1, "fab_yield_pct", ts, yld_pt, "SYNC"))
        for step in range(25):
            ts = now_utc - timedelta(minutes=step * 55)
            clr_pt = round(26.5 + (step % 5) * 0.8, 1)
            inv_pt = round(315.0 + (step % 6) * 4.5, 1)
            series_pts.append(MetricPointRow(m_port1, "clearance_hours", ts, clr_pt, "SYNC"))
            series_pts.append(MetricPointRow(m_hub1, "chip_inventory_k", ts, inv_pt, "SYNC"))

        series_written = _batch_replace_metric_series(
            cur,
            tenant_id=tenant_id,
            batch_id=_det_uuid(tenant_id, "series_batch", sub),
            points=series_pts,
        )

        _seed_rules_and_alerts(
            cur,
            tenant_id=tenant_id,
            specs=[
                AlertSeedSpec(
                    rule_name="semi_litho_thermal_drift_warn",
                    description="光刻或刻蚀机台腔体热漂移风险指数超阈值预警",
                    type_name="LithoTool",
                    metric="thermal_drift_risk",
                    op="GT",
                    threshold=18.0,
                    severity="WARN",
                    title_template="机台热漂移预警：{{node.title}}",
                    body_template=(
                        "机台 {{node.title}} 热漂移风险指数 {{value}} 分高于阈值 {{threshold}} 分。"
                    ),
                    alert_node_id=tool_ids[0],
                    alert_title="机台热漂移预警：LITHO-01-01",
                    alert_body="机台 LITHO-01-01 反应腔温度轻微上漂，热漂移风险指数 19.6 分。",
                    observed_value=19.6,
                ),
                AlertSeedSpec(
                    rule_name="semi_port_clearance_delay",
                    description="清关口岸平均查验转关耗时超过24小时触发航空包机快通告警",
                    type_name="CustomsPort",
                    metric="clearance_hours",
                    op="GT",
                    threshold=24.0,
                    severity="CRITICAL",
                    title_template="口岸清关拥堵告警：{{node.title}}",
                    body_template=(
                        "口岸 {{node.title}} 平均清关耗时 {{value}} 小时，"
                        "超过红线 {{threshold}} 小时。"
                    ),
                    alert_node_id=port_ids[2],
                    alert_title="口岸清关拥堵告警：仁川保税转运空港-03监管区",
                    alert_body="恒温监管库积压晶圆批次上升，当前清关耗时 28.5 小时。",
                    observed_value=28.5,
                ),
            ],
        )
    conn.commit()

    return {
        "world": sub,
        "dataset_id": str(ds_id),
        "lake_rows": lake_rows,
        "nodes": len(nodes),
        "links": len(links),
        "series_points": series_written,
        "main_anchors": {
            "fab01_id": str(m_fab1),
            "fab02_id": str(m_fab2),
            "port01_id": str(m_port1),
            "hub01_id": str(m_hub1),
            "oem01_id": str(m_oem1),
        },
        "elapsed_s": round(time.perf_counter() - t0, 3),
    }


def seed_med_dispatch(conn: psycopg.Connection[Any], tenant_id: str) -> dict[str, Any]:
    t0 = time.perf_counter()
    rng = random.Random(BASE_SEED + 34)
    sub = "med_dispatch"

    campus_defs = [
        ("市中心第一三甲总院", "黄浦中心区", 121.4737, 31.2304),
        ("浦东陆家嘴急诊医学中心", "浦东新区", 121.5255, 31.2363),
        ("徐汇重症医学临床总院", "徐汇区", 121.4369, 31.1993),
        ("静安北部区域医疗中心", "静安区", 121.4501, 31.2782),
        ("虹桥交通枢纽创伤中心", "闵行区", 121.3256, 31.1965),
        ("杨浦东北部急救临床中心", "杨浦区", 121.5260, 31.2985),
        ("普陀西部心脑血管总院", "普陀区", 121.3965, 31.2495),
        ("长宁国际空港急诊分院", "长宁区", 121.3812, 31.2150),
        ("张江科学城三甲综合院区", "浦东张江", 121.6080, 31.2010),
        ("宝山北翼重症救治院区", "宝山区", 121.4890, 31.3980),
        ("松江新城区域急诊中心", "松江区", 121.2280, 31.0320),
        ("嘉定北虹桥急救医学中心", "嘉定区", 121.2650, 31.3750),
    ]
    specialties = ["急诊抢救", "重症ICU", "胸痛中心", "卒中中心", "创伤中心"]
    device_kinds = ["ECMO", "有创呼吸机", "CRRT"]
    device_statuses = ["IN_USE", "IN_USE", "STANDBY", "IN_USE"]
    crew_levels = ["重症移动ICU", "高级生命支持ALS", "基础生命支持BLS"]
    acuity_levels = ["I级濒危", "II级危重", "III级急症", "IV级非急症"]
    complaints = ["急性胸痛", "卒中偏瘫", "多发创伤", "重症呼衰", "心源性休克", "急性腹痛"]

    nodes: list[NodeRow] = []
    links: list[LinkRow] = []

    campus_ids: list[uuid.UUID] = []
    campus_keys: list[str] = []
    dept_ids: list[uuid.UUID] = []
    dept_keys: list[str] = []
    lsu_ids: list[uuid.UUID] = []
    amb_ids: list[uuid.UUID] = []
    amb_keys: list[str] = []

    # First pre-allocate 12 campus IDs & 60 dept IDs
    for c_idx in range(1, 13):
        c_key = f"HOSP-{c_idx:02d}"
        campus_keys.append(c_key)
        campus_ids.append(_det_uuid(tenant_id, sub, c_key))
        for s_idx in range(5):
            d_seq = (c_idx - 1) * 5 + s_idx + 1
            d_key = f"DEPT-{d_seq:02d}"
            dept_keys.append(d_key)
            dept_ids.append(_det_uuid(tenant_id, sub, d_key))

    # 1) 96 LifeSupportUnit distributed across 60 ClinicalDepts (strict ONE_TO_MANY OPERATES_DEVICE)
    dept_device_utils: dict[int, list[float]] = {i: [] for i in range(60)}
    for dev_idx in range(1, 97):
        dev_key = f"LSU-{dev_idx:03d}"
        dev_id = _det_uuid(tenant_id, sub, dev_key)
        lsu_ids.append(dev_id)
        d_owner = (dev_idx - 1) % 60
        kind = device_kinds[(dev_idx - 1) % len(device_kinds)]
        st = device_statuses[(dev_idx - 1) % len(device_statuses)]
        dev_code = f"{kind[:4]}-{dev_idx:03d}"
        util = (
            round(74.0 + rng.random() * 23.0, 1)
            if st == "IN_USE"
            else round(20.0 + rng.random() * 25.0, 1)
        )
        rem_hours = round(6.0 + rng.random() * 66.0, 1)
        dept_device_utils[d_owner].append(util)
        nodes.append(
            NodeRow(
                node_id=dev_id,
                subgraph_slug=sub,
                type_name="LifeSupportUnit",
                node_key=dev_key,
                title=dev_code,
                properties_json=json.dumps(
                    {"device_code": dev_code, "device_kind": kind, "status": st},
                    ensure_ascii=False,
                ),
                metrics_json=json.dumps(
                    {"utilization_pct": util, "remaining_filter_hours": rem_hours}
                ),
                geo_lng=None,
                geo_lat=None,
            )
        )
        links.append(
            LinkRow(
                link_id=_det_uuid(
                    tenant_id, sub, f"OPERATES_DEVICE:{dept_keys[d_owner]}->{dev_key}"
                ),
                subgraph_slug=sub,
                link_type="OPERATES_DEVICE",
                src_node_id=dept_ids[d_owner],
                dst_node_id=dev_id,
                weight=0.75,
            )
        )

    # 2) 60 ClinicalDept + 12 HospitalCampus + 12 WardCapacityCube
    campus_dept_occ: dict[int, list[float]] = {i: [] for i in range(12)}
    for d_idx in range(60):
        c_owner = d_idx // 5
        s_idx = d_idx % 5
        d_key = dept_keys[d_idx]
        d_id = dept_ids[d_idx]
        c_name = campus_defs[c_owner][0]
        spec_name = specialties[s_idx]
        d_title = f"{c_name}·{spec_name}"
        phys_cnt = 7 + (d_idx % 8)
        occ_pct = round(76.0 + rng.random() * 21.5, 1)
        free_icu = float(max(1, int(round((100.0 - occ_pct) * 0.45))))
        crit_q = float(2 + (d_idx % 9))
        d_utils = dept_device_utils[d_idx]
        dev_load = round(sum(d_utils) / len(d_utils), 2) if d_utils else 65.0
        strain = round(occ_pct * 0.6 + crit_q * 2.5 + dev_load * 0.2, 2)
        campus_dept_occ[c_owner].append(occ_pct)
        nodes.append(
            NodeRow(
                node_id=d_id,
                subgraph_slug=sub,
                type_name="ClinicalDept",
                node_key=d_key,
                title=d_title,
                properties_json=json.dumps(
                    {
                        "dept_name": d_title,
                        "specialty": spec_name,
                        "on_duty_physicians": phys_cnt,
                    },
                    ensure_ascii=False,
                ),
                metrics_json=json.dumps(
                    {
                        "bed_occupancy_pct": occ_pct,
                        "free_icu_beds": free_icu,
                        "critical_queue_cnt": crit_q,
                        "device_load_pct": dev_load,
                        "dept_strain_score": strain,
                    }
                ),
                geo_lng=None,
                geo_lat=None,
            )
        )
        links.append(
            LinkRow(
                link_id=_det_uuid(tenant_id, sub, f"HAS_DEPT:{campus_keys[c_owner]}->{d_key}"),
                subgraph_slug=sub,
                link_type="HAS_CLINICAL_DEPT",
                src_node_id=campus_ids[c_owner],
                dst_node_id=d_id,
                weight=0.85,
            )
        )

    for c_idx in range(12):
        c_key = campus_keys[c_idx]
        c_id = campus_ids[c_idx]
        c_name, district, c_lng, c_lat = campus_defs[c_idx]
        ed_wait = round(22.0 + (c_idx % 6) * 4.8 + rng.random() * 4.0, 1)
        free_beds = float(24 + (c_idx % 7) * 6)
        surge_lvl = 2.0 if ed_wait >= 38.0 else 1.0
        avg_occ = round(sum(campus_dept_occ[c_idx]) / len(campus_dept_occ[c_idx]), 2)
        sat_idx = round(
            max(0.0, min(100.0, ed_wait * 0.8 + avg_occ * 0.6 - min(30.0, free_beds * 0.3))),
            2,
        )
        nodes.append(
            NodeRow(
                node_id=c_id,
                subgraph_slug=sub,
                type_name="HospitalCampus",
                node_key=c_key,
                title=c_name,
                properties_json=json.dumps(
                    {
                        "campus_name": c_name,
                        "hospital_grade": "三级甲等",
                        "district": district,
                    },
                    ensure_ascii=False,
                ),
                metrics_json=json.dumps(
                    {
                        "ed_wait_minutes": ed_wait,
                        "total_free_beds": free_beds,
                        "surge_level": surge_lvl,
                        "avg_dept_occupancy_pct": avg_occ,
                        "campus_saturation_idx": sat_idx,
                    }
                ),
                geo_lng=c_lng,
                geo_lat=c_lat,
            )
        )

        # 1 WardCapacityCube per campus -> 12 total
        w_key = f"WCUBE-{c_idx + 1:02d}"
        w_id = _det_uuid(tenant_id, sub, w_key)
        w_label = f"2026-W20-{c_key}"
        nodes.append(
            NodeRow(
                node_id=w_id,
                subgraph_slug=sub,
                type_name="WardCapacityCube",
                node_key=w_key,
                title=w_label,
                properties_json=json.dumps({"stat_window": w_label}, ensure_ascii=False),
                metrics_json=json.dumps(
                    {
                        "daily_admissions": float(115 + c_idx * 9),
                        "turnover_rate": round(1.12 + (c_idx % 5) * 0.08, 2),
                    }
                ),
                geo_lng=None,
                geo_lat=None,
            )
        )
        links.append(
            LinkRow(
                link_id=_det_uuid(tenant_id, sub, f"SUMMARIZES_WARD:{c_key}->{w_key}"),
                subgraph_slug=sub,
                link_type="SUMMARIZES_WARD",
                src_node_id=c_id,
                dst_node_id=w_id,
                weight=0.65,
            )
        )

    # 3) 64 AmbulanceUnit with widespread 2D WGS-84 coordinates + 320 TRANSPORTS_TO_CAMPUS links
    center_lng, center_lat = 121.4650, 31.2350
    for a_idx in range(1, 65):
        a_key = f"AMB-{a_idx:02d}"
        a_id = _det_uuid(tenant_id, sub, a_key)
        amb_ids.append(a_id)
        amb_keys.append(a_key)
        plate = f"沪A-120{a_idx:02d}急救"
        angle = (2.0 * math.pi * (a_idx - 1)) / 64.0
        ring_scale = 0.12 + ((a_idx * 7) % 5) * 0.075
        jitter_lng = (rng.random() - 0.5) * 0.02
        jitter_lat = (rng.random() - 0.5) * 0.02
        a_lng = round(center_lng + math.cos(angle) * ring_scale * 1.25 + jitter_lng, 5)
        a_lat = round(center_lat + math.sin(angle) * ring_scale + jitter_lat, 5)
        eta = round(8.5 + rng.random() * 24.0, 1)
        d_load = float(1 + (a_idx % 3))
        nodes.append(
            NodeRow(
                node_id=a_id,
                subgraph_slug=sub,
                type_name="AmbulanceUnit",
                node_key=a_key,
                title=plate,
                properties_json=json.dumps(
                    {
                        "plate_no": plate,
                        "crew_level": crew_levels[(a_idx - 1) % len(crew_levels)],
                    },
                    ensure_ascii=False,
                ),
                metrics_json=json.dumps({"eta_minutes": eta, "dispatch_load": d_load}),
                geo_lng=a_lng,
                geo_lat=a_lat,
            )
        )
        for hop in range(5):
            c_target = (a_idx + hop * 2) % 12
            links.append(
                LinkRow(
                    link_id=_det_uuid(
                        tenant_id, sub, f"TRANSPORTS:{a_key}->{campus_keys[c_target]}"
                    ),
                    subgraph_slug=sub,
                    link_type="TRANSPORTS_TO_CAMPUS",
                    src_node_id=a_id,
                    dst_node_id=campus_ids[c_target],
                    weight=0.85,
                )
            )

    # 4) 20 TriageVisit -> total nodes = 12 + 60 + 96 + 64 + 20 + 12 = 264; total links = 508
    for tv_idx in range(1, 21):
        tv_key = f"TV-{tv_idx:04d}"
        tv_id = _det_uuid(tenant_id, sub, tv_key)
        visit_no = f"ED2605-{tv_idx:04d}"
        d_target = (tv_idx - 1) % 60
        acuity = acuity_levels[(tv_idx - 1) % len(acuity_levels)]
        comp = complaints[(tv_idx - 1) % len(complaints)]
        d2d = round(3.0 + (tv_idx % 6) * 2.4, 1)
        nodes.append(
            NodeRow(
                node_id=tv_id,
                subgraph_slug=sub,
                type_name="TriageVisit",
                node_key=tv_key,
                title=visit_no,
                properties_json=json.dumps(
                    {"visit_no": visit_no, "acuity_level": acuity, "chief_complaint": comp},
                    ensure_ascii=False,
                ),
                metrics_json=json.dumps({"door_to_doctor_min": d2d}),
                geo_lng=None,
                geo_lat=None,
            )
        )
        links.append(
            LinkRow(
                link_id=_det_uuid(
                    tenant_id, sub, f"RECORDS_TRIAGE:{dept_keys[d_target]}->{tv_key}"
                ),
                subgraph_slug=sub,
                link_type="RECORDS_TRIAGE",
                src_node_id=dept_ids[d_target],
                dst_node_id=tv_id,
                weight=0.7,
            )
        )

    # Anchor nodes in `main` (geo=None so MapWidget & convex-hull never double-count)
    main_nodes: list[NodeRow] = [
        NodeRow(
            node_id=_det_uuid(tenant_id, "main", "HospitalCampus:HOSP-01"),
            subgraph_slug="main",
            type_name="HospitalCampus",
            node_key="HOSP-01",
            title="市中心第一三甲总院",
            properties_json=json.dumps(
                {
                    "campus_name": "市中心第一三甲总院",
                    "hospital_grade": "三级甲等",
                    "district": "黄浦中心区",
                },
                ensure_ascii=False,
            ),
            metrics_json=json.dumps(
                {
                    "ed_wait_minutes": 34.5,
                    "total_free_beds": 38.0,
                    "surge_level": 1.0,
                    "avg_dept_occupancy_pct": 88.5,
                    "campus_saturation_idx": 69.3,
                }
            ),
            geo_lng=None,
            geo_lat=None,
        ),
        NodeRow(
            node_id=_det_uuid(tenant_id, "main", "ClinicalDept:DEPT-01"),
            subgraph_slug="main",
            type_name="ClinicalDept",
            node_key="DEPT-01",
            title="市中心第一三甲总院·急诊抢救",
            properties_json=json.dumps(
                {
                    "dept_name": "市中心第一三甲总院·急诊抢救",
                    "specialty": "急诊抢救",
                    "on_duty_physicians": 12,
                },
                ensure_ascii=False,
            ),
            metrics_json=json.dumps(
                {
                    "bed_occupancy_pct": 88.5,
                    "free_icu_beds": 5.0,
                    "critical_queue_cnt": 9.0,
                    "device_load_pct": 86.0,
                    "dept_strain_score": 92.8,
                }
            ),
            geo_lng=None,
            geo_lat=None,
        ),
        NodeRow(
            node_id=_det_uuid(tenant_id, "main", "LifeSupportUnit:LSU-001"),
            subgraph_slug="main",
            type_name="LifeSupportUnit",
            node_key="LSU-001",
            title="ECMO-001",
            properties_json=json.dumps(
                {"device_code": "ECMO-001", "device_kind": "ECMO", "status": "IN_USE"},
                ensure_ascii=False,
            ),
            metrics_json=json.dumps({"utilization_pct": 86.0, "remaining_filter_hours": 28.5}),
            geo_lng=None,
            geo_lat=None,
        ),
        NodeRow(
            node_id=_det_uuid(tenant_id, "main", "AmbulanceUnit:AMB-01"),
            subgraph_slug="main",
            type_name="AmbulanceUnit",
            node_key="AMB-01",
            title="沪A-12001急救",
            properties_json=json.dumps(
                {"plate_no": "沪A-12001急救", "crew_level": "重症移动ICU"},
                ensure_ascii=False,
            ),
            metrics_json=json.dumps({"eta_minutes": 16.0, "dispatch_load": 2.0}),
            geo_lng=None,
            geo_lat=None,
        ),
    ]

    # 18,000-row DuckLake table `med_triage_events`
    lake_rows = 18_000
    now_utc = datetime.now(UTC).replace(microsecond=0)
    base_ts = now_utc - timedelta(hours=24)
    ev_ids_col = [f"MED-EV-{idx:06d}" for idx in range(1, lake_rows + 1)]
    c_key_col = [campus_keys[(idx - 1) % 12] for idx in range(1, lake_rows + 1)]
    d_key_col = [dept_keys[(idx - 1) % 60] for idx in range(1, lake_rows + 1)]
    a_key_col = [amb_keys[(idx - 1) % 64] for idx in range(1, lake_rows + 1)]
    acuity_col = [acuity_levels[(idx - 1) % len(acuity_levels)] for idx in range(1, lake_rows + 1)]
    comp_col = [complaints[(idx - 1) % len(complaints)] for idx in range(1, lake_rows + 1)]
    eta_col = [round(7.0 + ((idx * 17) % 280) * 0.1, 1) for idx in range(1, lake_rows + 1)]
    d2d_col = [round(2.5 + ((idx * 13) % 190) * 0.1, 1) for idx in range(1, lake_rows + 1)]
    bed_ok_col = [idx % 9 != 0 for idx in range(1, lake_rows + 1)]
    triaged_col = [
        base_ts + timedelta(seconds=(idx * 86400) // lake_rows) for idx in range(lake_rows)
    ]

    med_arrow = pa.Table.from_pydict(
        {
            "event_id": pa.array(ev_ids_col, type=pa.string()),
            "campus_key": pa.array(c_key_col, type=pa.string()),
            "dept_key": pa.array(d_key_col, type=pa.string()),
            "ambulance_key": pa.array(a_key_col, type=pa.string()),
            "acuity_level": pa.array(acuity_col, type=pa.string()),
            "chief_complaint": pa.array(comp_col, type=pa.string()),
            "eta_minutes": pa.array(eta_col, type=pa.float64()),
            "door_to_doctor_min": pa.array(d2d_col, type=pa.float64()),
            "bed_assigned": pa.array(bed_ok_col, type=pa.bool_()),
            "triaged_at": pa.array(triaged_col, type=pa.timestamp("us", tz="UTC")),
        }
    )
    lake_batch_id = _det_uuid(tenant_id, "lake_batch", "med_triage_events")
    snap_id = _write_arrow_to_ducklake(
        tenant_id=tenant_id,
        table_name="med_triage_events",
        arrow_tbl=med_arrow,
        batch_id=lake_batch_id,
    )

    med_cols = [
        ColumnMetaSpec("event_id", "STRING", "急诊分诊转运流水号", "分诊事件编号"),
        ColumnMetaSpec("campus_key", "STRING", "目标接诊三甲院区编码", "院区编码"),
        ColumnMetaSpec("dept_key", "STRING", "接诊急诊重症科室编码", "科室编码"),
        ColumnMetaSpec("ambulance_key", "STRING", "承运120急救车编码", "急救车编码"),
        ColumnMetaSpec("acuity_level", "STRING", "四级急诊预检分诊等级", "危重分级"),
        ColumnMetaSpec("chief_complaint", "STRING", "入院主诉症候群", "主诉症候群"),
        ColumnMetaSpec("eta_minutes", "FLOAT64", "院前转运预计抵院分钟", "预计抵院", "分钟"),
        ColumnMetaSpec("door_to_doctor_min", "FLOAT64", "抵院至首诊分钟", "首诊响应时间", "分钟"),
        ColumnMetaSpec("bed_assigned", "BOOLEAN", "是否已落实监护或留观床位", "床位落实标志"),
        ColumnMetaSpec("triaged_at", "TIMESTAMP", "预检分诊登记UTC时间戳", "分诊时间"),
    ]

    with conn.cursor() as cur:
        cur.execute("SELECT set_config('app.tenant_id', %s, true)", (tenant_id,))
        ds_id = _upsert_lake_dataset_metadata(
            cur,
            tenant_id=tenant_id,
            dataset_name="med_triage_events",
            display_name="城市急诊分诊与院前转运明细",
            description=(
                "全市12个三甲院区急诊预检分诊、120急救车转运ETA与首诊响应明细表（18,000 行）"
            ),
            columns=med_cols,
            row_count=lake_rows,
            snapshot_id=snap_id,
            tags=["showcase", "med_dispatch", "triage"],
        )

        _batch_replace_subgraph_topology(
            cur,
            tenant_id=tenant_id,
            subgraph_slug=sub,
            nodes=nodes,
            links=links,
        )
        _batch_upsert_nodes_and_links(cur, tenant_id=tenant_id, nodes=main_nodes, links=[])
        main_id_map = _lookup_node_id_map(
            cur,
            tenant_id=tenant_id,
            subgraph_slug="main",
            keys=[(n.type_name, n.node_key) for n in main_nodes],
        )
        m_hosp1 = main_id_map[("HospitalCampus", "HOSP-01")]
        m_dept1 = main_id_map[("ClinicalDept", "DEPT-01")]
        m_lsu1 = main_id_map[("LifeSupportUnit", "LSU-001")]
        m_amb1 = main_id_map[("AmbulanceUnit", "AMB-01")]

        cur.execute(
            """
            DELETE FROM xb_links
            WHERE tenant_id = %s AND subgraph_slug = 'main'
              AND link_type IN ('TRANSPORTS_TO_CAMPUS', 'HAS_CLINICAL_DEPT', 'OPERATES_DEVICE')
            """,
            (tenant_id,),
        )
        main_links = [
            LinkRow(
                _det_uuid(tenant_id, "main", "L:AMB1->HOSP1"),
                "main",
                "TRANSPORTS_TO_CAMPUS",
                m_amb1,
                m_hosp1,
                0.85,
            ),
            LinkRow(
                _det_uuid(tenant_id, "main", "L:HOSP1->DEPT1"),
                "main",
                "HAS_CLINICAL_DEPT",
                m_hosp1,
                m_dept1,
                0.85,
            ),
            LinkRow(
                _det_uuid(tenant_id, "main", "L:DEPT1->LSU1"),
                "main",
                "OPERATES_DEVICE",
                m_dept1,
                m_lsu1,
                0.75,
            ),
        ]
        _batch_upsert_nodes_and_links(cur, tenant_id=tenant_id, nodes=[], links=main_links)

        # Build exact 20,000 time-series points over trailing 24 hours
        series_pts: list[MetricPointRow] = []
        # 59 ClinicalDept nodes in med_dispatch * 330 points = 19,470
        for d_idx in range(59):
            did = dept_ids[d_idx]
            for step in range(330):
                ts = now_utc - timedelta(minutes=step * 4)
                wave = math.sin((step + d_idx) * 0.18)
                occ = round(84.5 + wave * 8.2, 2)
                series_pts.append(MetricPointRow(did, "bed_occupancy_pct", ts, occ, "SYNC"))
        # 380 points on main DEPT-01 + 150 points on main HOSP-01 & AMB-01 -> total 20,000 exact
        for step in range(380):
            ts = now_utc - timedelta(minutes=step * 3)
            wave = math.sin(step * 0.15)
            occ_m = round(86.8 + wave * 6.4, 2)
            series_pts.append(MetricPointRow(m_dept1, "bed_occupancy_pct", ts, occ_m, "SYNC"))
        for step in range(50):
            ts = now_utc - timedelta(minutes=step * 28)
            wait_m = round(32.0 + (step % 6) * 1.4, 1)
            eta_m = round(14.5 + (step % 5) * 1.2, 1)
            series_pts.append(
                MetricPointRow(m_hosp1, "total_free_beds", ts, float(36 + (step % 7)), "SYNC")
            )
            series_pts.append(MetricPointRow(m_hosp1, "ed_wait_minutes", ts, wait_m, "SYNC"))
            series_pts.append(MetricPointRow(m_amb1, "eta_minutes", ts, eta_m, "SYNC"))

        series_written = _batch_replace_metric_series(
            cur,
            tenant_id=tenant_id,
            batch_id=_det_uuid(tenant_id, "series_batch", sub),
            points=series_pts,
        )

        _seed_rules_and_alerts(
            cur,
            tenant_id=tenant_id,
            specs=[
                AlertSeedSpec(
                    rule_name="med_campus_saturation_critical",
                    description="医疗院区急诊饱和指数突破80分触发红色扩容与救护车分流预警",
                    type_name="HospitalCampus",
                    metric="campus_saturation_idx",
                    op="GT",
                    threshold=80.0,
                    severity="CRITICAL",
                    title_template="院区急诊饱和红线告警：{{node.title}}",
                    body_template=(
                        "院区 {{node.title}} 饱和指数 {{value}} 分超出安全阈值 {{threshold}} 分。"
                    ),
                    alert_node_id=campus_ids[2],
                    alert_title="院区急诊饱和红线告警：徐汇重症医学临床总院",
                    alert_body=(
                        "夜间急诊危重接诊高峰，饱和指数达 84.6 分，建议立即启用应急机动病区加床。"
                    ),
                    observed_value=84.6,
                ),
                AlertSeedSpec(
                    rule_name="med_dept_strain_high",
                    description="急诊与重症科室救治承压指数超限预警",
                    type_name="ClinicalDept",
                    metric="dept_strain_score",
                    op="GT",
                    threshold=82.0,
                    severity="WARN",
                    title_template="重症科室高负荷预警：{{node.title}}",
                    body_template=(
                        "科室 {{node.title}} 救治承压指数 {{value}} 分高于警戒线 {{threshold}} 分。"
                    ),
                    alert_node_id=dept_ids[0],
                    alert_title="重症科室高负荷预警：市中心第一三甲总院·急诊抢救",
                    alert_body="抢救室床位占用率 88.5%，危重待收治 9 人，承压指数 92.8 分。",
                    observed_value=92.8,
                ),
            ],
        )
    conn.commit()

    return {
        "world": sub,
        "dataset_id": str(ds_id),
        "lake_rows": lake_rows,
        "nodes": len(nodes),
        "links": len(links),
        "series_points": series_written,
        "main_anchors": {
            "hosp01_id": str(m_hosp1),
            "dept01_id": str(m_dept1),
            "lsu01_id": str(m_lsu1),
            "amb01_id": str(m_amb1),
        },
        "elapsed_s": round(time.perf_counter() - t0, 3),
    }


def _resolve_db_url(explicit: str | None) -> str:
    for candidate in (
        explicit,
        os.getenv("MIGRATION_DATABASE_URL"),
        os.getenv("DATABASE_URL"),
    ):
        if candidate and candidate.strip():
            return candidate.strip()
    raise RuntimeError("MIGRATION_DATABASE_URL or DATABASE_URL is required")


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Deterministic vectorized seeder for semi_supply and med_dispatch worlds."
    )
    parser.add_argument(
        "--world",
        choices=("semi_supply", "med_dispatch", "all"),
        default="all",
        help="Target industry world to materialize.",
    )
    parser.add_argument("--tenant", default="default", help="Target tenant_id.")
    parser.add_argument("--db-url", default=None, help="PostgreSQL connection URI override.")
    args = parser.parse_args()

    valid_tenant = validate_tenant_id(str(args.tenant))
    db_url = _resolve_db_url(args.db_url)

    summaries: list[dict[str, Any]] = []
    with psycopg.connect(db_url) as conn:
        if args.world in ("semi_supply", "all"):
            summaries.append(seed_semi_supply(conn, valid_tenant))
        if args.world in ("med_dispatch", "all"):
            summaries.append(seed_med_dispatch(conn, valid_tenant))

    output: dict[str, Any] = summaries[0] if len(summaries) == 1 else {"worlds": summaries}
    print(json.dumps(output, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
