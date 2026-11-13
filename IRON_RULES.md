# xBlueNode / AxPoint 工程开发铁律（MANDATORY）

> **本文档为项目根目录最高级别工程约束，所有人类开发者与 AI 编码代理在新建模块、重构代码、提交 PR 前必须先阅读并严格遵守。**

---

## 一、自主命名与产品边界铁律

1. **100% 自主命名体系**：本项目所有文档、代码注释、Crate/包名、数据库表名、API 路由、前端页面与 UI 文案均必须使用 **xBlueNode / AxPoint** 自主命名体系：
   - **AxHub / AxParser**：全源数据接入、连接器与双轨解析引擎
   - **AxGraph / AxAnalytics**：超图 + 业务节点时序双擎底座与 OLAP 制表引擎
   - **AxQuery**：五路融合检索与智能问数
   - **xBlueStudio / xBlueGraph**：多子图隔离建模、本体对齐融合、领域对象接口与拓扑浏览器
   - **AxFlow**：DAG 编排、持久化执行、规则与告警
   - **AxAgent**：API / Skill / MCP 发布、多模型路由与智能体
   - **AxSim**：写时复制（CoW）因果策略推演沙盘
   - **xBlueView**：业务孪生驾驶舱、3D 孪生场景、页面动作回写与全格式出品中心
   - **AxGuard**：企业级权限、动态脱敏与审计底座
   - **AxOps**：部署、备份恢复、保留策略、模板与交付追踪
2. **严禁外部品牌污染**：严禁在仓库任何位置出现外部竞品名称、相关公司名称及其衍生命名。禁用词正则只保存在仓库变量 `vars.NAMING_DENYLIST` 与本地环境变量 `NAMING_DENYLIST` 中，由 `scripts/naming-guard.sh` 在 CI 与本地执行，正则本身不得写入仓库。

---

## 二、Monorepo 分模块解耦与并行开发铁律

### 1. 物理边界与依赖倒置
- **Rust Workspace（`axpoint-core/crates/`）**：
  - `axpoint-domain` 是唯一核心公共契约层，仅允许放置纯领域模型（`struct`/`enum`）、错误类型（`AppError`）、纯算法（表达式求值、RRF、因果推演）与抽象接口（`trait`），**严禁依赖数据库驱动、HTTP 框架或特定外部 SDK**。
  - 各能力 Crate（`axpoint-db`、`axpoint-llm`、`axpoint-analytics`、`axpoint-flow`、`axpoint-agent`、`axpoint-security`）**严禁直接互相调用内部实现**，统一面向 `axpoint-domain` 定义的 `trait` 编程。端口 Trait 方法返回 `impl Future + Send`，不能做成 trait object：能力 Crate 以**泛型参数**消费端口，`axpoint-server` 装配具体类型；只有算子注册表、连接器注册表等需要运行时多态的位置，才定义返回 `Pin<Box<dyn Future>>` 的对象安全 trait 并以 `Arc<dyn …>` 持有（约定见蓝图 A1 §8）。
  - 仅在 `axpoint-server` 进行依赖注入与路由装配。
- **路由与入口防冲突**：
  - `axpoint-server/src/main.rs` 严禁堆砌业务逻辑或全量 Handler，必须按模块拆分为 `src/routes/<module>.rs`，`main.rs` 仅通过 `.merge(...)` 挂载子路由。
- **Python Sidecar（`axpoint-parser/`）**：
  - 按职责严格分包为 `parsers/`（文件解析）、`connectors/`（数据源连接）、`exporters/`（全格式出品），通过 `schemas.py` 约定的 Pydantic 模型和注册表对接，禁止把新解析/连接逻辑堆进 `main.py` 或单文件。
- **前端工程（`xbluenode-web/src/`）**：
  - API 客户端必须按模块拆分为 `src/lib/api/<module>.ts`，严禁把全平台接口耦合进单一 `api.ts`。
  - 页面与专属组件按子域目录隔离（`pages/` + `components/<module>/`），`App.tsx` 仅做模块注册表加载。

