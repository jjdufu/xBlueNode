# A7 · 需求追踪矩阵

本附录把全部能力项（59 项，按 A 底座、B 设计、C 构建、D 智能、E 推演、F 可视、G 系统分组，能力范围参照对标产品官网公开的功能说明整理）与全部数据源类别（26 类）映射到负责的 Step、实现小节与可执行的验收用例。“验收”列中带引号的名称是该 Step 门禁脚本 `scripts/gate/stepNN.sh` 中的断言名；其余为测试文件或端到端用例。每一项都有唯一负责的 Step。

---

## 1. 能力项

### A 底座

| # | 能力 | Step | 实现 | 验收 |
| :--- | :--- | :--- | :--- | :--- |
| C01 | 业务对象一体管理（对象、关系、属性、指标统一模型） | 03 / 05 | [04](../../plans/completed/v1-foundation-step00-14/04-STEP03-AXGRAPH-STORAGE-AND-GRAPH-QUERY.md) §03.2、[06](../../plans/completed/v1-foundation-step00-14/06-STEP05-06-XBLUESTUDIO-MODELING-AND-ROLLUP.md) §05.2 | step03 “link-unique”；step05 “publish-1” |
| C02 | 对象上的计算规则与派生属性 | 06 | [06](../../plans/completed/v1-foundation-step00-14/06-STEP05-06-XBLUESTUDIO-MODELING-AND-ROLLUP.md) §06.1、§06.3 | step06 “derived”、“expr-chain-cmp” |
| C03 | 写入即自动汇总（增量、并发安全） | 06 | §06.3 | step06 “agg-reconcile”、“agg-incremental” |
| C04 | 时间窗口聚合、历史明细与当前值并存 | 03 | [04](../../plans/completed/v1-foundation-step00-14/04-STEP03-AXGRAPH-STORAGE-AND-GRAPH-QUERY.md) §03.3 | step03 “rollup-day”、“window-sliding”、“metrics-merge” |
| C05 | 数据保留策略 | 00 / 14 | [01](../../plans/completed/v1-foundation-step00-14/01-STEP00-FOUNDATION-AND-GUARD-SKELETON.md) §00.6.6、[12](../../plans/completed/v1-foundation-step00-14/12-STEP14-OPERATIONS-TEMPLATES-ACCEPTANCE.md) §14.4 | step14 “retention-jobs”、“audit-anchor” |
| C06 | 对象级向量相似检索 | 03 | §03.4 | step03 “node-search” |
| C07 | 混合检索知识库（向量 + 图谱 + 全文，多路召回） | 仓库已有 / 04 | `routes/documents.rs`、[05](../../plans/completed/v1-foundation-step00-14/05-STEP04-AXANALYTICS-OLAP-AND-NL2SQL.md) §04.8 | step00 冒烟；step04 “ask-data” |
| C08 | 关联查询与路径分析 | 03 | §03.6 | step03 “neighbors”、“path” |
| C09 | 图算法（社区、中心性） | 03 | §03.6 | step03 “pagerank” |
| C10 | 空间地理数据与地图态势 | 03 / 11 | §03.2（`geo_lng/geo_lat`）、[10](../../plans/completed/v1-foundation-step00-14/10-STEP11-12-XBLUEVIEW-TWIN-DASHBOARD-EXPORT.md) §11.4 | step03 “geo-bbox”；`e2e/xblueview.spec.ts` 地图组件渲染 |
| C11 | 行级与子图级可见范围 | 13 | [11](../../plans/completed/v1-foundation-step00-14/11-STEP13-AXGUARD-SECURITY-GOVERNANCE.md) §13.6 | step13 “row-policy”、“row-policy-filter” |
| C12 | 敏感信息脱敏后协作 | 13 | §13.7、§13.8 | step13 “mask-phone”、“east-no-reveal”、“pii-egress”；step04 “pii-mask” |
| C13 | 跨部门共享访问 | 13 | §13.9.1 | step13 “cedar-deny”、“grant-active” |
| C14 | 第三方库全量 + 增量同步（含 Schema 演进） | 02 | [03](../../plans/completed/v1-foundation-step00-14/03-STEP02-AXHUB-CONNECTORS-AND-SYNC.md) §02.4、§02.9 | step02 “orders-rows”、“single-flight”；`tests/test_connectors_sql.py` |
| C15 | 流式数据与消息队列接入 | 02 | §02.8 | `tests/test_streams.py`（去重与位点） |
| C16 | 私有化部署、轻量起步、规模扩展 | 00 / 14 | §00.9、§14.2 | step14 “health-deep”；`all.sh` |
| C17 | 备份恢复 | 14 | §14.3 | `restore-drill.sh` 输出 `restore-drill: OK` |

