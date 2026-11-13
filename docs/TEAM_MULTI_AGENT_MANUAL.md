# xBlueNode / AxPoint 团队与多 Agent 协同实操手册

> **适用范围**：[xBlueNode / AxPoint 工程落地总蓝图](./blueprint/00-MASTER-INDEX.md)（Step 00～14）在 GitHub Monorepo 中的多人 / 多 AI Agent 并发实施，须与根目录 [IRON_RULES.md](../IRON_RULES.md) 配合执行。

---

## 1. 角色与写入范围

编制为「1 名总控 + 4 名执行者（工程师或 Agent）」；人员较少时可把 Agent B 与 Agent C 合并为一人。

| 角色 | 独占写入范围 | 负责的 Step |
| :--- | :--- | :--- |
| **Orchestrator（总控与契约官）** | `IRON_RULES.md`、`docs/`、`axpoint-core/Cargo.toml`、`axpoint-domain/src/{ports.rs, enums.rs}`、`axpoint-db/src/lib.rs`、`axpoint-server/src/{main.rs, state.rs, routes/mod.rs}`、`deploy/migrations/`、`xbluenode-web/src/modules/registry.ts`、`scripts/gate/{lib.sh, all.sh}` | 每个 Step 的契约 PR、集成挂载、门禁与合入审查；Step 00 |
| **Agent A（Python 数据与出品）** | `axpoint-parser/{parsers, connectors, lake, exporters, charts, pii, schemas.py, main.py, pyproject.toml, tests}/`、`deploy/docker/axpoint-parser.Dockerfile`、`deploy/docker-compose.test.yml` | Step 01、02（Sidecar 部分）、12（出品器与服务端图表）、13（PII 识别） |
| **Agent B（Rust 存储、分析与安全）** | `axpoint-core/crates/{axpoint-analytics, axpoint-security}/`、`axpoint-db/src/{hub_repo, graph_repo, analytics_repo, rules_repo, guard_repo}.rs` | Step 02（Core 部分）、03、04、08（规则仓储与求值）、13 |
| **Agent C（Rust 领域、编排、智能体与推演）** | `axpoint-core/crates/{axpoint-llm, axpoint-flow, axpoint-agent}/`、`axpoint-domain/src/{expr, rollup, sim, graph}/`、`axpoint-db/src/{studio_repo, flow_repo, agent_repo, sim_repo, view_repo, ops_repo}.rs`、`axpoint-server/src/routes/<module>.rs` | Step 05、06、07、08（告警路由）、09、10、11（Core 部分）、14（Core 部分） |
| **Agent D（React 前端）** | `xbluenode-web/src/{lib/api/<module>.ts, pages/, components/<module>/}`、`xbluenode-web/e2e/` | 全部 Step 的前端部分 |

每个 Step 文件的“文件变更清单”是写入范围的最终依据；清单中的公共入口文件（`Cargo.toml`、`axpoint-db/src/lib.rs`、`main.rs`、`state.rs`、`routes/mod.rs`、`registry.ts`、迁移目录）只由总控修改。

---

## 2. 环境与 Git Worktree 隔离

禁止多个执行者在同一个克隆目录里反复切换分支：Rust `target/` 增量缓存失效，`node_modules` 与构建产物互相覆盖。

### 2.1 初始化

```bash
git clone https://github.com/jjdufu/xBlueNode.git
cd xBlueNode                                  # 总控目录，始终追踪 main

git worktree add ../wt-a-parser    -b feat/parser-step01    main
git worktree add ../wt-b-graph     -b feat/axgraph-step03   main
git worktree add ../wt-c-domain    -b feat/axflow-step07    main
git worktree add ../wt-d-web       -b feat/web-axhub        main
```

门禁环境固定使用 PostgreSQL `5432`、Sidecar `8001`、Core `9995`（Step 00 §00.10），同一台机器同一时刻只能运行一套：多个执行者并行时各自使用独立的开发容器或虚拟机；共用一台机器时依次执行，切换 worktree 前先运行 `bash scripts/dev-down.sh`。

