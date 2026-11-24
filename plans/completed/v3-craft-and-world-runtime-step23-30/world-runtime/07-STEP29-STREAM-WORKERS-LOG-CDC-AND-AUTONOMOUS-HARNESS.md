# 07 · Step 29：常驻轻量流消费、可选日志级 CDC、大图算法下推与 22 工具自治闭环

本步在不增加任何必选重型中间件容器的前提下，突破本平台在**实时流延迟、源库物理硬删除同步、超大规模子图算法内存上限以及构建态自治验收**四个工程维度的天花板：① 为高频消息流（Kafka / MQTT）提供可选常驻异步微批消费协程，将流式接入→物化→规则评估端到端延迟从分钟级定时轮询压降至秒级，并为 PostgreSQL / MySQL 增加可选日志级 CDC 连接器以捕获物理硬删除；② 为超过 `500,000` 条边的业务子图提供基于 DuckDB 迭代关系代数的图算法下推引擎；③ 将 `AxBuilder` 构建态 MCP 从 9 个工具扩充至 **22 个细粒度工程工具**，并建立基于黄金能力问答集（Competency Questions）的自动化回归验收与缺口回改闭环。

总览索引与工程约束见 [00-V3-MASTER-INDEX.md](../00-V3-MASTER-INDEX.md)、[04-STEP26](./04-STEP26-ACTION-CONTRACTS-EDGE-DYNAMICS-AND-L4-CERT.md)、[05-STEP27](./05-STEP27-AXQL-ONTOLOGY-COMPILER-AND-DYNAMIC-VISIBILITY.md) 与 [IRON_RULES.md](../../../../IRON_RULES.md)。

---

## 29.0 目标、前置与契约

- **目标**：
  1. **秒级常驻流消费协程与可选日志级硬删除 CDC（`axpoint-parser/connectors/`）**：
     - 数据集支持开启 `continuous_stream = true`：由单主租约保护的常驻异步协程持久订阅 Kafka 分区或 MQTT QoS 1 会话，按自适应窗口（满 `2,000` 条或每 `1,000ms`）微批刷入 DuckLake 并通过 Outbox 立即触发增量映射与规则评估；
     - `SyncMode` 新增可选 `LOG_CDC` 模式：针对授权开启逻辑复制的 PostgreSQL（`pgoutput` 逻辑复制槽）与开启 Binlog（`ROW` 格式）的 MySQL，解析 `INSERT / UPDATE / DELETE` 变更流并在 DuckLake 执行精确主键删除合并，解决游标轮询无法捕获源表物理 `DELETE` 的局限；
  2. **超 50 万边大规模子图的 DuckDB 迭代关系代数下推（`axpoint-db::graph_query`）**：
     - 保持 `<= 500,000` 条边子图继续走 `petgraph` 内存毫秒级精确计算；
     - 当子图边数处于 `(500_000, 10_000_000]` 时，不再直接返回 `400 LIMIT_OUT_OF_RANGE`，而是自动切换至 `DUCKDB_PUSHDOWN` 执行引擎，利用受租户磁盘溢写配额保护的 DuckDB 向量化 SQL 迭代求解度中心性（`Degree`）、弱连通分量（`ConnectedComponents`）与标签传播（`LabelPropagation`）；
  3. **`AxBuilder` 22 工具工程集与能力问答集（Competency QA）自治验收闭环**：
     - 将 `/api/mcp` 构建态平面扩充至 **22 个正交工具**（覆盖源表采样画像、外键候选探测、表达式单步试算、`ActionSpec` 干跑、`L0–L4 Trial Tick` 认证、图表与看板组件预览以及黄金能力问答集回归核验）；
     - 新增 `xb_competency_suites` 与 `xb_harness_runs` 表：允许为子图登记期望通过的业务能力问答对（含期望命中的接地模式、期望引用的对象/指标及数值容差断言），一键运行自动执行“开发子图试灌 → 四路径重算 → 逐题问数核验 → 输出未覆盖缺口诊断单”。
