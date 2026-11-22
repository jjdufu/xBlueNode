## 变更所属模块
- [ ] `axpoint-core` (Rust)
- [ ] `axpoint-parser` (Python)
- [ ] `xbluenode-web` (React/TS)
- [ ] `deploy/migrations` (PostgreSQL)
- [ ] `docs/` / `plans/` (规范、门禁与实施方案)

## 变更说明
<!-- 写明本次 PR 对应的计划 Step 编号（见 plans/ 目录总索引）与核心实现 -->

## 铁律合规检查（对照根目录 `IRON_RULES.md` 与 `AGENTS.md`）
- [ ] 使用 100% `xBlueNode / AxPoint` 自主命名体系，无外部品牌字眼
- [ ] 写入范围符合 `docs/MULTI_AGENT_TEAM_WORKFLOW.md` 角色边界（`bash scripts/check-write-scope.sh` 通过）
- [ ] 未在 `main.rs`、`registry.ts`、单个迁移文件中集中堆砌跨模块耦合代码
- [ ] 表结构变更使用独立的 sqlx 编号迁移文件 `deploy/migrations/NNNN_<module>.sql`，已合入的迁移未被改动
- [ ] 本地已通过静态守卫：`naming-guard.sh`、`lint-guard.py`、`web-guard.py`、`python-guard.py`、`hallucination-guard.py`、`docs-and-plans-guard.py`
- [ ] Rust 已通过 `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`、`cargo doc` 与 `cargo test --workspace --locked`
- [ ] Python 已通过 `ruff check .`、`mypy --strict .` 与 `pytest -v axpoint-parser/tests`
- [ ] 前端已通过 `npm run build`
- [ ] 无 `target/`、`__pycache__/`、`*.egg-info/`、`*.tsbuildinfo`、`.env` 等无用文件提交
