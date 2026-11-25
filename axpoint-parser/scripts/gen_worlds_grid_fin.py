#!/usr/bin/env python3
"""Deterministic vectorized showcase world generator for Step 33 (`power_grid` & `fin_risk`).

Generates and bulk-loads in < 1s per world:
1. `power_grid` (seed = 202604):
   - 324 business nodes (18 PowerPlant, 26 Substation with closed GeoJSON Polygon geofences,
     160 DistTransformer, 90 CriticalLoad inside corresponding Substation polygons,
     20 SwitchOrder, 10 GridZoneSummary)
   - 552 relationship links (with `FEEDS_TRANSFORMER` edge metrics)
   - 43,200 15-minute node telemetry points (plus 15,360 link telemetry points) across recent 24h
   - DuckLake table `grid_feeder_telemetry` (43,200 rows) + `ds_datasets` / `ds_column_meta`
2. `fin_risk` (seed = 202604):
   - 1,210 business nodes (40 CorpGroup, 620 LegalEntity across 6 branches,
     180 BeneficialOwner with compliant PII fields, 350 CreditFacility, 20 IndustryExposureCube)
   - 4,280 relationship links (including 25 closed `CROSS_GUARANTEES` rings)
   - 9,600 24h node series points
   - DuckLake table `fin_corp_wire_transfers` (60,000 rows) + `ds_datasets` / `ds_column_meta`
"""
from __future__ import annotations

import argparse
import concurrent.futures
import json
import os
import random
import re
import sys
import time
import uuid
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Literal

PARSER_ROOT = Path(__file__).resolve().parent.parent
if str(PARSER_ROOT) not in sys.path:
    sys.path.insert(0, str(PARSER_ROOT))

import psycopg  # noqa: E402

from lake.catalog import (  # noqa: E402
    lake_connection,
    quote_lake_table,
    tenant_schema,
    tenant_schema_name,
)

WORLD_SEED = 202604
WORLD_NAMESPACE = uuid.UUID("6ba7b810-9dad-11d1-80b4-00c04fd430c8")
STRICT_IDENT_RE = re.compile(r"^[a-z][a-z0-9_]{0,62}$")

WorldKind = Literal["power_grid", "fin_risk", "all"]


@dataclass(frozen=True)
class ColumnMetaSpec:
    """Metadata specification for a registered DuckLake dataset column."""

    name: str
    data_type: str
    pii_category: str
    description: str
    unit: str


@dataclass(frozen=True)
class NodeRow:
    """In-memory business node prepared for bulk UNNEST insertion."""

    node_id: str
    type_name: str
    node_key: str
    title: str
    properties_json: str
    metrics_json: str
    geo_lng: float | None
    geo_lat: float | None
    geo_shape_json: str | None


@dataclass(frozen=True)
class LinkRow:
    """In-memory relationship link prepared for bulk UNNEST insertion."""

    link_id: str
    link_type: str
    src_node_id: str
    dst_node_id: str
    weight: float
    properties_json: str
    metrics_json: str


def det_uuid(tenant_id: str, scope: str, key: str) -> str:
    """Compute deterministic UUIDv5 string scoped to `(tenant_id, scope, key)`."""
    return str(uuid.uuid5(WORLD_NAMESPACE, f"{tenant_id}:{scope}:{key}"))


def validate_ident(raw: str) -> str:
    """Validate SQL/tenant/dataset identifier against strict lowercase ASCII whitelist."""
    cleaned = raw.strip()
    if not STRICT_IDENT_RE.match(cleaned):
        raise ValueError(f"invalid identifier: {cleaned!r}")
    return cleaned


def make_closed_polygon(cx: float, cy: float, dx: float, dy: float) -> dict[str, Any]:
    """Build a valid counter-clockwise closed GeoJSON Polygon around `(cx, cy)`."""
    x0 = round(cx - dx, 6)
    x1 = round(cx + dx, 6)
    y0 = round(cy - dy, 6)
    y1 = round(cy + dy, 6)
    ring: list[list[float]] = [
        [x0, y0],
        [x1, y0],
        [x1, y1],
        [x0, y1],
        [x0, y0],
    ]
    return {"type": "Polygon", "coordinates": [ring]}


def make_cn_id_card(area6: str, birth8: str, seq3: int) -> str:
    """Generate an 18-character synthetic ID number with valid ISO 7064 Mod 11-2 checksum."""
    body17 = f"{area6}{birth8}{seq3:03d}"
    weights = [7, 9, 10, 5, 8, 4, 2, 1, 6, 3, 7, 9, 10, 5, 8, 4, 2]
    check_chars = "10X98765432"
    total = sum(int(ch) * w for ch, w in zip(body17, weights, strict=True))
    return f"{body17}{check_chars[total % 11]}"


def make_luhn_card(prefix6: str, body12: int) -> str:
    """Generate a 19-digit synthetic bank card number passing Luhn check."""
    stem18 = f"{prefix6}{body12:012d}"
    total = 0
    for idx, ch in enumerate(reversed(stem18)):
        d = int(ch)
        if idx % 2 == 0:
            d *= 2
            if d > 9:
                d -= 9
        total += d
    check_digit = (10 - (total % 10)) % 10
    return f"{stem18}{check_digit}"


def execute_ducklake_ctas(
    *,
    tenant_id: str,
    table_name: str,
    select_sql: str,
    data_root: Path | None,
    catalog_dsn: str | None,
) -> tuple[int, int]:
    """Vectorized DuckDB CTAS into DuckLake returning `(row_count, snapshot_id)`."""
    valid_tenant = validate_ident(tenant_id)
    valid_table = validate_ident(table_name)
    s_q = tenant_schema(valid_tenant)
    t_q = quote_lake_table(valid_table)
    full_tbl = f"lake.{s_q}.{t_q}"

    with lake_connection(data_root=data_root, catalog_dsn=catalog_dsn) as con:
        con.execute(f"CREATE SCHEMA IF NOT EXISTS lake.{s_q}")
        con.execute(f"DROP TABLE IF EXISTS {full_tbl}")
        con.execute(f"CREATE TABLE {full_tbl} AS {select_sql}")
        cnt_row = con.execute(f"SELECT COUNT(*) FROM {full_tbl}").fetchone()
        snap_row = con.execute("SELECT * FROM ducklake_current_snapshot('lake')").fetchone()
        rows_written = int(cnt_row[0]) if cnt_row and cnt_row[0] is not None else 0
        snap_id = int(snap_row[0]) if snap_row and snap_row[0] is not None else 1
        return rows_written, snap_id


