# 07 · AxGuard 企业级权限、动态脱敏与生产部署验收规范（Step 12）

本文件定义 **AxGuard 企业级安全治理底座（`axpoint-security`）** 与 **生产级部署/全链路回归验收体系**，落实平台“权限细、能脱敏、可审计、可观测、轻量化稳定交付”的最终工程标准。

---

## Step 12：JWT/OIDC 认证、细粒度 RBAC、动态脱敏与生产部署

### 12.1 安全架构原则
1. **统一身份边界**：除 `/api/health`、`/api/auth/login` 与显式签发且仅暴露脱敏数据的 `/api/xblueview/share/{token}` 外，所有 `/api/*` 与 `/api/mcp/*` 路由均经过认证中间件校验（提取 `AuthPrincipal{user_id, tenant_id, roles, dept_id}`，取代前端可随意传递的裸 `tenant_id` 参数）。
2. **四级细粒度 RBAC**：
   - **Tenant 级**：多租户物理逻辑隔离。
   - **Module/Action 级**：`DATA_READ` / `DATA_SYNC` / `STUDIO_EDIT` / `STUDIO_PUBLISH` / `FLOW_RUN` / `APPROVAL_DECIDE` / `SIM_RUN` / `EXPORT_DATA` / `ADMIN`。
   - **Resource/Node 级**：按业务节点类型或行级属性条件（例如：`dept = '华东区'`）过滤 `xb_nodes`、`ds_datasets` 与 `hg_docs`。
   - **Field/Column 级**：依据 `ds_dataset_columns.pii_category` 与 `xb_node_types.properties[].is_pii` 决定字段是否可见或必须脱敏。
