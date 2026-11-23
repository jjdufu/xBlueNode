# xBlueNode / AxPoint 分阶段实施方案仓（`plans/`）

本目录集中存放平台各演进阶段的**分步工程实施方案（Step Blueprints）**，与 `docs/` 目录保持严格的动静分离：
- **`docs/`（常驻宪章与活态标准）**：存放任何阶段均须无条件遵守的门禁总则（[QUALITY_GATES_AND_VERIFICATION.md](../docs/QUALITY_GATES_AND_VERIFICATION.md)）、多团队/多分支/多子代理协同宪章（[MULTI_AGENT_TEAM_WORKFLOW.md](../docs/MULTI_AGENT_TEAM_WORKFLOW.md)）、全量接口契约（[API_CONTRACT.md](../docs/API_CONTRACT.md)）及跨阶段基准表（`docs/standards/A1～A7`）。
- **`plans/completed/`（已验收归档阶段）**：阶段内全部 Step 通过 `scripts/gate/all.sh` 与 GitHub CI 合入 `main` 后，整目录移入 `completed/` 只读封存，保留完整演进脉络。
- **`plans/<active-phase>/`（当前推进阶段）**：按依赖拓扑层（Layer）逐 Step 给出目标、前置依赖、契约清单、幂等迁移 DDL、Rust/Python/Web 代码契约与可执行验收门禁脚本。

---

## 阶段目录总览

| 阶段标识 | 状态 | 目录入口 | Step 范围 | 数据库迁移范围 | 核心交付能力 |
| :--- | :--- | :--- | :--- | :--- | :--- |
| **Phase 1 (`v1`)** | 已完成归档 | [completed/v1-foundation-step00-14/00-MASTER-INDEX.md](./completed/v1-foundation-step00-14/00-MASTER-INDEX.md) | Step 00 ～ Step 14 | `0001` ～ `0014` | 全源双轨解析、DuckLake 数据湖、超图与时序节点底座、OLAP 与受控问数、多子图建模、DAG 编排与规则告警、发布/MCP/智能体、CoW 因果推演、3D 孪生驾驶舱、全格式出品、Cedar ABAC 安全治理、运维备份与行业模板 |
| **Phase 2 (`v2`)** | 已完成归档 | [completed/v2-ux-and-grounded-ai-step15-22/00-V2-MASTER-INDEX.md](./completed/v2-ux-and-grounded-ai-step15-22/00-V2-MASTER-INDEX.md) | Step 15 ～ Step 22 | `0015` ～ `0019` | **专项一（本体级无幻觉 AI）**：云端/本地小模型受限解码（`json_schema`）、本体优先问数编译、法定连接路径守卫、文档引用角标核验与零依据拒答、50 例对抗评测基准；<br>**专项二（构建协同与全链路易用性）**：`AxBuilder` 构建态 MCP 与开发子图闭环、`xBlueStudio` 焦点感知助手 + 单节点横向血缘画布 + 模型全景规格书、5 步引导式概览与演练子图模板试用、Studio 一键双态包装（AI + BI）、`AxSim` 克隆前范围预检、`AxQuery` 显式作用域选择、`xBlueView` 五区骨架与发布自检、`AxAgent` 企业 IM 长连接双向接入 |

---

## 方案编写与推进守则

1. **命名纯净度**：`plans/` 下所有文档受 `scripts/naming-guard.sh` 全量扫描保护，严禁出现任何外部竞品名称、相关公司名或外部产品缩写，统一使用 [A1-CONVENTIONS-ENUMS-ROUTES.md](../docs/standards/A1-CONVENTIONS-ENUMS-ROUTES.md) 定义的自主命名体系。
2. **契约先行**：实施任意 Step 时，必须按 [MULTI_AGENT_TEAM_WORKFLOW.md](../docs/MULTI_AGENT_TEAM_WORKFLOW.md) §3 先由 `orchestrator` 提交契约 PR（同步将本阶段规划的迁移、枚举、路由与追踪项合并入 `docs/API_CONTRACT.md` 及 `docs/standards/A1～A7`），再由各领域分队在独立 `git worktree` 中并行编码。
3. **死链与规范校验**：每次新增或调整方案文件后，必须运行 `python3 scripts/docs-and-plans-guard.py` 校验相对链接完整性与文档规范。
