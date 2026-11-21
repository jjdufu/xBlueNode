# 离散制造产线与设备健康模板包（`manufacturing_equipment`）

## 业务场景概述

本模板面向装备制造、汽车零部件与精密机加车间的**设备预测性维护与产线 OEE 监控**场景，构建“生产产线（`Line`）— 机台设备（`Machine`）— 物联传感器（`Sensor`）— 生产工单（`WorkOrder`）”四类核心实体，实现主轴温振监控、产线 OEE 自动计算与超温巡检工单闭环下发。

## 包含对象清单

1. **领域本体模型（`schema.json`）**
   - 节点类型：`Line`、`Machine`、`Sensor`、`WorkOrder`。
   - 关系类型：`HAS_MACHINE`（产线下辖机台）、`MONITORS`（传感器监测机台）、`ASSIGNED_TO`（工单分派至机台）。
   - 动态规则：
     - 跨边聚合：`Line.max_spindle_temp = MAX(Machine.spindle_temp)`（沿 `HAS_MACHINE` 聚合）。
     - 节点内派生：`Line.oee = availability * performance * quality`。
2. **样例数据集与映射（`sample_data/*.csv` + `mappings.json`）**
   - `lines`、`machines`、`sensors`、`work_orders` 四张样例表，含超温与高振动异常样本。
3. **语义指标与告警规则（`metrics.json` + `rules.json`）**
   - 指标：`avg_spindle_temp`（机台平均主轴温度）、`total_planned_units`（工单计划总产量）。
   - 规则：`Machine.spindle_temp > 85.0` 超温红色告警、`Machine.vibration_rms > 4.5` 振动偏高预警。
4. **编排流程、驾驶舱与智能体**
   - 流程：`overheat-dispatch`（通过 `ACTION_EMIT` 的 `WORK_ITEM` 动作自动创建 P1 巡检工单）。
   - 驾驶舱：`equipment-overview`（产线 OEE、机台主轴温度曲线、设备工单拓扑、实时告警列表）。
   - 智能体：`equipment_advisor`（设备健康诊断与工单调度顾问）。

## 安装后运维指引

1. 在 **AxFlow · 编排中心** 将 `overheat-dispatch` 发布至 `PROD` 环境，并在超温规则中关联触发该流程。
2. 在 **AxOps · 工作项看板** 跟踪自动生成的 `P1` 机台超温巡检工单状态流转。
3. 访问 `/view/equipment-overview` 实时监控产线效率与设备温振态势。
