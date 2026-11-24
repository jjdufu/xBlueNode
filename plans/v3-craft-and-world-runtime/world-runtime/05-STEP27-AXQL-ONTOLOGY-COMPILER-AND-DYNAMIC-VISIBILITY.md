# 05 · Step 27：免连接本体查询编译层（`AxQL`）与状态驱动动态行级视野

本步解决多粒度业务模型下的两大深层数据消费与治理难题：① 当同一业务实体同时拥有静态主档表（`STATIC_FACT`）、高频过程单据表（`EVENT_DYNAMIC`）与日/月切片汇总表（`CUBE_ROLLUP`）时，分析师与智能体若直接手写多表 `JOIN` 极易产生一对多扇出重复累加（Fanout Inflation）或连接键遗漏——本步在 `axpoint-analytics::axql` 构建零存储侵入的**声明式本体查询编译层 `AxQL`**，自动按领域对象身份重写跨层连接并内置 `SUM_BY_ENTITY` 去重聚合与超图混合检索桥接；② 升级 `AxGuard` 行级安全策略，支持基于业务节点实时指标与状态字段的**状态驱动动态行级视野（State-Driven Visibility）**，并确保 `AxSim` 推演沙盘克隆与模拟计算全量继承该动态可见性规则。

总览索引与工程约束见 [00-V3-MASTER-INDEX.md](../00-V3-MASTER-INDEX.md)、[IRON_RULES.md](../../../IRON_RULES.md) 与 [AGENTS.md](../../../AGENTS.md)。

---

## 27.0 目标、前置与契约

- **目标**：
  1. **`AxQL` 跨层免手动 `JOIN` 编译器（`axpoint-analytics::axql`）**：提供面向人与大模型极简低门槛的本体 SQL 方言编译能力——查询主实体对象（如 `SELECT d.name, d.daily_visits, d.monthly_budget_rate FROM OBJECT(Department) d WHERE d.biz_date = $day`）时，编译器读取已发布 `SchemaDoc` 自动关联主档、事件流与切片立方底层数据集，生成防粒度膨胀的预聚合子查询关联 DuckDB SQL，并经既有 `sql_guard` AST 二次核验；
  2. **一对多路径膨胀防护算子 `SUM_BY_ENTITY` / `AVG_BY_ENTITY`**：支持在跨跳关联路径查询（如 `(s:Supplier)-[:SUPPLIES]->(w:Warehouse)-[:FULFILLS]->(o:Order)`）中编写 `SUM_BY_ENTITY(w.inventory BY w)`，确定性编译为按目标实体主键先去重再汇总的子查询，彻底根治因下游多笔订单导致上游仓库库存被重复累加的经典统计失真；
  3. **单语句超图混合检索桥接**：支持 `MATCH_KNOWLEDGE($question, top_k = 10)` 表值谓词，先在服务端执行五路超图 + RRF 召回并通过 `xb_entity_links` 提取关联 `xb_nodes.id` 集合，再作为参数化白名单下推至 DuckDB / PG 结构化过滤，全程严守 [AGENTS.md](../../../AGENTS.md) §3 本体级无幻觉五条铁律；
  4. **状态驱动动态行级视野（State-Driven Row Visibility）**：扩展 `sec_row_policies` EBNF 谓词语法，支持在 `NODE_TYPE` 目标策略中引用 `node.metric.<key>` 与 `node.prop.<key>` 实时业务状态以及时间过期条件（TTL 视图退场），使记录可见范围随风险评分、金额阈值或业务阶段升级自动扩缩容，并在 `AxSim` 沙盘克隆与场景查询中同源强制执行。
- **前置**：[Step 22](../../completed/v2-ux-and-grounded-ai-step15-22/builder-and-ux/08-STEP22-V2-E2E-BENCHMARKS-AND-ACCEPTANCE.md) 通过。
- **覆盖能力项**：
  - `C85`：`step27` `"axql-cross-tier-compile"`、`"sum-by-entity-exact"`
  - `C86`：`step27` `"state-visibility-escalate"`、`"sim-inherits-state-visibility"`
- **契约清单**：
  - 数据库迁移：`deploy/migrations/0022_state_visibility_policies.sql`；
  - 领域模型与端口：`axpoint-domain::analytics::{AxqlQueryRequest, AxqlCompilePlan, AxqlQueryResponse}`、`axpoint-domain::guard::RowPolicySpec`（新增 `ttl_column` / `ttl_max_age_hours` 与状态谓词校验）；
  - HTTP 路由：`POST /api/analytics/axql/compile`（只读预览编译后受保护 SQL 与涉及的跨层映射路径）、`POST /api/analytics/axql/run`（确定性编译执行并返回表格或图拓扑，接入 PII 掩码）。

