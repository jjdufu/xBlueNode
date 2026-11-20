-- 0011_xblueview_dashboards.sql：驾驶舱、组件、资产、平面图摆放、3D 孪生场景绑定

CREATE TABLE IF NOT EXISTS xv_dashboards (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id       TEXT NOT NULL,
    name            TEXT NOT NULL,
    slug            TEXT NOT NULL CHECK (slug ~ '^[a-z0-9][a-z0-9-]{0,62}$'),
    description     TEXT NOT NULL DEFAULT '',
    filters         JSONB NOT NULL DEFAULT '[]'::jsonb,
    theme           TEXT NOT NULL DEFAULT 'light' CHECK (theme IN ('light','dark','screen')),
    canvas          JSONB NOT NULL DEFAULT '{"cols":24,"row_height":30,"screen_width":1920,"screen_height":1080}'::jsonb,
    status          TEXT NOT NULL DEFAULT 'DRAFT' CHECK (status IN ('DRAFT','PUBLISHED')),
    version         INT NOT NULL DEFAULT 1,
    created_by      UUID,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, slug)
);

CREATE TABLE IF NOT EXISTS xv_widgets (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id       TEXT NOT NULL,
    dashboard_id    UUID NOT NULL REFERENCES xv_dashboards(id) ON DELETE CASCADE,
    widget_kind     TEXT NOT NULL CHECK (widget_kind IN (
        'KPI_CARD','TABLE','PIVOT_TABLE','LINE_CHART','BAR_CHART','PIE_CHART','SCATTER','SANKEY','HEATMAP',
        'GAUGE','RADAR','GRAPH_TOPOLOGY','MAP','FLOOR_PLAN','SCENE_3D','ALERT_LIST','SIM_COMPARE',
        'ACTION_BUTTON','ACTION_FORM','MARKDOWN_NOTE')),
    title           TEXT NOT NULL DEFAULT '',
    position        JSONB NOT NULL,
    data_source     JSONB NOT NULL DEFAULT '{"kind":"STATIC"}'::jsonb,
    encoding        JSONB NOT NULL DEFAULT '{}'::jsonb,
    options         JSONB NOT NULL DEFAULT '{}'::jsonb,
    drill           JSONB NOT NULL DEFAULT '[]'::jsonb,
    refresh_mode    TEXT NOT NULL DEFAULT 'LIVE' CHECK (refresh_mode IN ('LIVE','INTERVAL','MANUAL')),
    refresh_seconds INT NOT NULL DEFAULT 60 CHECK (refresh_seconds BETWEEN 5 AND 86400),
    deps            JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS idx_xv_widgets_dashboard ON xv_widgets (dashboard_id);
CREATE INDEX IF NOT EXISTS idx_xv_widgets_deps ON xv_widgets USING GIN (deps);

CREATE TABLE IF NOT EXISTS xv_assets (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id   TEXT NOT NULL,
    asset_kind  TEXT NOT NULL CHECK (asset_kind IN ('FLOOR_PLAN_IMAGE','GEOJSON','ICON','MODEL_3D')),
    name        TEXT NOT NULL,
    file_path   TEXT NOT NULL,
    mime_type   TEXT NOT NULL,
    sha256      TEXT NOT NULL,
    width_px    INT,
    height_px   INT,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, name)
);

CREATE TABLE IF NOT EXISTS xv_floor_placements (
    tenant_id  TEXT NOT NULL,
    asset_id   UUID NOT NULL REFERENCES xv_assets(id) ON DELETE CASCADE,
    node_id    UUID NOT NULL REFERENCES xb_nodes(id) ON DELETE CASCADE,
    x_ratio    DOUBLE PRECISION NOT NULL CHECK (x_ratio BETWEEN 0 AND 1),
    y_ratio    DOUBLE PRECISION NOT NULL CHECK (y_ratio BETWEEN 0 AND 1),
    PRIMARY KEY (asset_id, node_id)
);

CREATE TABLE IF NOT EXISTS xv_scene3d_bindings (
    tenant_id  TEXT NOT NULL,
    asset_id   UUID NOT NULL REFERENCES xv_assets(id) ON DELETE CASCADE,
    node_id    UUID NOT NULL REFERENCES xb_nodes(id) ON DELETE CASCADE,
    mesh_name  TEXT NOT NULL DEFAULT '',
    pos_x      DOUBLE PRECISION NOT NULL DEFAULT 0,
    pos_y      DOUBLE PRECISION NOT NULL DEFAULT 0,
    pos_z      DOUBLE PRECISION NOT NULL DEFAULT 0,
    scale      DOUBLE PRECISION NOT NULL DEFAULT 1 CHECK (scale > 0),
    PRIMARY KEY (asset_id, node_id)
);

SELECT ax_enable_tenant_rls(t) FROM unnest(ARRAY[
    'xv_dashboards','xv_widgets','xv_assets','xv_floor_placements','xv_scene3d_bindings'
]::regclass[]) AS t;
