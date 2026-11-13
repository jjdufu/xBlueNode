# 01 · Step 00：仓库基线、验证基础设施与 AxGuard 骨架

Step 00 把仓库从“可演示骨架”变成“每一步都能自动验证、从第一步起就有身份与租户隔离”的工程底座。后续所有 Step 都以本步交付的迁移执行器、测试数据库、门禁脚本、任务队列、事件外发表、身份提取器和前端路由为前提。

---

## 00.1 目标

1. 数据库迁移由 sqlx 统一执行并记录版本；Core 启动时自动迁移。
2. 身份只来自令牌：所有业务接口通过 `AuthPrincipal` 取得租户；PostgreSQL 行级安全（RLS）在数据库层兜底租户隔离。
3. 提供跨模块复用的基础设施：持久化任务队列 `ax_jobs`、事件外发表 `ax_outbox`、审计写入器、统一错误体、请求追踪与指标。
4. 全栈容器化：`postgres`、`axpoint-core`、`axpoint-parser` 三个必选服务可一条命令拉起并通过健康检查。
5. 门禁框架：`scripts/dev-up.sh`、`scripts/seed-demo.sh`、`scripts/gate/*.sh`、测试样本生成器、CI 数据库服务、前端页面冒烟。

## 00.2 前置条件

- 工具：Docker 24+（含 compose 插件）、Rust `1.100.0`（含 clippy、rustfmt）、`sqlx-cli 0.9.x`、Python 3.11、`uv 0.5+`、Node 24、`jq`、`openssl`、`psql`（PostgreSQL 客户端 16+）。
- 仓库处于 [00 §3](./00-MASTER-INDEX.md) 描述的基线状态。

## 00.3 契约清单

| 契约 | 位置 |
| :--- | :--- |
| 迁移 `0002_platform_foundation.sql` | `deploy/migrations/` |
| `AppError` 全集、`enums.rs`（`Permission`、`JobKind`、`JobStatus`、`OutboxTopic`、`AuditAction`） | `axpoint-domain` |
| 端口 `JobQueuePort`、`OutboxPort`、`AuditSink`、`CredentialStore` | `axpoint-domain/src/ports.rs` |
| `GET /api/auth/me`、`GET /api/axops/jobs`、`GET /api/axops/jobs/{id}`、错误体三字段 | `docs/API_CONTRACT.md` |
| `lib/api/auth.ts`、`lib/api/enums.ts` | `xbluenode-web` |

## 00.4 文件变更清单

| 动作 | 路径 | 内容 |
| :--- | :--- | :--- |
| 新建 | `deploy/migrations/0002_platform_foundation.sql` | 角色、RLS 辅助函数、`sec_*` 基础表、`ax_jobs`、`ax_outbox`、`ax_retention_policies`、`sec_tenants` |
| 修改 | `deploy/migrations/README.md` | sqlx 编号规则 |
| 修改 | `deploy/docker-compose.yml` | 三个服务、健康检查、内部网络、数据卷；去掉 initdb 迁移挂载 |
| 新建 | `deploy/docker-compose.test.yml` | 集成测试辅助服务（Step 02 起使用） |
| 新建 | `deploy/docker/postgres-init.sh` | 首次建库时创建 `axpoint_app`、`axpoint_lake` 并设置口令 |
| 新建 | `deploy/docker/axpoint-core.Dockerfile` | 多阶段：web 构建 → Rust 构建 → distroless 运行 |
| 新建 | `deploy/docker/axpoint-parser.Dockerfile` | `lite` / `full` 两个 target |
| 修改 | `deploy/.env.example` | 补齐 [A1 §10](./A1-CONVENTIONS-ENUMS-ROUTES.md) 的全部变量 |
| 修改 | `axpoint-core/Cargo.toml` | 成员 `axpoint-security`；依赖见 [A6](./A6-DEPENDENCY-BASELINE.md) Step 00 行 |
| 新建 | `axpoint-core/crates/axpoint-security/{Cargo.toml, src/lib.rs, src/principal.rs, src/authenticator.rs, src/audit.rs}` | 身份与审计骨架 |
| 新建 | `axpoint-core/crates/axpoint-domain/src/enums.rs` | 公共枚举 |
| 修改 | `axpoint-core/crates/axpoint-domain/src/lib.rs` | `AppError` 全集、`pub mod enums` |
| 修改 | `axpoint-core/crates/axpoint-domain/src/ports.rs` | 四个基础端口 |
| 新建 | `axpoint-core/crates/axpoint-db/src/tenant.rs` | `begin_tenant_tx` |
| 新建 | `axpoint-core/crates/axpoint-db/src/{jobs.rs, outbox.rs, audit.rs, tokens.rs}` | 端口实现 |
| 修改 | `axpoint-core/crates/axpoint-db/src/{lib.rs, topology.rs}` | 全部查询改走 `begin_tenant_tx` |
| 修改 | `axpoint-core/crates/axpoint-server/src/main.rs` | 迁移、遥测、后台任务、SPA 托管、`healthcheck` 子命令 |
| 新建 | `axpoint-core/crates/axpoint-server/src/{auth.rs, telemetry.rs, background.rs, spa.rs}` | 身份提取器、遥测、任务 Worker 与外发分发、静态托管 |
| 修改 | `axpoint-core/crates/axpoint-server/src/state.rs` | 删除 `TenantQuery` 与 `default_tenant`；`HttpError` 三字段错误体 |
| 修改 | `axpoint-core/crates/axpoint-server/src/routes/{mod.rs, system.rs, documents.rs, xblue.rs, axflow.rs, axsim.rs}` | 处理器参数改为 `AuthPrincipal`；`/api` JSON 404 回退 |
| 新建 | `axpoint-core/crates/axpoint-server/src/routes/auth.rs` | `GET /api/auth/me` |
| 新建 | `axpoint-core/crates/axpoint-server/src/routes/axops.rs` | `GET /api/axops/jobs`、`GET /api/axops/jobs/{id}`（任务查询，门禁脚本 `wait_job` 依赖） |
| 修改 | `axpoint-parser/pyproject.toml` | `pydantic-settings`、顶层模块声明与子包自动发现 |
| 新建 | `axpoint-parser/{settings.py, security.py}` | 配置与内部令牌中间件 |
| 修改 | `axpoint-parser/main.py` | 挂载中间件、统一错误体 |
| 新建 | `axpoint-parser/tests/fixtures/make_fixtures.py`、`fixtures.sha256` | 样本生成器（规格见 [A5 §1](./A5-TEST-FIXTURES-AND-GATES.md)） |
| 新建 | `axpoint-parser/tests/test_security_token.py` | 内部令牌用例 |
| 修改 | `xbluenode-web/package.json` | `react-router`、`@tanstack/react-query`、`zod`、`@playwright/test` |
| 修改 | `xbluenode-web/src/{App.tsx, main.tsx, modules/registry.ts}` | 路由化、注册表含 `path` |
| 修改 | `xbluenode-web/src/lib/api/{client.ts, rag.ts, xblue.ts}` | 携带令牌、去掉 `tenant_id` 参数 |
| 新建 | `xbluenode-web/src/lib/api/{auth.ts, enums.ts}`、`src/pages/LoginPage.tsx` | 登录与公共枚举 |
| 新建 | `xbluenode-web/{playwright.config.ts, e2e/smoke.spec.ts}` | 页面冒烟 |
| 新建 | `scripts/{dev-up.sh, dev-down.sh, start-services.sh, seed-demo.sh}`、`scripts/seed/00-baseline.sh` | 环境、本地服务与演示数据 |
| 新建 | `scripts/gate/{lib.sh, lib_test.sh, step00.sh, all.sh}` | 门禁框架与断言自测 |
| 修改 | `.github/workflows/ci.yml` | `rust-core` 增加 `services.postgres` 与 `sqlx migrate run`；`web-frontend` 增加 Playwright |
| 修改 | `.gitignore` | `.gate/`、`.env.test`、`xbluenode-web/test-results/`、`xbluenode-web/playwright-report/` |

---

## 00.5 迁移执行器

### 00.5.1 规则

