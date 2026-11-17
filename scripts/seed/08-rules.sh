#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "$0")/../gate/lib.sh"

mkdir -p "$ROOT/.gate/pids" "$ROOT/.gate/logs"

SINK_FILE="$ROOT/.gate/webhook.jsonl"
SINK_SECRET="whsec_step08_gate_secret_2026"

if [[ -f "$ROOT/.gate/pids/webhook_sink" ]]; then
  kill "$(cat "$ROOT/.gate/pids/webhook_sink")" 2>/dev/null || true
  rm -f "$ROOT/.gate/pids/webhook_sink"
fi
rm -f "$SINK_FILE"

nohup python3 "$ROOT/scripts/gate/tools/webhook_sink.py" \
  --host 127.0.0.1 \
  --port 18900 \
  --out "$SINK_FILE" \
  </dev/null >"$ROOT/.gate/logs/webhook_sink.log" 2>&1 &
echo $! > "$ROOT/.gate/pids/webhook_sink"

wait_until "webhook-sink-healthy" 30 \
  "curl --noproxy '*' -sf http://127.0.0.1:18900/health >/dev/null" >/dev/null

# 1. Create gate_sink WEBHOOK channel (allow_private_network=true, HMAC signing_secret)
CHANNEL_PAYLOAD=$(jq -n \
  --arg secret "$SINK_SECRET" \
  '{
    name: "gate_sink",
    kind: "WEBHOOK",
    config: {
      url: "http://127.0.0.1:18900/hook",
      headers: {
        "X-Source": "xBlueNode-Step08"
      }
    },
    secrets: {
      signing_secret: $secret
    },
    allow_private_network: true,
    enabled: true
  }')

CHANNEL_RESP=$(api POST /api/axflow/channels "$CHANNEL_PAYLOAD")
GATE_SINK_CHANNEL_ID=$(printf '%s' "$CHANNEL_RESP" | jq -r '.id // empty')
if [[ -z "$GATE_SINK_CHANNEL_ID" ]]; then
  echo "seed 08-rules failed creating gate_sink channel: $CHANNEL_RESP" >&2
  exit 1
fi

# 2. Create low_inventory rule (Warehouse.inventory < 50 -> gate_sink + daily_supply_refresh PROD run)
RULE_PAYLOAD=$(jq -n \
  --arg ch_id "$GATE_SINK_CHANNEL_ID" \
  --arg flow_id "$FLOW_ID" \
  '{
    name: "low_inventory",
    description: "仓储库存低于安全红线 (< 50) 时触发紧急告警并自动启动补货编排流",
    scope_kind: "METRIC",
    target: {
      type_name: "Warehouse",
      metric: "inventory"
    },
    condition: {
      kind: "THRESHOLD",
      op: "LT",
      value: 50,
      for_minutes: 0
    },
    severity: "CRITICAL",
    title_template: "[{{severity}}] {{node.title}} 库存红线告警 ({{value}} < {{threshold}})",
    body_template: "规则 {{rule.name}} 命中 {{node.type_name}}/{{node.key}}：{{metric}}={{value}}，低于安全阈值 {{threshold}}。",
    cooldown_minutes: 0,
    resolve_after_minutes: 0,
    channel_ids: [$ch_id],
    escalation_channel_ids: [],
    run_flow_id: $flow_id,
    enabled: true
  }')

RULE_RESP=$(api POST /api/axflow/rules "$RULE_PAYLOAD")
LOW_INV_RULE_ID=$(printf '%s' "$RULE_RESP" | jq -r '.id // empty')
if [[ -z "$LOW_INV_RULE_ID" ]]; then
  echo "seed 08-rules failed creating low_inventory rule: $RULE_RESP" >&2
  exit 1
fi

# 3. Seed an independent demo OPEN alert on Factory F-3 (no webhook channel so SINK_FILE stays empty for step08.sh)
F3_NODE_ID=$(api GET '/api/xblue/nodes?type_name=Factory&q=F-3' | jq -r '.items[] | select(.node_key=="F-3") | .id // empty')
if [[ -n "$F3_NODE_ID" ]]; then
  DEMO_RULE_PAYLOAD='{
    "name": "demo_factory_output_watch",
    "description": "深圳模组厂日产能低于基准线 (< 900) 演示监控规则",
    "scope_kind": "METRIC",
    "target": {
      "type_name": "Factory",
      "metric": "output",
      "node_filter": "prop.city == '\''深圳'\''"
    },
    "condition": {
      "kind": "THRESHOLD",
      "op": "LT",
      "value": 900,
      "for_minutes": 0
    },
    "severity": "WARN",
    "title_template": "[{{severity}}] {{node.title}} 产能低于基准 ({{value}} < {{threshold}})",
    "body_template": "制造节点 {{node.title}} 当前日产能 {{value}} 件，低于排产目标 {{threshold}} 件。",
    "cooldown_minutes": 30,
    "resolve_after_minutes": 1440,
    "channel_ids": [],
    "escalation_channel_ids": [],
    "enabled": true
  }'
  DEMO_RULE_RESP=$(api POST /api/axflow/rules "$DEMO_RULE_PAYLOAD")
  DEMO_RULE_ID=$(printf '%s' "$DEMO_RULE_RESP" | jq -r '.id // empty')
  if [[ -n "$DEMO_RULE_ID" ]]; then
    api POST /api/xblue/metrics/write "{\"points\":[{\"node_id\":\"$F3_NODE_ID\",\"metric\":\"output\",\"ts\":\"$(date -u +%FT%TZ)\",\"value\":860}],\"source_tag\":\"MANUAL\"}" >/dev/null
    wait_until "demo-alert-open" 30 \
      "api GET '/api/axflow/alerts?rule_id=$DEMO_RULE_ID&status=OPEN' | jq -e '.items | length == 1'" >/dev/null
  fi
fi

echo "SINK_FILE=$SINK_FILE"
echo "SINK_SECRET=$SINK_SECRET"
echo "GATE_SINK_CHANNEL_ID=$GATE_SINK_CHANNEL_ID"
echo "LOW_INV_RULE_ID=$LOW_INV_RULE_ID"
