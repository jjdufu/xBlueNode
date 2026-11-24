# A6 · 依赖基线表

本表锁定全部第三方依赖的**大版本与次版本**，补丁版本以执行该 Step 契约 PR 时包仓库的最新补丁为准，并由 `Cargo.lock`、`uv.lock`（或 `requirements.lock`）、`package-lock.json` 固化。引入本表之外的依赖，必须先在契约 PR 中修改本表并写明理由与许可证。

许可证红线：禁止引入 AGPL / SSPL / Elastic License / “Sustainable Use” 类许可的组件进入可分发镜像（例如 PyMuPDF、Metabase、Airbyte 平台、n8n 不得内嵌）。

---

## 1. 基础运行时

| 组件 | 版本 | 说明 |
| :--- | :--- | :--- |
| Rust 工具链 | `1.100.0`（`rust-version`），edition `2024` | 与 `axpoint-core/Cargo.toml` 一致 |
| Python | `3.11`（镜像） / `>=3.11`（`pyproject.toml`） | Sidecar |
| Node.js | `24` | 前端构建、服务端图表渲染 |
| PostgreSQL | `16`（镜像 `pgvector/pgvector:pg16`） | 可平滑使用 17 / 18；16 的社区维护期至 2028-11 |
| pgvector | `0.8.x` | 镜像内置 |
| DuckDB | `2.0.x` | Rust 与 Python 两侧保持同一大版本 |
| DuckLake 扩展 | `1.0.x` | 构建镜像时 `INSTALL ducklake; INSTALL postgres;`，运行期禁止自动安装 |
| sqlx-cli | `0.9.x` | `cargo install sqlx-cli --no-default-features --features rustls,postgres` |

## 2. Rust（`axpoint-core/Cargo.toml` → `[workspace.dependencies]`）

仓库已有：`axum 0.8`（`multipart`）、`chrono 0.4`、`dotenvy 0.15`、`pgvector 0.4`（`sqlx`）、`reqwest 0.12`（`rustls-tls`）、`serde 1`、`serde_json 1`、`sqlx 0.9`（`runtime-tokio`、`tls-rustls-ring`、`postgres`、`uuid`、`chrono`、`json`）、`thiserror 2`、`tokio 1`（`full`）、`tower-http 0.6`（`cors`、`fs`、`trace`）、`tracing 0.1`、`tracing-subscriber 0.3`（`env-filter`）、`uuid 1`（`v4`、`serde`）。

按 Step 引入：

| Step | 依赖 | 版本 | 用途 |
| :--- | :--- | :--- | :--- |
| 00 | `sqlx` 追加 feature `migrate`、`macros` | 0.9 | 嵌入式迁移、`#[sqlx::test]` |
| 00 | `tower-http` 追加 feature `limit`、`request-id`、`set-header`、`compression-gzip` | 0.6 | 上传上限、请求 ID、缓存头 |
| 00 | `tracing-subscriber` 追加 feature `json` | 0.3 | JSON 日志 |
| 00 | `tracing-opentelemetry` / `opentelemetry` / `opentelemetry_sdk` / `opentelemetry-otlp` | 0.34 / 0.33 / 0.33 / 0.33 | 分布式追踪 |
| 00 | `metrics` / `metrics-exporter-prometheus` | 0.24 / 0.18 | 指标 |
| 00 | `subtle` | 2 | 常量时间令牌比较 |
| 00 | `trait-variant` | 0.1 | 对象安全端口 |
| 02 | `aes-gcm` | 0.11 | 凭证加密（1.0 之前版本，锁定次版本） |
| 02 | `hmac` / `sha2` / `hex` / `base64` | 0.12 / 0.10 / 0.4 / 0.22 | Webhook 签名、哈希 |
| 02 | `argon2` | 0.6 | 口令哈希（Step 13 使用同一依赖） |
| 02 | `zeroize` | 1 | 解密后的秘密内存清零 |
| 02 | `cron` | 0.17 | 数据集定时同步；Step 07 流程调度复用 |
| 02 | `tower_governor` | 0.8 | Webhook 接收限流；Step 13 登录与分享接口复用 |
| 03 | `uuid` 追加 feature `v5` | 1 | 确定性业务节点与超图实体 UUID（`Uuid::new_v5`） |
| 03 | `petgraph` | 0.8 | 路径、PageRank、社区发现 |
| 03 | `hyperloglogplus` | 0.4 | 去重计数草图 |
| 03 | `unicode-normalization` | 0.1 | 实体名称 Unicode NFKC 归一化（§03.5.1，MIT / Apache-2.0） |
| 04 | `duckdb` | `1.20000`（对应 DuckDB 2.0，feature `bundled`、`parquet`、`json`、`vtab-arrow`、`vscalar`） | OLAP、Arrow 批次流与自定义标量脱敏函数 |
| 04 | `sqlparser` | 0.64 | AST 白名单（`DuckDbDialect`） |
| 04 | `lru` | 0.16 | 租户 DuckDB 实例池 |
| 04 | `arrow` | 与 `duckdb` crate 依赖的大版本一致 | 结果转换 |
| 05 | `async-graphql` / `async-graphql-axum` | 7.2（`dynamic`） / 7.2 | 发布本体时动态生成领域对象 GraphQL Schema 与内省端点 |
| 07 | `jsonschema` | 0.61 | 算子配置校验；Step 09 发布端点入参校验复用 |
| 08 | `lettre` | 0.11（`tokio1-rustls-tls`、`smtp-transport`、`builder`） | 邮件通知 |
| 09 | `rmcp` | 3.10（feature `server`、`client`、`transport-streamable-http-server`、`transport-streamable-http-client-reqwest`） | MCP Server / Client |
| 09 | `eventsource-stream` | 0.2 | 智能体 SSE |
| 09 | `moka` | 0.12（`future`） | 发布端点结果缓存 |
| 09 | `futures` | 0.3 | 流式响应组合 |
| 10 | `rand` / `rand_chacha` | 0.9 / 0.9 | 不确定性抽样（固定种子可复现） |
| 10 | `rayon` | 1 | 抽样并行 |
| 10 | `petgraph`（Step 03 已引入） | 0.8 | 强连通分量分解 |
| 11 | `moka`（Step 09 已引入） | 0.12 | 组件结果缓存 |
| 12 | `tokio-util` | 0.7（`io`） | 出品产物文件流式响应（`ReaderStream`） |
| 13 | `jsonwebtoken` | 11 | EdDSA 访问令牌 |
| 13 | `openidconnect` | 4.1 | OIDC 授权码 + PKCE |
| 13 | `cedar-policy` | 4.14 | ABAC 策略求值 |
| 13 | `hkdf` | 0.12 | 按租户派生脱敏密钥 |
| 测试 | `testcontainers` / `testcontainers-modules` | 0.28 | Kafka、MinIO 等外部服务集成测试 |
| 测试 | `insta` | 1 | 快照测试 |
| 测试 | `proptest` | 1 | 表达式 AST 格式化与重解析往返属性测试（Step 06） |