### B 设计

| # | 能力 | Step | 实现 | 验收 |
| :--- | :--- | :--- | :--- | :--- |
| C18 | 一张图设计对象、关系、属性 | 05 | §05.7 | `e2e/xbluestudio.spec.ts`；step05 “publish-1” |
| C19 | 草稿确认后发布 | 05 | §05.4 | step05 “publish-1”、“publish-2-conflict” |
| C20 | AI 从需求描述与数据生成模型草稿 | 05 | §05.5 | step05 “ai-draft”；`schema_draft_eval.rs` |
| C21 | 模型版本与变更影响分析 | 05 | §05.4 | step05 “diff”、“breaking” |
| C56 | 多子图部门独立建模与跨部门本体对齐融合 | 05 | §05.1、§05.4.3 | step05 “subgraph-publish”、“subgraph-isolated”、“subgraph-preview”、“subgraph-merge”、“subgraph-merged-main” |

### C 构建

| # | 能力 | Step | 实现 | 验收 |
| :--- | :--- | :--- | :--- | :--- |
| C22 | 多源接入（关系库、时序、数仓、文件、接口、消息） | 01 / 02 | [02](../../plans/completed/v1-foundation-step00-14/02-STEP01-AXPARSER-FILE-PARSING.md)、[03](../../plans/completed/v1-foundation-step00-14/03-STEP02-AXHUB-CONNECTORS-AND-SYNC.md)、[A3](../../plans/completed/v1-foundation-step00-14/A3-CONNECTOR-SPEC-CARDS.md) | 第 2 节逐类验收 |
| C23 | 数据资产目录（字段注释全文检索、术语、血缘） | 02 | §02.9.5 | step02 “catalog” |
| C24 | AI 生成数据处理逻辑并试运行 | 04 | [05](../../plans/completed/v1-foundation-step00-14/05-STEP04-AXANALYTICS-OLAP-AND-NL2SQL.md) §04.7 | `tests/transform.rs`（草稿 → 预览 → 应用生成 `DERIVED_VIEW`） |
| C25 | 子图建模写入对象库（映射） | 06 | §06.2 | step06 “mapped-orders”、“mapped-links”、“incremental-mode” |
| C26 | 可视化规则与流程编排（DAG） | 07 | [07](../../plans/completed/v1-foundation-step00-14/07-STEP07-08-AXFLOW-ORCHESTRATION-RULES-ALERTS.md) §07.2～§07.4 | step07 “cycle”、“steps-all” |
| C27 | 条件满足自动动作（写入即判定） | 08 | §08.2～§08.3 | step08 “alert-open”、“flow-started” |
| C28 | 定时任务 | 02 / 07 | §02.9.6、§07.5 | `axpoint-flow/tests/triggers.rs`（Cron 下次触发时间与咨询锁单例） |
| C29 | 测试环境与正式环境分开发布 | 07 / 09 | §07.6、[08](../../plans/completed/v1-foundation-step00-14/08-STEP09-AXAGENT-PUBLISHING-MCP-AGENTS.md) §09.3 | step07 “test-dry-run”；step09 “prod-pinned” |
| C30 | 查询即应用（页面、接口、Skill、MCP 共用） | 09 | §09.3～§09.4 | step09 “pub-ok”、“openapi”、“mcp-call” |
| C31 | 交付过程可追溯 | 14 | [12](../../plans/completed/v1-foundation-step00-14/12-STEP14-OPERATIONS-TEMPLATES-ACCEPTANCE.md) §14.7 | step14 “work-item” |
| C32 | 接入主流大模型（多厂商、按任务路由、回退、计量） | 09 | §09.2 | `axpoint-llm/tests/router.rs`（回退与用量记录） |
| C33 | 外部 MCP 与插件工具接入 | 09 | §09.4 | `axpoint-agent/tests/mcp_client.rs`（同步外部工具清单并调用） |
| C57 | 业务对象自动生成领域 REST / 动态 GraphQL 接口与 MCP 暴露 | 05 / 09 | §05.6、[08](../../plans/completed/v1-foundation-step00-14/08-STEP09-AXAGENT-PUBLISHING-MCP-AGENTS.md) §09.3～§09.4 | step05 “domain-graphql”、“domain-objects”；step09 “published-object”、“mcp-tools” |