def upsert_dataset_catalog_pg(
    cur: psycopg.Cursor[tuple[Any, ...]],
    *,
    tenant_id: str,
    dataset_id: str,
    dataset_name: str,
    display_name: str,
    description: str,
    columns: list[ColumnMetaSpec],
    row_count: int,
    snapshot_id: int,
) -> str:
    """Idempotently register DuckLake dataset and column metadata in PostgreSQL."""
    lake_schema_str = tenant_schema_name(tenant_id)
    schema_profiles = [
        {
            "name": col.name,
            "ordinal": idx,
            "data_type": col.data_type,
            "nullable": False,
            "pii_category": col.pii_category,
        }
        for idx, col in enumerate(columns)
    ]
    source_ref_json = json.dumps({"generator": "gen_worlds_grid_fin", "seed": WORLD_SEED})
    schema_json_str = json.dumps(schema_profiles, ensure_ascii=False)

    cur.execute(
        """
        INSERT INTO ds_datasets (
            id, tenant_id, name, display_name, description,
            source_kind, source_ref, sync_mode, schema_policy,
            lake_schema, lake_table, schema_json, schema_version,
            row_count, last_snapshot_id, last_synced_at, tags, status, updated_at
        )
        VALUES (
            %s::uuid, %s, %s, %s, %s,
            'FILE_TABLE', %s::jsonb, 'SNAPSHOT', 'ADDITIVE',
            %s, %s, %s::jsonb, 1,
            %s, %s, NOW(), ARRAY['showcase', 'step33'], 'READY', NOW()
        )
        ON CONFLICT (tenant_id, name) DO UPDATE SET
            display_name = EXCLUDED.display_name,
            description = EXCLUDED.description,
            source_ref = EXCLUDED.source_ref,
            lake_schema = EXCLUDED.lake_schema,
            lake_table = EXCLUDED.lake_table,
            schema_json = EXCLUDED.schema_json,
            schema_version = GREATEST(ds_datasets.schema_version, 1),
            row_count = EXCLUDED.row_count,
            last_snapshot_id = EXCLUDED.last_snapshot_id,
            last_synced_at = NOW(),
            status = 'READY',
            updated_at = NOW()
        RETURNING id::text
        """,
        (
            dataset_id,
            tenant_id,
            dataset_name,
            display_name,
            description,
            source_ref_json,
            lake_schema_str,
            dataset_name,
            schema_json_str,
            row_count,
            snapshot_id,
        ),
    )
    fetched = cur.fetchone()
    eff_ds_id = str(fetched[0]) if fetched else dataset_id

    cur.execute(
        "DELETE FROM ds_column_meta WHERE tenant_id = %s AND dataset_id = %s::uuid",
        (tenant_id, eff_ds_id),
    )
    cur.execute(
        """
        INSERT INTO ds_column_meta (
            tenant_id, dataset_id, column_name, data_type,
            pii_category, pii_confirmed, description, unit, ordinal
        )
        SELECT %s, %s::uuid, u.col_name, u.col_type,
               u.col_pii, u.col_conf, u.col_desc, u.col_unit, u.col_ord
          FROM UNNEST(
              %s::text[], %s::text[], %s::text[], %s::bool[],
              %s::text[], %s::text[], %s::int4[]
          ) AS u(col_name, col_type, col_pii, col_conf, col_desc, col_unit, col_ord)
        """,
        (
            tenant_id,
            eff_ds_id,
            [c.name for c in columns],
            [c.data_type for c in columns],
            [c.pii_category for c in columns],
            [c.pii_category != "NONE" for c in columns],
            [c.description for c in columns],
            [c.unit for c in columns],
            list(range(len(columns))),
        ),
    )
    return eff_ds_id


def bulk_replace_subgraph_graph_pg(
    cur: psycopg.Cursor[tuple[Any, ...]],
    *,
    tenant_id: str,
    subgraph_slug: str,
    nodes: list[NodeRow],
    links: list[LinkRow],
) -> None:
    """Bulk upsert nodes and links for `subgraph_slug` via PostgreSQL UNNEST."""
    keep_node_ids = [n.node_id for n in nodes]
    cur.execute(
        """
        DELETE FROM xb_nodes
         WHERE tenant_id = %s
           AND subgraph_slug = %s
           AND NOT (id = ANY(%s::uuid[]))
        """,
        (tenant_id, subgraph_slug, keep_node_ids),
    )
    cur.execute(
        """
        INSERT INTO xb_nodes (
            id, tenant_id, subgraph_slug, type_name, node_key, title,
            properties, metrics, geo_lng, geo_lat, geo_shape, updated_at
        )
        SELECT u.id, %s, %s, u.type_name, u.node_key, u.title,
               u.props::jsonb, u.metrics::jsonb, u.lng, u.lat, u.shape::jsonb, NOW()
          FROM UNNEST(
              %s::uuid[], %s::text[], %s::text[], %s::text[],
              %s::text[], %s::text[], %s::float8[], %s::float8[], %s::text[]
          ) AS u(id, type_name, node_key, title, props, metrics, lng, lat, shape)
        ON CONFLICT (tenant_id, subgraph_slug, type_name, node_key) DO UPDATE SET
            title = EXCLUDED.title,
            properties = EXCLUDED.properties,
            metrics = EXCLUDED.metrics,
            geo_lng = EXCLUDED.geo_lng,
            geo_lat = EXCLUDED.geo_lat,
            geo_shape = EXCLUDED.geo_shape,
            updated_at = NOW()
        WHERE xb_nodes.title IS DISTINCT FROM EXCLUDED.title
           OR xb_nodes.metrics IS DISTINCT FROM EXCLUDED.metrics
           OR xb_nodes.properties IS DISTINCT FROM EXCLUDED.properties
           OR xb_nodes.geo_shape IS DISTINCT FROM EXCLUDED.geo_shape
        """,
        (
            tenant_id,
            subgraph_slug,
            [n.node_id for n in nodes],
            [n.type_name for n in nodes],
            [n.node_key for n in nodes],
            [n.title for n in nodes],
            [n.properties_json for n in nodes],
            [n.metrics_json for n in nodes],
            [n.geo_lng for n in nodes],
            [n.geo_lat for n in nodes],
            [n.geo_shape_json for n in nodes],
        ),
    )

    keep_link_ids = [lk.link_id for lk in links]
    cur.execute(
        """
        DELETE FROM xb_links
         WHERE tenant_id = %s
           AND subgraph_slug = %s
           AND NOT (id = ANY(%s::uuid[]))
        """,
        (tenant_id, subgraph_slug, keep_link_ids),
    )
    cur.execute(
        """
        INSERT INTO xb_links (
            id, tenant_id, subgraph_slug, link_type,
            src_node_id, dst_node_id, weight, properties, metrics, updated_at
        )
        SELECT u.id, %s, %s, u.link_type,
               u.src_id, u.dst_id, u.weight, u.props::jsonb, u.metrics::jsonb, NOW()
          FROM UNNEST(
              %s::uuid[], %s::text[], %s::uuid[], %s::uuid[],
              %s::float8[], %s::text[], %s::text[]
          ) AS u(id, link_type, src_id, dst_id, weight, props, metrics)
        ON CONFLICT (tenant_id, subgraph_slug, link_type, src_node_id, dst_node_id) DO UPDATE SET
            weight = EXCLUDED.weight,
            properties = EXCLUDED.properties,
            metrics = EXCLUDED.metrics,
            updated_at = NOW()
        WHERE xb_links.metrics IS DISTINCT FROM EXCLUDED.metrics
           OR xb_links.properties IS DISTINCT FROM EXCLUDED.properties
           OR xb_links.weight IS DISTINCT FROM EXCLUDED.weight
        """,
        (
            tenant_id,
            subgraph_slug,
            [lk.link_id for lk in links],
            [lk.link_type for lk in links],
            [lk.src_node_id for lk in links],
            [lk.dst_node_id for lk in links],
            [lk.weight for lk in links],
            [lk.properties_json for lk in links],
            [lk.metrics_json for lk in links],
        ),
    )


def cleanup_orphan_main_edges(
    cur: psycopg.Cursor[tuple[Any, ...]],
    *,
    tenant_id: str,
) -> None:
    """Remove any legacy orphan edge types in `main` missing `src`/`dst` node types."""
    cur.execute(
        """
        DELETE FROM xb_edge_types et
         WHERE et.tenant_id = %s
           AND et.subgraph_slug = 'main'
           AND (
               NOT EXISTS (
                   SELECT 1
                     FROM xb_node_types nt
                    WHERE nt.tenant_id = et.tenant_id
                      AND nt.subgraph_slug = 'main'
                      AND nt.name = et.src_type_name
               )
               OR NOT EXISTS (
                   SELECT 1
                     FROM xb_node_types nt
                    WHERE nt.tenant_id = et.tenant_id
                      AND nt.subgraph_slug = 'main'
                      AND nt.name = et.dst_type_name
               )
           )
        """,
        (tenant_id,),
    )


