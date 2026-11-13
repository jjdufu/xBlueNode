# xBlueNode / AxPoint 团队与多 Agent 协同实操手册

> **适用范围**：本手册专用于 [xBlueNode / AxPoint 工程落地总蓝图](./blueprint/00-MASTER-INDEX.md)（**Step 01 ~ Step 12**）在 GitHub Monorepo 中的多人 / 多 AI Agent 并发开发，须与根目录 [IRON_RULES.md](../IRON_RULES.md) 配合执行。

---

## 1. 角色编制与零冲突分工矩阵

为兼顾并发吞吐与共享接口收敛，建议采用 **「1 名总控架构师 + 4 名并发执行 Agent（或工程师）」** 的编制（精简配置可合并 Rust 后端为 1 名，形成 `1 + 3`）。

| 角色标识 | 推荐配置 | 独占写入目录白名单（Strict Write Scope） | 对应蓝图步骤 |
| :--- | :--- | :--- | :--- |
| **Orchestrator（总控与契约官）** | 1 人或主 Agent | `IRON_RULES.md`、`docs/`、`axpoint-core/Cargo.toml`、`axpoint-core/crates/axpoint-domain/src/ports.rs`、`axpoint-core/crates/axpoint-server/src/{main.rs,state.rs,routes/mod.rs}`、`xbluenode-web/src/modules/registry.ts` | 全局波次调度、契约 PR、集成挂载与合入审查 |
| **Agent A（Python 数据与出品）** | 1 名 | `axpoint-parser/{parsers,connectors,exporters,schemas.py,main.py,pyproject.toml,tests}/` | **Step 01**（全格式解析）、**Step 02**（连接器/探查）、**Step 11**（全格式导出） |
| **Agent B（Rust 存储、OLAP 与安全）** | 1 名 | `axpoint-core/crates/{axpoint-db,axpoint-analytics,axpoint-security}/`、总控分配的 `deploy/migrations/00X_*.sql` | **Step 03**（超图+时序存储）、**Step 04**（DuckDB OLAP）、**Step 12**（AxGuard 安全审计） |
| **Agent C（Rust 领域、AI 与编排推演）** | 1 名 | `axpoint-core/crates/{axpoint-llm,axpoint-flow}/`、`axpoint-domain/src/topology.rs`、`axpoint-server/src/routes/<module>.rs` | **Step 05~06**（xBlueStudio 建模聚合）、**Step 07~08**（AxFlow DAG/MCP）、**Step 09**（AxSim 推演） |
| **Agent D（React 前端工作台）** | 1 名 | `xbluenode-web/src/{lib/api/<module>.ts,pages/,components/}/` | **Step 01~12** 全套前端工作台、画布、透视表与脱敏展示 |

---

## 2. 环境与 Git Worktree 隔离规范

禁止多个 Agent 在同一个本地克隆目录里反复切换分支——会导致 Rust `axpoint-core/target/` 增量缓存失效、Node `node_modules` 与 Vite 构建产物互相覆盖。

### 2.1 初始化多路 Worktree（以单仓上云后为例）

```bash
# 1. 主控保留主目录（始终追踪 main，仅做审核与集成）
git clone <你的仓库地址> xBlueNode-AxPoint
cd xBlueNode-AxPoint

# 2. 为 4 个执行 Agent 创建物理隔离的并行工作目录与分支
git worktree add ../wt-agent-a-parser    -b feat/parser-step01-02 main
git worktree add ../wt-agent-b-analytics -b feat/db-step03        main
git worktree add ../wt-agent-c-flow      -b feat/studio-step05    main
git worktree add ../wt-agent-d-web       -b feat/web-axhub        main
```

### 2.2 每个波次结束后的分支同步与清理

```bash
# 当总控将某波次 PR 合入 main 后，各 Agent 必须立即 rebase 最新 main：
git fetch origin main
git rebase origin/main

# 旧分支合入后清理重建下一波次 worktree，避免长期分叉导致冲突膨胀：
git worktree remove ../wt-agent-a-parser
git branch -d feat/parser-step01-02
```

---

## 3. 契约先行 + 分波次（Wave）并行推进表

遵循 [00-MASTER-INDEX.md](./blueprint/00-MASTER-INDEX.md) 的依赖拓扑，每个波次严格执行 **「Stage 1：总控合入契约 PR」→「Stage 2：多 Agent 并行实现」→「Stage 3：总控挂载与门禁合流」**。

