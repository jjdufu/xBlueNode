#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "$0")/lib.sh"

for tf in guard pivot semantic nl2sql_eval; do
  (cd "$ROOT/axpoint-core" && cargo test -p axpoint-analytics "$tf")
done

# 基本查询与租户改写
expect_json "sql-count" "$(api POST /api/analytics/sql/run '{"sql":"SELECT count(*) AS n FROM demo_orders"}')" '.rows[0].n == 100'
expect_json "platform-view" "$(api POST /api/analytics/sql/run '{"sql":"SELECT count(*) AS n FROM ax.nodes WHERE type_name = '\''Warehouse'\''"}')" '.rows[0].n >= 2'

# 守卫拒绝
for q in "SELECT * FROM read_csv('/etc/passwd')" "SELECT getenv('HOME')" "ATTACH ':memory:' AS x" \
         "SELECT * FROM lake.\"tenant_other\".orders" "SELECT * FROM ax_pg.public.sec_users" "COPY demo_orders TO '/tmp/x.csv'"; do
  expect_json "guard:$q" "$(api POST /api/analytics/sql/run "$(jq -nc --arg s "$q" '{sql:$s}')")" '.error_code == "SQL_REJECTED"'
done

# 截断
expect_json "truncate" "$(api POST /api/analytics/sql/run '{"sql":"SELECT * FROM range(20)","max_rows":10}')" '.truncated == true and (.rows | length) == 10'

# 透视小计与总计
expect_json "pivot-total" "$(api POST /api/analytics/pivot '{"dataset":"demo_orders","rows":[{"field":"status"}],"columns":[],"values":[{"field":"amount","agg":"SUM","alias":"amt"}],"filters":[],"sort":[],"subtotals":true,"grand_total":true,"row_limit":100,"column_limit":50}')" '(.row_kinds | index("GRAND_TOTAL")) != null'

# 指标与数据库对账（统一格式化为两位小数，避免 3517.50 与 3517.5 尾零差异）
EXPECTED=$(sql_owner "SELECT sum(amount) FROM demo_src.orders WHERE status='PAID'" | awk '{printf "%.2f", $1}')
ACTUAL=$(api POST "/api/analytics/metrics/$PAID_METRIC_NAME/query" '{"dimensions":[],"filters":[],"grain":"MONTH"}' | jq -r '[.rows[].value | tonumber] | add' | awk '{printf "%.2f", $1}')
expect_eq "metric-reconcile" "$ACTUAL" "$EXPECTED"

# PII 脱敏（演示表 phone 列，默认对包括 ADMIN 在内的全部主体脱敏）
expect_json "pii-mask" "$(api POST /api/analytics/sql/run '{"sql":"SELECT phone FROM demo_orders LIMIT 1"}')" '.rows[0].phone | test("^1[3-9][0-9]\\*{4}[0-9]{4}$")'

# 问数（门禁环境使用录制响应：AXPOINT_LLM_CASSETTE 指向评测录制文件）
expect_json "ask-data" "$(api POST /api/analytics/ask '{"question":"已支付订单的总金额是多少"}')" '.intent == "DATA" and (.sql | test("SUM"; "i"))'

# 历史与审计
expect_eq "audit-query" "$(sql_app default "SELECT count(*) > 0 FROM sec_audit_logs WHERE action='QUERY_SQL'")" "t"

gate_pass step04