- 文件名 `NNNN_<module>.sql`，`NNNN` 为四位递增整数；`0001_init_core.sql` 为基线。完整清单见 [A2](./A2-DATABASE-MIGRATIONS-INDEX.md)。
- 已有表只用 `ALTER TABLE ... ADD COLUMN IF NOT EXISTS`、`CREATE INDEX IF NOT EXISTS` 扩展；只有新表使用 `CREATE TABLE IF NOT EXISTS`。
- 每个含 `tenant_id` 的新表在同一迁移末尾调用 `SELECT ax_enable_tenant_rls('<table>');`。
- 唯一执行入口：
  - 命令行：`sqlx migrate run --source deploy/migrations --database-url "$MIGRATION_DATABASE_URL"`；
  - Core 启动：`AXPOINT_MIGRATE_ON_START=true` 时用 `MIGRATION_DATABASE_URL` 建立一次性连接执行 `sqlx::migrate!("../../../deploy/migrations")`，完成后关闭该连接，业务连接池使用 `DATABASE_URL`（角色 `axpoint_app`）。
- 迁移属主角色（compose 中的 `axpoint`）必须是超级用户或具备 `BYPASSRLS`，因为 `SECURITY DEFINER` 函数需要跨租户扫描任务表与外发表。

### 00.5.2 `deploy/migrations/0002_platform_foundation.sql`

