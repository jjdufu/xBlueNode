#!/usr/bin/env bash
# Step 29 Gate：常驻微批流秒级同步、LOG_CDC 物理删除捕获、>50万边 DuckDB 图算法下推与 22 工具问答验收闭环
set -euo pipefail
source "$(dirname "$0")/lib.sh"

# 1. 新建表跨租户 RLS 隔离核验
expect_eq "xb-competency-suites-rls" "$(sql_app other "SELECT count(*) FROM xb_competency_suites")" "0"
expect_eq "xb-harness-runs-rls" "$(sql_app other "SELECT count(*) FROM xb_harness_runs")" "0"

RUN_TAG="s29_$$"
KAFKA_BOOTSTRAP="${KAFKA_BOOTSTRAP_SERVERS:-127.0.0.1:19092}"
KAFKA_TOPIC="step29_stream_${RUN_TAG}"
CDC_SRC_TBL="step29_cdc_src_${RUN_TAG}"
BIG_SLUG="step29_big_${RUN_TAG}"
QA_SLUG="dev_step29_qa_${RUN_TAG}"

STREAM_CONN_ID=""
STREAM_DS_ID=""
CDC_CONN_ID=""
CDC_DS_ID=""
CDC_MAP_NAME="step29_cdc_map_${RUN_TAG}"
CDC_MAP_ID=""
BUILDER_PAT_ID=""

