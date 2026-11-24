#!/usr/bin/env bash
# xBlueNode / AxPoint 多团队、多分支、多 Subagent 独占写入边界核验脚本。
# 对照 docs/MULTI_AGENT_TEAM_WORKFLOW.md §1.1 所有权锁表，阻断执行者越权修改公共入口或他队目录。
#
# 用法：
#   bash scripts/check-write-scope.sh <role> [git-diff-range]
#   bash scripts/check-write-scope.sh --self-test
#
# 支持角色（role）：
#   orchestrator | squad-data | squad-analytics-guard | squad-domain-agent | squad-web | squad-qa
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

ORCHESTRATOR_WEB_LOCKED_REGEX='^xbluenode-web/src/(App\.tsx|modules/meta\.ts|modules/registry\.ts|lib/api/enums\.ts)$'

is_allowed_for_role() {
  local role="$1"
  local file="$2"

  case "$role" in
    orchestrator)
      # L1 架构总控允许修改契约、迁移、公共装配入口、全局规范、实施方案与守卫脚本
      return 0
      ;;
    squad-data)
      [[ "$file" =~ ^axpoint-parser/ ]] && [[ "$file" != "axpoint-parser/AGENTS.md" ]] && return 0
      [[ "$file" =~ ^axpoint-core/crates/axpoint-domain/src/(axhub|export)\.rs$ ]] && return 0
      [[ "$file" =~ ^deploy/collectors/ ]] && return 0
      [[ "$file" =~ ^scripts/seed/(01|02|12)- ]] && return 0
      return 1
      ;;
    squad-analytics-guard)
      [[ "$file" =~ ^axpoint-core/crates/(axpoint-analytics|axpoint-security)/ ]] && return 0
      [[ "$file" =~ ^axpoint-core/crates/axpoint-domain/src/(analytics|guard|grounding)\.rs$ ]] && return 0
      [[ "$file" =~ ^axpoint-core/crates/axpoint-domain/src/algo/ ]] && return 0
      [[ "$file" =~ ^axpoint-core/crates/axpoint-db/src/(hub_repo|axhub_repo|graph_repo|graph_query|graph_algo_duckdb|analytics_repo|rules_repo|guard_repo|sim_visibility)\.rs$ ]] && return 0
      [[ "$file" =~ ^axpoint-core/crates/axpoint-db/src/(rules_repo|guard_repo)/ ]] && return 0
      [[ "$file" =~ ^axpoint-core/crates/axpoint-db/tests/(rules_repo|step02_03_db|step04_analytics_db|step2[79]_[a-z0-9_]+)\.rs$ ]] && return 0
      [[ "$file" =~ ^axpoint-core/crates/axpoint-server/src/routes/(axhub|analytics|documents|axguard)\.rs$ ]] && return 0
      [[ "$file" =~ ^axpoint-core/crates/axpoint-server/src/routes/axguard/ ]] && return 0
      [[ "$file" =~ ^axpoint-core/crates/axpoint-server/tests/step2[79]_[a-z0-9_]+\.rs$ ]] && return 0
      [[ "$file" =~ ^tests/cassettes/ ]] && return 0
      return 1
      ;;
    squad-domain-agent)
      [[ "$file" =~ ^axpoint-core/crates/(axpoint-llm|axpoint-flow|axpoint-agent)/ ]] && return 0
      [[ "$file" =~ ^axpoint-core/crates/axpoint-domain/src/(agent|builder|flow|geo_ops|graph|im|ops|rules|topology|view|view_zones)\.rs$ ]] && return 0
      [[ "$file" =~ ^axpoint-core/crates/axpoint-domain/src/(expr|rollup|sim|studio)/ ]] && return 0
      [[ "$file" =~ ^axpoint-core/crates/axpoint-domain/tests/(studio|expr_rollup|sim|step2[5689]_[a-z0-9_]+)\.rs$ ]] && return 0
      [[ "$file" =~ ^axpoint-core/crates/axpoint-db/src/(studio_repo|flow_repo|agent_repo|im_repo|sim_repo|view_repo|ops_repo|tokens|nodes|metrics|topology|mapping_exec|aggregate_exec|action_exec|entity_link|knowledge_sync)\.rs$ ]] && return 0
      [[ "$file" =~ ^axpoint-core/crates/axpoint-db/src/(agent_repo|im_repo|ops_repo)/ ]] && return 0
      [[ "$file" =~ ^axpoint-core/crates/axpoint-db/tests/(studio_repo|agent_repo|im_repo|sim_repo|view_repo|step06_exec|step2[5689]_[a-z0-9_]+)\.rs$ ]] && return 0
      [[ "$file" =~ ^axpoint-core/crates/axpoint-server/src/(ops|view)/ ]] && return 0
      [[ "$file" =~ ^axpoint-core/crates/axpoint-server/src/routes/(xblue|xblue_graphql|axflow|axagent|published|mcp|axsim|xblueview|axops)\.rs$ ]] && return 0
      [[ "$file" =~ ^axpoint-core/crates/axpoint-server/tests/step2[5689]_[a-z0-9_]+\.rs$ ]] && return 0
      [[ "$file" =~ ^templates/packs/ ]] && return 0
      [[ "$file" =~ ^scripts/ops/ ]] && return 0
      return 1
      ;;
    squad-web)
      if [[ "$file" =~ $ORCHESTRATOR_WEB_LOCKED_REGEX ]]; then
        return 1
      fi
      [[ "$file" =~ ^xbluenode-web/src/ ]] && return 0
      [[ "$file" =~ ^xbluenode-web/package(-lock)?\.json$ ]] && return 0
      return 1
      ;;
    squad-qa)
      [[ "$file" =~ ^xbluenode-web/e2e/ ]] && return 0
      [[ "$file" =~ ^scripts/gate/step[0-9]+\.sh$ ]] && return 0
      [[ "$file" =~ ^scripts/gate/tools/ ]] && return 0
      [[ "$file" =~ ^scripts/(seed|bench|demo)/ ]] && return 0
      [[ "$file" =~ ^tests/ ]] && return 0
      return 1
      ;;
    *)
      echo "check-write-scope: 未知角色 '$role'" >&2
      exit 2
      ;;
  esac
}

