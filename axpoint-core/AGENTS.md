# AxPoint Core（Rust Workspace）编码记忆与强制约束

本规则在操作 `axpoint-core/` 及其任意成员 Crate 时无条件生效，与根目录 [../AGENTS.md](../AGENTS.md) 叠加执行。

---

## 1. 工具链与 Crate 依赖倒置边界

1. **工具链锁死**：固定使用 `rustc 1.100.0`（`edition = "2024"`、`rust-version = "1.100.0"`）。禁止修改 `rust-toolchain.toml`、`clippy.toml` 或在任何成员 `Cargo.toml` 中覆写 `lints.workspace = true`。
2. **8 个固定成员 Crate 与单向依赖层**：
   - `axpoint-domain`：唯一公共契约层（纯 DTO、枚举、`AppError`、表达式/RRF/推演纯算法、端口 `trait`），**严禁引入 `sqlx`、`axum`、`reqwest`、`duckdb` 等任何 IO / 网络 / 存储依赖**。
   - 能力实现层：`axpoint-db`、`axpoint-llm`、`axpoint-security`、`axpoint-analytics`、`axpoint-flow`、`axpoint-agent`（`axpoint-agent` 等可按既有边界依赖 `axpoint-security` 的 `AuthPrincipal` / `AuditWriter`，能力层之间严禁跨 Crate 直接耦合内部存储或路由实现）。
   - 装配入口层：`axpoint-server`（唯一进行具体类型实例化、`AppState` 组装与 `.merge(...)` 路由挂载的 Crate；业务逻辑按模块拆入 `src/routes/<module>.rs`）。
3. **异步端口范式**：
   - 常规仓储与服务端口在 `axpoint-domain/src/ports.rs` 中声明返回 `impl Future<Output = Result<T, AppError>> + Send`，消费方 Crate 使用**泛型类型参数 `R`** 静态分发。
   - 仅当需要异构动态集合（如算子注册表、`AgentModelPort`、`ProviderSecretResolver`）时，定义返回 `BoxFuture<'a, Result<T, AppError>>` 的对象安全 Trait 并以 `Arc<dyn ...>` 持有。

---

## 2. Rust 零绕过代码质量红线（`scripts/lint-guard.py` 强制核验）

1. **严禁任何豁免属性与不安全块**：
   - 禁止出现 `unsafe`、`#[allow(...)]`、`#![allow(...)]`、`#[expect(...)]`、`#![expect(...)]`、`#[warn(...)]`、`#[doc(hidden)]`。
   - 每个库 Crate 的 `src/lib.rs` 顶部必须包含 `#![doc(test(attr(deny(warnings), forbid(unsafe_code))))]`。
2. **严禁崩溃、下标切片与隐式强转**：
   - 禁止 `.unwrap()`、`.expect(...)`、`panic!`、`todo!`、`unimplemented!`、`unreachable!`。
   - 禁止 `slice[idx]` 下标索引访问，一律使用 `.get(idx)`、`.first()`、`.split_first()` 或迭代器。
   - 禁止 `as` 数值或指针转换（如 `x as u64`、`y as f64`），一律使用 `u64::try_from(x)`、`f64::from(y)` 并显式处理溢出。
3. **严禁吞错、标准输出与无界死循环**：
   - 禁止 `let _ = fallible_call()` 或 `.ok()` 丢弃 `#[must_use]` 结果；错误必须向上 `?` 传播或通过 `tracing::warn!` 记录上下文。
   - 禁止 `println!`、`eprintln!`、`dbg!`、`std::process::exit`，统一使用结构化 `tracing` 宏。
   - 后台常驻任务严禁裸 `loop {}`，必须监听 `tokio::sync::watch::Receiver<bool>` 停机信号优雅退出；避免跨 `.await` 持有 `MutexGuard`（`significant_drop_in_scrutinee`）。
4. **全量 Rustdoc 与 `# Errors` 规范**：
   - 所有 `pub` 结构体、字段、枚举变体、函数、Trait 方法必须编写 `///` 文档注释；注释中出现的类型名、路径、SQL 关键字与代码片段必须用反引号包裹（防止 `rustdoc::unescaped_backticks` 报错）。
   - 所有返回 `Result<..., AppError>` 的 `pub` 函数必须包含 `/// # Errors` 段落说明触发错误的具体条件。

---

## 3. 数据库迁移、RLS 与防幻觉硬约束

1. **SQLx 幂等迁移与租户隔离**：
   - 新迁移仅由 `orchestrator` 在 `deploy/migrations/NNNN_<module>.sql` 新增，已合入迁移严禁改动。
   - 新表末尾必须调用 `SELECT ax_enable_tenant_rls('<table>');`；`PgStore` 业务读写统一经 `begin_tenant_tx(tenant_id)` 设置 `SET LOCAL app.tenant_id`。
   - 跨租户未命中一律映射为 `AppError::NotFound`，严禁泄露其他租户主键存在性。
2. **大模型与问数接地防线**：
   - 外部模型调用必须经 `LlmRouter` 执行 `redact_messages_for_egress` 占位符脱敏与响应还原。
   - NL2SQL 必须经 `sql_guard::guard` AST 白名单与 `EXPLAIN` 预检，结果摘要必须经 `verify_or_fallback_summary` 单元格数值核验，结构化工具/Agent 输出必须经 `validate_instance_against_schema` 校验。

---

## 4. 本目录提交前必跑命令

```bash
python3 ../scripts/lint-guard.py
python3 ../scripts/hallucination-guard.py
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
RUSTDOCFLAGS='-D warnings' cargo doc --workspace --all-features --no-deps --locked
cargo test --workspace --locked -- --test-threads=1
cargo test --workspace --doc --all-features --locked
```
