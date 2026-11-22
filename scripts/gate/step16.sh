#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "$0")/lib.sh"

# 0. 迁移完整性核验
expect_json "step16-migrations-clean" "$(api GET /api/axops/health/deep)" '.migrations.pending == 0'

# 1. 领域接地核验器、本体连接守卫与双轨 50 例反幻觉基准测试（拦截率必须为 100%）
(
  cd "$ROOT/axpoint-core"
  cargo test -p axpoint-domain grounding --locked
  cargo test -p axpoint-analytics ontology_route --locked
  cargo test -p axpoint-analytics sql_guard --locked
  cargo test -p axpoint-analytics --test hallucination_eval --locked
)

# 触发管理端双轨基准持久化接口并核验云端轨 + 本地 7B 轨 50/50 100% 拦截与跨租户 RLS 隔离
EVAL_RESP=$(api POST /api/analytics/grounding-eval/run '{}')
expect_json "hallucination-eval-100" "$EVAL_RESP" \
  '(.runs | length) == 2 and ([.runs[].pass_rate] | all(. == 1.0)) and ([.runs[].intercepted_cases] | all(. == 50))'
expect_eq "grounding-eval-rls" "$(sql_app other "SELECT count(*) FROM an_grounding_eval_runs")" "0"

# 2. 第 0 阶段本体与认证指标优先路由：命中 paid_amount 认证指标时走 SEMANTIC_METRIC，且与直接调用指标语义接口结果完全一致
ASK_METRIC=$(api POST /api/analytics/ask '{"question":"按状态统计已支付订单金额（按月汇总）"}')
expect_json "ontology-metric-route" "$ASK_METRIC" \
  '.grounding_mode == "SEMANTIC_METRIC" and .abstained == false and (.slot_payload != null) and (.rows | length) >= 1'

DIRECT_METRIC=$(api POST /api/analytics/metrics/paid_amount/query '{"dimensions":["status"],"grain":"MONTH"}')
expect_eq "slot-compile-exact" \
  "$(printf '%s' "$ASK_METRIC" | jq -c '.rows')" \
  "$(printf '%s' "$DIRECT_METRIC" | jq -c '.rows')"
expect_eq "step16-ask-audit" "$(sql_app default "SELECT count(*) > 0 FROM sec_audit_logs WHERE action = 'ASK_DATA'")" "t"

# 准备幂等测试夹具（demo_warehouses 数据集 + 沙盒子图映射 + 补货审批制度文档），退出时清理临时子图与文档
STEP16_SUB_SLUG="dev_step16_join"
STEP16_DOC_ID=""
STEP16_CREATED_DS_ID=""

cleanup_step16() {
  api DELETE "/api/xblue/subgraphs/$STEP16_SUB_SLUG" >/dev/null 2>&1 || true
  if [[ -n "$STEP16_DOC_ID" && "$STEP16_DOC_ID" != "null" ]]; then
    api DELETE "/api/documents/$STEP16_DOC_ID" >/dev/null 2>&1 || true
  fi
  if [[ -n "$STEP16_CREATED_DS_ID" && "$STEP16_CREATED_DS_ID" != "null" ]]; then
    api DELETE "/api/axhub/datasets/$STEP16_CREATED_DS_ID" >/dev/null 2>&1 || true
  fi
}
trap cleanup_step16 EXIT

sql_owner "
CREATE SCHEMA IF NOT EXISTS demo_src;
CREATE TABLE IF NOT EXISTS demo_src.warehouses (
    warehouse_code TEXT PRIMARY KEY,
    warehouse_id   TEXT NOT NULL,
    region         TEXT NOT NULL,
    title          TEXT NOT NULL
);
INSERT INTO demo_src.warehouses (warehouse_code, warehouse_id, region, title) VALUES
    ('W-1', 'W-1', '华东', '华东仓'),
    ('W-2', 'W-2', '华南', '华南仓')
ON CONFLICT (warehouse_code) DO NOTHING;
GRANT USAGE ON SCHEMA demo_src TO axpoint_app;
GRANT SELECT ON ALL TABLES IN SCHEMA demo_src TO axpoint_app;
" >/dev/null

