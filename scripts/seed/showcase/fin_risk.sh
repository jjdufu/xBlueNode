#!/usr/bin/env bash
# Step 33 Showcase Seed (2/2): `fin_risk` 集团授信与互保风控可执行世界幂等秒级装载 (< 3s)
set -euo pipefail
source "$(dirname "$0")/../../gate/lib.sh"

mkdir -p "$ROOT/.gate"
PACK_DIR="$ROOT/templates/packs/fin_risk"
FIN_ANALYST_SH_USERNAME="fin_analyst_sh"
FIN_ANALYST_SH_PASSWORD="Axp#FinSh_2026!55"

upsert_ids_env() {
  local key="$1" val="$2" env_file="$ROOT/.gate/ids.env"
  touch "$env_file"
  if grep -q "^${key}=" "$env_file" 2>/dev/null; then
    local tmp="${env_file}.tmp.$$"
    grep -v "^${key}=" "$env_file" > "$tmp" || true
    printf '%s=%s\n' "$key" "$val" >> "$tmp"
    mv "$tmp" "$env_file"
  else
    printf '%s=%s\n' "$key" "$val" >> "$env_file"
  fi
}

if [[ ! -s "${AXPOINT_JWT_PRIVATE_KEY_PATH:-/nonexistent}" ]]; then
  "$ROOT/scripts/ops/gen-jwt-key.sh" k1 >/dev/null 2>&1 || true
fi

PY_BIN="$ROOT/axpoint-parser/.venv/bin/python"
if [[ ! -x "$PY_BIN" ]]; then
  PY_BIN="python3"
fi

SEED_TMP=$(mktemp -d)
trap 'rm -rf "$SEED_TMP"' EXIT

# 1. 创建或更新 fin_risk 子图并发布四域三层 Schema
SUB_LIST=$(api GET /api/xblue/subgraphs)
if ! printf '%s' "$SUB_LIST" | jq -e '(if type == "array" then . else (.items // []) end) | map(select(.slug == "fin_risk")) | length > 0' >/dev/null 2>&1; then
  api POST /api/xblue/subgraphs '{
    "slug": "fin_risk",
    "display_name": "集团授信与互保风控",
    "description": "集团母体—关联法人—实控人—授信借据—行业敞口可执行风控模型（三道敏感脱敏与逾期动态联防视野）",
    "is_dev_sandbox": false
  }' >/dev/null
fi

SCHEMA_PAYLOAD=$(jq -c '. + {"force": true, "change_summary": "publish fin_risk world schema"}' "$PACK_DIR/schema.json")
api POST "/api/xblue/schema?subgraph=fin_risk" "$SCHEMA_PAYLOAD" >/dev/null

# 2. 并行执行：向量化节点/互保边/6万行流水灌装 + 分行部门/风控员账号/STATE_DRIVEN视野 + 告警规则 + PROD动作流
"$PY_BIN" "$ROOT/axpoint-parser/scripts/gen_worlds_grid_fin.py" --world fin_risk --tenant default > "$SEED_TMP/gen.json" &
GEN_PID=$!

(
  api PUT /api/axguard/departments/sh_branch '{"name":"上海分行风控部"}' >/dev/null &
  api PUT /api/axguard/departments/hz_branch '{"name":"杭州分行风控部"}' >/dev/null &
  api PUT /api/axguard/departments/sz_branch '{"name":"深圳分行风控部"}' >/dev/null &
  api PUT /api/xblue/subgraphs/fin_risk/competency-suite "$(cat "$PACK_DIR/competency.json")" >/dev/null &
  api PUT /api/axguard/row-policy-defaults '{
    "target_kind": "NODE_TYPE",
    "target_ref": "LegalEntity",
    "default_deny": false
  }' >/dev/null &

  EXISTING_RP_ID=$(api GET /api/axguard/row-policies | jq -r '(if type == "array" then . else (.items // []) end) | map(select(.name == "fin_risk_cross_branch_overdue_escalate"))[0].id // empty')
  RP_BODY='{
    "name": "fin_risk_cross_branch_overdue_escalate",
    "target_kind": "NODE_TYPE",
    "target_ref": "LegalEntity",
    "applies_to": { "depts": ["sh_branch", "hz_branch", "sz_branch"] },
    "predicate": "properties.branch_dept == {{user.dept}} OR metrics.overdue_ratio >= 0.05",
    "policy_mode": "STATE_DRIVEN",
    "enabled": true
  }'
  if [[ -n "$EXISTING_RP_ID" ]]; then
    api PUT "/api/axguard/row-policies/$EXISTING_RP_ID" "$RP_BODY" >/dev/null &
  else
    api POST /api/axguard/row-policies "$RP_BODY" >/dev/null &
  fi

  EXISTING_RULES=$(api GET "/api/axflow/rules?limit=200")
  while IFS= read -r rule_json; do
    r_name=$(printf '%s' "$rule_json" | jq -r '.name')
    if ! printf '%s' "$EXISTING_RULES" | jq -e --arg nm "$r_name" '(if type == "array" then . else (.items // []) end) | map(select(.name == $nm)) | length > 0' >/dev/null 2>&1; then
      api POST /api/axflow/rules "$rule_json" >/dev/null &
    fi
  done < <(jq -c '.rules[]' "$PACK_DIR/rules.json")

  FIN_FLOW_ID=$(api GET "/api/axflow/flows?limit=200" | jq -r '(if type == "array" then . else (.items // []) end)[] | select(.slug == "fin-risk-freeze-credit") | .id' | head -n 1 || true)
  if [[ -z "$FIN_FLOW_ID" || "$FIN_FLOW_ID" == "null" ]]; then
    FLOW_BODY=$(jq -c 'del(.key)' "$PACK_DIR/flows/freeze_credit.json")
    FIN_FLOW_ID=$(api POST /api/axflow/flows "$FLOW_BODY" | jq -r '.id')
    api POST "/api/axflow/flows/$FIN_FLOW_ID/versions" '{"note":"fin_risk credit freeze v1"}' >/dev/null
    api POST "/api/axflow/flows/$FIN_FLOW_ID/deploy" '{"version":1,"environment":"PROD"}' >/dev/null
  fi
  printf '%s' "$FIN_FLOW_ID" > "$SEED_TMP/flow_id"

  wait
  EXISTING_UID=$(api GET /api/axguard/users | jq -r --arg u "$FIN_ANALYST_SH_USERNAME" '(if type == "array" then . else (.items // []) end) | map(select(.username == $u))[0].id // empty')
  if [[ -z "$EXISTING_UID" ]]; then
    api POST /api/axguard/users "$(jq -nc \
      --arg u "$FIN_ANALYST_SH_USERNAME" \
      --arg p "$FIN_ANALYST_SH_PASSWORD" '{
        username: $u,
        display_name: "上海分行风控分析师",
        password: $p,
        dept_id: "sh_branch",
        clearance: "CONFIDENTIAL",
        roles: ["ANALYST"],
        attributes: {}
      }')" >/dev/null
  fi
) &
SETUP_PID=$!