```sql
-- 0002_platform_foundation.sql：角色、租户行级安全、身份基础表、任务队列、事件外发表、保留策略

DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'axpoint_app') THEN
        CREATE ROLE axpoint_app LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOBYPASSRLS;
    END IF;
    IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'axpoint_lake') THEN
        CREATE ROLE axpoint_lake LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOBYPASSRLS;
    END IF;
END $$;

GRANT USAGE ON SCHEMA public TO axpoint_app;
CREATE SCHEMA IF NOT EXISTS lake_catalog AUTHORIZATION axpoint_lake;

-- 租户注册表（平台级，不受 RLS 约束，应用只读）
CREATE TABLE IF NOT EXISTS sec_tenants (
    tenant_id    TEXT PRIMARY KEY CHECK (tenant_id ~ '^[a-z0-9_-]{1,64}$'),
    display_name TEXT NOT NULL,
    timezone     TEXT NOT NULL DEFAULT 'Asia/Shanghai',
    status       TEXT NOT NULL DEFAULT 'ACTIVE' CHECK (status IN ('ACTIVE', 'DISABLED')),
    created_at   TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
GRANT SELECT ON sec_tenants TO axpoint_app;
INSERT INTO sec_tenants (tenant_id, display_name) VALUES ('default', '默认租户') ON CONFLICT DO NOTHING;

-- 为带 tenant_id 的表开启并强制 RLS，创建统一隔离策略，授予应用角色 DML 权限
CREATE OR REPLACE FUNCTION ax_enable_tenant_rls(tbl regclass) RETURNS void
LANGUAGE plpgsql AS $$
DECLARE
    rel_name TEXT := (SELECT relname FROM pg_class WHERE oid = tbl);
BEGIN
    EXECUTE format('ALTER TABLE %s ENABLE ROW LEVEL SECURITY', tbl);
    EXECUTE format('ALTER TABLE %s FORCE ROW LEVEL SECURITY', tbl);
    IF NOT EXISTS (
        SELECT 1 FROM pg_policies
        WHERE schemaname = 'public' AND tablename = rel_name AND policyname = 'p_tenant_isolation'
    ) THEN
        EXECUTE format(
            'CREATE POLICY p_tenant_isolation ON %s '
            'USING (tenant_id = current_setting(''app.tenant_id'', true)) '
            'WITH CHECK (tenant_id = current_setting(''app.tenant_id'', true))', tbl);
    END IF;
    EXECUTE format('GRANT SELECT, INSERT, UPDATE, DELETE ON %s TO axpoint_app', tbl);
END $$;

SELECT ax_enable_tenant_rls(t) FROM unnest(ARRAY[
    'hg_docs', 'hg_paras', 'hg_entities', 'hg_edges', 'hg_embeddings',
    'xb_node_types', 'xb_edge_types', 'xb_nodes', 'xb_links', 'ax_flows', 'ax_sandboxes'
]::regclass[]) AS t;

-- 身份基础表
CREATE TABLE IF NOT EXISTS sec_users (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id     TEXT NOT NULL REFERENCES sec_tenants(tenant_id),
    username      TEXT NOT NULL,
    display_name  TEXT NOT NULL,
    email         TEXT NOT NULL DEFAULT '',
    dept_id       TEXT NOT NULL DEFAULT 'default',
    password_hash TEXT,
    auth_source   TEXT NOT NULL DEFAULT 'LOCAL' CHECK (auth_source IN ('LOCAL', 'OIDC')),
    status        TEXT NOT NULL DEFAULT 'ACTIVE' CHECK (status IN ('ACTIVE', 'DISABLED')),
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, username)
);

CREATE TABLE IF NOT EXISTS sec_roles (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id   TEXT NOT NULL REFERENCES sec_tenants(tenant_id),
    name        TEXT NOT NULL,
    permissions TEXT[] NOT NULL DEFAULT '{}',
    is_builtin  BOOLEAN NOT NULL DEFAULT FALSE,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, name)
);

CREATE TABLE IF NOT EXISTS sec_user_roles (
    tenant_id TEXT NOT NULL,
    user_id   UUID NOT NULL REFERENCES sec_users(id) ON DELETE CASCADE,
    role_id   UUID NOT NULL REFERENCES sec_roles(id) ON DELETE CASCADE,
    PRIMARY KEY (user_id, role_id)
);

-- 个人访问令牌（API / MCP 调用）。明文格式 axp_<prefix8>_<secret32>，只在创建时返回一次
CREATE TABLE IF NOT EXISTS sec_api_tokens (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id    TEXT NOT NULL,
    user_id      UUID NOT NULL REFERENCES sec_users(id) ON DELETE CASCADE,
    name         TEXT NOT NULL,
    token_prefix TEXT NOT NULL,
    token_hash   TEXT NOT NULL UNIQUE,          -- hex(SHA-256(明文令牌))
    scopes       TEXT[] NOT NULL DEFAULT '{}',  -- Permission 子集；空数组 = 继承用户权限
    expires_at   TIMESTAMPTZ,
    last_used_at TIMESTAMPTZ,
    revoked_at   TIMESTAMPTZ,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- 审计日志（只追加）
CREATE TABLE IF NOT EXISTS sec_audit_logs (
    id           BIGSERIAL PRIMARY KEY,
    tenant_id    TEXT NOT NULL,
    user_id      UUID,
    username     TEXT NOT NULL,
    action       TEXT NOT NULL CHECK (action IN (
        'LOGIN','LOGOUT','TOKEN_ISSUE','TOKEN_REVOKE','QUERY_SQL','ASK_DATA','RAG_ASK','DATA_SYNC',
        'FILE_UPLOAD','SCHEMA_PUBLISH','FLOW_RUN','APPROVAL_DECIDE','MCP_CALL','AGENT_CHAT','SIM_APPLY',
        'EXPORT_FILE','SHARE_CREATE','SHARE_ACCESS','POLICY_CHANGE','BACKUP_RUN','TEMPLATE_INSTALL')),
    resource_ref TEXT NOT NULL DEFAULT '',
    status_code  INT NOT NULL,
    client_ip    TEXT NOT NULL DEFAULT '',
    trace_id     TEXT NOT NULL DEFAULT '',
    detail_json  JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS idx_sec_audit_lookup ON sec_audit_logs (tenant_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_sec_audit_action ON sec_audit_logs (tenant_id, action, created_at DESC);

-- 持久化任务队列（同步、解析、流程步骤、出品、规则判定、聚合、湖维护、保留清理、密钥轮换、备份、对象向量）
CREATE TABLE IF NOT EXISTS ax_jobs (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id       TEXT NOT NULL,
    kind            TEXT NOT NULL CHECK (kind IN (
        'SYNC_DATASET','PARSE_FILE','FLOW_STEP','EXPORT','RULE_EVAL','ROLLUP','LAKE_MAINTENANCE',
        'RETENTION','CREDENTIAL_ROTATE','BACKUP','EMBED_NODES','MAP_MATERIALIZE','GRAPH_AGGREGATE','KNOWLEDGE_SYNC')),
    payload         JSONB NOT NULL DEFAULT '{}'::jsonb,
    status          TEXT NOT NULL DEFAULT 'PENDING'
                    CHECK (status IN ('PENDING','RUNNING','SUCCEEDED','FAILED','CANCELED','EXPIRED')),
    priority        INT NOT NULL DEFAULT 100,
    attempt         INT NOT NULL DEFAULT 0,
    max_attempts    INT NOT NULL DEFAULT 5,
    next_run_at     TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    lease_owner     TEXT,
    lease_until     TIMESTAMPTZ,
    idempotency_key TEXT,
    last_error      TEXT,
    result          JSONB,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, kind, idempotency_key)
);
CREATE INDEX IF NOT EXISTS idx_ax_jobs_ready ON ax_jobs (priority, next_run_at)
    WHERE status IN ('PENDING', 'RUNNING');

-- 事务外发表：业务写入与事件在同一事务提交
CREATE TABLE IF NOT EXISTS ax_outbox (
    id            BIGSERIAL PRIMARY KEY,
    tenant_id     TEXT NOT NULL,
    topic         TEXT NOT NULL CHECK (topic IN (
        'dataset.synced','node.metrics_changed','node.upserted','approval.decided','sim.applied',
        'alert.opened','alert.resolved','export.finished')),
    payload       JSONB NOT NULL,
    claimed_by    TEXT,
    claimed_until TIMESTAMPTZ,
    processed_at  TIMESTAMPTZ,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS idx_ax_outbox_pending ON ax_outbox (id) WHERE processed_at IS NULL;

CREATE OR REPLACE FUNCTION ax_outbox_notify() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    PERFORM pg_notify('ax_outbox', NEW.id::text);
    RETURN NEW;
END $$;
DROP TRIGGER IF EXISTS trg_ax_outbox_notify ON ax_outbox;
CREATE TRIGGER trg_ax_outbox_notify AFTER INSERT ON ax_outbox FOR EACH ROW EXECUTE FUNCTION ax_outbox_notify();

-- 数据保留策略
CREATE TABLE IF NOT EXISTS ax_retention_policies (
    tenant_id TEXT NOT NULL,
    target    TEXT NOT NULL CHECK (target IN (
        'METRIC_SERIES','METRIC_ROLLUP_MINUTE','AUDIT','EXPORT_FILE','FLOW_STEP_LOG','JOB','OUTBOX','LAKE_SNAPSHOT','MESSAGE',
        'QUERY_HISTORY','CONVERSATION')),
    keep_days INT NOT NULL CHECK (keep_days > 0),
    PRIMARY KEY (tenant_id, target)
);

SELECT ax_enable_tenant_rls(t) FROM unnest(ARRAY[
    'sec_users', 'sec_roles', 'sec_user_roles', 'sec_api_tokens', 'sec_audit_logs',
    'ax_jobs', 'ax_outbox', 'ax_retention_policies'
]::regclass[]) AS t;

-- 审计只追加：收回修改与删除权限
REVOKE UPDATE, DELETE ON sec_audit_logs FROM axpoint_app;

-- 跨租户后台操作只通过 SECURITY DEFINER 函数进行
CREATE OR REPLACE FUNCTION ax_lease_jobs(p_worker TEXT, p_kinds TEXT[], p_limit INT, p_lease_seconds INT)
RETURNS SETOF ax_jobs LANGUAGE sql SECURITY DEFINER SET search_path = public AS $$
    UPDATE ax_jobs j
       SET status = 'RUNNING', lease_owner = p_worker,
           lease_until = NOW() + make_interval(secs => p_lease_seconds),
           attempt = j.attempt + 1, updated_at = NOW()
     WHERE j.id IN (
           SELECT id FROM ax_jobs
            WHERE kind = ANY (p_kinds)
              AND ((status = 'PENDING' AND next_run_at <= NOW())
                   OR (status = 'RUNNING' AND lease_until < NOW()))
            ORDER BY priority, next_run_at
            FOR UPDATE SKIP LOCKED
            LIMIT p_limit)
    RETURNING j.*;
$$;

CREATE OR REPLACE FUNCTION ax_claim_outbox(p_worker TEXT, p_limit INT, p_lease_seconds INT)
RETURNS SETOF ax_outbox LANGUAGE sql SECURITY DEFINER SET search_path = public AS $$
    UPDATE ax_outbox o
       SET claimed_by = p_worker, claimed_until = NOW() + make_interval(secs => p_lease_seconds)
     WHERE o.id IN (
           SELECT id FROM ax_outbox
            WHERE processed_at IS NULL AND (claimed_until IS NULL OR claimed_until < NOW())
            ORDER BY id
            FOR UPDATE SKIP LOCKED
            LIMIT p_limit)
    RETURNING o.*;
$$;

CREATE OR REPLACE FUNCTION ax_resolve_api_token(p_token_hash TEXT)
RETURNS TABLE (token_id UUID, tenant_id TEXT, user_id UUID, scopes TEXT[])
LANGUAGE sql SECURITY DEFINER SET search_path = public AS $$
    SELECT t.id, t.tenant_id, t.user_id, t.scopes
      FROM sec_api_tokens t
     WHERE t.token_hash = p_token_hash
       AND t.revoked_at IS NULL
       AND (t.expires_at IS NULL OR t.expires_at > NOW());
$$;

REVOKE ALL ON FUNCTION ax_lease_jobs(TEXT, TEXT[], INT, INT) FROM PUBLIC;
REVOKE ALL ON FUNCTION ax_claim_outbox(TEXT, INT, INT) FROM PUBLIC;
REVOKE ALL ON FUNCTION ax_resolve_api_token(TEXT) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION ax_lease_jobs(TEXT, TEXT[], INT, INT) TO axpoint_app;
GRANT EXECUTE ON FUNCTION ax_claim_outbox(TEXT, INT, INT) TO axpoint_app;
GRANT EXECUTE ON FUNCTION ax_resolve_api_token(TEXT) TO axpoint_app;

-- 内置角色与开发静态主体（默认租户）
INSERT INTO sec_roles (tenant_id, name, permissions, is_builtin) VALUES
 ('default', 'ADMIN', ARRAY['DATA_READ','DATA_SYNC','DATA_ADMIN','STUDIO_EDIT','STUDIO_PUBLISH','FLOW_EDIT','FLOW_RUN',
   'FLOW_PUBLISH','APPROVAL_DECIDE','RULE_EDIT','ALERT_ACK','AGENT_EDIT','AGENT_RUN','SIM_RUN','SIM_APPLY','VIEW_EDIT',
   'VIEW_SHARE','EXPORT_DATA','PII_RAW_READ','GUARD_ADMIN','OPS_ADMIN'], TRUE),
 ('default', 'ANALYST', ARRAY['DATA_READ','DATA_SYNC','STUDIO_EDIT','FLOW_EDIT','FLOW_RUN','AGENT_RUN','SIM_RUN',
   'VIEW_EDIT','EXPORT_DATA'], TRUE),
 ('default', 'OPERATOR', ARRAY['DATA_READ','FLOW_RUN','APPROVAL_DECIDE','ALERT_ACK','AGENT_RUN','SIM_RUN'], TRUE),
 ('default', 'VIEWER', ARRAY['DATA_READ'], TRUE)
ON CONFLICT (tenant_id, name) DO NOTHING;

INSERT INTO sec_users (id, tenant_id, username, display_name, dept_id)
VALUES ('00000000-0000-0000-0000-000000000000', 'default', 'dev', '开发管理员', 'default')
ON CONFLICT (tenant_id, username) DO NOTHING;

INSERT INTO sec_user_roles (tenant_id, user_id, role_id)
SELECT 'default', '00000000-0000-0000-0000-000000000000', id
  FROM sec_roles WHERE tenant_id = 'default' AND name = 'ADMIN'
ON CONFLICT DO NOTHING;

INSERT INTO ax_retention_policies (tenant_id, target, keep_days) VALUES
 ('default','METRIC_SERIES',730), ('default','METRIC_ROLLUP_MINUTE',30), ('default','AUDIT',1095),
 ('default','EXPORT_FILE',30), ('default','FLOW_STEP_LOG',180), ('default','JOB',30),
 ('default','OUTBOX',14), ('default','LAKE_SNAPSHOT',30), ('default','MESSAGE',365),
 ('default','QUERY_HISTORY',180), ('default','CONVERSATION',365)
ON CONFLICT DO NOTHING;

GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA public TO axpoint_app;
ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT USAGE, SELECT ON SEQUENCES TO axpoint_app;
```

### 00.5.3 `deploy/docker/postgres-init.sh`

```bash
#!/usr/bin/env bash
# 首次初始化数据目录时由 postgres 镜像执行：创建应用角色与湖目录角色并设置口令
set -euo pipefail
psql -v ON_ERROR_STOP=1 --username "$POSTGRES_USER" --dbname "$POSTGRES_DB" \
  -v app_pw="${AXPOINT_APP_DB_PASSWORD}" -v lake_pw="${AXPOINT_LAKE_DB_PASSWORD}" <<'SQL'
SELECT 'CREATE ROLE axpoint_app LOGIN NOBYPASSRLS' WHERE NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname='axpoint_app')\gexec
SELECT 'CREATE ROLE axpoint_lake LOGIN NOBYPASSRLS' WHERE NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname='axpoint_lake')\gexec
ALTER ROLE axpoint_app PASSWORD :'app_pw';
ALTER ROLE axpoint_lake PASSWORD :'lake_pw';
SQL
```