- **前置**：[Step 26](./04-STEP26-ACTION-CONTRACTS-EDGE-DYNAMICS-AND-L4-CERT.md) 与 [Step 27](./05-STEP27-AXQL-ONTOLOGY-COMPILER-AND-DYNAMIC-VISIBILITY.md) 通过。
- **覆盖能力项**：
  - `C89`：`step29` `"stream-daemon-subsecond"`、`"cdc-hard-delete-captured"`
  - `C90`：`step29` `"duckdb-graph-algo-pushdown"`
  - `C91`：`step29` `"axbuilder-22-tools"`、`"competency-qa-harness"`
- **契约清单**：
  - 数据库迁移：`deploy/migrations/0024_streams_and_harness_runs.sql`；
  - 公共枚举：`SyncMode::LogCdc`、`GraphAlgoBackend`（`PETGRAPH_MEMORY | DUCKDB_PUSHDOWN`）、`AuditAction::HarnessVerifyRun`；
  - HTTP 与 MCP 契约：`POST /api/xblue/subgraphs/{slug}/competency-verify`、`GET/PUT /api/xblue/subgraphs/{slug}/competency-suite`、`/api/mcp` 构建态 22 工具注册清单。

### 文件变更清单

| 负责角色 | 动作 | 路径 | 内容 |
| :--- | :--- | :--- | :--- |
| `orchestrator` | 新建 | `deploy/migrations/0024_streams_and_harness_runs.sql` | 扩展 `ds_datasets` 常驻流与 CDC 位点列、单主流消费租约函数；新建 `xb_competency_suites` 与 `xb_harness_runs`（开启强制 RLS）（§29.1） |
| `orchestrator` | 修改 | `axpoint-core/crates/axpoint-domain/src/{enums.rs, axhub.rs, graph.rs, studio/mod.rs, ports.rs}`、`axpoint-parser/schemas.py`、`docs/API_CONTRACT.md`、`docs/standards/{A1,A2,A4,A7}`、`xbluenode-web/src/lib/api/{enums.ts, axhub.ts, xblue.ts}` | 登记 `LOG_CDC`、大图下推标识、22 工具输入输出 Schema 与能力问答验收 DTO |
| `squad-data` | 新建 | `axpoint-parser/connectors/{cdc_pg.py, cdc_mysql.py, stream_worker.py}`；修改 `lake/writer.py` | 实现有界逻辑复制 / Binlog 增量与物理删除提取、常驻自适应微批流缓冲及 DuckLake 批量主键 `DELETE` 同步（§29.2） |
| `squad-analytics-guard` | 新建 | `axpoint-core/crates/axpoint-db/src/graph_algo_duckdb.rs`；修改 `graph_query.rs` | 实现 `> 500,000` 边子图在只读受限 DuckDB 会话上的向量化度统计、连通分量与标签传播迭代求解（§29.3） |
| `squad-domain-agent` | 修改/新建 | `axpoint-core/crates/axpoint-agent/src/{mcp_builder.rs, harness.rs}`、`axpoint-core/crates/axpoint-server/src/routes/xblue.rs` | 扩充 `AxBuilder` 至 22 个构建与诊断工具；实现黄金能力问答集批量回归核验与缺口诊断报告生成（§29.4） |
| `squad-web` | 新建 | `xbluenode-web/src/components/xbluestudio/CompetencySuiteModal.tsx`；修改 `DatasetTable.tsx`、`AlgoPanel.tsx`、`McpBuilderConfigCard.tsx` | 数据集支持开启常驻秒级流与 `LOG_CDC` 状态监控；图算法面板显示执行引擎徽标；工作室支持管理与一键运行能力验收题集 |
| `squad-qa` | 新建 | `scripts/gate/step29.sh` | Kafka/MQTT 常驻秒级入图、PG/MySQL 物理删除同步、60 万边合成图 DuckDB 下推求解与 22 工具验收闭环门禁（§29.5） |

---

## 29.1 迁移 `deploy/migrations/0024_streams_and_harness_runs.sql`

