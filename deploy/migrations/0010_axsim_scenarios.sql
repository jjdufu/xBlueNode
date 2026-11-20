-- 0010_axsim_scenarios.sql：沙盘范围与基线快照、写时复制覆盖层、场景、检查点、决策

ALTER TABLE ax_sandboxes ADD COLUMN IF NOT EXISTS scope            JSONB NOT NULL DEFAULT '{}'::jsonb;
ALTER TABLE ax_sandboxes ADD COLUMN IF NOT EXISTS status           TEXT NOT NULL DEFAULT 'READY'
    CHECK (status IN ('CLONING','READY','ARCHIVED'));
ALTER TABLE ax_sandboxes ADD COLUMN IF NOT EXISTS node_count       INT NOT NULL DEFAULT 0;
ALTER TABLE ax_sandboxes ADD COLUMN IF NOT EXISTS link_count       INT NOT NULL DEFAULT 0;
ALTER TABLE ax_sandboxes ADD COLUMN IF NOT EXISTS base_captured_at TIMESTAMPTZ;
ALTER TABLE ax_sandboxes ADD COLUMN IF NOT EXISTS schema_version   INT;
ALTER TABLE ax_sandboxes ADD COLUMN IF NOT EXISTS created_by       UUID;
ALTER TABLE ax_sandboxes ADD COLUMN IF NOT EXISTS updated_at       TIMESTAMPTZ NOT NULL DEFAULT NOW();

CREATE TABLE IF NOT EXISTS ax_sim_base_nodes (
    tenant_id   TEXT NOT NULL,
    sandbox_id  UUID NOT NULL REFERENCES ax_sandboxes(id) ON DELETE CASCADE,
    node_id     UUID NOT NULL,
    type_name   TEXT NOT NULL,
    node_key    TEXT NOT NULL,
    title       TEXT NOT NULL,
    metrics     JSONB NOT NULL,
    properties  JSONB NOT NULL,
    row_version BIGINT NOT NULL,                -- 克隆时生产节点的 row_version，用于下发冲突检测
    PRIMARY KEY (sandbox_id, node_id)
);

CREATE TABLE IF NOT EXISTS ax_sim_base_links (
    tenant_id   TEXT NOT NULL,
    sandbox_id  UUID NOT NULL REFERENCES ax_sandboxes(id) ON DELETE CASCADE,
    link_id     UUID NOT NULL,
    link_type   TEXT NOT NULL,
    src_node_id UUID NOT NULL,
    dst_node_id UUID NOT NULL,
    weight      DOUBLE PRECISION NOT NULL,
    PRIMARY KEY (sandbox_id, link_id)
);

CREATE TABLE IF NOT EXISTS ax_sim_overlay_nodes (
    tenant_id  TEXT NOT NULL,
    sandbox_id UUID NOT NULL REFERENCES ax_sandboxes(id) ON DELETE CASCADE,
    node_id    UUID NOT NULL,                   -- 沙盘内新建的节点使用新 UUID
    type_name  TEXT,
    node_key   TEXT,
    title      TEXT,
    metrics    JSONB,                           -- 只含被修改的键，合并到基线
    properties JSONB,
    is_new     BOOLEAN NOT NULL DEFAULT FALSE,
    deleted    BOOLEAN NOT NULL DEFAULT FALSE,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (sandbox_id, node_id)
);

CREATE TABLE IF NOT EXISTS ax_sim_overlay_links (
    tenant_id   TEXT NOT NULL,
    sandbox_id  UUID NOT NULL REFERENCES ax_sandboxes(id) ON DELETE CASCADE,
    link_id     UUID NOT NULL,
    link_type   TEXT,
    src_node_id UUID,
    dst_node_id UUID,
    weight      DOUBLE PRECISION,
    is_new      BOOLEAN NOT NULL DEFAULT FALSE,
    deleted     BOOLEAN NOT NULL DEFAULT FALSE,
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (sandbox_id, link_id)
);

