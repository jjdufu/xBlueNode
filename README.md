# xBlueNode / AxPoint — 节点智能与动态超图平台

> **xBlueNode**（业务节点建模、决策交互与出品工作台）× **AxPoint**（Rust 核心服务 + Python Sidecar 引擎底座）

技术栈：Rust `1.100.0`（`edition = "2024"`）+ PostgreSQL 16（`pgvector`）+ DuckDB / DuckLake + Python 3.11 FastAPI Sidecar（AxParser / AxHub）+ Vite / React 19。

完整的落地方案见 **[docs/blueprint/00-MASTER-INDEX.md](./docs/blueprint/00-MASTER-INDEX.md)**：Step 00～14 按依赖层逐步实施，每个 Step 给出目标、前置条件、迁移、文件清单、代码骨架、命令与可执行门禁。

---

## 一、模块体系

| 模块 | 职责 |
| :--- | :--- |
| **AxParser** | 文件双轨解析：CSV / TXT / Excel / PPT / Word / PDF / 图片 / JSON / XML / 日志 → 结构化 Markdown + 数据表 |
| **AxHub** | 连接器与同步：关系库、国产库、分析型库、NoSQL、时序库、对象存储、REST / GraphQL / Webhook、Kafka / MQTT / Pulsar，DuckLake 入湖与资产目录 |
| **AxGraph** | 超图（`hg_*`）+ 业务节点（`xb_*`）+ 时序指标 + 图查询与图算法 |
| **AxAnalytics / AxQuery** | DuckDB OLAP、透视制表、指标语义层、智能问数；五路召回 + RRF 知识问答 |
| **xBlueStudio / xBlueGraph** | 可视化建模、模型版本与发布、映射物化、派生与动态聚合；业务图浏览 |
| **AxFlow** | DAG 编排、持久化执行、测试 / 正式部署、规则引擎与告警中心 |
| **AxAgent** | 发布端点（API / Skill / MCP）、MCP Client、多模型路由、智能体与多轮工作台 |
| **AxSim** | 范围克隆沙盘、写时复制、因果传导、检查点回放、方案对比、决策下发 |
| **xBlueView** | 孪生驾驶舱、实时刷新、下钻、地图与平面图、全格式出品、安全分享 |
| **AxGuard** | 本地与 OIDC 认证、令牌轮换、Cedar ABAC、行级 / 列级策略、PII 识别、审计链 |
| **AxOps** | 生产部署、备份恢复、保留策略、行业模板包、演示数据、交付追踪 |

---

## 二、目录结构

```text
xBlueNode/
├── IRON_RULES.md          # 工程铁律（必读）
├── .github/               # CODEOWNERS、PR 模板、按路径触发的 CI（含命名守卫）
├── axpoint-core/          # Rust Workspace（clippy::pedantic、forbid(unsafe_code)）
│   └── crates/            # axpoint-domain / -llm / -db / -server，以及按 Step 创建的 -security / -analytics / -flow / -agent
├── axpoint-parser/        # Python FastAPI Sidecar：parsers/、connectors/、lake/、exporters/、charts/
├── xbluenode-web/         # Vite + React + TypeScript + Tailwind CSS 工作台
├── deploy/                # docker-compose、.env.example、migrations/（sqlx 格式 NNNN_<module>.sql）
├── scripts/               # naming-guard.sh，以及按 Step 增加的环境、种子、门禁、运维脚本
└── docs/                  # API_CONTRACT.md、TEAM_MULTI_AGENT_MANUAL.md、blueprint/
```

---

## 三、当前代码基线的本地运行

### 1. PostgreSQL（`pgvector`）

```bash
cd deploy
cp .env.example .env      # 填入 LLM_API_KEY / EMBED_API_KEY（兼容 OpenAI 协议的任意服务或本地 Ollama / vLLM）
docker compose up -d      # 首次建库时执行 deploy/migrations/0001_init_core.sql
```

### 2. Sidecar（`axpoint-parser`）

```bash
cd axpoint-parser
python3.11 -m venv .venv && source .venv/bin/activate
pip install -e .           # 完整 Docling + OCR：pip install -e ".[docling]"
uvicorn main:app --host 127.0.0.1 --port 8001
```

### 3. Core（`axpoint-core`）

```bash
cd xbluenode-web && npm ci && npm run build && cd ..
cd axpoint-core && cargo run -p axpoint-server
```

浏览器访问 `http://localhost:9995/`。前端开发模式：`cd xbluenode-web && npm run dev`。

Step 00 落地后，本地环境统一由 `scripts/dev-up.sh` + `scripts/start-services.sh` 启动，门禁由 `scripts/gate/stepNN.sh` 执行（见蓝图 Step 00 §00.10）。

---

## 四、核心特性

1. **多维度 Embedding**：`hg_embeddings.embedding` 为不定维 `vector`，按 `768` / `1024` / `1536` 维建立条件部分 HNSW 索引；`EmbeddingAdapter` 自动探测维度，切换模型无需迁移数据库。
2. **五路并行召回 + RRF 融合**：`Vector`、`BM25`、`EntityLink`、`Context (NEXT_PARA)`、`MultiHop (RELATES)` 并发召回，前端抽屉展示溯源证据与推理路径。
3. **严格的 Rust 规范**：全 Workspace 启用 `unsafe_code = "forbid"`、`clippy::pedantic = "deny"`、`unwrap_used` / `expect_used` / `as_conversions` / `indexing_slicing` / `missing_docs` 均为 `deny`。
4. **命名守卫**：CI 的 `naming-guard` 作业从仓库变量 `vars.NAMING_DENYLIST` 读取禁用词正则并扫描全仓；变量未设置时作业失败。