### 00.5.4 `deploy/migrations/README.md`（全文）

```markdown
# 数据库迁移规范

- 格式：sqlx，文件名 `NNNN_<module>.sql`，四位递增编号，只追加不修改。清单见 `docs/blueprint/A2-DATABASE-MIGRATIONS-INDEX.md`。
- 执行：`sqlx migrate run --source deploy/migrations --database-url "$MIGRATION_DATABASE_URL"`，或 Core 启动时自动执行（`AXPOINT_MIGRATE_ON_START=true`）。
- 已有表只用 `ALTER TABLE ... ADD COLUMN IF NOT EXISTS` 扩展；新表使用 `CREATE TABLE IF NOT EXISTS`。
- 每个带 `tenant_id` 的新表在迁移末尾执行 `SELECT ax_enable_tenant_rls('<table>');`。
- 检查：`sqlx migrate info --source deploy/migrations --database-url "$MIGRATION_DATABASE_URL"` 中不得出现 `pending`。
```

---

## 00.6 Rust：领域、身份与租户事务

### 00.6.1 `axpoint-domain/src/lib.rs` 中的 `AppError`

```rust
/// 统一错误类型。`code` 取值见 A1 §5。
#[derive(Debug, Error)]
pub enum AppError {
    /// 请求参数非法。
    #[error("{code}: {message}")]
    BadRequest { code: &'static str, message: String },
    /// 缺少或无效凭证。
    #[error("{0}")]
    Unauthorized(&'static str),
    /// 权限或策略拒绝。
    #[error("{0}")]
    Forbidden(&'static str),
    /// 资源不存在（含跨租户访问）。
    #[error("{0}")]
    NotFound(&'static str),
    /// 冲突。
    #[error("{0}")]
    Conflict(&'static str),
    /// 请求体超限或压缩炸弹。
    #[error("{0}")]
    PayloadTooLarge(&'static str),
    /// 不支持的格式。
    #[error("{0}")]
    Unsupported(&'static str),
    /// 语义上无法处理。
    #[error("{code}: {message}")]
    Unprocessable { code: &'static str, message: String },
    /// 限流。
    #[error("rate limited")]
    RateLimited,
    /// 超时。
    #[error("{0}")]
    Timeout(&'static str),
    /// 配置错误。
    #[error("configuration error: {0}")]
    Config(String),
    /// 数据库错误。
    #[error("database error: {0}")]
    Database(String),
    /// 大模型上游错误。
    #[error("llm error: {0}")]
    Llm(String),
    /// Sidecar 上游错误。
    #[error("parser error: {0}")]
    Parser(String),
    /// 其他上游错误（连接器、Webhook、MCP）。
    #[error("upstream error: {0}")]
    Upstream(String),
}

impl AppError {
    /// 构造 `BadRequest`。
    #[must_use]
    pub fn bad(code: &'static str, message: impl Into<String>) -> Self {
        Self::BadRequest { code, message: message.into() }
    }
}
```

`axpoint-server/src/state.rs` 中 `HttpError::into_response` 按 [A1 §5](./A1-CONVENTIONS-ENUMS-ROUTES.md) 映射状态码，响应体为 `{"error_code","message","trace_id"}`；`trace_id` 取当前 `tracing` span 的 OpenTelemetry trace id，没有则取 `x-request-id`。500 类错误的 `message` 固定为 `internal error`，原始信息只写日志。

### 00.6.2 基础端口（`axpoint-domain/src/ports.rs` 追加）

```rust
/// 任务队列端口。
pub trait JobQueuePort: Send + Sync {
    /// 入队；同一 (tenant, kind, idempotency_key) 重复入队返回已有任务 ID。
    fn enqueue(&self, tenant_id: &str, job: NewJob) -> impl Future<Output = Result<Uuid, AppError>> + Send;
    /// 跨租户领取任务（SECURITY DEFINER 函数 `ax_lease_jobs`）。
    fn lease(&self, worker: &str, kinds: &[JobKind], limit: u32, lease_secs: u32)
        -> impl Future<Output = Result<Vec<LeasedJob>, AppError>> + Send;
    /// 续租。
    fn heartbeat(&self, job: &LeasedJob, lease_secs: u32) -> impl Future<Output = Result<(), AppError>> + Send;
    /// 成功完成。
    fn complete(&self, job: &LeasedJob, result: serde_json::Value) -> impl Future<Output = Result<(), AppError>> + Send;
    /// 失败：attempt < max_attempts 时按 2^attempt 秒（上限 300 秒）退避重排，否则置 FAILED。
    fn fail(&self, job: &LeasedJob, error: &str) -> impl Future<Output = Result<(), AppError>> + Send;
}

/// 事件外发端口。
pub trait OutboxPort: Send + Sync {
    /// 跨租户认领未处理事件。
    fn claim(&self, worker: &str, limit: u32, lease_secs: u32) -> impl Future<Output = Result<Vec<OutboxEvent>, AppError>> + Send;
    /// 标记已处理。
    fn ack(&self, tenant_id: &str, ids: &[i64]) -> impl Future<Output = Result<(), AppError>> + Send;
}

/// 审计落库端口。
pub trait AuditSink: Send + Sync {
    /// 批量写入审计记录（同一批次可跨租户，实现内按租户分组写入）。
    fn write_batch(&self, records: Vec<AuditRecord>) -> impl Future<Output = Result<(), AppError>> + Send;
}

/// 令牌查询端口。
pub trait CredentialStore: Send + Sync {
    /// 按令牌哈希解析个人访问令牌。
    fn resolve_api_token(&self, token_hash: &str) -> impl Future<Output = Result<Option<ApiTokenRecord>, AppError>> + Send;
    /// 读取用户与其角色权限并集。
    fn load_principal(&self, tenant_id: &str, user_id: Uuid) -> impl Future<Output = Result<Option<PrincipalRecord>, AppError>> + Send;
}
```

`NewJob`、`LeasedJob`、`OutboxEvent`、`AuditRecord`、`ApiTokenRecord`、`PrincipalRecord` 定义在 `axpoint-domain/src/platform.rs`，字段与 0002 的列一一对应。

### 00.6.3 租户事务（`axpoint-db/src/tenant.rs`）

```rust
//! Tenant-scoped transactions that activate PostgreSQL row level security.

use axpoint_domain::AppError;
use sqlx::{PgPool, Postgres, Transaction};

/// 开启事务并设置 `app.tenant_id`，事务结束后设置自动失效。
///
/// # Errors
/// 连接或设置失败时返回 `AppError::Database`。
pub async fn begin_tenant_tx<'a>(pool: &'a PgPool, tenant_id: &str) -> Result<Transaction<'a, Postgres>, AppError> {
    let mut tx = pool.begin().await.map_err(|e| AppError::Database(e.to_string()))?;
    sqlx::query("SELECT set_config('app.tenant_id', $1, true)")
        .bind(tenant_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| AppError::Database(e.to_string()))?;
    Ok(tx)
}
```

`PgStore` 的全部公开方法（`lib.rs`、`topology.rs` 中现有的写入、查询、召回函数）改为：首行 `let mut tx = begin_tenant_tx(&self.pool, tenant_id).await?;`，所有语句在 `&mut *tx` 上执行，末尾 `tx.commit()`。只读方法同样走事务（`SET LOCAL` 语义需要事务）。

### 00.6.4 `axpoint-security`

```text
axpoint-core/crates/axpoint-security/
├── Cargo.toml          # 依赖：axpoint-domain、serde、sha2、hex、subtle、tokio、tracing、uuid
└── src/
    ├── lib.rs          # pub use principal::*, authenticator::*, audit::*
    ├── principal.rs    # AuthPrincipal、TokenKind、require()
    ├── authenticator.rs# Authenticator<C: CredentialStore>：解析 Bearer → AuthPrincipal
    └── audit.rs        # AuditWriter：mpsc 通道 + 批量刷写（200 条或 500ms）
```

