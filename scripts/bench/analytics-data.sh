#!/usr/bin/env bash
# Generates 10,000,000 rows in DuckLake table bench_orders (10 dimension cols, 5 numeric cols) per 05-STEP04 §04.11.
set -euo pipefail
source "$(dirname "$0")/../gate/lib.sh"

ROWS="${BENCH_ROWS:-10000000}"
echo "Generating $ROWS rows into DuckLake table bench_orders for tenant default..."

"$ROOT/axpoint-parser/.venv/bin/python" - "$ROWS" <<'PY'
from __future__ import annotations

import os
import sys
import duckdb

rows = int(sys.argv[1])
data_root = os.environ["AXPOINT_DATA_ROOT"]
lake_dsn = os.environ["AXPOINT_LAKE_CATALOG_DSN"]

from urllib.parse import urlparse, unquote
u = urlparse(lake_dsn)
libpq = (
    f"host={u.hostname or '127.0.0.1'} "
    f"port={u.port or 5432} "
    f"dbname={(u.path or '/xbluenode').lstrip('/')} "
    f"user={unquote(u.username or 'axpoint_lake')} "
    f"password={unquote(u.password or '')}"
)
lake_path = os.path.join(data_root, "lake")
os.makedirs(lake_path, exist_ok=True)

con = duckdb.connect()
con.execute("LOAD ducklake; LOAD postgres;")
con.execute(
    f"ATTACH 'ducklake:postgres:{libpq}' AS lake "
    f"(DATA_PATH '{lake_path}', METADATA_SCHEMA 'lake_catalog');"
)
con.execute('CREATE SCHEMA IF NOT EXISTS lake."tenant_default";')
con.execute(f"""
BEGIN;
CREATE OR REPLACE TABLE lake."tenant_default".bench_orders AS
SELECT
    i AS order_id,
    'R-' || (i % 10)::VARCHAR AS region,
    'C-' || (i % 50)::VARCHAR AS category,
    CASE WHEN i % 3 = 0 THEN 'PAID' WHEN i % 3 = 1 THEN 'SHIPPED' ELSE 'OPEN' END AS status,
    'CH-' || (i % 5)::VARCHAR AS channel,
    'WH-' || (i % 20)::VARCHAR AS warehouse,
    'SEG-' || (i % 4)::VARCHAR AS segment,
    'TIER-' || (i % 3)::VARCHAR AS tier,
    'CUR-' || (i % 2)::VARCHAR AS currency,
    'PAY-' || (i % 6)::VARCHAR AS pay_method,
    'REP-' || (i % 100)::VARCHAR AS sales_rep,
    ROUND(((i % 1000) * 1.37 + 10.0)::DOUBLE, 2) AS amount,
    ((i % 20) + 1)::BIGINT AS qty,
    ROUND(((i % 15) * 0.5)::DOUBLE, 2) AS discount,
    ROUND(((i % 1000) * 0.08)::DOUBLE, 2) AS tax,
    ROUND(((i % 50) * 0.9)::DOUBLE, 2) AS shipping_fee,
    TIMESTAMPTZ '2025-01-01 00:00:00+00' + ((i % 730) || ' days')::INTERVAL AS order_ts
FROM range(1, {rows} + 1) AS t(i);
COMMIT;
""")
cnt = con.execute('SELECT count(*) FROM lake."tenant_default".bench_orders').fetchone()
print(f"Materialized lake.tenant_default.bench_orders: {cnt[0] if cnt else 0} rows")
PY

# Register bench_orders dataset + metric in PostgreSQL metadata if not yet registered
EXISTING_ID=$(api GET "/api/axhub/datasets?q=bench_orders" | jq -r '.items[] | select(.name=="bench_orders") | .id' | head -n1)
if [[ -z "$EXISTING_ID" ]]; then
  sql_app default "
  INSERT INTO ds_datasets (
      tenant_id, name, display_name, description, source_kind, source_ref,
      sync_mode, schema_policy, lake_schema, lake_table, schema_json,
      schema_version, row_count, last_snapshot_id, last_synced_at, status
  ) VALUES (
      'default', 'bench_orders', '基准测试订单表', '05-STEP04 §04.11 1000 万行基准数据集',
      'DERIVED_VIEW', '{\"bench\":true}'::jsonb, 'SNAPSHOT', 'ADDITIVE',
      'tenant_default', 'bench_orders',
      '[
        {\"name\":\"order_id\",\"data_type\":\"INT64\",\"nullable\":false,\"ordinal\":0},
        {\"name\":\"region\",\"data_type\":\"STRING\",\"nullable\":false,\"ordinal\":1},
        {\"name\":\"category\",\"data_type\":\"STRING\",\"nullable\":false,\"ordinal\":2},
        {\"name\":\"status\",\"data_type\":\"STRING\",\"nullable\":false,\"ordinal\":3},
        {\"name\":\"channel\",\"data_type\":\"STRING\",\"nullable\":false,\"ordinal\":4},
        {\"name\":\"warehouse\",\"data_type\":\"STRING\",\"nullable\":false,\"ordinal\":5},
        {\"name\":\"segment\",\"data_type\":\"STRING\",\"nullable\":false,\"ordinal\":6},
        {\"name\":\"tier\",\"data_type\":\"STRING\",\"nullable\":false,\"ordinal\":7},
        {\"name\":\"currency\",\"data_type\":\"STRING\",\"nullable\":false,\"ordinal\":8},
        {\"name\":\"pay_method\",\"data_type\":\"STRING\",\"nullable\":false,\"ordinal\":9},
        {\"name\":\"sales_rep\",\"data_type\":\"STRING\",\"nullable\":false,\"ordinal\":10},
        {\"name\":\"amount\",\"data_type\":\"FLOAT64\",\"nullable\":false,\"ordinal\":11},
        {\"name\":\"qty\",\"data_type\":\"INT64\",\"nullable\":false,\"ordinal\":12},
        {\"name\":\"discount\",\"data_type\":\"FLOAT64\",\"nullable\":false,\"ordinal\":13},
        {\"name\":\"tax\",\"data_type\":\"FLOAT64\",\"nullable\":false,\"ordinal\":14},
        {\"name\":\"shipping_fee\",\"data_type\":\"FLOAT64\",\"nullable\":false,\"ordinal\":15},
        {\"name\":\"order_ts\",\"data_type\":\"TIMESTAMP\",\"nullable\":false,\"ordinal\":16}
      ]'::jsonb,
      1, $ROWS, 1, NOW(), 'READY'
  ) ON CONFLICT (tenant_id, name) DO UPDATE SET row_count = EXCLUDED.row_count, status = 'READY';
  " >/dev/null
  EXISTING_ID=$(api GET "/api/axhub/datasets?q=bench_orders" | jq -r '.items[] | select(.name=="bench_orders") | .id' | head -n1)
fi

api DELETE "/api/analytics/metrics/bench_revenue" >/dev/null 2>&1 || true
api POST "/api/analytics/metrics" "$(jq -nc --arg dsid "$EXISTING_ID" '{
  name: "bench_revenue",
  display_name: "基准营收总额",
  dataset_id: $dsid,
  expression: "SUM(amount)",
  filter_sql: "status = '\''PAID'\''",
  time_column: "order_ts",
  default_grain: "MONTH",
  dimensions: [{name: "region", column: "region", display_name: "区域"}],
  certified: true
}')" >/dev/null

echo "analytics-data: OK (dataset_id=$EXISTING_ID)"
