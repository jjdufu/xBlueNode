# 05 · AxSim 因果策略推演与多方案对比规范（Step 09）

本文件定义 **AxSim 写时复制因果策略推演系统** 的完整数学引擎、多策略并行对比（Scenario Comparison）以及“推演确认后落回生产动作”的决策闭环。

---

## Step 09：写时复制沙盘、多指标公式推演、多方案对比与决策闭环

### 9.1 推演引擎核心需求
针对真实企业决策场景（如供应链断供应急、产线排产切换、库存水位调整、风险传导评估），`AxSim` 必须解决纯线性比例推演的局限：
1. **写时复制（CoW）强隔离**：克隆生产 `xb_nodes` / `xb_links` 及 `xb_edge_types` 公式进入独立沙盘快照，任何推演试算零污染生产表。
2. **多指标联动与非负/产能封顶约束**：
   - 每个节点支持多个关联指标（例如 `output`、`inventory`、`backlog`、`fulfill_rate`、`cost`）。
   - 支持节点内部派生公式（例如：`fulfill_rate = min(1.0, output / max(demand, 1.0))`）与边上的因果传导公式（源自 `BlueEdgeTypeDef.impact_formula`）。
3. **有向环阻断与阻尼不动点收敛**：
   - 生产拓扑经常出现反馈环（如 `供应商 <-> 工厂 <-> 仓库`）。
   - 采用**边级路径防环 + 阻尼不动点迭代（最大迭代 16 轮，收敛阈值 `1e-4`）**，避免无界振荡或重复叠加。
4. **多策略并行对比（Scenario A / B / C Diff）**：
   - 在同一个沙盘快照基线上建立多个对比分支（如：`方案A：华东减产不调拨`、`方案B：从华南仓调拨 300 单位`、`方案C：启用备用供应商`）。
   - 自动计算各方案相对于基线及方案相互之间的全局 KPI 汇总差值，并支持调用 LLM 输出自然语言利弊分析。
5. **决策动作落地（Apply to Production）**：
   - 选定胜出策略后，可一键生成对应的 `AxFlow` 动作指令集（附人工审批单），经批准后下发执行。

### 9.2 数据库 Schema（`deploy/migrations/005_axsim_and_xblueview.sql` - 推演部分）

```sql
-- 1. 推演沙盘主表（持有一份不可变的基线 CoW 快照）
CREATE TABLE IF NOT EXISTS ax_sandboxes (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id       TEXT NOT NULL DEFAULT 'default',
    name            TEXT NOT NULL,
    description     TEXT NOT NULL DEFAULT '',
    base_snapshot   JSONB NOT NULL, -- {nodes: [...], links: [...], node_types: [...], edge_types: [...]}
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- 2. 沙盘内的多策略推演分支（一个沙盘可挂多个平行对比方案）
CREATE TABLE IF NOT EXISTS ax_sim_scenarios (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id       TEXT NOT NULL DEFAULT 'default',
    sandbox_id      UUID NOT NULL REFERENCES ax_sandboxes(id) ON DELETE CASCADE,
    name            TEXT NOT NULL,
    actions         JSONB NOT NULL DEFAULT '[]'::jsonb, -- [{target_node_id, metric_key, mode: 'RATIO'|'ABSOLUTE'|'ADD', value}]
    sim_result      JSONB NOT NULL DEFAULT '{}'::jsonb, -- {impacts, node_states, kpi_summary, convergence_rounds}
    ai_brief        TEXT NOT NULL DEFAULT '',
    applied_run_id  UUID REFERENCES ax_flow_runs(id) ON DELETE SET NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_ax_scenarios_sandbox ON ax_sim_scenarios (tenant_id, sandbox_id);
```

### 9.3 Rust 因果推演数学内核（`axpoint-domain/src/topology.rs`）

#### 干预动作模型（支持三种模式）
```rust
/// 推演干预动作模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum InterventionMode {
    /// 相对比例变化，如 -0.5 表示下降 50%。
    Ratio,
    /// 绝对值增量，如 +300 表示补充 300 单位库存。
    Add,
    /// 强制设定为目标值，如 0.0 表示完全停产。
    Absolute,
}

/// 单条推演干预动作。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AxSimAction {
    /// 目标节点 UUID。
    pub target_node_id: Uuid,
    /// 干预指标键名。
    pub metric_key: String,
    /// 干预模式。
    pub mode: InterventionMode,
    /// 干预数值。
    pub value: f64,
}
```

