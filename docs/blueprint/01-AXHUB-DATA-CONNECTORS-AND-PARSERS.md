# 01 · AxHub 全源数据接入与 AxParser 解析引擎规范（Step 01 ~ Step 02）

本文件定义 **AxHub（全源数据接入层）** 与 **AxParser（双轨统一解析引擎）** 的完整实现规范，确保平台能够统一接入非结构化文档、半结构化报文、结构化表格、日志文件、关系型/非关系型数据库以及外部 API，并将其分别路由至**超图文本通道（Markdown/Chunks）**与**结构化分析通道（Parquet/Arrow 表 + Schema 元数据）**。

---

## Step 01：统一全格式文件解析引擎（`axpoint-parser`）

### 1.1 目标与输出路由模型
统一解析引擎将任意文件解析为**双轨结构化载荷（Dual-Track Output）**，避免将 CSV/Excel/JSON 仅当作纯文本丢失结构信息：
1. **文本语义轨（Text Track）**：输出标准化的 Markdown 章节与摘要，写入 `hg_docs` / `hg_paras` / `hg_entities` 进行五路 GraphRAG 检索。
2. **结构化数据轨（Tabular Track）**：输出规范化 Parquet 文件与列 Schema 描述（列名、物理类型、空值率、样例值），注册为可被 OLAP 引擎直接查询与制表的数据集（Dataset）。

### 1.2 支持矩阵与解析策略

| 文件格式 | 后缀 | 解析库 | 文本轨输出策略 | 结构化轨输出策略 |
| :--- | :--- | :--- | :--- | :--- |
| **CSV / TSV** | `.csv`, `.tsv` | `polars` / `duckdb` | 生成前 N 行摘要 + 字段统计描述 | 自动探测编码（`chardet`）与分隔符，写出强类型 Parquet |
| **TXT** | `.txt` | 原生 + 编码探测 | UTF-8 解码与段落规整 | 按分隔符试探是否可结构化；纯文本仅走文本轨 |
| **Excel** | `.xlsx`, `.xls`, `.xlsm` | `openpyxl` + `calamine` + `pandas` | 每个 Sheet 转为 Markdown 表格摘要 | **每个 Sheet 独立导出为一张 Dataset（Parquet）**，自动处理合并单元格填充与公式求值结果提取 |
| **PPT** | `.pptx` | `python-pptx` | 按页抽取标题、正文、演讲者备注（Notes）、表格转为 Markdown | 提取幻灯片内嵌表格与图表底层数值系列生成附表 Dataset |
| **Word** | `.docx` | `docling` / `python-docx` | 完整标题层级、段落、内嵌表格转 Markdown | 将文档内嵌大表格（行数 >= 3）同步提取为 Dataset |
| **PDF / 图像** | `.pdf`, `.png`, `.jpg` | `docling` + `easyocr`（降级 `pypdf`） | 版面还原 + OCR + 公式 + 表格转 Markdown | 抽取出的规范表格同步落地为附表 Dataset |
| **JSON / JSONL** | `.json`, `.jsonl`, `.ndjson` | `orjson` + `polars` | 生成结构概览与关键路径文本 | 数组对象/JSONL 自动扁平化（`.` 连接嵌套键）为 Parquet |
| **XML** | `.xml` | `lxml` | 标签树转结构化 Markdown | 提取重复元素节点集合（Record Tag）扁平化为 Parquet |
| **Logs** | `.log`, `.out` | 正则引擎 + Drain 模板聚类 + `orjson` | 聚合日志模板（Log Templates）与异常堆栈摘要 | 解析为结构化日志表（`timestamp`, `level`, `service`, `trace_id`, `message`, `raw`）写入 Parquet |

### 1.3 Python 模块目录结构（`axpoint-parser/`）