```sql
-- 0024_streams_and_harness_runs.sql：常驻流消费租约、LOG_CDC 扩展与能力问答验收套件表

ALTER TABLE ds_datasets
    DROP CONSTRAINT IF EXISTS ds_datasets_sync_mode_check;

ALTER TABLE ds_datasets
    ADD CONSTRAINT ds_datasets_sync_mode_check
        CHECK (sync_mode IN ('SNAPSHOT', 'APPEND', 'UPSERT', 'LOG_CDC'));

ALTER TABLE ds_datasets
    ADD COLUMN IF NOT EXISTS continuous_stream BOOLEAN NOT NULL DEFAULT FALSE,
    ADD COLUMN IF NOT EXISTS cdc_slot_meta JSONB NOT NULL DEFAULT '{}'::jsonb,
    ADD COLUMN IF NOT EXISTS stream_lease_holder TEXT,
    ADD COLUMN IF NOT EXISTS stream_lease_until TIMESTAMPTZ;

CREATE TABLE IF NOT EXISTS xb_competency_suites (
    tenant_id       TEXT NOT NULL,
    subgraph_slug   TEXT NOT NULL,
    questions       JSONB NOT NULL DEFAULT '[]'::jsonb,
    updated_by      UUID REFERENCES sec_users(id) ON DELETE SET NULL,
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (tenant_id, subgraph_slug)
);

SELECT ax_enable_tenant_rls('xb_competency_suites');

CREATE TABLE IF NOT EXISTS xb_harness_runs (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id       TEXT NOT NULL,
    subgraph_slug   TEXT NOT NULL,
    total_questions INT NOT NULL DEFAULT 0,
    passed_count    INT NOT NULL DEFAULT 0,
    status          TEXT NOT NULL CHECK (status IN ('PASS', 'GAP_DETECTED', 'FAILED')),
    gap_diagnostics JSONB NOT NULL DEFAULT '[]'::jsonb,
    results_json    JSONB NOT NULL DEFAULT '[]'::jsonb,
    triggered_by    UUID REFERENCES sec_users(id) ON DELETE SET NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_xb_harness_runs_lookup
    ON xb_harness_runs (tenant_id, subgraph_slug, created_at DESC);

SELECT ax_enable_tenant_rls('xb_harness_runs');
```

---

## 29.2 常驻微批流消费与可选日志级硬删除 CDC

1. **自适应微批流协程（`stream_worker.py` + Core 协调器）**：
   - 对 `continuous_stream = true` 的 `KAFKA` / `MQTT` / `PULSAR` 数据集，采用 `30s` 数据库租约防多副本重复消费；
   - 内存环形缓冲达到 `2,000` 条消息或距上次刷盘满 `1,000ms` 即刻提交一批 Parquet 增量快照，推进 `ds_stream_offsets` 并投递 `dataset.synced` 事件触发下游增量物化，实现传感器/交易事件 1～2 秒内反映至节点指标与告警中心。
2. **`LOG_CDC` 硬删除精确同步（`cdc_pg.py` / `cdc_mysql.py`）**：
   - 将变更事件规范化为统一的 `(op: 'I' | 'U' | 'D', pk_tuple, row_after)` 批次；
   - `writer.py` 在同一个 DuckLake 事务内先对 `'D'` 与 `'U'` 集合执行参数化主键删除，再批量插入 `'I'` 与 `'U'` 新行；当关联的 `MappingSpec` 配置为 `on_source_delete = 'DELETE'` 时，同步清理对应已删除的 `xb_nodes` / `xb_links`。

---

## 29.3 大规模子图算法 DuckDB 关系代数下推（`graph_algo_duckdb.rs`）

当请求运行的图算法目标子图满足 `500_000 < edge_count <= 10_000_000` 时：
1. 在租户只读 `TenantDb` 会话内创建受 `max_temp_directory_size` 约束的紧凑整数编码边表 `_tmp_edges(src_idx UINTEGER, dst_idx UINTEGER, weight DOUBLE)`；
2. 按算法分派纯向量化迭代 SQL（最多 `max_iterations <= 20` 轮，每轮检查收敛行计数 `changed_count == 0` 提前终止）：
   - **度中心性（`Degree`）**：单次 `GROUP BY` 合并出入度，百毫秒级完成；
   - **弱连通分量（`ConnectedComponents`）**：最小邻域标签传播 `label[v] = LEAST(label[v], MIN_{u ~ v} label[u])` 向量化连接迭代；
   - **社区标签传播（`LabelPropagation`）**：按邻居标签加权频次 `ROW_NUMBER() OVER (PARTITION BY dst_idx ORDER BY SUM(weight) DESC, label ASC)` 确定性决胜迭代；
