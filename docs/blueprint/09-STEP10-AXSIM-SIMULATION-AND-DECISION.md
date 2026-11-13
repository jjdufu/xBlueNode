# 09 · Step 10：AxSim 推演与决策

AxSim 在与生产隔离的沙盘中回答“如果……会怎样”：按范围克隆业务图，以写时复制覆盖层记录假设修改，用基于强连通分量分解与阻尼雅可比迭代的传导算法求解干预（do 算子）的影响，保存检查点用于逐步回放，对多个方案并排比较，支持以自然语言发起推演与追问结果，最终经审批把选定的修改下发回生产。

---

## 10.0 目标、前置与契约

- **目标**：范围克隆沙盘；节点 / 关系 / 指标的写时复制编辑；稳态与分步两种传导模式；三种干预方式；不确定性抽样；检查点回放；方案对比；推演问数；决策下发（审批、冲突检测、原子写入、触发流程）。
- **前置**：Step 06、Step 07 通过。
- **契约清单**：`0010_axsim_scenarios.sql`；`axpoint-domain::sim`（`SimGraph`、`AxSimAction`、`PropagationSpec`、`SimParams`、`SimResult`、`ImpactRow`、`Checkpoint`）；端口 `SimRepository`；`docs/API_CONTRACT.md` 的 `/api/axsim/*`；`lib/api/axsim.ts`。

### 文件变更清单

| 动作 | 路径 | 内容 |
| :--- | :--- | :--- |
| 新建 | `deploy/migrations/0010_axsim_scenarios.sql` | §10.1 |
| 新建 | `axpoint-core/crates/axpoint-domain/src/sim/{mod.rs, graph.rs, action.rs, scc.rs, steady.rs, stepped.rs, uncertainty.rs, explain.rs}` | 推演内核 |
| 修改 | `axpoint-core/crates/axpoint-domain/src/topology.rs` | `AxSimAction` 改为 §10.3 结构（保留 `delta_ratio` 简写）；`simulate_causal_propagation` 改为调用 `sim::steady::solve`，删除缺失指标时的默认基值 |
| 新建 | `axpoint-core/crates/axpoint-db/src/sim_repo.rs` | 克隆、覆盖层、场景、检查点、决策 |
| 修改 | `axpoint-core/crates/axpoint-server/src/routes/axsim.rs` | §10.9 路由 |
| 新建 | `axpoint-core/crates/axpoint-server/src/sim_ask.rs` | 推演问数 |
| 修改 | `axpoint-core/crates/axpoint-flow/src/operators/causal_sim.rs` | 切换为完整推演端口 |
| 修改 | `axpoint-core/crates/axpoint-agent/src/builtin_tools.rs` | 注册 `run_simulation` |
| 修改 | `xbluenode-web/src/pages/AxSimPage.tsx`；新建 `src/components/axsim/{SandboxWizard.tsx, OverlayEditor.tsx, ActionList.tsx, ImpactTable.tsx, ImpactGraph.tsx, ReplayTimeline.tsx, ScenarioCompare.tsx, SimAsk.tsx, DecisionPanel.tsx}` | 推演工作台 |
| 新建 | `axpoint-core/crates/axpoint-domain/tests/sim.rs`、`scripts/bench/sim-bench.sh` | 测试与基准 |
| 新建 | `scripts/seed/10-axsim.sh` | 演示沙盘与场景 |
| 新建 | `scripts/gate/step10.sh` | 门禁 |

---

## 10.1 迁移 `deploy/migrations/0010_axsim_scenarios.sql`