wait "$GEN_PID"
wait "$SETUP_PID"

FIN_DS_ID=$(jq -r '.dataset_id' "$SEED_TMP/gen.json")
FIN_FLOW_ID=$(cat "$SEED_TMP/flow_id")

# 3. 并行配置湖表 PII 脱敏列（同步失效 SessionPool）、双图算法预跑与五区风控数字看板发布
api PUT "/api/axhub/datasets/$FIN_DS_ID/columns/operator_phone" '{"pii_category":"PHONE","description":"经办财务人员手机号"}' >/dev/null &
api PUT "/api/axhub/datasets/$FIN_DS_ID/columns/payee_card_no" '{"pii_category":"BANK_CARD","description":"对手方结算银行卡号"}' >/dev/null &
api POST /api/xblue/graph/algorithms/PAGERANK '{"subgraph":"fin_risk"}' >/dev/null &
api POST /api/xblue/graph/algorithms/LABEL_PROPAGATION '{"subgraph":"fin_risk"}' >/dev/null &
wait

EXISTING_DASH=$(api GET /api/xblueview/dashboards/by-slug/fin-risk-cockpit 2>/dev/null || echo "{}")
FIN_DASH_ID=$(printf '%s' "$EXISTING_DASH" | jq -r '.id // empty' 2>/dev/null || true)
FIN_DASH_STATUS=$(printf '%s' "$EXISTING_DASH" | jq -r '.status // empty' 2>/dev/null || true)
if [[ -z "$FIN_DASH_ID" || "$FIN_DASH_STATUS" != "PUBLISHED" ]]; then
  if [[ -n "$FIN_DASH_ID" ]]; then
    api DELETE "/api/xblueview/dashboards/$FIN_DASH_ID" >/dev/null 2>&1 || true
  fi
  CREATE_DASH_BODY=$(jq -c '{name, slug, description, theme, canvas}' "$PACK_DIR/dashboards/cockpit.json")
  FIN_DASH_ID=$(api POST /api/xblueview/dashboards "$CREATE_DASH_BODY" | jq -r '.id')
  while IFS= read -r w_json; do
    api POST "/api/xblueview/dashboards/$FIN_DASH_ID/widgets" "$w_json" >/dev/null &
  done < <(jq -c --arg fid "$FIN_FLOW_ID" '
    .widgets[] | if .data_source.kind == "ACTION" then .data_source.flow_id = $fid else . end
  ' "$PACK_DIR/dashboards/cockpit.json")
  wait
  api POST "/api/xblueview/dashboards/$FIN_DASH_ID/publish" '{}' >/dev/null
fi

upsert_ids_env "FIN_RISK_DATASET_ID" "$FIN_DS_ID"
upsert_ids_env "FIN_RISK_DASHBOARD_ID" "$FIN_DASH_ID"
upsert_ids_env "FIN_ANALYST_SH_USERNAME" "$FIN_ANALYST_SH_USERNAME"
upsert_ids_env "FIN_ANALYST_SH_PASSWORD" "$FIN_ANALYST_SH_PASSWORD"

# 5. 执行 L4 运行态认证落库
CERT_OUT=$(api POST /api/xblue/subgraphs/fin_risk/certify '{}')
CERT_LVL=$(printf '%s' "$CERT_OUT" | jq -r '.cert_level // empty')
if [[ "$CERT_LVL" != "L4_CERTIFIED" ]]; then
  echo "fin_risk.sh: expected L4_CERTIFIED, got: $CERT_OUT" >&2
  exit 1
fi

echo "FIN_RISK_DATASET_ID=$FIN_DS_ID"
