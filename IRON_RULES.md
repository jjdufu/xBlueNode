# xBlueNode / AxPoint 工程开发铁律（MANDATORY）

> **本文档与根目录 `AGENTS.md`（及同源 `GEMINI.md`、`CLAUDE.md`）构成项目最高级别工程约束。所有开发者、分队主代理与子代理在新建模块、重构代码、提交 PR 前必须先阅读并无条件遵守。**

---

## 一、自主命名与产品边界铁律

1. **100% 自主命名体系**：本项目所有文档、实施方案（`plans/`）、记忆文件、代码注释、Crate/包名、数据库表名、API 路由、前端页面与 UI 文案均必须使用 **xBlueNode / AxPoint** 自主命名体系：
   - **AxHub / AxParser**：全源数据接入、连接器与双轨解析引擎
   - **AxGraph / AxAnalytics**：超图 + 业务节点时序双擎底座与 OLAP 制表引擎
   - **AxQuery**：五路融合检索与本体受控智能问数
   - **xBlueStudio / xBlueGraph**：多子图隔离建模、本体对齐融合、数据血缘、领域对象接口与拓扑浏览器
   - **AxFlow**：DAG 编排、持久化执行、规则与告警
   - **AxAgent / AxBuilder**：API / Skill / 运行态与构建态 MCP 发布、多模型路由、IM 连接与智能体
   - **AxSim**：范围预检与写时复制（CoW）因果策略推演沙盘
   - **xBlueView**：业务孪生驾驶舱、3D 孪生场景、页面动作回写与全格式出品中心
   - **AxGuard**：企业级权限、动态脱敏与审计底座
   - **AxOps**：部署、备份恢复、保留策略、行业模板与交付追踪
2. **严禁外部品牌污染**：严禁在仓库任何位置（含 `docs/`、`plans/`、`AGENTS.md`、测试样本）出现外部竞品名称、相关公司名称及其衍生命名。禁用词正则只保存在仓库变量 `vars.NAMING_DENYLIST` 与本地环境变量 `NAMING_DENYLIST` 中，由 `scripts/naming-guard.sh` 在 CI 与本地执行，正则本身不得写入仓库。

---

## 二、Monorepo 分模块解耦与契约先行铁律

### 1. 物理边界与依赖倒置
- **Rust Workspace（`axpoint-core/crates/`）**：
  - `axpoint-domain` 是唯一核心公共契约层，仅允许放置纯领域模型（`struct`/`enum`）、错误类型（`AppError`）、纯算法（表达式求值、RRF、因果推演、接地核验）与抽象接口（`trait`），**严禁依赖数据库驱动、HTTP 框架或特定外部 SDK**。
  - 各能力 Crate（`axpoint-db`、`axpoint-llm`、`axpoint-analytics`、`axpoint-flow`、`axpoint-agent`、`axpoint-security`）**严禁直接互相调用内部实现**，统一面向 `axpoint-domain` 定义的 `trait` 编程。端口 Trait 方法返回 `impl Future + Send`，能力 Crate 以**泛型参数**消费端口，`axpoint-server` 装配具体类型；算子注册表、连接器注册表、模型网关等需要运行时多态的位置，定义返回 `Pin<Box<dyn Future>>` 的对象安全 trait 并以 `Arc<dyn …>` 持有（约定见 [A1](./docs/standards/A1-CONVENTIONS-ENUMS-ROUTES.md) §8）。
  - 仅在 `axpoint-server` 进行依赖注入与路由装配。
- **路由与入口防冲突**：
  - `axpoint-server/src/main.rs` 严禁堆砌业务逻辑或全量 Handler，必须按模块拆分为 `src/routes/<module>.rs`，`main.rs` 仅通过 `.merge(...)` 挂载子路由。
- **Python Sidecar（`axpoint-parser/`）**：
  - 按职责严格分包为 `parsers/`（文件解析）、`connectors/`（数据源连接）、`lake/`（数据湖写入）、`exporters/`（全格式出品）、`charts/`（服务端图表）、`pii/`（敏感识别），通过 `schemas.py` 约定的 Pydantic 模型和注册表对接，禁止把新解析/连接逻辑堆进 `main.py` 或单文件。
- **前端工程（`xbluenode-web/src/`）**：
  - API 客户端必须按模块拆分为 `src/lib/api/<module>.ts`，严禁把全平台接口耦合进单一 `api.ts`。
  - 页面与专属组件按子域目录隔离（`pages/` + `components/<module>/`），导航与页面标题唯一事实来源为 `src/modules/meta.ts`，`App.tsx` 仅做模块注册表加载。

