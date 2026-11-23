# xBlueNode / AxPoint 根级项目记忆与编码铁律（Root Project Memory）

本文件为仓库根级强制编码记忆，适用于所有 AI 编码代理与协作工程师。子目录规则（`axpoint-core/AGENTS.md`、`axpoint-parser/AGENTS.md`、`xbluenode-web/AGENTS.md`）在进入对应代码树时叠加生效。

---

## 1. 仓库真源地图（Single Source of Truth）

- **最高工程铁律**：[IRON_RULES.md](./IRON_RULES.md)
- **三级门禁规范**：[docs/QUALITY_GATES_AND_VERIFICATION.md](./docs/QUALITY_GATES_AND_VERIFICATION.md)
- **多团队 / 多分支 Worktree / 多 Subagent 宪章**：[docs/MULTI_AGENT_TEAM_WORKFLOW.md](./docs/MULTI_AGENT_TEAM_WORKFLOW.md)
- **全量活态接口契约**：[docs/API_CONTRACT.md](./docs/API_CONTRACT.md) 与 [docs/standards/A4-API-CONTRACT-INDEX.md](./docs/standards/A4-API-CONTRACT-INDEX.md)
- **公共枚举 / 路由 / 错误码 / 迁移 / 追溯矩阵**：`docs/standards/{A1,A2,A5,A6,A7}-*.md`
- **分阶段实施方案仓**：[plans/README.md](./plans/README.md)
  - 已完成基线归档（Step 00～14）：`plans/completed/v1-foundation-step00-14/`（只读封存）
  - 已完成二期归档（Step 15～22）：`plans/completed/v2-ux-and-grounded-ai-step15-22/`（只读封存）
- **严禁重新创建遗留混合目录 `docs/blueprint/`**（`docs/` 只放跨阶段宪章与活标准，`plans/` 只放分阶段实施方案）。

---

## 2. 自主命名与安全合规红线（零例外）

1. **100% 自主命名体系**：代码、注释、表名、API、UI、文档、`plans/` 与提交信息只允许使用 `xBlueNode / AxPoint` 词表（`AxHub`、`AxParser`、`AxGraph`、`AxAnalytics`、`AxQuery`、`xBlueStudio`、`xBlueGraph`、`AxFlow`、`AxAgent`、`AxBuilder`、`AxSim`、`xBlueView`、`AxGuard`、`AxOps`）。严禁在仓库任意文件中出现外部竞品名称、相关公司名或外部产品衍生词（由 `scripts/naming-guard.sh` 全仓扫描阻断）。
2. **多租户 RLS 隔离**：所有 `/api/*` 业务接口严禁从查询参数或请求体读取 `tenant_id`，租户身份只从 `AuthPrincipal` 提取；每张含 `tenant_id` 的新表必须在迁移末尾执行 `SELECT ax_enable_tenant_rls('<table>');`；跨租户访问资源一律返回 `404`。
3. **三道强制 PII 脱敏**：涉及手机号、身份证、邮箱、银行卡、姓名、地址、密钥的数据，在**查询返回、送入大模型 Prompt（`redact_messages_for_egress`）、全格式导出与分享**三处必须默认掩码；仅在持有 `PII_RAW_READ` 权限且显式请求时还原并写入哈希审计链。

---

## 3. 本体级无幻觉 AI 五条铁律

无论对接云端大模型 API 还是本地推理服务（Ollama / vLLM 7B～32B 模型），编写或修改任何 AI 相关逻辑必须满足：

1. **禁止大模型心算**：聚合、占比、同环比、排名与因果推演一律由 DuckDB、`PgStore` 或 `axpoint-domain::sim` 确定性计算，禁止模型在文本中推算业务数字。
2. **本体与认证指标优先**：命中已定义 `MetricDef` 或已发布 `NodeTypeSpec` 的提问优先提取结构化槽位走 `semantic.rs` 确定性编译；多表 SQL 必须通过 `sql_guard` AST 校验与 `MappingLinkSpec` 外键连接路径约束。
3. **结构化受限解码**：凡要求模型输出 JSON（NL2SQL、槽位提取、建模草案、看板生成、Agent `output_schema`），必须做 Schema 校验；在支持受限解码的模型提供方上透传严格 `response_format: json_schema`，数值与结构提取任务锁定 `temperature = 0.0`。
4. **后置接地核验与零依据拒答**：表格摘要必须经 `verify_or_fallback_summary` 逐数字比对结果集单元格；文档问答必须核验 `[文N]` 引用角标未越界；零召回或语义歧义时必须返回确定性澄清或无依据说明。
5. **AI 变更草稿隔离**：AI 助手与构建态 MCP 的写操作只允许落入 `DRAFT / PROPOSED` 草稿并自动保存上一版本快照，禁止未经人工确认或审批直接覆写 `PROD` 资产。

---

## 4. 契约先行、多代理写入锁与分支规约

1. **两段式开发**：跨模块或跨语言需求先由 `orchestrator` 提交**契约 PR**（迁移 `deploy/migrations/NNNN_*.sql` + `axpoint-domain` 类型与端口 + `docs/API_CONTRACT.md` + `docs/standards/` + `xbluenode-web/src/lib/api/<module>.ts` + `scripts/gate/stepNN.sh`），合入 `main` 后各分队在独立 `git worktree` 中并行提交**实现 PR**。
2. **公共入口禁改令**：非 `orchestrator` 角色严禁修改 `Cargo.toml`、`rust-toolchain.toml`、`clippy.toml`、`axpoint-domain/src/{lib,ports,enums}.rs`、`axpoint-db/src/lib.rs`、`axpoint-server/src/{main,state,routes/mod}.rs`、`xbluenode-web/src/{App.tsx,modules/meta.ts,modules/registry.ts,lib/api/enums.ts}`、`deploy/migrations/` 及 `scripts/*guard*`。分队提交前必须运行 `bash scripts/check-write-scope.sh <role> origin/main...HEAD`。
3. **编写与审查分离**：分队主代理派发 `Author Subagent` 编码后，必须由独立上下文的 `Verifier Subagent` 执行门禁与契约核对，禁止自写自审。
4. **受保护主干**：严禁直接 `push` 到 `main`；使用 `feat/<module>-<topic>` 或 `fix/<module>-<topic>` 分支提 PR，GitHub Actions 全绿后合入并清理分支与 worktree。

---

## 5. 提交前必跑验证命令速查

```bash
# 1. 全栈静态守卫（每次提交必跑）
python3 scripts/lint-guard.py
python3 scripts/web-guard.py
python3 scripts/python-guard.py
python3 scripts/hallucination-guard.py
python3 scripts/docs-and-plans-guard.py
python3 scripts/check_routes.py --routes axpoint-core/crates/axpoint-server/src/routes --index docs/standards/A4-API-CONTRACT-INDEX.md
python3 scripts/check_traceability.py docs/standards/A7-REQUIREMENTS-TRACEABILITY.md scripts/gate
bash scripts/check-write-scope.sh --self-test

# 2. 按触及代码栈执行编译与测试
(cd axpoint-core && cargo fmt --all -- --check && cargo clippy --workspace --all-targets --all-features --locked -- -D warnings && RUSTDOCFLAGS='-D warnings' cargo doc --workspace --all-features --no-deps --locked && cargo test --workspace --locked -- --test-threads=1 && cargo test --workspace --doc --all-features --locked)
(cd axpoint-parser && ruff check . && mypy --strict . && pytest -v -m "not heavy" tests)
(cd xbluenode-web && npm run build)
```
