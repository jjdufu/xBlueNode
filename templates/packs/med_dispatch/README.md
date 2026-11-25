# 城市急诊与医疗床位调度（med_dispatch）

面向城市级三甲医院院区统筹、急诊重症床位周转、ECMO/呼吸机紧缺生命支持设备与 120 院前急救车辆空间调度的可执行业务模型模板包。

## 1. 三大业务域与三层语义结构

| 对象类型 | 中文名 | 业务域 (`domain_group`) | 语义分层 (`semantic_layer_tier`) | 说明 |
| :--- | :--- | :--- | :--- | :--- |
| `HospitalCampus` | 医疗院区 | `院区统筹域` | `STATIC_FACT` | 城市核心三甲医院院区，支持 `activate_surge_beds` 一键启用应急加床 |
| `ClinicalDept` | 急诊与重症科室 | `临床救治域` | `STATIC_FACT` | 急诊抢救、重症 ICU、胸痛中心、卒中中心与创伤中心科室 |
| `LifeSupportUnit` | 生命支持设备 | `临床救治域` | `STATIC_FACT` | ECMO、有创呼吸机与连续肾脏替代治疗（CRRT）关键设备 |
| `AmbulanceUnit` | 120 急救转运车 | `院前急救域` | `STATIC_FACT` | 携带实时经纬度坐标 `geo` 的院前急救车，支持空间覆盖凸包计算与 `divert_ambulance` 改派 |
| `TriageVisit` | 急诊分诊接诊单 | `临床救治域` | `EVENT_DYNAMIC` | 折叠内嵌于 `ClinicalDept` 的 I～IV 级急诊分诊流转单 |
| `WardCapacityCube` | 院区周转汇总块 | `院区统筹域` | `CUBE_ROLLUP` | 折叠内嵌于 `HospitalCampus` 的日均收治与床位周转率汇总块 |

## 2. 内置动作契约与五区急诊态势驾驶舱

- **启用应急机动病区加床（`HospitalCampus.activate_surge_beds`）**：原子更新 `total_free_beds` 与 `surge_level`，无需人工审批阻塞，支持突发高峰秒级扩容回写。
- **急救车分流改派（`AmbulanceUnit.divert_ambulance`）**：前置校验 `eta_minutes <= 35`，将院前急救车动态改派至邻近低饱和院区。
- **五区急诊态势看板（`med-dispatch-cockpit`）**：采用 12 列 `STANDARD_FIVE_ZONE` 布局，集成急诊候诊/空床/床位占用率 KPI、全市院区与 120 急救车空间分布地图（支持一键开启凸包覆盖包络面）、科室床位占用率实时曲线、分诊告警列表与启用应急加床快捷动作按钮。