```rust
/// 已认证主体。
#[derive(Debug, Clone, Serialize)]
pub struct AuthPrincipal {
    /// 用户 ID；开发静态令牌为全零 UUID。
    pub user_id: Uuid,
    /// 租户。
    pub tenant_id: String,
    /// 用户名。
    pub username: String,
    /// 部门。
    pub dept_id: String,
    /// 角色名。
    pub roles: Vec<String>,
    /// 权限并集。
    pub permissions: BTreeSet<Permission>,
    /// 令牌类型。
    pub token_kind: TokenKind,
}

impl AuthPrincipal {
    /// 校验权限。
    ///
    /// # Errors
    /// 不具备该权限时返回 `AppError::Forbidden("PERMISSION_DENIED")`。
    pub fn require(&self, p: Permission) -> Result<(), AppError> {
        if self.permissions.contains(&p) { Ok(()) } else { Err(AppError::Forbidden("PERMISSION_DENIED")) }
    }
}
```

`Authenticator::authenticate(header_value)` 的判定顺序：

1. 缺少 `Authorization` → `Unauthorized("TOKEN_MISSING")`。
2. `AXPOINT_ENV != production` 且配置了 `AXPOINT_DEV_TOKEN`，并与请求令牌常量时间比较相等（`subtle::ConstantTimeEq`）→ `{tenant_id: "default", username: "dev", roles: ["ADMIN"], permissions: 全集, token_kind: DEV_STATIC}`。
3. 以 `axp_` 开头 → 计算 SHA-256，调用 `CredentialStore::resolve_api_token`，再 `load_principal`；`scopes` 非空时权限取交集；异步刷新 `last_used_at`。
4. 其他 → Step 13 接入 EdDSA 访问令牌校验；在 Step 13 之前返回 `Unauthorized("TOKEN_INVALID")`。

Axum 提取器写在 `axpoint-server/src/auth.rs`：`impl FromRequestParts<AppState> for AuthPrincipal`（`AppState` 是本 Crate 类型，满足孤儿规则），内部调用 `state.authenticator.authenticate(...)`，并把 `tenant_id`、`username` 记录到当前 span。

### 00.6.5 处理器改造规则

- 删除 `state.rs` 中的 `TenantQuery`、`default_tenant`；所有处理器签名增加 `principal: AuthPrincipal`，用 `principal.tenant_id` 调用存储。
- 领域请求类型中的 `tenant_id` 字段（`IngestTextRequest`、`ChatQueryRequest`、`SaveXBlueSchemaRequest`、`UpsertBlueNodeRequest`、`CreateAxFlowRequest`、`CreateAxSandboxRequest`）删除；如果客户端仍传入该字段，`serde` 以 `#[serde(deny_unknown_fields)]` 拒绝并返回 `400 INVALID_ARGUMENT`。
- 写操作的权限：`POST /api/documents/*` 需要 `DATA_SYNC`；`POST /api/xblue/schema` 需要 `STUDIO_EDIT`；`POST /api/xblue/nodes` 需要 `STUDIO_EDIT`；`POST /api/axflow/flows` 需要 `FLOW_EDIT`；`POST /api/axflow/flows/{id}/run` 需要 `FLOW_RUN`；`POST /api/axsim/*` 需要 `SIM_RUN`；读操作需要 `DATA_READ`。
- 任务查询：`GET /api/axops/jobs?kind=&status=&limit=&cursor=` 与 `GET /api/axops/jobs/{id}` 需要 `DATA_READ`，在租户事务中读取 `ax_jobs`，响应字段 `{id, kind, status, attempt, max_attempts, next_run_at, last_error, result, created_at, updated_at}`；不存在或属于其他租户时返回 404 `JOB_NOT_FOUND`。
- `routes/mod.rs`：

```rust
pub(crate) fn api_router(state: AppState) -> Router {
    let api = Router::new()
        .merge(system::router())
        .merge(auth::router())
        .merge(documents::router())
        .merge(xblue::router())
        .merge(axflow::router())
        .merge(axsim::router())
        .merge(axops::router())
        .fallback(|| async { HttpError(AppError::NotFound("ROUTE_NOT_FOUND")) });
    Router::new().nest("/api", api).with_state(state)
}
```

  子路由中的路径相应去掉 `/api` 前缀（例如 `system.rs` 中写 `.route("/health", ...)`）。`GET /api/health` 不需要令牌，其余均需要。

### 00.6.6 后台任务（`axpoint-server/src/background.rs`）

- `JobWorker`：每个进程启动 `AXPOINT_WORKERS`（默认 4）个协程，循环 `lease(worker_id, kinds, 1, 60)`；按 `JobKind` 分派到 `JobHandlerRegistry`（`HashMap<JobKind, Box<dyn JobHandler>>`，`JobHandler` 为 `axpoint-domain` 中对象安全的 trait）；执行期间每 20 秒 `heartbeat`；处理器在 `begin_tenant_tx(job.tenant_id)` 上下文中执行。Step 00 注册 `RETENTION` 处理器（按 `ax_retention_policies` 删除 `sec_audit_logs` 之外的过期 `ax_jobs`、`ax_outbox` 记录；审计日志清理在 Step 14 交给属主角色定时任务执行，因为应用角色无删除权限）。
- `OutboxDispatcher`：`PgListener` 监听 `ax_outbox`，收到通知或每 5 秒执行 `claim(worker, 100, 30)`，把事件交给 `OutboxSubscriber` 列表（Step 07 起注册流程触发器、Step 08 注册规则判定、Step 11 注册 SSE 推送），全部订阅者成功后 `ack`。
- 调度单例：凡是“全局只应有一个实例执行”的循环（Cron 扫描、保留清理入队）在执行前调用 `SELECT pg_try_advisory_lock(hashtext($1))`，获取失败则跳过本轮。

### 00.6.7 遥测与托管（`telemetry.rs`、`spa.rs`）

- 日志：`tracing_subscriber::fmt().json()`，字段含 `trace_id`、`tenant_id`、`username`、`route`、`status`、`latency_ms`。
- 追踪：设置 `OTEL_EXPORTER_OTLP_ENDPOINT` 时启用 `tracing-opentelemetry` + OTLP 导出；Core 调 Sidecar 时注入 W3C `traceparent` 请求头。
- 请求 ID：`tower_http::request_id::SetRequestIdLayer::x_request_id(MakeRequestUuid)` + `PropagateRequestIdLayer`。
- 指标：`metrics-exporter-prometheus` 监听 `AXPOINT_METRICS_ADDR`（默认 `127.0.0.1:9464`，不经过反向代理），指标名：`axpoint_http_requests_total{route,status}`、`axpoint_http_request_duration_seconds`、`axpoint_jobs_total{kind,status}`、`axpoint_outbox_lag_seconds`、`axpoint_llm_tokens_total{model,direction}`、`axpoint_duckdb_query_seconds`、`axpoint_recall_channel_seconds{channel}`。
- 上传上限：`DefaultBodyLimit::max(AXPOINT_MAX_UPLOAD_MB * 1024 * 1024)`（用 `usize::try_from` 计算，禁止 `as`）。
- SPA 托管：

```rust
let index = format!("{web_dist_dir}/index.html");
let static_files = ServeDir::new(&web_dist_dir).not_found_service(ServeFile::new(index));
let app = routes::api_router(state)
    .fallback_service(static_files)
    .layer(SetResponseHeaderLayer::if_not_present(CACHE_CONTROL, cache_control_for_path));
```

  `cache_control_for_path`：`/assets/*` → `public, max-age=31536000, immutable`；其余 → `no-cache`。

- `axpoint-server healthcheck` 子命令：请求 `http://127.0.0.1:9995/api/health`，200 则退出码 0，用于 distroless 镜像的容器健康检查。

---

## 00.7 Sidecar 内部令牌

`axpoint-parser/settings.py`：

```python
from __future__ import annotations

from pathlib import Path
from urllib.parse import unquote, urlparse
from pydantic import Field
from pydantic_settings import BaseSettings


class Settings(BaseSettings):
    axpoint_env: str = "development"
    axpoint_internal_token: str = Field(min_length=32)
    axpoint_data_root: Path = Path("/var/lib/axpoint")
    axpoint_model_path: Path = Path("/var/lib/axpoint/models")
    axpoint_max_upload_mb: int = 512
    axpoint_lake_catalog_dsn: str = ""

    def lake_catalog_libpq(self) -> str:
        u = urlparse(self.axpoint_lake_catalog_dsn)
        parts = [
            f"dbname={u.path.lstrip('/')}",
            f"host={u.hostname or '127.0.0.1'}",
            f"port={u.port or 5432}",
        ]
        if u.username:
            parts.append(f"user={unquote(u.username)}")
        if u.password:
            parts.append(f"password={unquote(u.password)}")
        return " ".join(parts)


settings = Settings()  # type: ignore[call-arg]
```

`axpoint-parser/security.py`：

