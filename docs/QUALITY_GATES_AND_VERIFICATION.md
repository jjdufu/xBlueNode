# xBlueNode / AxPoint 质量门禁与验证总则

本文档规定仓库在**本地提交前、PR 审查阶段、主干合入与阶段终验**三个层级的强制性验证门禁。任何代码、迁移、规范或实施方案变更均须通过对应门禁后方可合入 `main`。

---

## 1. 三级门禁矩阵

| 门禁层级 | 触发时机 | 必跑检查项 | 失败阻断策略 |
| :--- | :--- | :--- | :--- |
| **G1 · 静态防绕过与契约守卫** | 每次本地 Commit 前 / 每个 PR 必跑 | 全部 8 个静态守卫脚本（见 §2） | 任一脚本退出码非 `0` 立即阻断提交与 CI `naming-guard` 作业 |
| **G2 · 分栈编译、类型、文档与单测** | 触及对应目录的每个 PR 必跑 | Rust `1.100.0` 全套严格检查 / Python `ruff + mypy --strict + pytest` / Web `tsc + vite build + playwright smoke`（见 §3） | 任一警告或单测失败立即阻断对应 CI 作业 |
| **G3 · 端到端业务门禁与无幻觉评测** | 每个 Step 实现 PR 与阶段发布前 | `scripts/gate/stepNN.sh`、`scripts/gate/all.sh`、黄金评测集（NL2SQL / Agent / 接地防幻觉）、`full-journey` E2E（见 §4） | 门禁脚本末行未输出 `GATE stepNN PASS` 或 `ALL GATES PASS` 禁止合入 |

---

## 2. G1 静态守卫套件（零外部依赖，毫秒级执行）

所有静态守卫仅依赖 Bash 与 Python 3.11 标准库，可在开发机、远程测试机与 CI 容器内直接运行：

| 脚本路径 | 守护领域 | 核心拦截规则 |
| :--- | :--- | :--- |
| `scripts/naming-guard.sh` | 自主品牌纯净度 | 从环境变量 `NAMING_DENYLIST` 读取外部竞品禁用正则，全仓扫描源码、文档、`plans/` 与记忆文件；变量缺失返回退出码 `2`，命中禁用词返回退出码 `1`。 |
| `scripts/lint-guard.py` | Rust 零豁免红线 | 锁死 `rust-toolchain.toml` 为 `1.100.0`；校验全 Workspace `Cargo.toml` 与 `clippy.toml` 阈值；剥离注释后扫描全部 `.rs` 文件，拦截 `unsafe`、`#[allow]`、`#[expect]`、`#[warn]`、`#[doc(hidden)]`、裸 `println!/eprintln!/dbg!` 与缺少 `#![doc(test(...))]` 的库入口。 |
| `scripts/web-guard.py` | 前端严格类型与微文案 | 扫描 `xbluenode-web/src/`：拦截 `: any`、`as any`、`@ts-ignore`、`@ts-expect-error`、`eslint-disable`；拦截业务页面绕过 `primitives.tsx` 直接使用原生 `<select>`；拦截交互赘词、回调链按钮文案、冗长占位符等反模式。 |
| `scripts/python-guard.py` | Sidecar 类型与 SQL 安全 | 扫描 `axpoint-parser/`：强制业务模块声明 `from __future__ import annotations`；拦截无错误码的裸 `# type: ignore`；AST 级拦截 `execute(...)` 内使用 f-string / `%` / `.format()` 拼接动态 SQL；拦截日志直接输出密钥字段。 |
| `scripts/hallucination-guard.py` | 无幻觉 AI 五道防线 | 审计 `axpoint-analytics`、`axpoint-llm`、`axpoint-agent`：确认数值总结路径接入 `verify_or_fallback_summary`、SQL 生成路径接入 `sql_guard::guard` 与 `EXPLAIN` 校验、出域请求接入 `redact_messages_for_egress`、系统提示词保留不确定澄清拒答约束。 |
| `scripts/docs-and-plans-guard.py` | 文档链接、记忆预算与文风 | 校验 `README.md`、`IRON_RULES.md`、各级 `AGENTS.md` / `GEMINI.md` / `CLAUDE.md`、`.agents/`、`docs/`、`plans/` 内所有相对 Markdown 链接均指向真实存在的文件；校验每个记忆规则文件体积严格 `< 24,000` 字节；校验文档纯净度规范。 |
| `scripts/check_routes.py` | 路由契约双向一致性 | 解析 `axpoint-server/src/routes/*.rs` 全部 Axum 路由树，与 [A4-API-CONTRACT-INDEX.md](./standards/A4-API-CONTRACT-INDEX.md) 逐条双向比对，杜绝未登记暗路由或未实现空契约。 |
| `scripts/check_traceability.py` | 需求到门禁可追溯性 | 解析 [A7-REQUIREMENTS-TRACEABILITY.md](./standards/A7-REQUIREMENTS-TRACEABILITY.md) 全部能力项（`C01+`）与数据源项（`D01～D26`），核验引用的每个 `stepNN` 断言名真实存在于 `scripts/gate/stepNN.sh`。 |
| `scripts/check-write-scope.sh` | 多代理写入边界锁 | 核验当前分支相对 `origin/main` 的变更文件集合是否严格落在指定角色/分队的独占路径白名单内，防止越权改动公共入口文件。 |

