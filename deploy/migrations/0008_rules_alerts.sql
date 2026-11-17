-- 0008_rules_alerts.sql：规则、告警、告警事件、通知渠道、投递记录、静默

CREATE TABLE IF NOT EXISTS ax_notify_channels (
    id                    UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id             TEXT NOT NULL,
    name                  TEXT NOT NULL,
    kind                  TEXT NOT NULL CHECK (kind IN ('WEBHOOK','EMAIL','WECOM','DINGTALK','FEISHU')),
    config                JSONB NOT NULL DEFAULT '{}'::jsonb,   -- 非秘密字段
    credential_id         UUID REFERENCES ds_credentials(id) ON DELETE SET NULL, -- 秘密字段（URL 中的 key、签名密钥、SMTP 口令）
    allow_private_network BOOLEAN NOT NULL DEFAULT FALSE,
    enabled               BOOLEAN NOT NULL DEFAULT TRUE,
    created_at            TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, name)
);

CREATE TABLE IF NOT EXISTS ax_rules (
    id                     UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id              TEXT NOT NULL,
    name                   TEXT NOT NULL,
    description            TEXT NOT NULL DEFAULT '',
    scope_kind             TEXT NOT NULL CHECK (scope_kind IN ('METRIC','DATASET_QUALITY','EVENT')),
    target                 JSONB NOT NULL,      -- METRIC: {type_name, metric, node_filter?}；DATASET_QUALITY: {dataset_id?}；EVENT: {topic, filter?}
    condition              JSONB NOT NULL,      -- Condition（§08.2）
    severity               TEXT NOT NULL CHECK (severity IN ('INFO','WARN','CRITICAL')),
    title_template         TEXT NOT NULL,
    body_template          TEXT NOT NULL DEFAULT '',
    cooldown_minutes       INT NOT NULL DEFAULT 30 CHECK (cooldown_minutes >= 0),
    resolve_after_minutes  INT NOT NULL DEFAULT 10 CHECK (resolve_after_minutes >= 0),
    escalate_after_minutes INT CHECK (escalate_after_minutes > 0),
    channel_ids            UUID[] NOT NULL DEFAULT '{}',
    escalation_channel_ids UUID[] NOT NULL DEFAULT '{}',
    run_flow_id            UUID REFERENCES ax_flows(id) ON DELETE SET NULL,
    enabled                BOOLEAN NOT NULL DEFAULT TRUE,
    version                INT NOT NULL DEFAULT 1,
    created_by             UUID,
    created_at             TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at             TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, name)
);

CREATE TABLE IF NOT EXISTS ax_alerts (
    id             UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id      TEXT NOT NULL,
    rule_id        UUID NOT NULL REFERENCES ax_rules(id) ON DELETE CASCADE,
    dedup_key      TEXT NOT NULL,                  -- 规则 id + 对象（节点 id / 数据集 id / 事件键）
    severity       TEXT NOT NULL CHECK (severity IN ('INFO','WARN','CRITICAL')),
    status         TEXT NOT NULL DEFAULT 'OPEN' CHECK (status IN ('OPEN','ACK','RESOLVED')),
    title          TEXT NOT NULL,
    body           TEXT NOT NULL DEFAULT '',
    node_id        UUID,
    dataset_id     UUID,
    last_value     DOUBLE PRECISION,
    detail         JSONB NOT NULL DEFAULT '{}'::jsonb,
    occurrences    INT NOT NULL DEFAULT 1,
    first_seen_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_seen_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_false_at  TIMESTAMPTZ,                    -- 条件最近一次为假的时间（自动恢复计时）
    acked_by       UUID,
    acked_at       TIMESTAMPTZ,
    escalated_at   TIMESTAMPTZ,
    resolved_at    TIMESTAMPTZ,
    resolve_reason TEXT CHECK (resolve_reason IN ('AUTO','MANUAL','RULE_DISABLED'))
);
CREATE UNIQUE INDEX IF NOT EXISTS uq_ax_alerts_active ON ax_alerts (tenant_id, dedup_key) WHERE status <> 'RESOLVED';
CREATE INDEX IF NOT EXISTS idx_ax_alerts_status ON ax_alerts (tenant_id, status, last_seen_at DESC);