```python
from __future__ import annotations

import hmac
from fastapi import Request
from fastapi.responses import JSONResponse
from starlette.middleware.base import BaseHTTPMiddleware

from settings import settings

PUBLIC_PATHS = {"/health"}


class InternalTokenMiddleware(BaseHTTPMiddleware):
    async def dispatch(self, request: Request, call_next):  # type: ignore[no-untyped-def]
        if request.url.path in PUBLIC_PATHS:
            return await call_next(request)
        supplied = request.headers.get("x-axpoint-internal-token", "")
        if not hmac.compare_digest(supplied, settings.axpoint_internal_token):
            return JSONResponse(status_code=401, content={
                "error_code": "TOKEN_INVALID", "message": "internal token required",
                "trace_id": request.headers.get("traceparent", "")})
        return await call_next(request)
```

`main.py` 执行 `app.add_middleware(InternalTokenMiddleware)`，并注册统一异常处理器输出三字段错误体。Core 侧 `AppState` 中的 `reqwest::Client` 通过 `default_headers` 带上 `x-axpoint-internal-token`。`pyproject.toml` 依赖追加 `pydantic-settings>=2.4,<3`（同步写入 A6），并设置 `py-modules = ["main", "schemas", "settings", "security"]` 与 `[tool.setuptools.packages.find] include = ["parsers*", "connectors*", "exporters*", "lake*", "charts*", "pii*"]`，确保后续各 Step 新增子包无需重复维护包列表。

---

## 00.8 前端路由与令牌

- `registry.ts` 的 `ModuleRouteSpec` 增加 `path: string` 与 `public?: boolean`，条目按 [A1 §3](./A1-CONVENTIONS-ENUMS-ROUTES.md)；本步只注册仓库已有的 7 个页面与 `LoginPage`。
- `App.tsx`：`createBrowserRouter([{ path: "/login", element: <LoginPage/> }, { element: <ShellLayout/>, children: MODULE_REGISTRY.filter(m => !m.public).map(m => ({ path: m.path, element: <m.component/> })) }])`；`ShellLayout` 保留现有侧栏、主题切换和版本号（`__APP_VERSION__`），未登录时重定向 `/login`。
- `lib/api/auth.ts`：令牌只保存在内存模块变量中；开发模式下若存在 `import.meta.env.VITE_DEV_TOKEN` 则自动使用。`LoginPage` 在 Step 00 提供“粘贴访问令牌”表单，调用 `GET /api/auth/me` 校验；Step 13 替换为账号口令与 OIDC 登录。
- `lib/api/client.ts`：`requestJson` 自动附加 `Authorization`；响应非 2xx 时解析三字段错误体并抛出 `ApiError { status, errorCode, message, traceId }`；收到 401 跳转 `/login`。
- `lib/api/rag.ts`、`lib/api/xblue.ts`：删除全部 `tenantId` 参数与 `tenant_id` 查询串、表单字段。
- `e2e/smoke.spec.ts`：以 `VITE_DEV_TOKEN` 构建后，逐个访问注册表中的非公开路由，断言页面标题可见且控制台无 `error` 级别日志。

---

## 00.9 容器化

### 00.9.1 `deploy/docker/axpoint-core.Dockerfile`

```dockerfile
FROM node:24-bookworm AS web
WORKDIR /src/xbluenode-web
COPY xbluenode-web/package.json xbluenode-web/package-lock.json ./
RUN npm ci
COPY xbluenode-web/ ./
RUN npm run build

FROM rust:1.100-bookworm AS core
WORKDIR /src
COPY axpoint-core/ axpoint-core/
COPY deploy/migrations/ deploy/migrations/
COPY --from=web /src/xbluenode-web/src/ xbluenode-web/src/
RUN mkdir -p /out/nonroot-home/.duckdb/extensions \
 && cd axpoint-core && cargo build --release -p axpoint-server

FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=core /src/axpoint-core/target/release/axpoint-server /app/axpoint-server
COPY --from=web /src/xbluenode-web/dist /app/web
COPY --from=core --chown=65532:65532 /out/nonroot-home/ /home/nonroot/
ENV HOME=/home/nonroot WEB_DIST_DIR=/app/web AXPOINT_BIND_ADDR=0.0.0.0:9995 AXPOINT_DATA_ROOT=/var/lib/axpoint
EXPOSE 9995
USER nonroot
HEALTHCHECK --interval=10s --timeout=3s --retries=6 CMD ["/app/axpoint-server", "healthcheck"]
ENTRYPOINT ["/app/axpoint-server"]
```

### 00.9.2 `deploy/docker/axpoint-parser.Dockerfile`

```dockerfile
FROM node:24-bookworm-slim AS node24

FROM python:3.11-slim-bookworm AS base
ENV PYTHONDONTWRITEBYTECODE=1 PYTHONUNBUFFERED=1 HOME=/home/axp
RUN apt-get update && apt-get install -y --no-install-recommends \
      libpango-1.0-0 libpangoft2-1.0-0 libharfbuzz-subset0 libgdk-pixbuf-2.0-0 shared-mime-info fonts-noto-cjk \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --uid 10001 --create-home axp
WORKDIR /app
COPY axpoint-parser/pyproject.toml ./
COPY axpoint-parser/ ./

FROM base AS lite
RUN pip install --no-cache-dir .
USER axp
RUN python -c "import importlib.util as u; \
    d = __import__('duckdb').connect() if u.find_spec('duckdb') else None; \
    [d.execute(f'INSTALL {e}') for e in ('ducklake','postgres')] if d else None"
EXPOSE 8001
HEALTHCHECK --interval=10s --timeout=3s --retries=6 CMD python -c "import urllib.request,sys; sys.exit(0 if urllib.request.urlopen('http://127.0.0.1:8001/health').status==200 else 1)"
CMD ["uvicorn", "main:app", "--host", "0.0.0.0", "--port", "8001", "--workers", "2"]

FROM lite AS full
USER root
COPY --from=node24 /usr/local/ /usr/local/
RUN apt-get update && apt-get install -y --no-install-recommends \
      libreoffice-core libreoffice-writer libreoffice-calc libreoffice-impress \
 && rm -rf /var/lib/apt/lists/* \
 && if python -c "import tomllib,sys; d=tomllib.load(open('pyproject.toml','rb')); sys.exit(0 if 'full' in d.get('project',{}).get('optional-dependencies',{}) else 1)"; then \
      pip install --no-cache-dir ".[full]"; \
    fi \
 && if [ -f /app/charts/package.json ]; then npm --prefix /app/charts ci --omit=dev; fi
USER axp
```

DuckDB 扩展在 `USER axp` 下安装到 `/home/axp/.duckdb/extensions`；运行期设置 `autoinstall_known_extensions=false`。

### 00.9.3 `deploy/docker-compose.yml`

```yaml
name: xbluenode
services:
  postgres:
    image: pgvector/pgvector:pg16
    restart: unless-stopped
    environment:
      POSTGRES_USER: axpoint
      POSTGRES_PASSWORD: ${POSTGRES_PASSWORD:-axpoint_secret}
      POSTGRES_DB: xbluenode
      AXPOINT_APP_DB_PASSWORD: ${AXPOINT_APP_DB_PASSWORD:-axpoint_app_secret}
      AXPOINT_LAKE_DB_PASSWORD: ${AXPOINT_LAKE_DB_PASSWORD:-axpoint_lake_secret}
    ports: ["127.0.0.1:5432:5432"]
    volumes:
      - pgdata:/var/lib/postgresql/data
      - ./docker/postgres-init.sh:/docker-entrypoint-initdb.d/10-roles.sh:ro
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U axpoint -d xbluenode"]
      interval: 5s
      retries: 20
    networks: [internal]

  axpoint-parser:
    image: ${AXPOINT_REGISTRY:-dex2all}/axpoint-parser:${AXPOINT_PARSER_VERSION:-v0.1.0}-${AXPOINT_PARSER_VARIANT:-full}
    build:
      context: ..
      dockerfile: deploy/docker/axpoint-parser.Dockerfile
      target: ${AXPOINT_PARSER_VARIANT:-full}
    env_file: .env
    environment:
      AXPOINT_LAKE_CATALOG_DSN: postgres://axpoint_lake:${AXPOINT_LAKE_DB_PASSWORD:-axpoint_lake_secret}@postgres:5432/xbluenode
    volumes:
      - axpoint_data:/var/lib/axpoint
    depends_on:
      postgres: { condition: service_healthy }
    networks: [internal]

  axpoint-core:
    image: ${AXPOINT_REGISTRY:-dex2all}/axpoint-core:${AXPOINT_VERSION:-v0.1.0}
    build:
      context: ..
      dockerfile: deploy/docker/axpoint-core.Dockerfile
    env_file: .env
    environment:
      DATABASE_URL: postgres://axpoint_app:${AXPOINT_APP_DB_PASSWORD:-axpoint_app_secret}@postgres:5432/xbluenode
      MIGRATION_DATABASE_URL: postgres://axpoint:${POSTGRES_PASSWORD:-axpoint_secret}@postgres:5432/xbluenode
      AXPOINT_LAKE_CATALOG_DSN: postgres://axpoint_lake:${AXPOINT_LAKE_DB_PASSWORD:-axpoint_lake_secret}@postgres:5432/xbluenode
      PARSER_BASE_URL: http://axpoint-parser:8001
      AXPOINT_MIGRATE_ON_START: "true"
    ports: ["9995:9995"]
    volumes:
      - axpoint_data:/var/lib/axpoint
    depends_on:
      postgres: { condition: service_healthy }
      axpoint-parser: { condition: service_healthy }
    networks: [internal]

volumes:
  pgdata:
  axpoint_data:

networks:
  internal:
```

