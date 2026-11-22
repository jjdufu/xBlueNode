-- 0015_llm_structured_grounding.sql：受限解码能力分级、确定性种子、本地小模型标识、个人模型归属与默认模型偏好

ALTER TABLE ax_llm_providers
    ADD COLUMN IF NOT EXISTS structured_mode TEXT NOT NULL DEFAULT 'NONE'
        CHECK (structured_mode IN ('NONE', 'JSON_OBJECT', 'JSON_SCHEMA'));

ALTER TABLE ax_llm_providers
    ADD COLUMN IF NOT EXISTS deterministic_seed BIGINT;

ALTER TABLE ax_llm_providers
    ADD COLUMN IF NOT EXISTS is_local_model BOOLEAN NOT NULL DEFAULT FALSE;

ALTER TABLE ax_llm_providers
    ADD COLUMN IF NOT EXISTS max_context_tokens INT NOT NULL DEFAULT 32768
        CHECK (max_context_tokens >= 2048 AND max_context_tokens <= 2097152);

-- NULL 表示租户级系统提供方（仅管理员可写、全员只读可见）；非空表示该用户私有提供方
ALTER TABLE ax_llm_providers
    ADD COLUMN IF NOT EXISTS owner_user_id UUID REFERENCES sec_users(id) ON DELETE CASCADE;

ALTER TABLE ax_llm_providers
    DROP CONSTRAINT IF EXISTS ax_llm_providers_tenant_id_name_key;

CREATE UNIQUE INDEX IF NOT EXISTS uq_ax_llm_providers_system_name
    ON ax_llm_providers (tenant_id, name)
    WHERE owner_user_id IS NULL;

CREATE UNIQUE INDEX IF NOT EXISTS uq_ax_llm_providers_owner_name
    ON ax_llm_providers (tenant_id, owner_user_id, name)
    WHERE owner_user_id IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_ax_llm_providers_owner
    ON ax_llm_providers (tenant_id, owner_user_id);

CREATE TABLE IF NOT EXISTS ax_user_llm_preferences (
    tenant_id   TEXT NOT NULL,
    user_id     UUID NOT NULL REFERENCES sec_users(id) ON DELETE CASCADE,
    purpose     TEXT NOT NULL
                CHECK (purpose IN ('CHAT', 'EMBED', 'NL2SQL', 'EXTRACT', 'AGENT', 'SUMMARIZE')),
    provider_id UUID NOT NULL REFERENCES ax_llm_providers(id) ON DELETE CASCADE,
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (tenant_id, user_id, purpose)
);

CREATE INDEX IF NOT EXISTS idx_ax_user_llm_preferences_provider
    ON ax_user_llm_preferences (provider_id);

SELECT ax_enable_tenant_rls('ax_user_llm_preferences');
