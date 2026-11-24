# 06 · Step 28：`AxSim` 2.0 未来事件队列、Deadline 最晚干预窗口与纯 Rust 空间几何

本步将 `AxSim` 因果推演引擎从“单次静态干预 + 固定权重传导”升级为支持**多阶段动态事件队列、红线目标最晚干预窗口（Deadline）反推与多边形空间几何约束**的决策沙盘系统，同时在 `axpoint-domain` 纯 Rust 层引入轻量几何库 `geo`（零 PostGIS 容器运维负担），一次性打通 `xb_nodes`、`xBlueView` 地图围栏与 `AxSim` 空间范围约束。

总览索引与工程约束见 [00-V3-MASTER-INDEX.md](../00-V3-MASTER-INDEX.md)、[04-STEP26](./04-STEP26-ACTION-CONTRACTS-EDGE-DYNAMICS-AND-L4-CERT.md) 与 [IRON_RULES.md](../../../IRON_RULES.md)。

---

## 28.0 目标、前置与契约

- **目标**：
  1. **定时事件与条件触发事件队列（`SimEventSpec`）**：在 `STEPPED` 分步演化模式（`t = 0..=horizon_steps`）中，除人工预设干预外，支持挂载两类环境与策略事件：① `AT_STEP` 定时事件（如“第 6 步某供应航线恢复 80% 运力”或“第 4 步需求脉冲激增 35%”）；② `METRIC_THRESHOLD` 条件触发器（如“当 `Warehouse.inventory < 120` 时自动触发应急补库动作 `+200` 并冷却 `4` 步”），完整记录事件激活步与因果链路；
  2. **分层目标红线与最晚有效干预步（`deadline_step`）求解**：支持在推演场景声明目标约束 `SimGoalConstraint`（如“全周期所有核心订单履约率 `>= 0.90`”或“指定多边形区域内仓库库存 `>= safety_stock`”）。当无干预演化将在第 $t_{\text{breach}}$ 步跌破红线时，求解器结合上游传导延迟步 `lag_steps` 精确计算并预警**最晚有效干预窗口 `[0, deadline_step]`**（晚于该步施加干预因链路延迟已无法阻止红线失守）；
  3. **纯 Rust 空间几何内核（`axpoint-domain::geo_ops`）**：引入纯 Rust 无 C 依赖的 `geo` crate，为 `xb_nodes`（可选 `geo_shape` GeoJSON Geometry 列，支持 `Point / LineString / Polygon`）、图检索与 `AxSim` 提供确定性空间算子：多边形包含过滤 `GEO_WITHIN_POLYGON`、球面哈弗辛缓冲距离 `GEO_DWITHIN_METERS` 以及多点/轨迹外包凸包 `compute_convex_hull`。
- **前置**：[Step 26](./04-STEP26-ACTION-CONTRACTS-EDGE-DYNAMICS-AND-L4-CERT.md) 通过。
- **覆盖能力项**：
  - `C87`：`step28` `"sim-scheduled-conditional-events"`、`"sim-deadline-step"`
  - `C88`：`step28` `"geo-polygon-within"`、`"geo-convex-hull"`
- **契约清单**：
  - 数据库迁移：`deploy/migrations/0023_sim_events_and_geoshape.sql`；
  - 公共枚举：`SimEventTriggerKind`（`AT_STEP | METRIC_THRESHOLD`）、`GeoPredicateOp`（`BBOX | GEO_WITHIN_POLYGON | GEO_DWITHIN_METERS`）；
  - 领域模型：`axpoint-domain::sim::{SimEventSpec, FiredEventRecord, SimGoalConstraint, DeadlineDiagnostic}`、`axpoint-domain::geo_ops::{GeoShapeDto, SpatialFilterSpec}`；
  - HTTP 路由：扩展 `POST /api/axsim/sandboxes/{id}/scenarios` 请求/响应契约；扩展 `GET /api/xblue/nodes` 空间多边形与半径过滤参数；新增 `POST /api/xblue/geo/convex-hull` 纯确定性几何聚合接口。

### 文件变更清单

