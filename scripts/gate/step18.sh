#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "$0")/lib.sh"

# 0. 迁移完整性核验
expect_json "step18-migrations-clean" "$(api GET /api/axops/health/deep)" '.migrations.pending == 0'

# 1. 单元与文档测试：覆盖成熟度反序列化兼容、pass_count 计算、单节点血缘与 7 章节规格书确定性编译
(cd "$ROOT/axpoint-core" && cargo test -p axpoint-domain studio --locked && cargo test -p axpoint-llm studio_copilot --locked)

# 2. 创建独立测试子图与初始草案，验证正向 pass_count 完备度计数
SUB_SLUG="dev_step18_studio"
cleanup_step18() {
  api DELETE "/api/xblue/subgraphs/$SUB_SLUG" >/dev/null 2>&1 || true
}
trap cleanup_step18 EXIT

api DELETE "/api/xblue/subgraphs/$SUB_SLUG" >/dev/null 2>&1 || true
api POST /api/xblue/subgraphs "{\"slug\":\"$SUB_SLUG\",\"display_name\":\"Step18 建模演练\",\"is_dev_sandbox\":true}" >/dev/null || true

DRAFT_RES=$(api POST /api/xblue/drafts "{
  \"subgraph_slug\": \"$SUB_SLUG\",
  \"title\": \"Step18 助手与血缘验证草案\",
  \"source\": \"MANUAL\",
  \"schema_doc\": {
    \"node_types\": [
      {
        \"name\": \"Supplier\",
        \"display_name\": \"供应商\",
        \"description\": \"上游物料供应商\",
        \"semantic_layer_tier\": \"STATIC_FACT\",
        \"domain_group\": \"采购域\",
        \"properties\": [
          { \"name\": \"tier_code\", \"display_name\": \"等级\", \"data_type\": \"STRING\", \"kind\": \"ATTRIBUTE\", \"required\": true },
          { \"name\": \"lead_days\", \"display_name\": \"交付周期\", \"unit\": \"天\", \"data_type\": \"FLOAT64\", \"kind\": \"METRIC\", \"required\": false }
        ]
      },
      {
        \"name\": \"Warehouse\",
        \"display_name\": \"区域仓\",
        \"description\": \"区域履约仓库\",
        \"semantic_layer_tier\": \"CUBE_ROLLUP\",
        \"domain_group\": \"履约域\",
        \"properties\": [
          { \"name\": \"wh_region\", \"display_name\": \"大区\", \"data_type\": \"STRING\", \"kind\": \"ATTRIBUTE\", \"required\": true },
          { \"name\": \"stock_qty\", \"display_name\": \"在库量\", \"unit\": \"件\", \"data_type\": \"FLOAT64\", \"kind\": \"METRIC\", \"required\": false },
          { \"name\": \"avg_supplier_lead\", \"display_name\": \"上游平均交期\", \"unit\": \"天\", \"data_type\": \"FLOAT64\", \"kind\": \"METRIC\", \"required\": false }
        ],
        \"agg_rules\": [
          {
            \"target_metric\": \"avg_supplier_lead\",
            \"op\": \"AVG\",
            \"via_link_type\": \"SUPPLIES\",
            \"direction\": \"IN\",
            \"source_type_name\": \"Supplier\",
            \"source_metric\": \"lead_days\"
          }
        ]
      }
    ],
    \"edge_types\": [
      { \"name\": \"SUPPLIES\", \"src_type_name\": \"Supplier\", \"dst_type_name\": \"Warehouse\", \"cardinality\": \"MANY_TO_MANY\", \"impact_weight\": 0.7 }
    ],
    \"mappings\": []
  }
}")
DRAFT_ID=$(echo "$DRAFT_RES" | jq -r .id)
expect_json "pass-count-metrics" "$DRAFT_RES" '.pass_count >= 5'

# 3. 验证 Copilot DIAGNOSE_ONLY 只读诊断：零草案修改、零快照新增
BEFORE_DOC=$(api GET "/api/xblue/drafts/$DRAFT_ID" | jq -c .schema_doc)
SNAP_BEFORE=$(sql_app default "SELECT count(*) FROM xb_schema_draft_snapshots WHERE draft_id = '$DRAFT_ID'")