is_allowed_for_composite_role() {
  local role_expr="$1"
  local file="$2"
  if [[ "$role_expr" == *"+"* ]]; then
    local IFS='+'
    local parts=($role_expr)
    local sub_role
    for sub_role in "${parts[@]}"; do
      if [[ "$sub_role" == "orchestrator" || -z "$sub_role" ]]; then
        echo "check-write-scope: 组合角色 '$role_expr' 禁止混入 orchestrator 或空项" >&2
        exit 2
      fi
    done
    for sub_role in "${parts[@]}"; do
      if is_allowed_for_role "$sub_role" "$file"; then
        return 0
      fi
    done
    return 1
  fi
  is_allowed_for_role "$role_expr" "$file"
}

run_self_test() {
  # 正向断言
  is_allowed_for_role "orchestrator" "deploy/migrations/0015_llm_structured_grounding.sql"
  is_allowed_for_role "squad-data" "axpoint-parser/connectors/sql_family.py"
  is_allowed_for_role "squad-analytics-guard" "axpoint-core/crates/axpoint-analytics/src/nl2sql/mod.rs"
  is_allowed_for_role "squad-analytics-guard" "axpoint-core/crates/axpoint-db/src/sim_visibility.rs"
  is_allowed_for_role "squad-analytics-guard" "axpoint-core/crates/axpoint-db/src/graph_algo_duckdb.rs"
  is_allowed_for_role "squad-domain-agent" "axpoint-core/crates/axpoint-agent/src/mcp_server.rs"
  is_allowed_for_role "squad-domain-agent" "axpoint-core/crates/axpoint-domain/src/builder.rs"
  is_allowed_for_role "squad-domain-agent" "axpoint-core/crates/axpoint-db/src/action_exec.rs"
  is_allowed_for_role "squad-domain-agent" "axpoint-core/crates/axpoint-server/src/ops/templates.rs"
  is_allowed_for_role "squad-domain-agent" "axpoint-core/crates/axpoint-server/src/view/ai_gen.rs"
  is_allowed_for_role "squad-domain-agent" "axpoint-core/crates/axpoint-domain/src/view_zones.rs"
  is_allowed_for_role "squad-domain-agent" "axpoint-core/crates/axpoint-domain/src/geo_ops.rs"
  is_allowed_for_role "squad-web" "xbluenode-web/src/pages/XBlueStudioPage.tsx"
  is_allowed_for_role "squad-qa" "scripts/gate/step16.sh"
  is_allowed_for_composite_role "squad-web+squad-qa" "xbluenode-web/e2e/smoke.spec.ts"
  is_allowed_for_composite_role "squad-web+squad-qa" "xbluenode-web/src/pages/XBlueStudioPage.tsx"

  # 越界阻断断言（期望返回非 0）
  if is_allowed_for_role "squad-web" "xbluenode-web/src/modules/registry.ts"; then
    echo "check-write-scope self-test FAILED: squad-web 不得修改 registry.ts" >&2
    exit 1
  fi
  if is_allowed_for_composite_role "squad-web+squad-qa" "xbluenode-web/src/App.tsx"; then
    echo "check-write-scope self-test FAILED: squad-web+squad-qa 不得修改 App.tsx" >&2
    exit 1
  fi
  if is_allowed_for_role "squad-domain-agent" "axpoint-core/crates/axpoint-domain/src/ports.rs"; then
    echo "check-write-scope self-test FAILED: squad-domain-agent 不得直接修改 ports.rs" >&2
    exit 1
  fi
  if is_allowed_for_role "squad-analytics-guard" "deploy/migrations/0016_grounded_ask_eval.sql"; then
    echo "check-write-scope self-test FAILED: 分队不得直接写入 deploy/migrations/" >&2
    exit 1
  fi
  if is_allowed_for_role "squad-data" "axpoint-parser/AGENTS.md"; then
    echo "check-write-scope self-test FAILED: 分队不得直接篡改项目记忆 AGENTS.md" >&2
    exit 1
  fi

  echo "check-write-scope: self-test OK"
}

