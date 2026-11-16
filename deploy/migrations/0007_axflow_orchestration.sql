-- 0007_axflow_orchestration.sql：DAG 定义、版本、部署、运行、步骤日志、审批、动作日志、流程 Webhook

ALTER TABLE ax_flows ADD COLUMN IF NOT EXISTS nodes       JSONB NOT NULL DEFAULT '[]'::jsonb;
ALTER TABLE ax_flows ADD COLUMN IF NOT EXISTS edges       JSONB NOT NULL DEFAULT '[]'::jsonb;
ALTER TABLE ax_flows ADD COLUMN IF NOT EXISTS trigger_cfg JSONB NOT NULL DEFAULT '{"type":"MANUAL"}'::jsonb;
ALTER TABLE ax_flows ADD COLUMN IF NOT EXISTS slug        TEXT;
ALTER TABLE ax_flows ADD COLUMN IF NOT EXISTS created_by  UUID;
ALTER TABLE ax_flows ADD COLUMN IF NOT EXISTS updated_at  TIMESTAMPTZ NOT NULL DEFAULT NOW();
CREATE UNIQUE INDEX IF NOT EXISTS uq_ax_flows_name ON ax_flows (tenant_id, name);
CREATE UNIQUE INDEX IF NOT EXISTS uq_ax_flows_slug ON ax_flows (tenant_id, slug) WHERE slug IS NOT NULL;

-- 仓库已有的线性 steps 转换为链式 DAG：
--   kind 映射 INGEST→SOURCE_INGEST、TRANSFORM→SQL_TRANSFORM、NODE_MAP→XBLUE_MAP、ACTION→ACTION_EMIT，其他值→ACTION_EMIT（dry_run=true）
--   节点 id 沿用 step.id，边按数组顺序连接
UPDATE ax_flows f
   SET nodes = sub.nodes, edges = sub.edges
  FROM (
    SELECT id,
           jsonb_agg(jsonb_build_object(
               'id', s->>'id',
               'kind', CASE s->>'kind' WHEN 'INGEST' THEN 'SOURCE_INGEST' WHEN 'TRANSFORM' THEN 'SQL_TRANSFORM'
                                       WHEN 'NODE_MAP' THEN 'XBLUE_MAP' ELSE 'ACTION_EMIT' END,
               'label', s->>'label',
               'config', CASE WHEN s->>'kind' IN ('INGEST','TRANSFORM','NODE_MAP','ACTION') THEN s->'config'
                              ELSE (s->'config') || '{"dry_run":true}'::jsonb END,
               'pos', jsonb_build_object('x', 200 * (ord - 1), 'y', 120)) ORDER BY ord) AS nodes,
           COALESCE(jsonb_agg(jsonb_build_object('from', prev_id, 'to', s->>'id')) FILTER (WHERE prev_id IS NOT NULL), '[]'::jsonb) AS edges
      FROM (
        SELECT f2.id, s, ord, lag(s->>'id') OVER (PARTITION BY f2.id ORDER BY ord) AS prev_id
          FROM ax_flows f2, jsonb_array_elements(f2.steps) WITH ORDINALITY AS t(s, ord)
      ) x
     GROUP BY id
  ) sub
 WHERE f.id = sub.id AND f.nodes = '[]'::jsonb AND jsonb_array_length(f.steps) > 0;

CREATE TABLE IF NOT EXISTS ax_flow_versions (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id    TEXT NOT NULL,
    flow_id      UUID NOT NULL REFERENCES ax_flows(id) ON DELETE CASCADE,
    version      INT NOT NULL,
    nodes        JSONB NOT NULL,
    edges        JSONB NOT NULL,
    trigger_cfg  JSONB NOT NULL,
    note         TEXT NOT NULL DEFAULT '',
    published_by UUID,
    published_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (flow_id, version)
);

CREATE TABLE IF NOT EXISTS ax_flow_deployments (
    tenant_id   TEXT NOT NULL,
    flow_id     UUID NOT NULL REFERENCES ax_flows(id) ON DELETE CASCADE,
    environment TEXT NOT NULL CHECK (environment IN ('TEST','PROD')),
    version     INT NOT NULL,
    deployed_by UUID,
    deployed_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (flow_id, environment)
);

