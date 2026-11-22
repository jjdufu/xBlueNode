---
name: multi-agent-coordinator
description: >-
  Orchestrate multi-team, multi-branch Git worktrees, and role-isolated
  Research / Author / Verifier / RedTeam subagents in xBlueNode / AxPoint
  without file conflicts or port collisions.
---

# Multi-Agent & Multi-Team Coordinator Skill

Activate this skill when coordinating multiple parallel tasks, teams, branches, or subagents across the `xBlueNode / AxPoint` monorepo. Full specification: [MULTI_AGENT_TEAM_WORKFLOW.md](../../../docs/MULTI_AGENT_TEAM_WORKFLOW.md).

## 1. Disjoint File Ownership Enforcement

Never assign overlapping file paths to concurrent agents. Every implementation task maps to one role in `scripts/check-write-scope.sh`:
- `orchestrator`: Contracts, migrations, root assembly (`Cargo.toml`, `ports.rs`, `enums.rs`, `main.rs`, `routes/mod.rs`, `App.tsx`, `modules/{meta,registry}.ts`), `docs/`, `plans/`, guard scripts.
- `squad-data`: `axpoint-parser/**`, `axpoint-domain/src/{axhub,export}.rs`.
- `squad-analytics-guard`: `axpoint-analytics/**`, `axpoint-security/**`, `routes/{axhub,analytics,documents,axguard}.rs`, corresponding DB repos & cassettes.
- `squad-domain-agent`: `axpoint-llm/**`, `axpoint-flow/**`, `axpoint-agent/**`, `axpoint-domain/src/{expr,rollup,sim,studio}/**`, Studio/Flow/Agent/Sim/View/Ops repos & routes.
- `squad-web`: `xbluenode-web/src/**` (excluding the 4 orchestrator-locked entry files).
- `squad-qa`: `xbluenode-web/e2e/**`, `scripts/gate/step*.sh`, `scripts/{seed,bench,demo}/**`, `tests/**`.

## 2. Subagent Separation of Duties (No Self-Approval)

Split complex work within any squad into distinct subagent invocations:
1. **Research Subagent (Read-Only)**: Survey traits, schemas, and call sites; return exact file paths and line ranges.
2. **Author Subagent (Scoped Write)**: Pass explicit `ALLOWED_PATHS` and stack `AGENTS.md` constraints. Instruct the subagent to halt immediately rather than editing locked entrypoints if an upstream trait or enum is missing.
3. **Verifier Subagent (Fresh Context Review)**: Run `bash scripts/check-write-scope.sh <role> origin/main...HEAD`, all G1 static guards, compiler/linter suites, and contract field comparisons against [API_CONTRACT.md](../../../docs/API_CONTRACT.md).
4. **RedTeam Subagent (Security & Grounding Audit)**: Required whenever SQL, connectors, RBAC/ABAC/RLS, PII masking, or LLM prompts/parsers are modified.

## 3. Concurrent Worktree & Test Isolation

1. Create sibling worktrees (`git worktree add ../wt-<role>-<topic> -b feat/<role>-<topic> main`) rather than switching branches in-place.
2. When running concurrent integration tests on a shared host (such as the `.50` test server), assign distinct `WT_SLOT` database names (`xbluenode_wt1..4`) and non-overlapping `CORE_BASE_URL` / `PARSER_BASE_URL` ports per [MULTI_AGENT_TEAM_WORKFLOW.md](../../../docs/MULTI_AGENT_TEAM_WORKFLOW.md) §2.3.
3. Run final `scripts/gate/stepNN.sh` and `scripts/gate/all.sh` sequentially on the clean primary slot after `bash scripts/dev-down.sh`.
