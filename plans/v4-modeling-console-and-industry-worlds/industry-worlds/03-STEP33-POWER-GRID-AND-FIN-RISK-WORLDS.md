# 03 · Step 33：新型电力配网调度（`power_grid`）与集团授信互保风控（`fin_risk`）行业世界

本步作为 Phase 4 跨行业仿真世界底座（Track B）的第一路并行工程，构建两套高复杂度、高密度的真实产业可执行模型与大规模仿真数据：
1. **`power_grid`（新型电力与配网调度）**：重点展现**空间多边形供电围栏（`geo_shape`）+ 输配电馈线边时序指标与四路径（`V↔E`）过载传导 + 43,200 点 15 分钟级负荷时序 + 主变跳闸 `deadline_step` 最晚干预反推**；
2. **`fin_risk`（集团授信与互保风控）**：重点展现**1,200+ 节点与 4,200+ 持股/互保/资金边的高密度图算法（PageRank / 连通分量 / 社区发现）+ 60,000 行对公流水湖表驱动 `@visactor/react-vtable` 多维透视 + 实际控制人三道强制 PII 脱敏 + 逾期状态驱动动态行级视野（`STATE_DRIVEN`）**。

总览索引与工程约束见 [00-V4-MASTER-INDEX.md](../00-V4-MASTER-INDEX.md)、[IRON_RULES.md](../../../IRON_RULES.md) 与 [A1-CONVENTIONS-ENUMS-ROUTES.md](../../../docs/standards/A1-CONVENTIONS-ENUMS-ROUTES.md)。

---

## 33.0 目标、前置与契约

- **目标**：
  1. 交付完整可复用行业模板包 `templates/packs/power_grid/` 与 `templates/packs/fin_risk/`（含 `manifest.json`、三层结构 `schema.json`、语义指标、告警规则、五区数字看板定义与黄金验收问题集）。
  2. 交付确定性向量化数据生成脚本 `axpoint-parser/scripts/gen_worlds_grid_fin.py` 与装载子脚本 `scripts/seed/showcase/{power_grid.sh,fin_risk.sh}`（固定随机种子 `seed = 202604`，零外部网络依赖，单世界灌装耗时 `< 3s`）。
  3. 两个行业子图均通过 `/api/xblue/subgraphs/{slug}/certify` 达成 **`L4_CERTIFIED`**（`dead_knowledge_cnt == 0`、`trial_tick_passed == true`）。
- **前置**：L19 契约 PR 已合入（无新增数据库迁移）。
- **覆盖能力项**：
  - `C95`：`step33` `"power-grid-l4-certified"`、`"power-grid-feeder-deadline"`
  - `C96`：`step33` `"fin-risk-60k-lake-rows"`、`"fin-risk-pii-and-dynamic-rls"`

### 文件变更清单

| 负责角色 | 动作 | 路径 | 内容 |
| :--- | :--- | :--- | :--- |
| `squad-domain-agent` | 新建 | `templates/packs/power_grid/{manifest.json,schema.json,dashboards.json,rules.json,competency.json}` | 新型电力与配网调度模板包（6 类对象、4 类关系边、2 个 `ActionSpec`、五区配网看板） |
| `squad-domain-agent` | 新建 | `templates/packs/fin_risk/{manifest.json,schema.json,dashboards.json,rules.json,competency.json}` | 集团授信与互保风控模板包（5 类对象、4 类关系边、PII 标注、五区风控看板） |
| `squad-data` | 新建 | `axpoint-parser/scripts/gen_worlds_grid_fin.py` | 使用 DuckDB / PyArrow 向量化生成 `grid_feeder_telemetry`（43,200 行）与 `fin_corp_wire_transfers`（60,000 行）Parquet 及批量节点/边 JSONL |
| `squad-qa` | 新建 | `scripts/seed/showcase/{power_grid.sh,fin_risk.sh}`、`scripts/gate/step33.sh` | 幂等灌装子图、空间围栏、边时序、动态行级视野策略与预置推演场景，执行全套门禁核验 |

---

## 33.1 行业世界一：`power_grid`（新型电力与配网调度）

### 33.1.1 业务模型结构（4 业务域 × 三层对象分层）