一键执行 G1 静态守卫：

```bash
bash scripts/naming-guard.sh
python3 scripts/lint-guard.py
python3 scripts/web-guard.py
python3 scripts/python-guard.py
python3 scripts/hallucination-guard.py
python3 scripts/docs-and-plans-guard.py
python3 scripts/check_routes.py --routes axpoint-core/crates/axpoint-server/src/routes --index docs/standards/A4-API-CONTRACT-INDEX.md
python3 scripts/check_traceability.py docs/standards/A7-REQUIREMENTS-TRACEABILITY.md scripts/gate
bash scripts/check-write-scope.sh --self-test
```

---

## 3. G2 分栈构建与单元验证标准

### 3.1 Rust 核心引擎（`axpoint-core/`）

```bash
cd axpoint-core
rustc --version | grep -qF "rustc 1.100.0 "
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
RUSTDOCFLAGS='-D warnings' cargo doc --workspace --all-features --no-deps --locked
cargo test --workspace --locked -- --test-threads=1
cargo test --workspace --doc --all-features --locked
```

数据库集成测试约定：设置 `AXPOINT_REQUIRE_DB_TESTS=1` 时，所有 PostgreSQL / RLS 集成测试禁止跳过，连接不可达直接判失败。

### 3.2 Python 解析与连接器服务（`axpoint-parser/`）

```bash
cd axpoint-parser
ruff check .
mypy --strict .
pytest -v -m "not heavy" tests
```

涉及外部重型数据源容器的完整连接器回归执行 `pytest -v -m "heavy" tests`（配合 `deploy/docker-compose.test.yml`）。

### 3.3 React 前端工作台（`xbluenode-web/`）

```bash
cd xbluenode-web
npm ci
npm run build
npx playwright test e2e/smoke.spec.ts
```

---

## 4. G3 分步业务门禁与全链路回归

门禁公共断言函数库定义见 [A5-TEST-FIXTURES-AND-GATES.md](./standards/A5-TEST-FIXTURES-AND-GATES.md) §2（`scripts/gate/lib.sh`）。

### 4.1 门禁脚本编写规范
1. 每个新 Step 必须在契约 PR 中同步提交 `scripts/gate/stepNN.sh`，首行引入 `source "$(dirname "$0")/lib.sh"`，末行调用 `gate_pass stepNN`（标准输出最后一行固定为 `GATE stepNN PASS`）。
2. 脚本内严禁出现需人工替换的占位符；所有连接地址、测试令牌与前置种子 ID 自动从 `.env.test` 与 `.gate/ids.env` 读取。
3. 凡包含新增数据库迁移 `NNNN_*.sql` 的 Step，门禁脚本必须包含：
   - 无 `pending` 迁移断言；
   - 新增租户表的跨租户零可见断言（`sql_app other "SELECT count(*) FROM <table>"` 结果为 `0`）；
   - 新增写操作路由的审计落库断言（`sec_audit_logs` 存在对应 `action` 记录）。
4. 凡涉及 AI 生成、问数、知识检索或智能体工具调用的 Step，门禁脚本与对应集成测试必须使用确定性录制回放夹具（`AXPOINT_LLM_CASSETTE=tests/cassettes`），同时覆盖云端标准模型响应与本地小模型噪声/越界响应两类回放向量。

### 4.2 单步与全量执行命令

```bash
# 单步验收（服务已由 dev-up.sh + start-services.sh + seed-demo.sh 拉起）
bash scripts/gate/step14.sh

# 全量回归（自动重置环境、执行迁移、启动双服务、装载种子、跑通全部 step00..stepNN 与全旅程 Playwright E2E）
bash scripts/gate/all.sh
```