```text
axpoint-parser/
├── main.py                       # FastAPI 入口，路由注册与生命周期管理
├── schemas.py                    # Pydantic v2 请求/响应契约
├── parsers/
│   ├── __init__.py
│   ├── base.py                   # BaseParser 抽象基类与 ParseResult 模型
│   ├── registry.py               # 基于 MIME + 后缀 + 魔数的解析器路由注册表
│   ├── tabular_parser.py         # CSV / TSV / Excel 多 Sheet 解析器
│   ├── presentation_parser.py    # PPTX 解析器（含备注与图表数据提取）
│   ├── document_parser.py        # Docling(PDF/DOCX/Image) + TXT + pypdf 降级
│   ├── structured_parser.py      # JSON / JSONL / XML 递归扁平化解析器
│   └── log_parser.py             # 多格式日志（Nginx/Syslog/JSONL/Java Stacktrace）解析器
└── tests/
    ├── fixtures/                 # 各类测试样本文件
    └── test_parsers.py           # 全格式解析自动化单元测试
```

### 1.4 核心接口与数据契约

#### Pydantic 响应模型（`schemas.py`）
```python
class ColumnProfile(BaseModel):
    name: str
    data_type: Literal["STRING", "INT64", "FLOAT64", "BOOLEAN", "TIMESTAMP", "JSON"]
    nullable: bool
    null_count: int
    distinct_estimate: int
    sample_values: list[str]
    detected_pii: Literal["NONE", "PHONE", "ID_CARD", "EMAIL", "BANK_CARD"] = "NONE"

class ExtractedTableArtifact(BaseModel):
    table_name: str
    row_count: int
    parquet_path: str
    columns: list[ColumnProfile]

class UnifiedParseResponse(BaseModel):
    title: str
    source_format: str
    engine: str
    markdown: str
    tables: list[ExtractedTableArtifact]
    warnings: list[str] = []
```

#### FastAPI 接口契约（`axpoint-parser` 端口 `8001`）
- `POST /parse/unified`
  - 请求：`multipart/form-data`，字段 `file`、`tenant_id`、`dataset_dir`（Parquet 输出目录，默认 `/var/lib/axpoint/datasets`）、`log_pattern`（可选自定义正则）。
  - 响应：`UnifiedParseResponse`。
- 兼容保留 `POST /parse` 返回 `{title, markdown, engine}`，内部代理调用统一解析管线。

### 1.5 Step 01 可验证门禁（Verification Gate）

```bash
# 1. 启动解析单元测试（覆盖 10 种格式样本：csv, txt, xlsx多sheet, pptx, docx, pdf, json, xml, log, png）
cd axpoint-parser
pytest -v tests/test_parsers.py

# 2. 验证多 Sheet Excel 解析同时输出 Markdown 与 Parquet 表结构
curl -s -F "file=@tests/fixtures/supply_chain.xlsx" -F "tenant_id=default" \
  http://127.0.0.1:8001/parse/unified | jq '{source_format, table_count: (.tables | length), first_cols: .tables[0].columns}'
```
**通过标准**：
1. 10 类样本测试全部 `PASSED`，无未捕获异常。
2. Excel 多 Sheet 文件返回 `tables.length >= 2`，且对应的 `.parquet` 文件在磁盘可读、列名与类型推断准确（数值列不退化为 `STRING`）。
3. PPTX 解析结果包含幻灯片正文与备注；XML/JSON 嵌套对象扁平化字段完整；日志文件提取出 `timestamp` 与 `level` 列。

---

## Step 02：异构数据源连接器与元数据探查（SQL / NoSQL / API）

### 2.1 数据库 Schema 设计（`deploy/migrations/002_data_fabric.sql`）