Sidecar 不映射宿主机端口；开发时本机直接运行 `uvicorn`。可选反向代理见 Step 14。

---

## 00.10 环境与门禁脚本

### 00.10.1 `scripts/dev-up.sh`

```bash
#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
test -f deploy/.env || cp deploy/.env.example deploy/.env
docker compose -f deploy/docker-compose.yml --env-file deploy/.env up -d postgres
until docker compose -f deploy/docker-compose.yml exec -T postgres pg_isready -U axpoint -d xbluenode >/dev/null; do sleep 1; done

mkdir -p .gate/jwt/public
if [[ ! -f .gate/jwt/private.pem ]]; then
  openssl genpkey -algorithm ed25519 -out .gate/jwt/private.pem
  openssl pkey -in .gate/jwt/private.pem -pubout -out .gate/jwt/public/1.pem
fi

gen() { python3 -c "import secrets;print(secrets.token_urlsafe(32))"; }
key() { python3 -c "import os,base64;print(base64.b64encode(os.urandom(32)).decode())"; }
if [[ ! -f .env.test ]]; then
  cat > .env.test <<ENV
AXPOINT_ENV=test
MIGRATION_DATABASE_URL=postgres://axpoint:axpoint_secret@127.0.0.1:5432/xbluenode
DATABASE_URL=postgres://axpoint_app:axpoint_app_secret@127.0.0.1:5432/xbluenode
AXPOINT_LAKE_CATALOG_DSN=postgres://axpoint_lake:axpoint_lake_secret@127.0.0.1:5432/xbluenode
AXPOINT_DEV_TOKEN=$(gen)
AXPOINT_INTERNAL_TOKEN=$(gen)
AXPOINT_CREDENTIAL_KEYS=1:$(key)
AXPOINT_MASK_KEYS=1:$(key)
AXPOINT_JWT_PRIVATE_KEY_PATH=$PWD/.gate/jwt/private.pem
AXPOINT_JWT_PUBLIC_KEYS_DIR=$PWD/.gate/jwt/public
AXPOINT_DATA_ROOT=$PWD/.gate/data
AXPOINT_MODEL_PATH=$PWD/.gate/models
AXPOINT_LLM_CASSETTE=$PWD/tests/cassettes
PARSER_BASE_URL=http://127.0.0.1:8001
CORE_BASE_URL=http://127.0.0.1:9995
ENV
fi
set -a; source .env.test; set +a
mkdir -p "$AXPOINT_DATA_ROOT"/{lake,assets,exports,tenants} "$AXPOINT_MODEL_PATH" .gate/seeded

(cd axpoint-parser && uv sync --extra dev \
  && uv run python -c "import importlib.util as u; d=__import__('duckdb').connect() if u.find_spec('duckdb') else None; [d.execute(f'INSTALL {e}') for e in ('ducklake','postgres','mysql_scanner','sqlite_scanner')] if d else None" \
  && uv run python tests/fixtures/make_fixtures.py)

psql "$MIGRATION_DATABASE_URL" -v ON_ERROR_STOP=1 -c "ALTER ROLE axpoint_app PASSWORD 'axpoint_app_secret'" \
  -c "ALTER ROLE axpoint_lake PASSWORD 'axpoint_lake_secret'" 2>/dev/null || true
sqlx migrate run --source deploy/migrations --database-url "$MIGRATION_DATABASE_URL"
psql "$MIGRATION_DATABASE_URL" -v ON_ERROR_STOP=1 -c "ALTER ROLE axpoint_app PASSWORD 'axpoint_app_secret'" \
  -c "ALTER ROLE axpoint_lake PASSWORD 'axpoint_lake_secret'"
echo "dev-up: OK"
```

`scripts/dev-down.sh`：遍历 `.gate/pids/*` 结束全部后台进程后执行 `docker compose -f deploy/docker-compose.yml down -v && rm -rf .gate .env.test`。

### 00.10.2 本地运行服务 `scripts/start-services.sh`

```bash
#!/usr/bin/env bash
# 用法：start-services.sh [--restart core|parser]；后台启动 Sidecar 与 Core，真实进程 PID 写入 .gate/pids/
set -euo pipefail
cd "$(dirname "$0")/.."
set -a; source .env.test; set +a
export PATH="$PWD/axpoint-parser/.venv/bin:$PATH"
mkdir -p .gate/pids .gate/logs
start_parser() { (cd axpoint-parser && nohup uvicorn main:app --host 127.0.0.1 --port 8001 > ../.gate/logs/parser.log 2>&1 & echo $! > ../.gate/pids/parser); }
start_core()   { (cd axpoint-core && cargo build -q -p axpoint-server && nohup ./target/debug/axpoint-server > ../.gate/logs/core.log 2>&1 & echo $! > ../.gate/pids/core); }
stop() { [[ -f ".gate/pids/$1" ]] && kill "$(cat ".gate/pids/$1")" 2>/dev/null || true; rm -f ".gate/pids/$1"; }
wait_http() { for _ in $(seq 1 180); do curl -sf "$1" >/dev/null && return 0; sleep 1; done; echo "timeout: $1" >&2; exit 1; }
if [[ "${1:-}" == "--restart" ]]; then
  stop "$2"; "start_$2"
else
  stop parser; stop core
  (cd xbluenode-web && VITE_DEV_TOKEN="$AXPOINT_DEV_TOKEN" npm run build >/dev/null)
  start_parser; start_core
fi
wait_http "$PARSER_BASE_URL/health"
wait_http "$CORE_BASE_URL/api/health"
echo "services: OK"
```


### 00.10.3 演示数据与 ID 文件

`scripts/seed-demo.sh [MAX_STEP]` 按编号升序扫描 `scripts/seed/[0-9][0-9]-*.sh`（传 `MAX_STEP` 时只执行编号 ≤ `MAX_STEP` 的脚本，不传则执行全部），对尚未存在 `.gate/seeded/<脚本名>` 标记的脚本依次运行，把输出的 `KEY=VALUE` 行追加到 `.gate/ids.env` 并写入标记，从而支持单步执行与 `all.sh` 全量回归重复调用而不重复插入：

```bash
#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
MAX_STEP="${1:-99}"
mkdir -p .gate/seeded
touch .gate/ids.env
for f in scripts/seed/[0-9][0-9]-*.sh; do
  [[ -e "$f" ]] || continue
  base="$(basename "$f")"
  step_num="${base%%-*}"
  (( 10#$step_num <= 10#$MAX_STEP )) || continue
  [[ -f ".gate/seeded/$base" ]] && continue
  bash "$f" >> .gate/ids.env
  touch ".gate/seeded/$base"
done
```

Step 00 的 `scripts/seed/00-baseline.sh`：

```bash
#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "$0")/../gate/lib.sh"
DOC=$(api POST /api/documents/ingest '{"title":"供应链制度","source_name":"policy.md","content":"华东仓（W-1）负责履约订单 O-1 与 O-2。晶圆一厂又称 Fab-1。"}' | jq -r .doc_id)
api POST /api/xblue/schema '{"node_types":[{"name":"Warehouse","display_name":"仓库"},{"name":"Order","display_name":"订单"}],"edge_types":[{"name":"FULFILLS","src_type_name":"Warehouse","dst_type_name":"Order","impact_weight":0.8}]}' >/dev/null
W1=$(api POST /api/xblue/nodes '{"type_name":"Warehouse","node_key":"W-1","title":"华东仓","metrics":{"inventory":500}}' | jq -r .id)
O1=$(api POST /api/xblue/nodes "{\"type_name\":\"Order\",\"node_key\":\"O-1\",\"title\":\"订单 O-1\",\"metrics\":{\"demand\":120},\"link_to_node_id\":\"$W1\",\"link_type\":\"FULFILLS\"}" | jq -r .id)
O2=$(api POST /api/xblue/nodes "{\"type_name\":\"Order\",\"node_key\":\"O-2\",\"title\":\"订单 O-2\",\"metrics\":{\"demand\":80},\"link_to_node_id\":\"$W1\",\"link_type\":\"FULFILLS\"}" | jq -r .id)
echo "DOC_ID=$DOC"; echo "WAREHOUSE_NODE_ID=$W1"; echo "ORDER1_NODE_ID=$O1"; echo "ORDER2_NODE_ID=$O2"
```

