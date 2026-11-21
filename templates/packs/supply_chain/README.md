# 供应链协同行业模板包（`supply_chain`）

## 业务场景概述

本模板面向制造与流通企业的**多级供应链协同与履约风险管控**场景，开箱构建“供应商（`Supplier`）— 工厂（`Factory`）— 仓库（`Warehouse`）— 客户订单（`Order`）”四层业务本体网络，打通库存水位监控、产能断供因果推演与端到端订单履约驾驶舱。

## 包含对象清单

1. **领域本体模型（`schema.json`）**
   - 节点类型：`Supplier`、`Factory`、`Warehouse`、`Order`（其中 `Order.customer_phone` 内置 `PHONE` 敏感分类与动态脱敏保护）。
   - 关系类型：`SUPPLIES`（供应商供货工厂）、`REPLENISHES`（工厂补货仓库）、`FULFILLS`（仓库履约订单）。
   - 动态派生规则：`Warehouse.fill_rate = if(demand_qty > 0, inventory_qty / demand_qty, 1.0)`。
2. **样例数据集与映射（`sample_data/*.csv` + `mappings.json`）**
   - `suppliers`、`factories`、`warehouses`、`orders` 四张 UTF-8 样例表，自动物化业务节点与上下游链路。
3. **语义指标与告警规则（`metrics.json` + `rules.json`）**
   - 指标：`total_order_amount`（订单履约总金额）、`avg_warehouse_inventory`（仓库平均可用库存）。
   - 规则：仓库安全库存不足预警（`inventory_qty < 200`）与履约率跌破下限告警（`fill_rate < 0.5`）。
4. **编排流程、驾驶舱与智能体**
   - 流程：`supply-daily-refresh`（全图重算履约率与聚合指标）。
   - 驾驶舱：`supply-overview`（KPI 卡片、各仓现货柱图、多级网络拓扑、实时风险告警列表）。
   - 智能体：`supply_analyst`（支持智能问数与知识检索的履约风险诊断助手）。

## 安装后运维指引

1. 在 **AxFlow · 告警中心** 为已安装的低库存与低履约率规则绑定企业通知渠道。
2. 在 **AxSim · 推演沙盘** 可针对 `F-1`（华东一号工厂）模拟产能下降场景，评估受影响的下游仓库与订单。
3. 访问 `/view/supply-overview` 查看实时履约大盘，或创建受控外部分享链接。
