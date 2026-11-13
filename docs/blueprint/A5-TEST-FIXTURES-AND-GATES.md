# A5 · 测试样本、门禁函数库、评测集与基准

本附录定义：全部文件样本的生成规格（§1）、门禁脚本函数库 `scripts/gate/lib.sh` 的完整实现（§2）、各 Step 的种子输出变量（§3）、大模型评测集（§4）、基准脚本（§5）与其他表驱动测试数据（§6）。所有样本由代码生成、可复现，不提交来源不明的二进制文件。

---

## 1. 文件样本（`axpoint-parser/tests/fixtures/`）

- 生成器：`axpoint-parser/tests/fixtures/make_fixtures.py`（Step 00 建立，各 Step 追加生成函数）。固定随机种子 `20260101`，固定时间基准 `2026-01-01T00:00:00+08:00`，Office 文件写入固定的 `core.xml` 创建 / 修改时间，ZIP 条目使用固定时间戳，保证逐字节可复现。
- 校验：`fixtures.sha256` 记录全部样本哈希；`tests/test_fixtures.py::test_fixtures_reproducible` 重新生成并比对。
- 早期 Office 格式样本（`.xls`、`.ppt`、`.doc`）由生成器调用 LibreOffice（`soffice --headless --convert-to`，镜像内置）从对应的现代格式转换得到；转换结果因 LibreOffice 版本可能不同，因此这 3 个文件不进入 `fixtures.sha256` 的逐字节比对，只比对解析输出快照。
- 命令：`cd axpoint-parser && uv run python tests/fixtures/make_fixtures.py && sha256sum -c tests/fixtures/fixtures.sha256`。

| 文件 | 规格 | 使用者 |
| :--- | :--- | :--- |
| `orders.csv` | UTF-8，2 000 行；列 `order_id, order_date, customer_name, phone, amount, status, warehouse_code`；`phone` 为通过号段规则的 11 位手机号；`amount` 两位小数 | 01.3、Step 01 门禁 `csv-pii` |
| `orders_gbk.csv` | 与 `orders.csv` 同内容，GB18030 编码 | 01.3 编码探测 |
| `sensors.tsv` | 制表符分隔，500 行，`ts, device, temperature, humidity` | 01.3 |
| `broken.csv` | 20 行，其中 3 行引号不闭合 | 01.3 坏行隔离 |
| `plain.txt` | 中文段落 30 段 | 01.4 文本轨 |
| `fixed_width.txt` | 定宽列 6 列 × 200 行 | 01.4 定宽识别 |
| `key_value.txt` | `键: 值` 形式 50 行 | 01.4 |
| `delimited.txt` | 竖线分隔 300 行 | 01.4 |
| `supply_chain.xlsx` | Sheet `工厂`（3 行）；Sheet `订单`（两级表头 `数量 / 计划`、`数量 / 实际`，纵向合并单元格，40 行）；Sheet `库存`（一个 Sheet 内两个区块，各带合计行）。解析得到 4 张表，其中库存第二个区块表名 `ku_cun_2` | 01.5、Step 01 门禁、Step 02 种子与门禁、Step 14 端到端 |
| `formulas_uncached.xlsx` | openpyxl 写入公式且无缓存值 | 01.5 公式重算 |
| `early_format.xls` | 由 `supply_chain.xlsx` 转换 | 01.5 早期格式 |
| `report.xlsb` | 由生成器经 LibreOffice 转换 1 张 100 行表 | 01.5 |
| `plan.ods` | 1 张 50 行表 | 01.5 |
| `briefing.pptx` | 5 页：备注、1 个表格、1 个柱状图（图表数据成表）、1 个组合形状、1 个 SmartArt | 01.6、门禁 `pptx-notes` |
| `early_format.ppt` | 由 `briefing.pptx` 转换 | 01.6 |
| `contract.docx` | 3 级标题、1 个含合并单元格的表格、1 个嵌套表格、1 个 2 行小表（小表不成数据表） | 01.7、门禁 `docx-tables` |
| `early_format.doc` | 由 `contract.docx` 转换 | 01.7 |
| `annual_report.pdf` | reportlab 生成：文本页 3 页 + 表格页 1 页 | 01.8 |
| `scanned_invoice.png` | 把发票文本渲染为 1 654 × 2 339 灰度图 | 01.8 OCR |
| `api_orders.json` | `{"data":{"items":[...]}}`，200 条，每条含 `customer` 嵌套对象与 `lines` 对象数组；子表名 `api_orders__lines` | 01.9、门禁 `json-child` |
| `events.jsonl` | 5 000 行，字段随行变化（模拟 Schema 漂移） | 01.9 |
| `catalog.xml` | 默认命名空间 + `xsi` 前缀，100 个 `item`（属性 + 嵌套 `price`） | 01.10、门禁 `xml-records` |
| `xxe.xml` | 外部实体引用 `/etc/passwd` | 01.1、门禁 `xxe-blocked` |
| `billion_laughs.xml` | 10 层实体展开 | 01.1 |
| `zip_bomb.xlsx` | 单条目压缩比 > 100 | 01.1 |
| `nginx_access.log` | combined 格式 1 000 行 | 01.11 |
| `app_java.log` | log4j 格式，含 3 段 Java 堆栈（多行合并） | 01.11、门禁 `log-ts` |
| `syslog.log` | RFC 3164，300 行 | 01.11 |
| `k8s.log` | 容器 JSON 日志，300 行 | 01.11 |

