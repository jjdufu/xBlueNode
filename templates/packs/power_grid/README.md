# 新型电力与配网调度模板包（`power_grid`）

## 业务场景概述

本模板面向省市级电网调度与新型配电网运行管控场景，完整构建“电源域（发电场站）— 输变电域（变电站 + 倒闸跳闸事件）— 配电域（台区配变）— 用电域（重点负荷用户 + 供电分区负荷汇总）”四域三层可执行业务模型，打通空间多边形供电围栏、馈线边电流/线损四路径动态传导、主变跳闸最晚干预窗口推演与五区数字看板。

## 包含对象清单

1. **四域三层业务模型（`schema.json`）**
   - 节点类型：`PowerPlant`（发电场站）、`Substation`（变电站，含多边形供电围栏 `geo_shape`）、`DistTransformer`（台区配变）、`CriticalLoad`（重点负荷用户）、`SwitchOrder`（倒闸与跳闸事件，`EVENT_DYNAMIC`）、`GridZoneSummary`（供电分区负荷汇总，`CUBE_ROLLUP`）。
   - 关系类型：`SUPPLIES_SUBSTATION`、`FEEDS_TRANSFORMER`（携带边指标 `current_amp`、`line_load_ratio`、`line_loss_mw` 及 `VERTEX_TO_EDGE` / `EDGE_TO_VERTEX` 过载传导规则）、`SERVES_LOAD`、`RECORDS_SWITCH`、`SUMMARIZES_ZONE`。
   - 业务动作契约：`dispatch_ess_discharge`（储能顶峰放电）与 `shed_noncritical_load`（可中断负荷柔性削减）。
2. **样例数据集与物化映射（`sample_data/*.csv` + `mappings.json`）**
   - 包含发电场站、变电站、台区配变、重点负荷、倒闸记录与供电分区六组 UTF-8 样例表，支持一键预览与开发演练子图克隆。
3. **认证指标、告警规则与黄金验收题（`metrics.json` + `rules.json` + `competency.json`）**
   - 覆盖变电站负载率越限、台区配变过载风险与供电分区备裕度预警规则，以及确定性业务问数验收题集。
4. **编排流、五区数字看板与领域助手**
   - 流程：`power-grid-ess-dispatch`（储能协同放电与全图重算流程）。
   - 数字看板：`power-grid-cockpit`（12 列标准五区布局：全局控制、核心指标、供电围栏地图、负荷趋势洞察与告警/动作决策）。
   - 智能体：`grid_dispatcher`（配网负荷诊断与保供调度助手）。