| 对象类型 (`name`) | 中文名 | 业务域 (`business_domain`) | 语义分层 (`semantic_layer_tier`) | 核心属性与时序指标 | 空间几何与动作契约 |
| :--- | :--- | :--- | :--- | :--- | :--- |
| `PowerPlant` | 发电场站 | `电源域` | `STATIC_FACT` | 属性：`plant_kind`（火电/风电/光伏/储能）、`rated_mw`；指标：`generation_mw`、`Spinning_reserve_mw` | 经纬度点坐标；动作 `dispatch_ess_discharge`（储能顶峰放电） |
| `Substation` | 变电站 | `输变电域` | `STATIC_FACT` | 属性：`voltage_kv`（`220` / `110`）、`grid_zone`；指标：`active_load_mw`、`load_ratio`（派生：`active_load_mw / rated_capacity_mw`）、`bus_voltage_pu` | **多边形供电围栏 `geo_shape`（`Polygon`）** |
| `DistTransformer` | 台区配变 | `配电域` | `STATIC_FACT` | 属性：`feeder_code`、`capacity_kva`；指标：`demand_mw`、`oil_temp_c`、`overload_risk` | 经纬度点坐标；动作 `shed_noncritical_load`（可中断负荷柔性削减） |
| `CriticalLoad` | 重点负荷用户 | `用电域` | `STATIC_FACT` | 属性：`load_category`（三甲医院/晶圆厂/交通枢纽/商业综合体）、`interruptible`；指标：`power_draw_mw`、`backup_hours` | 位于对应 `Substation` 供电多边形内 |
| `SwitchOrder` | 倒闸与跳闸事件 | `输变电域` | `EVENT_DYNAMIC` | 属性：`event_type`、`operator_team`；指标：`tripped_mw` | 挂接于 `Substation` 从属事件抽屉 |
| `GridZoneSummary` | 供电分区负荷汇总 | `用电域` | `CUBE_ROLLUP` | 指标：`zone_total_load_mw`、`peak_margin_ratio` | 挂接于 `Substation` 汇总层 |

### 33.1.2 四路径动力学与沙盘推演设计
1. **关系边与边时序指标**：
   - `SUPPLIES_SUBSTATION`（`PowerPlant -> Substation`）
   - `FEEDS_TRANSFORMER`（`Substation -> DistTransformer`，携带边指标 `current_amp`、`line_load_ratio`、`line_loss_mw`；配置 `V→E` 边派生规则由上下游功率差推算 `line_load_ratio`，配置 `E→V` 规则向台区传导电压跌落与过载风险）
   - `SERVES_LOAD`（`DistTransformer -> CriticalLoad`）
2. **数据体量**：**324 个业务节点**（18 座场站、26 座变电站含供电多边形围栏、160 台配变、90 户重点负荷、20 条倒闸事件、10 个分区汇总）、**548 条关系边**、**43,200 条 15 分钟级负荷与馈线电流时序点**。
3. **预置推演场景（`迎峰度夏城北 220kV 主变跳闸推演`）**：
   - 定时事件 `step = 3` 主变跳闸减供 `-120 MW` + 条件事件 `load_ratio >= 0.92` 自动投入站内储能 `+35 MW`；
   - 红线目标约束：供电多边形围栏（`GEO_WITHIN_POLYGON`）内重点负荷母线电压 `bus_voltage_pu >= 0.93`，精确反推 `breach_step` 与最晚切负荷干预窗口 `deadline_step`。

---

## 33.2 行业世界二：`fin_risk`（集团授信与互保风控）

### 33.2.1 业务模型结构与三道 PII 脱敏设计

| 对象类型 (`name`) | 中文名 | 业务域 | 语义分层 | 核心属性、PII 分类与指标 |
| :--- | :--- | :--- | :--- | :--- |
| `CorpGroup` | 集团系主体 | `集团授信域` | `STATIC_FACT` | 属性：`industry`、`credit_rating`；指标：`group_total_exposure`、`contagion_score` |
| `LegalEntity` | 关联法人企业 | `集团授信域` | `STATIC_FACT` | 属性：`unified_code`、`branch_dept`（所属分行部门）、`reg_capital`；指标：`loan_balance`、`overdue_amount`、`overdue_ratio`（派生）、`guarantee_out_total`（沿 `CROSS_GUARANTEES` 聚合） |
| `BeneficialOwner` | 实际控制人 | `穿透核查域` | `STATIC_FACT` | **PII 属性**：`full_name`（`NAME`）、`mobile_phone`（`PHONE`）、`national_id`（`ID_CARD`）、`settlement_card`（`BANK_CARD`）；指标：`controlled_exposure` |
| `CreditFacility` | 授信借据明细 | `信贷台账域` | `EVENT_DYNAMIC` | 属性：`product_kind`、`maturity_date`；指标：`principal_amt`、`days_past_due` |
| `IndustryExposureCube` | 行业敞口汇总 | `风险监测域` | `CUBE_ROLLUP` | 指标：`sector_exposure_sum`、`npl_ratio` |

