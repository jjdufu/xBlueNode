# 集团授信与互保风控模板包（`fin_risk`）

## 业务场景概述

本模板面向商业银行对公授信、集团关联图谱穿透与交叉互保风险联防场景，构建“集团授信域（集团系主体 + 关联法人企业）— 穿透核查域（实际控制人）— 信贷台账域（授信借据明细）— 风险监测域（行业敞口汇总）”四域三层可执行业务模型，内置实控人三道强制敏感脱敏、逾期状态驱动动态行级视野、6 万行对公资金流水多维透视及一键冻结授信额度闭环。

## 包含对象清单

1. **四域三层业务模型与敏感脱敏契约（`schema.json`）**
   - 节点类型：`CorpGroup`（集团系主体）、`LegalEntity`（关联法人企业）、`BeneficialOwner`（实际控制人，精确标注 `NAME` / `PHONE` / `ID_CARD` / `BANK_CARD` 四类敏感字段）、`CreditFacility`（授信借据明细，`EVENT_DYNAMIC`）、`IndustryExposureCube`（行业敞口汇总，`CUBE_ROLLUP`）。
   - 关系类型：`CONTROLS`（集团控股）、`HOLDS_EQUITY`（实控人穿透持股）、`CROSS_GUARANTEES`（携带担保金额与违约传染规则的交叉互保边）、`TRANSFERS_FUNDS`（大额对公资金划转）、`DRAWS_FACILITY`、`BELONGS_TO_SECTOR`。
   - 业务动作契约：`freeze_credit_line`（冻结授信额度）。
2. **样例数据集与物化映射（`sample_data/*.csv` + `mappings.json`）**
   - 包含集团主体、关联法人、实际控制人、授信借据与行业敞口五组 UTF-8 样例表，全部采用合规合成测试数据。
3. **认证指标、告警规则与黄金验收题（`metrics.json` + `rules.json` + `competency.json`）**
   - 覆盖法人逾期率越界、集团传染风险评分偏高与行业不良率预警规则，以及确定性问数验收题集。
4. **编排流、五区数字看板与风控助手**
   - 流程：`fin-risk-freeze-credit`（风险授信额度冻结与敞口重算流程）。
   - 数字看板：`fin-risk-cockpit`（12 列标准五区布局：控制栏、总敞口/不良率指标卡、60,000 行对公流水 VTable 多维透视、互保传染图谱拓扑及告警/额度冻结决策）。
   - 智能体：`risk_investigator`（互保圈违约传染追踪与集团穿透核查助手）。