3. **三道强制脱敏拦截门（Zero-Leak Policy）**：
   - **Gate A（查询与制表输出前）**：`axpoint-db` 与 `axpoint-analytics` 返回数据至前端前，根据角色权限对 PII 列执行统一脱敏。
   - **Gate B（送入 LLM Prompt 前）**：在 [`AiGateway`](file:///home/antigravity/xBlueNode-AxPoint/axpoint-core/crates/axpoint-llm/src/lib.rs) 发送任何文档片段、数据库样例值或分析结果给外部大模型 API 前，强制执行 PII 替换脱敏（防止企业敏感信息外泄至第三方模型）。
   - **Gate C（出品导出与分享前）**：Excel / PDF / Word / PPT / 公开分享页严格继承当前用户/分享令牌的脱敏视图。

### 12.2 数据库 Schema（`deploy/migrations/006_axguard_security.sql`）

```sql
-- 1. 用户表（支持本地密码 Argon2id 哈希或外部 OIDC 映射）
CREATE TABLE IF NOT EXISTS sec_users (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id     TEXT NOT NULL DEFAULT 'default',
    username      TEXT NOT NULL,
    display_name  TEXT NOT NULL,
    email         TEXT NOT NULL DEFAULT '',
    dept_name     TEXT NOT NULL DEFAULT 'default',
    password_hash TEXT NOT NULL,
    status        TEXT NOT NULL DEFAULT 'ACTIVE', -- 'ACTIVE' | 'DISABLED'
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, username)
);

-- 2. 角色定义表
CREATE TABLE IF NOT EXISTS sec_roles (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id   TEXT NOT NULL DEFAULT 'default',
    name        TEXT NOT NULL, -- 'ADMIN' | 'ANALYST' | 'OPERATOR' | 'VIEWER' | 自定义角色
    permissions JSONB NOT NULL DEFAULT '[]'::jsonb, -- 权限动作字符串数组
    can_view_raw_pii BOOLEAN NOT NULL DEFAULT FALSE,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, name)
);

-- 3. 用户-角色关联表
CREATE TABLE IF NOT EXISTS sec_user_roles (
    user_id UUID NOT NULL REFERENCES sec_users(id) ON DELETE CASCADE,
    role_id UUID NOT NULL REFERENCES sec_roles(id) ON DELETE CASCADE,
    PRIMARY KEY (user_id, role_id)
);

-- 4. 行级/资源级策略与动态脱敏规则表
CREATE TABLE IF NOT EXISTS sec_policies (
    id             UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id      TEXT NOT NULL DEFAULT 'default',
    role_id        UUID NOT NULL REFERENCES sec_roles(id) ON DELETE CASCADE,
    resource_kind  TEXT NOT NULL, -- 'DATASET' | 'NODE_TYPE' | 'DOC' | 'DASHBOARD'
    resource_key   TEXT NOT NULL, -- 具体名称或 '*'
    row_filter_key TEXT,          -- 例如 'region' / 'dept'
    row_filter_val TEXT,          -- 例如 '华东'
    mask_rules     JSONB NOT NULL DEFAULT '{}'::jsonb, -- {col_or_prop: 'MASK_PHONE'|'MASK_ID_CARD'|'MASK_EMAIL'|'HASH'|'REDACT'}
    created_at     TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- 5. 全链路安全与操作审计日志表
CREATE TABLE IF NOT EXISTS sec_audit_logs (
    id            BIGSERIAL PRIMARY KEY,
    tenant_id     TEXT NOT NULL,
    user_id       UUID,
    username      TEXT NOT NULL,
    action        TEXT NOT NULL, -- 'LOGIN' | 'QUERY_SQL' | 'RAG_ASK' | 'EXPORT_FILE' | 'APPROVE_FLOW' | 'SIM_APPLY'
    resource_ref  TEXT NOT NULL DEFAULT '',
    status_code   INT NOT NULL,
    client_ip     TEXT NOT NULL DEFAULT '',
    detail_json   JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_sec_audit_lookup ON sec_audit_logs (tenant_id, created_at DESC);
```

### 12.3 Rust 安全 Crate 设计（`axpoint-core/crates/axpoint-security/`）

```text
axpoint-core/crates/axpoint-security/
├── Cargo.toml
└── src/
    ├── lib.rs          # AuthPrincipal 提取器（Axum FromRequestParts）与 JWT 签发校验
    ├── password.rs     # Argon2id 密码哈希与校验
    ├── rbac.rs         # 权限校验与行级过滤谓词生成器
    ├── masking.rs      # 高性能确定性脱敏引擎（手机/身份证/邮箱/银行卡/自定义掩码/SHA256截断）
    └── audit.rs        # 异步非阻塞审计日志落库通道（tokio::mpsc 批量写入）
```

#### 脱敏算法规范（`masking.rs`）
| 脱敏策略 | 输入示例 | 输出示例 | 适用场景 |
| :--- | :--- | :--- | :--- |
| `MASK_PHONE` | `13812345678` | `138****5678` | 手机号 / 联系电话 |
| `MASK_ID_CARD` | `310101199001011234` | `3101**********1234` | 身份证 / 证件号 |
| `MASK_EMAIL` | `alice.wang@corp.com` | `a***@corp.com` | 电子邮箱 |
| `MASK_BANK_CARD` | `6222021234567890123` | `6222***********0123` | 银行卡 / 账号 |
| `HASH` | `secret-token` | `h_9f86d081` | 需保留关联去重能力的唯一标识 |
| `REDACT` | 任意敏感文本 | `[REDACTED]` | 密钥、高密备注送入 LLM 前 |

---

### 12.4 可观测性与生产部署规范

#### 1. 镜像规划与交付架构（`deploy/docker-compose.yml`）
平台采用**单后端二进制 + 轻量 Sidecar + 关系向量库**的三容器极简交付架构，生产默认包含以下服务与健康检查探针：

| 服务名 | 镜像地址规范 | 运行时与交付形态 | 核心端口与共享卷 |
| :--- | :--- | :--- | :--- |
| `postgres` | `pgvector/pgvector:pg16` | PostgreSQL 16 + `pgvector`，挂载 `./migrations` 自动幂等建表 | `5432`；卷 `pgdata` |
| `axpoint-core` | `${AXPOINT_REGISTRY:-dex2all}/axpoint-core:${AXPOINT_VERSION:-v0.1.0}` | 多阶段构建 Rust 1.100.0 精简镜像（单一二进制 `axpoint-server` + 同镜像静态目录 `WEB_DIST_DIR` 托管 `xbluenode-web/dist`） | `9995`；共享卷 `axpoint_datasets:/var/lib/axpoint/datasets` |
| `axpoint-parser` | `${AXPOINT_REGISTRY:-dex2all}/axpoint-parser:${AXPOINT_PARSER_VERSION:-v0.1.0}` | Python 3.11 + FastAPI + 连接器 + 出品引擎（支持 `-lite` 结构化轻量版与 `-full` 完整 Docling/EasyOCR 版） | `8001`；共享卷 `axpoint_datasets` 与 OCR 模型卷 |
| `reverse-proxy` | `caddy:2-alpine` 或 `nginx:alpine`（可选） | 统一 TLS 终止、Brotli/Gzip 压缩、大文件上传超时与 SSE 流式关闭缓冲 | `80` / `443` |

##### 镜像发布与企业交付规范
1. **双仓库地址与版本对齐**：标准 Docker Hub 推送路径为 `dex2all/axpoint-core:<tag>` 与 `dex2all/axpoint-parser:<tag>`；通过 `AXPOINT_REGISTRY` 变量可无缝切换至企业内网私有 Harbor/ACR。两个镜像严格保持同步语义化版本（如 `v0.1.0` + `v0.1.0-g<short_sha>`），确保 Parquet 契约与 HTTP 接口版本一致。
2. **`axpoint-parser` 分级镜像标签**：
   - `dex2all/axpoint-parser:<tag>-lite`：包含 `CSV/Excel/JSON/XML/Logs/pypdf`、SQL/NoSQL/API 连接器、DuckDB 与全格式出品引擎，体积极小，适合 CI 与纯结构化分析场景。
   - `dex2all/axpoint-parser:<tag>-full`（默认）：集成 `Docling + EasyOCR` 深度版面与公式识别能力；OCR 权重支持通过持久化卷挂载，避免基础镜像频繁膨胀。
3. **多架构构建**：正式发版通过 `docker buildx build --platform linux/amd64,linux/arm64` 推送多架构镜像索引，统一兼容开发端、主流 x86_64 服务器与 ARM64 信创节点。
4. **企业级前端静态托管原则**：
   - 前端 `xbluenode-web/dist` 在多阶段构建的临时 `node:24` 阶段编译完成后，直接拷贝进 `axpoint-core` 镜像文件系统并由 `ServeDir`（`WEB_DIST_DIR`）托管，**不使用 `rust-embed` 硬编码进二进制字节**——既实现单容器零跨域交付，又允许企业客户通过挂载外部卷替换定制 Logo、登录页或静态热修资源。
   - 若未来演进为公网大规模 SaaS / CDN 动静分离架构，只需把 `xbluenode-web/dist` 交由 CDN 或独立 Nginx 托管并将 `/api/*` 反代至 `axpoint-core:9995`，前后端代码无需任何改造；引入前端 URL 子路由时须同步配置 `index.html` Fallback 与资源缓存策略（`/assets/*` 长期缓存、`index.html` 协商缓存）。

#### 2. 结构化可观测性（`axpoint-server`）
- `GET /api/metrics`：暴露 Prometheus 格式指标（HTTP QPS/P95 延迟、DuckDB 查询耗时、五路召回耗时分布、Flow 执行成功率、LLM Token 与失败率计数）。
- 结构化 JSON 日志（`tracing_subscriber::fmt().json()`），每条请求注入 `trace_id` 贯穿 Rust Server → Postgres → Python Parser。

---

### 12.5 Step 12 与全平台总验收门禁（End-to-End Acceptance Suite）

在发布前必须通过以下 **10 项全链路端到端自动验收测试**：

```bash
# 1. 编译与代码质量全量检查
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
pytest -v axpoint-parser/tests
cd xbluenode-web && npm run build

# 2. 容器化全栈拉起与健康检查
docker compose -f deploy/docker-compose.yml up -d --build
curl -sf http://127.0.0.1:9995/api/health | jq .
curl -sf http://127.0.0.1:8001/health | jq .
```

#### 全链路业务闭环验证清单
1. **[鉴权与越权拦截]** 未携带 Token 访问 `/api/stats` 必须返回 `401`；`VIEWER` 角色访问 `POST /api/xblue/schema` 必须返回 `403`。
2. **[全格式接入与双轨入库]** 分别上传 `.csv`、`.xlsx`（多 Sheet）、`.pptx`、`.docx`、`.pdf`、`.json`、`.xml`、`.log` 文件，均返回 `200`，文本内容进入超图且结构化表落盘为 Parquet 并完成 PII 自动标记。
3. **[数据库/API 连接与增量同步]** 配置外部 SQL 表与 REST API 连接，完成探索、首次快照与增量水位线同步，凭证密文存储不回显。
4. **[分析制表与智能问数]** 通过 `/api/analytics/pivot` 完成多维透视；通过 `/api/analytics/ask` 提问返回正确聚合数值表与图表配置；注入恶意 SQL 返回 `400`。
5. **[脱敏三道防线]** 使用普通分析员账号查询含手机号/身份证的数据集，返回表格、LLM 问答上下文以及导出的 Excel 中均显示 `138****5678` 掩码，而特权管理员可查看原文且 `sec_audit_logs` 留存记录。
6. **[xBlueStudio 建模与动态聚合]** 在画布确认 AI 生成的模型草案并发布，绑定数据集映射执行后，上游父节点聚合指标自动重算并写入 `xb_metric_series`。
7. **[AxFlow 编排与人机审批]** 运行串联 `SOURCE_INGEST -> SQL_TRANSFORM -> XBLUE_MAP -> HUMAN_APPROVAL -> ACTION_EMIT` 的流水线，挂起后批准恢复至 `SUCCEEDED`。
8. **[API 与 MCP 能力发布]** 通过 `/api/mcp/default` 调用已发布 Tool 与内置 4 个平台 Tool，返回合规 JSON-RPC 2.0 结果。
9. **[AxSim 因果推演与策略对比]** 在隔离沙盘运行 2 个带环路的对比策略分支，输出横向 Diff 表，点击 `/apply` 生成审批单并确认回写生产，验证推演期间生产数据零污染。
10. **[xBlueView 看板与全格式出品]** 装配包含 KPI、透视表、ECharts 折线图、拓扑图的驾驶舱并开启 `/share/{token}` 访问；分别成功下载可用 `.xlsx`、`.csv`、`.pdf`、`.docx`、`.pptx` 出品文件。