CREATE TABLE IF NOT EXISTS ax_sim_scenarios (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id     TEXT NOT NULL,
    sandbox_id    UUID NOT NULL REFERENCES ax_sandboxes(id) ON DELETE CASCADE,
    name          TEXT NOT NULL,
    description   TEXT NOT NULL DEFAULT '',
    actions       JSONB NOT NULL,               -- AxSimAction[]
    params        JSONB NOT NULL,               -- SimParams
    overlay_hash  TEXT NOT NULL,                -- 运行时覆盖层内容哈希，覆盖层变化后结果标记为过期
    result        JSONB,                        -- SimResult 摘要（Top 200 影响、汇总 KPI、收敛信息）
    status        TEXT NOT NULL DEFAULT 'PENDING' CHECK (status IN ('PENDING','RUNNING','SUCCEEDED','FAILED')),
    error_message TEXT,
    created_by    UUID,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    finished_at   TIMESTAMPTZ,
    UNIQUE (sandbox_id, name)
);

CREATE INDEX IF NOT EXISTS idx_ax_sim_scenarios_sandbox
    ON ax_sim_scenarios (sandbox_id, created_at DESC);

CREATE TABLE IF NOT EXISTS ax_sim_impacts (
    tenant_id    TEXT NOT NULL,
    scenario_id  UUID NOT NULL REFERENCES ax_sim_scenarios(id) ON DELETE CASCADE,
    node_id      UUID NOT NULL,
    metric       TEXT NOT NULL,
    before_val   DOUBLE PRECISION,              -- ABSOLUTE 干预目标节点缺失该指标时为 NULL
    after_val    DOUBLE PRECISION NOT NULL,
    delta        DOUBLE PRECISION,
    delta_ratio  DOUBLE PRECISION,
    p10          DOUBLE PRECISION,
    p90          DOUBLE PRECISION,
    hop_distance INT NOT NULL,
    top_path     JSONB NOT NULL DEFAULT '[]'::jsonb,   -- 贡献最大的传导路径 [node_id...]
    PRIMARY KEY (scenario_id, node_id, metric)
);

CREATE INDEX IF NOT EXISTS idx_ax_sim_impacts_scenario_metric
    ON ax_sim_impacts (scenario_id, metric);

CREATE TABLE IF NOT EXISTS ax_sim_checkpoints (
    tenant_id   TEXT NOT NULL,
    scenario_id UUID NOT NULL REFERENCES ax_sim_scenarios(id) ON DELETE CASCADE,
    seq         INT NOT NULL,                   -- 分步模式为时间步；稳态模式为迭代轮次采样
    label       TEXT NOT NULL,
    changes     JSONB NOT NULL,                 -- 相对上一检查点变化的 {node_id: {metric: value}}
    PRIMARY KEY (scenario_id, seq)
);

CREATE TABLE IF NOT EXISTS ax_sim_decisions (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id       TEXT NOT NULL,
    scenario_id     UUID NOT NULL REFERENCES ax_sim_scenarios(id) ON DELETE CASCADE,
    title           TEXT NOT NULL,
    changes         JSONB NOT NULL,             -- [{node_id, metric, base_value, target_value}]
    trigger_flow_id UUID REFERENCES ax_flows(id) ON DELETE SET NULL,
    status          TEXT NOT NULL DEFAULT 'PENDING_APPROVAL'
                    CHECK (status IN ('PENDING_APPROVAL','APPROVED','APPLIED','REJECTED','CONFLICT','FAILED')),
    approval_id     UUID,
    conflict_report JSONB NOT NULL DEFAULT '[]'::jsonb,
    tolerance_ratio DOUBLE PRECISION NOT NULL DEFAULT 0.0,
    created_by      UUID,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    applied_at      TIMESTAMPTZ,
    applied_by      UUID
);

CREATE INDEX IF NOT EXISTS idx_ax_sim_decisions_scenario
    ON ax_sim_decisions (scenario_id, created_at DESC);

SELECT ax_enable_tenant_rls(t) FROM unnest(ARRAY[
    'ax_sim_base_nodes','ax_sim_base_links','ax_sim_overlay_nodes','ax_sim_overlay_links',
    'ax_sim_scenarios','ax_sim_impacts','ax_sim_checkpoints','ax_sim_decisions'
]::regclass[]) AS t;
