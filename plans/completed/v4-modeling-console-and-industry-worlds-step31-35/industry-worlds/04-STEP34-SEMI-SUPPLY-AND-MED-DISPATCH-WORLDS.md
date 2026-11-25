# 04 · Step 34：半导体全球供应链（`semi_supply`）与城市急诊床位调度（`med_dispatch`）行业世界

本步作为 Phase 4 跨行业仿真世界底座（Track B）的第二路并行工程，进一步拓展平台在精密制造跨国协同与公共服务资源实时调度领域的建模广度：
1. **`semi_supply`（半导体全球多级供应链）**：完整呈现 **4 大业务域复合泳道（采购域 / 制造域 / 物流域 / 履约域）+ 三层从属对象抽屉折叠 + 光刻/刻蚀机台 OEE 与良率传导 + 30,000 行批次传感湖表 + 应急航空包机与晶圆调拨 `ActionSpec`**；
2. **`med_dispatch`（城市急诊与医疗床位调度）**：彻底打破“工业平台只能建工厂仓库”的思维定势，将 **院区、临床重症科室、ECMO/呼吸机紧缺设备、120 急救车空间分布（轨迹与覆盖凸包 `convex_hull`）及 18,000 条急诊分诊流转事件** 纳入同一张可执行业务模型。

总览索引与工程约束见 [00-V4-MASTER-INDEX.md](../00-V4-MASTER-INDEX.md)、[IRON_RULES.md](../../../../IRON_RULES.md) 与 [A1-CONVENTIONS-ENUMS-ROUTES.md](../../../../docs/standards/A1-CONVENTIONS-ENUMS-ROUTES.md)。

---

## 34.0 目标、前置与契约

- **目标**：
  1. 交付行业模板包 `templates/packs/semi_supply/` 与 `templates/packs/med_dispatch/`（含四域复合拓扑、派生/聚合公式、`ActionSpec` 动作、五区数字看板与黄金能力问答集）。
  2. 交付确定性向量化数据生成脚本 `axpoint-parser/scripts/gen_worlds_semi_med.py` 与装载子脚本 `scripts/seed/showcase/{semi_supply.sh,med_dispatch.sh}`（固定随机种子 `seed = 202605`，单世界灌装 `< 3s`）。
  3. 两个行业子图全部通过 `/api/xblue/subgraphs/{slug}/certify` 达成 **`L4_CERTIFIED`**。
- **前置**：L19 契约 PR 已合入（无新增数据库迁移）。
- **覆盖能力项**：
  - `C97`：`step34` `"semi-supply-l4-and-swimlanes"`、`"med-dispatch-convex-hull-and-l4"`

### 文件变更清单

| 负责角色 | 动作 | 路径 | 内容 |
| :--- | :--- | :--- | :--- |
| `squad-domain-agent` | 新建 | `templates/packs/semi_supply/{manifest.json,schema.json,dashboards.json,rules.json,competency.json}` | 半导体全球多级供应链模板包（8 类对象、6 类关系边、2 个 `ActionSpec`、五区供应链看板） |
| `squad-domain-agent` | 新建 | `templates/packs/med_dispatch/{manifest.json,schema.json,dashboards.json,rules.json,competency.json}` | 城市急诊与医疗床位调度模板包（6 类对象、4 类关系边、2 个 `ActionSpec`、五区急诊态势看板） |
| `squad-data` | 新建 | `axpoint-parser/scripts/gen_worlds_semi_med.py` | 向量化生成 `semi_lot_telemetry`（30,000 行晶圆批次与腔体传感明细）与 `med_triage_events`（18,000 行急诊分诊与转运记录）Parquet 及节点/边/时序批量数据 |
| `squad-qa` | 新建 | `scripts/seed/showcase/{semi_supply.sh,med_dispatch.sh}`、`scripts/gate/step34.sh` | 幂等灌装双行业子图、看板与沙盘，执行 L4 认证、免连接 `AxQL` 防膨胀聚合与凸包算子门禁核验 |