## 3. Python（`axpoint-parser/pyproject.toml`）

```toml
[project]
requires-python = ">=3.11"
dependencies = [
  "fastapi>=0.115,<1", "uvicorn[standard]>=0.30,<1", "python-multipart>=0.0.12", "pydantic>=2.9,<3", "pydantic-settings>=2.4,<3",
  "duckdb==2.0.*", "polars==2.1.*", "pyarrow>=20",
  "python-calamine==0.9.*", "openpyxl==3.1.*", "python-pptx==1.0.*", "python-docx==1.2.*",
  "pypdf>=5", "lxml==6.1.*", "defusedxml==0.7.*", "orjson>=3.10", "ijson>=3.3",
  "charset-normalizer==3.5.*", "pypinyin>=0.53", "drain3==0.9.11", "genson==1.4.*", "pandera==0.36.*",
  # Step 02 连接器（A3 中镜像层为 lite 的全部驱动）
  "sqlalchemy==2.1.*", "connectorx==0.4.*", "sqlglot>=25", "psycopg[binary]>=3.2,<4", "pymysql>=1.1,<2",
  "clickhouse-connect==1.12.*", "pymongo==4.20.*", "redis==8.1.*", "elasticsearch==9.5.*", "neo4j>=5.26",
  "influxdb-client>=1.46", "influxdb3-python>=0.10", "taospy>=2.7", "apache-iotdb>=1.3",
  "confluent-kafka>=2.6,<3", "paho-mqtt>=2.1,<3", "pulsar-client>=3.5,<4",
  "s3fs>=2024.10", "paramiko>=3.5",
  "httpx>=0.28,<1", "tenacity>=9", "jsonpath-ng>=1.7,<2", "graphql-core>=3.2,<4",
  # Step 12 出品
  "xlsxwriter==3.3.*", "jinja2>=3.1,<4", "weasyprint==70.*", "cairosvg>=2.7,<3",
  # Step 13 PII 识别（spaCy 中文模型 zh_core_web_sm 以 wheel 形式在镜像构建时离线安装）
  "presidio-analyzer==2.2.*", "presidio-anonymizer==2.2.*", "spacy>=3.8,<4",
  "opentelemetry-instrumentation-fastapi>=0.50b0",
]

[project.optional-dependencies]
full = [
  "docling==2.150.*", "easyocr>=1.7,<2",
  # A3 中镜像层为 full 的驱动（依赖系统库或体积较大）
  "pymssql>=2.3,<3", "oracledb>=2", "dmPython>=2.5", "pyhive[hive_pure_sasl]>=0.7",
  "cassandra-driver>=3.29,<4", "happybase>=1.2,<2",
]
dev = ["pytest>=8", "pytest-asyncio>=0.24", "testcontainers>=4.8", "ruff>=0.8", "mypy>=1.13", "reportlab>=4.2", "syrupy>=4.8", "respx>=0.21"]
```

