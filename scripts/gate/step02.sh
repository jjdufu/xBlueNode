#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "$0")/lib.sh"

(cd "$ROOT/axpoint-core" && cargo test -p axpoint-security keyring && cargo test -p axpoint-db axhub)
(cd "$ROOT/axpoint-parser" && pytest -q -m "not heavy" tests/test_connectors_sql.py tests/test_connectors_nosql.py \
   tests/test_connectors_ts.py tests/test_connectors_api.py tests/test_streams.py tests/test_lake_sync.py \
   tests/test_evolve.py tests/test_quality.py tests/test_typemap.py)

# 连接与秘密不回显
expect_json "conn-ok"        "$(api POST /api/axhub/connections/$DEMO_PG_CONN_ID/test)" '.ok == true'
expect_json "secret-hidden"  "$(api GET /api/axhub/connections/$DEMO_PG_CONN_ID)" '(.config | has("password") | not) and (.secret_fields_set | index("password") != null)'
expect_eq   "cipher-at-rest" "$(sql_owner "SELECT count(*) FROM ds_credentials WHERE convert_from(ciphertext,'LATIN1') LIKE '%axpoint%'")" "0"

# 同步与幂等
JOB=$(api POST /api/axhub/datasets/$DEMO_ORDERS_DATASET_ID/sync | jq -r .job_id)
wait_job "$JOB"
expect_json "orders-rows"    "$(api GET /api/axhub/datasets/$DEMO_ORDERS_DATASET_ID)" '.row_count == 100 and .status == "READY"'
expect_json "single-flight"  "$( { api POST /api/axhub/datasets/$DEMO_ORDERS_DATASET_ID/sync; api POST /api/axhub/datasets/$DEMO_ORDERS_DATASET_ID/sync; } | jq -s . )" '.[0].job_id == .[1].job_id'
wait_dataset_idle "$DEMO_ORDERS_DATASET_ID"

# 上传入湖 + 入知识库
expect_json "upload-commit"  "$(api GET /api/axhub/uploads/$SUPPLY_UPLOAD_ID)" '.status == "COMMITTED" and (.dataset_ids | length) == 4 and .doc_id != null'

# Webhook：签名正确 202，错误 401，重复投递不重复入库
BODY='[{"device":"D-1","temp":21.5},{"device":"D-2","temp":22.0}]'
TS=$(date +%s); SIG=$(printf '%s.%s' "$TS" "$BODY" | openssl dgst -sha256 -hmac "$WEBHOOK_SECRET" -hex | awk '{print $2}')
hook() { curl -s -o /dev/null -w '%{http_code}' -X POST "$CORE_BASE_URL/api/axhub/webhooks/default/$WEBHOOK_SLUG/ingest" \
  -H "Content-Type: application/json" -H "X-AxPoint-Timestamp: $TS" -H "X-AxPoint-Delivery: d-1" -H "X-AxPoint-Signature: sha256=$1" -d "$BODY"; }
expect_eq "webhook-ok"   "$(hook "$SIG")" "202"
expect_eq "webhook-dup"  "$(hook "$SIG")" "200"
expect_eq "webhook-bad"  "$(hook deadbeef)" "401"
WH_DS=$(api GET "/api/axhub/datasets?q=$WEBHOOK_SLUG" | jq -r '.items[0].id')
wait_job "$(api POST /api/axhub/datasets/$WH_DS/sync | jq -r .job_id)"
expect_json "webhook-rows" "$(api GET /api/axhub/datasets/$WH_DS)" '.row_count == 2'

# 租户隔离与目录
expect_eq   "rls-ds"         "$(sql_app other "SELECT count(*) FROM ds_datasets")" "0"
expect_json "catalog"        "$(api GET '/api/axhub/catalog/search?q=orders')" '[.items[].name] | index("demo_orders") != null'

gate_pass step02