Step 12 出品模板样本（`axpoint-parser/tests/data/templates/`，由同一生成器的 `make_templates()` 产出）：

| 文件 | 规格 |
| :--- | :--- |
| `report_template.docx` | 页眉 `{{title}}`、正文 `{{subtitle}}`、`{{generated_at}}`、独占一段的 `{{sections}}`；定义表格样式 `AxTable` |
| `deck_template.pptx` | 母版含“标题”与“标题和内容”版式，标题占位符 `{{title}}` |

Step 11 3D 数字孪生模型样本（`axpoint-parser/tests/data/models/`，由同一生成器的 `make_models_3d()` 产出并记入 `fixtures.sha256`）：

| 文件 | 规格 |
| :--- | :--- |
| `warehouse_twin.glb` | 最小合法 glTF 2.0 二进制模型（`.glb`），包含单个命名网格节点 `"Rack_A"`（单位立方体几何体 + PBR 材质），逐字节确定性生成；供 [10](./10-STEP11-12-XBLUEVIEW-TWIN-DASHBOARD-EXPORT.md) §11.4 与 Step 11 门禁 `scene-3d` 使用 |

---

## 2. 门禁函数库 `scripts/gate/lib.sh`

```bash
#!/usr/bin/env bash
# 门禁公共函数。所有 scripts/gate/stepNN.sh 与 scripts/seed/*.sh 以 source 方式引入。
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
[[ -d "$ROOT/axpoint-parser/.venv/bin" ]] && export PATH="$ROOT/axpoint-parser/.venv/bin:$PATH"
set -a
# shellcheck disable=SC1091
source "$ROOT/.env.test"
[[ -f "$ROOT/.gate/ids.env" ]] && source "$ROOT/.gate/ids.env"
set +a
: "${CORE_BASE_URL:?}" "${PARSER_BASE_URL:?}" "${AXPOINT_DEV_TOKEN:?}"
GATE_NAME="$(basename "${0%.sh}")"

_fail() { echo "GATE ${GATE_NAME} FAIL: $1" >&2; [[ -n "${2:-}" ]] && echo "$2" | head -c 2000 >&2; exit 1; }

# api METHOD PATH [JSON]：带开发令牌调用 Core，输出响应体（非 2xx 时同样输出，便于断言错误体）
api() {
  local m=$1 p=$2 body=${3:-}
  if [[ -n "$body" ]]; then
    curl -sS -X "$m" -H "Authorization: Bearer $AXPOINT_DEV_TOKEN" -H "Accept: application/json" \
         -H "Content-Type: application/json" "$CORE_BASE_URL$p" -d "$body"
  else
    curl -sS -X "$m" -H "Authorization: Bearer $AXPOINT_DEV_TOKEN" -H "Accept: application/json" "$CORE_BASE_URL$p"
  fi
}

# api_noauth METHOD PATH [JSON]：不带令牌
api_noauth() {
  local m=$1 p=$2 body=${3:-}
  if [[ -n "$body" ]]; then
    curl -sS -X "$m" -H "Accept: application/json" -H "Content-Type: application/json" "$CORE_BASE_URL$p" -d "$body"
  else
    curl -sS -X "$m" -H "Accept: application/json" "$CORE_BASE_URL$p"
  fi
}

expect_eq() { [[ "$2" == "$3" ]] || _fail "$1" "actual=[$2] expected=[$3]"; echo "ok  $1"; }

# expect_code NAME CODE METHOD PATH [JSON] [TOKEN]
#   METHOD 以 _NOAUTH 结尾（GET_NOAUTH、POST_NOAUTH 等）时不带令牌；TOKEN 非空时用其替代开发令牌
expect_code() {
  local name=$1 code=$2 m=$3 p=$4 body=${5:-} tok=${6:-$AXPOINT_DEV_TOKEN} auth=()
  if [[ "$m" == *_NOAUTH ]]; then m=${m%_NOAUTH}; else auth=(-H "Authorization: Bearer $tok"); fi
  local args=(-sS -o /dev/null -w '%{http_code}' -X "$m" "${auth[@]}" -H "Accept: application/json")
  [[ -n "$body" ]] && args+=(-H "Content-Type: application/json" -d "$body")
  local got; got=$(curl "${args[@]}" "$CORE_BASE_URL$p")
  expect_eq "$name" "$got" "$code"
}

# expect_json NAME JSON JQ_EXPR：jq 表达式结果必须为 true
expect_json() {
  local out; out=$(printf '%s' "$2" | jq -e "$3" 2>&1) || _fail "$1" "$2"
  [[ "$out" == "true" ]] || _fail "$1" "$2"
  echo "ok  $1"
}

sql_owner() { psql "$MIGRATION_DATABASE_URL" -v ON_ERROR_STOP=1 -Atqc "$1"; }

# sql_app TENANT SQL：以应用角色、指定租户上下文执行（验证 RLS）
sql_app() {
  psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -Atq \
    -c "BEGIN; SELECT set_config('app.tenant_id', '$1', true) \\g /dev/null" -c "$2" -c "COMMIT;" | sed '/^$/d'
}

# wait_until NAME TIMEOUT_S CMD：每秒执行 CMD，直到成功或超时
wait_until() {
  local name=$1 timeout=$2 cmd=$3 i=0
  until bash -c "source '$ROOT/scripts/gate/lib.sh' >/dev/null 2>&1; $cmd" >/dev/null 2>&1; do
    i=$((i + 1)); (( i >= timeout )) && _fail "$name" "timeout ${timeout}s: $cmd"; sleep 1
  done
  echo "ok  $name"
}

# 任务与业务对象等待（默认 120 秒）
wait_job() {
  local id=$1 st
  for _ in $(seq 1 120); do
    st=$(api GET "/api/axops/jobs/$id" | jq -r .status)
    case "$st" in SUCCEEDED) return 0 ;; FAILED|CANCELED|EXPIRED) _fail "job $id" "$(api GET "/api/axops/jobs/$id")" ;; esac
    sleep 1
  done; _fail "job $id" "timeout"
}
wait_dataset_idle() {
  wait_until "dataset-idle:$1" 120 \
    "api GET '/api/axhub/datasets/$1/sync-jobs?limit=1' | jq -e '.items[0].status == \"SUCCEEDED\"'"
}
wait_aggregate_idle() {
  wait_until "aggregate-idle" 120 \
    "[[ \$(sql_app default \"SELECT count(*) FROM ax_jobs WHERE kind='GRAPH_AGGREGATE' AND status IN ('PENDING','RUNNING')\") == 0 ]]"
}
wait_mapping_idle() {
  wait_until "mapping-idle:$1" 120 \
    "api GET '/api/xblue/mappings/$1/runs?limit=1' | jq -e '.items[0].status == \"SUCCEEDED\"'"
}
wait_run_status() {
  wait_until "run:$1:$2" 120 "api GET '/api/axflow/runs/$1' | jq -e '.status == \"$2\"'"
}
wait_scenario() {
  wait_until "scenario:$1" 120 "api GET '/api/axsim/scenarios/$1' | jq -e '.status == \"SUCCEEDED\"'"
}
wait_export() {
  wait_until "export:$1" 180 "api GET '/api/xblueview/exports/$1' | jq -e '.status == \"SUCCEEDED\"'"
}

# restart_core：重启本地 Core 进程（scripts/start-services.sh 记录 PID），等待健康检查
restart_core() {
  bash "$ROOT/scripts/start-services.sh" --restart core
  wait_until "core-healthy" 60 "curl -sf '$CORE_BASE_URL/api/health'"
}

gate_pass() { echo "GATE $1 PASS"; }
```