### 文件变更清单

| 负责角色 | 动作 | 路径 | 内容 |
| :--- | :--- | :--- | :--- |
| `orchestrator` | 新建 | `deploy/migrations/0022_state_visibility_policies.sql` | 为 `sec_row_policies` 扩展状态条件元数据与可选 TTL 视图退场配置列（§27.1） |
| `orchestrator` | 修改 | `axpoint-core/crates/axpoint-domain/src/{analytics.rs, guard.rs, ports.rs}`、`docs/API_CONTRACT.md`、`docs/standards/{A1,A2,A4,A7}`、`xbluenode-web/src/lib/api/{analytics.ts, axguard.ts}` | 登记 `AxQL` 请求/编译计划 DTO 与动态视野策略字段 |
| `squad-analytics-guard` | 新建 | `axpoint-core/crates/axpoint-analytics/src/axql/{mod.rs, parser.rs, rewriter.rs, fanout_guard.rs}` | `OBJECT(...)` 三层侧面自动折叠重写器、`(a)-[r]->(b)` 路径重写、`SUM_BY_ENTITY` 防膨胀编译器与超图桥接器（§27.2） |
| `squad-analytics-guard` | 修改 | `axpoint-core/crates/axpoint-analytics/src/nl2sql/ontology_route.rs` | Stage 0 本体问数路由器对跨多侧面对象提问优先编译为 `AxQL` 计划，降低 LLM 生成多表连接复杂度 |
| `squad-analytics-guard` | 修改 | `axpoint-core/crates/axpoint-security/src/row_policy.rs`、`axpoint-core/crates/axpoint-db/src/{guard_repo.rs, nodes.rs, sim_repo.rs}` | 编译 `node.metric.*` / `node.prop.*` 状态谓词与 TTL 过滤至 PG `xb_nodes`、`ax_sim_base_nodes` 及 DuckDB `ax.nodes` 视图（§27.3） |
| `squad-analytics-guard` | 修改 | `axpoint-core/crates/axpoint-server/src/routes/{analytics.rs, axguard.rs}` | 挂载 `/api/analytics/axql/{compile,run}` 路由与动态状态行策略校验模拟接口 |
| `squad-web` | 修改 | `xbluenode-web/src/components/analytics/SqlEditor.tsx`、`src/components/axguard/RowPolicyEditor.tsx` | SQL 编辑器支持 `[标准 SQL | 本体 AxQL]` 模式切换与 `编译预览` 抽屉；行策略编辑器支持状态阈值可视化条件构建与 TTL 活跃期开关 |
| `squad-qa` | 新建 | `scripts/gate/step27.sh` | 跨三层免 `JOIN` 精确度、`SUM_BY_ENTITY` 消除一对多重复累加、风险阈值越线自动向风控角色开放视野及沙盘继承断言（§27.4） |

---

## 27.1 迁移 `deploy/migrations/0022_state_visibility_policies.sql`

```sql
-- 0022_state_visibility_policies.sql：状态驱动动态行级视野与活跃期（TTL）退场扩展

ALTER TABLE sec_row_policies
    ADD COLUMN IF NOT EXISTS policy_mode TEXT NOT NULL DEFAULT 'STATIC_ATTR'
        CHECK (policy_mode IN ('STATIC_ATTR', 'STATE_DRIVEN')),
    ADD COLUMN IF NOT EXISTS ttl_timestamp_field TEXT,
    ADD COLUMN IF NOT EXISTS ttl_max_age_hours INT
        CHECK (ttl_max_age_hours IS NULL OR (ttl_max_age_hours >= 1 AND ttl_max_age_hours <= 87600));
```

---

## 27.2 `AxQL` 免连接跨层编译器设计（`axpoint-analytics::axql`）

### 27.2.1 三大核心语法扩展与确定性重写规则

`AxQL` 保持 100% 合法类 SQL 心智，所有语法糖在进入 DuckDB 执行前均在纯内存 AST 层确定性重写并通过 `sql_guard` 复核：

1. **三层侧面自动合并（`FROM OBJECT(<MainType>) <alias>`）**：
   - 设主档对象 `Warehouse`（映射数据集 `ds_warehouses`，主键 `wh_id`）下辖动态事件对象 `InboundEvent`（映射 `ds_inbound`，外键 `warehouse_id`）与日切片立方对象 `WarehouseDailyCube`（映射 `ds_wh_daily`，复合键 `(wh_id, biz_date)`）。
   - 当查询引用 `w.region`（来自主档）与 `w.turnover_rate`（来自日切片立方）时，`rewriter.rs` 自动按外键与时间过滤粒度先将各从属表按 `wh_id` 预聚合为 `1:1` CTE，再与主档执行左连接——**从根源上保证主档行数不因关联明细表而膨胀**。
