#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "$0")/lib.sh"

(cd axpoint-core && cargo test -p axpoint-flow rules && cargo test -p axpoint-flow notify && cargo test -p axpoint-db rules_repo -- --test-threads=1)

# 触发：库存写为 10 → 告警 OPEN、通知送达、流程启动
api POST /api/xblue/metrics/write "{\"points\":[{\"node_id\":\"$WAREHOUSE_NODE_ID\",\"metric\":\"inventory\",\"ts\":\"$(date -u +%FT%TZ)\",\"value\":10}],\"source_tag\":\"MANUAL\"}" >/dev/null
wait_until "alert-open" 60 "api GET '/api/axflow/alerts?rule_id=$LOW_INV_RULE_ID&status=OPEN' | jq -e '.items | length == 1'"
wait_until "notified"   60 "[[ -f $SINK_FILE ]] && awk '/\"event\":\"OPENED\"/{c++} END{exit !(c==1)}' $SINK_FILE"
expect_eq "signature" "$(python3 scripts/gate/tools/webhook_sink.py --verify "$SINK_FILE" --secret "$SINK_SECRET")" "OK"
ALERT=$(api GET "/api/axflow/alerts?rule_id=$LOW_INV_RULE_ID&status=OPEN" | jq -r '.items[0].id')
expect_json "flow-started" "$(api GET /api/axflow/alerts/$ALERT/events)" '[.items[].event] | index("FLOW_STARTED") != null'

# 去重：再次写低值不新开告警
api POST /api/xblue/metrics/write "{\"points\":[{\"node_id\":\"$WAREHOUSE_NODE_ID\",\"metric\":\"inventory\",\"ts\":\"$(date -u +%FT%TZ)\",\"value\":9}],\"source_tag\":\"MANUAL\"}" >/dev/null
sleep 3
expect_eq "dedup" "$(sql_app default "SELECT count(*) FROM ax_alerts WHERE rule_id='$LOW_INV_RULE_ID' AND status <> 'RESOLVED'")" "1"

# 确认与自动恢复
expect_code "ack" 200 POST /api/axflow/alerts/$ALERT/ack
api POST /api/xblue/metrics/write "{\"points\":[{\"node_id\":\"$WAREHOUSE_NODE_ID\",\"metric\":\"inventory\",\"ts\":\"$(date -u +%FT%TZ)\",\"value\":500}],\"source_tag\":\"MANUAL\"}" >/dev/null
wait_until "auto-resolve" 90 "api GET /api/axflow/alerts/$ALERT | jq -e '.status == \"RESOLVED\" and .resolve_reason == \"AUTO\"'"
wait_until "resolved-notified" 60 "[[ -f $SINK_FILE ]] && awk '/\"event\":\"RESOLVED\"/{c++} END{exit !(c==1)}' $SINK_FILE"

gate_pass step08