### 2.2 一个依赖层合入后的同步

```bash
git fetch origin main && git rebase origin/main     # 各执行者立即同步
git worktree remove ../wt-a-parser                  # 分支合入后删除 worktree 与分支
git branch -d feat/parser-step01
git worktree add ../wt-a-parser -b feat/axhub-step02 main
```

---

## 3. 契约先行 + 按依赖层并行

依赖层定义见 [00-MASTER-INDEX.md](./blueprint/00-MASTER-INDEX.md) §7。每个 Step 都分三段：**① 总控合入契约 PR → ② 执行者并行实现 → ③ 总控挂载并跑门禁合入**。同一依赖层内的多个 Step 的契约 PR 按迁移编号从小到大依次合入。

| 依赖层 | Step | 契约 PR 内容（总控） | 并行实现 | 合入门禁 |
| :--- | :--- | :--- | :--- | :--- |
| L0 | 00 | `0002_platform_foundation.sql`、`AppError` 全集、`enums.rs`、基础端口、`lib/api/{auth,enums}.ts` | 总控独立完成 | `step00.sh` |
| L1 | 01、03 | `schemas.py` 统一契约；`0004_axgraph_timeseries.sql`、图查询 DTO、`lib/api/xblue.ts` 扩展 | A：Step 01；B：Step 03；D：解析预览页、图浏览页 | `step01.sh`、`step03.sh` |
| L2 | 02 | `0003_data_fabric.sql`、连接器协议、`lib/api/axhub.ts` | A：Sidecar 连接器与入湖；B：Core 路由、密钥环、调度；D：AxHub 页面 | `step02.sh` |
| L3 | 04 | `0005_analytics_semantic.sql`、创建 `axpoint-analytics` 成员 | B：DuckDB 会话、守卫、透视、语义层、问数；D：AxQuery 页面 | `step04.sh` |
| L4 | 05 | `0006_xbluestudio_modeling.sql`、多子图与模型文档类型 | C：子图建模与融合、校验、发布、差异、AI 草案、动态 GraphQL；D：建模与融合画布 | `step05.sh` |
| L5 | 06 | 表达式 AST 与求值报告类型 | C：表达式引擎、映射物化、动态聚合 | `step06.sh` |
| L6 | 07 | `0007_axflow_orchestration.sql`、创建 `axpoint-flow` 成员、算子注册表 trait | C：DAG 引擎与算子；D：编排画布 | `step07.sh` |
| L7 | 08、09、10 | `0008`、`0009`（创建 `axpoint-agent` 与 `SimExecutionPort` 解耦端口）、`0010` 依次合入 | B：Step 08 规则求值与通知；C：Step 09、10（按独立路由文件隔离并行）；D：告警中心、智能体工作台、推演沙盘 | `step08.sh`、`step09.sh`、`step10.sh` |
| L8 | 11 | `0011_xblueview_dashboards.sql`、组件数据源与 3D 场景绑定类型 | C：组件执行、AI 看板生成、动作回写、缓存、SSE；D：驾驶舱、3D 孪生场景、地图、大屏 | `step11.sh` |
| L9 | 12 | `0012_export_and_share.sql`、`ExportDocument` | A：出品器与服务端图表；C：出品任务与分享；D：出品中心 | `step12.sh` |
| L10 | 13 | `0013_axguard_policies.sql`、策略与授权类型 | B：认证、令牌、Cedar、行列策略、审计；A：PII 引擎；D：登录与治理页面 | `step13.sh` |
| L11 | 14 | `0014_ops_templates_delivery.sql` | C：模板安装、工作项、运维接口；总控：部署、`axpoint-ops` 备份恢复执行器、总验收；D：运维面板 | `step14.sh`、`all.sh` |

---

## 4. 高发问题与规避