### D 智能

| # | 能力 | Step | 实现 | 验收 |
| :--- | :--- | :--- | :--- | :--- |
| C34 | 智能问数（NL2SQL + 图表） | 04 | §04.6 | step04 “ask-data”；评测集 ≥ 85% |
| C35 | 多轮 AI 工作台（会话、记忆、挂载知识与数据） | 09 | §09.5 | `axpoint-agent/tests/runtime.rs`（会话续接）；`e2e/axagent.spec.ts` |
| C36 | 可配置智能体与嵌入链接 | 09 | §09.5、§09.7 | step09 “embed-ok”、“embed-revoked” |
| C37 | 多智能体协作 | 09 | §09.5 | step09 “multi-agent” |
| C38 | 知识库批量上传解析 | 01 / 02 | §01.2～§01.11、§02.6 | step02 “upload-commit”；step01 全部断言 |
| C60 | 云端与本地小模型 `json_schema` 受限解码及确定性零温采样 | 15 | [01-STEP15](../../plans/v2-ux-and-grounded-ai/grounded-ai/01-STEP15-CONSTRAINED-DECODING-AND-LLM-CAPS.md) §15.2～§15.3 | step15 “schema-guided-ok”、“temp-zero-enforced” |

### E 推演

| # | 能力 | Step | 实现 | 验收 |
| :--- | :--- | :--- | :--- | :--- |
| C39 | 按业务范围克隆隔离沙盘（写时复制） | 10 | [09](../../plans/completed/v1-foundation-step00-14/09-STEP10-AXSIM-SIMULATION-AND-DECISION.md) §10.2 | step10 “prod-untouched” |
| C40 | 施加行动观察层层传导 | 10 | §10.3～§10.4 | step10 “propagated”、“shorthand-equal”、“missing-metric”；`tests/sim.rs` |
| C41 | 多方案并排对比 | 10 | §10.7.2 | step10 “compare” |
| C42 | 检查点、过程回看、推演问数 | 10 | §10.5、§10.7.3 | step10 “checkpoints”；`sim_ask_eval` |
| C43 | AI 研判建议 | 10 | §10.7.2 | step10 “compare”（`briefing.recommended_scenario_id`） |
| C44 | 决策落到动作（审批后执行） | 10 | §10.8 | step10 “applied”、“audit-sim-apply”、“conflict” |

### F 可视

