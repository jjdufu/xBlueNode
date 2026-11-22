-- 0016_grounded_ask_eval.sql：问数轮次接地路径审计列与双轨反幻觉评测记录表

ALTER TABLE an_ask_turns
    ADD COLUMN IF NOT EXISTS grounding_mode TEXT NOT NULL DEFAULT 'GUARDED_SQL'
        CHECK (grounding_mode IN (
            'SEMANTIC_METRIC',
            'DOMAIN_OBJECT',
            'GUARDED_SQL',
            'GROUNDED_DOC',
            'ABSTAINED',
            'CLARIFICATION'
        ));

ALTER TABLE an_ask_turns
    ADD COLUMN IF NOT EXISTS verified_numbers INT NOT NULL DEFAULT 0
        CHECK (verified_numbers >= 0);

ALTER TABLE an_ask_turns
    ADD COLUMN IF NOT EXISTS verified_citations INT NOT NULL DEFAULT 0
        CHECK (verified_citations >= 0);

ALTER TABLE an_ask_turns
    ADD COLUMN IF NOT EXISTS abstained BOOLEAN NOT NULL DEFAULT FALSE;

ALTER TABLE an_ask_turns
    ADD COLUMN IF NOT EXISTS slot_payload JSONB;

CREATE INDEX IF NOT EXISTS idx_an_ask_turns_grounding
    ON an_ask_turns (tenant_id, grounding_mode, created_at DESC);

CREATE TABLE IF NOT EXISTS an_grounding_eval_runs (
    id                 UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id          TEXT NOT NULL,
    suite_name         TEXT NOT NULL,
    track              TEXT NOT NULL CHECK (track IN ('CLOUD', 'LOCAL_7B')),
    model_name         TEXT NOT NULL,
    total_cases        INT NOT NULL CHECK (total_cases > 0),
    intercepted_cases  INT NOT NULL CHECK (intercepted_cases >= 0),
    pass_rate          DOUBLE PRECISION NOT NULL CHECK (pass_rate >= 0.0 AND pass_rate <= 1.0),
    category_breakdown JSONB NOT NULL DEFAULT '{}'::jsonb,
    failures           JSONB NOT NULL DEFAULT '[]'::jsonb,
    duration_ms        BIGINT NOT NULL DEFAULT 0,
    created_at         TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_an_grounding_eval_runs_lookup
    ON an_grounding_eval_runs (tenant_id, created_at DESC);

SELECT ax_enable_tenant_rls('an_grounding_eval_runs');
