#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "$0")/lib.sh"

(cd axpoint-core && cargo test -p axpoint-domain expr && cargo test -p axpoint-domain rollup && cargo test -p axpoint-db mapping_exec && cargo test -p axpoint-db aggregate_exec && cargo test -p axpoint-db knowledge_sync)

# 映射物化：100 个订单节点
wait_job "$(api POST "/api/xblue/mappings/$ORDER_MAPPING_ID/run" '{"mode":"FULL"}' | jq -r .job_id)"
expect_eq "mapped-orders" "$(sql_app default "SELECT count(*) FROM xb_nodes WHERE type_name='Order' AND source_dataset_id='$DEMO_ORDERS_DATASET_ID'")" "100"
expect_eq "mapped-links"  "$(sql_app default "SELECT count(*) FROM xb_links l JOIN xb_nodes o ON o.id=l.dst_node_id WHERE l.link_type='FULFILLS' AND o.source_dataset_id='$DEMO_ORDERS_DATASET_ID'")" "100"

# 聚合与数据库对账：W-1 的 total_demand = 其 FULFILLS 关系指向的全部订单 demand 之和（独立 SQL 计算）
wait_aggregate_idle
EXP=$(sql_app default "SELECT sum((o.metrics->>'demand')::numeric) FROM xb_links l JOIN xb_nodes o ON o.id=l.dst_node_id WHERE l.src_node_id='$WAREHOUSE_NODE_ID' AND l.link_type='FULFILLS'" | awk '{printf "%.2f", $1}')
ACT=$(api GET "/api/xblue/nodes/$WAREHOUSE_NODE_ID" | jq -r '.metrics.total_demand' | awk '{printf "%.2f", $1}')
expect_eq "agg-reconcile" "$ACT" "$EXP"

# 增量：源改 1 行金额 → 同步 → 映射增量 → 聚合自动刷新
sql_owner "UPDATE demo_src.orders SET amount = amount + 1000, updated_at = now() WHERE order_id = 2" >/dev/null
api PATCH "/api/axhub/datasets/$DEMO_ORDERS_DATASET_ID" '{"sync_mode":"UPSERT","primary_key":["order_id"],"cursor_column":"updated_at"}' >/dev/null
wait_job "$(api POST "/api/axhub/datasets/$DEMO_ORDERS_DATASET_ID/sync" | jq -r .job_id)"
wait_mapping_idle "$ORDER_MAPPING_ID"; wait_aggregate_idle
ACT2=$(api GET "/api/xblue/nodes/$WAREHOUSE_NODE_ID" | jq -r '.metrics.total_demand' | awk '{printf "%.2f", $1}')
expect_eq "agg-incremental" "$(awk -v a="$ACT2" -v b="$ACT" 'BEGIN {printf "%.2f", a - b}')" "1000.00"
expect_json "incremental-mode" "$(api GET "/api/xblue/mappings/$ORDER_MAPPING_ID/runs")" '.items[0].mode == "INCREMENTAL"'

# 派生指标与可解释性
expect_json "derived" "$(api GET "/api/xblue/nodes/$WAREHOUSE_NODE_ID/explain/fill_rate")" '.producer == "DERIVED" and (.inputs | length) == 2'

# 表达式校验
expect_json "expr-chain-cmp" "$(api POST /api/xblue/expr/validate '{"type_name":"Warehouse","expression":"1 < 2 < 3"}')" '(.issues | length) >= 1'

gate_pass step06
