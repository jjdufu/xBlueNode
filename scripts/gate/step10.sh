#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "$0")/lib.sh"

(cd axpoint-core && cargo test -p axpoint-domain sim && cargo test -p axpoint-db sim_repo)

PROD_BEFORE=$(sql_app default "SELECT md5(string_agg(id::text || metrics::text, ',' ORDER BY id)) FROM xb_nodes")

# 写时复制：沙盘编辑不影响生产
api PATCH "/api/axsim/sandboxes/$SANDBOX_ID/nodes/$WAREHOUSE_NODE_ID" '{"metrics":{"inventory":1}}' >/dev/null

# 场景：简写与完整写法等价
S1=$(api POST "/api/axsim/sandboxes/$SANDBOX_ID/scenarios" "{\"name\":\"s1\",\"actions\":[{\"target_node_id\":\"$FACTORY1_NODE_ID\",\"metric_key\":\"output\",\"delta_ratio\":-0.3}],\"params\":{}}" | jq -r .id)
S2=$(api POST "/api/axsim/sandboxes/$SANDBOX_ID/scenarios" "{\"name\":\"s2\",\"actions\":[{\"target_node_id\":\"$FACTORY1_NODE_ID\",\"metric_key\":\"output\",\"mode\":\"RATIO\",\"value\":-0.3}],\"params\":{}}" | jq -r .id)
wait_scenario "$S1"; wait_scenario "$S2"
expect_eq "shorthand-equal" "$(api GET "/api/axsim/scenarios/$S1/impacts?limit=500" | jq -c '[.items[] | [.node_id,.metric,.after_val]] | sort')" \
                            "$(api GET "/api/axsim/scenarios/$S2/impacts?limit=500" | jq -c '[.items[] | [.node_id,.metric,.after_val]] | sort')"
expect_json "propagated" "$(api GET "/api/axsim/scenarios/$S1/impacts?metric=fulfillment&limit=5")" '(.items | length) > 0 and .items[0].hop_distance >= 2'

# 缺失指标不使用默认基值
expect_json "missing-metric" "$(api POST "/api/axsim/sandboxes/$SANDBOX_ID/simulate" "{\"actions\":[{\"target_node_id\":\"$FACTORY1_NODE_ID\",\"metric_key\":\"no_such_metric\",\"delta_ratio\":-0.5}]}")" '.error_code == "INVALID_ARGUMENT"'

# 对比与回放
expect_json "compare" "$(api GET "/api/axsim/scenarios/compare?ids=$S1,$S2&briefing=true")" '(.kpis | length) > 0 and (.briefing.recommended_scenario_id as $r | ["'"$S1"'","'"$S2"'"] | index($r) != null)'
S3=$(api POST "/api/axsim/sandboxes/$SANDBOX_ID/scenarios" "{\"name\":\"s3\",\"actions\":[{\"target_node_id\":\"$FACTORY1_NODE_ID\",\"metric_key\":\"output\",\"delta_ratio\":-0.3}],\"params\":{\"mode\":\"STEPPED\",\"horizon_steps\":6}}" | jq -r .id)
wait_scenario "$S3"
expect_json "checkpoints" "$(api GET "/api/axsim/scenarios/$S3/checkpoints")" '(.items | length) == 7'

# 生产未被修改
expect_eq "prod-untouched" "$(sql_app default "SELECT md5(string_agg(id::text || metrics::text, ',' ORDER BY id)) FROM xb_nodes")" "$PROD_BEFORE"

# 决策下发：审批 → 写入 → 冲突检测
DEC=$(api POST "/api/axsim/scenarios/$S1/decisions" '{"title":"下调 F-1 产能","tolerance_ratio":0}' | jq -r .id)
APR=$(api GET "/api/axsim/decisions/$DEC" | jq -r .approval_id)
api POST "/api/axflow/approvals/$APR/decide" '{"decision":"APPROVE"}' >/dev/null
wait_until "applied" 60 "api GET /api/axsim/decisions/$DEC | jq -e '.status == \"APPLIED\"'"
expect_eq "audit-sim-apply" "$(sql_app default "SELECT count(*) > 0 FROM sec_audit_logs WHERE action='SIM_APPLY'")" "t"
DEC2=$(api POST "/api/axsim/scenarios/$S1/decisions" '{"title":"重复下发","tolerance_ratio":0}' | jq -r .id)
APR2=$(api GET "/api/axsim/decisions/$DEC2" | jq -r .approval_id)
api POST "/api/axflow/approvals/$APR2/decide" '{"decision":"APPROVE"}' >/dev/null
wait_until "conflict" 60 "api GET /api/axsim/decisions/$DEC2 | jq -e '.status == \"CONFLICT\"'"

gate_pass step10
