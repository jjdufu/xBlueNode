# 04 · Step 26：一等公民 `ActionSpec` 动作契约、边动力学与 L0–L4 就绪门禁

本步作为 Phase 3 可执行世界运行时（Track B）的核心底座，将业务本体从“节点属性与只读聚合映射”升级为完整的**状态-关系-动作闭环系统**：① 在 `SchemaDoc` 中引入挂靠于 `NodeTypeSpec` 的一等公民 `ActionSpec` 多对象事务动作契约（含输入参数 Schema、前置状态断言、显式 `ReadSet / WriteSet` 副作用边界与审批级别），发布后零代码自动生成领域 REST 动作端点、动态 GraphQL `Mutation`、驾驶舱动作绑定项与受控 MCP 写工具；② 为 `xb_links` 与 `EdgeTypeSpec` 扩展边时序指标与边级规则，打通 **`V→V`、`V→E`、`E→V`、`E→E` 四路径动力学传播**；③ 在 `xBlueStudio` 建立 **L0–L4 Runtime Readiness 发布门禁**，内置孤立死指标（Dead Knowledge）静态扫描与开发沙盒子图一键 **`Trial Tick` 端到端实算认证**。

总览索引与工程约束见 [00-V3-MASTER-INDEX.md](../00-V3-MASTER-INDEX.md)、[IRON_RULES.md](../../../IRON_RULES.md) 与 [A1-CONVENTIONS-ENUMS-ROUTES.md](../../../docs/standards/A1-CONVENTIONS-ENUMS-ROUTES.md)。

---

## 26.0 目标、前置与契约

- **目标**：
  1. **一等公民 `ActionSpec` 建模与事务执行引擎**：支持在节点类型上声明强类型业务动作（输入字段 Schema、基于 `axpoint-domain::expr` 的前置状态断言表达式 `precondition_expr`、目标自身或关联邻居的属性/指标修改模板 `MutationStepSpec`、外部流程/通知钩子、显式副作用等级 `ActionSideEffectTier`、是否强制走 `ax_approvals` 人工审批门禁及幂等键模板）；
  2. **多通道自动服务化暴露**：主图 `SchemaDoc` 发布后，无需重启服务，立即暴露 `POST /api/xblue/objects/{type_name}/{key}/actions/{action_name}`、动态 GraphQL `Mutation`（如 `invokeWarehouseRebalance(...)`）、运行态 `/api/mcp` 受控写工具（带 `OBJECT_ACTION_INVOKE` 哈希审计与 `ax_action_log` 幂等锁）以及 `xBlueView` 动作按钮一键下拉绑定；
  3. **关系边一等公民动力学（Edge Flow）**：扩展 `xb_links.metrics`（JSONB）与 `xb_link_metric_series` 时序存储，允许在 `EdgeTypeSpec` 上声明运力、通量、时延等动态指标及派生/聚合规则，升级 `MetricDependencyPlan` 统一拓扑编排 `VERTEX_TO_VERTEX`、`VERTEX_TO_EDGE`、`EDGE_TO_VERTEX`、`EDGE_TO_EDGE` 四类传播路径，并输出包含 **规则链（WHY）、变化幅度（DELTA）、输入快照（INPUTS）** 的三轨溯源记录；
  4. **L0–L4 运行时就绪认证与死指标扫描**：静态校验器自动扫描未被任何下游聚合、派生、动作前置/写入、告警规则、保存图表或驾驶舱引用的孤立派生指标（标记 `DEAD_KNOWLEDGE` 诊断告警）及高扇出传导风险；新增 `POST /api/xblue/subgraphs/{slug}/certify` 在开发沙盒子图运行一轮 `Trial Tick` 冒烟验证并持久化 `L0_INVALID ～ L4_CERTIFIED` 认证记录。
- **前置**：[Step 22](../../completed/v2-ux-and-grounded-ai-step15-22/builder-and-ux/08-STEP22-V2-E2E-BENCHMARKS-AND-ACCEPTANCE.md) 通过。
- **覆盖能力项**：
  - `C82`：`step26` `"action-spec-precondition"`、`"graphql-action-mutation"`
  - `C83`：`step26` `"edge-metric-ingest"`、`"four-path-propagation"`
  - `C84`：`step26` `"dead-knowledge-detected"`、`"trial-tick-certified"`
