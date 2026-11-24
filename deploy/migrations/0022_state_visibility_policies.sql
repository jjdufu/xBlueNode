-- 0022_state_visibility_policies.sql：状态驱动动态行级视野与活跃期（TTL）退场扩展

ALTER TABLE sec_row_policies
    ADD COLUMN IF NOT EXISTS policy_mode TEXT NOT NULL DEFAULT 'STATIC_ATTR'
        CHECK (policy_mode IN ('STATIC_ATTR', 'STATE_DRIVEN')),
    ADD COLUMN IF NOT EXISTS ttl_timestamp_field TEXT,
    ADD COLUMN IF NOT EXISTS ttl_max_age_hours INT
        CHECK (ttl_max_age_hours IS NULL OR (ttl_max_age_hours >= 1 AND ttl_max_age_hours <= 87600));