if [[ "${1:-}" == "--self-test" ]]; then
  run_self_test
  exit 0
fi

ROLE="${1:-}"
RANGE="${2:-origin/main...HEAD}"

if [[ -z "$ROLE" ]]; then
  echo "用法: bash scripts/check-write-scope.sh <role|--self-test> [git-diff-range]" >&2
  exit 2
fi

cd "$ROOT"
CHANGED_FILES=()
while IFS= read -r line; do
  [[ -n "$line" ]] && CHANGED_FILES+=("$line")
done < <(git diff --name-only "$RANGE")

if [[ ${#CHANGED_FILES[@]} -eq 0 ]]; then
  echo "check-write-scope ($ROLE): OK (0 changed files in $RANGE)"
  exit 0
fi

VIOLATIONS=()
for f in "${CHANGED_FILES[@]}"; do
  if ! is_allowed_for_composite_role "$ROLE" "$f"; then
    VIOLATIONS+=("$f")
  fi
done

if [[ ${#VIOLATIONS[@]} -gt 0 ]]; then
  echo "check-write-scope FAILED: 角色 '$ROLE' 越界修改了以下非授权路径：" >&2
  for v in "${VIOLATIONS[@]}"; do
    echo "  - $v" >&2
  done
  exit 1
fi

echo "check-write-scope ($ROLE): OK (${#CHANGED_FILES[@]} files verified)"