```sql
-- 0010_axsim_scenarios.sql：沙盘范围与基线快照、写时复制覆盖层、场景、检查点、决策

ALTER TABLE ax_sandboxes ADD COLUMN IF NOT EXISTS scope            JSONB NOT NULL DEFAULT '{}'::jsonb;
ALTER TABLE ax_sandboxes ADD COLUMN IF NOT EXISTS status           TEXT NOT NULL DEFAULT 'READY'
    CHECK (status IN ('CLONING','READY','ARCHIVED'));
ALTER TABLE ax_sandboxes ADD COLUMN IF NOT EXISTS node_count       INT NOT NULL DEFAULT 0;
ALTER TABLE ax_sandboxes ADD COLUMN IF NOT EXISTS link_count       INT NOT NULL DEFAULT 0;
ALTER TABLE ax_sandboxes ADD COLUMN IF NOT EXISTS base_captured_at TIMESTAMPTZ;
ALTER TABLE ax_sandboxes ADD COLUMN IF NOT EXISTS schema_version   INT;
ALTER TABLE ax_sandboxes ADD COLUMN IF NOT EXISTS created_by       UUID;
ALTER TABLE ax_sandboxes ADD COLUMN IF NOT EXISTS updated_at       TIMESTAMPTZ NOT NULL DEFAULT NOW();

CREATE TABLE IF NOT EXISTS ax_sim_base_nodes (
    tenant_id  TEXT NOT NULL,
    sandbox_id UUID NOT NULL REFERENCES ax_sandboxes(id) ON DELETE CASCADE,
    node_id    UUID NOT NULL,
    type_name  TEXT NOT NULL,
    node_key   TEXT NOT NULL,
    title      TEXT NOT NULL,
    metrics    JSONB NOT NULL,
    properties JSONB NOT NULL,
    row_version BIGINT NOT NULL,                -- 克隆时生产节点的 row_version，用于下发冲突检测
    PRIMARY KEY (sandbox_id, node_id)
);

CREATE TABLE IF NOT EXISTS ax_sim_base_links (
    tenant_id   TEXT NOT NULL,
    sandbox_id  UUID NOT NULL REFERENCES ax_sandboxes(id) ON DELETE CASCADE,
    link_id     UUID NOT NULL,
    link_type   TEXT NOT NULL,
    src_node_id UUID NOT NULL,
    dst_node_id UUID NOT NULL,
    weight      DOUBLE PRECISION NOT NULL,
    PRIMARY KEY (sandbox_id, link_id)
);

CREATE TABLE IF NOT EXISTS ax_sim_overlay_nodes (
    tenant_id  TEXT NOT NULL,
    sandbox_id UUID NOT NULL REFERENCES ax_sandboxes(id) ON DELETE CASCADE,
    node_id    UUID NOT NULL,                   -- 沙盘内新建的节点使用新 UUID
    type_name  TEXT,
    node_key   TEXT,
    title      TEXT,
    metrics    JSONB,                           -- 只含被修改的键，合并到基线
    properties JSONB,
    is_new     BOOLEAN NOT NULL DEFAULT FALSE,
    deleted    BOOLEAN NOT NULL DEFAULT FALSE,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (sandbox_id, node_id)
);

CREATE TABLE IF NOT EXISTS ax_sim_overlay_links (
    tenant_id   TEXT NOT NULL,
    sandbox_id  UUID NOT NULL REFERENCES ax_sandboxes(id) ON DELETE CASCADE,
    link_id     UUID NOT NULL,
    link_type   TEXT,
    src_node_id UUID,
    dst_node_id UUID,
    weight      DOUBLE PRECISION,
    is_new      BOOLEAN NOT NULL DEFAULT FALSE,
    deleted     BOOLEAN NOT NULL DEFAULT FALSE,
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (sandbox_id, link_id)
);

CREATE TABLE IF NOT EXISTS ax_sim_scenarios (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id    TEXT NOT NULL,
    sandbox_id   UUID NOT NULL REFERENCES ax_sandboxes(id) ON DELETE CASCADE,
    name         TEXT NOT NULL,
    description  TEXT NOT NULL DEFAULT '',
    actions      JSONB NOT NULL,                -- AxSimAction[]
    params       JSONB NOT NULL,                -- SimParams
    overlay_hash TEXT NOT NULL,                 -- 运行时覆盖层内容哈希，覆盖层变化后结果标记为过期
    result       JSONB,                         -- SimResult 摘要（Top 200 影响、汇总 KPI、收敛信息）
    status       TEXT NOT NULL DEFAULT 'PENDING' CHECK (status IN ('PENDING','RUNNING','SUCCEEDED','FAILED')),
    error_message TEXT,
    created_by   UUID,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    finished_at  TIMESTAMPTZ,
    UNIQUE (sandbox_id, name)
);

CREATE TABLE IF NOT EXISTS ax_sim_impacts (
    tenant_id     TEXT NOT NULL,
    scenario_id   UUID NOT NULL REFERENCES ax_sim_scenarios(id) ON DELETE CASCADE,
    node_id       UUID NOT NULL,
    metric        TEXT NOT NULL,
    before_val    DOUBLE PRECISION NOT NULL,
    after_val     DOUBLE PRECISION NOT NULL,
    delta         DOUBLE PRECISION NOT NULL,
    delta_ratio   DOUBLE PRECISION,
    p10           DOUBLE PRECISION,
    p90           DOUBLE PRECISION,
    hop_distance  INT NOT NULL,
    top_path      JSONB NOT NULL DEFAULT '[]'::jsonb,   -- 贡献最大的传导路径 [node_id...]
    PRIMARY KEY (scenario_id, node_id, metric)
);

CREATE TABLE IF NOT EXISTS ax_sim_checkpoints (
    tenant_id   TEXT NOT NULL,
    scenario_id UUID NOT NULL REFERENCES ax_sim_scenarios(id) ON DELETE CASCADE,
    seq         INT NOT NULL,                   -- 分步模式为时间步；稳态模式为迭代轮次采样
    label       TEXT NOT NULL,
    changes     JSONB NOT NULL,                 -- 相对上一检查点变化的 {node_id: {metric: value}}
    PRIMARY KEY (scenario_id, seq)
);

CREATE TABLE IF NOT EXISTS ax_sim_decisions (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id       TEXT NOT NULL,
    scenario_id     UUID NOT NULL REFERENCES ax_sim_scenarios(id) ON DELETE CASCADE,
    title           TEXT NOT NULL,
    changes         JSONB NOT NULL,             -- [{node_id, metric, base_value, target_value}]
    trigger_flow_id UUID REFERENCES ax_flows(id) ON DELETE SET NULL,
    status          TEXT NOT NULL DEFAULT 'PENDING_APPROVAL'
                    CHECK (status IN ('PENDING_APPROVAL','APPROVED','APPLIED','REJECTED','CONFLICT','FAILED')),
    approval_id     UUID,
    conflict_report JSONB NOT NULL DEFAULT '[]'::jsonb,
    tolerance_ratio DOUBLE PRECISION NOT NULL DEFAULT 0.0,
    created_by      UUID,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    applied_at      TIMESTAMPTZ,
    applied_by      UUID
);

SELECT ax_enable_tenant_rls(t) FROM unnest(ARRAY[
    'ax_sim_base_nodes','ax_sim_base_links','ax_sim_overlay_nodes','ax_sim_overlay_links',
    'ax_sim_scenarios','ax_sim_impacts','ax_sim_checkpoints','ax_sim_decisions'
]::regclass[]) AS t;
```