`scripts/gate/lib.sh`（完整函数清单见 [A5 §2](./A5-TEST-FIXTURES-AND-GATES.md)）把 `$ROOT/axpoint-parser/.venv/bin` 前置到 `PATH`，并提供 `api METHOD PATH [JSON]`、`api_noauth`、`expect_eq NAME ACTUAL EXPECTED`、`expect_code NAME CODE METHOD PATH [JSON]`（`METHOD` 支持 `GET_NOAUTH` / `POST_NOAUTH` 无令牌请求）、`expect_json NAME JSON JQ_EXPR`、`sql_owner SQL`、`sql_app TENANT SQL`、`gate_pass STEP`。

---

## 00.11 CI

`.github/workflows/ci.yml` 的 `rust-core` 作业：

```yaml
  rust-core:
    needs: detect-changes
    if: needs.detect-changes.outputs.rust == 'true'
    runs-on: ubuntu-latest
    services:
      postgres:
        image: pgvector/pgvector:pg16
        env: { POSTGRES_USER: axpoint, POSTGRES_PASSWORD: axpoint_secret, POSTGRES_DB: xbluenode }
        ports: ["5432:5432"]
        options: >-
          --health-cmd "pg_isready -U axpoint -d xbluenode" --health-interval 5s --health-retries 20
    env:
      MIGRATION_DATABASE_URL: postgres://axpoint:axpoint_secret@localhost:5432/xbluenode
      DATABASE_URL: postgres://axpoint_app:axpoint_app_secret@localhost:5432/xbluenode
    steps:
      - uses: actions/checkout@v4
      # 工具链安装（dtolnay/rust-toolchain，含 clippy、rustfmt）与 Rust 构建缓存步骤与现有 rust-core 作业相同
      - run: cargo install sqlx-cli --version ^0.9 --no-default-features --features rustls,postgres --locked
      - run: |
          sqlx migrate run --source deploy/migrations --database-url "$MIGRATION_DATABASE_URL"
          psql "$MIGRATION_DATABASE_URL" -v ON_ERROR_STOP=1 \
            -c "ALTER ROLE axpoint_app PASSWORD 'axpoint_app_secret'" \
            -c "ALTER ROLE axpoint_lake PASSWORD 'axpoint_lake_secret'"
      - working-directory: axpoint-core
        run: cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
```

`detect-changes` 的 `rust` 过滤器增加 `deploy/migrations/**`；Rust 集成测试统一使用 `DATABASE_URL`（非超级用户 `axpoint_app`）构造业务 `PgPool` 以真实触发 RLS，仅在测试夹具初始化需跨租户预置数据时读取 `MIGRATION_DATABASE_URL`。`web-frontend` 作业在 `npm run build` 后执行 `npx playwright install --with-deps chromium && npx playwright test e2e/smoke.spec.ts`（使用 `vite preview` 与 Mock 夹具，不依赖后端）。

---

## 00.12 测试清单

| 层 | 测试 | 命令 |
| :--- | :--- | :--- |
| Rust | `axpoint-db::tests::rls_blocks_without_tenant`：以 `axpoint_app` 连接且不设 `app.tenant_id` 查询 `xb_nodes` 返回 0 行 | `cargo test -p axpoint-db rls` |
| Rust | `axpoint-db::tests::rls_isolates_tenants`：租户 `t1` 写入的节点在 `t2` 上下文不可见，`WITH CHECK` 拒绝向 `t2` 写入 `t1` 数据 | 同上 |
| Rust | `axpoint-db::tests::jobs_lease_is_exclusive`：两个并发 `lease` 各取 50 条，ID 集合无交集 | `cargo test -p axpoint-db jobs` |
| Rust | `axpoint-db::tests::jobs_retry_backoff`：`fail` 后 `next_run_at` 递增、超过 `max_attempts` 置 `FAILED` | 同上 |
| Rust | `axpoint-db::tests::outbox_claim_and_ack` | `cargo test -p axpoint-db outbox` |
| Rust | `axpoint-security::tests::dev_token_rejected_in_production` | `cargo test -p axpoint-security` |
| Rust | `axpoint-server::tests::api_fallback_is_json_404`、`spa_fallback_serves_index` | `cargo test -p axpoint-server` |
| Python | `test_security_token.py`：无令牌 `POST /parse` → 401；带令牌 → 200；`/health` 无令牌 → 200 | `pytest -v tests/test_security_token.py` |
| Python | `test_fixtures_reproducible`：两次运行 `make_fixtures.py` 输出与 `fixtures.sha256` 一致 | `pytest -v tests/test_fixtures.py` |
| Web | `e2e/smoke.spec.ts` | `npx playwright test e2e/smoke.spec.ts` |

---

## 00.13 验收

`scripts/gate/step00.sh`：

```bash
#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "$0")/lib.sh"

# 0. 门禁断言函数自测
bash "$(dirname "$0")/lib_test.sh"

# 1. 迁移无待执行
expect_eq "migrations-applied" \
  "$(sqlx migrate info --source deploy/migrations --database-url "$MIGRATION_DATABASE_URL" | awk '/pending/{c++} END{print c+0}')" "0"

# 2. 身份边界
expect_code "health-public" 200 GET_NOAUTH /api/health
expect_code "stats-requires-token" 401 GET_NOAUTH /api/stats
expect_code "stats-with-token" 200 GET /api/stats
expect_json "me-is-dev" "$(api GET /api/auth/me)" '.tenant_id == "default" and (.roles | index("ADMIN") != null)'
expect_code "tenant-param-rejected" 400 POST /api/documents/ingest \
  '{"tenant_id":"x","title":"t","source_name":"t.md","content":"c"}'

# 3. 错误体与回退
expect_json "api-404-json" "$(api GET /api/does-not-exist)" '.error_code == "ROUTE_NOT_FOUND" and (.trace_id | length > 0)'
expect_eq "spa-fallback" "$(curl -s -o /dev/null -w '%{http_code}' "$CORE_BASE_URL/graph")" "200"
ASSET_FILE="$(find xbluenode-web/dist/assets -maxdepth 1 -type f -printf '%f\n' | sed -n '1p')"
expect_eq "assets-immutable" \
  "$(curl -sI "$CORE_BASE_URL/assets/$ASSET_FILE" | awk 'BEGIN{IGNORECASE=1} /^cache-control:.*immutable/{c=1} END{print c+0}')" "1"

# 4. 行级安全
bash scripts/seed-demo.sh 00
expect_eq "rls-no-tenant" "$(sql_app '' 'SELECT count(*) FROM xb_nodes')" "0"
expect_eq "rls-default" "$(sql_app default 'SELECT count(*) >= 3 FROM xb_nodes')" "t"
expect_eq "rls-other-tenant" "$(sql_app t_other 'SELECT count(*) FROM xb_nodes')" "0"

# 5. Sidecar 内部令牌
expect_eq "parser-requires-internal-token" \
  "$(curl -s -o /dev/null -w '%{http_code}' -F file=@axpoint-parser/tests/fixtures/plain.txt "$PARSER_BASE_URL/parse")" "401"

# 6. 指标
expect_eq "metrics" "$(curl -s http://127.0.0.1:9464/metrics | awk '/^axpoint_http_requests_total/{c=1} END{print c+0}')" "1"

# 7. 单测与冒烟
(cd axpoint-core && cargo test --workspace -q)
(cd axpoint-parser && pytest -q tests)
(cd xbluenode-web && npx playwright test e2e/smoke.spec.ts)

gate_pass step00
```

**通过标准**：在全新克隆的仓库上依次执行 `bash scripts/dev-up.sh`、`bash scripts/start-services.sh`（§00.10.2）、`bash scripts/gate/step00.sh`，无任何人工干预，最后一行输出 `GATE step00 PASS`；`docker compose -f deploy/docker-compose.yml --env-file deploy/.env up -d --build` 后三个服务均为 `healthy`。
