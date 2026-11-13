#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "$0")/lib.sh"

(cd axpoint-core && cargo test -p axpoint-domain algo graph && cargo test -p axpoint-db nodes metrics rollup graph_query entity_link)

# 幂等写入：重复写同一关系不产生重复
api POST /api/xblue/links "{\"link_type\":\"FULFILLS\",\"src_node_id\":\"$WAREHOUSE_NODE_ID\",\"dst_node_id\":\"$ORDER1_NODE_ID\",\"weight\":0.8}" >/dev/null
expect_eq "link-unique" "$(sql_app default "SELECT count(*) FROM xb_links WHERE src_node_id='$WAREHOUSE_NODE_ID' AND dst_node_id='$ORDER1_NODE_ID' AND link_type='FULFILLS'")" "1"

# 指标合并而非覆盖
api POST /api/xblue/nodes '{"type_name":"Warehouse","node_key":"W-1","title":"华东仓","metrics":{"capacity":900}}' >/dev/null
expect_json "metrics-merge" "$(api GET /api/xblue/nodes/$WAREHOUSE_NODE_ID)" '.metrics.capacity == 900 and (.metrics | has("inventory"))'

# 汇总：日粒度 SUM 与种子数据的精确值一致（每日 24 点，值 100..123，和 = 2676）
wait_job "$(api POST /api/axops/jobs/rollup-now | jq -r .job_id)"
expect_json "rollup-day" "$(api GET "/api/xblue/nodes/$WAREHOUSE_NODE_ID/metrics/inventory?from=$SEED_DAY1&to=$SEED_DAY2&grain=DAY&agg=SUM")" '.[0].value == 2676'
expect_json "window-sliding" "$(api POST /api/xblue/metrics/window "{\"node_ids\":[\"$WAREHOUSE_NODE_ID\"],\"metric\":\"inventory\",\"from\":\"$SEED_DAY1\",\"to\":\"$SEED_DAY2\",\"window\":{\"kind\":\"SLIDING\",\"size\":\"3h\",\"step\":\"1h\"},\"agg\":\"AVG\"}")" '(.items | length) == 24'

# 检索与地理
expect_json "node-search" "$(api GET '/api/xblue/nodes/search?q=华东')" '.items[0].node_key == "W-1"'
expect_json "geo-bbox"    "$(api GET '/api/xblue/nodes?bbox=120,30,122,32&type_name=Factory')" '(.items | length) >= 1'

# 实体链接
api POST /api/xblue/entities/resolve '{}' >/dev/null
expect_json "entity-link" "$(api GET /api/xblue/nodes/$WAREHOUSE_NODE_ID/documents)" '(.items | length) >= 1'

# 图查询与算法
expect_json "neighbors" "$(api GET "/api/xblue/graph/neighbors?node_id=$WAREHOUSE_NODE_ID&depth=1&direction=BOTH")" '(.nodes | length) >= 3'
expect_json "path"      "$(api GET "/api/xblue/graph/path?from=$FACTORY1_NODE_ID&to=$ORDER20_NODE_ID&max_depth=4")" '(.path | length) == 3'
expect_json "pagerank"  "$(api POST /api/xblue/graph/algorithms/PAGERANK '{"params":{}}')" '.node_count >= 25 and (.top | length) > 0'

# 行级安全覆盖分区子表
expect_eq "rls-partition" "$(sql_app other "SELECT count(*) FROM xb_metric_series")" "0"

gate_pass step03
