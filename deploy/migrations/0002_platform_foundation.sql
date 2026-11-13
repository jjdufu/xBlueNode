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
