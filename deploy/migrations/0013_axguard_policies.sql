-- 0013_axguard_policies.sql：登录安全、会话、部门、属性、ABAC 策略、行列策略、跨部门授权、审计链（Step 13 §13.1）

ALTER TABLE sec_users ADD COLUMN IF NOT EXISTS failed_logins       INT NOT NULL DEFAULT 0;
ALTER TABLE sec_users ADD COLUMN IF NOT EXISTS locked_until        TIMESTAMPTZ;
ALTER TABLE sec_users ADD COLUMN IF NOT EXISTS last_login_at       TIMESTAMPTZ;
ALTER TABLE sec_users ADD COLUMN IF NOT EXISTS oidc_subject        TEXT;
ALTER TABLE sec_users ADD COLUMN IF NOT EXISTS tokens_valid_after  TIMESTAMPTZ NOT NULL DEFAULT 'epoch';
ALTER TABLE sec_users ADD COLUMN IF NOT EXISTS password_changed_at TIMESTAMPTZ;
CREATE UNIQUE INDEX IF NOT EXISTS uq_sec_users_oidc ON sec_users (tenant_id, oidc_subject) WHERE oidc_subject IS NOT NULL;

CREATE TABLE IF NOT EXISTS sec_departments (
    tenant_id TEXT NOT NULL,
    dept_id   TEXT NOT NULL CHECK (dept_id ~ '^[a-z0-9_-]{1,64}$'),
    name      TEXT NOT NULL,
    parent_id TEXT,
    PRIMARY KEY (tenant_id, dept_id)
);
INSERT INTO sec_departments (tenant_id, dept_id, name) VALUES ('default','default','默认部门') ON CONFLICT DO NOTHING;

CREATE TABLE IF NOT EXISTS sec_user_attributes (
    tenant_id   TEXT NOT NULL,
    user_id     UUID NOT NULL REFERENCES sec_users(id) ON DELETE CASCADE,
    attr_key    TEXT NOT NULL CHECK (attr_key ~ '^[a-z][a-z0-9_]{0,62}$'),
    attr_values TEXT[] NOT NULL,
    PRIMARY KEY (user_id, attr_key)
);