| # | 能力 | Step | 实现 | 验收 |
| :--- | :--- | :--- | :--- | :--- |
| C45 | 按业务对象的孪生视图 | 11 | [10](../../plans/completed/v1-foundation-step00-14/10-STEP11-12-XBLUEVIEW-TWIN-DASHBOARD-EXPORT.md) §11.3（`NodeDrawer`） | `e2e/xblueview.spec.ts` |
| C46 | 拖拽驾驶舱（指标、趋势、告警、图表、场景视图） | 11 | §11.2、§11.6 | step11 “kpi”；`e2e/xblueview.spec.ts` 8 个组件渲染 |
| C47 | 组件绑定查询，数据变化即时反映 | 11 | §11.5 | step11 “sse-invalidate”、“kpi-refreshed” |
| C48 | 运行页问数与下钻 | 11 | §11.3、§11.6 | step11 “drill”；`e2e/xblueview.spec.ts` 面包屑 |
| C49 | 一键发布独立页面与嵌入 | 12 | §12.5.2 | step12 “share-view-1”、“share-max-views”、“share-revoked” |
| C50 | 汇报与验收出品（Excel、Word、PPT、PDF 等） | 12 | §12.2～§12.4 | step12 六种格式 `verify_export.py`、“chart-png”、“audit-export” |
| C51 | 图可视化（筛选、属性、沿关系展开、检索） | 03 / 11 | §03.8、§11.6 | step03 “neighbors”；`graph-render-bench.mjs` |
| C58 | AI 一句话生成孪生看板与页面级动作回写闭环 | 11 | §11.2、§11.6 | step11 “ai-dashboard”、“widget-action-loop”、“widget-action-log” |
| C59 | 3D 空间数字孪生场景与实时指标驱动着色 | 11 | §11.1、§11.4 | step11 “scene-3d”；`e2e/xblueview.spec.ts` |

### G 系统

| # | 能力 | Step | 实现 | 验收 |
| :--- | :--- | :--- | :--- | :--- |
| C52 | 用户角色、访问令牌、操作日志 | 00 / 09 / 13 | §00.6.4、§09.8、§13.3～§13.4、§13.9.2 | step00 “stats-requires-token”；step13 “refresh-reuse”、“audit-chain” |
| C53 | 运营中心：实时监控与预警告警 | 08 / 14 | §08.3～§08.4、§14.4.3 | step08 “notified”、“auto-resolve”；step14 “health-deep” |
| C54 | 行业应用模板复用 | 14 | §14.5 | step14 “template-installed”、“template-dashboard”、“template-uninstalled” |
| C55 | 样例数据一键装载演示 | 14 | §14.6 | `POST /api/axops/demo/supply_chain/load` 后 `scripts/demo/supply_chain.md` 各步结果；`e2e/full-journey.spec.ts` |
| C61 | 系统级只读模型与用户个人默认模型分级覆盖 | 15 | [01-STEP15](../../plans/v2-ux-and-grounded-ai/grounded-ai/01-STEP15-CONSTRAINED-DECODING-AND-LLM-CAPS.md) §15.3.4、§15.4 | step15 “user-default-provider”、“system-provider-readonly” |

---

## 2. 数据源类别