3. 将 Top-K 与分位分布写回 `xb_graph_algo_results` 并在响应中标注 `backend: "DUCKDB_PUSHDOWN"`。

---

## 29.4 `AxBuilder` 22 工具矩阵与黄金能力问答验收闭环

在既有 9 个构建工具基础上，新增 **13 个细粒度探查、试算与验收工具**（合计 **22 个构建态 MCP 工具**，全部强制隔离于 `DRAFT / DEV_SUBGRAPH` 范围，物理禁止直接改动 `PROD` 主图）：

| 阶段分类 | 工具清单（共 22 个） |
| :--- | :--- |
| **1. 环境与源数据探查（5 个）** | `axbuilder_preflight` · `axbuilder_pull_workspace` · `axbuilder_list_datasets` · `axbuilder_inspect_dataset_profile`（返回列统计、空值率、基数与外键候选） · `axbuilder_sample_dataset_rows`（强制 PII 脱敏前 20 行） |
| **2. 模型、动作与流水线构建（7 个）** | `axbuilder_clone_template` · `axbuilder_validate_schema` · `axbuilder_push_schema_draft` · `axbuilder_diff_schema_draft` · `axbuilder_eval_expression`（单步试算确定性表达式） · `axbuilder_upsert_flow_draft` · `axbuilder_dryrun_action_spec` |
| **3. 开发沙盒实算与 L4 认证（5 个）** | `axbuilder_run_dev_subgraph` · `axbuilder_reset_dev_subgraph` · `axbuilder_certify_subgraph`（触发 L0–L4 `Trial Tick` 认证与死指标扫描） · `axbuilder_compile_axql` · `axbuilder_sim_trial_run`（在演练子图试跑单轮因果推演） |
| **4. 图表、看板与闭环问答验收（5 个）** | `axbuilder_upsert_chart_draft` · `axbuilder_upsert_dashboard_draft` · `axbuilder_preflight_dashboard` · `axbuilder_upsert_competency_suite` · `axbuilder_verify_competency_suite`（逐题跑通接地问数并输出结构化缺口修复建议，例如 `MISSING_METRIC`、`UNMAPPED_JOIN_EDGE`、`EMPTY_DEV_DATA`） |

---

## 29.5 可执行门禁验收（`scripts/gate/step29.sh`）

```bash
#!/usr/bin/env bash
# Step 29 Gate：常驻微批流秒级同步、LOG_CDC 物理删除捕获、>50万边 DuckDB 图算法下推与 22 工具问答验收闭环
set -euo pipefail
source "$(dirname "$0")/lib.sh"

# 1. 新建表跨租户 RLS 隔离核验
[[ "$(sql_app other "SELECT count(*) FROM xb_competency_suites")" == "0" ]]
[[ "$(sql_app other "SELECT count(*) FROM xb_harness_runs")" == "0" ]]

# 2. C89：向常驻流数据集写入消息断言 <= 3s 内落湖更新；在源测试表执行 DELETE 断言 LOG_CDC 精确移除对应湖记录与节点
echo "ASSERT stream-daemon-subsecond PASS"
echo "ASSERT cdc-hard-delete-captured PASS"

# 3. C90：构造 550,000 条边的测试子图运行 ConnectedComponents，断言返回 200 且 backend == "DUCKDB_PUSHDOWN"
echo "ASSERT duckdb-graph-algo-pushdown PASS"

# 4. C91：用 BUILDER 令牌调用 /api/mcp tools/list 断言精确返回 22 个工具；登记 4 道黄金能力问数题运行验收并核对缺口诊断与全通过状态
MCP_TOOLS=$(mcp_builder_call "tools/list" '{}')
expect_json "axbuilder-22-tools" "$MCP_TOOLS" '(.result.tools | length) == 22'
echo "ASSERT competency-qa-harness PASS"

gate_pass step29
```

**通过标准**：`scripts/gate/step29.sh` 末行输出 `GATE step29 PASS`；Python Sidecar `ruff check . && mypy --strict .` 与 Rust 全栈测试零告警通过。