EXISTING_WH_DS=$(api GET "/api/axhub/datasets?q=demo_warehouses" | jq -r '.items[]? | select(.name=="demo_warehouses") | .id' | head -n1)
if [[ -z "$EXISTING_WH_DS" ]]; then
  PG_CONN_ID="${DEMO_PG_CONN_ID:-$(api GET /api/axhub/connections | jq -r '.items[] | select(.name=="demo_pg") | .id' | head -n1)}"
  WH_DS_RESP=$(api POST /api/axhub/datasets "$(jq -nc --arg cid "$PG_CONN_ID" '{
    connection_id: $cid,
    name: "demo_warehouses",
    display_name: "演示仓库表",
    description: "区域履约仓库主数据",
    source_kind: "SQL_TABLE",
    source_ref: { schema: "demo_src", table: "warehouses" },
    sync_mode: "SNAPSHOT",
    primary_key: ["warehouse_code"],
    schema_policy: "ADDITIVE",
    tags: ["demo", "warehouses"]
  }')")
  STEP16_CREATED_DS_ID=$(printf '%s' "$WH_DS_RESP" | jq -r .id)
  WH_SYNC_JOB=$(api POST "/api/axhub/datasets/$STEP16_CREATED_DS_ID/sync" | jq -r .job_id)
  wait_job "$WH_SYNC_JOB"
fi

api DELETE "/api/xblue/subgraphs/$STEP16_SUB_SLUG" >/dev/null 2>&1 || true
api POST /api/xblue/subgraphs "{\"slug\":\"$STEP16_SUB_SLUG\",\"display_name\":\"Step16 连接守卫验证\",\"is_dev_sandbox\":true}" >/dev/null
api POST "/api/xblue/schema?subgraph=$STEP16_SUB_SLUG" '{
  "node_types": [
    {
      "name": "Warehouse",
      "display_name": "仓库",
      "properties": [
        {"name": "region", "display_name": "区域", "data_type": "STRING", "kind": "ATTRIBUTE", "required": false, "pii_category": "NONE"}
      ]
    }
  ],
  "edge_types": [],
  "mappings": [
    {
      "name": "warehouses_to_warehouse_nodes",
      "dataset": "demo_warehouses",
      "node_type": "Warehouse",
      "key_column": "warehouse_code",
      "title": "{title}",
      "attributes": {"region": "region"},
      "metrics": {},
      "links": [],
      "on_source_delete": "KEEP",
      "mode": "FULL"
    }
  ]
}' >/dev/null

# 3. 多表本体外键路径守卫：无 MappingLinkSpec 依据的自由跨表 JOIN 返回 400 JOIN_PATH_UNGROUNDED；已建模关联边正常放行
UNGROUNDED_SQL=$(api POST /api/analytics/sql/run '{
  "sql":"SELECT o.order_id, w.warehouse_code FROM demo_orders o JOIN demo_warehouses w ON CAST(o.amount AS VARCHAR) = w.region",
  "strict_ontology_join":true
}')
expect_json "ungrounded-join-rejected" "$UNGROUNDED_SQL" '.error_code == "JOIN_PATH_UNGROUNDED"'

MAPPED_SQL=$(api POST /api/analytics/sql/run '{
  "sql":"SELECT o.order_id, w.warehouse_code FROM demo_orders o JOIN demo_warehouses w ON o.warehouse_code = w.warehouse_code LIMIT 5",
  "strict_ontology_join":true
}')
expect_json "mapped-join-ok" "$MAPPED_SQL" '(.rows | type) == "array" and (.columns | length) == 2'

# 4. 引用角标越界剥离核验：注入补货审批制度文档并核验 DOC 问答剥离越界角标 [文99]
INGEST_DOC=$(api POST /api/documents/ingest '{
  "title": "补货审批制度",
  "source_name": "replenishment_policy_step16.md",
  "content": "第一条 补货审批制度触发条件：当区域仓在库库存低于安全库存阈值 500 件，或预测未来 7 日缺口大于 200 件时，自动触发紧急补货审批流程。\n\n第二条 紧急补货单须经区域仓储主管与供应链总监双级签批后方可下发执行。"
}')
STEP16_DOC_ID=$(printf '%s' "$INGEST_DOC" | jq -r .doc_id)

DOC_ANS=$(api POST /api/chat/query '{"query":"补货审批制度的具体触发条件是什么？","mode":"DOC","top_k":3}')
expect_json "phantom-citation-stripped" "$DOC_ANS" \
  '.grounding_mode == "GROUNDED_DOC" and ((.citations | length) as $max_c | $max_c >= 1 and ([.answer | scan("\\[文([0-9]+)\\]") | .[0] | tonumber] | all(. >= 1 and . <= $max_c)))'

# 5. 零召回确定性拒答：询问完全不存在的制度条款，五路召回为空时必须返回 ABSTAINED 且不编造引用
EMPTY_ANS=$(api POST /api/chat/query '{"query":"反物质曲率引擎反应堆舱门维护规程第88条是什么？","mode":"DOC","top_k":5}')
expect_json "empty-recall-abstain" "$EMPTY_ANS" \
  '.grounding_mode == "ABSTAINED" and .abstained == true and (.citations | length) == 0 and .verified_citations == 0'

gate_pass step16