-- 刷新令牌会话：同一登录形成一个 family，轮换后被替换的令牌作废；重放已作废令牌则整个 family 吊销
CREATE TABLE IF NOT EXISTS sec_sessions (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id     TEXT NOT NULL,
    user_id       UUID NOT NULL REFERENCES sec_users(id) ON DELETE CASCADE,
    family_id     UUID NOT NULL,
    refresh_hash  TEXT NOT NULL UNIQUE,
    auth_source   TEXT NOT NULL CHECK (auth_source IN ('LOCAL','OIDC')),
    user_agent    TEXT NOT NULL DEFAULT '',
    client_ip     TEXT NOT NULL DEFAULT '',
    expires_at    TIMESTAMPTZ NOT NULL,
    rotated_at    TIMESTAMPTZ,
    revoked_at    TIMESTAMPTZ,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS idx_sec_sessions_family ON sec_sessions (family_id);

-- OIDC 登录中间态（state → nonce、PKCE verifier），10 分钟有效，一次性
CREATE TABLE IF NOT EXISTS sec_login_states (
    state         TEXT PRIMARY KEY,
    tenant_id     TEXT NOT NULL,
    nonce         TEXT NOT NULL,
    pkce_verifier TEXT NOT NULL,
    return_to     TEXT NOT NULL DEFAULT '/',
    expires_at    TIMESTAMPTZ NOT NULL
);

-- 资源归属与密级
ALTER TABLE ds_datasets   ADD COLUMN IF NOT EXISTS owner_dept  TEXT NOT NULL DEFAULT 'default';
ALTER TABLE ds_datasets   ADD COLUMN IF NOT EXISTS sensitivity TEXT NOT NULL DEFAULT 'INTERNAL'
    CHECK (sensitivity IN ('PUBLIC','INTERNAL','CONFIDENTIAL','SECRET'));
ALTER TABLE xb_subgraphs  ADD COLUMN IF NOT EXISTS sensitivity TEXT NOT NULL DEFAULT 'INTERNAL'
    CHECK (sensitivity IN ('PUBLIC','INTERNAL','CONFIDENTIAL','SECRET'));
ALTER TABLE xb_node_types ADD COLUMN IF NOT EXISTS owner_dept  TEXT NOT NULL DEFAULT 'default';
ALTER TABLE xb_node_types ADD COLUMN IF NOT EXISTS sensitivity TEXT NOT NULL DEFAULT 'INTERNAL'
    CHECK (sensitivity IN ('PUBLIC','INTERNAL','CONFIDENTIAL','SECRET'));
ALTER TABLE xv_dashboards ADD COLUMN IF NOT EXISTS owner_dept  TEXT NOT NULL DEFAULT 'default';
ALTER TABLE xv_dashboards ADD COLUMN IF NOT EXISTS sensitivity TEXT NOT NULL DEFAULT 'INTERNAL'
    CHECK (sensitivity IN ('PUBLIC','INTERNAL','CONFIDENTIAL','SECRET'));
ALTER TABLE ax_agents     ADD COLUMN IF NOT EXISTS owner_dept  TEXT NOT NULL DEFAULT 'default';
ALTER TABLE ax_agents     ADD COLUMN IF NOT EXISTS sensitivity TEXT NOT NULL DEFAULT 'INTERNAL'
    CHECK (sensitivity IN ('PUBLIC','INTERNAL','CONFIDENTIAL','SECRET'));
ALTER TABLE sec_users     ADD COLUMN IF NOT EXISTS clearance   TEXT NOT NULL DEFAULT 'INTERNAL'
    CHECK (clearance IN ('PUBLIC','INTERNAL','CONFIDENTIAL','SECRET'));

UPDATE sec_users SET clearance = 'SECRET'
 WHERE id = '00000000-0000-0000-0000-000000000000' AND username = 'dev';

-- 租户自定义 Cedar 策略（基线策略随代码发布，不入库）
CREATE TABLE IF NOT EXISTS sec_policies (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id   TEXT NOT NULL,
    name        TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    policy_text TEXT NOT NULL,
    enabled     BOOLEAN NOT NULL DEFAULT TRUE,
    version     INT NOT NULL DEFAULT 1,
    updated_by  UUID,
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, name)
);

CREATE TABLE IF NOT EXISTS sec_row_policies (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id    TEXT NOT NULL,
    name         TEXT NOT NULL,
    target_kind  TEXT NOT NULL CHECK (target_kind IN ('DATASET','NODE_TYPE')),
    target_ref   TEXT NOT NULL,
    applies_to   JSONB NOT NULL DEFAULT '{}'::jsonb,
    predicate    TEXT NOT NULL,
    enabled      BOOLEAN NOT NULL DEFAULT TRUE,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, name)
);
CREATE TABLE IF NOT EXISTS sec_row_policy_defaults (
    tenant_id    TEXT NOT NULL,
    target_kind  TEXT NOT NULL CHECK (target_kind IN ('DATASET','NODE_TYPE')),
    target_ref   TEXT NOT NULL,
    default_deny BOOLEAN NOT NULL DEFAULT FALSE,
    PRIMARY KEY (tenant_id, target_kind, target_ref)
);

CREATE TABLE IF NOT EXISTS sec_column_policies (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id    TEXT NOT NULL,
    target_kind  TEXT NOT NULL CHECK (target_kind IN ('DATASET','NODE_TYPE')),
    target_ref   TEXT NOT NULL,
    column_name  TEXT NOT NULL,
    mask_rule    TEXT NOT NULL CHECK (mask_rule IN ('NONE','MASK_PHONE','MASK_ID_CARD','MASK_EMAIL','MASK_BANK_CARD',
                                                    'MASK_NAME','MASK_ADDRESS','HMAC_HASH','REDACT')),
    exempt       JSONB NOT NULL DEFAULT '{}'::jsonb,
    UNIQUE (tenant_id, target_kind, target_ref, column_name)
);