```sql
-- 1. 外部数据源连接配置表
CREATE TABLE IF NOT EXISTS ds_connections (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id       TEXT NOT NULL DEFAULT 'default',
    name            TEXT NOT NULL,
    connector_type  TEXT NOT NULL, -- 'POSTGRES' | 'MYSQL' | 'CLICKHOUSE' | 'MSSQL' | 'ORACLE' | 'SQLITE' | 'MONGODB' | 'REDIS' | 'ELASTICSEARCH' | 'REST_API' | 'GRAPHQL'
    encrypted_cfg   BYTEA NOT NULL, -- AES-256-GCM 加密的 JSON 连接配置（host/port/user/password/headers/token）
    cfg_nonce       BYTEA NOT NULL,
    status          TEXT NOT NULL DEFAULT 'ACTIVE', -- 'ACTIVE' | 'ERROR' | 'DISABLED'
    last_tested_at  TIMESTAMPTZ,
    last_error      TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, name)
);

-- 2. 统一数据集注册表（文件表 + 外部同步表 + API 拉取表 + SQL 派生视图）
CREATE TABLE IF NOT EXISTS ds_datasets (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id       TEXT NOT NULL DEFAULT 'default',
    connection_id   UUID REFERENCES ds_connections(id) ON DELETE SET NULL,
    source_doc_id   UUID REFERENCES hg_docs(id) ON DELETE SET NULL,
    name            TEXT NOT NULL,
    display_name    TEXT NOT NULL,
    source_kind     TEXT NOT NULL, -- 'FILE_PARQUET' | 'SQL_TABLE' | 'NOSQL_COLLECTION' | 'API_STREAM' | 'DERIVED_VIEW'
    physical_ref    TEXT NOT NULL, -- Parquet 路径 或 源表名/URL
    row_count       BIGINT NOT NULL DEFAULT 0,
    sync_mode       TEXT NOT NULL DEFAULT 'SNAPSHOT', -- 'SNAPSHOT' | 'INCREMENTAL'
    watermark_col   TEXT,
    watermark_val   TEXT,
    schema_version  INT NOT NULL DEFAULT 1,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, name)
);

-- 3. 数据集列级元数据与敏感字段画像
CREATE TABLE IF NOT EXISTS ds_dataset_columns (
    id                UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    dataset_id        UUID NOT NULL REFERENCES ds_datasets(id) ON DELETE CASCADE,
    ordinal_pos       INT NOT NULL,
    col_name          TEXT NOT NULL,
    data_type         TEXT NOT NULL,
    nullable          BOOLEAN NOT NULL DEFAULT TRUE,
    null_ratio        DOUBLE PRECISION NOT NULL DEFAULT 0.0,
    distinct_count    BIGINT NOT NULL DEFAULT 0,
    sample_values     JSONB NOT NULL DEFAULT '[]'::jsonb,
    pii_category      TEXT NOT NULL DEFAULT 'NONE', -- 'NONE' | 'PHONE' | 'ID_CARD' | 'EMAIL' | 'BANK_CARD' | 'SECRET'
    semantic_hint     TEXT NOT NULL DEFAULT '',     -- AI/探查推断的业务含义（如 "工厂产能" / "订单主键"）
    UNIQUE (dataset_id, col_name)
);

-- 4. 数据同步作业历史表
CREATE TABLE IF NOT EXISTS ds_sync_jobs (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id     TEXT NOT NULL DEFAULT 'default',
    dataset_id    UUID NOT NULL REFERENCES ds_datasets(id) ON DELETE CASCADE,
    status        TEXT NOT NULL, -- 'RUNNING' | 'SUCCEEDED' | 'FAILED'
    rows_synced   BIGINT NOT NULL DEFAULT 0,
    error_message TEXT,
    started_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    finished_at   TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_ds_datasets_tenant ON ds_datasets (tenant_id);
CREATE INDEX IF NOT EXISTS idx_ds_sync_jobs_dataset ON ds_sync_jobs (dataset_id, started_at DESC);
```

### 2.2 连接器适配器架构（`axpoint-parser/connectors/`）

```text
axpoint-parser/connectors/
├── __init__.py
├── base.py              # BaseConnector 协议: test_connection(), discover_objects(), profile_object(), extract_to_parquet()
├── crypto.py            # AES-256-GCM 凭证加解密（密钥源自环境变量 AXPOINT_CREDENTIAL_KEY）
├── sql_connector.py     # 基于 SQLAlchemy + ConnectorX/Polars 支持 Postgres/MySQL/ClickHouse/MSSQL/Oracle/SQLite
├── mongo_connector.py   # MongoDB 集合 Schema 采样推断 + 嵌套文档展开写入 Parquet
├── es_connector.py      # Elasticsearch / OpenSearch Mapping 探查 + Scroll/PIT 拉取
├── redis_connector.py   # Redis Key Pattern / Hash / Stream 采样转表
├── api_connector.py     # REST / GraphQL 连接器（Bearer/Basic/APIKey/OAuth2 + JSONPath 数据定位 + 游标/页码分页）
└── profiler.py          # 自动统计 null_ratio、distinct_count、正则敏感列检测（手机号/身份证/邮箱/银行卡）
```