`ax_sandboxes.base_snapshot`、`actions`、`sim_result` 为仓库已有列：本步起 `base_snapshot` 不再写入（基线存于 `ax_sim_base_*`），`actions` 与 `sim_result` 只由快速推演接口（§10.9 的 `/simulate`）写入最近一次结果。

---

## 10.2 范围克隆与写时复制

### 10.2.1 创建沙盘 `POST /api/axsim/sandboxes`

```json
{"name": "华东断供推演", "scope": {"root_node_ids": ["…"], "depth": 3, "direction": "BOTH",
 "type_names": [], "link_types": [], "max_nodes": 50000}}
```

- `scope` 缺省（仓库已有调用只传 `name`）时克隆整个租户业务图，节点数超过 `max_nodes`（默认 50 000）返回 400 `LIMIT_OUT_OF_RANGE`，提示指定范围。
- 克隆在一个租户事务中执行：递归 CTE 按范围收集节点 ID（深度 ≤ 6），`INSERT INTO ax_sim_base_nodes SELECT ... FROM xb_nodes WHERE id = ANY(...)`，关系只复制两端都在范围内的；记录 `base_captured_at = now()`、`schema_version`、计数。
- 写审计 `SIM_APPLY` 之外的普通操作不审计；克隆耗时超过 5 秒时改为后台任务（状态 `CLONING`，前端轮询）。

### 10.2.2 读取视图

沙盘内节点 = `base LEFT JOIN overlay`：`deleted=true` 的排除；`metrics = base.metrics || overlay.metrics`；`is_new=true` 的覆盖层节点直接加入。关系同理。`sim_repo.rs::load_graph(sandbox_id) -> SimGraph` 一次读出合并结果（两条 SQL + 内存合并）。

### 10.2.3 编辑覆盖层