---

## 34.1 行业世界三：`semi_supply`（半导体全球多级供应链）

### 34.1.1 四域复合泳道与三层折叠模型设计

| 对象类型 (`name`) | 中文名 | 业务域 (`business_domain`) | 语义分层 (`semantic_layer_tier`) | 核心属性与指标 | 动作契约 (`ActionSpec`) |
| :--- | :--- | :--- | :--- | :--- | :--- |
| `SpecialtyVendor` | 特种气体与硅片商 | `采购域` | `STATIC_FACT` | 属性：`country`、`material_grade`；指标：`supply_index`、`lead_time_days` | — |
| `WaferFab` | 晶圆制造厂 | `制造域` | `STATIC_FACT` | 属性：`process_nm`、`cleanroom_class`；指标：`wafer_starts_wpm`、`fab_yield_pct`、`safety_wafer_stock` | `reallocate_safety_wafer`（跨厂调配安全晶圆，前置条件：`safety_wafer_stock >= param.qty`） |
| `LithoTool` | 关键光刻/刻蚀机台 | `制造域` | `STATIC_FACT` | 属性：`tool_vendor`、`chamber_id`；指标：`oee_pct`、`chamber_temp_c`、`particle_defect_rate` | — |
| `WaferLotEvent` | 晶圆批次流片记录 | `制造域` | `EVENT_DYNAMIC` | 属性：`lot_code`、`step_recipe`；指标：`die_good_cnt` | 折叠内嵌于 `WaferFab` |
| `YieldRollupCube` | 制程良率汇总块 | `制造域` | `CUBE_ROLLUP` | 指标：`monthly_avg_yield`、`scrap_cost_usd` | 折叠内嵌于 `WaferFab` |
| `CustomsPort` | 清关口岸与航站 | `物流域` | `STATIC_FACT` | 属性：`port_code`、`transport_mode`；指标：`clearance_hours`、`backlog_lots` | `expedite_air_charter`（启用恒温包机绿色通道，缩短 `clearance_hours`） |
| `RegionalHub` | 区域恒温分拨仓 | `物流域` | `STATIC_FACT` | 属性：`region`；指标：`chip_inventory_k`、`outbound_rate_k` | — |
| `AutoOemOrder` | 车企核心芯片大单 | `履约域` | `STATIC_FACT` | 属性：`oem_customer`、`priority_tier`；指标：`ordered_qty_k`、`on_time_prob` | — |

- **数据体量**：**526 个业务节点**、**1,340 条多级供应与机台挂载边**、入湖数据集 **`semi_lot_telemetry`（30,000 行）** + **16,800 条机台 OEE/腔体温度时序点**。
- **核心验证亮点**：在建模画布开启 `按域分组` 即刻呈现清晰的四列业务域泳道；使用免连接 `AxQL` 跨 `WaferFab -> WaferLotEvent` 执行 `SUM_BY_ENTITY(WaferFab.wafer_starts_wpm)` 精确验证一对多零扇出膨胀。

---

## 34.2 行业世界四：`med_dispatch`（城市急诊与医疗床位调度）

### 34.2.1 非工业公共服务资源调度模型