### 2. 数据库迁移编号
- 迁移使用 sqlx 格式，文件名 `deploy/migrations/NNNN_<module>.sql`（四位递增编号），清单与归属 Step 见 [A2-DATABASE-MIGRATIONS-INDEX.md](./docs/standards/A2-DATABASE-MIGRATIONS-INDEX.md)。
- 已合入的迁移文件只追加、不修改；结构调整一律写新迁移。新表用 `CREATE TABLE IF NOT EXISTS`，已有表用 `ALTER TABLE ... ADD COLUMN IF NOT EXISTS`，索引用 `CREATE INDEX IF NOT EXISTS`。
- 每个带 `tenant_id` 的新表在迁移末尾执行 `SELECT ax_enable_tenant_rls('<table>');`。
- 同一依赖层内并行实施的 Step，迁移随各自契约 PR 按编号从小到大依次合入。

### 3. 契约先行（Contract-First）与文档动静分离
- **文档与方案隔离**：`docs/` 仅存放跨阶段常驻门禁、多团队/多代理协作宪章、`API_CONTRACT.md` 与 `docs/standards/` 基准表；具体阶段的分步实施方案统一存放在 `plans/`（已完成方案归档至 `plans/completed/`，当前推进方案存放于对应阶段子目录）。
- **两段式合流**：
  1. **第一步（契约 PR，仅总控可写公共入口）**：先定义并合入 `deploy/migrations/NNNN_*.sql`、`axpoint-domain` 公共类型与 Trait、`docs/API_CONTRACT.md`、`docs/standards/{A1,A2,A4,A6,A7}` 增量条目及前端 `src/lib/api/<module>.ts` 类型定义。
  2. **第二步（并行实现 PR，按分队/子代理授权路径隔离）**：契约合入 `main` 后，Rust Crate、Python Sidecar 与 React 前端在独立 `git worktree` 中并行开发与单测，提交前执行 `bash scripts/check-write-scope.sh` 验证零越界修改。

---

## 三、代码质量零容忍红线

### 1. Rust 强制红线（由 `python3 scripts/lint-guard.py` 强制拦截）
- **工具链与配置物理锁死**：`axpoint-core/rust-toolchain.toml` 锁定 `1.100.0`（`edition = "2024"`、`rust-version = "1.100.0"`），`axpoint-core/clippy.toml` 锁定复杂度与参数上限，所有成员 Crate 必须继承 `lints.workspace = true`，每个库 Crate 的 `src/lib.rs` 顶部必须声明 `#![doc(test(attr(deny(warnings), forbid(unsafe_code))))]`。
- **零绕过铁律**：全仓严禁任何 `#[allow(...)]`、`#![allow(...)]`、`#[expect(...)]`、`#![expect(...)]`、`#[warn(...)]`、`#[doc(hidden)]`，严禁任何 `unsafe` 关键字，严禁在 `Cargo.toml` / `.cargo/config.toml` / `clippy.toml` 中放宽阈值或写入 `"allow"` / `"warn"`。
- **禁止崩溃、越界与未完成占位**：`unsafe_code = "forbid"`，`clippy::all = "deny"`、`clippy::pedantic = "deny"`，`unwrap_used`、`expect_used`、`get_unwrap`、`panic`、`unreachable`、`todo`、`unimplemented`、`indexing_slicing` 全部 `deny`（集合访问统一使用 `.get(...)` 或迭代器）。
- **禁止隐式转换、静默吞错与调试残留**：`as_conversions`、`as_underscore`、`trivial_casts`、`lossy_float_literal` 全部 `deny`（统一使用 `TryFrom` / `From`）；`let_underscore_must_use`、`unused_result_ok`、`mem_forget` 全部 `deny`（`Result` 错误必须传播或经 `tracing::warn!` 记录，禁止 `let _ = ...` 静默丢弃）；`dbg_macro`、`print_stdout`、`print_stderr`、`exit` 全部 `deny`。
- **并发与后台任务健壮性**：`rc_buffer`、`rc_mutex`、`infinite_loop`、`significant_drop_in_scrutinee`、`non_send_fields_in_send_ty` 全部 `deny`（后台常驻 worker 必须接入 `watch::Receiver<bool>` 优雅停机信号）。
- **文档与文档测试全量覆盖**：`missing_docs`、`missing_errors_doc`、`missing_panics_doc`、`rustdoc::all` 全部 `deny`。

### 2. Python 强制红线（由 `python3 scripts/python-guard.py` + `mypy --strict` 强制拦截）
- 全量使用 `from __future__ import annotations` 与 Python `3.11+` 类型注解 + Pydantic 2.x 数据契约，禁止无错误码的裸 `# type: ignore`。
- SQL 连接器与解析器严禁字符串拼接 SQL，表名/字段名必须经白名单正则校验；严禁明文存储或日志打印连接密码/Token。

### 3. TypeScript / 前端与产品微文案红线（由 `python3 scripts/web-guard.py` 强制拦截）
- 启用严格模式，严禁 `: any`、`as any`、`@ts-ignore`、`@ts-expect-error`、`eslint-disable` 绕过检查；严禁业务组件直接使用原生 `<select>`（统一使用 `src/components/ui/primitives.tsx`）。
- 维持黑白灰 + 科技蓝（`#2563eb`）极简风格，确保亮色/暗色双主题与中英双语 `tx()` 完整对齐。
- 控件标签仅用精炼动词或名词（2～4 汉字 / 1～2 英文词），严禁交互赘词、回调链描述、冗长引导占位符与教学式说明横幅。

