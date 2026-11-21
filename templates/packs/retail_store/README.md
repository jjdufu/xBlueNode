# 连锁零售门店经营模板包（`retail_store`）

## 业务场景概述

本模板面向商超便利、品牌专卖与全渠道零售企业的**区域门店日销监控、单品断货预警与促销效果推演**场景，构建“经营大区（`Region`）— 零售门店（`Store`）— 商品单品（`Sku`）— 促销活动（`Promotion`）”四维经营图谱，支持大区营收自动上卷汇总与全格式经营日报出品。

## 包含对象清单

1. **领域本体模型（`schema.json`）**
   - 节点类型：`Region`、`Store`、`Sku`、`Promotion`。
   - 关系类型：`OPERATES_STORE`（大区管辖门店）、`STOCKS_SKU`（门店陈列销售单品）、`PROMOTES_SKU`（促销拉动单品）。
   - 动态跨边聚合规则：`Region.total_revenue = SUM(Store.daily_sales)`（沿 `OPERATES_STORE` 自动汇总）。
2. **样例数据集与映射（`sample_data/*.csv` + `mappings.json`）**
   - `regions`、`stores`、`skus`、`promotions` 四张 UTF-8 样例表，覆盖八大经营区域与典型热销单品。
3. **语义指标与告警规则（`metrics.json` + `rules.json`）**
   - 指标：`total_daily_sales`（全国门店日销售总额）、`avg_store_daily_sales`（单店日均销售额）。
   - 规则：单品断货率高于 `15%` 预警（`Sku.stockout_rate > 0.15`）、门店在库件数不足 `300` 件预警。
4. **编排流程、驾驶舱与智能体**
   - 流程：`retail-daily-sync`（触发门店至大区营收全图汇总）。
   - 驾驶舱：`retail-overview`（大区营收 KPI、门店销售额对比、四维经营拓扑、实时断货告警）。
   - 智能体：`retail_copilot`（门店经营复盘、补货与促销评估助手）。

## 安装后运维指引

1. 访问 `/view/retail-overview` 查看全渠道经营驾驶舱，并在 **xBlueView · 出品中心** 一键导出 XLSX / DOCX 销售经营简报。
2. 在 **AxSim · 推演沙盘** 调整 `Promotion.discount_rate` 或门店客流指标，对比不同促销力度的营收拉动效果。
3. 在 **AxFlow · 告警中心** 为高断货率规则配置门店督导通知渠道。
