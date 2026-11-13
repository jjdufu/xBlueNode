## 变更所属模块
- [ ] `axpoint-core` (Rust)
- [ ] `axpoint-parser` (Python)
- [ ] `xbluenode-web` (React/TS)
- [ ] `deploy/migrations` (PostgreSQL)
- [ ] `docs/blueprint` (架构文档)

## 变更说明
<!-- 简述本次 PR 对应的蓝图 Step 与核心实现 -->

## 铁律合规检查（对照根目录 `IRON_RULES.md`）
- [ ] 使用 100% `xBlueNode / AxPoint` 自主命名体系，无外部品牌字眼
- [ ] 未在 `main.rs`、`api.ts`、`init_schema.sql` 集中堆砌跨模块耦合代码
- [ ] 新增数据库表结构已拆分为独立编号迁移文件 `deploy/migrations/00X_*.sql`
- [ ] Rust 已通过 `cargo clippy --workspace --all-targets -- -D warnings` 与 `cargo test --workspace`
- [ ] Python 已通过 `pytest -v axpoint-parser/tests`
- [ ] 前端已通过 `npm run build`
- [ ] 无 `target/`、`__pycache__/`、`*.egg-info/`、`*.tsbuildinfo`、`.env` 等无用文件提交