### 2.3 安全与性能设计要点
1. **凭证零明文落库**：前端提交连接配置至 `axpoint-server` 后，Rust 服务端立即使用 `AXPOINT_CREDENTIAL_KEY`（32 字节）通过 `aes-gcm` Crate 加密后写入 `ds_connections.encrypted_cfg`，所有查询接口返回的连接详情均对密码/Token 字段执行掩码脱敏。
2. **只读探查与防注入**：SQL 连接器的表名与字段名全部经 `^[a-zA-Z0-9_\.]+$` 白名单校验并使用方言标识引号转义；`discover_objects()` 仅查询 `information_schema` / 系统目录，不扫描全表数据。
3. **增量同步水位线（Watermark）**：支持按自增 ID 或 `updated_at` 时间戳列执行增量拉取，并将新分片追加合并至数据集 Parquet 目录，同步记录写入 `ds_sync_jobs`。

### 2.4 核心 API 契约（`axpoint-server` `/api/data/*`）

| 方法与路径 | 说明 | 请求体 / 查询参数 | 响应结构 |
| :--- | :--- | :--- | :--- |
| `POST /api/data/connections/test` | 连通性测试（不落库） | `{connector_type, config}` | `{ok, latency_ms, server_version, error}` |
| `POST /api/data/connections` | 创建并加密保存连接 | `{tenant_id, name, connector_type, config}` | `ConnectionSummary`（脱敏） |
| `GET /api/data/connections` | 列出租户下所有连接 | `?tenant_id=default` | `Vec<ConnectionSummary>` |
| `GET /api/data/connections/{id}/discover` | 探查外部库表/集合/端点 | `?tenant_id=default` | `Vec<{object_name, object_kind, est_rows}>` |
| `POST /api/data/datasets/sync` | 同步指定对象到本地数据湖并自动画像 | `{tenant_id, connection_id, object_name, display_name, sync_mode, watermark_col?}` | `SyncJobRecord` + `DatasetDetail` |
| `GET /api/data/datasets` | 列出所有已注册数据集（含文件上传生成表） | `?tenant_id=default` | `Vec<DatasetSummary>` |
| `GET /api/data/datasets/{id}/profile` | 获取列元数据、分布与 PII 标记 | `?tenant_id=default` | `DatasetProfileResponse` |
| `GET /api/data/datasets/{id}/preview` | 预览前 N 行样本数据（默认 100 行） | `?tenant_id=default&limit=100` | `{columns, rows}` |
| `POST /api/data/webhooks/{dataset_id}` | 接收外部 JSON/Log 事件流写入增量表 | Header `X-AxPoint-Webhook-Secret` + JSON Array/Object | `{accepted_rows}` |

### 2.5 Step 02 可验证门禁（Verification Gate）

```bash
# 1. 执行数据库迁移并确认 ds_* 表结构
psql "$DATABASE_URL" -f deploy/migrations/002_data_fabric.sql

# 2. 运行连接器集成测试（内置 SQLite/Postgres/Mock REST API/Mongo/Log 采样）
cd axpoint-parser
pytest -v tests/test_connectors.py

# 3. 验证 Rust 侧加密存储与 API 端到端链路
cargo test -p axpoint-db data_fabric
cargo clippy --workspace --all-targets -- -D warnings

# 4. 实测连接探索与同步入湖
curl -s -X POST http://127.0.0.1:9995/api/data/connections/test \
  -H "Content-Type: application/json" \
  -d '{"connector_type":"REST_API","config":{"url":"http://127.0.0.1:8001/health","method":"GET"}}' | jq .
```
**通过标准**：
1. `ds_connections.encrypted_cfg` 在数据库中为不可读密文，GET 接口响应中无明文密码/密钥泄露。
2. 成功完成至少 1 个 SQL 连接表、1 个 JSON API 端点与 1 个日志文件数据集的同步，生成 Parquet 文件并在 `ds_dataset_columns` 中准确标出类型、`null_ratio` 与自动检测出的 `PHONE`/`EMAIL` PII 标记。
3. 重复触发增量同步时，仅拉取 `watermark` 之后的数据，总行数无重复膨胀。