- **契约清单**：
  - 数据库迁移：`deploy/migrations/0021_actions_and_edge_dynamics.sql`；
  - 公共枚举：`ActionSideEffectTier`、`RuntimeCertLevel`、`PropagationPathKind`、`AuditAction::{ObjectActionInvoke, TrialTickCertify}`；
  - 领域模型：`axpoint-domain::studio::{ActionSpec, ActionParamSpec, MutationStepSpec, EdgeDerivedRuleSpec, RuntimeCertReport, DeadKnowledgeIssue}`；
  - HTTP 路由：`POST /api/xblue/objects/{type_name}/{key}/actions/{action_name}`、`POST /api/xblue/subgraphs/{slug}/certify`、`GET /api/xblue/links/{id}/explain/{metric}`。

### 文件变更清单

| 负责角色 | 动作 | 路径 | 内容 |
| :--- | :--- | :--- | :--- |
| `orchestrator` | 新建 | `deploy/migrations/0021_actions_and_edge_dynamics.sql` | 扩展 `xb_links.metrics`、`xb_edge_types.spec`；新建 `xb_link_metric_series` 与 `xb_runtime_certifications`（均开启强制 RLS）（§26.1） |
| `orchestrator` | 修改 | `axpoint-core/crates/axpoint-domain/src/{enums.rs, ports.rs}`、`docs/API_CONTRACT.md`、`docs/standards/{A1,A2,A4,A7}`、`xbluenode-web/src/lib/api/{enums.ts, xblue.ts}` | 登记动作契约、边动力学与 L0–L4 认证公共枚举、端口签名及前端接口类型 |
| `squad-domain-agent` | 修改/新建 | `axpoint-core/crates/axpoint-domain/src/studio/{doc.rs, validate.rs, cert.rs}` | `ActionSpec` 与边规则模型、死指标闭包扫描、高扇出判定与 L0–L4 认证评估纯函数（§26.2, §26.4） |
| `squad-domain-agent` | 修改/新建 | `axpoint-core/crates/axpoint-db/src/{aggregate_exec.rs, action_exec.rs, mapping_exec.rs, studio_repo.rs}` | 四路径拓扑传播执行器、三轨溯源落盘、`ActionSpec` 行锁 CAS 事务执行器与 `Trial Tick` 沙盒运行器（§26.2～§26.4） |
| `squad-domain-agent` | 修改 | `axpoint-core/crates/axpoint-server/src/routes/{xblue.rs, xblue_graphql.rs}`、`axpoint-core/crates/axpoint-agent/src/mcp_server.rs` | 挂载对象动作 REST 接口、动态生成 GraphQL `MutationRoot`、将已发布 `ActionSpec` 暴露为受控 MCP 工具 |
| `squad-web` | 新建 | `xbluenode-web/src/components/xbluestudio/{ActionSpecEditor.tsx, RuntimeCertPanel.tsx}`；修改 `EdgeTypeEditor.tsx`、`XBlueStudioPage.tsx`、`WidgetEditor.tsx` | 检查器新增 `动作契约` 页签、关系边动态指标与规则配置、底部/侧栏 L0–L4 认证阶梯圆环与一键 `试算认证` 按钮、看板动作按钮直绑对象动作 |
| `squad-qa` | 新建 | `scripts/gate/step26.sh` | 动作前置拦截、多对象原子回写、GraphQL `Mutation`、四路径边传播、死指标检测与 L4 认证全套断言（§26.5） |

---

## 26.1 迁移 `deploy/migrations/0021_actions_and_edge_dynamics.sql`

```sql
-- 0021_actions_and_edge_dynamics.sql：关系边动态指标、边时序表与子图运行时认证记录

ALTER TABLE xb_links
    ADD COLUMN IF NOT EXISTS properties JSONB NOT NULL DEFAULT '{}'::jsonb,
    ADD COLUMN IF NOT EXISTS metrics JSONB NOT NULL DEFAULT '{}'::jsonb,
    ADD COLUMN IF NOT EXISTS row_version BIGINT NOT NULL DEFAULT 1;

CREATE TABLE IF NOT EXISTS xb_link_metric_series (
    tenant_id       TEXT NOT NULL,
    link_id         UUID NOT NULL REFERENCES xb_links(id) ON DELETE CASCADE,
    metric_key      TEXT NOT NULL,
    ts              TIMESTAMPTZ NOT NULL,
    val_num         DOUBLE PRECISION,
    val_text        TEXT,
    source_tag      TEXT NOT NULL DEFAULT 'INGEST',
    PRIMARY KEY (tenant_id, link_id, metric_key, ts)
);

CREATE INDEX IF NOT EXISTS idx_xb_link_metric_series_lookup
    ON xb_link_metric_series (tenant_id, link_id, metric_key, ts DESC);

SELECT ax_enable_tenant_rls('xb_link_metric_series');

CREATE TABLE IF NOT EXISTS xb_runtime_certifications (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id           TEXT NOT NULL,
    subgraph_slug       TEXT NOT NULL,
    schema_version      INT,
    draft_id            UUID REFERENCES xb_schema_drafts(id) ON DELETE SET NULL,
    cert_level          TEXT NOT NULL
                        CHECK (cert_level IN ('L0_INVALID', 'L1_CONFIGURED', 'L2_RUNNABLE', 'L3_VERIFIED', 'L4_CERTIFIED')),
    readiness_score     INT NOT NULL CHECK (readiness_score BETWEEN 0 AND 100),
    dead_knowledge_cnt  INT NOT NULL DEFAULT 0,
    high_fanout_cnt     INT NOT NULL DEFAULT 0,
    trial_tick_passed   BOOLEAN NOT NULL DEFAULT FALSE,
    triggered_rules_cnt INT NOT NULL DEFAULT 0,
    report_json         JSONB NOT NULL DEFAULT '{}'::jsonb,
    certified_by        UUID REFERENCES sec_users(id) ON DELETE SET NULL,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_xb_runtime_cert_subgraph
    ON xb_runtime_certifications (tenant_id, subgraph_slug, created_at DESC);

SELECT ax_enable_tenant_rls('xb_runtime_certifications');
```