| 接口 | 说明 |
| :--- | :--- |
| `PATCH /api/axsim/sandboxes/{id}/nodes/{node_id}` | `{metrics?, properties?, title?}` 写覆盖层（键级合并） |
| `POST /api/axsim/sandboxes/{id}/nodes` | 新建沙盘节点（`is_new=true`） |
| `DELETE /api/axsim/sandboxes/{id}/nodes/{node_id}` | 覆盖层标记删除（并删除其沙盘关系） |
| `POST /api/axsim/sandboxes/{id}/links`、`PATCH .../links/{link_id}`、`DELETE .../links/{link_id}` | 关系同理 |
| `POST /api/axsim/sandboxes/{id}/reset` | 清空覆盖层 |
| `POST /api/axsim/sandboxes/{id}/refresh-base` | 以当前生产数据重新克隆基线（覆盖层保留，基线中已不存在的节点的覆盖层条目标为孤立并在响应中列出） |

生产表在任何沙盘操作中都不被写入（门禁断言 §10.10）。

---

## 10.3 干预与传导参数

### 10.3.1 `AxSimAction`

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AxSimAction {
    pub target_node_id: Uuid,
    pub metric_key: String,
    #[serde(default)]
    pub mode: Option<InterventionMode>,   // RATIO | ADD | ABSOLUTE
    #[serde(default)]
    pub value: Option<f64>,
    /// 简写：等价于 mode = RATIO、value = delta_ratio。
    #[serde(default)]
    pub delta_ratio: Option<f64>,
    /// 分步模式中干预生效的时间步，默认 0。
    #[serde(default)]
    pub at_step: u32,
}
```

规范化（`action.rs::normalize`）：`delta_ratio` 与 `mode/value` 同时出现 → 400 `INVALID_ARGUMENT`；只有 `delta_ratio` → `(RATIO, delta_ratio)`；`mode` 存在而 `value` 缺失 → 400。`RATIO` 要求 `value > -1`；目标节点缺少该指标且模式为 `RATIO` / `ADD` → 400 `INVALID_ARGUMENT`（`metric missing on target`），`ABSOLUTE` 允许（基值视为缺失，结果中 `before_val` 记为 `null` 并在 `delta_ratio` 中给 `null`）。

### 10.3.2 传导规格（`xb_edge_types.propagation`）

```json
{
  "rules": [
    {"src_metric": "output", "dst_metric": "inventory", "mode": "RATIO", "coefficient": 0.8, "lag_steps": 1},
    {"src_metric": "inventory", "dst_metric": "fulfillment", "mode": "RATIO", "coefficient": null, "lag_steps": 0}
  ],
  "clamp": {"min": 0, "max": null}
}
```

- `coefficient=null` 时取关系实例的 `weight`（缺省为关系类型 `impact_weight`）。
- 关系类型未定义 `rules` 时使用默认规则：同名指标、`RATIO`、`coefficient = weight`、`lag_steps = 0`。
- `mode=RATIO`：传导相对变化率；`mode=ADD`：传导绝对变化量。
- `clamp`：目标值的上下界（缺省下界 0，适用于库存、产能等非负指标；可设 `null` 取消）。

### 10.3.3 `SimParams`

| 字段 | 默认 | 说明 |
| :--- | :--- | :--- |
| `mode` | `STEADY` | `STEADY`（稳态）或 `STEPPED`（分步） |
| `damping` | `0.85` | 阻尼系数 `α ∈ (0, 1]` |
| `tolerance` | `1e-9` | 收敛阈值（无穷范数） |
| `max_iterations` | `500` | 环内迭代上限 |
| `min_effect` | `1e-6` | 相对变化低于此值视为无影响（不写入结果） |
| `max_hops` | `null` | 限制传导跳数（`null` 不限） |
| `horizon_steps` | `12` | 分步模式的步数（1～104） |
| `uncertainty` | `null` | `{samples: 1..=500, coefficient_cv: 0.2, seed: 42}` |
| `checkpoint_every` | `1` | 分步模式每 N 步保存一个检查点；稳态模式保存第 1、2、4、8… 轮与最终结果 |

---

## 10.4 稳态传导算法（`sim/steady.rs`）

对每个指标通道（`(type, metric)` 组合经传导规则连成的子图）分别求解。记 `x` 为节点该指标的变化量（`RATIO` 通道为相对变化率，`ADD` 通道为绝对变化量），`W[v][u]` 为 `u → v` 的系数，`b` 为干预项，`F` 为被干预（do 算子固定）的节点集合。

稳态方程：`x_v = b_v`（`v ∈ F`）；`x_v = Σ_u W[v][u] · x_u`（`v ∉ F`）。

1. **do 算子**：被干预节点的入边全部切断（其值由干预固定，不受上游影响），这正是“干预”与“观察”的区别。
2. **SCC 分解**：对切断后的图做 Tarjan 强连通分量分解，得到分量的拓扑序。
3. **按拓扑序求解**：
   - 单节点且无自环的分量：一次计算 `x_v = Σ W[v][u] · x_u`（上游已全部求得），精确；
   - 含环分量：阻尼雅可比迭代 `x^{k+1} = (1-α) x^k + α (W_C x^k + c)`，其中 `c` 为来自分量外上游的已知输入；收敛（`‖x^{k+1} - x^k‖∞ < tolerance`）则停止；达到 `max_iterations` 仍未收敛时，若 `‖x‖∞` 在最近 50 轮单调增长则判定发散，结果标记 `converged=false, reason="DIVERGENT"` 并返回 400 `SIM_DIVERGENT`（提示环路系数乘积 ≥ 1），否则标记 `converged=false, reason="MAX_ITER"` 并返回最后一次迭代值。
4. **转换为指标值**：`RATIO`：`after = before × (1 + x)`；`ADD`：`after = before + x`；`ABSOLUTE` 干预的节点 `after = value`，其通道变化量 `x = (value - before) / before`（`RATIO`）或 `value - before`（`ADD`）。再应用 `clamp`。
5. **缺失指标**：节点缺少该指标时，该节点在该通道上不产生结果，但仍以 `x` 向下游传导（`RATIO` 语义下变化率可传递）；结果中记录 `warnings: [{node_id, code: "METRIC_MISSING"}]`。不使用任何默认基值。
6. **跳数与路径**：从干预节点集合做多源 BFS 得到 `hop_distance`；`top_path` 为使 `Π |W|` 最大的路径（在 DAG 化后的分量图上做最长乘积路径，环内取迭代中贡献最大的入边回溯）。

**正确性测试**（`tests/sim.rs`）：

| 用例 | 期望 |
| :--- | :--- |
| 链 A→B→C，系数 0.5、0.4，A 干预 −50% | B = −25%，C = −10%（精确） |
| 菱形 A→B→D、A→C→D | D 为两条路径之和 |
| 二元环 A⇄B（系数 0.5），干预 A | B 收敛到 −25%（A 被固定，环被切断），与解析解一致 |
| 三元环 B→C→D→B（乘积 0.125），外部输入 A→B | 迭代结果与 `(I - W_C)^{-1} c` 的解析解误差 < 1e-8 |
| 环乘积 = 1.2 | `SIM_DIVERGENT` |
| 目标缺少指标且 `RATIO` | 400；中间节点缺少指标时下游仍有结果，`warnings` 含 `METRIC_MISSING` |
| `delta_ratio` 简写与 `mode=RATIO,value` 等价 | 结果逐项相同 |

---

## 10.5 分步传导与检查点（`sim/stepped.rs`）

- 时间步 `t = 0..horizon_steps`；干预在 `at_step` 生效（`ABSOLUTE` 从该步起保持固定值，`RATIO` / `ADD` 为一次性冲击后保持）。
- 每步：`x_v(t) = Σ_u W[v][u] · x_u(t - lag(u→v))`（`lag=0` 的边在同一步内按 SCC 拓扑序计算，含环且 `lag=0` 的分量在该步内用 §10.4 迭代求解）。
- 每 `checkpoint_every` 步写一个检查点（只存相对上一检查点变化的值），最后一步结果写入 `ax_sim_impacts`。
- 回放：`GET /api/axsim/scenarios/{id}/checkpoints` 返回检查点序列，前端 `ReplayTimeline` 播放 / 拖动时间轴时在图上着色。
- 分步模式下 `after_val` 为最后一步的值；每个节点另返回 `series: [{step, value}]`（只对 Top 50 影响节点返回，以控制响应大小）。

---

## 10.6 不确定性（`sim/uncertainty.rs`）

`uncertainty` 不为空时：以 `seed` 初始化 `rand_chacha::ChaCha8Rng`，每次抽样把每条边系数乘以 `max(0, 1 + N(0, coefficient_cv))`，重复求解 `samples` 次（rayon 并行），对每个 `(node, metric)` 取 P10 / P50 / P90；`after_val` 取确定性求解值，`p10` / `p90` 写入结果。同一 `seed` 结果完全可复现。

---

## 10.7 场景、对比与推演问数

### 10.7.1 场景

`POST /api/axsim/sandboxes/{id}/scenarios {name, actions, params}` → 后台执行（小图同步返回）：`load_graph` → 规范化干预 → 求解 → 写 `ax_sim_impacts`、检查点、`result` 摘要（Top 200、按类型汇总的 KPI：对每个类型的每个指标求 `before` 总和、`after` 总和、变化率、受影响节点数）→ `overlay_hash`。覆盖层后续变化时，场景列表显示“结果已过期”，`POST /scenarios/{id}/rerun` 重新计算。

### 10.7.2 对比

`GET /api/axsim/scenarios/compare?ids=a,b,c&metrics=inventory,fulfillment&type_name=Order`：同一沙盘内 2～5 个场景；返回 KPI 对比表（每个场景一列）与节点级对比（`after_val` 与相对基线变化，按场景间差异最大排序，前 200 行）。不同沙盘的场景对比返回 400。

**AI 研判**：`briefing=true` 时追加 `briefing` 字段：以 `SUMMARIZE` 用途模型输入 KPI 对比表与各场景 Top 10 影响，输出 `{summary, recommended_scenario_id, reasons[], risks[]}`（JSON Schema 约束）；`recommended_scenario_id` 必须是参与对比的场景之一，文本中出现的数值必须能在输入表中找到（同 Step 04 §04.6 的数值校验），校验失败时只返回对比表并在 `briefing.error` 中说明。

### 10.7.3 推演问数（`sim_ask.rs`）

`POST /api/axsim/ask {question, sandbox_id, scenario_id?}`：

1. 意图：`WHAT_IF`（问题含“如果 / 假如 / 假设 / 若 / 下降 / 增加 / 停产 / 断供”等并能解析出对象与变化）或 `QUERY_RESULT`（针对已有场景追问）。
2. `WHAT_IF`：大模型输出 `{actions: [{node_ref: {type_name?, text}, metric_key, mode, value}], name}`；`node_ref.text` 经 Step 03 节点混合检索在沙盘节点中解析（多候选时返回 `need_clarification` 与候选列表）；创建并运行场景；用结果 KPI 与 Top 10 影响生成回答（数值校验同 Step 04 §04.6.6）。
3. `QUERY_RESULT`：把场景的 `ax_sim_impacts` 加载到内存 DuckDB 表 `impacts(node_id, type_name, title, metric, before_val, after_val, delta, delta_ratio, hop_distance)`，以 Step 04 问数管线（只开放这一张表）生成并执行 SQL，返回回答、SQL 与结果。
4. 写审计 `ASK_DATA`（`detail_json.source = "axsim"`）。

---

## 10.8 决策下发

| 步骤 | 接口 / 行为 |
| :--- | :--- |
| 提案 | `POST /api/axsim/scenarios/{id}/decisions {title, changes?: [{node_id, metric, target_value}], tolerance_ratio, trigger_flow_id?}`（`SIM_RUN`）。`changes` 缺省时取该场景的干预项（`ABSOLUTE` 取 `value`，`RATIO` / `ADD` 换算为目标值）；预测值也可选入。每项记录 `base_value`（沙盘基线值）。创建 `ax_approvals(source_kind=SIM_APPLY, assignee_roles=['ADMIN','OPERATOR'])`，状态 `PENDING_APPROVAL` |
| 审批 | 复用 Step 07 审批接口；审批人需具备 `SIM_APPLY` |
| 冲突检测 | 审批通过后，在租户事务中 `SELECT ... FOR UPDATE` 锁定目标生产节点，比较当前生产值与 `base_value`：`|current - base| / max(|base|, 1e-9) > tolerance_ratio` 的项写入 `conflict_report`，存在冲突则状态 `CONFLICT`、不写入任何项；用户可 `POST /decisions/{id}/rebase`（以当前生产值为新基值重建提案并重新审批） |
| 写入 | 无冲突时同一事务内：逐项写 `xb_nodes.metrics` 与 `xb_metric_series(source_tag=ACTION)`、`ax_action_log(source_kind=SIM_APPLY)`、外发 `sim.applied {decision_id, scenario_id, node_ids}` 与 `node.metrics_changed`、审计 `SIM_APPLY`；状态 `APPLIED` |
| 后续 | `trigger_flow_id` 不为空时以 `trigger_source=SIM_APPLY`、载荷 `{decision_id, changes}`、幂等键 `decision:<id>` 启动该流程 `PROD` 部署；聚合与规则经 `node.metrics_changed` 自动刷新 |

---

## 10.9 路由（`/api/axsim` 下）

| 方法与路径 | 权限 |
| :--- | :--- |
| `GET /sandboxes`、`POST /sandboxes`、`GET /sandboxes/{id}`、`DELETE /sandboxes/{id}` | `DATA_READ` / `SIM_RUN` |
| `GET /sandboxes/{id}/graph`（合并后的节点与关系，供画布） | `DATA_READ` |
| 覆盖层编辑（§10.2.3） | `SIM_RUN` |
| `POST /sandboxes/{id}/simulate`（仓库已有路径：快速推演，`{actions, params?}`，不创建场景，返回 `SimResult` 并写 `ax_sandboxes.actions/sim_result`） | `SIM_RUN` |
| `GET /sandboxes/{id}/scenarios`、`POST /sandboxes/{id}/scenarios`、`GET /scenarios/{id}`、`POST /scenarios/{id}/rerun`、`DELETE /scenarios/{id}` | `DATA_READ` / `SIM_RUN` |
| `GET /scenarios/{id}/impacts?type_name=&metric=&limit=&cursor=`、`GET /scenarios/{id}/checkpoints` | `DATA_READ` |
| `GET /scenarios/compare`（`?ids=&metrics=&type_name=&briefing=`） | `DATA_READ` |
| `POST /ask` | `SIM_RUN` |
| `POST /scenarios/{id}/decisions`、`GET /decisions/{id}`、`POST /decisions/{id}/rebase` | `SIM_RUN` |

---

## 10.10 前端 `AxSimPage`（`/sim`）

| 组件 | 功能 |
| :--- | :--- |
| `SandboxWizard` | 选择根节点（检索）、深度、方向、类型与关系过滤，显示预计节点数 |
| `OverlayEditor` | 沙盘图（ECharts graph）与表格两种视图，直接编辑节点指标、增删节点与关系，被修改元素以虚线框标识 |
| `ActionList` | 干预列表（节点、指标、方式、值、生效步），传导参数面板 |
| `ImpactTable` / `ImpactGraph` | 影响排序表（前后值、变化率、P10～P90、跳数、路径），图上按变化率着色 |
| `ReplayTimeline` | 检查点播放 |
| `ScenarioCompare` | KPI 对比与节点级差异 |
| `SimAsk` | 推演问数对话框 |
| `DecisionPanel` | 选择下发项、容差、触发流程，显示审批与冲突状态 |

Playwright：`e2e/axsim.spec.ts` 创建沙盘、添加干预 `F-1.output −30%`、运行场景后影响表出现订单行。

---

## 10.11 基准

`scripts/bench/sim-bench.sh`：生成 50 000 节点 / 200 000 关系（含 100 个规模 10～50 的环）的随机图；稳态求解 P95 ≤ 2 秒；`uncertainty.samples = 200` 时 P95 ≤ 20 秒（4 vCPU）。

---

## 10.12 验收

`scripts/seed/10-axsim.sh`：为 `SUPPLIES` 关系类型设置传导规则（`output → inventory`，`RATIO`，系数 `null` 即取关系权重）、`FULFILLS`（`inventory → fulfillment`）；为工厂写 `output`，为订单写 `fulfillment = 1`；创建沙盘 `gate_sandbox`（根 `F-1`，深度 3）。输出 `SANDBOX_ID`。

`scripts/gate/step10.sh`：

```bash
#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "$0")/lib.sh"