DIAG_RES=$(api POST "/api/xblue/drafts/$DRAFT_ID/copilot" '{
  "mode": "DIAGNOSE_ONLY",
  "prompt": "完备度体检：检查 Warehouse 的指标与映射完备度",
  "focus": { "selected_node_type": "Warehouse", "active_tab": "properties" }
}')
AFTER_DIAG_DOC=$(api GET "/api/xblue/drafts/$DRAFT_ID" | jq -c .schema_doc)
SNAP_AFTER_DIAG=$(sql_app default "SELECT count(*) FROM xb_schema_draft_snapshots WHERE draft_id = '$DRAFT_ID'")
expect_json "copilot-readonly-diag" "$DIAG_RES" \
  '.mode == "DIAGNOSE_ONLY" and .mutated == false and .snapshot_id == null and (.diagnosis.findings | length) >= 1'
expect_eq "copilot-readonly-immutable" "$BEFORE_DOC|$SNAP_BEFORE" "$AFTER_DIAG_DOC|$SNAP_AFTER_DIAG"

# 4. 验证 Copilot PATCH_DRAFT：自动写入前置快照、新要素标记 PROPOSED、一键回退精准复原并写审计
PATCH_RES=$(api POST "/api/xblue/drafts/$DRAFT_ID/copilot" '{
  "mode": "PATCH_DRAFT",
  "prompt": "为 Warehouse 增加安全库存指标 safety_stock（单位：件）与缺口派生公式 shortage_gap = max(0, safety_stock - stock_qty)",
  "focus": { "selected_node_type": "Warehouse", "active_tab": "rules" }
}')
SNAP_ID=$(echo "$PATCH_RES" | jq -r .snapshot_id)
expect_json "copilot-patch-applied" "$PATCH_RES" \
  '.mutated == true and (.snapshot_id | length) > 10 and .snapshot_no == 1'
expect_eq "studio-snapshots-rls" "$(sql_app other "SELECT count(*) FROM xb_schema_draft_snapshots")" "0"

# 回退至快照 #1，验证 schema_doc 完整恢复且记录 STUDIO_SNAPSHOT_RESTORE 审计
RESTORED=$(api POST "/api/xblue/drafts/$DRAFT_ID/snapshots/$SNAP_ID/restore" '{}')
RESTORED_DOC=$(echo "$RESTORED" | jq -c .schema_doc)
AUDIT_RESTORE=$(sql_app default "SELECT count(*) > 0 FROM sec_audit_logs WHERE action = 'STUDIO_SNAPSHOT_RESTORE'")
expect_eq "copilot-mutate-rollback" "$RESTORED_DOC|$AUDIT_RESTORE" "$BEFORE_DOC|t"

# 5. 再次应用补丁，验证 PROPOSED 阻断未签收直接发布，确认后成功发布正式版本
PATCH2=$(api POST "/api/xblue/drafts/$DRAFT_ID/copilot" '{
  "mode": "PATCH_DRAFT",
  "prompt": "为 Warehouse 增加安全库存指标 safety_stock（单位：件）",
  "focus": { "selected_node_type": "Warehouse" }
}')
expect_code "unconfirmed-publish-blocked" 400 POST "/api/xblue/drafts/$DRAFT_ID/publish" '{"change_summary":"try before confirm"}'

CONFIRM_RES=$(api POST "/api/xblue/drafts/$DRAFT_ID/confirm-proposals" '{"scope":"ALL"}')
PUB_RES=$(api POST "/api/xblue/drafts/$DRAFT_ID/publish" '{"change_summary":"confirm proposals and publish v1"}')
expect_json "proposal-confirm-flow" "$PUB_RES" '.version >= 1'

# 6. 验证单节点左→右五段血缘管道图与 Np·Nm·Nr 徽标计数
LINEAGE_RES=$(api GET "/api/xblue/subgraphs/$SUB_SLUG/lineage/Warehouse")
expect_json "node-lineage-graph" "$LINEAGE_RES" \
  '.node_type == "Warehouse" and (.stages | length) == 5 and .badge_counts.metric_count >= 2 and (.edges | length) >= 1'

# 7. 验证 7 章节模型全景规格书与 q1..q6 推荐验收问数集
SPEC_RES=$(api GET "/api/xblue/subgraphs/$SUB_SLUG/spec-doc?format=json")
expect_json "subgraph-spec-doc" "$SPEC_RES" \
  '(.sections | length) == 7 and (.acceptance_prompts | length) == 6 and .acceptance_prompts[0].id == "q1"'

gate_pass step18