| 波次 | Stage 1：总控契约 PR（先合入 `main`） | Stage 2：并发认领（互不阻塞） | Stage 3：合流验收门禁 |
| :--- | :--- | :--- | :--- |
| **Wave 1** | 预留迁移号 `002_data_fabric.sql`、`003_xblue_timeseries.sql`；更新 `API_CONTRACT.md`、Python `schemas.py`、`axpoint-domain/src/ports.rs` 与前端 `lib/api/axhub.ts` | • **Agent A**：Step 01 + Step 02 (`axpoint-parser`)<br>• **Agent B**：Step 03 (`axpoint-db` 超图+时序)<br>• **Agent D**：`AxHub` 数据源管理页面（Mock 先行） | `pytest` + `cargo clippy/test` + `npm run build` 全绿 |
| **Wave 2** | 注册新 Crate `axpoint-analytics` 到 `axpoint-core/Cargo.toml`；预留 `003b_xbluestudio_mappings.sql`；锁定 OLAP 与节点映射契约 | • **Agent B**：Step 04 (`axpoint-analytics` DuckDB OLAP + Text-to-SQL)<br>• **Agent C**：Step 05~06 (`xBlueStudio` 映射/聚合 + AI 发现)<br>• **Agent D**：`AxQuery` 问数透视表 + `xBlueStudio` 画布 | DuckDB 透视与 Text-to-SQL 只读校验通过、前端联动联调 |
| **Wave 3** | 注册新 Crate `axpoint-flow`；预留 `004_axflow_and_agents.sql`、`005_axsim_and_xblueview.sql`；锁定 DAG/MCP 与出品契约 | • **Agent C**：Step 07~08 (`axpoint-flow` DAG + MCP) & Step 09 (`AxSim` 多公式对比)<br>• **Agent A**：Step 11 (`axpoint-parser/exporters/` 全格式出品)<br>• **Agent D**：`AxFlow` 编排画布 + `AxSim` 对比沙盘 + `xBlueView` 看板 | DAG 防环/断点续跑单测、多格式文件导出字节校验 |
| **Wave 4** | 注册新 Crate `axpoint-security`；预留 `006_axguard_security.sql`；定义 `AxGuard` 认证与三道脱敏策略 Trait | • **Agent B**：Step 12 (`axpoint-security` + 行级权限 + 脱敏实现)<br>• **Agent C**：`axpoint-server` 中间件与 LLM/导出拦截接入<br>• **Agent D**：登录态、RBAC 权限控制与脱敏视图（Step 12） | 查询/LLM/导出三道 PII 脱敏拦截 E2E 全绿、`naming-guard` 通过 |

---

## 4. 本项目专属高发坑点与强制规避方案

针对 `xBlueNode-AxPoint` 的技术栈、严格 Lint 规则与蓝图架构，所有协作者必须执行以下 **9 项避坑规范**：

### 坑点 1：Rust MSRV 与本地工具链版本不一致导致门禁失效
- **现象**：[axpoint-core/Cargo.toml](../axpoint-core/Cargo.toml) 声明了 `edition = "2024"` 与 `rust-version = "1.100.0"`，若本地环境运行低于 `1.100.0` 的 `rustc`，直接执行 `cargo check` 会因版本检查报错。
- **规避**：
  1. 开发机与子 Agent 环境统一安装 `rustup toolchain install stable`（确保 `rustc >= 1.100.0`），并安装 `clippy` 与 `rustfmt`。
  2. 严禁任何子 Agent 为了绕过版本检查而私自修改 `Cargo.toml` 中的 `rust-version` 或删除 `[workspace.lints]` 严格规则；如受限于临时沙盒容器仅含旧版 `rustc`，只允许在本地预检命令追加 `--ignore-rust-version`，最终以 GitHub Actions `rust-core` 流水线（`dtolnay/rust-toolchain@stable`）为唯一准入标准。