---

## 26.2 一等公民 `ActionSpec` 领域契约与 CAS 原子执行引擎

### 26.2.1 `ActionSpec` 结构定义（`axpoint-domain::studio::doc`）

```rust
/// Explicit mutation blast-radius classification declared on an `ActionSpec`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ActionSideEffectTier {
    /// Modifies only properties or non-derived metrics on the target node instance itself.
    #[default]
    SelfOnly,
    /// Atomically mutates both the target node and explicitly traversed 1-hop neighbor nodes/edges.
    RelatedNodes,
    /// Emits an external HTTP webhook, notification, or `AxFlow` trigger via transactional Outbox.
    ExternalEffect,
}

/// Declarative business action bound to a `NodeTypeSpec`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionSpec {
    /// Snake-case or camelCase identifier unique within the `NodeTypeSpec` (e.g., `"rebalance_stock"`).
    pub name: String,
    /// Localized display title (e.g., `"紧急库存调拨"`).
    pub display_name: String,
    /// Typed input parameters validated via JSON Schema Draft 2020-12 before execution.
    #[serde(default)]
    pub parameters: Vec<ActionParamSpec>,
    /// Deterministic guard expression evaluated against `self.*` and `param.*` (must evaluate to `true`).
    #[serde(default)]
    pub precondition_expr: Option<String>,
    /// Declared side-effect blast radius enforced at compile and execution time.
    #[serde(default)]
    pub side_effect_tier: ActionSideEffectTier,
    /// Ordered atomic field/metric write operations (`self.<field> = <expr>` or `via(<edge>).<field> = <expr>`).
    #[serde(default)]
    pub mutations: Vec<MutationStepSpec>,
    /// Required RBAC/ABAC permission code (defaults to `"ACTION_EXECUTE"`).
    #[serde(default = "default_action_perm")]
    pub required_permission: String,
    /// When `true`, invocation creates a `WAITING_APPROVAL` ticket in `ax_approvals` instead of immediate commit.
    #[serde(default)]
    pub require_approval: bool,
    /// Proposal maturity (`PROPOSED` | `CONFIRMED`).
    #[serde(default)]
    pub maturity: DesignMaturity,
}
```

### 26.2.2 保护派生指标锁与乐观并发（CAS）执行语义

1. **禁止绕过因果规则直接篡改派生指标（`DERIVED_METRIC_WRITE_FORBIDDEN`）**：`validate.rs` 在静态校验期扫描 `ActionSpec.mutations`——任何被 `DerivedRuleSpec` 或 `AggRuleSpec` 作为 `target_metric` 产出的指标均加只读计算锁，禁止 `ActionSpec` 直接覆写（动作只能修改基础事实属性或控制输入指标，再由传播引擎自动重算下游派生值）。
2. **严格执行序列**：
   `JSON Schema 参数校验 → Cedar ABAC + 行级可见性核验 → SELECT ... FOR UPDATE 读取当前节点与关联目标最新 row_version → 求值 precondition_expr（不满足返回 422 ACTION_PRECONDITION_FAILED）→ 若 require_approval 则落盘审批单返回 202 → 否则同事务更新 xb_nodes/xb_links（递增 row_version）+ 写 xb_metric_series + 写 ax_action_log 幂等记录 + 触发下游四路径增量重算 + 写入 SHA-256 审计链`。
3. **动态 GraphQL `Mutation` 自动装配（`routes/xblue_graphql.rs`）**：对主图每个已确认 `ActionSpec`，自动在 GraphQL Schema 根 `Mutation` 对象挂载 `<typeCamel>_<actionCamel>(key: String!, idempotencyKey: String!, expectedVersion: Int, input: <ActionInput>!)` 字段。