#### 收敛与防环传导算法规范（`simulate_scenario_cow`）
1. **初始化状态矩阵**：在内存中克隆基线节点的所有 `metrics`，形成 `baseline: HashMap<(Uuid, String), f64>` 与 `current: HashMap<(Uuid, String), f64>`，不注入默认虚构基线值（若节点缺少某指标则按 `0.0` 处理并标记）。
2. **施加初始干预**：将 `actions` 按 `Ratio` / `Add` / `Absolute` 应用到目标节点，计算初始相对变化率 `delta_ratio` 与绝对变化量 `delta_abs`，写入初始激活队列。
3. **带访问路径防环的迭代传导**：
   - 使用 `(curr_node_id, metric_key, visited_path_set)` 防止同一条传播路径重复穿越同一个节点（彻底消除环路自激放大）。
   - 对每条出边 `link` 查找对应 `BlueEdgeTypeDef.impact_formula`（默认 `src_delta_ratio * weight`），借助 `axpoint-domain::expr` 安全求值器计算对下游节点目标指标的影响量。
   - 若节点定义了上下界约束（如 `min = 0.0`, `max = capacity`），使用 `clamp` 截断。
4. **节点内部指标联动**：每次外部传导修改某节点主指标后，按拓扑序重新求值该节点类型的衍生指标公式（例如 `fulfill_rate`、`revenue_loss`）。
5. **生成全局 KPI 摘要（`KpiSummary`）**：按节点类型汇总核心指标的 `before_total`、`after_total`、`net_delta` 与 `affected_node_count`。

### 9.4 多策略并行对比（Scenario Diff Engine）
函数 `compare_scenarios(scenarios: &[ScenarioSnapshot]) -> ScenarioCompareResult`：
- 对齐每个节点的各指标在不同方案下的最终值，输出对比矩阵：
  - `node_id`, `title`, `type_name`, `metric_key`, `baseline_val`, `values_by_scenario: HashMap<Uuid, f64>`
  - 各方案的 `KpiSummary`（如总产出损失、受波及订单数、预计履约率、挽回金额）
- 自动标出每个 KPI 维度下的最优方案（`best_scenario_id`）。
- 支持调用 `AiGateway` 生成《多策略对比决策简报》。

### 9.5 决策下发生产闭环
接口 `POST /api/axsim/scenarios/{scenario_id}/apply`：
1. 将该策略分支的 `actions` 转化为一条受控的 `AxFlow` 运行请求（结构：`HUMAN_APPROVAL` → `ACTION_EMIT` 写回生产节点/触发外部 ERP/MES Webhook）。
2. 自动生成待审批单，确保符合“沙盘试算 → 人类确认 → 生产生效”的安全边界。

### 9.6 核心 API 契约（`/api/axsim/*`）

| 方法与路径 | 说明 |
| :--- | :--- |
| `POST /api/axsim/sandboxes` | 从当前生产 `xb_nodes`/`xb_links`/`xb_node_types`/`xb_edge_types` 克隆不可变基线快照 |
| `GET /api/axsim/sandboxes/{id}` | 获取沙盘详情及挂载的所有对比策略分支 |
| `POST /api/axsim/sandboxes/{id}/scenarios` | 在沙盘内创建/运行一条推演策略分支 |
| `POST /api/axsim/sandboxes/{id}/compare` | 传入多个 `scenario_ids`，返回全指标横向 Diff 对比表与 AI 决策简报 |
| `POST /api/axsim/scenarios/{scenario_id}/apply` | 将选定策略转化为带人工审批闸门的 AxFlow 生产执行单 |

### 9.7 Step 09 可验证门禁（Verification Gate）

```bash
# 1. 执行数据库迁移与推演数学内核单测（覆盖：三模式干预、有向环防自激、上下界封顶、多策略Diff）
psql "$DATABASE_URL" -f deploy/migrations/005_axsim_and_xblueview.sql
cargo test -p axpoint-domain topology
cargo clippy --workspace --all-targets -- -D warnings

# 2. 端到端沙盘对比验证
curl -s -X POST http://127.0.0.1:9995/api/axsim/sandboxes/<SB_ID>/compare \
  -H "Content-Type: application/json" \
  -d '{"tenant_id":"default","scenario_ids":["<SCEN_1>","<SCEN_2>"]}' | jq '{kpi_comparison, best_by_metric}'
```
**通过标准**：
1. 存在 `A -> B -> A` 有向环时，推演正常终止，不会出现无限放大或 `NaN`，且节点 `A` 不会被自己发出的波及再次二次扣减。
2. `compare` 接口清晰返回方案 1 与方案 2 对下游订单 `C` 的差异影响量，且生产库 `xb_nodes` 在 `/apply` 审批通过前保持绝对只读不变。
3. `/apply` 触发后成功创建 `WAITING_APPROVAL` 审批单；审批通过后生产节点的指标按策略动作更新并记录 `xb_metric_series(source_tag='ACTION')`。
