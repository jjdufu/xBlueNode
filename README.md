# xBlueNode / AxPoint — 节点智能与动态超图平台

> **xBlueNode**（网络、动态图谱与业务节点拓扑工作台）× **AxPoint**（AI + x + Point，Rust 高性能算法、编排与推演底座）

本项目已完成 **Phase 1–3 核心全栈架构**搭建，采用 **Rust `1.100.0` (`edition = "2024"`) + PostgreSQL (`pgvector`) + Python FastAPI Sidecar (`AxParser`) + Vite/React (`shadcn/ui` 新瑞士极简风)** 架构。

---

## 一、核心模块体系

- **xBlueNode 前端工作台 (`xbluenode-web`)**：
  - **系统概览**：平台实时指标（`Doc / Para / Entity / Edge / Embedding`）与多维度兼容状态。
  - **AxIngest · 知识入库**：上传 PDF/Word/Markdown/图像或粘贴文本，构建三级超图。
  - **AxQuery · 五路问答**：并行触发 `Vector / BM25 / EntityLink / Context / MultiHop` 五路召回与 RRF 融合回答。
  - **xBlueGraph · 拓扑视图**：超图实体与文档关系网络浏览器。
  - **xBlueStudio · 节点建模**：定义业务节点类型（`BlueNodeType`）、因果关系（`BlueEdgeType`）与聚合规则。
  - **AxFlow · 编排流水线**：串联 `INGEST → TRANSFORM → NODE_MAP → ACTION` 的可运行业务流水线。
  - **AxSim · 因果推演**：基于写时复制（CoW）克隆生产节点快照，在隔离分支试算 What-If 因果传导。

---

## 二、目录结构

```text
xBlueNode-AxPoint/
├── IRON_RULES.md          # Monorepo 分模块并行开发与自主命名铁律（必读）
├── .github/               # CODEOWNERS、PR 模板与按路径触发的 CI 工作流
├── axpoint-core/          # Rust 1.100.0 Cargo Workspace（严格 clippy::pedantic / forbid(unsafe_code)）
│   └── crates/
│       ├── axpoint-domain # 领域模型、端口 Trait（ports）、切分/RRF 融合与因果推演引擎
│       ├── axpoint-llm    # OpenAI 兼容 LLM、EmbeddingAdapter 与 AiPort 实现
│       ├── axpoint-db     # PostgreSQL 三级超图、xb_*/ax_* 持久化及 Repository 实现
│       └── axpoint-server # Axum HTTP 服务（按业务域拆分 routes/{system,documents,xblue,axflow,axsim}）
├── axpoint-parser/        # AxParser: Python FastAPI 无状态解析与连接器微服务
│   ├── parsers/           # 文档解析引擎（Docling / pypdf 等）
│   ├── connectors/        # 异构数据源连接器（SQL/NoSQL/API 等）
│   ├── exporters/         # 结果导出器（CSV/XLSX/PDF/Markdown 等）
│   └── tests/             # Sidecar 单元测试
├── xbluenode-web/         # Vite + React + TypeScript + Tailwind CSS（Geist 黑白灰极简工作台）
│   └── src/
│       ├── lib/api/       # 按后端路由域拆分的强类型 API 客户端（client/rag/xblue）
│       └── modules/       # 导航与模块注册表（registry.ts）
├── deploy/                # docker-compose.yml、.env.example 与数据库增量迁移
│   └── migrations/        # 增量 SQL 迁移脚本（001_init_core.sql 起步，只增不改）
└── docs/                  # API_CONTRACT.md、TEAM_MULTI_AGENT_MANUAL.md 与 blueprint/ 详尽落地实践蓝图
```

---

## 三、快速启动指南

### 1. 启动 PostgreSQL (`pgvector`)
```bash
cd deploy
cp .env.example .env
# 编辑 .env 填入你的 LLM_API_KEY / EMBED_API_KEY（支持阿里云百炼、DeepSeek 或本地 Ollama/vLLM）
docker compose up -d
```

### 2. 启动 Python 文档解析 Sidecar (`axpoint-parser`)
```bash
cd axpoint-parser
source .venv/bin/activate
# 如需启用完整 Docling + EasyOCR 版面/表格/公式识别，可执行：
# uv pip install -e ".[docling]"
uvicorn main:app --host 0.0.0.0 --port 8001
```

### 3. 启动 Rust 核心服务 (`axpoint-core`)
```bash
cd axpoint-core
# 使用本地已安装的 Rust 1.100.0 工具链运行（自带托管 xbluenode-web/dist 静态资源）
cargo run -p axpoint-server
```
启动后直接访问：`http://localhost:9995/` 即可打开 **xBlueNode** 极简工作台！

### 4. 前端开发模式（可选）
```bash
cd xbluenode-web
npm run dev
```

---

## 四、核心特性说明

1. **多维度 Embedding 兼容层**：`hg_embeddings` 表采用不锁死维度的 `vector` 类型，同时为 `768`/`1024`/`1536` 维建立条件部分 HNSW 索引；`EmbeddingAdapter` 自动探测维度，可随时切换本地 `bge-m3`（1024 维）或云端 `text-embedding-v4`（1536 维）而无需迁移数据库。
2. **五路并行召回 + RRF 融合**：每次提问并发触发 `Vector`、`BM25`、`EntityLink`、`Context (NEXT_PARA)`、`MultiHop (RELATES)` 五路召回，并在前端右侧抽屉完整展示溯源证据与推理路径。
3. **最高级别 Rust 代码规范**：全 Workspace 启用 `unsafe_code = "forbid"`、`clippy::pedantic = "deny"`、`unwrap_used = "deny"`、`expect_used = "deny"`、`as_conversions = "deny"`、`indexing_slicing = "deny"` 与 `missing_docs = "deny"`。