CREATE TABLE IF NOT EXISTS ax_alert_events (
    id         BIGSERIAL PRIMARY KEY,
    tenant_id  TEXT NOT NULL,
    alert_id   UUID NOT NULL REFERENCES ax_alerts(id) ON DELETE CASCADE,
    event      TEXT NOT NULL CHECK (event IN ('OPENED','RETRIGGERED','ACKED','ESCALATED','RESOLVED','NOTIFIED','NOTIFY_FAILED','SILENCED','FLOW_STARTED')),
    detail     JSONB NOT NULL DEFAULT '{}'::jsonb,
    actor      UUID,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS idx_ax_alert_events_alert ON ax_alert_events (tenant_id, alert_id, id ASC);

CREATE TABLE IF NOT EXISTS ax_notify_deliveries (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id     TEXT NOT NULL,
    alert_id      UUID REFERENCES ax_alerts(id) ON DELETE CASCADE,
    channel_id    UUID NOT NULL REFERENCES ax_notify_channels(id) ON DELETE CASCADE,
    event         TEXT NOT NULL CHECK (event IN ('OPENED','ESCALATED','RESOLVED','TEST','ACTION')),
    status        TEXT NOT NULL DEFAULT 'PENDING' CHECK (status IN ('PENDING','SENT','FAILED')),
    attempt       INT NOT NULL DEFAULT 0,
    response_code INT,
    error_message TEXT,
    next_retry_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    sent_at       TIMESTAMPTZ,
    UNIQUE (alert_id, channel_id, event)
);
CREATE INDEX IF NOT EXISTS idx_ax_notify_deliveries_due ON ax_notify_deliveries (next_retry_at ASC) WHERE status = 'PENDING';

CREATE TABLE IF NOT EXISTS ax_alert_silences (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id  TEXT NOT NULL,
    matcher    JSONB NOT NULL,          -- {rule_ids?, severities?, node_ids?, type_names?}
    starts_at  TIMESTAMPTZ NOT NULL,
    ends_at    TIMESTAMPTZ NOT NULL CHECK (ends_at > starts_at),
    reason     TEXT NOT NULL,
    created_by UUID,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

SELECT ax_enable_tenant_rls(t) FROM unnest(ARRAY[
    'ax_notify_channels','ax_rules','ax_alerts','ax_alert_events','ax_notify_deliveries','ax_alert_silences'
]::regclass[]) AS t;

GRANT USAGE, SELECT ON SEQUENCE ax_alert_events_id_seq TO axpoint_app;

CREATE OR REPLACE FUNCTION ax_absence_rules()
RETURNS TABLE (tenant_id TEXT, rule_id UUID)
LANGUAGE sql SECURITY DEFINER SET search_path = public AS $$
    SELECT r.tenant_id, r.id FROM ax_rules r
     WHERE r.enabled AND (r.condition->>'kind' = 'ABSENCE'
        OR EXISTS (SELECT 1 FROM ax_alerts a WHERE a.rule_id = r.id AND a.status <> 'RESOLVED'));
$$;

CREATE OR REPLACE FUNCTION ax_due_notify_deliveries(p_limit INT DEFAULT 100)
RETURNS TABLE (tenant_id TEXT, delivery_id UUID)
LANGUAGE sql SECURITY DEFINER SET search_path = public AS $$
    SELECT d.tenant_id, d.id
      FROM ax_notify_deliveries d
     WHERE d.status = 'PENDING' AND d.next_retry_at <= NOW()
     ORDER BY d.next_retry_at ASC
     LIMIT GREATEST(COALESCE(p_limit, 100), 1);
$$;

REVOKE ALL ON FUNCTION ax_absence_rules() FROM PUBLIC;
REVOKE ALL ON FUNCTION ax_due_notify_deliveries(INT) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION ax_absence_rules() TO axpoint_app;
GRANT EXECUTE ON FUNCTION ax_due_notify_deliveries(INT) TO axpoint_app;