CREATE TABLE IF NOT EXISTS ax_flow_runs (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id       TEXT NOT NULL,
    flow_id         UUID NOT NULL REFERENCES ax_flows(id) ON DELETE CASCADE,
    version         INT NOT NULL,
    environment     TEXT NOT NULL CHECK (environment IN ('TEST','PROD')),
    trigger_source  TEXT NOT NULL CHECK (trigger_source IN ('MANUAL','CRON','OUTBOX_EVENT','WEBHOOK','API','MCP','RULE','SIM_APPLY')),
    trigger_payload JSONB NOT NULL DEFAULT '{}'::jsonb,
    idempotency_key TEXT,
    status          TEXT NOT NULL DEFAULT 'PENDING'
                    CHECK (status IN ('PENDING','RUNNING','WAITING_APPROVAL','SUCCEEDED','FAILED','CANCELED')),
    outputs         JSONB NOT NULL DEFAULT '{}'::jsonb,
    error_message   TEXT,
    started_by      UUID,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    started_at      TIMESTAMPTZ,
    finished_at     TIMESTAMPTZ,
    UNIQUE (flow_id, idempotency_key)
);
CREATE INDEX IF NOT EXISTS idx_ax_flow_runs_flow ON ax_flow_runs (tenant_id, flow_id, created_at DESC);

CREATE TABLE IF NOT EXISTS ax_flow_step_logs (
    id          BIGSERIAL PRIMARY KEY,
    tenant_id   TEXT NOT NULL,
    run_id      UUID NOT NULL REFERENCES ax_flow_runs(id) ON DELETE CASCADE,
    node_id     TEXT NOT NULL,
    attempt     INT NOT NULL DEFAULT 1,
    status      TEXT NOT NULL CHECK (status IN ('PENDING','RUNNING','WAITING_APPROVAL','SUCCEEDED','FAILED','SKIPPED')),
    job_id      UUID,
    input       JSONB NOT NULL DEFAULT '{}'::jsonb,
    output      JSONB,
    output_ref  TEXT,                         -- 输出超过 1 MB 时写文件，相对 AXPOINT_DATA_ROOT
    error_code  TEXT,
    error_message TEXT,
    started_at  TIMESTAMPTZ,
    finished_at TIMESTAMPTZ,
    UNIQUE (run_id, node_id, attempt)
);

CREATE TABLE IF NOT EXISTS ax_approvals (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id       TEXT NOT NULL,
    run_id          UUID REFERENCES ax_flow_runs(id) ON DELETE CASCADE,
    node_id         TEXT,
    source_kind     TEXT NOT NULL DEFAULT 'FLOW' CHECK (source_kind IN ('FLOW','SIM_APPLY','AGENT')),
    source_ref      TEXT NOT NULL DEFAULT '',
    title           TEXT NOT NULL,
    detail          JSONB NOT NULL DEFAULT '{}'::jsonb,
    assignee_roles  TEXT[] NOT NULL DEFAULT '{}',
    assignee_users  UUID[] NOT NULL DEFAULT '{}',
    status          TEXT NOT NULL DEFAULT 'PENDING' CHECK (status IN ('PENDING','APPROVED','REJECTED','EXPIRED')),
    decided_by      UUID,
    decided_at      TIMESTAMPTZ,
    comment         TEXT NOT NULL DEFAULT '',
    expires_at      TIMESTAMPTZ,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS idx_ax_approvals_pending ON ax_approvals (tenant_id, status) WHERE status = 'PENDING';

CREATE TABLE IF NOT EXISTS ax_action_log (
    id          BIGSERIAL PRIMARY KEY,
    tenant_id   TEXT NOT NULL,
    run_id      UUID,
    node_id     TEXT,
    source_kind TEXT NOT NULL CHECK (source_kind IN ('FLOW','SIM_APPLY','AGENT','RULE','VIEW')),
    action_kind TEXT NOT NULL CHECK (action_kind IN ('WRITE_NODE_METRIC','WRITE_NODE_PROPERTY','HTTP','NOTIFY','WORK_ITEM')),
    target      TEXT NOT NULL,
    payload     JSONB NOT NULL,
    status      TEXT NOT NULL CHECK (status IN ('DONE','DRY_RUN','FAILED')),
    idempotency_key TEXT NOT NULL,
    error_message TEXT,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, idempotency_key)
);

