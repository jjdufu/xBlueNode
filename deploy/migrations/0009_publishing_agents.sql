-- 0009_publishing_agents.sql：模型提供方与用量、发布端点、MCP 客户端、智能体、会话、消息、嵌入链接

CREATE TABLE IF NOT EXISTS ax_llm_providers (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id     TEXT NOT NULL,
    name          TEXT NOT NULL,
    base_url      TEXT NOT NULL,                     -- OpenAI 兼容接口根地址
    model         TEXT NOT NULL,
    purposes      TEXT[] NOT NULL CHECK (purposes <@ ARRAY['CHAT','EMBED','NL2SQL','EXTRACT','AGENT','SUMMARIZE']::TEXT[]),
    credential_id UUID REFERENCES ds_credentials(id),
    priority      INT NOT NULL DEFAULT 100,          -- 同一用途内数值小者优先
    temperature   REAL NOT NULL DEFAULT 0.2,
    max_tokens    INT NOT NULL DEFAULT 4096,
    timeout_s     INT NOT NULL DEFAULT 60,
    rpm_limit     INT,
    supports_tools BOOLEAN NOT NULL DEFAULT TRUE,
    embedding_dim INT,
    allow_private_network BOOLEAN NOT NULL DEFAULT FALSE,
    enabled       BOOLEAN NOT NULL DEFAULT TRUE,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, name)
);

CREATE TABLE IF NOT EXISTS ax_llm_usage (
    tenant_id         TEXT NOT NULL,
    provider_id       UUID NOT NULL REFERENCES ax_llm_providers(id) ON DELETE CASCADE,
    usage_date        DATE NOT NULL,
    purpose           TEXT NOT NULL,
    requests          BIGINT NOT NULL DEFAULT 0,
    errors            BIGINT NOT NULL DEFAULT 0,
    prompt_tokens     BIGINT NOT NULL DEFAULT 0,
    completion_tokens BIGINT NOT NULL DEFAULT 0,
    PRIMARY KEY (tenant_id, provider_id, usage_date, purpose)
);

CREATE TABLE IF NOT EXISTS ax_published_endpoints (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id       TEXT NOT NULL,
    slug            TEXT NOT NULL CHECK (slug ~ '^[a-z0-9-]{3,64}$'),
    environment     TEXT NOT NULL CHECK (environment IN ('TEST','PROD')),
    kind            TEXT NOT NULL CHECK (kind IN ('SAVED_QUERY','METRIC','FLOW','AGENT','OBJECT_TYPE')),
    target_id       TEXT NOT NULL,                   -- saved_query id / metric name / flow id / agent id / 主图已发布 NodeType.name
    target_version  INT,                             -- PROD 固定版本；TEST 为空表示跟随最新
    target_snapshot JSONB,                           -- PROD 发布时目标定义的快照（保存查询 spec / 指标定义 / NodeTypeSpec）
    title           TEXT NOT NULL,
    description     TEXT NOT NULL DEFAULT '',
    input_schema    JSONB NOT NULL,                  -- JSON Schema（draft 2020-12）
    output_shape    TEXT NOT NULL DEFAULT 'ROWS' CHECK (output_shape IN ('ROWS','OBJECT','TEXT','RUN')),
    required_scope  TEXT NOT NULL DEFAULT 'AGENT_RUN',
    rate_limit_rpm  INT NOT NULL DEFAULT 60,
    cache_ttl_s     INT NOT NULL DEFAULT 0,
    expose_mcp      BOOLEAN NOT NULL DEFAULT FALSE,
    show_as_app     BOOLEAN NOT NULL DEFAULT TRUE,
    enabled         BOOLEAN NOT NULL DEFAULT TRUE,
    published_by    UUID,
    published_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, environment, slug)
);

CREATE TABLE IF NOT EXISTS ax_mcp_clients (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id     TEXT NOT NULL,
    name          TEXT NOT NULL CHECK (name ~ '^[a-z][a-z0-9_]{0,31}$'),
    url           TEXT NOT NULL,                     -- Streamable HTTP 端点
    credential_id UUID REFERENCES ds_credentials(id),-- {"authorization": "Bearer ..."} 或自定义请求头
    allowed_tools TEXT[] NOT NULL DEFAULT '{}',      -- 空数组 = 全部
    tools_cache   JSONB NOT NULL DEFAULT '[]'::jsonb,
    allow_private_network BOOLEAN NOT NULL DEFAULT FALSE,
    enabled       BOOLEAN NOT NULL DEFAULT TRUE,
    last_synced_at TIMESTAMPTZ,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, name)
);