### 2. 数据库迁移编号
- 迁移使用 sqlx 格式，文件名 `deploy/migrations/NNNN_<module>.sql`（四位递增编号），清单与归属 Step 见 `docs/blueprint/A2-DATABASE-MIGRATIONS-INDEX.md`。
- 已合入的迁移文件只追加、不修改；结构调整一律写新迁移。新表用 `CREATE TABLE IF NOT EXISTS`，已有表用 `ALTER TABLE ... ADD COLUMN IF NOT EXISTS`，索引用 `CREATE INDEX IF NOT EXISTS`。
- 每个带 `tenant_id` 的新表在迁移末尾执行 `SELECT ax_enable_tenant_rls('<table>');`。
- 同一依赖层内并行实施的 Step，迁移随各自契约 PR 按编号从小到大依次合入。

### 3. 契约先行（Contract-First）并行规范
任何跨模块/跨语言新功能开发必须遵循以下两段式合流：
1. **第一步（契约 PR）**：先定义并合入 `deploy/migrations/NNNN_*.sql`、`axpoint-domain` 公共类型与 Trait、`docs/API_CONTRACT.md` 及前端 `src/lib/api/<module>.ts` 类型定义。
2. **第二步（并行实现 PR）**：契约合入 `main` 后，Rust Crate、Python Sidecar 与 React 前端各自基于契约与 Mock/Stub 并行开发与单测，互不阻塞。

---

## 三、代码质量零容忍红线

### 1. Rust 强制红线（与 `axpoint-core/Cargo.toml` 同步）
- `unsafe_code = "forbid"`（全仓禁止任何 `unsafe`）
- `clippy::all = "deny"`、`clippy::pedantic = "deny"`
- `unwrap_used = "deny"`、`expect_used = "deny"`、`panic = "deny"`、`todo = "deny"`、`unimplemented = "deny"`
- `as_conversions = "deny"`（严禁使用 `as` 强转数值，统一使用 `TryFrom` / `From` / 安全方法）
- `indexing_slicing = "deny"`（严禁直接 `arr[i]` 下标切片，统一使用 `.get(...)` 或迭代器）
- `missing_docs = "deny"`、`missing_errors_doc = "deny"`、`missing_panics_doc = "deny"`

### 2. Python 强制红线
- 全量使用 `from __future__ import annotations` 与 Python `3.11+` 类型注解 + Pydantic 2.x 数据契约。
- SQL 连接器与解析器严禁字符串拼接 SQL，表名/字段名必须经白名单正则校验；严禁明文存储或打印连接密码/Token。

### 3. TypeScript / 前端强制红线
- 启用严格模式，严禁使用 `any` 或 `// @ts-ignore` 绕过类型检查。
- 维持黑白灰 + 科技蓝（`#2563eb`）极简风格，确保浅色/深色主题完整兼容。

### 4. 数据安全与脱敏红线
- 凡涉及 PII（手机号、身份证、邮箱、银行卡、密钥）的数据，在**查询返回、送入 LLM Prompt、全格式导出/分享**三处必须强制经过统一脱敏策略拦截（实现见蓝图 Step 04 §04.3 与 Step 13 §13.7～§13.8）；默认对所有人脱敏，查看原值需要 `PII_RAW_READ` 权限并显式请求，且写入审计。

---

## 四、Git 协作、分支与无用文件清理铁律

1. **禁止提交无用缓存与构建产物**：
   - `.gitignore` 必须永久屏蔽 `target/`、`node_modules/`、`dist/`、`.venv/`、`__pycache__/`、`*.pyc`、`*.egg-info/`、`*.tsbuildinfo`、`.env`、本地数据湖临时文件及测试输出产物。
2. **分支与 Worktree 模型**：
   - `main` 为受保护主干分支，禁止直接 `push`，所有变更必须经 PR + CI 全绿合入。
   - 特性分支命名：`feat/<module>-<topic>` 或 `fix/<module>-<topic>`，合入后即删除，小步提交。
   - 多模块并发开发时优先使用 `git worktree` 隔离工作目录，避免反复切换分支破坏 Rust 增量编译缓存。
3. **每步可验证门禁（Verification Gate）**：
   - 任何模块合入前，必须按 [docs/blueprint/00-MASTER-INDEX.md](./docs/blueprint/00-MASTER-INDEX.md) §8～§9 跑通该 Step 的 `scripts/gate/stepNN.sh`（最后一行 `GATE stepNN PASS`）、`scripts/gate/all.sh` 回归与以下全局检查：
     - `bash scripts/naming-guard.sh`
     - `cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets -- -D warnings`
     - `cargo test --workspace`
     - `cd axpoint-parser && ruff check . && mypy --strict . && pytest -v tests`
     - `cd xbluenode-web && npm run build`
