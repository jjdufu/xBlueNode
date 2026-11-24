-- 0023_sim_events_and_geoshape.sql：空间多边形几何列与推演沙盘事件队列及最晚干预窗口诊断

ALTER TABLE xb_nodes
    ADD COLUMN IF NOT EXISTS geo_shape JSONB;

ALTER TABLE ax_sim_base_nodes
    ADD COLUMN IF NOT EXISTS geo_lng DOUBLE PRECISION,
    ADD COLUMN IF NOT EXISTS geo_lat DOUBLE PRECISION,
    ADD COLUMN IF NOT EXISTS geo_shape JSONB;

ALTER TABLE ax_sim_overlay_nodes
    ADD COLUMN IF NOT EXISTS geo_lng DOUBLE PRECISION,
    ADD COLUMN IF NOT EXISTS geo_lat DOUBLE PRECISION,
    ADD COLUMN IF NOT EXISTS geo_shape JSONB;

ALTER TABLE ax_sim_scenarios
    ADD COLUMN IF NOT EXISTS scheduled_events JSONB NOT NULL DEFAULT '[]'::jsonb,
    ADD COLUMN IF NOT EXISTS goal_constraints JSONB NOT NULL DEFAULT '[]'::jsonb,
    ADD COLUMN IF NOT EXISTS fired_events JSONB NOT NULL DEFAULT '[]'::jsonb,
    ADD COLUMN IF NOT EXISTS deadline_report JSONB;