(cd axpoint-core && cargo test -p axpoint-domain sim && cargo test -p axpoint-db sim_repo)

PROD_BEFORE=$(sql_app default "SELECT md5(string_agg(id::text || metrics::text, ',' ORDER BY id)) FROM xb_nodes")

# 写时复制：沙盘编辑不影响生产
api PATCH /api/axsim/sandboxes/$SANDBOX_ID/nodes/$WAREHOUSE_NODE_ID '{"metrics":{"inventory":1}}' >/dev/null

# 场景：简写与完整写法等价
S1=$(api POST /api/axsim/sandboxes/$SANDBOX_ID/scenarios "{\"name\":\"s1\",\"actions\":[{\"target_node_id\":\"$FACTORY1_NODE_ID\",\"metric_key\":\"output\",\"delta_ratio\":-0.3}],\"params\":{}}" | jq -r .id)
S2=$(api POST /api/axsim/sandboxes/$SANDBOX_ID/scenarios "{\"name\":\"s2\",\"actions\":[{\"target_node_id\":\"$FACTORY1_NODE_ID\",\"metric_key\":\"output\",\"mode\":\"RATIO\",\"value\":-0.3}],\"params\":{}}" | jq -r .id)
wait_scenario "$S1"; wait_scenario "$S2"
expect_eq "shorthand-equal" "$(api GET "/api/axsim/scenarios/$S1/impacts?limit=500" | jq -c '[.items[] | [.node_id,.metric,.after_val]] | sort')" \
                            "$(api GET "/api/axsim/scenarios/$S2/impacts?limit=500" | jq -c '[.items[] | [.node_id,.metric,.after_val]] | sort')"