### 坑点 2：触发 `clippy::pedantic`、`as_conversions`、`indexing_slicing` 与 `missing_docs` 编译失败
- **现象**：本项目在 [Cargo.toml](../axpoint-core/Cargo.toml#L16-L43) 启用了全工作区最严格 Lint（`unsafe_code = "forbid"`、`unwrap_used = "deny"`、`expect_used = "deny"`、`panic = "deny"`、`as_conversions = "deny"`、`indexing_slicing = "deny"`、`missing_docs = "deny"`）。AI Agent 按常规习惯写 `items[0]`、`val as f64`、`resp.unwrap()` 或漏写 `/// # Errors` 就会直接编译阻断。
- **规避**：
  1. 集合访问一律使用 `.get(idx)`、`.first()` 或迭代器（如 `.iter().enumerate()`）。
  2. 数值类型转换一律使用 `usize::try_from(...)`、`i64::from(...)` 或安全截断辅助函数，禁止 `as`。
  3. 所有 `pub struct`、`pub enum`、字段、`pub fn` 必须补全 `///` 文档注释；返回 `Result` 的公开函数必须带 `/// # Errors` 段。
  4. **每次提交前必跑**：`cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings`。

### 坑点 3：多 Agent 同时向 `axpoint-core/Cargo.toml`、`main.rs` 或 `registry.ts` 塞入新模块导致合并冲突
- **现象**：新增 `axpoint-analytics`、`axpoint-flow`、`axpoint-security` 或新增 HTTP 子路由、前端页面时，若多个分支同时改动 [Cargo.toml](../axpoint-core/Cargo.toml)、[main.rs](../axpoint-core/crates/axpoint-server/src/main.rs)、[routes/mod.rs](../axpoint-core/crates/axpoint-server/src/routes/mod.rs) 或 [registry.ts](../xbluenode-web/src/modules/registry.ts)，必出 Git 冲突。
- **规避**：
  1. **注册动作全部上收到总控**：新 Crate 声明、子路由 `.merge(...)` 挂载、`AppState` 新服务字段注入、`MODULE_REGISTRY` 菜单注册，统一在各波次的 **Stage 1（契约 PR）** 或 **Stage 3（集成 PR）** 由总控一次性完成。
  2. 执行 Agent 仅在自己的新 Crate 目录、`routes/<module>.rs` 或 `pages/<Page>.tsx` 内导出独立工厂函数/组件，不直接改公共入口。

### 坑点 4：多个 Rust Crate 互相直接依赖，破坏 `axpoint-domain` 端口隔离
- **现象**：例如在实现 **Step 07（AxFlow）** 或 **Step 06（动态聚合）** 时，Agent C 直接在 `axpoint-flow` 里 `use axpoint_db::PgStore` 或 `use axpoint_llm::AiGateway`，导致 Crate 环状或网状紧耦合，后续难以独立测试与替换实现。
- **规避**：
  1. 所有跨 Crate 能力调用（存储、LLM、OLAP 查询、安全脱敏）必须先由总控在 [ports.rs](../axpoint-core/crates/axpoint-domain/src/ports.rs) 定义 `trait`。
  2. 能力 Crate 仅依赖 `axpoint-domain` 并通过泛型/Trait Object（`Arc<dyn XxxPort>`）消费能力，仅在 `axpoint-server` 装配具体实现。

### 坑点 5：PostgreSQL 增量迁移编号撞车或非幂等脚本破坏旧库
- **现象**：Agent B 和 Agent C 在同一波次各自新建 `003_xxx.sql`，合并到 `main` 后出现相同编号或表字段冲突；或者未写 `IF NOT EXISTS` 导致二次执行失败。
- **规避**：
  1. 严格执行 [deploy/migrations/README.md](../deploy/migrations/README.md) 规则：编号按 `001`、`002`、`003`、`003b`、`004`、`005`、`006` 由**总控在 Stage 1 提前占位分配**，执行 Agent 只能往分配给自己的脚本里填充 DDL。
  2. 已经合并到 `main` 的迁移文件**永久冻结只读**，新增字段一律走下一个新编号迁移脚本（`ALTER TABLE ... ADD COLUMN IF NOT EXISTS`）。

### 坑点 6：前端等待后端接口写完才动工，导致工期与并发效率减半
- **现象**：Agent D 空转等待 Agent A/B/C 完成后端和数据库才开始写 React 页面，最后集成期前端集中爆发接口不对齐问题。
- **规避**：
  1. Stage 1 契约 PR 合入后，**Agent D 立即开工**：在 `xbluenode-web/src/lib/api/<module>.ts` 严格按 [API_CONTRACT.md](./API_CONTRACT.md) 定义 TS Request/Response 类型，并在模块内内置轻量 Mock 开关或静态夹具（Fixture）先行开发 `React Flow` 画布、`ECharts` 图表、`TanStack Table` 透视表。
  2. 后端合入后只需关闭 Mock 开关即可直连真接口联调。

### 坑点 7：Python Sidecar 连接器拼 SQL、漏校验、明文泄露密钥与依赖污染
- **现象**：实现 **Step 02（AxHub 连接器）** 时直接拼接表名/列名引发 SQL 注入；或将数据库密码明文返回给前端、写入 `tracing`/Python 日志；或在 `axpoint-parser` 根目录直接塞代码导致 `main.py` 膨胀。
- **规避**：
  1. 所有外部表名、列名、Schema 必须经过严格白名单正则（`^[A-Za-z_][A-Za-z0-9_]*$`）与只读 `SELECT` 校验；值参数一律走参数化查询。
  2. 数据源连接凭证在入库前加密、在 API 响应与日志中一律掩码脱敏，严禁明文落盘或打印。
  3. 严格按 `parsers/`、`connectors/`、`exporters/` 分包注册，新增依赖必须显式写入 `axpoint-parser/pyproject.toml` 并验证单元测试。

### 坑点 8：自主命名污染与构建缓存误提交被 CI 阻断
- **现象**：AI Agent 在注释、测试样例或文档中顺手写出外部竞品名称，或未开 `.gitignore` 误将 `target/`、`node_modules/`、`dist/`、`__pycache__/`、`.env` 提交进 PR，触发 [.github/workflows/ci.yml](../.github/workflows/ci.yml) 的 `naming-guard` 或代码审查拦截。
- **规避**：
  1. 严格核对 `IRON_RULES.md` 第一章自主命名词表（`AxHub`、`AxParser`、`AxGraph`、`AxAnalytics`、`AxQuery`、`xBlueStudio`、`xBlueGraph`、`AxFlow`、`AxSim`、`xBlueView`、`AxGuard`）。
  2. 每次 `git add` 前先执行 `git status`，确认无构建缓存与 `.env` 文件；提交前在本地跑一次与 CI 一致的关键词扫描脚本。

### 坑点 9：容器镜像版本错配与前端静态资源硬编码
- **现象**：将 `xbluenode-web/dist` 用 `rust-embed` 等方式硬编进 Rust 二进制导致企业定制换皮必须重编镜像；或 `dex2all/axpoint-core` 与 `dex2all/axpoint-parser` 分别发版时各自使用裸 `:latest` 导致跨服务接口/Parquet 契约错配。
- **规避**：
  1. 保持 `axpoint-server` 通过 `WEB_DIST_DIR` 目录托管前端 `dist/`（同容器交付、可挂载卷热替换定制资源），并预留压缩与 SPA `index.html` Fallback 扩展点，不引入 `rust-embed`。
  2. 容器发布统一推送至 `dex2all/axpoint-core:<tag>` 与 `dex2all/axpoint-parser:<tag>`（`axpoint-parser` 区分 `-lite` 与 `-full` 变体），双镜像保持同步语义化版本与 Git SHA 双标签；Compose 模板使用 `${AXPOINT_REGISTRY:-dex2all}` 以兼容企业内网私有仓库。

---

## 5. 标准协同提示词（Prompt）模板库

### 5.1 总控 / 架构师：Stage 1 契约 PR 下发 Prompt

```markdown
你是 xBlueNode / AxPoint 项目的架构总控（Orchestrator）。
请先阅读根目录 `IRON_RULES.md`、`docs/blueprint/00-MASTER-INDEX.md` 以及本波次蓝图：
- `{BLUEPRINT_FILES}`

请仅完成【{WAVE_NAME} · Stage 1 契约先行 PR】，不要编写具体业务实现：
1. 在 `deploy/migrations/` 新增并锁定本波次幂等 SQL 迁移占位文件：`{MIGRATION_FILES}`（严禁修改已合入的旧迁移）。
2. 在 `axpoint-core/crates/axpoint-domain/` 定义纯领域 DTO 与抽象端口 `trait`（更新 `ports.rs`），严禁引入数据库/HTTP/外部 SDK 依赖。
3. 如本波次需新增 Rust Crate，在 `axpoint-core/Cargo.toml` 声明成员并继承 `[workspace.lints]`。
4. 同步更新 `docs/API_CONTRACT.md`、`axpoint-parser/schemas.py`（如涉及 Sidecar）及前端强类型接口文件 `xbluenode-web/src/lib/api/{MODULE}.ts`。

验收要求：
- 100% 自主命名（不得出现任何外部竞品字眼）。
- 执行 `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace` 全绿。
- 输出本波次可并行派发给 Agent A / B / C / D 的任务清单与严格路径白名单。
```

---

### 5.2 并行执行 Agent（A / B / C / D）：Stage 2 落地开发 Prompt

```markdown
你正在独立工作目录 `{WORKTREE_PATH}` 的分支 `{BRANCH_NAME}` 上并行开发 xBlueNode / AxPoint。

【必读文件（动手前先完整阅读）】
1. `IRON_RULES.md`（最高红线，任何违反都会被 CI 与审核驳回）
2. `docs/TEAM_MULTI_AGENT_MANUAL.md`（特别注意第 4 节全部避坑规范）
3. 本次蓝图：`{BLUEPRINT_FILE}` 的 **{STEP_RANGE}**
4. 接口契约：`docs/API_CONTRACT.md`、`axpoint-core/crates/axpoint-domain/src/ports.rs`

【本次交付目标】
实现 **{MODULE_NAME}**（{STEP_RANGE}）：
{TASK_DETAILS}

【严格路径隔离】
- 仅允许新建或修改以下路径（白名单）：
  `{ALLOWED_PATHS}`
- 严禁修改：
  - 非本模块目录的任何文件
  - `axpoint-core/Cargo.toml`、`axpoint-core/crates/axpoint-server/src/main.rs`、`xbluenode-web/src/modules/registry.ts`（如需新增挂载，请在汇报中列出挂载语句，由总控在集成时合入）
  - 既有已合入的 `deploy/migrations/001_init_core.sql` 等旧迁移文件
  - 如发现契约字段缺失，立即停止并明确回报需要总控补充的契约定义，禁止私自修改 `axpoint-domain/src/ports.rs`

【编码硬性约束】
- Rust：遵守 `forbid(unsafe_code)`、`clippy::pedantic = "deny"`；禁止 `unwrap`/`expect`/`panic`/`as` 强转/直接下标 `arr[i]`；所有公开符号与 `# Errors` 必须写文档注释。
- Python：全量 Python 3.11+ 类型注解 + Pydantic v2；SQL 标识符必须白名单正则校验，参数化查询，凭证严禁明文日志/返回。
- Web：TypeScript 严格模式禁止 `any`；维持黑白灰 + `#2563eb` 极简风格并兼容明暗主题；契约先行场景可先使用模块内 Mock 开关独立联调。
- 命名：100% 自主命名体系，零外部品牌污染。

【必跑自验门禁（提交前贴出实测输出）】
1. 跑通 `{BLUEPRINT_FILE}` 中 **{STEP_RANGE}** 规定的全部 Verification Gate。
2. 跑通对应栈基线检查：
   - Rust：`cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`
   - Python：`pytest -v axpoint-parser/tests`
   - Web：`cd xbluenode-web && npm ci && npm run build`
3. 检查 `git status`，确保无 `target/`、`node_modules/`、`dist/`、`__pycache__/`、`.env` 等无用缓存。
```

---

### 5.3 PR 验收 / QA Review Agent：Stage 3 合入前审查 Prompt

```markdown
请以严格的代码审查官身份审核 PR 分支 `{BRANCH_NAME}`（合入目标 `main`），逐条核验：
1. 【白名单越界检查】：执行 `git diff --name-only main...{BRANCH_NAME}`，核实所有修改均落在 `{ALLOWED_PATHS}` 内，未擅自改动公共入口、旧迁移脚本或他人模块。
2. 【自主命名检查】：执行 `.github/workflows/ci.yml` 中的 `naming-guard` 正则扫描，确保 0 泄漏。
3. 【质量门禁复核】：核对对应模块已完整通过 Rust（`fmt` + `clippy -D warnings` + `test`）、Python（`pytest`）、Web（`npm run build`）及对应蓝图 Step 的 Verification Gate。
4. 【安全与 PII 脱敏复核】：涉及 SQL 连接器的必须参数化+白名单；涉及凭证的必须加密脱敏；涉及查询结果、LLM Prompt、全格式导出的必须经过统一 PII 脱敏策略。
5. 【契约一致性】：请求/响应字段、状态码、错误码与 `docs/API_CONTRACT.md` 完全一致。

请按以下格式输出审查结果：
- 结论：`PASS`（可合入并由总控执行公共路由/菜单挂载）或 `BLOCK`（阻断合入）
- 逐项核查证据（附文件与行号）
- 若为 `BLOCK`，给出最小化精确修改建议清单。
```