`lite` 镜像安装 `dependencies`；`full` 镜像安装 `dependencies + full`，并内置 LibreOffice（早期 Office 格式转换、公式重算）。

## 4. Web（`xbluenode-web/package.json`）

仓库已有：`react 19`、`react-dom 19`、`lucide-react 0.475`、`clsx 2`、`tailwind-merge 3`、`vite 6`、`@vitejs/plugin-react 4`、`tailwindcss 4`、`@tailwindcss/vite 4`、`typescript 5.7`。

| Step | 依赖 | 版本 | 用途 |
| :--- | :--- | :--- | :--- |
| 00 | `react-router` | 8.5 | URL 路由 |
| 00 | `@tanstack/react-query` | 5 | 请求缓存 |
| 00 | `zod` | 4 | 响应校验 |
| 00（dev） | `@playwright/test` | 1.5x | 页面冒烟 |
| 02 | `@tanstack/react-table` | 9.3 | 表格 |
| 04 | `echarts` | 6.1 | 图表 |
| 04 | `@monaco-editor/react` | 4.7 | SQL 编辑器 |
| 05 | `@xyflow/react` | 12.12 | 建模 / 编排画布 |
| 05 | `elkjs` | 0.12 | 自动布局 |
| 11 | `react-grid-layout` | 2.3（2.x API，与 1.x 不兼容） | 驾驶舱栅格 |
| 11 | `sigma` / `graphology` / `graphology-layout-forceatlas2` | 4.0 / 0.26 / 0.10 | 大图 WebGL 渲染 |
| 11 | `@microsoft/fetch-event-source` | 2 | 携带 `Authorization` 头的 SSE |
| 11 | `maplibre-gl` | 5 | 瓦片底图（设置 `AXPOINT_MAP_TILE_URL` 时） |
| 11 | `react-markdown` | 10 | Markdown 组件（禁用原始 HTML） |
| 11 | `three` / `@types/three` | 0.187 / 0.187 | 3D 空间数字孪生组件（`SCENE_3D`，glTF / `.glb` 加载与网格指标着色） |
| 23 | `react-resizable-panels` | 4.x | 弹性可调分栏工作台原语（MIT） |
| 23 | `cmdk` | 1.1.x | 全局 `⌘K / Ctrl+K` 无头命令搜索面板（MIT） |
| 24 | `@antv/g6` / `@antv/g6-extension-react` | 5.x / 0.2.x | 因果传导与资产血缘图谱异步分包渲染（MIT） |
| 24 | `zustand` / `zundo` | 5.x / 2.3.x | 画布状态与 50 步撤销重做栈（MIT） |
| 25 | `@visactor/vtable` / `@visactor/react-vtable` | 1.16.x / 1.16.x | 高性能 Canvas 虚拟化多维透视表（MIT / Apache-2.0） |

Sidecar 服务端图表（`axpoint-parser/charts/package.json`，Step 12，`lite` 与 `full` 镜像均内置）：`echarts` 6.1（与 Web 同版本）、`@resvg/resvg-js` 2.6。

## 5. 容器镜像

| 镜像 | 基础 | 说明 |
| :--- | :--- | :--- |
| `axpoint-core` | 构建：`rust:1.100-bookworm` + `node:24-bookworm`；运行：`gcr.io/distroless/cc-debian12:nonroot` | 单二进制 + `dist/` |
| `axpoint-parser:<tag>-lite` | `python:3.11-slim-bookworm` + Node 24（ECharts SSR） | 结构化解析、连接器、服务端图表与全格式出品 |
| `axpoint-parser:<tag>-full` | `python:3.11-slim-bookworm` + LibreOffice + Node 24（ECharts SSR） | 全量（含早期 Office 转换、公式重算、Docling / EasyOCR 与重型数据库驱动） |
| `postgres` | `pgvector/pgvector:pg16` | 元数据、超图、DuckLake 目录 |
| 测试辅助 | `redpandadata/redpanda`、`eclipse-mosquitto:2`、`minio/minio`、`mongo:8`、`redis:8`、`mysql:8.4`、`clickhouse/clickhouse-server:25`、`influxdb:3-core`、`tdengine/tdengine:3`、`apache/iotdb:2`、`atmoz/sftp`、`docker.elastic.co/elasticsearch/elasticsearch`（9.x，与 Python 客户端同一大版本）、`apachepulsar/pulsar:3.3`；`heavy` 测试另用的镜像见 A3 各卡片 | 只用于集成测试，不进入可分发镜像 |
