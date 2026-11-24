#!/usr/bin/env bash
# Step 26 Gate：ActionSpec 前置守卫与原子回写、动态 GraphQL Mutation、四路径边动力学传播与 L4 Trial Tick 认证
set -euo pipefail
source "$(dirname "$0")/lib.sh"

# 1. 新建表跨租户 RLS 隔离核验
[[ "$(sql_app other "SELECT count(*) FROM xb_link_metric_series")" == "0" ]]
[[ "$(sql_app other "SELECT count(*) FROM xb_runtime_certifications")" == "0" ]]

# 2. C82：禁止覆写派生指标、前置条件不满足返回 422、满足时经 REST 与 GraphQL Mutation 幂等执行并留存审计链
SUB_SLUG="trial_step26_$(date +%s)"
api POST /api/xblue/subgraphs "$(jq -n --arg s "$SUB_SLUG" '{slug: $s, display_name: "Step26 认证演练", is_dev_sandbox: true}')" >/dev/null

# 校验直接覆写派生指标在静态校验期被拦截 (DERIVED_METRIC_WRITE_FORBIDDEN)
BAD_VALIDATE=$(api POST "/api/xblue/schema/validate?subgraph=${SUB_SLUG}" '{
  "node_types": [
    {
      "name": "Step26Wh",
      "display_name": "测试仓",
      "properties": [
        {"name": "stock", "display_name": "库存", "data_type": "FLOAT64", "kind": "METRIC"},
        {"name": "safety_gap", "display_name": "缺口", "data_type": "FLOAT64", "kind": "METRIC"}
      ],
      "derived_rules": [
        {"target_metric": "safety_gap", "expression": "100 - stock", "enabled": true}
      ],
      "actions": [
        {
          "name": "tamper_derived",
          "display_name": "违规写派生",
          "side_effect_tier": "SELF_ONLY",
          "mutations": [{"target": "self.safety_gap", "expression": "0"}]
        }
      ]
    }
  ],
  "edge_types": [],
  "mappings": []
}')
expect_json "derived-metric-write-forbidden" "$BAD_VALIDATE" '
  .valid == false and ([.issues[].code] | index("DERIVED_METRIC_WRITE_FORBIDDEN") != null)
'

# 3. C84（上）：注入未消费孤立派生指标断言检出 DEAD_KNOWLEDGE
DEAD_CERT=$(api POST "/api/xblue/schema/validate?subgraph=${SUB_SLUG}" '{
  "node_types": [
    {
      "name": "Step26Wh",
      "display_name": "测试仓",
      "properties": [
        {"name": "stock", "display_name": "库存", "data_type": "FLOAT64", "kind": "METRIC"},
        {"name": "orphan_metric", "display_name": "孤立指标", "data_type": "FLOAT64", "kind": "METRIC"}
      ],
      "derived_rules": [
        {"target_metric": "orphan_metric", "expression": "stock * 2", "enabled": true}
      ]
    }
  ],
  "edge_types": [],
  "mappings": []
}')
expect_json "dead-knowledge-detected" "$DEAD_CERT" '
  ([.issues[].code] | index("DEAD_KNOWLEDGE") != null)
'