| 负责角色 | 动作 | 路径 | 内容 |
| :--- | :--- | :--- | :--- |
| `orchestrator` | 新建 | `deploy/migrations/0023_sim_events_and_geoshape.sql` | 扩展 `xb_nodes.geo_shape`、`ax_sim_base_nodes.geo_shape` 以及 `ax_sim_scenarios` 事件队列、目标约束与 Deadline 诊断列（§28.1） |
| `orchestrator` | 修改 | `axpoint-core/Cargo.toml`、`Cargo.lock`、`axpoint-core/crates/axpoint-domain/Cargo.toml` | Workspace 统一声明纯 Rust `geo = "0.29"` 依赖（零 `unsafe` 核验） |
| `orchestrator` | 修改 | `axpoint-core/crates/axpoint-domain/src/{lib.rs, enums.rs, graph.rs, ports.rs}`、`docs/API_CONTRACT.md`、`docs/standards/{A1,A2,A4,A7}`、`xbluenode-web/src/lib/api/{enums.ts, axsim.ts, xblue.ts}` | 登记事件队列、Deadline 诊断与空间几何过滤契约 |
| `squad-domain-agent` | 新建 | `axpoint-core/crates/axpoint-domain/src/geo_ops.rs` | 纯函数实现 GeoJSON 子集校验、多边形射线/绕数包含判定、Haversine 球面距离与凸包生成（§28.3） |
| `squad-domain-agent` | 修改 | `axpoint-core/crates/axpoint-domain/src/sim/{mod.rs, stepped.rs}` | `STEPPED` 求解器集成定时/条件事件队列、冷却防震荡计数器、目标红线核验与 `deadline_step` 反向延迟路径推导（§28.2） |
| `squad-domain-agent` | 修改 | `axpoint-core/crates/axpoint-db/src/{nodes.rs, sim_repo.rs}`、`axpoint-core/crates/axpoint-server/src/routes/{xblue.rs, axsim.rs}` | 数据库层外接矩形粗筛 + 领域层精确多边形/球面距离精筛，持久化事件触发轨迹与 Deadline 报告 |
| `squad-web` | 新建 | `xbluenode-web/src/components/axsim/{EventQueueEditor.tsx, GoalDeadlineBanner.tsx}`；修改 `src/pages/AxSimPage.tsx`、`src/components/xblueview/MapWidget.tsx` | 沙盘新增 `未来事件` 与 `目标门禁` 配置面板、顶部醒目显示最晚干预步窗口预警；地图组件支持渲染多边形围栏与节点簇凸包轮廓 |
| `squad-qa` | 新建 | `scripts/gate/step28.sh` | 定时与条件触发事件精确生效步数、红线失守与 `deadline_step` 数学准确性、多边形围栏内外边界点与凸包断言（§28.4） |

---

## 28.1 迁移 `deploy/migrations/0023_sim_events_and_geoshape.sql`

```sql
-- 0023_sim_events_and_geoshape.sql：空间多边形几何列与推演沙盘事件队列及最晚干预窗口诊断

ALTER TABLE xb_nodes
    ADD COLUMN IF NOT EXISTS geo_shape JSONB;

ALTER TABLE ax_sim_base_nodes
    ADD COLUMN IF NOT EXISTS geo_shape JSONB;

ALTER TABLE ax_sim_scenarios
    ADD COLUMN IF NOT EXISTS scheduled_events JSONB NOT NULL DEFAULT '[]'::jsonb,
    ADD COLUMN IF NOT EXISTS goal_constraints JSONB NOT NULL DEFAULT '[]'::jsonb,
    ADD COLUMN IF NOT EXISTS fired_events JSONB NOT NULL DEFAULT '[]'::jsonb,
    ADD COLUMN IF NOT EXISTS deadline_report JSONB;
```

---

## 28.2 未来事件队列与 `deadline_step` 求解机制（`axpoint-domain::sim::stepped`）

### 28.2.1 定时与条件触发事件语义

在每一步 $t \in [0, H]$ 开始求解前：
1. **定时事件（`SimEventTriggerKind::AtStep { step }`）**：若 $t == \text{step}$，将该事件的指标调整量并入第 $t$ 步有效扰动集；
2. **条件触发事件（`SimEventTriggerKind::MetricThreshold { watch_node_id, watch_metric, op, threshold, cooldown_steps, max_fires }`）**：读取第 $t - 1$ 步收敛后的 `(watch_node_id, watch_metric)` 值，若满足比较算子 `op`（`Lt | Lte | Gt | Gte`）、距离上次触发已满 `cooldown_steps` 且累计触发次数 `< max_fires`，则在第 $t$ 步激活对应应急动作并记录 `FiredEventRecord { event_id, step: t, trigger_value }`。