CREATE TABLE IF NOT EXISTS ax_agents (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id   TEXT NOT NULL,
    name        TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    spec        JSONB NOT NULL,                      -- AgentSpec 草稿（§09.5.1）
    version     INT NOT NULL DEFAULT 0,              -- 最新发布版本
    created_by  UUID,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, name)
);

CREATE TABLE IF NOT EXISTS ax_agent_versions (
    tenant_id    TEXT NOT NULL,
    agent_id     UUID NOT NULL REFERENCES ax_agents(id) ON DELETE CASCADE,
    version      INT NOT NULL,
    spec         JSONB NOT NULL,
    published_by UUID,
    published_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (agent_id, version)
);

CREATE TABLE IF NOT EXISTS ax_conversations (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id     TEXT NOT NULL,
    agent_id      UUID NOT NULL REFERENCES ax_agents(id) ON DELETE CASCADE,
    agent_version INT NOT NULL,
    user_id       UUID,
    channel       TEXT NOT NULL CHECK (channel IN ('WORKBENCH','EMBED','API','FLOW','MCP')),
    embed_id      UUID,
    title         TEXT NOT NULL DEFAULT '',
    status        TEXT NOT NULL DEFAULT 'ACTIVE' CHECK (status IN ('ACTIVE','WAITING_APPROVAL','CLOSED')),
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS idx_ax_conversations_user ON ax_conversations (tenant_id, user_id, updated_at DESC);

CREATE TABLE IF NOT EXISTS ax_messages (
    id              BIGSERIAL PRIMARY KEY,
    tenant_id       TEXT NOT NULL,
    conversation_id UUID NOT NULL REFERENCES ax_conversations(id) ON DELETE CASCADE,
    role            TEXT NOT NULL CHECK (role IN ('USER','ASSISTANT','TOOL','SYSTEM')),
    content         TEXT NOT NULL DEFAULT '',
    tool_calls      JSONB,                           -- [{id, name, arguments}]
    tool_call_id    TEXT,
    agent_name      TEXT,                            -- 多智能体时标识发言者
    provider_id     UUID,
    prompt_tokens   INT,
    completion_tokens INT,
    latency_ms      INT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS idx_ax_messages_conv ON ax_messages (tenant_id, conversation_id, id);

CREATE TABLE IF NOT EXISTS ax_agent_embeds (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id       TEXT NOT NULL,
    agent_id        UUID NOT NULL REFERENCES ax_agents(id) ON DELETE CASCADE,
    token_hash      TEXT NOT NULL UNIQUE,            -- hex(SHA-256(axe_<32 字节 base62>))
    allowed_origins TEXT[] NOT NULL DEFAULT '{}',
    permissions     TEXT[] NOT NULL DEFAULT ARRAY['DATA_READ','AGENT_RUN'],
    rate_limit_rpm  INT NOT NULL DEFAULT 20,
    expires_at      TIMESTAMPTZ,
    revoked_at      TIMESTAMPTZ,
    created_by      UUID NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

SELECT ax_enable_tenant_rls(t) FROM unnest(ARRAY[
    'ax_llm_providers','ax_llm_usage','ax_published_endpoints','ax_mcp_clients','ax_agents','ax_agent_versions',
    'ax_conversations','ax_messages','ax_agent_embeds'
]::regclass[]) AS t;

GRANT USAGE, SELECT ON SEQUENCE ax_messages_id_seq TO axpoint_app;

CREATE OR REPLACE FUNCTION ax_resolve_agent_embed(p_token_hash TEXT)
RETURNS TABLE (embed_id UUID, tenant_id TEXT, agent_id UUID, created_by UUID, permissions TEXT[],
               allowed_origins TEXT[], rate_limit_rpm INT)
LANGUAGE sql SECURITY DEFINER SET search_path = public AS $$
    SELECT e.id, e.tenant_id, e.agent_id, e.created_by, e.permissions, e.allowed_origins, e.rate_limit_rpm
      FROM ax_agent_embeds e
     WHERE e.token_hash = p_token_hash AND e.revoked_at IS NULL AND (e.expires_at IS NULL OR e.expires_at > NOW());
$$;
REVOKE ALL ON FUNCTION ax_resolve_agent_embed(TEXT) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION ax_resolve_agent_embed(TEXT) TO axpoint_app;