expect_json "propagated" "$(api GET "/api/axsim/scenarios/$S1/impacts?metric=fulfillment&limit=5")" '(.items | length) > 0 and .items[0].hop_distance >= 2'

# 缺失指标不使用默认基值
expect_json "missing-metric" "$(api POST /api/axsim/sandboxes/$SANDBOX_ID/simulate "{\"actions\":[{\"target_node_id\":\"$FACTORY1_NODE_ID\",\"metric_key\":\"no_such_metric\",\"delta_ratio\":-0.5}]}")" '.error_code == "INVALID_ARGUMENT"'

# 对比与回放
expect_json "compare" "$(api GET "/api/axsim/scenarios/compare?ids=$S1,$S2&briefing=true")" '(.kpis | length) > 0 and (.briefing.recommended_scenario_id as $r | ["'"$S1"'","'"$S2"'"] | index($r) != null)'
S3=$(api POST /api/axsim/sandboxes/$SANDBOX_ID/scenarios "{\"name\":\"s3\",\"actions\":[{\"target_node_id\":\"$FACTORY1_NODE_ID\",\"metric_key\":\"output\",\"delta_ratio\":-0.3}],\"params\":{\"mode\":\"STEPPED\",\"horizon_steps\":6}}" | jq -r .id)
wait_scenario "$S3"
expect_json "checkpoints" "$(api GET /api/axsim/scenarios/$S3/checkpoints)" '(.items | length) == 7'

