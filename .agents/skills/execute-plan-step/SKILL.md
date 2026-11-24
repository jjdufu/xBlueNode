---
name: execute-plan-step
description: >-
  Execute a Step blueprint from plans/ in xBlueNode / AxPoint following the
  mandatory two-phase Contract-First PR workflow, SQLx idempotent migration rules,
  stack-level zero-bypass guards, and executable gate verification (stepNN.sh).
---

# Execute Plan Step Workflow (`xBlueNode / AxPoint`)

Activate this skill whenever implementing or reviewing a Step defined under `plans/`.

## 1. Pre-Flight Context Check

1. Read [IRON_RULES.md](../../../IRON_RULES.md), [AGENTS.md](../../../AGENTS.md), and the active phase master index (see [00-V3-MASTER-INDEX.md](../../../plans/completed/v3-craft-and-world-runtime-step23-30/00-V3-MASTER-INDEX.md) for the Phase 3 reference baseline).
2. Read the target Step blueprint file completely (objectives, prerequisites, file-change manifest, migration DDL, Rust/Python/Web contracts, and gate assertions).
3. Verify prerequisite Steps are already merged into `main`.

## 2. Phase 1 — Contract PR (`orchestrator` Role Only)

Create branch `feat/contract-step<NN>-<slug>` from `origin/main` and commit ONLY contract artifacts:
1. **Migration (`deploy/migrations/NNNN_<module>.sql`)**:
   - `CREATE TABLE IF NOT EXISTS` / `ALTER TABLE ... ADD COLUMN IF NOT EXISTS` / `CREATE INDEX IF NOT EXISTS`.
   - Call `SELECT ax_enable_tenant_rls('<table>');` at the end of every new tenant-scoped table.
   - Register the migration in [A2-DATABASE-MIGRATIONS-INDEX.md](../../../docs/standards/A2-DATABASE-MIGRATIONS-INDEX.md).
2. **Domain Contracts (`axpoint-core/crates/axpoint-domain/`)**:
   - Pure DTOs, enums, error codes, and port traits (`impl Future<Output = Result<T, AppError>> + Send`), zero IO crate dependencies.
   - Update [A1-CONVENTIONS-ENUMS-ROUTES.md](../../../docs/standards/A1-CONVENTIONS-ENUMS-ROUTES.md) if new enums, permissions, or audit actions are added.
3. **API & Frontend Type Contracts**:
   - Document endpoints in [API_CONTRACT.md](../../../docs/API_CONTRACT.md) (and [A4-API-CONTRACT-INDEX.md](../../../docs/standards/A4-API-CONTRACT-INDEX.md) once route stubs are mounted).
   - Add Pydantic models in `axpoint-parser/schemas.py` (if Sidecar is touched) and TypeScript types in `xbluenode-web/src/lib/api/<module>.ts`.
4. **Gate Script (`scripts/gate/step<NN>.sh`)**:
   - Source `lib.sh`, assert cross-tenant RLS (`0` rows visible to `other` tenant), assert audit records, and end with `gate_pass step<NN>`.

Run G1 static guards + G2 compile checks before merging the Contract PR.

## 3. Phase 2 — Parallel Implementation PRs (Isolated Worktrees)

Create isolated worktrees (`git worktree add ../wt-<squad>-step<NN> -b feat/<squad>-step<NN>-<topic> main`) per squad:
- Enforce stack rules in [axpoint-core/AGENTS.md](../../../axpoint-core/AGENTS.md), [axpoint-parser/AGENTS.md](../../../axpoint-parser/AGENTS.md), and [xbluenode-web/AGENTS.md](../../../xbluenode-web/AGENTS.md).
- Verify write scope before committing:
  ```bash
  bash scripts/check-write-scope.sh <squad-role> origin/main...HEAD
  ```

## 4. Phase 3 — Integration, Gate & CI Verification

1. Run all static guards (`naming-guard.sh`, `lint-guard.py`, `web-guard.py`, `python-guard.py`, `hallucination-guard.py`, `docs-and-plans-guard.py`, `check_routes.py`, `check_traceability.py`).
2. Run stack unit/doc tests and `bash scripts/gate/step<NN>.sh` (confirm last line `GATE step<NN> PASS`).
3. Open PR against `main`, confirm all GitHub Actions jobs pass green, merge, and remove worktree/branch.