`.env.test` 由 `scripts/dev-up.sh` 生成（[01](./01-STEP00-FOUNDATION-AND-GUARD-SKELETON.md) §00.10.1），包含 `CORE_BASE_URL`、`PARSER_BASE_URL`、`DATABASE_URL`、`MIGRATION_DATABASE_URL`、`AXPOINT_DEV_TOKEN`、`AXPOINT_INTERNAL_TOKEN` 等；`all.sh` 另行设置 `AXPOINT_LLM_CASSETTE` 指向录制文件。

**自测**：`scripts/gate/lib_test.sh` 用 `python3 -m http.server` 模拟接口，覆盖 `expect_json` 的真 / 假 / 非法 JSON、`expect_code` 的 `_NOAUTH` 与覆盖令牌分支、`wait_until` 超时；作为 Step 00 门禁的第一项执行。

---

## 3. 种子脚本输出（`.gate/ids.env`）

| Step | 脚本 | 输出变量 |
| :--- | :--- | :--- |
| 00 | `00-baseline.sh` | `DOC_ID`、`WAREHOUSE_NODE_ID`、`ORDER1_NODE_ID`、`ORDER2_NODE_ID` |
| 02 | `02-axhub.sh` | `DEMO_PG_CONN_ID`、`DEMO_ORDERS_DATASET_ID`、`SUPPLY_UPLOAD_ID`、`WEBHOOK_SLUG`、`WEBHOOK_SECRET` |
| 03 | `03-axgraph.sh` | `FACTORY1_NODE_ID`、`ORDER20_NODE_ID`、`SEED_DAY1`、`SEED_DAY2` 等 |
| 04 | `04-analytics.sh` | `PAID_METRIC_NAME` |
| 05 | `05-studio.sh` | `ORDER_MAPPING_ID` 等映射 ID 与模型版本（见 [06](./06-STEP05-06-XBLUESTUDIO-MODELING-AND-ROLLUP.md) §05.8；Step 06 无种子脚本） |
| 07 / 08 | `07-axflow.sh`、`08-rules.sh` | `FLOW_ID`、`CYCLIC_FLOW_ID`、`LOW_INV_RULE_ID`、`SINK_SECRET`、`SINK_FILE`（见 [07](./07-STEP07-08-AXFLOW-ORCHESTRATION-RULES-ALERTS.md)） |
| 09 | `09-agents.sh` | `GATE_PAT`、`PAID_ENDPOINT_SLUG`、`OBJECT_ENDPOINT_SLUG`、`SUPERVISOR_AGENT_ID` |
| 10 | `10-axsim.sh` | `SANDBOX_ID`、`SIM_S1_ID`、`SIM_S2_ID` |
| 11 | `11-xblueview.sh` | `DASHBOARD_ID`、`KPI_WIDGET_ID`、`BAR_WIDGET_ID`、`MODEL3D_ASSET_ID`、`SCENE3D_WIDGET_ID`、`ACTION_WIDGET_ID` |
| 13 | `13-axguard.sh` | `EAST_PASSWORD`、`SOUTH_PASSWORD`、`AUDITOR_PASSWORD`、`OPS_ADMIN_PASSWORD`、`ORDERS_BY_STATUS_QUERY_ID` |

