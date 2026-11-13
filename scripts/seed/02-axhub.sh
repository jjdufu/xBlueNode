#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "$0")/../gate/lib.sh"

# 1. Prepare a read-only demo source schema + 100-row orders table in PostgreSQL
sql_owner "
CREATE SCHEMA IF NOT EXISTS demo_src;
DROP TABLE IF EXISTS demo_src.orders;
CREATE TABLE demo_src.orders (
    order_id       INT PRIMARY KEY,
    customer       TEXT NOT NULL,
    phone          TEXT NOT NULL,
    amount         NUMERIC(12, 2) NOT NULL,
    status         TEXT NOT NULL,
    warehouse_code TEXT NOT NULL,
    region         TEXT NOT NULL,
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
INSERT INTO demo_src.orders (order_id, customer, phone, amount, status, warehouse_code, region, updated_at)
SELECT g,
       '客户-' || g,
       '1380000' || LPAD(g::text, 4, '0'),
       ROUND((g * 19.75)::numeric, 2),
       CASE WHEN g % 3 = 0 THEN 'PAID' WHEN g % 3 = 1 THEN 'SHIPPED' ELSE 'OPEN' END,
       CASE WHEN g <= 80 THEN 'W-1' ELSE 'W-2' END,
       CASE WHEN g <= 80 THEN '华东' ELSE '华南' END,
       TIMESTAMPTZ '2026-01-01 00:00:00+00' + (g || ' minutes')::interval
  FROM generate_series(1, 100) AS g;
GRANT USAGE ON SCHEMA demo_src TO axpoint_app;
GRANT SELECT ON ALL TABLES IN SCHEMA demo_src TO axpoint_app;
" >/dev/null

# 2. Extract connection parameters from DATABASE_URL and register AxHub POSTGRES connection
CONN_PAYLOAD=$(python3 - <<'PY'
import json, os
from urllib.parse import urlparse, unquote

u = urlparse(os.environ["DATABASE_URL"])
payload = {
    "name": "demo_pg",
    "connector_type": "POSTGRES",
    "allow_private_network": True,
    "config": {
        "host": u.hostname or "127.0.0.1",
        "port": u.port or 5432,
        "database": (u.path or "/xbluenode").lstrip("/"),
        "username": unquote(u.username or "axpoint_app"),
        "password": unquote(u.password or "axpoint_app_secret"),
        "schema": "demo_src",
        "sslmode": "disable",
    },
}
print(json.dumps(payload))
PY
)
CONN_ID=$(api POST /api/axhub/connections "$CONN_PAYLOAD" | jq -r .id)
api POST "/api/axhub/connections/$CONN_ID/test" >/dev/null

# 3. Register demo_orders dataset backed by demo_src.orders and run initial sync
DS_PAYLOAD=$(jq -nc --arg cid "$CONN_ID" '{
  connection_id: $cid,
  name: "demo_orders",
  display_name: "演示订单表",
  description: "供应链订单主数据 orders",
  source_kind: "SQL_TABLE",
  source_ref: { schema: "demo_src", table: "orders" },
  sync_mode: "UPSERT",
  primary_key: ["order_id"],
  cursor_column: "updated_at",
  schema_policy: "ADDITIVE",
  tags: ["demo", "orders"]
}')
ORDERS_DS_ID=$(api POST /api/axhub/datasets "$DS_PAYLOAD" | jq -r .id)
INIT_SYNC_JOB=$(api POST "/api/axhub/datasets/$ORDERS_DS_ID/sync" | jq -r .job_id)
wait_job "$INIT_SYNC_JOB"

# 4. Upload supply_chain.xlsx through unified dual-track upload and wait for PARSE_FILE job
UPLOAD_JSON=$(curl -sS -X POST \
  -H "Authorization: Bearer $AXPOINT_DEV_TOKEN" \
  -H "Accept: application/json" \
  -F "file=@$ROOT/axpoint-parser/tests/fixtures/supply_chain.xlsx" \
  -F "to_knowledge=true" \
  -F "to_datasets=true" \
  -F "dataset_prefix=supply_chain" \
  "$CORE_BASE_URL/api/axhub/uploads")
SUPPLY_UPLOAD_ID=$(printf '%s' "$UPLOAD_JSON" | jq -r .upload_id)
UPLOAD_JOB_ID=$(printf '%s' "$UPLOAD_JSON" | jq -r '.job_id // empty')
if [[ -n "$UPLOAD_JOB_ID" ]]; then
  wait_job "$UPLOAD_JOB_ID"
fi

# 5. Create inbound HMAC webhook receiver
WEBHOOK_SLUG="iotstream"
WH_JSON=$(api POST /api/axhub/webhooks "{\"dataset_name\":\"$WEBHOOK_SLUG\",\"slug\":\"$WEBHOOK_SLUG\",\"auth_scheme\":\"HMAC_SHA256\",\"record_path\":\"$\"}")
WEBHOOK_SECRET=$(printf '%s' "$WH_JSON" | jq -r .secret)

echo "DEMO_PG_CONN_ID=$CONN_ID"
echo "DEMO_ORDERS_DATASET_ID=$ORDERS_DS_ID"
echo "SUPPLY_UPLOAD_ID=$SUPPLY_UPLOAD_ID"
echo "WEBHOOK_SLUG=$WEBHOOK_SLUG"
echo "WEBHOOK_SECRET=$WEBHOOK_SECRET"
