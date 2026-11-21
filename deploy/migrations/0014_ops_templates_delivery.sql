-- 0014_ops_templates_delivery.sql：工作项、模板安装记录、备份记录、审计链锚点

CREATE TABLE IF NOT EXISTS ax_work_items (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id   TEXT NOT NULL,
    item_no     BIGINT NOT NULL,                     -- 租户内递增编号，界面显示 WI-<n>
    title       TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    item_kind   TEXT NOT NULL DEFAULT 'TASK' CHECK (item_kind IN ('TASK','ISSUE','CHANGE')),
    status      TEXT NOT NULL DEFAULT 'OPEN' CHECK (status IN ('OPEN','IN_PROGRESS','BLOCKED','DONE','CANCELED')),
    priority    TEXT NOT NULL DEFAULT 'P2' CHECK (priority IN ('P0','P1','P2','P3')),
    assignee    UUID REFERENCES sec_users(id) ON DELETE SET NULL,
    source_kind TEXT NOT NULL DEFAULT 'MANUAL' CHECK (source_kind IN ('MANUAL','FLOW_ACTION','ALERT','SIM_DECISION','TEMPLATE')),
    source_ref  TEXT NOT NULL DEFAULT '',
    node_ids    UUID[] NOT NULL DEFAULT '{}',
    due_at      TIMESTAMPTZ,
    created_by  UUID,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    closed_at   TIMESTAMPTZ,
    UNIQUE (tenant_id, item_no)
);
CREATE INDEX IF NOT EXISTS idx_ax_work_items_status ON ax_work_items (tenant_id, status, priority);

CREATE TABLE IF NOT EXISTS ax_work_item_events (
    id           BIGSERIAL PRIMARY KEY,
    tenant_id    TEXT NOT NULL,
    work_item_id UUID NOT NULL REFERENCES ax_work_items(id) ON DELETE CASCADE,
    event_kind   TEXT NOT NULL CHECK (event_kind IN ('CREATED','STATUS','ASSIGN','COMMENT','LINK')),
    detail       JSONB NOT NULL DEFAULT '{}'::jsonb,
    actor        UUID,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS idx_ax_work_item_events_item ON ax_work_item_events (tenant_id, work_item_id, id ASC);

CREATE TABLE IF NOT EXISTS ax_template_installs (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id     TEXT NOT NULL,
    pack_id       TEXT NOT NULL,
    pack_version  TEXT NOT NULL,
    status        TEXT NOT NULL CHECK (status IN ('PLANNED','INSTALLING','INSTALLED','FAILED','UNINSTALLED')),
    plan          JSONB NOT NULL,                      -- 安装计划：对象清单与冲突
    object_map    JSONB NOT NULL DEFAULT '{}'::jsonb,  -- 模板对象键 → 创建出的对象 ID 与内容哈希
    error_message TEXT,
    installed_by  UUID,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    finished_at   TIMESTAMPTZ
);
CREATE UNIQUE INDEX IF NOT EXISTS uq_ax_template_active ON ax_template_installs (tenant_id, pack_id)
    WHERE status IN ('INSTALLING','INSTALLED');

-- 平台级备份记录（不按租户隔离；只有 default 租户中具备 OPS_ADMIN 的主体可访问）
CREATE TABLE IF NOT EXISTS ax_backups (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    backup_kind   TEXT NOT NULL CHECK (backup_kind IN ('FULL','DATABASE','LAKE')),
    status        TEXT NOT NULL CHECK (status IN ('PENDING','RUNNING','SUCCEEDED','FAILED','VERIFIED')),
    location      TEXT NOT NULL DEFAULT '',
    size_bytes    BIGINT,
    manifest      JSONB NOT NULL DEFAULT '{}'::jsonb,  -- 文件清单与 SHA-256、迁移版本、湖快照号
    error_message TEXT,
    started_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    finished_at   TIMESTAMPTZ,
    verified_at   TIMESTAMPTZ
);
GRANT SELECT, INSERT, UPDATE ON ax_backups TO axpoint_app;

-- 审计保留清理后，验证从锚点开始
ALTER TABLE sec_audit_chain_heads ADD COLUMN IF NOT EXISTS anchor_id   BIGINT;
ALTER TABLE sec_audit_chain_heads ADD COLUMN IF NOT EXISTS anchor_hash TEXT;

SELECT ax_enable_tenant_rls(t) FROM unnest(ARRAY[
    'ax_work_items','ax_work_item_events','ax_template_installs'
]::regclass[]) AS t;

GRANT USAGE, SELECT ON SEQUENCE ax_work_item_events_id_seq TO axpoint_app;