# 生产未被修改
expect_eq "prod-untouched" "$(sql_app default "SELECT md5(string_agg(id::text || metrics::text, ',' ORDER BY id)) FROM xb_nodes")" "$PROD_BEFORE"

# 决策下发：审批 → 写入 → 冲突检测
DEC=$(api POST /api/axsim/scenarios/$S1/decisions '{"title":"下调 F-1 产能","tolerance_ratio":0}' | jq -r .id)
APR=$(api GET /api/axsim/decisions/$DEC | jq -r .approval_id)
api POST /api/axflow/approvals/$APR/decide '{"decision":"APPROVE"}' >/dev/null
wait_until "applied" 60 "api GET /api/axsim/decisions/$DEC | jq -e '.status == \"APPLIED\"'"
expect_eq "audit-sim-apply" "$(sql_app default "SELECT count(*) > 0 FROM sec_audit_logs WHERE action='SIM_APPLY'")" "t"
DEC2=$(api POST /api/axsim/scenarios/$S1/decisions '{"title":"重复下发","tolerance_ratio":0}' | jq -r .id)
APR2=$(api GET /api/axsim/decisions/$DEC2 | jq -r .approval_id)
api POST /api/axflow/approvals/$APR2/decide '{"decision":"APPROVE"}' >/dev/null
wait_until "conflict" 60 "api GET /api/axsim/decisions/$DEC2 | jq -e '.status == \"CONFLICT\"'"

gate_pass step10
```

`wait_scenario` 由 `lib.sh` 提供（轮询场景状态至 `SUCCEEDED`，失败则门禁失败）。第二次下发因第一次已改变生产值（与沙盘基线不同）而判定冲突。

**通过标准**：最后一行 `GATE step10 PASS`；§10.4 正确性测试全部通过；基准环境中 §10.11 断言成立。