def generate_series_and_rollups_in_pg(
    cur: psycopg.Cursor[tuple[Any, ...]],
    *,
    tenant_id: str,
    all_node_ids: list[str],
    streams: list[tuple[str, str, float]],
) -> int:
    """Vectorized in-PostgreSQL `generate_series(0, 95)` 15-min series & `HOUR` rollups."""
    cur.execute("SELECT ax_ensure_metric_partitions(2, 2)")
    cur.execute(
        "DELETE FROM xb_metric_series WHERE tenant_id = %s AND node_id = ANY(%s::uuid[])",
        (tenant_id, all_node_ids),
    )
    cur.execute(
        "DELETE FROM xb_metric_rollups WHERE tenant_id = %s AND node_id = ANY(%s::uuid[])",
        (tenant_id, all_node_ids),
    )
    if not streams:
        return 0

    cur.execute(
        """
        WITH anchor AS (
            SELECT date_trunc('hour', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'
                   + (floor(extract(minute FROM now()) / 15.0) * interval '15 minutes') AS t_end
        ),
        stream_defs AS (
            SELECT u.nid, u.mkey, u.base_v, u.ord
              FROM UNNEST(%s::uuid[], %s::text[], %s::float8[], %s::int4[])
                   AS u(nid, mkey, base_v, ord)
        )
        INSERT INTO xb_metric_series (tenant_id, node_id, metric, ts, value, source_tag)
        SELECT %s,
               s.nid,
               s.mkey,
               a.t_end - ((95 - g.step) * interval '15 minutes') AS ts,
               CASE
                   WHEN s.mkey = 'bus_voltage_pu' THEN
                       round(
                           (
                               1.015
                               - 0.035 * (0.78 + 0.22 * sin((g.step - 24) * pi() / 48.0))
                           )::numeric,
                           4
                       )::float8
                   ELSE
                       round(
                           (
                               s.base_v
                               * (
                                   0.78
                                   + 0.22 * sin((g.step - 24) * pi() / 48.0)
                                   + (s.ord %% 10) * 0.01
                               )
                           )::numeric,
                           3
                       )::float8
               END AS val,
               'SYNC'
          FROM stream_defs s
          CROSS JOIN generate_series(0, 95) AS g(step)
          CROSS JOIN anchor a
        """,
        (
            [st[0] for st in streams],
            [st[1] for st in streams],
            [st[2] for st in streams],
            list(range(len(streams))),
            tenant_id,
        ),
    )
    written = len(streams) * 96

    cur.execute(
        """
        INSERT INTO xb_metric_rollups (
            tenant_id, node_id, metric, grain, bucket_start,
            cnt, sum_v, min_v, max_v, last_v, last_ts
        )
        SELECT tenant_id,
               node_id,
               metric,
               'HOUR',
               date_trunc('hour', ts),
               COUNT(*),
               SUM(value),
               MIN(value),
               MAX(value),
               (array_agg(value ORDER BY ts DESC))[1],
               MAX(ts)
          FROM xb_metric_series
         WHERE tenant_id = %s
           AND node_id = ANY(%s::uuid[])
         GROUP BY tenant_id, node_id, metric, date_trunc('hour', ts)
        ON CONFLICT (tenant_id, node_id, metric, grain, bucket_start) DO UPDATE SET
            cnt = EXCLUDED.cnt,
            sum_v = EXCLUDED.sum_v,
            min_v = EXCLUDED.min_v,
            max_v = EXCLUDED.max_v,
            last_v = EXCLUDED.last_v,
            last_ts = EXCLUDED.last_ts
        """,
        (tenant_id, all_node_ids),
    )
    return written