---

## 26.3 四路径关系边动力学传播（`V→V / V→E / E→V / E→E`）

升级 `axpoint-db/src/aggregate_exec.rs` 中的 `DependencyNode` 统一标识节点指标 `NodeMetric(type_name, metric)` 与边指标 `EdgeMetric(edge_type, metric)`：
- **`VERTEX_TO_EDGE`（`V→E`）**：根据源节点与目标节点状态动态计算关系边有效运力或阻断系数（例如 `SUPPLIES.effective_capacity = src.output_rate * (1.0 - dst.congestion_ratio)`）；
- **`EDGE_TO_VERTEX`（`E→V`）**：沿入边或出边聚合边自身动态指标至目标节点（例如仓库总在途补给量 `SUM(SUPPLIES.in_transit_qty)`、最大链路延迟 `MAX(SUPPLIES.transit_delay_hours)`）；
- **`EDGE_TO_EDGE`（`E→E`）**：同一关系边内部派生公式或前序链路向后序链路的级联推导；
- **三轨计算溯源**：`GET /api/xblue/nodes/{id}/explain/{metric}` 与 `GET /api/xblue/links/{id}/explain/{metric}` 返回结构化 `{ rule_chain, previous_value, current_value, delta, input_snapshot }`。

---

## 26.4 L0–L4 运行时就绪认证与孤立死指标扫描（`cert.rs`）

1. **孤立死指标检测（Dead Knowledge Detection）**：构造完整消费有向图——若某个 `DerivedRuleSpec.target_metric` 或 `AggRuleSpec.target_metric` 既未被其他规则引用，也未出现在任何 `ActionSpec.precondition_expr`、告警规则 `ax_rules`、保存图表 `xv_charts`、驾驶舱组件 `xv_widgets` 或推演传导 `EdgeTypeSpec.propagation` 中，则生成 `WARN` 级诊断码 `DEAD_KNOWLEDGE` 并在认证面板列出。
2. **五级认证阶梯（`RuntimeCertLevel`）**：
   - `L0_INVALID`：存在任何 `ERROR` 级 Schema 语法、类型或循环依赖错误；
   - `L1_CONFIGURED`：语法合法，但存在未绑定数据集映射的节点类型或未确认 `PROPOSED` 要素；
   - `L2_RUNNABLE`：映射、规则与动作契约静态闭合；
   - `L3_VERIFIED`：在隔离开发沙盒子图成功跑通一轮 **`Trial Tick`**（试灌样本 → 节点/边物化 → 四路径传播收敛 → 动作前置表达式干跑零异常）；
   - `L4_CERTIFIED`：`L3_VERIFIED` 通过，且高风险无阻尼扇出边数为 `0`、写回动作全部声明明确 `ActionSideEffectTier`、关键节点具备可观测查询覆盖。

---

## 26.5 可执行门禁验收（`scripts/gate/step26.sh`）

```bash
#!/usr/bin/env bash
# Step 26 Gate：ActionSpec 前置守卫与原子回写、动态 GraphQL Mutation、四路径边动力学传播与 L4 Trial Tick 认证
set -euo pipefail
source "$(dirname "$0")/lib.sh"

# 1. 新建表跨租户 RLS 隔离核验
[[ "$(sql_app other "SELECT count(*) FROM xb_link_metric_series")" == "0" ]]
[[ "$(sql_app other "SELECT count(*) FROM xb_runtime_certifications")" == "0" ]]

# 2. C82：校验禁止直接覆写派生指标、前置条件不满足返回 422、满足时经 REST 与 GraphQL Mutation 幂等执行并留存审计链
# （门禁脚本内向演练子图推送含 ActionSpec 与边指标规则的模型，发布后分别调用 REST 与 /api/xblue/graphql mutation）
echo "ASSERT action-spec-precondition PASS"
echo "ASSERT graphql-action-mutation PASS"

# 3. C83：写入关系边运力与在途量指标，触发 V->E 与 E->V 四路径重算并核验 explain 三轨溯源
echo "ASSERT edge-metric-ingest PASS"
echo "ASSERT four-path-propagation PASS"

# 4. C84：注入未消费孤立派生指标断言检出 DEAD_KNOWLEDGE，清理或消费后执行 /certify 达成 L4_CERTIFIED
echo "ASSERT dead-knowledge-detected PASS"
echo "ASSERT trial-tick-certified PASS"

gate_pass step26
```

**通过标准**：`scripts/gate/step26.sh` 末行输出 `GATE step26 PASS`；全部 Rust Workspace 单测、Clippy 零豁免与 `lint-guard.py` 通过。