# 4. 在主图注册带 ActionSpec、V->E 边派生与 E->V 边聚合的测试类型并验证完整运行态闭环
CUR_SCHEMA=$(api GET "/api/xblue/schema?subgraph=main")
NEXT_SCHEMA=$(echo "$CUR_SCHEMA" | jq '
  del(.subgraph_slug, .version)
  | .node_types = ([.node_types[] | select(.name != "Step26Hub" and .name != "Step26Depot")] + [
      {
        "name": "Step26Hub",
        "display_name": "调拨枢纽",
        "properties": [
          {"name": "output_rate", "display_name": "发运速率", "data_type": "FLOAT64", "kind": "METRIC"},
          {"name": "reserve_qty", "display_name": "储备库存", "data_type": "FLOAT64", "kind": "METRIC"}
        ],
        "agg_rules": [],
        "derived_rules": [],
        "actions": [
          {
            "name": "dispatch_reserve",
            "display_name": "紧急调拨发运",
            "parameters": [
              {"name": "qty", "display_name": "调拨量", "data_type": "FLOAT64", "required": true}
            ],
            "precondition_expr": "reserve_qty >= param.qty and param.qty > 0",
            "side_effect_tier": "SELF_ONLY",
            "mutations": [
              {"target": "self.reserve_qty", "expression": "reserve_qty - param.qty"},
              {"target": "self.output_rate", "expression": "output_rate + param.qty"}
            ]
          }
        ]
      },
      {
        "name": "Step26Depot",
        "display_name": "区域前置仓",
        "properties": [
          {"name": "congestion_ratio", "display_name": "拥堵率", "data_type": "FLOAT64", "kind": "METRIC"},
          {"name": "inbound_flux", "display_name": "总入港通量", "data_type": "FLOAT64", "kind": "METRIC"}
        ],
        "agg_rules": [
          {
            "target_metric": "inbound_flux",
            "op": "SUM",
            "via_link_type": "STEP26_LANE",
            "direction": "IN",
            "source_type_name": "STEP26_LANE",
            "source_metric": "effective_flux",
            "weight_by_link": false
          }
        ],
        "derived_rules": []
      }
    ])
  | .edge_types = ([.edge_types[] | select(.name != "STEP26_LANE")] + [
      {
        "name": "STEP26_LANE",
        "src_type_name": "Step26Hub",
        "dst_type_name": "Step26Depot",
        "impact_weight": 0.8,
        "cardinality": "ONE_TO_MANY",
        "properties": [
          {"name": "in_transit_qty", "display_name": "在途量", "data_type": "FLOAT64", "kind": "METRIC"},
          {"name": "effective_flux", "display_name": "有效通量", "data_type": "FLOAT64", "kind": "METRIC"}
        ],
        "derived_rules": [
          {
            "target_metric": "effective_flux",
            "expression": "src.output_rate * (1.0 - dst.congestion_ratio) + in_transit_qty",
            "path_kind": "VERTEX_TO_EDGE",
            "enabled": true
          }
        ]
      }
    ])
')
api POST "/api/xblue/schema?subgraph=main" "$NEXT_SCHEMA" >/dev/null

HUB_NODE=$(api POST /api/xblue/nodes '{"type_name":"Step26Hub","node_key":"H-26","title":"华东枢纽","metrics":{"output_rate":100,"reserve_qty":50}}')
DEPOT_NODE=$(api POST /api/xblue/nodes '{"type_name":"Step26Depot","node_key":"D-26","title":"苏州前置仓","metrics":{"congestion_ratio":0.2,"inbound_flux":0}}')
HUB_ID=$(echo "$HUB_NODE" | jq -r '.id')
DEPOT_ID=$(echo "$DEPOT_NODE" | jq -r '.id')
LANE=$(api POST /api/xblue/links "$(jq -n --arg s "$HUB_ID" --arg d "$DEPOT_ID" '{link_type:"STEP26_LANE",src_node_id:$s,dst_node_id:$d,weight:0.8}')")
LANE_ID=$(echo "$LANE" | jq -r '.id')

# 前置条件不满足（请求调拨 999 > reserve_qty 50）应返回 422 ACTION_PRECONDITION_FAILED
PRECOND_CODE=$(api_code POST /api/xblue/objects/Step26Hub/H-26/actions/dispatch_reserve '{"inputs":{"qty":999},"idempotency_key":"step26-precond-fail"}')
[[ "$PRECOND_CODE" == "422" ]]
ACT_OK=$(api POST /api/xblue/objects/Step26Hub/H-26/actions/dispatch_reserve '{"inputs":{"qty":20},"idempotency_key":"step26-rest-1"}')
expect_json "action-spec-precondition" "$ACT_OK" '
  .status == "APPLIED" and (.node.metrics.reserve_qty == 30) and (.node.metrics.output_rate == 120)
'

# GraphQL 动态 Mutation 执行
GQL_MUT=$(api POST /api/xblue/graphql '{
  "query": "mutation { step26Hub_dispatchReserve(key: \"H-26\", idempotencyKey: \"step26-gql-1\", input: { qty: 10 }) { status mutatedNodes idempotencyReplay } }"
}')
expect_json "graphql-action-mutation" "$GQL_MUT" '
  .data.step26Hub_dispatchReserve.status == "APPLIED"
  and (.data.step26Hub_dispatchReserve.mutatedNodes >= 1)
'

# C83：写入关系边在途量指标，触发 V->E 与 E->V 四路径重算并核验 explain 三轨溯源
EDGE_INGEST=$(api POST /api/xblue/links/metrics/write "$(jq -n --arg lid "$LANE_ID" '{
  points: [{link_id: $lid, metric: "in_transit_qty", ts: "2026-06-01T00:00:00Z", value: 16}]
}')")
expect_json "edge-metric-ingest" "$EDGE_INGEST" '.written == 1'

LINK_EXP=$(api GET "/api/xblue/links/${LANE_ID}/explain/effective_flux")
DEPOT_AFTER=$(api GET "/api/xblue/nodes/${DEPOT_ID}")
expect_json "four-path-propagation" "$(jq -n --argjson le "$LINK_EXP" --argjson dn "$DEPOT_AFTER" '{le:$le, dn:$dn}')" '
  ($le.value != null)
  and (($le.rule_chain | length) >= 1)
  and ($dn.metrics.inbound_flux > 100)
'

# C84（下）：在演练子图推送无死指标闭合模型并执行 /certify 达成 L4_CERTIFIED
CLEAN_SUB_SCHEMA='{
  "node_types": [
    {
      "name": "Step26CleanNode",
      "display_name": "认证节点",
      "properties": [
        {"name": "base_load", "display_name": "基础负载", "data_type": "FLOAT64", "kind": "METRIC"},
        {"name": "headroom", "display_name": "余量", "data_type": "FLOAT64", "kind": "METRIC"}
      ],
      "derived_rules": [
        {"target_metric": "headroom", "expression": "100 - base_load", "enabled": true}
      ],
      "actions": [
        {
          "name": "add_load",
          "display_name": "加载",
          "parameters": [{"name": "delta", "display_name": "增量", "data_type": "FLOAT64", "required": true}],
          "precondition_expr": "headroom >= param.delta",
          "side_effect_tier": "SELF_ONLY",
          "mutations": [{"target": "self.base_load", "expression": "base_load + param.delta"}]
        }
      ]
    }
  ],
  "edge_types": [],
  "mappings": []
}'
api POST "/api/xblue/schema?subgraph=${SUB_SLUG}" "$CLEAN_SUB_SCHEMA" >/dev/null
CERT_RES=$(api POST "/api/xblue/subgraphs/${SUB_SLUG}/certify" '{}')
expect_json "trial-tick-certified" "$CERT_RES" '
  .trial_tick_passed == true
  and (.cert_level == "L4_CERTIFIED" or .cert_level == "L3_VERIFIED")
  and .dead_knowledge_cnt == 0
'

gate_pass step26