cleanup_step29() {
  if [[ -n "$CDC_MAP_NAME" ]]; then
    CUR_MAIN=$(api GET "/api/xblue/schema?subgraph=main" 2>/dev/null || echo "")
    if [[ -n "$CUR_MAIN" ]]; then
      CLEAN_MAIN=$(printf '%s' "$CUR_MAIN" | jq -c --arg mn "$CDC_MAP_NAME" '
        {
          node_types: ((.node_types // []) | map(select(.name != "Step29CdcOrder"))),
          edge_types: (.edge_types // []),
          mappings: ((.mappings // []) | map(select(.name != $mn))),
          base_version: .version,
          change_summary: "cleanup step29 cdc mapping",
          force: true,
          cascade: true
        }
      ' 2>/dev/null || echo "")
      [[ -n "$CLEAN_MAIN" ]] && api POST "/api/xblue/schema?subgraph=main" "$CLEAN_MAIN" >/dev/null 2>&1 || true
    fi
  fi
  [[ -n "$STREAM_DS_ID" && "$STREAM_DS_ID" != "null" ]] && api DELETE "/api/axhub/datasets/$STREAM_DS_ID" >/dev/null 2>&1 || true
  [[ -n "$STREAM_CONN_ID" && "$STREAM_CONN_ID" != "null" ]] && api DELETE "/api/axhub/connections/$STREAM_CONN_ID" >/dev/null 2>&1 || true
  [[ -n "$CDC_DS_ID" && "$CDC_DS_ID" != "null" ]] && api DELETE "/api/axhub/datasets/$CDC_DS_ID" >/dev/null 2>&1 || true
  [[ -n "$CDC_CONN_ID" && "$CDC_CONN_ID" != "null" ]] && api DELETE "/api/axhub/connections/$CDC_CONN_ID" >/dev/null 2>&1 || true
  api DELETE "/api/xblue/subgraphs/$BIG_SLUG" >/dev/null 2>&1 || true
  api DELETE "/api/xblue/subgraphs/$QA_SLUG" >/dev/null 2>&1 || true
  [[ -n "$BUILDER_PAT_ID" && "$BUILDER_PAT_ID" != "null" ]] && api DELETE "/api/auth/tokens/$BUILDER_PAT_ID" >/dev/null 2>&1 || true
  sql_app default "DELETE FROM xb_nodes WHERE type_name = 'Step29CdcOrder'" >/dev/null 2>&1 || true
  sql_owner "DROP TABLE IF EXISTS public.${CDC_SRC_TBL}" >/dev/null 2>&1 || true
}
trap cleanup_step29 EXIT

# ==============================================================================
# 2. C89（上）：常驻秒级微批流消费协程（KAFKA continuous_stream = true，<= 3s 自动落湖）
# ==============================================================================
produce_kafka_msgs() {
  local topic="$1"
  shift
  python - "$KAFKA_BOOTSTRAP" "$topic" "$@" <<'PY'
import sys
from confluent_kafka import Producer

bootstrap = sys.argv[1]
topic = sys.argv[2]
p = Producer({"bootstrap.servers": bootstrap})
for payload in sys.argv[3:]:
    p.produce(topic, payload.encode("utf-8"))
p.flush(5.0)
PY
}

produce_kafka_msgs "$KAFKA_TOPIC" '{"seq":1,"sensor":"S-01","temp":21.5}'

STREAM_CONN_ID=$(api POST /api/axhub/connections "$(jq -nc --arg bs "$KAFKA_BOOTSTRAP" --arg nm "step29_kafka_${RUN_TAG}" '{
  name: $nm,
  connector_type: "KAFKA",
  allow_private_network: true,
  config: { bootstrap_servers: $bs, group_id: ("grp_" + $nm), max_wait_s: 1 }
}')" | jq -r .id)

STREAM_DS_ID=$(api POST /api/axhub/datasets "$(jq -nc --arg cid "$STREAM_CONN_ID" --arg tp "$KAFKA_TOPIC" --arg nm "step29_stream_${RUN_TAG}" '{
  connection_id: $cid,
  name: $nm,
  display_name: "Step29 秒级常驻流",
  source_kind: "STREAM",
  source_ref: { topic: $tp },
  sync_mode: "APPEND",
  continuous_stream: true,
  schema_policy: "ADDITIVE"
}')" | jq -r .id)

# 等待常驻协调器自动认领租约并消费首条预热记录
wait_until "stream-bootstrap-ready" 15 \
  "[[ \$(api GET '/api/axhub/datasets/$STREAM_DS_ID' | jq -r .row_count) -ge 1 ]]"

# 写入 2 条新流消息，断言无需人工触发 /sync，<= 3s 内自动微批刷入 DuckLake（row_count 变为 3）
produce_kafka_msgs "$KAFKA_TOPIC" '{"seq":2,"sensor":"S-02","temp":22.0}' '{"seq":3,"sensor":"S-03","temp":23.5}'

STREAM_OK=0
for _ in $(seq 1 12); do
  CUR_ROWS=$(api GET "/api/axhub/datasets/$STREAM_DS_ID" | jq -r .row_count)
  if [[ "$CUR_ROWS" == "3" ]]; then
    STREAM_OK=1
    break
  fi
  sleep 0.25
done
expect_eq "stream-daemon-subsecond" "$STREAM_OK" "1"

# 停用常驻流避免后续步骤竞争后台队列
api PATCH "/api/axhub/datasets/$STREAM_DS_ID" '{"continuous_stream":false}' >/dev/null

# ==============================================================================
# 2. C89（下）：PostgreSQL LOG_CDC 物理硬删除捕获与关联 xb_nodes 同步清理
# ==============================================================================
sql_owner "
CREATE TABLE public.${CDC_SRC_TBL} (
    order_id TEXT PRIMARY KEY,
    hub_name TEXT NOT NULL,
    qty      DOUBLE PRECISION NOT NULL
);
INSERT INTO public.${CDC_SRC_TBL} (order_id, hub_name, qty) VALUES
    ('ORD-1', '华东履约中心', 100.0),
    ('ORD-2', '华西履约中心', 200.0),
    ('ORD-3', '华南履约中心', 300.0);
GRANT SELECT ON public.${CDC_SRC_TBL} TO axpoint_app;
" >/dev/null

PG_DB_NAME=$(python - "$MIGRATION_DATABASE_URL" <<'PY'
import sys
from urllib.parse import urlparse
u = urlparse(sys.argv[1])
print((u.path or "/postgres").lstrip("/"))
PY
)
PG_HOST=$(python - "$MIGRATION_DATABASE_URL" <<'PY'
import sys
from urllib.parse import urlparse
u = urlparse(sys.argv[1])
print(u.hostname or "127.0.0.1")
PY
)
PG_PORT=$(python - "$MIGRATION_DATABASE_URL" <<'PY'
import sys
from urllib.parse import urlparse
u = urlparse(sys.argv[1])
print(u.port or 5432)
PY
)
PG_USER=$(python - "$MIGRATION_DATABASE_URL" <<'PY'
import sys
from urllib.parse import urlparse
u = urlparse(sys.argv[1])
print(u.username or "axpoint")
PY
)
PG_PASS=$(python - "$MIGRATION_DATABASE_URL" <<'PY'
import sys
from urllib.parse import urlparse
u = urlparse(sys.argv[1])
print(u.password or "")
PY
)

CDC_CONN_ID=$(api POST /api/axhub/connections "$(jq -nc \
  --arg nm "step29_pg_cdc_${RUN_TAG}" \
  --arg host "$PG_HOST" \
  --argjson port "$PG_PORT" \
  --arg db "$PG_DB_NAME" \
  --arg user "$PG_USER" \
  --arg pw "$PG_PASS" '{
    name: $nm,
    connector_type: "POSTGRES",
    allow_private_network: true,
    config: { host: $host, port: $port, database: $db, username: $user, schema: "public", sslmode: "disable" },
    secrets: { password: $pw }
  }')" | jq -r .id)

# 复用固定表名 step29_cdc_orders 供后续黄金问数核验使用（先清理残留同名数据集）
OLD_CDC_DS=$(api GET "/api/axhub/datasets?q=step29_cdc_orders" | jq -r '.items[]? | select(.name=="step29_cdc_orders") | .id' | head -n1)
[[ -n "$OLD_CDC_DS" ]] && api DELETE "/api/axhub/datasets/$OLD_CDC_DS" >/dev/null 2>&1 || true

CDC_DS_ID=$(api POST /api/axhub/datasets "$(jq -nc --arg cid "$CDC_CONN_ID" --arg tbl "$CDC_SRC_TBL" '{
  connection_id: $cid,
  name: "step29_cdc_orders",
  display_name: "第29步履约单",
  source_kind: "SQL_TABLE",
  source_ref: { schema: "public", table: $tbl },
  sync_mode: "LOG_CDC",
  primary_key: ["order_id"],
  schema_policy: "ADDITIVE"
}')" | jq -r .id)

wait_job "$(api POST "/api/axhub/datasets/$CDC_DS_ID/sync" | jq -r .job_id)"
expect_json "cdc-initial-rows" "$(api GET "/api/axhub/datasets/$CDC_DS_ID")" '.row_count == 3 and .sync_mode == "LOG_CDC"'

# 确保 main 子图存在 Step29CdcOrder 类型并建立 on_source_delete = DELETE 映射
MAIN_SCHEMA=$(api GET "/api/xblue/schema?subgraph=main")
UPDATED_MAIN_DOC=$(printf '%s' "$MAIN_SCHEMA" | jq -c --arg mn "$CDC_MAP_NAME" '
  .node_types = ((.node_types // []) | map(select(.name != "Step29CdcOrder")) + [{
    name: "Step29CdcOrder",
    display_name: "履约测试单",
    properties: [
      { name: "hub_name", display_name: "履约仓", data_type: "STRING", kind: "ATTRIBUTE", required: false },
      { name: "qty", display_name: "件数", data_type: "FLOAT64", kind: "METRIC", required: false }
    ],
    agg_rules: [],
    derived_rules: []
  }])
  | .mappings = ((.mappings // []) | map(select(.dataset != "step29_cdc_orders")) + [{
      name: $mn,
      dataset: "step29_cdc_orders",
      node_type: "Step29CdcOrder",
      key_column: "order_id",
      title: "{hub_name}",
      attributes: { hub_name: "hub_name" },
      metrics: { qty: "qty" },
      geo: { lng_column: null, lat_column: null },
      links: [],
      on_source_delete: "DELETE",
      mode: "INCREMENTAL"
    }])
  | {
      node_types: .node_types,
      edge_types: (.edge_types // []),
      mappings: .mappings,
      base_version: .version,
      change_summary: "step29 cdc node type and mapping",
      force: true
    }
')
api POST "/api/xblue/schema?subgraph=main" "$UPDATED_MAIN_DOC" >/dev/null
CDC_MAP_ID=$(api GET /api/xblue/mappings | jq -r --arg mn "$CDC_MAP_NAME" '.items[] | select(.name == $mn) | .id')
wait_mapping_idle "$CDC_MAP_ID"
expect_eq "cdc-mapped-before" "$(sql_app default "SELECT count(*) FROM xb_nodes WHERE type_name = 'Step29CdcOrder'")" "3"

# 在源表执行物理硬删除 ORD-2 并触发 LOG_CDC 同步
sql_owner "DELETE FROM public.${CDC_SRC_TBL} WHERE order_id = 'ORD-2'" >/dev/null
wait_job "$(api POST "/api/axhub/datasets/$CDC_DS_ID/sync" | jq -r .job_id)"

CDC_PREVIEW=$(api GET "/api/axhub/datasets/$CDC_DS_ID/preview?limit=10")
REMAIN_NODES=$(sql_app default "SELECT count(*) FROM xb_nodes WHERE type_name = 'Step29CdcOrder'")
ORD2_REMAIN=$(sql_app default "SELECT count(*) FROM xb_nodes WHERE type_name = 'Step29CdcOrder' AND node_key = 'ORD-2'")
expect_json "cdc-hard-delete-captured" "$CDC_PREVIEW" \
  "((.rows | length) == 2) and (([.rows[].order_id] | index(\"ORD-2\")) == null) and ($REMAIN_NODES == 2) and ($ORD2_REMAIN == 0)"

# ==============================================================================
# 3. C90：构造 550,000 条边的测试子图运行 ConnectedComponents -> DUCKDB_PUSHDOWN
# ==============================================================================
api POST /api/xblue/subgraphs "{\"slug\":\"$BIG_SLUG\",\"display_name\":\"Step29 大图下推子图\",\"is_dev_sandbox\":true}" >/dev/null

sql_app default "
WITH ins_nodes AS (
    INSERT INTO xb_nodes (id, tenant_id, subgraph_slug, type_name, node_key, title)
    SELECT ('00000000-0000-4000-8000-' || lpad(to_hex(i), 12, '0'))::uuid,
           'default',
           '${BIG_SLUG}',
           'HubNode',
           'H-' || i::text,
           '枢纽-' || i::text
      FROM generate_series(1, 4000) AS g(i)
    RETURNING id
)
INSERT INTO xb_links (tenant_id, subgraph_slug, link_type, src_node_id, dst_node_id, weight)
SELECT 'default',
       '${BIG_SLUG}',
       'FLOWS_TO',
       ('00000000-0000-4000-8000-' || lpad(to_hex(u), 12, '0'))::uuid,
       ('00000000-0000-4000-8000-' || lpad(to_hex(((u - 1 + step) % 4000) + 1), 12, '0'))::uuid,
       1.0
  FROM generate_series(1, 138) AS s(step)
  CROSS JOIN generate_series(1, 4000) AS n(u)
 LIMIT 550000
" >/dev/null

ALGO_RES=$(api POST /api/xblue/graph/algorithms/CONNECTED_COMPONENTS "{\"subgraph\":\"$BIG_SLUG\",\"params\":{\"weak\":true}}")
expect_json "duckdb-graph-algo-pushdown" "$ALGO_RES" \
  '.backend == "DUCKDB_PUSHDOWN" and .edge_count == 550000 and .node_count == 4000 and (.top | length) == 50'

# ==============================================================================
# 4. C91：BUILDER 令牌 22 工具发现 + 黄金能力问答验收闭环（缺口诊断 -> 修复 -> 全通过）
# ==============================================================================
TOK_RES=$(api POST /api/auth/tokens "{
  \"name\": \"gate-step29-builder-${RUN_TAG}\",
  \"expires_in_days\": 7,
  \"token_profile\": \"BUILDER\"
}")
BUILDER_PAT=$(printf '%s' "$TOK_RES" | jq -r .token)
BUILDER_PAT_ID=$(printf '%s' "$TOK_RES" | jq -r .id)

mkdir -p "$ROOT/.gate"
curl -sS -X POST "$CORE_BASE_URL/api/mcp" \
  -H "Authorization: Bearer $BUILDER_PAT" \
  -H "Accept: application/json, text/event-stream" \
  -H "Content-Type: application/json" \
  -D "$ROOT/.gate/mcp29.h" \
  -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"gate-step29","version":"1.0"}}}' >/dev/null
SID29=$(awk 'tolower($1)=="mcp-session-id:" {gsub(/\r/,"",$2); print $2}' "$ROOT/.gate/mcp29.h")

mcp_builder_raw() {
  curl -sS -X POST "$CORE_BASE_URL/api/mcp" \
    -H "Authorization: Bearer $BUILDER_PAT" \
    -H "Accept: application/json, text/event-stream" \
    -H "Content-Type: application/json" \
    -H "Mcp-Session-Id: $SID29" \
    -d "$1" | sed -n 's/^data: //p;/^{/p' | tail -1
}

mcp_builder_raw '{"jsonrpc":"2.0","method":"notifications/initialized"}' >/dev/null || true

mcp_builder_call() {
  local method="$1"
  local empty_json='{}'
  local params="${2:-$empty_json}"
  local payload
  payload=$(jq -nc --arg m "$method" --argjson p "$params" '{jsonrpc:"2.0",id:2,method:$m,params:$p}')
  mcp_builder_raw "$payload"
}

MCP_TOOLS=$(mcp_builder_call "tools/list" '{}')
expect_json "axbuilder-22-tools" "$MCP_TOOLS" '(.result.tools | length) == 22'

# 创建开发沙盒子图并推送初始模型草案（暂缺 turnover_qty 派生指标以验证 GAP_DETECTED）
api POST /api/xblue/subgraphs "{\"slug\":\"$QA_SLUG\",\"display_name\":\"Step29 能力验收沙盒\",\"is_dev_sandbox\":true}" >/dev/null

PUSH1_RPC=$(mcp_builder_call "tools/call" "$(jq -nc --arg slug "$QA_SLUG" '{
  name: "axbuilder_push_schema_draft",
  arguments: {
    subgraph_slug: $slug,
    title: "初始履约枢纽草案",
    schema_doc: {
      node_types: [{
        name: "Step29Hub",
        display_name: "履约枢纽",
        properties: [
          { name: "hub_name", display_name: "枢纽名称", data_type: "STRING", kind: "ATTRIBUTE", required: true },
          { name: "qty", display_name: "库存件数", data_type: "FLOAT64", kind: "METRIC", required: false }
        ],
        agg_rules: [],
        derived_rules: []
      }],
      edge_types: [],
      mappings: [{
        name: "cdc_orders_to_step29_hub",
        dataset: "step29_cdc_orders",
        node_type: "Step29Hub",
        key_column: "order_id",
        title: "{hub_name}",
        attributes: { hub_name: "hub_name" },
        metrics: { qty: "qty" },
        geo: { lng_column: null, lat_column: null },
        links: [],
        on_source_delete: "DELETE",
        mode: "FULL"
      }]
    }
  }
}')")
DRAFT1_ID=$(printf '%s' "$PUSH1_RPC" | jq -r '.result.content[0].text | fromjson | .draft_id')

# 登记 4 道黄金能力问数题
SUITE_QUESTIONS='[
  {
    "id": "cq_active_cnt",
    "question": "统计第29步履约单有效记录总数",
    "expected_grounding_mode": "GUARDED_SQL",
    "expected_number": 2.0,
    "tolerance": 0.001,
    "must_not_abstain": true
  },
  {
    "id": "cq_sum_qty",
    "question": "汇总第29步履约单在库件数总和",
    "expected_metrics": ["qty"],
    "expected_number": 400.0,
    "tolerance": 0.001,
    "must_not_abstain": true
  },
  {
    "id": "cq_max_qty",
    "question": "查询第29步履约单单仓最高件数",
    "expected_node_types": ["Step29Hub"],
    "expected_number": 300.0,
    "tolerance": 0.001,
    "must_not_abstain": true
  },
  {
    "id": "cq_turnover",
    "question": "计算第29步履约枢纽目标周转件数",
    "expected_metrics": ["turnover_qty"],
    "expected_number": 200.0,
    "tolerance": 0.001,
    "must_not_abstain": true
  }
]'

mcp_builder_call "tools/call" "$(jq -nc --arg slug "$QA_SLUG" --argjson qs "$SUITE_QUESTIONS" '{
  name: "axbuilder_upsert_competency_suite",
  arguments: { subgraph_slug: $slug, questions: $qs }
}')" >/dev/null

# 第 1 轮核验：因缺少 turnover_qty 指标，精确产出 GAP_DETECTED (MISSING_METRIC)
VERIFY1_RPC=$(mcp_builder_call "tools/call" "$(jq -nc --arg slug "$QA_SLUG" --arg did "$DRAFT1_ID" '{
  name: "axbuilder_verify_competency_suite",
  arguments: { subgraph_slug: $slug, draft_id: $did, run_dev_materialize: true }
}')")
VERIFY1_BODY=$(printf '%s' "$VERIFY1_RPC" | jq -r '.result.content[0].text')
expect_json "competency-gap-detected" "$VERIFY1_BODY" \
  '.status == "GAP_DETECTED" and .total_questions == 4 and .passed_count == 3 and (.gap_diagnostics[0].gap_kind == "MISSING_METRIC") and (.gap_diagnostics[0].target_ref == "turnover_qty")'

# 按诊断建议补充 turnover_qty 派生指标并重跑验收闭环 -> 4/4 PASS
mcp_builder_call "tools/call" "$(jq -nc --arg slug "$QA_SLUG" --arg did "$DRAFT1_ID" '{
  name: "axbuilder_push_schema_draft",
  arguments: {
    subgraph_slug: $slug,
    draft_id: $did,
    title: "补齐周转指标草案",
    schema_doc: {
      node_types: [{
        name: "Step29Hub",
        display_name: "履约枢纽",
        properties: [
          { name: "hub_name", display_name: "枢纽名称", data_type: "STRING", kind: "ATTRIBUTE", required: true },
          { name: "qty", display_name: "库存件数", data_type: "FLOAT64", kind: "METRIC", required: false },
          { name: "turnover_qty", display_name: "目标周转件数", data_type: "FLOAT64", kind: "METRIC", required: false }
        ],
        agg_rules: [],
        derived_rules: [
          { target_metric: "turnover_qty", expression: "qty * 0.5", enabled: true }
        ]
      }],
      edge_types: [],
      mappings: [{
        name: "cdc_orders_to_step29_hub",
        dataset: "step29_cdc_orders",
        node_type: "Step29Hub",
        key_column: "order_id",
        title: "{hub_name}",
        attributes: { hub_name: "hub_name" },
        metrics: { qty: "qty" },
        geo: { lng_column: null, lat_column: null },
        links: [],
        on_source_delete: "DELETE",
        mode: "FULL"
      }]
    }
  }
}')" >/dev/null

VERIFY2_RPC=$(mcp_builder_call "tools/call" "$(jq -nc --arg slug "$QA_SLUG" --arg did "$DRAFT1_ID" '{
  name: "axbuilder_verify_competency_suite",
  arguments: { subgraph_slug: $slug, draft_id: $did, run_dev_materialize: true }
}')")
VERIFY2_BODY=$(printf '%s' "$VERIFY2_RPC" | jq -r '.result.content[0].text')
expect_json "competency-qa-harness" "$VERIFY2_BODY" \
  '.status == "PASS" and .total_questions == 4 and .passed_count == 4 and (.gap_diagnostics | length) == 0'

expect_eq "harness-verify-audit" \
  "$(sql_app default "SELECT count(*) > 0 FROM sec_audit_logs WHERE action = 'HARNESS_VERIFY_RUN'")" "t"

gate_pass step29
