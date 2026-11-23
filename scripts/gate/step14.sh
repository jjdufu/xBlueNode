#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "$0")/lib.sh"

# 契约与需求追踪静态双向核验（A4 §13 & A7 §3）
python3 "$ROOT/scripts/check_routes.py" --routes "$ROOT/axpoint-core/crates/axpoint-server/src/routes" --index "$ROOT/docs/standards/A4-API-CONTRACT-INDEX.md"
python3 "$ROOT/scripts/check_traceability.py" "$ROOT/docs/standards/A7-REQUIREMENTS-TRACEABILITY.md" "$ROOT/scripts/gate"

(cd "$ROOT/axpoint-core" && cargo test -p axpoint-server ops::)

# 深度健康
expect_json "health-deep" "$(api GET /api/axops/health/deep)" '.database.ok and .parser.ok and .lake.ok and .migrations.pending == 0'
AXPOINT_API_TOKEN="$AXPOINT_DEV_TOKEN" CORE_BASE_URL="$CORE_BASE_URL" bash "$ROOT/scripts/ops/health-report.sh" >/dev/null

# 模板安装：计划 → 应用 → 对象存在 → 卸载
PLAN=$(api POST /api/axops/templates/retail_store/plan '{"on_conflict":"RENAME"}')
IID=$(echo "$PLAN" | jq -r .id)
api POST "/api/axops/templates/installs/$IID/apply" '{}' >/dev/null
wait_until "template-installed" 300 "api GET /api/axops/templates/installs | jq -e '.items[] | select(.id==\"$IID\") | .status == \"INSTALLED\"'"
expect_json "template-dashboard" "$(api GET /api/xblueview/dashboards/by-slug/retail-overview)" '.status == "PUBLISHED"'
expect_eq "audit-template" "$(sql_app default "SELECT count(*) > 0 FROM sec_audit_logs WHERE action='TEMPLATE_INSTALL'")" "t"
api POST "/api/axops/templates/installs/$IID/uninstall" '{}' >/dev/null
expect_code "template-uninstalled" 404 GET "/api/xblueview/dashboards/by-slug/retail-overview"

# 工作项：流程动作生成
FLOW=$(api POST /api/axflow/flows '{"name":"wi_gate","slug":"wi-gate","nodes":[{"id":"a","kind":"ACTION_EMIT","label":"建工作项","pos":{"x":0,"y":0},"config":{"actions":[{"kind":"WORK_ITEM","title":"门禁工作项","priority":"P1"}]}}],"edges":[],"trigger_cfg":{"type":"MANUAL"}}' | jq -r .id)
VER=$(api POST "/api/axflow/flows/$FLOW/versions" '{}' | jq -r .version)
api POST "/api/axflow/flows/$FLOW/deploy" "{\"environment\":\"PROD\",\"version\":$VER}" >/dev/null
RUN=$(api POST "/api/axflow/flows/$FLOW/runs" '{"environment":"PROD"}' | jq -r .run_id)
wait_run_status "$RUN" SUCCEEDED
expect_json "work-item" "$(api GET '/api/axops/work-items?source_kind=FLOW_ACTION')" '[.items[] | select(.title=="门禁工作项")] | length == 1'

# 保留：过期任务被清理
sql_owner "INSERT INTO ax_jobs (tenant_id, kind, status, created_at, updated_at) VALUES ('default','RETENTION','SUCCEEDED', now() - interval '400 days', now() - interval '400 days')" >/dev/null
wait_job "$(api POST /api/axops/retention/run-now '{}' | jq -r .job_id)"
expect_eq "retention-jobs" "$(sql_owner "SELECT count(*) FROM ax_jobs WHERE created_at < now() - interval '399 days'")" "0"

# 审计保留脚本与锚点
AXPOINT_AUDIT_FORCE_KEEP_DAYS=1 bash "$ROOT/scripts/ops/retention-audit.sh" >/dev/null
expect_json "audit-anchor" "$(api GET /api/axguard/audit/verify)" '.ok == true'

# 备份与恢复演练（先刷平后台增量时序脏桶，避免恢复对账期间周期汇总搬移行数）
wait_job "$(api POST /api/axops/jobs/rollup-now | jq -r .job_id)"
export AXPOINT_BACKUP_DIR="$ROOT/.gate/backups"
bash "$ROOT/scripts/ops/backup.sh" FULL
bash "$ROOT/scripts/ops/restore-drill.sh" "$AXPOINT_BACKUP_DIR/latest"

gate_pass step14