种子脚本只通过公开 API（以及 `sql_owner` 建立外部演示源库）写数据，重复执行幂等（按名称查找已存在对象并复用）。

---

## 4. 大模型评测集

| 评测 | 位置 | 内容 | CI 运行方式 | 门槛 |
| :--- | :--- | :--- | :--- | :--- |
| 智能问数 | `axpoint-analytics/tests/nl2sql_eval/cases.yaml` + `cassette.jsonl` | 60 条：单表聚合、多表关联、时间过滤、同比环比、TopN、指标引用、节点过滤、多轮追问、拒答（越权 / 无关） | 录制回放 | 执行结果与标准答案一致 ≥ 85%；拒答类 100% |
| 模型草案 | `axpoint-llm/tests/schema_draft_eval.rs` | 供应链 3 个数据集 → 节点类型、关系、映射 | 录制回放 | 生成 Factory / Warehouse / Order 及两类关系，映射列全部存在 |
| 智能体 | `axpoint-agent/tests/agent_eval/` | 20 条任务：工具选择、参数正确、多智能体委派、拒绝越权工具 | 录制回放 | 工具调用序列匹配 ≥ 90% |
| 推演问数 | `axpoint-server/tests/sim_ask_eval/` | 15 条“如果……”问题 → 干预解析 | 录制回放 | 目标节点与指标、方式、数值全部正确 ≥ 90% |
| AI 驾驶舱生成 | `axpoint-server/tests/dashboard_gen_eval/` | 10 条业务场景一句话描述 → 驾驶舱栅格布局与组件绑定草案 | 录制回放 | 组件 `widget_kind` 合法、引用的数据集 / 指标 / 节点类型全部存在 ≥ 90% |