def seed_power_grid_world(
    conn: psycopg.Connection[tuple[Any, ...]],
    *,
    tenant_id: str,
    data_root: Path | None,
    catalog_dsn: str | None,
) -> dict[str, Any]:
    """Generate and load `power_grid` (324 nodes, 552 links, 43,200 series & lake rows)."""
    t0 = time.perf_counter()
    rng = random.Random(WORLD_SEED)
    sub_slug = "power_grid"

    zone_names = [
        "城北分区",
        "钱塘分区",
        "高新分区",
        "临港分区",
        "城西分区",
        "滨江分区",
        "萧山分区",
        "余杭分区",
        "富阳分区",
        "临平分区",
    ]
    plant_kinds = ["火电", "储能", "风电", "光伏"]
    load_cats = ["三甲医院", "晶圆厂", "交通枢纽", "商业综合体"]

    nodes: list[NodeRow] = []
    links: list[LinkRow] = []

    # 1. 18 PowerPlants
    plant_ids: list[str] = []
    for i in range(1, 19):
        key = f"PP-{i:03d}"
        nid = det_uuid(tenant_id, sub_slug, key)
        plant_ids.append(nid)
        if i == 1:
            p_name, p_kind, rated, gen, res = "城北燃机热电厂", "火电", 300.0, 210.0, 65.0
            lng, lat = 120.125, 30.345
        elif i == 2:
            p_name, p_kind, rated, gen, res = "城北独立储能电站", "储能", 150.0, 80.0, 70.0
            lng, lat = 120.165, 30.335
        else:
            p_kind = plant_kinds[(i - 1) % len(plant_kinds)]
            p_name = f"{zone_names[(i - 1) % 10]}{i:02d}号{p_kind}场站"
            rated = round(rng.uniform(80.0, 320.0), 1)
            gen = round(rated * rng.uniform(0.55, 0.82), 2)
            res = round(rated - gen, 2)
            lng = round(119.95 + (i % 6) * 0.08 + rng.uniform(-0.01, 0.01), 6)
            lat = round(30.10 + (i // 6) * 0.09 + rng.uniform(-0.01, 0.01), 6)
        nodes.append(
            NodeRow(
                node_id=nid,
                type_name="PowerPlant",
                node_key=key,
                title=p_name,
                properties_json=json.dumps(
                    {"plant_name": p_name, "plant_kind": p_kind, "rated_mw": rated},
                    ensure_ascii=False,
                ),
                metrics_json=json.dumps(
                    {"generation_mw": gen, "spinning_reserve_mw": res},
                ),
                geo_lng=lng,
                geo_lat=lat,
                geo_shape_json=None,
            )
        )

    # 2. 10 GridZoneSummary
    zone_ids = [det_uuid(tenant_id, sub_slug, f"GZ-{z_idx + 1:03d}") for z_idx in range(10)]
    zone_loads: list[float] = [0.0] * 10

    # 3. 26 Substations (each with closed valid GeoJSON Polygon geofence)
    sub_ids: list[str] = []
    sub_centers: list[tuple[float, float]] = []
    sub_zones: list[int] = []
    sub_loads: list[float] = []
    sub_ratios: list[float] = []
    sub01_poly = make_closed_polygon(120.150, 30.320, 0.024, 0.020)

    for s_idx in range(1, 27):
        key = f"SUB-{s_idx:03d}"
        sid = det_uuid(tenant_id, sub_slug, key)
        sub_ids.append(sid)
        z_i = 0 if s_idx == 1 else (s_idx - 1) % 10
        sub_zones.append(z_i)
        if s_idx == 1:
            cx, cy = 120.150, 30.320
            poly = sub01_poly
            v_kv = "220"
            s_name = "220kV城北变电站"
            rated_cap = 240.0
            act_load = 210.0
            bus_v = 0.990
        else:
            col_i = (s_idx - 1) % 6
            row_i = (s_idx - 1) // 6
            cx = round(119.96 + col_i * 0.085, 6)
            cy = round(30.08 + row_i * 0.075, 6)
            poly = make_closed_polygon(cx, cy, 0.022, 0.018)
            v_kv = "220" if s_idx <= 8 else "110"
            s_name = f"{v_kv}kV{zone_names[z_i]}{s_idx:02d}变电站"
            rated_cap = 240.0 if v_kv == "220" else 126.0
            act_load = round(rated_cap * rng.uniform(0.58, 0.88), 2)
            bus_v = round(rng.uniform(0.972, 1.012), 4)

        l_ratio = round(act_load / rated_cap, 4)
        sub_centers.append((cx, cy))
        sub_loads.append(act_load)
        sub_ratios.append(l_ratio)
        zone_loads[z_i] += act_load

        nodes.append(
            NodeRow(
                node_id=sid,
                type_name="Substation",
                node_key=key,
                title=s_name,
                properties_json=json.dumps(
                    {
                        "station_name": s_name,
                        "voltage_kv": v_kv,
                        "grid_zone": zone_names[z_i],
                    },
                    ensure_ascii=False,
                ),
                metrics_json=json.dumps(
                    {
                        "rated_capacity_mw": rated_cap,
                        "active_load_mw": act_load,
                        "load_ratio": l_ratio,
                        "bus_voltage_pu": bus_v,
                        "feeder_loss_total_mw": round(act_load * 0.018, 3),
                    }
                ),
                geo_lng=cx,
                geo_lat=cy,
                geo_shape_json=json.dumps(poly, separators=(",", ":")),
            )
        )

        p_a = plant_ids[0] if s_idx == 1 else plant_ids[(s_idx - 1) % 18]
        p_b = plant_ids[1] if s_idx == 1 else plant_ids[(s_idx + 4) % 18]
        for p_src in (p_a, p_b):
            links.append(
                LinkRow(
                    link_id=det_uuid(tenant_id, sub_slug, f"SUP:{p_src}:{sid}"),
                    link_type="SUPPLIES_SUBSTATION",
                    src_node_id=p_src,
                    dst_node_id=sid,
                    weight=1.0,
                    properties_json="{}",
                    metrics_json="{}",
                )
            )
        links.append(
            LinkRow(
                link_id=det_uuid(tenant_id, sub_slug, f"ZONE:{sid}:{zone_ids[z_i]}"),
                link_type="SUMMARIZES_ZONE",
                src_node_id=sid,
                dst_node_id=zone_ids[z_i],
                weight=1.0,
                properties_json="{}",
                metrics_json="{}",
            )
        )

    for z_idx, z_name in enumerate(zone_names):
        key = f"GZ-{z_idx + 1:03d}"
        z_load = round(zone_loads[z_idx], 2)
        margin = round((1200.0 - z_load) / 1200.0, 4) if z_load > 0 else 1.0
        nodes.append(
            NodeRow(
                node_id=zone_ids[z_idx],
                type_name="GridZoneSummary",
                node_key=key,
                title=z_name,
                properties_json=json.dumps({"zone_name": z_name}, ensure_ascii=False),
                metrics_json=json.dumps(
                    {"zone_total_load_mw": z_load, "peak_margin_ratio": margin}
                ),
                geo_lng=None,
                geo_lat=None,
                geo_shape_json=None,
            )
        )

    # 4. 160 DistTransformers + 160 FEEDS_TRANSFORMER links
    dt_ids: list[str] = []
    dt_parent_sub: list[int] = []
    feeder_links: list[tuple[str, float]] = []
    for d_idx in range(1, 161):
        key = f"DT-{d_idx:03d}"
        did = det_uuid(tenant_id, sub_slug, key)
        dt_ids.append(did)
        s_i = 0 if d_idx <= 7 else (d_idx - 1) % 26
        dt_parent_sub.append(s_i)
        cx, cy = sub_centers[s_i]
        lng = round(cx + rng.uniform(-0.015, 0.015), 6)
        lat = round(cy + rng.uniform(-0.012, 0.012), 6)
        feeder_code = f"FD-{s_i + 1:02d}-{(d_idx % 8) + 1:02d}"
        t_name = f"{zone_names[sub_zones[s_i]]}{d_idx:03d}号台区配变"
        cap_kva = float(rng.choice([800, 1000, 1250, 1600, 2000]))
        demand = round(rng.uniform(0.45, 1.45), 3)
        oil_temp = round(79.5 if s_i == 0 and d_idx <= 3 else rng.uniform(56.0, 77.0), 2)
        cur_amp = round(demand * 58.0 + rng.uniform(8.0, 26.0), 2)
        line_ratio = round(sub_ratios[s_i] * 0.65 + demand * 0.08 + cur_amp * 0.0005, 4)
        line_loss = round(line_ratio * demand * 0.04, 4)
        ov_risk = round(line_ratio * 0.65 + (0.35 if oil_temp > 75.0 else 0.1), 4)

        nodes.append(
            NodeRow(
                node_id=did,
                type_name="DistTransformer",
                node_key=key,
                title=t_name,
                properties_json=json.dumps(
                    {
                        "transformer_name": t_name,
                        "feeder_code": feeder_code,
                        "capacity_kva": cap_kva,
                    },
                    ensure_ascii=False,
                ),
                metrics_json=json.dumps(
                    {
                        "demand_mw": demand,
                        "oil_temp_c": oil_temp,
                        "feeder_load_in": line_ratio,
                        "overload_risk": ov_risk,
                    }
                ),
                geo_lng=lng,
                geo_lat=lat,
                geo_shape_json=None,
            )
        )

        fl_id = det_uuid(tenant_id, sub_slug, f"FEED:{sub_ids[s_i]}:{did}")
        feeder_links.append((fl_id, cur_amp))
        links.append(
            LinkRow(
                link_id=fl_id,
                link_type="FEEDS_TRANSFORMER",
                src_node_id=sub_ids[s_i],
                dst_node_id=did,
                weight=1.0,
                properties_json=json.dumps({"feeder_code": feeder_code}),
                metrics_json=json.dumps(
                    {
                        "current_amp": cur_amp,
                        "line_load_ratio": line_ratio,
                        "line_loss_mw": line_loss,
                    }
                ),
            )
        )

    # 5. 90 CriticalLoads strictly inside corresponding Substation Polygon geofences
    cl_ids: list[str] = []
    for c_idx in range(1, 91):
        key = f"CL-{c_idx:03d}"
        cid = det_uuid(tenant_id, sub_slug, key)
        cl_ids.append(cid)
        primary_dt_idx = (c_idx - 1) if c_idx <= 7 else ((c_idx * 7) % 160)
        s_i = dt_parent_sub[primary_dt_idx]
        cx, cy = sub_centers[s_i]
        lng = round(cx + rng.uniform(-0.013, 0.013), 6)
        lat = round(cy + rng.uniform(-0.010, 0.010), 6)
        cat = load_cats[(c_idx - 1) % len(load_cats)]
        interruptible = cat == "商业综合体"
        l_name = f"{zone_names[sub_zones[s_i]]}{c_idx:02d}号{cat}"
        p_draw = round(rng.uniform(0.35, 1.20), 3)
        backup_h = round(rng.uniform(4.0, 16.0), 1)
        bus_v = 0.960 if s_i == 0 else round(rng.uniform(0.955, 0.995), 4)

        nodes.append(
            NodeRow(
                node_id=cid,
                type_name="CriticalLoad",
                node_key=key,
                title=l_name,
                properties_json=json.dumps(
                    {
                        "load_name": l_name,
                        "load_category": cat,
                        "interruptible": interruptible,
                    },
                    ensure_ascii=False,
                ),
                metrics_json=json.dumps(
                    {
                        "power_draw_mw": p_draw,
                        "backup_hours": backup_h,
                        "bus_voltage_pu": bus_v,
                    }
                ),
                geo_lng=lng,
                geo_lat=lat,
                geo_shape_json=None,
            )
        )

        links.append(
            LinkRow(
                link_id=det_uuid(tenant_id, sub_slug, f"SERV:{dt_ids[primary_dt_idx]}:{cid}"),
                link_type="SERVES_LOAD",
                src_node_id=dt_ids[primary_dt_idx],
                dst_node_id=cid,
                weight=1.0,
                properties_json="{}",
                metrics_json="{}",
            )
        )
        backup_cnt = 3 if c_idx <= 24 else 2
        for b_off in range(1, backup_cnt + 1):
            alt_dt_idx = (primary_dt_idx + b_off) % 160
            if alt_dt_idx == primary_dt_idx:
                alt_dt_idx = (primary_dt_idx + 3) % 160
            links.append(
                LinkRow(
                    link_id=det_uuid(
                        tenant_id, sub_slug, f"SERV:{dt_ids[alt_dt_idx]}:{cid}:{b_off}"
                    ),
                    link_type="SERVES_LOAD",
                    src_node_id=dt_ids[alt_dt_idx],
                    dst_node_id=cid,
                    weight=0.5,
                    properties_json="{}",
                    metrics_json="{}",
                )
            )

    # 6. 20 SwitchOrders + 20 RECORDS_SWITCH links
    event_types = ["主变N-1跳闸演练", "母联备自投动作", "10kV联络线转供", "站内储能并网切改"]
    for sw_idx in range(1, 21):
        key = f"SW-{sw_idx:03d}"
        swid = det_uuid(tenant_id, sub_slug, key)
        s_i = 0 if sw_idx == 1 else (sw_idx - 1) % 26
        ocode = f"SW-202607-{sw_idx:03d}"
        ev_type = event_types[(sw_idx - 1) % len(event_types)]
        team = f"{zone_names[sub_zones[s_i]]}调控{(sw_idx % 3) + 1}班"
        tripped = round(rng.uniform(8.0, 48.0), 2)
        nodes.append(
            NodeRow(
                node_id=swid,
                type_name="SwitchOrder",
                node_key=key,
                title=ocode,
                properties_json=json.dumps(
                    {
                        "order_code": ocode,
                        "event_type": ev_type,
                        "operator_team": team,
                    },
                    ensure_ascii=False,
                ),
                metrics_json=json.dumps({"tripped_mw": tripped}),
                geo_lng=None,
                geo_lat=None,
                geo_shape_json=None,
            )
        )
        links.append(
            LinkRow(
                link_id=det_uuid(tenant_id, sub_slug, f"REC:{sub_ids[s_i]}:{swid}"),
                link_type="RECORDS_SWITCH",
                src_node_id=sub_ids[s_i],
                dst_node_id=swid,
                weight=0.7,
                properties_json="{}",
                metrics_json="{}",
            )
        )

    # 7. Vectorized 43,200-row DuckLake table `grid_feeder_telemetry` (450 x 96 slots)
    grid_ctas_sql = """
    WITH anchor AS (
        SELECT date_trunc('hour', now())
               + (floor(extract(minute FROM now()) / 15.0)::BIGINT * INTERVAL '15 minutes') AS t_end
    ),
    raw AS (
        SELECT (i // 96) AS f_idx,
               (i % 96) AS step,
               (i // 96) % 26 AS s_i,
               0.78 + 0.22 * sin(((i % 96) - 24) * pi() / 48.0) AS fac,
               2.4 + ((i // 96) % 11) * 0.35 AS base_mw
          FROM range(0, 43200) AS r(i)
    ),
    calc AS (
        SELECT *,
               least(1.08, (base_mw * fac / 6.5) * 0.92) AS l_rat
          FROM raw
    )
    SELECT format('GT-{:03d}-{:02d}', f_idx + 1, step + 1)::VARCHAR AS telemetry_id,
           (a.t_end - ((95 - step) * INTERVAL '15 minutes'))::TIMESTAMPTZ AS ts,
           format('SUB-{:03d}', s_i + 1)::VARCHAR AS substation_code,
           format('FD-{:02d}-{:02d}', s_i + 1, (f_idx // 26) + 1)::VARCHAR AS feeder_code,
           ([
               '城北分区','钱塘分区','高新分区','临港分区','城西分区',
               '滨江分区','萧山分区','余杭分区','富阳分区','临平分区'
            ])[(s_i % 10) + 1]::VARCHAR AS grid_zone,
           (CASE WHEN s_i < 8 THEN '220' ELSE '110' END)::VARCHAR AS voltage_kv,
           round(base_mw * fac * 57.5, 2)::DOUBLE AS current_amp,
           round(base_mw * fac, 3)::DOUBLE AS active_mw,
           round(l_rat, 4)::DOUBLE AS line_load_ratio,
           round((base_mw * fac) * l_rat * 0.032, 4)::DOUBLE AS line_loss_mw,
           round(1.008 - l_rat * 0.065, 4)::DOUBLE AS bus_voltage_pu,
           'seed-202604'::VARCHAR AS "_ax_batch_id",
           now()::TIMESTAMPTZ AS "_ax_synced_at"
      FROM calc
      CROSS JOIN anchor a
    """
    # 450 node-metric streams x 96 = 43,200 points in `xb_metric_series`
    monitored_streams: list[tuple[str, str, float]] = []
    for i in range(18):
        monitored_streams.append((plant_ids[i], "generation_mw", 160.0 + (i % 5) * 22.0))
        monitored_streams.append((plant_ids[i], "spinning_reserve_mw", 45.0 + (i % 4) * 8.0))
    for i in range(26):
        monitored_streams.append((sub_ids[i], "active_load_mw", sub_loads[i]))
        monitored_streams.append((sub_ids[i], "load_ratio", sub_ratios[i]))
        monitored_streams.append((sub_ids[i], "bus_voltage_pu", 0.985))
    for i in range(160):
        monitored_streams.append((dt_ids[i], "demand_mw", 0.85 + (i % 7) * 0.08))
    for i in range(90):
        monitored_streams.append((cl_ids[i], "power_draw_mw", 0.62 + (i % 6) * 0.07))
    for i in range(86):
        monitored_streams.append((dt_ids[i], "oil_temp_c", 64.0 + (i % 9) * 1.6))

    grid_ds_id = det_uuid(tenant_id, "dataset", "grid_feeder_telemetry")
    grid_cols = [
        ColumnMetaSpec("telemetry_id", "STRING", "NONE", "遥测采样记录主键", ""),
        ColumnMetaSpec("ts", "TIMESTAMP", "NONE", "15分钟采样UTC时间戳", ""),
        ColumnMetaSpec("substation_code", "STRING", "NONE", "所属变电站编码", ""),
        ColumnMetaSpec("feeder_code", "STRING", "NONE", "10kV馈线编号", ""),
        ColumnMetaSpec("grid_zone", "STRING", "NONE", "供电网格分区", ""),
        ColumnMetaSpec("voltage_kv", "STRING", "NONE", "主变电压等级", "kV"),
        ColumnMetaSpec("current_amp", "FLOAT64", "NONE", "馈线实时相电流", "A"),
        ColumnMetaSpec("active_mw", "FLOAT64", "NONE", "馈线有功功率", "MW"),
        ColumnMetaSpec("line_load_ratio", "FLOAT64", "NONE", "馈线实时负载率", "ratio"),
        ColumnMetaSpec("line_loss_mw", "FLOAT64", "NONE", "馈线有功网损", "MW"),
        ColumnMetaSpec("bus_voltage_pu", "FLOAT64", "NONE", "母线运行电压标幺值", "p.u."),
    ]

    all_node_ids = [n.node_id for n in nodes]
    all_link_ids = [lk.link_id for lk in links]
    with concurrent.futures.ThreadPoolExecutor(max_workers=1) as pool:
        lake_fut = pool.submit(
            execute_ducklake_ctas,
            tenant_id=tenant_id,
            table_name="grid_feeder_telemetry",
            select_sql=grid_ctas_sql,
            data_root=data_root,
            catalog_dsn=catalog_dsn,
        )
        with conn.cursor() as cur:
            cur.execute("SELECT set_config('app.tenant_id', %s, true)", (tenant_id,))
            cleanup_orphan_main_edges(cur, tenant_id=tenant_id)
            bulk_replace_subgraph_graph_pg(
                cur,
                tenant_id=tenant_id,
                subgraph_slug=sub_slug,
                nodes=nodes,
                links=links,
            )
            series_cnt = generate_series_and_rollups_in_pg(
                cur,
                tenant_id=tenant_id,
                all_node_ids=all_node_ids,
                streams=monitored_streams,
            )
            cur.execute(
                """
                DELETE FROM xb_link_metric_series
                 WHERE tenant_id = %s AND link_id = ANY(%s::uuid[])
                """,
                (tenant_id, all_link_ids),
            )
            cur.execute(
                """
                WITH anchor AS (
                    SELECT date_trunc('hour', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'
                           + (
                               floor(extract(minute FROM now()) / 15.0)
                               * interval '15 minutes'
                           ) AS t_end
                ),
                fl AS (
                    SELECT u.lid, u.amp
                      FROM UNNEST(%s::uuid[], %s::float8[]) AS u(lid, amp)
                )
                INSERT INTO xb_link_metric_series (
                    tenant_id, link_id, metric_key, ts, val_num, source_tag
                )
                SELECT %s,
                       fl.lid,
                       'current_amp',
                       a.t_end - ((95 - g.step) * interval '15 minutes'),
                       round(
                           (fl.amp * (0.78 + 0.22 * sin((g.step - 24) * pi() / 48.0)))::numeric,
                           2
                       )::float8,
                       'INGEST'
                  FROM fl
                  CROSS JOIN generate_series(0, 95) AS g(step)
                  CROSS JOIN anchor a
                """,
                (
                    [f[0] for f in feeder_links],
                    [f[1] for f in feeder_links],
                    tenant_id,
                ),
            )
            lake_rows, snap_id = lake_fut.result()
            eff_ds_id = upsert_dataset_catalog_pg(
                cur,
                tenant_id=tenant_id,
                dataset_id=grid_ds_id,
                dataset_name="grid_feeder_telemetry",
                display_name="配网10kV馈线遥测时序表",
                description="26座变电站及下辖馈线最近24小时15分钟级电流、有功、线损与电压明细",
                columns=grid_cols,
                row_count=lake_rows,
                snapshot_id=snap_id,
            )
        conn.commit()

    elapsed_ms = round((time.perf_counter() - t0) * 1000.0, 1)
    return {
        "world": sub_slug,
        "nodes": len(nodes),
        "links": len(links),
        "series_points": series_cnt,
        "link_series_points": len(feeder_links) * 96,
        "lake_rows": lake_rows,
        "dataset_id": eff_ds_id,
        "elapsed_ms": elapsed_ms,
        "refs": {
            "pp01_id": plant_ids[0],
            "pp02_id": plant_ids[1],
            "sub01_id": sub_ids[0],
            "sub01_poly": sub01_poly,
        },
    }


def seed_fin_risk_world(
    conn: psycopg.Connection[tuple[Any, ...]],
    *,
    tenant_id: str,
    data_root: Path | None,
    catalog_dsn: str | None,
) -> dict[str, Any]:
    """Generate and load `fin_risk` (1,210 nodes, 4,280 links, 60,000 wire rows)."""
    t0 = time.perf_counter()
    rng = random.Random(WORLD_SEED + 7)
    sub_slug = "fin_risk"

    sectors = [
        "高端制造",
        "基础设施",
        "大宗流通",
        "化工新材",
        "汽车零部件",
        "新能源装备",
        "商业地产",
        "港口航运",
        "生物医药",
        "电子信息",
        "精密机械",
        "现代物流",
        "冷链供应链",
        "绿色建材",
        "纺织化纤",
        "有色冶金",
        "光电显示",
        "环保水务",
        "食品加工",
        "智能家居",
    ]
    branches = [
        "sh_branch",
        "hz_branch",
        "sz_branch",
        "bj_branch",
        "gz_branch",
        "nj_branch",
    ]
    ratings = ["AAA", "AA+", "AA", "AA-", "A+", "A", "BBB+"]
    products = ["流动资金贷款", "银行承兑汇票", "国内信用证", "供应链保理"]
    surnames = [
        "周", "林", "许", "顾", "陈", "张", "王", "李", "赵", "沈",
        "陆", "徐", "孙", "胡", "朱", "高", "何", "郭", "马", "罗",
    ]
    given_a = ["建", "振", "志", "敏", "文", "浩", "明", "永", "晓", "静"]
    given_b = ["明", "华", "强", "慧", "杰", "宇", "轩", "平", "芳", "磊"]
    area_codes = ["310101", "330102", "440304", "110105", "440106", "320102"]

    nodes: list[NodeRow] = []
    links: list[LinkRow] = []
    seen_edges: set[tuple[str, str, str]] = set()

    def add_unique_link(
        ltype: str,
        src_id: str,
        dst_id: str,
        weight: float,
        props_json: str = "{}",
        metrics_json: str = "{}",
    ) -> bool:
        edge_key = (ltype, src_id, dst_id)
        if src_id == dst_id or edge_key in seen_edges:
            return False
        seen_edges.add(edge_key)
        links.append(
            LinkRow(
                link_id=det_uuid(tenant_id, sub_slug, f"{ltype}:{src_id}:{dst_id}"),
                link_type=ltype,
                src_node_id=src_id,
                dst_node_id=dst_id,
                weight=weight,
                properties_json=props_json,
                metrics_json=metrics_json,
            )
        )
        return True

    # 1. 40 CorpGroups & 20 IndustryExposureCubes
    group_ids = [det_uuid(tenant_id, sub_slug, f"CG-{i:03d}") for i in range(1, 41)]
    group_exp = [0.0] * 40
    sector_ids = [det_uuid(tenant_id, sub_slug, f"SEC-{i:02d}") for i in range(1, 21)]
    sec_exp = [0.0] * 20
    sec_ovd = [0.0] * 20

    # 2. 620 LegalEntities across 6 branches
    le_ids: list[str] = []
    le_loans: list[float] = []
    le_ovd_ratios: list[float] = []

    for i in range(1, 621):
        key = f"LE-{i:04d}"
        lid = det_uuid(tenant_id, sub_slug, key)
        le_ids.append(lid)
        g_idx = (i - 1) % 40
        sec_idx = (i - 1) % 20
        b_dept = branches[(i - 1) % len(branches)]

        ename = f"{sectors[sec_idx][:2]}实业{i:04d}有限公司"
        ucode = f"913100007{i:08d}X"
        reg_cap = round(rng.uniform(2000.0, 25000.0), 1)
        loan_bal = round(rng.uniform(1200.0, 9800.0), 2)
        credit_lim = round(loan_bal * rng.uniform(1.15, 1.45), 2)

        # Deterministic overdue profile: every 7th entity (and LE-0002) has >= 0.062
        is_overdue_escalated = (i % 7 == 0) or (i == 2)
        if is_overdue_escalated:
            ovd_ratio = round(rng.uniform(0.062, 0.145), 4)
            frozen_amt = round(credit_lim * 0.15, 2)
        else:
            ovd_ratio = round(rng.uniform(0.0, 0.028), 4)
            frozen_amt = 0.0
        ovd_amt = round(loan_bal * ovd_ratio, 2)
        eff_ratio = round(ovd_amt / loan_bal, 4)

        le_loans.append(loan_bal)
        le_ovd_ratios.append(eff_ratio)
        group_exp[g_idx] += loan_bal
        sec_exp[sec_idx] += loan_bal
        sec_ovd[sec_idx] += ovd_amt

        nodes.append(
            NodeRow(
                node_id=lid,
                type_name="LegalEntity",
                node_key=key,
                title=ename,
                properties_json=json.dumps(
                    {
                        "entity_name": ename,
                        "unified_code": ucode,
                        "branch_dept": b_dept,
                        "reg_capital": reg_cap,
                    },
                    ensure_ascii=False,
                ),
                metrics_json=json.dumps(
                    {
                        "credit_limit": credit_lim,
                        "frozen_amount": frozen_amt,
                        "loan_balance": loan_bal,
                        "overdue_amount": ovd_amt,
                        "overdue_ratio": eff_ratio,
                        "guarantee_out_total": 0.0,
                        "inbound_contagion_risk": 0.0,
                    }
                ),
                geo_lng=None,
                geo_lat=None,
                geo_shape_json=None,
            )
        )

        add_unique_link("CONTROLS", group_ids[g_idx], lid, 0.85)
        add_unique_link("BELONGS_TO_SECTOR", lid, sector_ids[sec_idx], 0.70)

    for g_idx in range(40):
        key = f"CG-{g_idx + 1:03d}"
        g_name = f"华信{g_idx + 1:02d}号控股集团"
        tot_exp = round(group_exp[g_idx], 2)
        c_score = round(min(100.0, tot_exp / 500.0), 2) if tot_exp > 0 else 0.0
        nodes.append(
            NodeRow(
                node_id=group_ids[g_idx],
                type_name="CorpGroup",
                node_key=key,
                title=g_name,
                properties_json=json.dumps(
                    {
                        "group_name": g_name,
                        "industry": sectors[g_idx % 20],
                        "credit_rating": ratings[g_idx % len(ratings)],
                    },
                    ensure_ascii=False,
                ),
                metrics_json=json.dumps(
                    {"group_total_exposure": tot_exp, "contagion_score": c_score}
                ),
                geo_lng=None,
                geo_lat=None,
                geo_shape_json=None,
            )
        )

    for s_idx, s_name in enumerate(sectors):
        key = f"SEC-{s_idx + 1:02d}"
        s_e = round(sec_exp[s_idx], 2)
        s_o = round(sec_ovd[s_idx], 2)
        npl = round(s_o / s_e, 4) if s_e > 0 else 0.0
        nodes.append(
            NodeRow(
                node_id=sector_ids[s_idx],
                type_name="IndustryExposureCube",
                node_key=key,
                title=s_name,
                properties_json=json.dumps({"sector_name": s_name}, ensure_ascii=False),
                metrics_json=json.dumps(
                    {
                        "sector_exposure_sum": s_e,
                        "sector_overdue_sum": s_o,
                        "npl_ratio": npl,
                    }
                ),
                geo_lng=None,
                geo_lat=None,
                geo_shape_json=None,
            )
        )

    # 3. 180 BeneficialOwners with compliant format-valid PII + 540 HOLDS_EQUITY links
    for b_idx in range(1, 181):
        key = f"BO-{b_idx:03d}"
        bid = det_uuid(tenant_id, sub_slug, key)
        fname = (
            f"{surnames[(b_idx - 1) % len(surnames)]}"
            f"{given_a[(b_idx // 3) % len(given_a)]}"
            f"{given_b[(b_idx // 7) % len(given_b)]}"
        )
        phone = f"13{8 + (b_idx % 2)}{b_idx:04d}{(b_idx * 37) % 10000:04d}"
        birth = f"{1968 + (b_idx % 25):04d}{(b_idx % 12) + 1:02d}{(b_idx % 27) + 1:02d}"
        id_card = make_cn_id_card(area_codes[b_idx % len(area_codes)], birth, 100 + b_idx)
        bank_card = make_luhn_card("622202", 100100000000 + b_idx * 137)

        ctrl_exp = 0.0
        for h_slot in range(3):
            target_le_idx = ((b_idx - 1) * 3 + h_slot * 17) % 620
            eq_ratio = round(0.25 + 0.18 * (3 - h_slot), 2)
            ctrl_exp += le_loans[target_le_idx] * eq_ratio
            add_unique_link(
                "HOLDS_EQUITY",
                bid,
                le_ids[target_le_idx],
                eq_ratio,
                metrics_json=json.dumps({"equity_ratio": eq_ratio}),
            )

        nodes.append(
            NodeRow(
                node_id=bid,
                type_name="BeneficialOwner",
                node_key=key,
                title=f"实控人·{key}",
                properties_json=json.dumps(
                    {
                        "full_name": fname,
                        "mobile_phone": phone,
                        "national_id": id_card,
                        "settlement_card": bank_card,
                    },
                    ensure_ascii=False,
                ),
                metrics_json=json.dumps({"controlled_exposure": round(ctrl_exp, 2)}),
                geo_lng=None,
                geo_lat=None,
                geo_shape_json=None,
            )
        )

    # 4. 350 CreditFacilities + 350 DRAWS_FACILITY links
    for f_idx in range(1, 351):
        key = f"CF-{f_idx:04d}"
        fid = det_uuid(tenant_id, sub_slug, key)
        le_i = (f_idx * 5) % 620
        fcode = f"LN-2026-{f_idx:04d}"
        pkind = products[(f_idx - 1) % len(products)]
        mdate = f"2027-{(f_idx % 12) + 1:02d}-{(f_idx % 28) + 1:02d}"
        p_amt = round(le_loans[le_i] * rng.uniform(0.30, 0.65), 2)
        dpd = float((f_idx % 45) + 15) if le_ovd_ratios[le_i] >= 0.05 else 0.0
        nodes.append(
            NodeRow(
                node_id=fid,
                type_name="CreditFacility",
                node_key=key,
                title=fcode,
                properties_json=json.dumps(
                    {
                        "facility_code": fcode,
                        "product_kind": pkind,
                        "maturity_date": mdate,
                    },
                    ensure_ascii=False,
                ),
                metrics_json=json.dumps({"principal_amt": p_amt, "days_past_due": dpd}),
                geo_lng=None,
                geo_lat=None,
                geo_shape_json=None,
            )
        )
        add_unique_link("DRAWS_FACILITY", le_ids[le_i], fid, 0.80)

    # 5. 1,150 CROSS_GUARANTEES links (including 25 closed mutual-guarantee rings of size 5)
    cg_added = 0
    for ring_id in range(25):
        base_idx = ring_id * 20
        ring_members = [base_idx + k for k in range(5)]
        for pos in range(5):
            u_i = ring_members[pos]
            v_i = ring_members[(pos + 1) % 5]
            g_amt = round(rng.uniform(800.0, 4200.0), 2)
            c_flux = round(g_amt * le_ovd_ratios[u_i], 2)
            if add_unique_link(
                "CROSS_GUARANTEES",
                le_ids[u_i],
                le_ids[v_i],
                0.75,
                props_json=json.dumps({"ring_code": f"RING-{ring_id + 1:02d}"}),
                metrics_json=json.dumps(
                    {"guarantee_amt": g_amt, "contagion_flux": c_flux}
                ),
            ):
                cg_added += 1

    cg_cursor = 0
    while cg_added < 1150:
        u_i = cg_cursor % 620
        v_i = (u_i + 3 + (cg_cursor // 620) * 7) % 620
        cg_cursor += 1
        g_amt = round(rng.uniform(500.0, 3600.0), 2)
        c_flux = round(g_amt * le_ovd_ratios[u_i], 2)
        if add_unique_link(
            "CROSS_GUARANTEES",
            le_ids[u_i],
            le_ids[v_i],
            0.75,
            metrics_json=json.dumps({"guarantee_amt": g_amt, "contagion_flux": c_flux}),
        ):
            cg_added += 1

    # 6. 1,000 TRANSFERS_FUNDS links (total links = 4,280 >= 4,260)
    tf_added = 0
    tf_cursor = 0
    while tf_added < 1000:
        u_i = tf_cursor % 620
        v_i = (u_i + 11 + (tf_cursor // 620) * 13) % 620
        tf_cursor += 1
        t_amt = round(rng.uniform(300.0, 5800.0), 2)
        if add_unique_link(
            "TRANSFERS_FUNDS",
            le_ids[u_i],
            le_ids[v_i],
            0.65,
            metrics_json=json.dumps({"transfer_amt": t_amt}),
        ):
            tf_added += 1

    # 7. Vectorized 60,000-row `fin_corp_wire_transfers` DuckLake CTAS
    wire_ctas_sql = """
    WITH anchor AS (
        SELECT date_trunc('minute', now()) AS t_end
    ),
    raw AS (
        SELECT i,
               (i % 620) AS u_idx,
               ((i * 7 + 13) % 620) AS v_idx
          FROM range(0, 60000) AS r(i)
    )
    SELECT format('WT-2026-{:06d}', i + 1)::VARCHAR AS tx_id,
           (a.t_end - (((60000 - i) % 43200) * INTERVAL '1 minute'))::TIMESTAMPTZ AS tx_time,
           format('LE-{:04d}', u_idx + 1)::VARCHAR AS payer_entity_key,
           format('实业{:04d}有限公司', u_idx + 1)::VARCHAR AS payer_name,
           format('LE-{:04d}', v_idx + 1)::VARCHAR AS payee_entity_key,
           format('实业{:04d}有限公司', v_idx + 1)::VARCHAR AS payee_name,
           format(
               '138{:04d}{:04d}', (u_idx % 9000) + 1000, (u_idx * 73) % 10000
           )::VARCHAR AS operator_phone,
           format('622848{:013d}', 2002000000000 + v_idx * 211)::VARCHAR AS payee_card_no,
           ([
               'sh_branch','hz_branch','sz_branch','bj_branch','gz_branch','nj_branch'
            ])[(u_idx % 6) + 1]::VARCHAR AS branch_dept,
           ([
               '高端制造','基础设施','大宗流通','化工新材','汽车零部件',
               '新能源装备','商业地产','港口航运','生物医药','电子信息'
            ])[(u_idx % 10) + 1]::VARCHAR AS industry,
           (['大额支付系统','超级网银','银企直联','跨境人民币'])[(i % 4) + 1]::VARCHAR AS channel,
           (CASE
               WHEN ((u_idx + 1) % 7 = 0) OR (u_idx + 1 = 2) THEN 'HIGH'
               WHEN (i % 6) IN (3, 4) THEN 'MEDIUM'
               ELSE 'LOW'
            END)::VARCHAR AS risk_tier,
           round(45.0 + (i % 97) * 14.5 + (i % 13) * 8.25, 2)::DOUBLE AS transfer_amt,
           ((i % 3) + 1)::BIGINT AS tx_count,
           'seed-202604'::VARCHAR AS "_ax_batch_id",
           now()::TIMESTAMPTZ AS "_ax_synced_at"
      FROM raw
      CROSS JOIN anchor a
    """
    fin_streams: list[tuple[str, str, float]] = [
        (group_ids[g_idx], "group_total_exposure", round(group_exp[g_idx], 2))
        for g_idx in range(40)
    ] + [
        (le_ids[le_i], "loan_balance", le_loans[le_i])
        for le_i in range(60)
    ]

    fin_ds_id = det_uuid(tenant_id, "dataset", "fin_corp_wire_transfers")
    fin_cols = [
        ColumnMetaSpec("tx_id", "STRING", "NONE", "资金划转流水唯一编号", ""),
        ColumnMetaSpec("tx_time", "TIMESTAMP", "NONE", "交易记账UTC时间戳", ""),
        ColumnMetaSpec("payer_entity_key", "STRING", "NONE", "付款法人节点标识", ""),
        ColumnMetaSpec("payer_name", "STRING", "NONE", "付款方对公户名", ""),
        ColumnMetaSpec("payee_entity_key", "STRING", "NONE", "收款法人节点标识", ""),
        ColumnMetaSpec("payee_name", "STRING", "NONE", "收款方对公户名", ""),
        ColumnMetaSpec("operator_phone", "STRING", "PHONE", "经办财务人员手机号", ""),
        ColumnMetaSpec("payee_card_no", "STRING", "BANK_CARD", "对手方结算卡号", ""),
        ColumnMetaSpec("branch_dept", "STRING", "NONE", "主办管户分行部门", ""),
        ColumnMetaSpec("industry", "STRING", "NONE", "付款企业所属行业", ""),
        ColumnMetaSpec("channel", "STRING", "NONE", "清算支付渠道", ""),
        ColumnMetaSpec("risk_tier", "STRING", "NONE", "反洗钱与回流风险等级", ""),
        ColumnMetaSpec("transfer_amt", "FLOAT64", "NONE", "对公划转金额", "万元"),
        ColumnMetaSpec("tx_count", "INT64", "NONE", "归并划转笔数", "笔"),
    ]

    all_node_ids = [n.node_id for n in nodes]
    with concurrent.futures.ThreadPoolExecutor(max_workers=1) as pool:
        lake_fut = pool.submit(
            execute_ducklake_ctas,
            tenant_id=tenant_id,
            table_name="fin_corp_wire_transfers",
            select_sql=wire_ctas_sql,
            data_root=data_root,
            catalog_dsn=catalog_dsn,
        )
        with conn.cursor() as cur:
            cur.execute("SELECT set_config('app.tenant_id', %s, true)", (tenant_id,))
            cleanup_orphan_main_edges(cur, tenant_id=tenant_id)
            bulk_replace_subgraph_graph_pg(
                cur,
                tenant_id=tenant_id,
                subgraph_slug=sub_slug,
                nodes=nodes,
                links=links,
            )
            series_cnt = generate_series_and_rollups_in_pg(
                cur,
                tenant_id=tenant_id,
                all_node_ids=all_node_ids,
                streams=fin_streams,
            )
            lake_rows, snap_id = lake_fut.result()
            eff_ds_id = upsert_dataset_catalog_pg(
                cur,
                tenant_id=tenant_id,
                dataset_id=fin_ds_id,
                dataset_name="fin_corp_wire_transfers",
                display_name="对公资金划转流水表（6万行）",
                description="集团关联法人对公账户60,000行大额资金划转与对手方穿透流水明细",
                columns=fin_cols,
                row_count=lake_rows,
                snapshot_id=snap_id,
            )
        conn.commit()

    elapsed_ms = round((time.perf_counter() - t0) * 1000.0, 1)
    return {
        "world": sub_slug,
        "nodes": len(nodes),
        "links": len(links),
        "series_points": series_cnt,
        "lake_rows": lake_rows,
        "dataset_id": eff_ds_id,
        "elapsed_ms": elapsed_ms,
        "refs": {
            "sh_healthy_key": "LE-0001",
            "hz_overdue_key": "LE-0002",
            "sz_healthy_key": "LE-0003",
        },
    }


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    """Parse CLI options for `gen_worlds_grid_fin.py`."""
    parser = argparse.ArgumentParser(
        description="Deterministic vectorized generator for power_grid and fin_risk worlds."
    )
    parser.add_argument(
        "--world",
        choices=["power_grid", "fin_risk", "all"],
        default="all",
        help="Target industry world to generate and ingest.",
    )
    parser.add_argument(
        "--tenant",
        default="default",
        help="Target tenant identifier (default: 'default').",
    )
    parser.add_argument(
        "--dsn",
        default="",
        help="Optional PostgreSQL DSN override (defaults to DATABASE_URL env).",
    )
    parser.add_argument(
        "--data-root",
        default="",
        help="Optional AXPOINT_DATA_ROOT override.",
    )
    parser.add_argument(
        "--catalog-dsn",
        default="",
        help="Optional AXPOINT_LAKE_CATALOG_DSN override.",
    )
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    """Entry point for CLI execution."""
    args = parse_args(argv)
    tenant_id = validate_ident(str(args.tenant))
    db_dsn = (str(args.dsn).strip() or os.environ.get("DATABASE_URL", "").strip())
    if not db_dsn:
        raise RuntimeError("DATABASE_URL or --dsn must be provided")

    data_root = Path(args.data_root).resolve() if str(args.data_root).strip() else None
    cat_dsn = str(args.catalog_dsn).strip() or None
    target_world: WorldKind = args.world

    results: list[dict[str, Any]] = []
    with psycopg.connect(db_dsn) as conn:
        if target_world in ("power_grid", "all"):
            results.append(
                seed_power_grid_world(
                    conn,
                    tenant_id=tenant_id,
                    data_root=data_root,
                    catalog_dsn=cat_dsn,
                )
            )
        if target_world in ("fin_risk", "all"):
            results.append(
                seed_fin_risk_world(
                    conn,
                    tenant_id=tenant_id,
                    data_root=data_root,
                    catalog_dsn=cat_dsn,
                )
            )

    payload: dict[str, Any] = results[0] if len(results) == 1 else {"worlds": results}
    print(json.dumps(payload, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
