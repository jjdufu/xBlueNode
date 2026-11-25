# 半导体全球多级供应链（semi_supply）

面向晶圆制造、特种材料跨国采购、航空恒温物流与车规芯片履约协同的可执行业务模型模板包。

## 1. 四域复合泳道与三层语义结构

| 对象类型 | 中文名 | 业务域 (`domain_group`) | 语义分层 (`semantic_layer_tier`) | 说明 |
| :--- | :--- | :--- | :--- | :--- |
| `SpecialtyVendor` | 特种气体与硅片商 | `采购域` | `STATIC_FACT` | 上游高纯特气、光刻胶与大硅片供应商 |
| `WaferFab` | 晶圆制造厂 | `制造域` | `STATIC_FACT` | 核心晶圆流片厂，支持 `reallocate_safety_wafer` 跨厂调配安全晶圆 |
| `LithoTool` | 关键光刻/刻蚀机台 | `制造域` | `STATIC_FACT` | 光刻与刻蚀关键装备，监测腔体温度、颗粒缺陷率与设备综合效率 |
| `WaferLotEvent` | 晶圆批次流片记录 | `制造域` | `EVENT_DYNAMIC` | 折叠内嵌于 `WaferFab` 的晶圆批次动态流片明细 |
| `YieldRollupCube` | 制程良率汇总块 | `制造域` | `CUBE_ROLLUP` | 折叠内嵌于 `WaferFab` 的月度制程良率与报废成本汇总块 |
| `CustomsPort` | 清关口岸与航站 | `物流域` | `STATIC_FACT` | 恒温航空口岸与清关枢纽，支持 `expedite_air_charter` 绿色包机快通 |
| `RegionalHub` | 区域恒温分拨仓 | `物流域` | `STATIC_FACT` | 覆盖各大区的车规芯片恒温分拨中心 |
| `AutoOemOrder` | 车企核心芯片大单 | `履约域` | `STATIC_FACT` | 下游新能源与智能驾驶车企核心芯片保供大单 |

## 2. 内置动作契约与五区驾驶舱

- **跨厂调配安全晶圆（`WaferFab.reallocate_safety_wafer`）**：校验 `safety_wafer_stock >= param.qty`，原子扣减安全库存并提升当月投片产能。
- **启用恒温包机绿色通道（`CustomsPort.expedite_air_charter`）**：校验 `clearance_hours > 4`，缩短清关耗时并快速疏解积压晶圆批次。
- **五区数字看板（`semi-supply-cockpit`）**：采用 12 列 `STANDARD_FIVE_ZONE` 标准五区布局，集成投片/良率/机台效率核心指标、全球四域供应链拓扑、机台趋势图、区域分拨库存对比与风险告警协同区。
