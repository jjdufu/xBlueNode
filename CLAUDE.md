# xBlueNode / AxPoint 项目级工程记忆（Claude Code Entry）

本文件与根目录 [AGENTS.md](./AGENTS.md)、[GEMINI.md](./GEMINI.md) 保持同源同步。进入子模块工作时，必须同时遵守 `axpoint-core/AGENTS.md`、`axpoint-parser/AGENTS.md`、`xbluenode-web/AGENTS.md` 中的栈专属约束；标准工作流技能见 `.agents/skills/`。

---

## 1. 必读权威索引

- **最高铁律**：[IRON_RULES.md](./IRON_RULES.md)
- **门禁与验证总则**：[docs/QUALITY_GATES_AND_VERIFICATION.md](./docs/QUALITY_GATES_AND_VERIFICATION.md)
- **多团队 / 多分支 Worktree / 多 Subagent 协同规范**：[docs/MULTI_AGENT_TEAM_WORKFLOW.md](./docs/MULTI_AGENT_TEAM_WORKFLOW.md)
- **接口与契约基准**：[docs/API_CONTRACT.md](./docs/API_CONTRACT.md) 及 `docs/standards/A1～A7`
- **实施方案目录**：[plans/README.md](./plans/README.md)（第一阶段 Step 00～14 归档于 `plans/completed/v1-foundation-step00-14/`；第二阶段 Step 15～22 归档于 `plans/completed/v2-ux-and-grounded-ai-step15-22/`；第三阶段 Step 23～30 交互重塑与可执行世界运行时归档于 `plans/completed/v3-craft-and-world-runtime-step23-30/`）。

---

## 2. 编码、安全与防幻觉硬约束

1. **100% 自主命名**：全仓所有标识符、注释、文档与方案仅允许使用 `xBlueNode / AxPoint` 命名体系，严禁出现任何外部竞品名称或衍生缩写（`bash scripts/naming-guard.sh`）。
2. **架构解耦与独占写入锁**：
   - `axpoint-domain` 纯净零 IO 依赖，能力 Crate 之间禁止直接互引实现，仅面向 `axpoint-domain::ports` 泛型编程，仅在 `axpoint-server` 注入装配。
   - 遵循「契约 PR → 多 Worktree 并行实现 PR → 独立 Verifier 子代理审查」流程；非 `orchestrator` 禁止修改公共入口、全局路由表与 `deploy/migrations/`（`bash scripts/check-write-scope.sh`）。
3. **全栈零豁免静态守卫**：
   - Rust `1.100.0`：零 `unsafe`、零 `#[allow]`/`#[expect]`、零 `unwrap`/`expect`/`panic`/`todo`、零 `as` 数值强转、零 `[]` 下标切片、零 `let _ =` 吞错、公开符号全量文档与 `# Errors`（`python3 scripts/lint-guard.py`）。
   - Python `3.11+`：全量 `from __future__ import annotations`、Pydantic v2、`mypy --strict`、SQL 参数化与白名单标识符校验（`python3 scripts/python-guard.py`）。
   - Web React 19 + TS Strict：零 `any`、零 `@ts-ignore`、禁用原生 `<select>`（统一用 `primitives.tsx`）、全量双语 `tx()` 与极简动词微文案（`python3 scripts/web-guard.py`）。
4. **安全与三道 PII 脱敏**：所有租户表末尾调用 `SELECT ax_enable_tenant_rls('<table>');`；查询返回、LLM 出域提示词、导出分享三处强制脱敏。
5. **本体级无幻觉 AI**：严禁 LLM 心算统计或推演数值；优先路由 `semantic.rs` 确定性指标编译并校验 `MappingLinkSpec` 连接路径；结构化输出启用 JSON Schema 受限解码与 `temperature = 0.0`；摘要输出经 `verify_or_fallback_summary` 单元格核验，知识引用经 `[文N]` 边界核验，零召回强制拒答澄清；AI 写操作仅写草稿并留存回退快照（`python3 scripts/hallucination-guard.py`）。

---

## 3. 提交前最小必跑检查

```bash
python3 scripts/lint-guard.py && python3 scripts/web-guard.py && python3 scripts/python-guard.py
python3 scripts/hallucination-guard.py && python3 scripts/docs-and-plans-guard.py
python3 scripts/check_routes.py && python3 scripts/check_traceability.py
bash scripts/check-write-scope.sh --self-test
```

触及具体代码栈时，必须追加运行 [AGENTS.md](./AGENTS.md) §5 中对应的 `cargo clippy/doc/test`、`ruff/mypy/pytest` 或 `npm run build` 全套检查，通过 PR + GitHub CI 合入受保护的 `main` 分支。