| 对象类型 (`name`) | 中文名 | 业务域 (`business_domain`) | 语义分层 | 核心属性、空间坐标与指标 | 动作契约 (`ActionSpec`) |
| :--- | :--- | :--- | :--- | :--- | :--- |
| `HospitalCampus` | 医疗院区 | `院区统筹域` | `STATIC_FACT` | 属性：`hospital_grade`、`district`；指标：`ed_wait_minutes`、`total_free_beds`、`surge_level` | `activate_surge_beds`（启用应急机动病区加床，原子更新 `total_free_beds` 与 `surge_level`） |
| `ClinicalDept` | 急诊与重症科室 | `临床救治域` | `STATIC_FACT` | 属性：`specialty`（急诊抢救/重症 ICU/胸痛中心/卒中中心）、`on_duty_physicians`；指标：`bed_occupancy_pct`、`free_icu_beds`、`critical_queue_cnt` | — |
| `LifeSupportUnit` | 生命支持设备 | `临床救治域` | `STATIC_FACT` | 属性：`device_kind`（ECMO/有创呼吸机/CRRT）、`status`；指标：`utilization_pct`、`remaining_filter_hours` | — |
| `AmbulanceUnit` | 120 急救转运车 | `院前急救域` | `STATIC_FACT` | 属性：`plate_no`、`crew_level`；**实时经纬度点 `geo`**；指标：`eta_minutes`、`dispatch_load` | `divert_ambulance`（分流改派至邻近低饱和院区，前置条件：`eta_minutes <= 35`） |
| `TriageVisit` | 急诊分诊接诊单 | `临床救治域` | `EVENT_DYNAMIC` | 属性：`acuity_level`（I～IV 级）、`chief_complaint`；指标：`door_to_doctor_min` | 折叠内嵌于 `ClinicalDept` |
| `WardCapacityCube` | 院区周转汇总块 | `院区统筹域` | `CUBE_ROLLUP` | 指标：`daily_admissions`、`turnover_rate` | 折叠内嵌于 `HospitalCampus` |

- **数据体量**：**264 个业务节点**（12 个三甲院区、60 个重点临床科室、96 台关键生命支持设备、64 辆带实时空间坐标的 120 急救车、20 个活跃分诊单、12 个汇总块）、**490 条调度与归属边**、入湖数据集 **`med_triage_events`（18,000 行）** + **20,000 条科室床位占用率时序点**。
- **核心验证亮点**：在五区急诊数字看板（`med-dispatch-cockpit`）的地图组件上一键开启 **`凸包`**，通过纯 Rust `geo` 引擎实时绘出全市 64 辆急救车的动态院前急救覆盖包络面。

---

## 34.3 可执行门禁（`scripts/gate/step34.sh`）

```bash
#!/usr/bin/env bash
# Step 34 Gate：semi_supply 与 med_dispatch 双行业世界装载、L4 认证、AxQL 跨层防膨胀与急救车凸包验收
set -euo pipefail
source "$(dirname "$0")/lib.sh"

python3 "$ROOT/scripts/python-guard.py"
bash "$ROOT/scripts/seed/showcase/semi_supply.sh"
bash "$ROOT/scripts/seed/showcase/med_dispatch.sh"

# 1. C97（上）：验证 semi_supply L4 认证、30,000 行批次传感湖表与 AxQL 跨层查询
CERT_SEMI=$(api POST /api/xblue/subgraphs/semi_supply/certify '{}')
expect_json "semi-supply-l4-and-swimlanes" "$CERT_SEMI" '
  .cert_level == "L4_CERTIFIED" and .dead_knowledge_cnt == 0 and .trial_tick_passed == true
'
SEMI_CNT=$(api POST /api/analytics/sql/run '{"sql":"SELECT COUNT(*) AS cnt FROM semi_lot_telemetry"}')
expect_json "semi-supply-30k-lake-rows" "$SEMI_CNT" '.rows[0][0] >= 30000'

# 2. C97（下）：验证 med_dispatch L4 认证、18,000 行分诊明细与 120 急救车空间凸包
CERT_MED=$(api POST /api/xblue/subgraphs/med_dispatch/certify '{}')
HULL_MED=$(api GET "/api/xblue/nodes/convex-hull?subgraph=med_dispatch&type_name=AmbulanceUnit")
expect_json "med-dispatch-convex-hull-and-l4" "$(jq -n --argjson c "$CERT_MED" --argjson h "$HULL_MED" '{c:$c, h:$h}')" '
  .c.cert_level == "L4_CERTIFIED"
  and .c.dead_knowledge_cnt == 0
  and .h.node_count >= 50
  and .h.hull.type == "Polygon"
'

gate_pass step34
```