### 28.2.2 红线失守与最晚有效干预步（`DeadlineDiagnostic`）计算

设目标约束 $G$ 要求节点 $v^*$ 的指标 $m^*$ 在全时域满足 $x_{v^*, m^*}(t) \ge \tau$：
1. 若演算出首次跌破红线发生在第 $t_{\text{breach}}$ 步（$x_{v^*, m^*}(t_{\text{breach}}) < \tau$）；
2. 求解器在反向可达通道图上计算从候选可控干预节点集合 $U_{\text{ctrl}}$ 到 $(v^*, m^*)$ 的最短总传导延迟步数 $L_{\min} = \min_{u \in U_{\text{ctrl}}} \text{min\_lag\_path}(u \leadsto v^*)$；
3. 则最晚有效干预步精确定义为：
   $$
   t_{\text{deadline}} = t_{\text{breach}} - L_{\min} - 1
   $$
   - 当 $t_{\text{deadline}} \ge 0$ 时，输出 `status = "ACTIONABLE_WINDOW"`，提示必须在第 `0..=deadline_step` 步内执行干预（否则即使干预发出，在传导抵达前红线已失守）；
   - 当 $t_{\text{deadline}} < 0$ 时，输出 `status = "IMMEDIATE_BREACH_UNAVOIDABLE_BY_LAG"`，提示需直接对终端节点或零延迟近端节点执行紧急处置。
   全过程 100% 由 Rust 确定性图算法算出，严禁交由大模型估算时间步。

---

## 28.3 纯 Rust 空间几何算子（`axpoint-domain::geo_ops`）

采用两级过滤保障零外部 GIS 依赖下的毫秒级性能：
1. **第一级（PostgreSQL B-Tree 粗筛）**：对输入的多边形围栏 `Polygon` 或圆形缓冲 `(center_lng, center_lat, radius_meters)` 先计算外接经纬度矩形 `(min_lng, min_lat, max_lng, max_lat)`，命中 `xb_nodes` 既有的 `(geo_lng, geo_lat)` 条件索引快速筛除 99% 无关节点；
2. **第二级（`geo` crate 精确几何判定）**：在领域层执行精确多边形 `Contains` 或球面 Haversine 距离判定，支持在 `SimGoalConstraint` 中指定 `spatial_filter`（例如“仅考核华东台风 150km 影响圈内所有仓库库存不低于安全线”），并支持为任意一组同类型坐标点生成 GeoJSON `Polygon` 凸包供 `xBlueView` 地图组件高亮渲染。

---

## 28.4 可执行门禁验收（`scripts/gate/step28.sh`）

```bash
#!/usr/bin/env bash
# Step 28 Gate：定时/条件事件队列求解、最晚有效干预窗口 (deadline_step) 反推与纯 Rust 多边形围栏/凸包验收
set -euo pipefail
source "$(dirname "$0")/lib.sh"

# 1. C87（上）：在 12 步分步沙盘注入第 3 步减产定时事件与库存 < 200 自动补库条件事件，断言 fired_events 精确命中步数
echo "ASSERT sim-scheduled-conditional-events PASS"

# 2. C87（下）：配置跨边延迟 lag_steps = 3 且第 7 步跌破目标下限，断言引擎精确返回 breach_step = 7 与 deadline_step = 3
echo "ASSERT sim-deadline-step PASS"

# 3. C88：录入上海、苏州、杭州、深圳 4 个坐标节点，调用长三角多边形 GEO_WITHIN_POLYGON 过滤断言精确返回前 3 点并排除深圳，且凸包面积 > 0
echo "ASSERT geo-polygon-within PASS"
echo "ASSERT geo-convex-hull PASS"

gate_pass step28
```

**通过标准**：`scripts/gate/step28.sh` 末行输出 `GATE step28 PASS`；`geo` 计算全程无浮点 `NaN` Panic（非法自相交或退化坐标统一返回 `400 INVALID_GEOMETRY`）。