### 4.1 Rust 工具链版本不一致
`axpoint-core/Cargo.toml` 声明 `edition = "2024"`、`rust-version = "1.100.0"`。所有环境执行 `rustup toolchain install stable --component clippy rustfmt` 并确认 `rustc >= 1.100.0`。禁止修改 `rust-version` 或删除 `[workspace.lints]`；以 CI 的 `rust-core` 作业为准。

### 4.2 严格 Lint 阻断编译
工作区启用 `unsafe_code = "forbid"`、`unwrap_used` / `expect_used` / `panic` / `as_conversions` / `indexing_slicing` / `missing_docs` 均为 `deny`。
- 集合访问用 `.get(i)`、`.first()` 或迭代器；数值转换用 `TryFrom` / `From`。
- 所有公开符号写 `///` 文档；返回 `Result` 的公开函数写 `/// # Errors`。
- 提交前执行 `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings`。

### 4.3 公共入口合并冲突
`Cargo.toml` 成员、`routes/mod.rs` 的 `.merge(...)`、`AppState` 字段、`registry.ts` 页面注册只由总控在契约 PR 或集成 PR 中修改。执行者在自己的 `routes/<module>.rs` 中导出 `pub fn router() -> Router<AppState>`，在 PR 描述中写出需要的挂载语句。

### 4.4 Crate 之间直接依赖
能力 Crate 之间禁止直接 `use` 对方的实现类型。跨 Crate 能力先由总控在 `axpoint-domain/src/ports.rs` 定义端口。端口方法返回 `impl Future + Send`，不能做成 trait object：能力 Crate 用**泛型参数**消费端口，`axpoint-server` 装配具体类型；算子注册表、连接器注册表等需要运行时多态的位置使用返回 `Pin<Box<dyn Future>>` 的对象安全 trait（见蓝图 A1 §8）。

### 4.5 迁移编号冲突与非幂等脚本
迁移为 sqlx 格式 `NNNN_<module>.sql`，编号与归属 Step 固定在 [A2](./blueprint/A2-DATABASE-MIGRATIONS-INDEX.md)，只由总控在契约 PR 中创建。已合入的迁移只读；结构调整写下一个编号。新表 `CREATE TABLE IF NOT EXISTS`，已有表 `ALTER TABLE ... ADD COLUMN IF NOT EXISTS`；带 `tenant_id` 的表末尾执行 `ax_enable_tenant_rls`。规则全文见 [deploy/migrations/README.md](../deploy/migrations/README.md)。

### 4.6 前端等待后端
契约 PR 合入后 Agent D 立即开工：按 [API_CONTRACT.md](./API_CONTRACT.md) 与 [A4](./blueprint/A4-API-CONTRACT-INDEX.md) 写 `lib/api/<module>.ts` 类型，用模块内 Mock 夹具开发页面；后端合入后关闭 Mock 直连。

### 4.7 Sidecar 安全
外部标识符经白名单正则 `^[A-Za-z_][A-Za-z0-9_]*$` 校验，值一律参数化；连接器只读。凭证只以 Core 密钥环加密存储，Sidecar 只在请求内存中持有明文，响应与日志一律掩码。按 `parsers/`、`connectors/`、`lake/`、`exporters/` 分包注册，依赖写入 `pyproject.toml` 并符合 [A6](./blueprint/A6-DEPENDENCY-BASELINE.md)。

### 4.8 命名污染与误提交
注释、样例、文档只使用 [A1](./blueprint/A1-CONVENTIONS-ENUMS-ROUTES.md) §1 的自主命名词表。提交前执行 `bash scripts/naming-guard.sh`（读取本地 `NAMING_DENYLIST`）与 `git status`，确认没有 `target/`、`node_modules/`、`dist/`、`__pycache__/`、`.env`、`.gate/`。

### 4.9 镜像版本错配与静态资源
`axpoint-server` 通过 `WEB_DIST_DIR` 托管前端 `dist/`，不引入 `rust-embed`。镜像发布为 `${AXPOINT_REGISTRY:-dex2all}/axpoint-core:<tag>` 与 `axpoint-parser:<tag>-{lite,full}`，两者使用同一语义化版本并附 Git SHA 标签，不使用裸 `latest`。