-- 跨部门授权：资源属主部门把读权限授予其他部门 / 角色 / 用户，经审批生效
CREATE TABLE IF NOT EXISTS sec_grants (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id     TEXT NOT NULL,
    resource_kind TEXT NOT NULL CHECK (resource_kind IN ('DATASET','NODE_TYPE','SUBGRAPH','DASHBOARD','AGENT')),
    resource_ref  TEXT NOT NULL,
    grantee_kind  TEXT NOT NULL CHECK (grantee_kind IN ('DEPT','ROLE','USER')),
    grantee_ref   TEXT NOT NULL,
    actions       TEXT[] NOT NULL DEFAULT ARRAY['DATA_READ'],
    row_predicate TEXT,
    reason        TEXT NOT NULL,
    status        TEXT NOT NULL DEFAULT 'PENDING' CHECK (status IN ('PENDING','ACTIVE','REJECTED','REVOKED','EXPIRED')),
    approval_id   UUID,
    requested_by  UUID NOT NULL,
    expires_at    TIMESTAMPTZ NOT NULL,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    decided_at    TIMESTAMPTZ
);
ALTER TABLE ax_approvals DROP CONSTRAINT IF EXISTS ax_approvals_source_kind_check;
ALTER TABLE ax_approvals ADD CONSTRAINT ax_approvals_source_kind_check
    CHECK (source_kind IN ('FLOW','SIM_APPLY','AGENT','GRANT'));

-- 大模型提供方是否允许接收 PII 原文
ALTER TABLE ax_llm_providers ADD COLUMN IF NOT EXISTS allow_pii BOOLEAN NOT NULL DEFAULT FALSE;

-- 文档段落中的 PII 位置
ALTER TABLE hg_paras ADD COLUMN IF NOT EXISTS pii_spans JSONB NOT NULL DEFAULT '[]'::jsonb;

-- 审计哈希链
ALTER TABLE sec_audit_logs ADD COLUMN IF NOT EXISTS prev_hash TEXT NOT NULL DEFAULT '';
ALTER TABLE sec_audit_logs ADD COLUMN IF NOT EXISTS row_hash  TEXT NOT NULL DEFAULT '';
CREATE TABLE IF NOT EXISTS sec_audit_chain_heads (
    tenant_id TEXT PRIMARY KEY,
    last_id   BIGINT NOT NULL,
    last_hash TEXT NOT NULL
);

SELECT ax_enable_tenant_rls(t) FROM unnest(ARRAY[
    'sec_departments','sec_user_attributes','sec_sessions','sec_policies','sec_row_policies',
    'sec_row_policy_defaults','sec_column_policies','sec_grants','sec_audit_chain_heads'
]::regclass[]) AS t;
GRANT SELECT, INSERT, DELETE ON sec_login_states TO axpoint_app;

-- 登录前按用户名定位（不暴露口令哈希以外字段）
CREATE OR REPLACE FUNCTION ax_lookup_login(p_tenant TEXT, p_username TEXT)
RETURNS TABLE (user_id UUID, password_hash TEXT, status TEXT, locked_until TIMESTAMPTZ, failed_logins INT)
LANGUAGE sql SECURITY DEFINER SET search_path = public AS $$
    SELECT id, password_hash, status, locked_until, failed_logins
      FROM sec_users WHERE tenant_id = p_tenant AND username = p_username AND auth_source = 'LOCAL';
$$;

-- 刷新令牌定位
CREATE OR REPLACE FUNCTION ax_resolve_session(p_refresh_hash TEXT)
RETURNS TABLE (session_id UUID, tenant_id TEXT, user_id UUID, family_id UUID, expires_at TIMESTAMPTZ,
               rotated_at TIMESTAMPTZ, revoked_at TIMESTAMPTZ)
LANGUAGE sql SECURITY DEFINER SET search_path = public AS $$
    SELECT id, tenant_id, user_id, family_id, expires_at, rotated_at, revoked_at
      FROM sec_sessions WHERE refresh_hash = p_refresh_hash;
$$;

REVOKE ALL ON FUNCTION ax_lookup_login(TEXT, TEXT) FROM PUBLIC;
REVOKE ALL ON FUNCTION ax_resolve_session(TEXT) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION ax_lookup_login(TEXT, TEXT) TO axpoint_app;
GRANT EXECUTE ON FUNCTION ax_resolve_session(TEXT) TO axpoint_app;

INSERT INTO ax_retention_policies (tenant_id, target, keep_days) VALUES ('default','AUDIT',1095) ON CONFLICT DO NOTHING;