| # | 类别 | 覆盖格式 / 产品 | Step | 实现 | 验收 |
| :--- | :--- | :--- | :--- | :--- | :--- |
| D01 | CSV / TSV | `.csv`、`.tsv`，UTF-8 / GB18030，坏行隔离 | 01 | [02](../../plans/completed/v1-foundation-step00-14/02-STEP01-AXPARSER-FILE-PARSING.md) §01.3 | step01 “csv-pii”、“staging-under-root”；`test_csv.py` |
| D02 | TXT | 纯文本、定宽、键值、分隔 | 01 | §01.4 | `test_text.py` |
| D03 | Excel | `.xlsx`、`.xlsm`、`.xls`、`.xlsb`、`.ods`；多表头、合并单元格、多区块、公式 | 01 | §01.5 | step01 “xlsx-blocks”、“xlsx-types”；`test_excel.py` |
| D04 | PPT | `.pptx`、`.ppt`；备注、表格、图表数据 | 01 | §01.6 | step01 “pptx-notes” |
| D05 | Word | `.docx`、`.doc`；标题层级、合并 / 嵌套表格 | 01 | §01.7 | step01 “docx-tables” |
| D06 | PDF | 文本页与表格页 | 01 | §01.8 | `test_pdf.py` |
| D07 | 图片 | `.png`、`.jpg`、`.tiff`（OCR） | 01 | §01.8 | `test_pdf.py` OCR 用例（full 镜像；lite 镜像断言 415） |
| D08 | JSON | `.json`、`.jsonl`；嵌套扁平化、子表拆分、Schema 漂移 | 01 | §01.9 | step01 “json-child” |
| D09 | XML | 命名空间、属性、记录识别；XXE 防护 | 01 | §01.10 | step01 “xml-records”、“xxe-blocked” |
| D10 | 日志 | nginx、log4j 多行、syslog、容器日志；持续采集（Vector / Fluent Bit） | 01 / 02 | §01.11、[03](../../plans/completed/v1-foundation-step00-14/03-STEP02-AXHUB-CONNECTORS-AND-SYNC.md) §02.7.4 | step01 “log-ts”；step02 “webhook-rows” |
| D11 | 对象存储 / SFTP | `S3`、`SFTP` | 02 | §02.6.2、[A3](../../plans/completed/v1-foundation-step00-14/A3-CONNECTOR-SPEC-CARDS.md) | §02.6.2 测试（minio 中 3 个文件合并为一个数据集，第 4 个文件 APPEND 只读入新行） |
| D12 | 通用关系库 | `POSTGRES`、`MYSQL`、`MSSQL`、`ORACLE`、`SQLITE` | 02 | §02.4 | step02 “conn-ok”、“orders-rows”；`test_connectors_sql.py` |
| D13 | 国产数据库 | `DAMENG`、`KINGBASE`、`OPENGAUSS`、`OCEANBASE`、`TIDB` | 02 | §02.4 | `test_connectors_sql.py -m heavy`；MySQL / PG 协议级用例 |
| D14 | 分析型库与数仓 | `CLICKHOUSE`、`DORIS`、`STARROCKS`、`HIVE` | 02 | §02.4 | `test_connectors_sql.py`（ClickHouse 常驻；其余 heavy） |
| D15 | MongoDB | `MONGODB` | 02 | §02.5 | `test_connectors_nosql.py` |
| D16 | Redis | `REDIS` | 02 | §02.5 | `test_connectors_nosql.py`（`HASH` 100 键 → 100 行） |
| D17 | Elasticsearch | `ELASTICSEARCH` | 02 | §02.5 | `test_connectors_nosql.py`（`nested` 拆子表） |
| D18 | 宽列库 | `CASSANDRA`、`HBASE` | 02 | §02.5 | `test_connectors_nosql.py -m heavy` |
| D19 | 图数据库 | `NEO4J` | 02 | §02.5 | `test_connectors_nosql.py -m heavy` |
| D20 | 时序库 | `INFLUXDB`、`TDENGINE`、`IOTDB` | 02 | §02.5 | `test_connectors_ts.py`（InfluxDB 增量常驻；其余 heavy） |
| D21 | Kafka | `KAFKA`（JSON / Avro / CSV / RAW） | 02 | §02.8 | `test_streams.py`（1 000 条、故障重跑去重） |
| D22 | MQTT | `MQTT` | 02 | §02.8 | `test_streams.py`（QoS 1 持久会话 50 条） |
| D23 | Pulsar | `PULSAR` | 02 | §02.8 | `test_streams.py::test_pulsar_ack` |
| D24 | REST API | `REST_API`（5 种分页、6 种认证、限速重试） | 02 | §02.7.1 | `test_connectors_api.py` |
| D25 | GraphQL | `GRAPHQL`（Relay 分页，拒绝变更操作） | 02 | §02.7.2 | `test_connectors_api.py` |
| D26 | Webhook 推送 | `WEBHOOK`（HMAC / Bearer、去重、防重放） | 02 | §02.7.3 | step02 “webhook-ok”、“webhook-bad”、“webhook-dup”、“webhook-rows” |

---

## 3. 检查方法

```bash
# 每个能力项与数据源类别都在本表出现且只出现一次
grep -cE '^\| C[0-9]{2} ' docs/standards/A7-REQUIREMENTS-TRACEABILITY.md   # 59
grep -cE '^\| D[0-9]{2} ' docs/standards/A7-REQUIREMENTS-TRACEABILITY.md   # 26

# 表中引用的门禁断言名都存在于对应门禁脚本（输出缺失项，期望为空）
python3 scripts/check_traceability.py docs/standards/A7-REQUIREMENTS-TRACEABILITY.md scripts/gate
```