---

## 5. 协同提示词模板

### 5.1 总控：契约 PR

```markdown
你是 xBlueNode / AxPoint 项目的总控（Orchestrator）。
先阅读 `IRON_RULES.md`、`docs/blueprint/00-MASTER-INDEX.md`、`docs/blueprint/A1-CONVENTIONS-ENUMS-ROUTES.md` 与本次 Step 文件 `{STEP_FILE}` 的 Step {STEP_ID}。

只完成 Step {STEP_ID} 的契约 PR，不写业务实现：
1. 按 Step 文件“迁移”小节创建 `deploy/migrations/{MIGRATION_FILE}`，内容与 A2 条目一致。
2. 按“契约清单”在 `axpoint-domain` 定义 DTO、枚举与端口；需要的新 Crate 在 `axpoint-core/Cargo.toml` 声明并继承 `[workspace.lints]`。
3. 写入 `docs/API_CONTRACT.md` 条目、`axpoint-parser/schemas.py`（如涉及 Sidecar）、`xbluenode-web/src/lib/api/{MODULE}.ts` 类型、A6 中该 Step 的依赖。
4. 创建 `scripts/gate/step{STEP_ID}.sh`（内容取自 Step 文件“验收”小节）。

验收：全局门禁（00-MASTER-INDEX §9）全绿；输出可并行派发给 Agent A / B / C / D 的任务清单与写入范围。
```

### 5.2 执行者：实现 PR

```markdown
你在 worktree `{WORKTREE_PATH}` 的分支 `{BRANCH_NAME}` 上实施 xBlueNode / AxPoint。

必读：`IRON_RULES.md`；`docs/TEAM_MULTI_AGENT_MANUAL.md` 第 4 节；`{STEP_FILE}` 中 Step {STEP_ID} 的全部内容；`docs/blueprint/A1-CONVENTIONS-ENUMS-ROUTES.md`；`docs/API_CONTRACT.md`。

交付：Step {STEP_ID} “文件变更清单”中属于 {ROLE} 的全部条目：
{TASK_DETAILS}

写入范围：只允许修改 `{ALLOWED_PATHS}`。公共入口（`Cargo.toml`、`main.rs`、`routes/mod.rs`、`registry.ts`、`deploy/migrations/`）与 `axpoint-domain/src/ports.rs` 不得修改；需要挂载或契约字段缺失时停止并在汇报中写明。

编码约束：Rust 严格 Lint；Python 3.11 类型注解 + Pydantic 2.x，SQL 标识符白名单 + 参数化；TypeScript 严格模式禁用 `any`；只使用自主命名词表。

提交前贴出实测输出：
1. `bash scripts/gate/step{STEP_ID}.sh` 最后一行 `GATE step{STEP_ID} PASS`；
2. 对应栈检查：`cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`；`cd axpoint-parser && ruff check . && mypy --strict . && pytest -v tests`；`cd xbluenode-web && npm ci && npm run build`；
3. `git status` 无构建产物与 `.env`。
```

### 5.3 审查：合入前

```markdown
审查 PR 分支 `{BRANCH_NAME}`（目标 `main`），逐条核验：
1. 写入范围：`git diff --name-only origin/main...{BRANCH_NAME}` 全部落在 `{ALLOWED_PATHS}`。
2. 命名：`bash scripts/naming-guard.sh` 通过。
3. 质量：Rust / Python / Web 检查与 `scripts/gate/step{STEP_ID}.sh`、`scripts/gate/all.sh` 全部通过。
4. 安全：连接器参数化 + 白名单；凭证加密且不回显；查询结果、LLM 提示、出品与分享经过统一脱敏；外部入口按蓝图 Step 13 §13.2 鉴权。
5. 契约：请求 / 响应字段、状态码、错误码与 `docs/API_CONTRACT.md`、A1、A4 一致。

输出：结论 `PASS` 或 `BLOCK`；逐项证据（文件与行号）；`BLOCK` 时给出最小修改清单。
```