### 33.2.2 图算法、6 万行 VTable 透视与动态行级视野
1. **数据体量**：**1,210 个节点**（40 个集团系、620 家关联法人、180 名实控人、350 笔大额借据、20 个行业汇总）、**4,260 条关系边**（`CONTROLS`、`HOLDS_EQUITY`、`CROSS_GUARANTEES` 含互保环形团伙、`TRANSFERS_FUNDS`），以及入湖数据集 **`fin_corp_wire_transfers`（60,000 行对公资金划转流水）**。
2. **状态驱动动态行级视野（`STATE_DRIVEN`）**：
   - 在 `sec_row_policies` 为 `LegalEntity` 注册动态策略：`properties.branch_dept == {{user.dept}} OR metrics.overdue_ratio >= 0.05`；
   - 常态下异地分行风控员仅可见本分行授信企业；一旦关联担保圈内某异地法人逾期率突破 `5%`，该风险企业自动进入跨分行联防视野。
3. **五区风控数字看板（`fin-risk-cockpit`）**：预置总敞口/不良率火花线 KPI、60,000 行流水多维 VTable 透视表（开启数据条与热力色阶）、互保传染图谱拓扑与一键冻结授信额度 `ActionSpec` 按钮。

---

## 33.3 可执行门禁（`scripts/gate/step33.sh`）

```bash
#!/usr/bin/env bash
# Step 33 Gate：power_grid 与 fin_risk 双行业世界装载、L4 认证、6 万行湖表、PII 脱敏与动态视野验收
set -euo pipefail
source "$(dirname "$0")/lib.sh"

python3 "$ROOT/scripts/python-guard.py"
bash "$ROOT/scripts/seed/showcase/power_grid.sh"
bash "$ROOT/scripts/seed/showcase/fin_risk.sh"

# 1. C95：验证 power_grid L4_CERTIFIED、空间多边形围栏命中与主变跳闸 deadline_step
CERT_PG=$(api POST /api/xblue/subgraphs/power_grid/certify '{}')
expect_json "power-grid-l4-certified" "$CERT_PG" '
  .cert_level == "L4_CERTIFIED" and .dead_knowledge_cnt == 0 and .trial_tick_passed == true
'
PG_SCENARIOS=$(api GET "/api/axsim/sandboxes/${POWER_GRID_SANDBOX_ID}/scenarios")
expect_json "power-grid-feeder-deadline" "$PG_SCENARIOS" '
  (.items | length) >= 1 and (.items[0].deadline_report.earliest_deadline_step != null)
'

# 2. C96：验证 fin_risk L4_CERTIFIED、60,000 行对公流水湖表、实控人 PII 脱敏与动态视野
CERT_FIN=$(api POST /api/xblue/subgraphs/fin_risk/certify '{}')
expect_json "fin-risk-l4-certified" "$CERT_FIN" '.cert_level == "L4_CERTIFIED" and .dead_knowledge_cnt == 0'

WIRE_CNT=$(api POST /api/analytics/sql/run '{"sql":"SELECT COUNT(*) AS cnt FROM fin_corp_wire_transfers"}')
expect_json "fin-risk-60k-lake-rows" "$WIRE_CNT" '.rows[0][0] >= 60000'

# 验证无 PII_RAW_READ 角色查询 BeneficialOwner 自动掩码手机号与身份证
ANALYST_NODES=$(api_as "$ANALYST_TOKEN" GET "/api/xblue/nodes?subgraph=fin_risk&type_name=BeneficialOwner&limit=5")
expect_json "fin-risk-pii-and-dynamic-rls" "$ANALYST_NODES" '
  (.items | length) >= 1
  and ((.items[0].properties.mobile_phone | tostring) | contains("*"))
  and ((.items[0].properties.national_id | tostring) | contains("*"))
'

gate_pass step33
```
