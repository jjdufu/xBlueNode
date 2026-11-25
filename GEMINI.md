# xBlueNode / AxPoint 项目级工程记忆（Gemini / Antigravity Entry）

本文件与根目录 [AGENTS.md](./AGENTS.md)、[CLAUDE.md](./CLAUDE.md) 保持同源同步，确保多端 AI 编码代理开箱执行一致的零容忍工程标准。模块专属细则位于 `axpoint-core/AGENTS.md`、`axpoint-parser/AGENTS.md`、`xbluenode-web/AGENTS.md`；复杂流程技能位于 `.agents/skills/`。

---

## 1. 核心架构与文档导航

- **工程铁律**：[IRON_RULES.md](./IRON_RULES.md)
- **质量门禁总则**：[docs/QUALITY_GATES_AND_VERIFICATION.md](./docs/QUALITY_GATES_AND_VERIFICATION.md)
- **多团队 / 多分支 Worktree / 多 Subagent 协同宪章**：[docs/MULTI_AGENT_TEAM_WORKFLOW.md](./docs/MULTI_AGENT_TEAM_WORKFLOW.md)
- **全量接口与基准规范**：[docs/API_CONTRACT.md](./docs/API_CONTRACT.md) 与 `docs/standards/A1～A7`
- **分阶段实施方案仓**：[plans/README.md](./plans/README.md)（`plans/completed/v1-foundation-step00-14/`、`plans/completed/v2-ux-and-grounded-ai-step15-22/`、`plans/completed/v3-craft-and-world-runtime-step23-30/` 与 `plans/completed/v4-modeling-console-and-industry-worlds-step31-35/` 均为已完成归档）。`docs/` 与 `plans/` 严格动静分离，禁止新建 `docs/blueprint/`。

---

## 2. 不可逾越的六条全局红线

1. **自主命名纯净度**：全仓仅允许使用 `xBlueNode / AxPoint` 自主命名词表（见 [A1](./docs/standards/A1-CONVENTIONS-ENUMS-ROUTES.md) §1），严禁任何外部竞品品牌、公司名及衍生缩写进入源码、注释、`plans/` 或记忆文件（`scripts/naming-guard.sh` 强制拦截）。
2. **契约先行与公共入口锁**：跨栈需求必须先由 `orchestrator` 合入契约 PR，再由各分队在独立 `git worktree` 并行开发。非 `orchestrator` 严禁触碰 `Cargo.toml`、`rust-toolchain.toml`、`clippy.toml`、`ports.rs`、`enums.rs`、`main.rs`、`routes/mod.rs`、`App.tsx`、`modules/{meta,registry}.ts`、`deploy/migrations/`（由 `scripts/check-write-scope.sh` 拦截）。
3. **代码零绕过守卫**：
   - Rust（`1.100.0`，Edition 2024）：严禁 `unsafe`、`#[allow]`、`#[expect]`、`unwrap_used`、`expect_used`、`as_conversions`、`indexing_slicing`、`let _ =`、`println!`（`scripts/lint-guard.py`）。
   - Python（`3.11+`）：强制 `from __future__ import annotations`、`mypy --strict`、参数化 SQL + 白名单正则、零裸 `# type: ignore`（`scripts/python-guard.py`）。
   - Web（React 19 + TS Strict）：严禁 `any`、`@ts-ignore`、原生 `<select>`，强制双语 `tx()` 与极简动词微文案（`scripts/web-guard.py`）。
4. **多租户 RLS 与三道 PII 脱敏**：业务表强制 `ax_enable_tenant_rls`；查询结果、大模型出域 Prompt（`redact_messages_for_egress`）、导出与分享强制掩码保护。
5. **本体级无幻觉 AI**：禁止模型心算业务数值；命中认证指标/领域对象优先走确定性语义编译；结构化输出绑定 JSON Schema 受限解码与 `temperature = 0.0`；结果摘要强制走 `verify_or_fallback_summary` 单元格数值核验，文档问答强制核验 `[文N]` 引用并在零依据时确定性拒答；AI 写操作一律落入带快照回退点的草稿态（`scripts/hallucination-guard.py`）。
6. **Git 与门禁纪律**：严禁直接 `push` 到 `main`；禁止提交任何缓存目录或 `.env`；提交 PR 前必须跑通 `docs/QUALITY_GATES_AND_VERIFICATION.md` 规定的 G1 静态守卫与对应栈 G2 编译测试。