录制与回放：`AXPOINT_EVAL_RECORD=1` 时连接真实模型并把 `(提示 SHA-256 → 响应)` 追加写入 cassette；`AXPOINT_LLM_CASSETTE=<文件>` 时 `axpoint-llm` 只从 cassette 读取，未命中直接返回 `UPSTREAM_LLM` 错误（保证 CI 不出网）；`AXPOINT_EVAL_LIVE=1` 连接真实模型并打印准确率，不作为 CI 门槛。

---

## 5. 基准（`scripts/bench/`）

基准环境：4 vCPU / 8 GB / NVMe SSD，PostgreSQL 与服务同机。结果写入 `scripts/bench/results/<name>-<git sha>.json`，不在 CI 中执行。

| 脚本 | 场景 | 断言 | 定义 |
| :--- | :--- | :--- | :--- |
| `analytics-data.sh` / `analytics-bench.sh` | 1 000 万行聚合、透视、指标、守卫 | 见 [05](./05-STEP04-AXANALYTICS-OLAP-AND-NL2SQL.md) §04.11 | Step 04 |
| `sim-bench.sh` | 5 万节点 / 20 万关系推演 | 稳态 P95 ≤ 2 秒；200 次抽样 P95 ≤ 20 秒 | [09](./09-STEP10-AXSIM-SIMULATION-AND-DECISION.md) §10.11 |
| `graph-render-bench.mjs` | 图浏览器 1 万节点 / 3 万关系 | 帧率 ≥ 30 fps（Playwright + `requestAnimationFrame` 计数） | [10](./10-STEP11-12-XBLUEVIEW-TWIN-DASHBOARD-EXPORT.md) §11.6 |
| `guard-bench.sh` | 含行列策略的 SQL 改写 | P95 ≤ 10 毫秒 | [11](./11-STEP13-AXGUARD-SECURITY-GOVERNANCE.md) §13.6.3 |

---

## 6. 其他表驱动测试数据

| 文件 | 内容 | Step |
| :--- | :--- | :--- |
| `axpoint-analytics/tests/guard.rs`（内嵌表） | SQL 守卫 ≥ 60 条 | 04 |
| `axpoint-domain/tests/expr_cases.json` | 表达式 ≥ 150 条 | 06 |
| `axpoint-parser/tests/test_typemap.py`（内嵌表） | 每个连接器特有类型映射 | 02 |
| `axpoint-security/tests/data/mask_cases.json` | 8 种脱敏规则各 ≥ 10 例；Rust 与 Python 共用 | 13（Step 01 列画像预览同样使用） |
| `axpoint-security/tests/data/row_policy_cases.json` | 行策略表达式 → DuckDB / PostgreSQL SQL；注入用例 | 13 |
| `axpoint-domain/tests/sim.rs`（内嵌表） | 推演解析解对照 | 10 |
