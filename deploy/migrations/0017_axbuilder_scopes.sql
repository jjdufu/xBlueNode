-- 0017_axbuilder_scopes.sql：AxBuilder 构建令牌档位、前端深链基址、开发子图沙盒与运行态节点子图隔离

-- 1. 扩展个人访问令牌：区分通用令牌与受限构建令牌，记录可选前端深链基址
ALTER TABLE sec_api_tokens
    ADD COLUMN IF NOT EXISTS token_profile TEXT NOT NULL DEFAULT 'GENERAL'
        CHECK (token_profile IN ('GENERAL', 'BUILDER'));

ALTER TABLE sec_api_tokens
    ADD COLUMN IF NOT EXISTS web_base_url TEXT;

-- 更新令牌解析函数，向认证层返回 token_profile 与 web_base_url
DROP FUNCTION IF EXISTS ax_resolve_api_token(TEXT);
CREATE FUNCTION ax_resolve_api_token(p_token_hash TEXT)
RETURNS TABLE (
    token_id      UUID,
    tenant_id     TEXT,
    user_id       UUID,
    scopes        TEXT[],
    token_profile TEXT,
    web_base_url  TEXT
)
LANGUAGE sql SECURITY DEFINER SET search_path = public AS $$
    SELECT t.id, t.tenant_id, t.user_id, t.scopes, t.token_profile, t.web_base_url
      FROM sec_api_tokens t
     WHERE t.token_hash = p_token_hash
       AND t.revoked_at IS NULL
       AND (t.expires_at IS NULL OR t.expires_at > NOW());
$$;
REVOKE ALL ON FUNCTION ax_resolve_api_token(TEXT) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION ax_resolve_api_token(TEXT) TO axpoint_app;

-- 2. 扩展业务子图元数据：标识开发沙盒、来源模板包与最近清空时间
ALTER TABLE xb_subgraphs
    ADD COLUMN IF NOT EXISTS is_dev_sandbox BOOLEAN NOT NULL DEFAULT FALSE;

ALTER TABLE xb_subgraphs
    ADD COLUMN IF NOT EXISTS source_template_id TEXT;

ALTER TABLE xb_subgraphs
    ADD COLUMN IF NOT EXISTS last_reset_at TIMESTAMPTZ;

ALTER TABLE xb_subgraphs DROP CONSTRAINT IF EXISTS chk_xb_subgraphs_main_not_sandbox;
ALTER TABLE xb_subgraphs ADD CONSTRAINT chk_xb_subgraphs_main_not_sandbox
    CHECK (NOT (is_main AND is_dev_sandbox));

-- 3. 运行态业务节点与关系按 subgraph_slug 物理隔离，保障开发子图试灌与清空绝不影响企业主图 main
ALTER TABLE xb_nodes
    ADD COLUMN IF NOT EXISTS subgraph_slug TEXT NOT NULL DEFAULT 'main';

ALTER TABLE xb_nodes DROP CONSTRAINT IF EXISTS xb_nodes_tenant_id_type_name_node_key_key;
CREATE UNIQUE INDEX IF NOT EXISTS uq_xb_nodes_subgraph_key
    ON xb_nodes (tenant_id, subgraph_slug, type_name, node_key);
CREATE INDEX IF NOT EXISTS idx_xb_nodes_subgraph_type
    ON xb_nodes (tenant_id, subgraph_slug, type_name);

ALTER TABLE xb_links
    ADD COLUMN IF NOT EXISTS subgraph_slug TEXT NOT NULL DEFAULT 'main';

DROP INDEX IF EXISTS uq_xb_links_endpoints;
CREATE UNIQUE INDEX IF NOT EXISTS uq_xb_links_subgraph_endpoints
    ON xb_links (tenant_id, subgraph_slug, link_type, src_node_id, dst_node_id);
CREATE INDEX IF NOT EXISTS idx_xb_links_subgraph
    ON xb_links (tenant_id, subgraph_slug);

-- 4. 扩展审计日志动作集合（含 Step 17 构建变更与 Step 19 模板演练克隆动作）
ALTER TABLE sec_audit_logs DROP CONSTRAINT IF EXISTS sec_audit_logs_action_check;
ALTER TABLE sec_audit_logs ADD CONSTRAINT sec_audit_logs_action_check
    CHECK (action IN (
        'LOGIN','LOGOUT','TOKEN_ISSUE','TOKEN_REVOKE','QUERY_SQL','ASK_DATA','RAG_ASK','DATA_SYNC',
        'FILE_UPLOAD','SCHEMA_PUBLISH','FLOW_RUN','APPROVAL_DECIDE','MCP_CALL','AGENT_CHAT','SIM_APPLY',
        'EXPORT_FILE','SHARE_CREATE','SHARE_ACCESS','POLICY_CHANGE','BACKUP_RUN','TEMPLATE_INSTALL',
        'BUILDER_MCP_MUTATE','TRIAL_TEMPLATE_CLONE'
    ));
