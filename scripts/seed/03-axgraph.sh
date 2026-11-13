#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "$0")/../gate/lib.sh"

# 1. Extend xBlueStudio schema with Factory, Warehouse, Order and SUPPLIES / FULFILLS edge types
api POST /api/xblue/schema '{
  "node_types": [
    {"name": "Factory", "display_name": "工厂", "pos_x": 80, "pos_y": 140},
    {"name": "Warehouse", "display_name": "仓库", "pos_x": 360, "pos_y": 140},
    {"name": "Order", "display_name": "订单", "pos_x": 640, "pos_y": 140}
  ],
  "edge_types": [
    {"name": "SUPPLIES", "src_type_name": "Factory", "dst_type_name": "Warehouse", "impact_weight": 0.85},
    {"name": "FULFILLS", "src_type_name": "Warehouse", "dst_type_name": "Order", "impact_weight": 0.8}
  ]
}' >/dev/null

# 2. Enrich existing W-1 with Shanghai geo coordinate and bulk-upsert factories, warehouses, orders & topology links
api PATCH "/api/xblue/nodes/$WAREHOUSE_NODE_ID" '{"geo":{"lng":121.49,"lat":31.22},"properties":{"region":"华东","city":"上海"}}' >/dev/null

BULK_PAYLOAD=$(python3 - <<'PY'
import json

nodes = [
    {
        "type_name": "Factory",
        "node_key": "F-1",
        "title": "晶圆一厂",
        "properties": {"region": "华东", "city": "上海", "alias": "Fab-1"},
        "metrics": {"output": 1200.0},
        "geo": {"lng": 121.4737, "lat": 31.2304},
    },
    {
        "type_name": "Factory",
        "node_key": "F-2",
        "title": "苏州封测厂",
        "properties": {"region": "华东", "city": "苏州"},
        "metrics": {"output": 950.0},
        "geo": {"lng": 120.5853, "lat": 31.2989},
    },
    {
        "type_name": "Factory",
        "node_key": "F-3",
        "title": "深圳模组厂",
        "properties": {"region": "华南", "city": "深圳"},
        "metrics": {"output": 880.0},
        "geo": {"lng": 114.0579, "lat": 22.5431},
    },
    {
        "type_name": "Warehouse",
        "node_key": "W-2",
        "title": "华南仓",
        "properties": {"region": "华南", "city": "广州"},
        "metrics": {"inventory": 420.0},
        "geo": {"lng": 113.2644, "lat": 23.1291},
    },
]

links = [
    {"link_type": "SUPPLIES", "src": {"type_name": "Factory", "node_key": "F-1"}, "dst": {"type_name": "Warehouse", "node_key": "W-1"}, "weight": 0.85},
    {"link_type": "SUPPLIES", "src": {"type_name": "Factory", "node_key": "F-2"}, "dst": {"type_name": "Warehouse", "node_key": "W-1"}, "weight": 0.8},
    {"link_type": "SUPPLIES", "src": {"type_name": "Factory", "node_key": "F-3"}, "dst": {"type_name": "Warehouse", "node_key": "W-2"}, "weight": 0.85},
]

for idx in range(3, 23):
    okey = f"O-{idx}"
    wh_key = "W-1" if idx <= 20 else "W-2"
    nodes.append({
        "type_name": "Order",
        "node_key": okey,
        "title": f"订单 {okey}",
        "properties": {"priority": "HIGH" if idx % 4 == 0 else "NORMAL"},
        "metrics": {"demand": float(50 + idx * 3)},
    })
    links.append({
        "link_type": "FULFILLS",
        "src": {"type_name": "Warehouse", "node_key": wh_key},
        "dst": {"type_name": "Order", "node_key": okey},
        "weight": 0.8,
    })

print(json.dumps({"nodes": nodes, "links": links}, ensure_ascii=False))
PY
)
api POST /api/xblue/nodes/bulk "$BULK_PAYLOAD" >/dev/null

FACTORY1_NODE_ID=$(api GET '/api/xblue/nodes?type_name=Factory&q=F-1' | jq -r '.items[] | select(.node_key=="F-1") | .id')
ORDER20_NODE_ID=$(api GET '/api/xblue/nodes?type_name=Order&q=O-20' | jq -r '.items[] | select(.node_key=="O-20") | .id')

# 3. Seed 24 hourly inventory points (values 100..123, sum = 2676) aligned to tenant timezone Asia/Shanghai day boundary
SEED_DAY1=$(sql_app default "SELECT to_char(((date_trunc('day', now() AT TIME ZONE 'Asia/Shanghai') - interval '2 days') AT TIME ZONE 'Asia/Shanghai') AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"')")
SEED_DAY2=$(sql_app default "SELECT to_char(((date_trunc('day', now() AT TIME ZONE 'Asia/Shanghai') - interval '1 day') AT TIME ZONE 'Asia/Shanghai') AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"')")

METRICS_PAYLOAD=$(python3 - "$WAREHOUSE_NODE_ID" "$SEED_DAY1" <<'PY'
import json, sys
from datetime import datetime, timedelta, timezone

wid = sys.argv[1]
start = datetime.strptime(sys.argv[2], "%Y-%m-%dT%H:%M:%SZ").replace(tzinfo=timezone.utc)
points = []
for h in range(24):
    ts = (start + timedelta(hours=h)).strftime("%Y-%m-%dT%H:%M:%SZ")
    points.append({
        "node_id": wid,
        "metric": "inventory",
        "ts": ts,
        "value": float(100 + h),
    })
print(json.dumps({"points": points, "source_tag": "SYNC"}))
PY
)
ROLLUP_JOB=$(api POST /api/xblue/metrics/write "$METRICS_PAYLOAD" | jq -r .rollup_job_id)
wait_job "$ROLLUP_JOB"

# 4. Ingest operational bulletin for entity-link verification and wait for background EMBED_NODES jobs
api POST /api/documents/ingest '{"title":"华东仓履约周报","source_name":"weekly-ops.md","content":"晶圆一厂本周向华东仓稳定供货，华东仓安全库存保持高位并保障重点订单准时交付。"}' >/dev/null

wait_until "embed-nodes-idle" 60 \
  "[[ \$(sql_app default \"SELECT count(*) FROM ax_jobs WHERE kind='EMBED_NODES' AND status IN ('PENDING','RUNNING')\") == 0 ]]" >/dev/null

api POST /api/xblue/entities/resolve '{}' >/dev/null

echo "FACTORY1_NODE_ID=$FACTORY1_NODE_ID"
echo "ORDER20_NODE_ID=$ORDER20_NODE_ID"
echo "SEED_DAY1=$SEED_DAY1"
echo "SEED_DAY2=$SEED_DAY2"
