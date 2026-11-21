#!/usr/bin/env bash
# 按租户清理超期 sec_audit_logs，并在同一事务中更新 sec_audit_chain_heads 锚点与写入 POLICY_CHANGE 审计记录（§14.4.2）
set -euo pipefail
: "${MIGRATION_DATABASE_URL:?MIGRATION_DATABASE_URL is required}"

FORCE_KEEP="${AXPOINT_AUDIT_FORCE_KEEP_DAYS:-}"
if [[ -n "$FORCE_KEEP" && ! "$FORCE_KEEP" =~ ^[0-9]+$ ]]; then
  echo "retention-audit: invalid AXPOINT_AUDIT_FORCE_KEEP_DAYS='$FORCE_KEEP'" >&2
  exit 2
fi

psql "$MIGRATION_DATABASE_URL" -v ON_ERROR_STOP=1 -v force_keep="${FORCE_KEEP}" <<'SQL' >/dev/null
SELECT set_config('axpoint.force_keep', :'force_keep', false);
DO $$
DECLARE
    v_force_raw   TEXT := NULLIF(current_setting('axpoint.force_keep', true), '');
    r             RECORD;
    v_keep_days   INT;
    v_deleted     BIGINT;
    v_prev_hash   TEXT;
    v_new_id      BIGINT;
    v_ts          TIMESTAMPTZ;
    v_ts_rfc3339  TEXT;
    v_canonical   TEXT;
    v_row_hash    TEXT;
    v_min_id      BIGINT;
    v_min_prev    TEXT;
BEGIN
    FOR r IN SELECT tenant_id FROM sec_tenants WHERE status = 'ACTIVE' ORDER BY tenant_id LOOP
        IF v_force_raw IS NOT NULL THEN
            v_keep_days := v_force_raw::INT;
        ELSE
            SELECT keep_days INTO v_keep_days
              FROM ax_retention_policies
             WHERE tenant_id = r.tenant_id AND target = 'AUDIT';
            v_keep_days := COALESCE(v_keep_days, 1095);
        END IF;

        IF v_keep_days <= 0 THEN
            CONTINUE;
        END IF;

        -- 串行化该租户的审计链写入
        PERFORM pg_advisory_xact_lock(hashtext('audit_chain:' || r.tenant_id));
        SELECT last_hash INTO v_prev_hash
          FROM sec_audit_chain_heads
         WHERE tenant_id = r.tenant_id
           FOR UPDATE;

        IF NOT FOUND THEN
            SELECT row_hash INTO v_prev_hash
              FROM sec_audit_logs
             WHERE tenant_id = r.tenant_id AND row_hash <> ''
             ORDER BY id DESC
             LIMIT 1;
            v_prev_hash := COALESCE(v_prev_hash, '');
        END IF;

        DELETE FROM sec_audit_logs
         WHERE tenant_id = r.tenant_id
           AND created_at < NOW() - make_interval(days => v_keep_days);
        GET DIAGNOSTICS v_deleted = ROW_COUNT;

        v_new_id := nextval('sec_audit_logs_id_seq');
        v_ts := date_trunc('microseconds', clock_timestamp());
        v_ts_rfc3339 := to_char(v_ts AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"');

        -- 与 axpoint_security::audit_chain::compute_audit_row_hash 严格一致的字典序紧凑 JSON
        v_canonical := format(
            '{"action":"POLICY_CHANGE","client_ip":"127.0.0.1","created_at":"%s","detail_json":{"retention_deleted":%s},"id":%s,"resource_ref":"retention:AUDIT","status_code":200,"tenant_id":"%s","trace_id":"","user_id":null,"username":"system"}',
            v_ts_rfc3339,
            v_deleted,
            v_new_id,
            r.tenant_id
        );
        v_row_hash := encode(pg_catalog.sha256(convert_to(v_prev_hash || v_canonical, 'UTF8')), 'hex');

        INSERT INTO sec_audit_logs (
            id, tenant_id, user_id, username, action, resource_ref,
            status_code, client_ip, trace_id, detail_json, prev_hash, row_hash, created_at
        ) VALUES (
            v_new_id, r.tenant_id, NULL, 'system', 'POLICY_CHANGE', 'retention:AUDIT',
            200, '127.0.0.1', '', jsonb_build_object('retention_deleted', v_deleted),
            v_prev_hash, v_row_hash, v_ts
        );

        -- 锁定剩余最小有哈希审计行作为验证锚点（若历史行全被清理，则新写入的 POLICY_CHANGE 即为锚点）
        SELECT id, prev_hash INTO v_min_id, v_min_prev
          FROM sec_audit_logs
         WHERE tenant_id = r.tenant_id
           AND row_hash <> ''
         ORDER BY id ASC
         LIMIT 1;

        INSERT INTO sec_audit_chain_heads (tenant_id, last_id, last_hash, anchor_id, anchor_hash)
        VALUES (r.tenant_id, v_new_id, v_row_hash, v_min_id, v_min_prev)
        ON CONFLICT (tenant_id) DO UPDATE
           SET last_id     = EXCLUDED.last_id,
               last_hash   = EXCLUDED.last_hash,
               anchor_id   = EXCLUDED.anchor_id,
               anchor_hash = EXCLUDED.anchor_hash;
    END LOOP;
END $$;
SQL

echo "retention-audit: OK"