### 4. 数据安全与脱敏红线
- 凡涉及 PII（手机号、身份证、邮箱、银行卡、姓名、地址、密钥）的数据，在**查询返回、送入 LLM Prompt、全格式导出/分享**三处必须强制经过统一脱敏策略拦截；默认对所有人脱敏，查看原值需要 `PII_RAW_READ` 权限并显式请求，且写入不可篡改哈希审计链。

---

## 四、本体级无幻觉 AI 工程铁律（由 `python3 scripts/hallucination-guard.py` 强制拦截）

无论接入云端大模型 API 还是本地私有化推理服务（Ollama / vLLM 7B～32B 模型），所有 AI 能力开发必须遵守以下五条确定性铁律：

1. **严禁心算业务数值**：严禁让语言模型在上下文内自行计算求和、均值、占比、同环比、排名或因果影响；一切数值计算必须下推至 DuckDB OLAP、`PgStore` 图计算或 `axpoint-domain::sim` 确定性推演内核。
2. **本体与语义层优先路由**：智能问数与智能体工具调用在命中已定义认证指标（`MetricDef`）或已发布业务节点类型（`NodeTypeSpec`）时，优先提取结构化参数交由确定性语义编译器执行；多表关联 SQL 必须经过 `sql_guard` AST 白名单与 `MappingLinkSpec` 本体边路径校验。
3. **结构化受限解码（Constrained Decoding）**：所有要求模型返回 JSON 的场景（NL2SQL、槽位提取、建模草案、看板草案、Agent `output_schema`）必须绑定 JSON Schema 校验器；针对支持受限解码的提供方（尤其本地中小参数模型），透传严格 `response_format: json_schema` 并将数值/结构提取任务的 `temperature` 锁死为 `0.0`。
4. **后置接地核验与零依据拒答**：数据摘要输出必须经过 `verify_or_fallback_summary` 单元格数值逐一核对，任何未出现在结果集中的数字立即触发拦截并回退确定性模板；文档问答必须核验引用角标 `[文N]` 不越界，检索召回为空或语义歧义时必须走确定性澄清/拒答分支，严禁模型自由编造。
5. **AI 写操作草稿隔离与快照回退**：AI 编码助手、建模侧栏助手或构建态 MCP 对模型、流程、看板的任何修改一律写入 `DRAFT / PROPOSED` 态并自动生成可回退版本快照，严禁 AI 未经人工发布或审批直接覆写 `PROD` 生产态资产。

---

## 五、Git 多分支协作与可验证门禁铁律

1. **禁止提交缓存与临时产物**：`.gitignore` 永久屏蔽 `target/`、`node_modules/`、`dist/`、`.venv/`、`__pycache__/`、`*.pyc`、`*.egg-info/`、`*.tsbuildinfo`、`.env`、`.env.test`、`.gate/` 及本地数据湖/备份临时文件。
2. **多团队 Worktree 与受保护主干**：
   - `main` 为受保护分支，禁止直接 `push`，所有变更经 `feat/<module>-<topic>` 或 `fix/<module>-<topic>` PR + GitHub CI 全绿合入，合入后立即清理分支与 worktree。
   - 多代理/多团队并发协议与写入范围锁表见 [docs/MULTI_AGENT_TEAM_WORKFLOW.md](./docs/MULTI_AGENT_TEAM_WORKFLOW.md)。
3. **全栈必过门禁（详见 [docs/QUALITY_GATES_AND_VERIFICATION.md](./docs/QUALITY_GATES_AND_VERIFICATION.md)）**：
   - 静态守卫：`bash scripts/naming-guard.sh`、`python3 scripts/lint-guard.py`、`python3 scripts/web-guard.py`、`python3 scripts/python-guard.py`、`python3 scripts/hallucination-guard.py`、`python3 scripts/docs-and-plans-guard.py`、`python3 scripts/check_routes.py`、`python3 scripts/check_traceability.py`
   - Rust：`cd axpoint-core && cargo fmt --all -- --check && cargo clippy --workspace --all-targets --all-features --locked -- -D warnings && RUSTDOCFLAGS='-D warnings' cargo doc --workspace --all-features --no-deps --locked && cargo test --workspace --locked && cargo test --workspace --doc --all-features --locked`
   - Python：`cd axpoint-parser && ruff check . && mypy --strict . && pytest -v tests`
   - Web：`cd xbluenode-web && npm run build && npx playwright test e2e/smoke.spec.ts`
   - 分步与全量集成门禁：`bash scripts/gate/stepNN.sh`（末行输出 `GATE stepNN PASS`）与 `bash scripts/gate/all.sh`