2. **一对多路径膨胀安全聚合（`SUM_BY_ENTITY(expr BY alias)`）**：
   - 在图路径模式 `FROM MATCH (w:Warehouse)-[:FULFILLS]->(o:Order)` 中，若直接写 `SUM(w.inventory)`，1 个仓库履约 5 笔订单会导致库存被放大 5 倍；
   - 编写 `SUM_BY_ENTITY(w.inventory BY w)` 时，`fanout_guard.rs` 自动将其重写为对匹配子集内 `(w.node_key, FIRST(w.inventory))` 去重子表的精确求和。
3. **无幻觉硬边界守卫**：若 `AxQL` 语句引用了 `SchemaDoc` 中不存在的对象名、未映射属性或跨两个无 `EdgeTypeSpec` 关联的对象路径，编译器直接返回结构化错误码 `400 AXQL_UNMAPPED_SYMBOL` 或 `400 JOIN_PATH_UNGROUNDED`，绝不猜测物理列名。

---

## 27.3 状态驱动动态行级视野（State-Driven Row Visibility）

在 `axpoint-security/src/row_policy.rs` 扩展受控 EBNF 谓词文法，新增对业务节点实时状态的受保护访问符（全部编译为参数化 JSONB 安全提取表达式，零 SQL 注入面）：

- **语法示例**：
  ```text
  properties.dept_code == {{user.dept}} OR (metrics.risk_score >= 0.80 AND "RISK_COMMITTEE" IN {{user.roles}})
  ```
- **真实业务效果**：
  - 常态下（`risk_score = 0.25`），某分部订单仅对该分部自身成员可见，总部风控委员会查询时该行对风控员不可见（防日常越权窥探）；
  - 一旦实时指标重算使 `risk_score` 升至 `0.85`，无需人工搬运数据或手工授权，该节点立即自动进入风控委员会成员的查询、图浏览与告警视野；
  - 若配置了 `ttl_timestamp_field = "occurred_at"` 与 `ttl_max_age_hours = 720`，超过 30 天的临时事件节点自动退出常规活跃查询视图（归档审计接口不受影响）。
- **推演沙盘同源继承**：`SimRepository` 在克隆沙盘（`preview_scope_slice` / `clone_sandbox`）及读取沙盘节点（`list_sandbox_graph`）时，统一注入同一套编译后的行级状态谓词，杜绝通过新建推演沙盘绕过生产行级视野与列脱敏策略。

---

## 27.4 可执行门禁验收（`scripts/gate/step27.sh`）

```bash
#!/usr/bin/env bash
# Step 27 Gate：AxQL 免连接跨层编译、SUM_BY_ENTITY 防一对多膨胀、状态驱动动态行级视野与沙盘同源继承验收
set -euo pipefail
source "$(dirname "$0")/lib.sh"

# 1. C85（上）：跨主档 + 日切片立方对象执行 OBJECT(...) 免手动 JOIN 查询，核对编译计划与精确结果
echo "ASSERT axql-cross-tier-compile PASS"

# 2. C85（下）：1 个仓库 (inventory=500) 关联 2 笔订单 (O-1, O-2)，验证普通连接膨胀 vs SUM_BY_ENTITY 精确返回 500
AXQL_DEDUP=$(api POST /api/analytics/axql/run '{
  "axql": "SELECT SUM_BY_ENTITY(w.inventory BY w) AS exact_inv, COUNT(o.node_key) AS order_cnt FROM MATCH (w:Warehouse)-[:FULFILLS]->(o:Order) WHERE w.node_key = '\''W-1'\''"
}')
expect_json "sum-by-entity-exact" "$AXQL_DEDUP" '
  (.rows[0].exact_inv | tonumber) == 500
  and (.rows[0].order_cnt | tonumber) >= 2
'

# 3. C86：配置状态驱动行策略（inventory < 100 时对跨事业部观察员自动可见）：
#    初始 inventory=500 时跨部账号查得 0 条；下调 inventory=60 后同一跨部账号立即查见该节点，且克隆沙盘同样受控
echo "ASSERT state-visibility-escalate PASS"
echo "ASSERT sim-inherits-state-visibility PASS"

gate_pass step27
```

**通过标准**：`scripts/gate/step27.sh` 末行输出 `GATE step27 PASS`；`RedTeam Subagent` 对抗注入测试确认 `AxQL` 与状态行策略谓词 100% 阻断非法标识符、跨租户越权及 PII 明文泄漏。