CREATE TABLE IF NOT EXISTS ax_flow_hooks (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id     TEXT NOT NULL,
    flow_id       UUID NOT NULL REFERENCES ax_flows(id) ON DELETE CASCADE,
    credential_id UUID NOT NULL REFERENCES ds_credentials(id),   -- Bearer 秘密，密钥环加密
    enabled       BOOLEAN NOT NULL DEFAULT TRUE,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (flow_id)
);

SELECT ax_enable_tenant_rls(t) FROM unnest(ARRAY[
    'ax_flow_versions','ax_flow_deployments','ax_flow_runs','ax_flow_step_logs','ax_approvals','ax_action_log','ax_flow_hooks'
]::regclass[]) AS t;

GRANT USAGE, SELECT ON SEQUENCE ax_flow_step_logs_id_seq TO axpoint_app;
GRANT USAGE, SELECT ON SEQUENCE ax_action_log_id_seq TO axpoint_app;

CREATE OR REPLACE FUNCTION ax_scheduled_flows()
RETURNS TABLE (tenant_id TEXT, flow_id UUID, version INT, trigger_cfg JSONB, last_run_at TIMESTAMPTZ)
LANGUAGE sql SECURITY DEFINER SET search_path = public AS $$
    SELECT d.tenant_id, d.flow_id, d.version, v.trigger_cfg,
           (SELECT max(r.created_at) FROM ax_flow_runs r WHERE r.flow_id = d.flow_id AND r.trigger_source = 'CRON')
      FROM ax_flow_deployments d
      JOIN ax_flow_versions v ON v.flow_id = d.flow_id AND v.version = d.version
     WHERE d.environment = 'PROD' AND v.trigger_cfg->>'type' = 'CRON';
$$;

CREATE OR REPLACE FUNCTION ax_event_flows(p_topic TEXT)
RETURNS TABLE (tenant_id TEXT, flow_id UUID, version INT, trigger_cfg JSONB)
LANGUAGE sql SECURITY DEFINER SET search_path = public AS $$
    SELECT d.tenant_id, d.flow_id, d.version, v.trigger_cfg
      FROM ax_flow_deployments d
      JOIN ax_flow_versions v ON v.flow_id = d.flow_id AND v.version = d.version
     WHERE d.environment = 'PROD' AND v.trigger_cfg->>'type' = 'OUTBOX_EVENT' AND v.trigger_cfg->>'topic' = p_topic;
$$;

CREATE OR REPLACE FUNCTION ax_resolve_flow_hook(p_tenant TEXT, p_slug TEXT)
RETURNS TABLE (flow_id UUID, credential_id UUID)
LANGUAGE sql SECURITY DEFINER SET search_path = public AS $$
    SELECT f.id, h.credential_id
      FROM ax_flows f JOIN ax_flow_hooks h ON h.flow_id = f.id
     WHERE f.tenant_id = p_tenant AND f.slug = p_slug AND h.enabled;
$$;

REVOKE ALL ON FUNCTION ax_scheduled_flows() FROM PUBLIC;
REVOKE ALL ON FUNCTION ax_event_flows(TEXT) FROM PUBLIC;
REVOKE ALL ON FUNCTION ax_resolve_flow_hook(TEXT, TEXT) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION ax_scheduled_flows() TO axpoint_app;
GRANT EXECUTE ON FUNCTION ax_event_flows(TEXT) TO axpoint_app;
GRANT EXECUTE ON FUNCTION ax_resolve_flow_hook(TEXT, TEXT) TO axpoint_app;
