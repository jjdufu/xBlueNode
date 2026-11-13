# 11 · Step 13：AxGuard 完整安全治理

Step 00 建立了租户隔离、开发令牌与个人访问令牌、权限校验和只追加审计。本步把安全治理补齐为生产形态：账号口令与 OIDC 登录、EdDSA 访问令牌与刷新令牌轮换、Cedar ABAC 资源策略、数据行级与列级策略（覆盖 SQL、业务节点接口、出品、智能体、问答与分享全部出口）、基于 Presidio 的 PII 识别与大模型出境过滤、跨部门授权审批、防篡改审计链，以及全部外部入口的统一鉴权清单。

---

## 13.0 目标、前置与契约

- **目标**：见上；任何数据出口都不能绕过行列策略；任何外部入口都有明确的令牌类型、校验方式与限流。
- **前置**：Step 12、Step 09 通过。
- **契约清单**：`0013_axguard_policies.sql`；`axpoint-security::{password, jwt, session, oidc, cedar_authz, row_policy, masking}`；`PolicyContext`（传入 `sql_guard`）；Sidecar `/pii/analyze`、`/pii/anonymize`；`/api/auth/*`（完整）、`/api/axguard/*`；页面 `axguard`（`/guard`）、`LoginPage`（账号口令与 OIDC）。

### 文件变更清单

| 动作 | 路径 | 内容 |
| :--- | :--- | :--- |
| 新建 | `deploy/migrations/0013_axguard_policies.sql` | §13.1 |
| 新建 | `axpoint-core/crates/axpoint-security/src/{password.rs, jwt.rs, session.rs, oidc.rs, cedar_authz.rs, row_policy.rs, masking.rs, audit_chain.rs}` | §13.3～§13.9 |
| 新建 | `axpoint-core/crates/axpoint-security/cedar/{schema.cedarschema, base.cedar}` | Cedar 模式与基线策略 |
| 新建 | `axpoint-core/crates/axpoint-security/tests/{jwt.rs, cedar.rs, row_policy.rs, masking.rs}`、`tests/data/mask_cases.json`、`tests/data/row_policy_cases.json` | 测试 |
| 修改 | `axpoint-core/crates/axpoint-security/src/authenticator.rs` | 第 4 分支：EdDSA 访问令牌 |
| 修改 | `axpoint-core/crates/axpoint-analytics/src/sql_guard.rs` | `GuardOptions.policy: Option<PolicyContext>`，表引用改写为安全子查询（§13.6） |
| 修改 | `axpoint-core/crates/axpoint-analytics/src/session.rs` | 注册脱敏标量函数（§13.7） |
| 修改 | `axpoint-core/crates/axpoint-db/src/topology.rs` 等节点查询 | 节点类型行策略编译为 PostgreSQL 条件，属性脱敏 |
| 修改 | `axpoint-core/crates/axpoint-llm/src/router.rs` | 出境 PII 过滤（§13.8.3） |
| 新建 | `axpoint-core/crates/axpoint-server/src/routes/{auth.rs（扩展）, axguard.rs}` | §13.10 路由 |
| 新建 | `axpoint-parser/pii/{__init__.py, recognizers.py, engine.py}` | Presidio 引擎与中文识别器 |
| 修改 | `axpoint-parser/main.py`、`pyproject.toml`、`deploy/docker/axpoint-parser.Dockerfile` | `/pii/*`；依赖与 spaCy 中文模型离线安装 |
| 新建 | `xbluenode-web/src/pages/AxGuardPage.tsx`、`src/components/axguard/{UsersPanel.tsx, RolesPanel.tsx, DeptTree.tsx, PolicyEditor.tsx, RowPolicyEditor.tsx, ColumnPolicyEditor.tsx, GrantsPanel.tsx, AuditExplorer.tsx}` | 管理界面 |
| 修改 | `xbluenode-web/src/pages/LoginPage.tsx`、`src/lib/api/auth.ts` | 账号口令、OIDC、静默刷新 |
| 新建 | `scripts/ops/{create-admin.sh, gen-jwt-key.sh, rotate-jwt-key.sh}` | 运维脚本 |
| 新建 | `scripts/gate/step13.sh` | 门禁 |

---

## 13.1 迁移 `deploy/migrations/0013_axguard_policies.sql`

```sql
-- 0013_axguard_policies.sql：登录安全、会话、部门、属性、ABAC 策略、行列策略、跨部门授权、审计链

ALTER TABLE sec_users ADD COLUMN IF NOT EXISTS failed_logins       INT NOT NULL DEFAULT 0;
ALTER TABLE sec_users ADD COLUMN IF NOT EXISTS locked_until        TIMESTAMPTZ;
ALTER TABLE sec_users ADD COLUMN IF NOT EXISTS last_login_at       TIMESTAMPTZ;
ALTER TABLE sec_users ADD COLUMN IF NOT EXISTS oidc_subject        TEXT;
ALTER TABLE sec_users ADD COLUMN IF NOT EXISTS tokens_valid_after  TIMESTAMPTZ NOT NULL DEFAULT 'epoch';
ALTER TABLE sec_users ADD COLUMN IF NOT EXISTS password_changed_at TIMESTAMPTZ;
CREATE UNIQUE INDEX IF NOT EXISTS uq_sec_users_oidc ON sec_users (tenant_id, oidc_subject) WHERE oidc_subject IS NOT NULL;

CREATE TABLE IF NOT EXISTS sec_departments (
    tenant_id TEXT NOT NULL,
    dept_id   TEXT NOT NULL CHECK (dept_id ~ '^[a-z0-9_-]{1,64}$'),
    name      TEXT NOT NULL,
    parent_id TEXT,
    PRIMARY KEY (tenant_id, dept_id)
);
INSERT INTO sec_departments (tenant_id, dept_id, name) VALUES ('default','default','默认部门') ON CONFLICT DO NOTHING;

CREATE TABLE IF NOT EXISTS sec_user_attributes (
    tenant_id TEXT NOT NULL,
    user_id   UUID NOT NULL REFERENCES sec_users(id) ON DELETE CASCADE,
    attr_key  TEXT NOT NULL CHECK (attr_key ~ '^[a-z][a-z0-9_]{0,62}$'),
    attr_values TEXT[] NOT NULL,
    PRIMARY KEY (user_id, attr_key)
);

-- 刷新令牌会话：同一登录形成一个 family，轮换后被替换的令牌作废；重放已作废令牌则整个 family 吊销
CREATE TABLE IF NOT EXISTS sec_sessions (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id     TEXT NOT NULL,
    user_id       UUID NOT NULL REFERENCES sec_users(id) ON DELETE CASCADE,
    family_id     UUID NOT NULL,
    refresh_hash  TEXT NOT NULL UNIQUE,
    auth_source   TEXT NOT NULL CHECK (auth_source IN ('LOCAL','OIDC')),
    user_agent    TEXT NOT NULL DEFAULT '',
    client_ip     TEXT NOT NULL DEFAULT '',
    expires_at    TIMESTAMPTZ NOT NULL,
    rotated_at    TIMESTAMPTZ,
    revoked_at    TIMESTAMPTZ,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS idx_sec_sessions_family ON sec_sessions (family_id);

-- OIDC 登录中间态（state → nonce、PKCE verifier），10 分钟有效，一次性
CREATE TABLE IF NOT EXISTS sec_login_states (
    state         TEXT PRIMARY KEY,
    tenant_id     TEXT NOT NULL,
    nonce         TEXT NOT NULL,
    pkce_verifier TEXT NOT NULL,
    return_to     TEXT NOT NULL DEFAULT '/',
    expires_at    TIMESTAMPTZ NOT NULL
);

-- 资源归属与密级
ALTER TABLE ds_datasets   ADD COLUMN IF NOT EXISTS owner_dept  TEXT NOT NULL DEFAULT 'default';
ALTER TABLE ds_datasets   ADD COLUMN IF NOT EXISTS sensitivity TEXT NOT NULL DEFAULT 'INTERNAL'
    CHECK (sensitivity IN ('PUBLIC','INTERNAL','CONFIDENTIAL','SECRET'));
ALTER TABLE xb_subgraphs  ADD COLUMN IF NOT EXISTS sensitivity TEXT NOT NULL DEFAULT 'INTERNAL'
    CHECK (sensitivity IN ('PUBLIC','INTERNAL','CONFIDENTIAL','SECRET'));
ALTER TABLE xb_node_types ADD COLUMN IF NOT EXISTS owner_dept  TEXT NOT NULL DEFAULT 'default';
ALTER TABLE xb_node_types ADD COLUMN IF NOT EXISTS sensitivity TEXT NOT NULL DEFAULT 'INTERNAL'
    CHECK (sensitivity IN ('PUBLIC','INTERNAL','CONFIDENTIAL','SECRET'));
ALTER TABLE xv_dashboards ADD COLUMN IF NOT EXISTS owner_dept  TEXT NOT NULL DEFAULT 'default';
ALTER TABLE xv_dashboards ADD COLUMN IF NOT EXISTS sensitivity TEXT NOT NULL DEFAULT 'INTERNAL'
    CHECK (sensitivity IN ('PUBLIC','INTERNAL','CONFIDENTIAL','SECRET'));
ALTER TABLE ax_agents     ADD COLUMN IF NOT EXISTS owner_dept  TEXT NOT NULL DEFAULT 'default';
ALTER TABLE ax_agents     ADD COLUMN IF NOT EXISTS sensitivity TEXT NOT NULL DEFAULT 'INTERNAL'
    CHECK (sensitivity IN ('PUBLIC','INTERNAL','CONFIDENTIAL','SECRET'));
ALTER TABLE sec_users     ADD COLUMN IF NOT EXISTS clearance   TEXT NOT NULL DEFAULT 'INTERNAL'
    CHECK (clearance IN ('PUBLIC','INTERNAL','CONFIDENTIAL','SECRET'));

-- 租户自定义 Cedar 策略（基线策略随代码发布，不入库）
CREATE TABLE IF NOT EXISTS sec_policies (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id   TEXT NOT NULL,
    name        TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    policy_text TEXT NOT NULL,
    enabled     BOOLEAN NOT NULL DEFAULT TRUE,
    version     INT NOT NULL DEFAULT 1,
    updated_by  UUID,
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, name)
);

CREATE TABLE IF NOT EXISTS sec_row_policies (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id    TEXT NOT NULL,
    name         TEXT NOT NULL,
    target_kind  TEXT NOT NULL CHECK (target_kind IN ('DATASET','NODE_TYPE')),
    target_ref   TEXT NOT NULL,                       -- 数据集 ID 或节点类型名
    applies_to   JSONB NOT NULL DEFAULT '{}'::jsonb,  -- {roles?, depts?, users?}；空对象 = 全部主体
    predicate    TEXT NOT NULL,                       -- 行策略表达式（§13.6.1）
    enabled      BOOLEAN NOT NULL DEFAULT TRUE,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (tenant_id, name)
);
CREATE TABLE IF NOT EXISTS sec_row_policy_defaults (
    tenant_id    TEXT NOT NULL,
    target_kind  TEXT NOT NULL CHECK (target_kind IN ('DATASET','NODE_TYPE')),
    target_ref   TEXT NOT NULL,
    default_deny BOOLEAN NOT NULL DEFAULT FALSE,      -- TRUE：没有任何匹配策略的主体看不到任何行
    PRIMARY KEY (tenant_id, target_kind, target_ref)
);

CREATE TABLE IF NOT EXISTS sec_column_policies (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id    TEXT NOT NULL,
    target_kind  TEXT NOT NULL CHECK (target_kind IN ('DATASET','NODE_TYPE')),
    target_ref   TEXT NOT NULL,
    column_name  TEXT NOT NULL,                       -- NODE_TYPE 时为属性名
    mask_rule    TEXT NOT NULL CHECK (mask_rule IN ('NONE','MASK_PHONE','MASK_ID_CARD','MASK_EMAIL','MASK_BANK_CARD',
                                                    'MASK_NAME','MASK_ADDRESS','HMAC_HASH','REDACT')),
    exempt       JSONB NOT NULL DEFAULT '{}'::jsonb,  -- {roles?, depts?, users?}：豁免主体看原文
    UNIQUE (tenant_id, target_kind, target_ref, column_name)
);

-- 跨部门授权：资源属主部门把读权限授予其他部门 / 角色 / 用户，经审批生效
CREATE TABLE IF NOT EXISTS sec_grants (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id     TEXT NOT NULL,
    resource_kind TEXT NOT NULL CHECK (resource_kind IN ('DATASET','NODE_TYPE','SUBGRAPH','DASHBOARD','AGENT')),
    resource_ref  TEXT NOT NULL,
    grantee_kind  TEXT NOT NULL CHECK (grantee_kind IN ('DEPT','ROLE','USER')),
    grantee_ref   TEXT NOT NULL,
    actions       TEXT[] NOT NULL DEFAULT ARRAY['DATA_READ'],
    row_predicate TEXT,                               -- 可选：附加行条件（同 §13.6.1 语法）
    reason        TEXT NOT NULL,
    status        TEXT NOT NULL DEFAULT 'PENDING' CHECK (status IN ('PENDING','ACTIVE','REJECTED','REVOKED','EXPIRED')),
    approval_id   UUID,
    requested_by  UUID NOT NULL,
    expires_at    TIMESTAMPTZ NOT NULL,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    decided_at    TIMESTAMPTZ
);
ALTER TABLE ax_approvals DROP CONSTRAINT IF EXISTS ax_approvals_source_kind_check;
ALTER TABLE ax_approvals ADD CONSTRAINT ax_approvals_source_kind_check
    CHECK (source_kind IN ('FLOW','SIM_APPLY','AGENT','GRANT'));

-- 大模型提供方是否允许接收 PII 原文
ALTER TABLE ax_llm_providers ADD COLUMN IF NOT EXISTS allow_pii BOOLEAN NOT NULL DEFAULT FALSE;

-- 文档段落中的 PII 位置
ALTER TABLE hg_paras ADD COLUMN IF NOT EXISTS pii_spans JSONB NOT NULL DEFAULT '[]'::jsonb;

-- 审计哈希链
ALTER TABLE sec_audit_logs ADD COLUMN IF NOT EXISTS prev_hash TEXT NOT NULL DEFAULT '';
ALTER TABLE sec_audit_logs ADD COLUMN IF NOT EXISTS row_hash  TEXT NOT NULL DEFAULT '';
CREATE TABLE IF NOT EXISTS sec_audit_chain_heads (
    tenant_id TEXT PRIMARY KEY,
    last_id   BIGINT NOT NULL,
    last_hash TEXT NOT NULL
);

SELECT ax_enable_tenant_rls(t) FROM unnest(ARRAY[
    'sec_departments','sec_user_attributes','sec_sessions','sec_policies','sec_row_policies',
    'sec_row_policy_defaults','sec_column_policies','sec_grants','sec_audit_chain_heads'
]::regclass[]) AS t;
GRANT SELECT, INSERT, DELETE ON sec_login_states TO axpoint_app;   -- 登录前无租户上下文，不启用 RLS

-- 登录前按用户名定位（不暴露口令哈希以外字段）
CREATE OR REPLACE FUNCTION ax_lookup_login(p_tenant TEXT, p_username TEXT)
RETURNS TABLE (user_id UUID, password_hash TEXT, status TEXT, locked_until TIMESTAMPTZ, failed_logins INT)
LANGUAGE sql SECURITY DEFINER SET search_path = public AS $$
    SELECT id, password_hash, status, locked_until, failed_logins
      FROM sec_users WHERE tenant_id = p_tenant AND username = p_username AND auth_source = 'LOCAL';
$$;

-- 刷新令牌定位
CREATE OR REPLACE FUNCTION ax_resolve_session(p_refresh_hash TEXT)
RETURNS TABLE (session_id UUID, tenant_id TEXT, user_id UUID, family_id UUID, expires_at TIMESTAMPTZ,
               rotated_at TIMESTAMPTZ, revoked_at TIMESTAMPTZ)
LANGUAGE sql SECURITY DEFINER SET search_path = public AS $$
    SELECT id, tenant_id, user_id, family_id, expires_at, rotated_at, revoked_at
      FROM sec_sessions WHERE refresh_hash = p_refresh_hash;
$$;

REVOKE ALL ON FUNCTION ax_lookup_login(TEXT, TEXT) FROM PUBLIC;
REVOKE ALL ON FUNCTION ax_resolve_session(TEXT) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION ax_lookup_login(TEXT, TEXT) TO axpoint_app;
GRANT EXECUTE ON FUNCTION ax_resolve_session(TEXT) TO axpoint_app;

INSERT INTO ax_retention_policies (tenant_id, target, keep_days) VALUES ('default','AUDIT',1095) ON CONFLICT DO NOTHING;
```

登录请求不带令牌，租户由请求体 `tenant`（缺省 `default`）给出；`ax_lookup_login` 是登录前唯一的跨 RLS 读取。

---

## 13.2 外部入口鉴权总表

除下表外，所有 `/api/*` 接口只接受 `ACCESS`（EdDSA 访问令牌）、`API_TOKEN`（`axp_`）与非生产环境的 `DEV_STATIC`。

| 入口 | 路径 | 凭证 | 校验 | 租户来源 | 限流 |
| :--- | :--- | :--- | :--- | :--- | :--- |
| 登录 | `POST /api/auth/login` | 用户名 + 口令 | argon2id；连续 5 次失败锁定 15 分钟 | 请求体 `tenant` | 每 IP 每分钟 10 次 |
| 刷新 | `POST /api/auth/refresh` | `axr_` 刷新令牌（HttpOnly Cookie） | `ax_resolve_session` + 轮换 + 重放检测 | 会话记录 | 每 IP 每分钟 60 次 |
| OIDC 回调 | `GET /api/auth/oidc/callback` | `code` + `state` | state 一次性、PKCE S256、ID Token 签名与 `nonce`、`aud`、`iss`、`exp` | `sec_login_states.tenant_id` | 每 IP 每分钟 30 次 |
| 已发布端点 | `/api/published/{env}/{tenant_id}/{slug}` | `axp_` 个人访问令牌 | 令牌租户必须等于路径租户；令牌 `scopes` 含端点要求权限；端点 `PROD` 快照 | 路径（与令牌核对） | 端点 `rate_limit_rpm` |
| MCP Server | `/api/mcp` | `axp_` | 同上；每个工具调用再做权限与策略校验 | 令牌 | 每令牌每分钟 120 次 |
| Webhook 接收 | `/api/axhub/webhooks/{tenant_id}/{slug}/ingest` | HMAC-SHA256 或 Bearer 密钥 | `X-AxPoint-Signature`、`X-AxPoint-Timestamp`（±300 秒）、`X-AxPoint-Delivery` 去重 | 路径 + `ax_resolve_webhook` | 每端点每分钟 600 次 |
| 流程钩子 | `/api/axflow/hooks/{tenant_id}/{slug}` | 同 Webhook | `ax_resolve_flow_hook` + 签名 | 路径 | 每钩子每分钟 120 次 |
| 分享页 | `/api/xblueview/share/{token}/*` | `axs_` | `ax_resolve_share`；口令会话；锁定筛选；创建者身份取数 | 令牌解析 | 每 IP 每分钟 120 次；解锁 10 次 |
| 智能体嵌入 | `/api/axagent/embed/{token}/chat` | `axe_` | `ax_resolve_agent_embed`；`Origin` 白名单；权限为创建者子集 | 令牌解析 | 嵌入 `rate_limit_rpm` |
| 健康检查 | `GET /api/health` | 无 | — | — | — |

共同规则：令牌只以 SHA-256 哈希入库；明文只在创建时返回一次；全部比较为常量时间；失败统一返回 401 / 404，不区分“不存在”与“已失效”；`SHARE` 与 `EMBED` 类型主体在普通业务接口一律 401。

---

## 13.3 口令与本地登录（`password.rs`）

- 哈希：`argon2` crate，Argon2id，`m=19456 KiB, t=2, p=1`，PHC 字符串存 `sec_users.password_hash`；登录成功时若参数低于当前配置则透明重新哈希。
- 口令规则：长度 ≥ 12，且不得包含用户名；`POST /api/auth/password {old_password, new_password}` 修改后 `tokens_valid_after = now()`、吊销该用户全部会话。
- 用户名不存在时仍对固定伪哈希执行一次校验，保持耗时一致。
- `scripts/ops/create-admin.sh <tenant> <username>`：以属主连接创建 `ADMIN` 用户并从标准输入读取口令（不回显），写审计 `TOKEN_ISSUE` 之外的 `POLICY_CHANGE`。

---

## 13.4 令牌（`jwt.rs`、`session.rs`）

### 13.4.1 访问令牌

- 算法 EdDSA（Ed25519），`jsonwebtoken` crate；头部 `kid` = 公钥文件名（不含扩展名）。
- 载荷：`{iss: "axpoint", aud: "axpoint-api", sub: <user_id>, tid: <tenant_id>, dept, roles, typ: "access", iat, exp, jti}`；有效期 900 秒；权限不放入令牌，由 `load_principal` 按角色解析（缓存 30 秒，`POLICY_CHANGE` 时经 `pg_notify('ax_policy_changed')` 失效）。
- 校验：签名（按 `kid` 在 `AXPOINT_JWT_PUBLIC_KEYS_DIR` 中查找，目录内容每 60 秒重新加载）、`iss`、`aud`、`exp`（允许 30 秒时钟偏差）、`typ`；再比较 `iat >= sec_users.tokens_valid_after` 且用户 `ACTIVE`（同一缓存）。
- `GET /api/auth/jwks.json`：以 JWK 格式输出全部公钥（供外部系统校验）。

### 13.4.2 刷新令牌

- 明文 `axr_<32 字节 base62>`，SHA-256 入库；有效期 14 日；只通过 `Set-Cookie: axp_refresh=...; HttpOnly; Secure; SameSite=Strict; Path=/api/auth` 下发（开发环境 `AXPOINT_ENV=development` 时省略 `Secure`）。
- 轮换：`/refresh` 时在事务中把当前会话 `rotated_at = now()`，插入同 `family_id` 的新会话，返回新访问令牌与新 Cookie。
- 重放检测：提交的刷新令牌对应会话已 `rotated_at` 或 `revoked_at` → 吊销该 `family_id` 下全部会话，返回 401 `TOKEN_INVALID`，审计 `TOKEN_REVOKE`（`detail_json.reason = "refresh_reuse"`）。
- 退出：`POST /api/auth/logout` 吊销当前 family 并清除 Cookie；`POST /api/auth/logout-all` 吊销用户全部会话并设置 `tokens_valid_after`。
- 前端：访问令牌只存内存；应用启动与令牌到期前 60 秒调用 `/refresh`；401 时尝试刷新一次再跳转登录。

### 13.4.3 密钥轮换

`scripts/ops/gen-jwt-key.sh <kid>` 生成 Ed25519 密钥对（`openssl genpkey -algorithm ed25519`）；`rotate-jwt-key.sh <new_kid>`：生成新密钥 → 把公钥放入公钥目录（全部实例 60 秒内加载）→ 等待 60 秒 → 把 `AXPOINT_JWT_PRIVATE_KEY_PATH` 指向新私钥并滚动重启 → 900 秒（访问令牌最长寿命）后删除上一把公钥。生产启动时私钥缺失则拒绝启动。

---

## 13.5 OIDC（`oidc.rs`）

- `openidconnect` crate；启动时按 `AXPOINT_OIDC_ISSUER` 拉取发现文档与 JWKS（每 3 600 秒刷新）。
- `GET /api/auth/oidc/start?tenant=&return_to=`：生成 `state`、`nonce`、PKCE verifier（S256），写 `sec_login_states`（10 分钟），302 到授权端点（`scope=openid profile email`）。`return_to` 只允许以 `/` 开头的站内路径。
- `GET /api/auth/oidc/callback?code=&state=`：删除并读取 state（不存在或过期 → 400）；换取令牌；校验 ID Token；以 `(tenant, sub)` 查找用户，不存在时按 `AXPOINT_OIDC_AUTO_PROVISION=true` 自动创建（`auth_source=OIDC`，用户名取 `AXPOINT_OIDC_USERNAME_CLAIM`，默认 `preferred_username`）；角色与部门映射来自 `AXPOINT_OIDC_ROLE_MAP`（JSON：`{"groups_claim":"groups","map":{"xbn-admin":"ADMIN"},"dept_claim":"department"}`），每次登录同步；建立会话（`auth_source=OIDC`），302 到 `return_to`；审计 `LOGIN`。

---

## 13.6 Cedar ABAC 与行级策略

### 13.6.1 Cedar（`cedar_authz.rs`）

模式 `cedar/schema.cedarschema`：

```cedar
namespace AxPoint {
  entity Dept in [Dept];
  entity User in [Dept] = {
    permissions: Set<String>,     // 角色权限并集
    clearance: Long,              // PUBLIC=0 INTERNAL=1 CONFIDENTIAL=2 SECRET=3
    grant_keys: Set<String>,      // ["user:<uuid>", "dept:<dept_id>", "role:<name>", ...]
  };
  entity Resource = {
    kind: String,
    owner_dept: Dept,
    sensitivity: Long,
    granted: Set<String>,         // 对该资源生效且包含本次动作的授权主体键
  };
  action "DATA_READ", "DATA_SYNC", "DATA_ADMIN", "STUDIO_EDIT", "STUDIO_PUBLISH", "FLOW_EDIT", "FLOW_RUN",
         "FLOW_PUBLISH", "APPROVAL_DECIDE", "RULE_EDIT", "ALERT_ACK", "AGENT_EDIT", "AGENT_RUN", "SIM_RUN",
         "SIM_APPLY", "VIEW_EDIT", "VIEW_SHARE", "EXPORT_DATA", "PII_RAW_READ", "GUARD_ADMIN", "OPS_ADMIN"
    appliesTo { principal: User, resource: Resource,
                context: { action: String, ip: String, hour: Long, env: String } };
}
```

`context.action` 为动作名字符串（Cedar 不能把动作 UID 当作字符串比较，因此由调用方同时传入）；`hour` 为租户时区的当前小时；部门层级以实体父子关系表达（上级部门包含下级部门）。

基线策略 `cedar/base.cedar`：

```cedar
// 1. 具备权限、资源属于本部门（或本部门的下级部门）、密级足够
permit (principal, action, resource)
when { principal.permissions.contains(context.action)
       && principal in resource.owner_dept
       && principal.clearance >= resource.sensitivity };

// 2. 公开与内部资源：具备权限即可跨部门访问
permit (principal, action, resource)
when { principal.permissions.contains(context.action) && resource.sensitivity <= 1 };

// 3. 机密及以上资源跨部门访问：需要包含本次动作的生效授权（经属主部门审批，视同密级满足）
permit (principal, action, resource)
when { principal.permissions.contains(context.action)
       && resource.granted.containsAny(principal.grant_keys) };

// 4. 平台管理动作不受资源归属约束
permit (principal, action in [AxPoint::Action::"GUARD_ADMIN", AxPoint::Action::"OPS_ADMIN"], resource)
when { principal.permissions.contains(context.action) };

// 5. 数据管理员对全部数据资源执行其具备的动作（与行策略对 DATA_ADMIN 的豁免一致）
permit (principal, action, resource)
when { principal.permissions.contains("DATA_ADMIN") && principal.permissions.contains(context.action) };
```

第 3 条使授权成为跨部门访问机密资源的唯一途径：授权由资源属主部门审批（§13.9.1），批准即视为对该主体放开该资源的密级要求；`SECRET` 资源的授权申请额外要求 `GUARD_ADMIN` 审批。

- 租户策略（`sec_policies`）只追加，通常为 `forbid`（例如工作时间外禁止 `EXPORT_DATA`：`forbid (principal, action == AxPoint::Action::"EXPORT_DATA", resource) when { context.hour < 8 || context.hour >= 20 };`）。保存时以模式校验（`Validator::validate`），失败返回 400 `SCHEMA_INVALID` 与错误位置；审计 `POLICY_CHANGE`。
- 每租户 `PolicySet` 缓存，`ax_policy_changed` 通知时重建。
- 调用位置：每个带资源的处理器在 `principal.require(p)` 之后调用 `authz.authorize(&principal, p, &ResourceRef{kind, id})`；拒绝时 403 `POLICY_DENIED`，审计记录命中的策略 ID。列表接口在 SQL 层预过滤：`owner_dept = ANY(可见部门) OR sensitivity <= 1 OR id = ANY(授权资源)`，再对结果逐条 `authorize`（保证与 Cedar 判定一致）。
- 测试 `tests/cedar.rs`：同部门 / 跨部门 / 各密级 / 有无授权 / 授权过期 / 租户 `forbid` 覆盖 `permit` 的组合矩阵（≥ 40 例）。

### 13.6.2 行策略表达式

语法（`row_policy.rs`，用 `sqlparser` 解析为表达式后按白名单校验）：

```ebnf
predicate  = or_expr ;
or_expr    = and_expr { "OR" and_expr } ;
and_expr   = not_expr { "AND" not_expr } ;
not_expr   = [ "NOT" ] atom ;
atom       = "(" predicate ")" | comparison | in_expr | null_test | like_expr ;
comparison = operand ( "=" | "<>" | "<" | "<=" | ">" | ">=" ) operand ;
in_expr    = operand [ "NOT" ] "IN" "(" operand { "," operand } ")" | operand [ "NOT" ] "IN" placeholder ;
null_test  = operand "IS" [ "NOT" ] "NULL" ;
like_expr  = operand [ "NOT" ] "LIKE" string ;
operand    = column | string | number | boolean | placeholder ;
placeholder = "{{" ( "user.id" | "user.username" | "user.dept" | "user.attr." ident ) "}}" ;
```

- `column` 必须是目标数据集的列（或节点类型的 `node_key`、`title` 与属性名）；不允许函数、子查询。
- 占位符在执行时以主体值替换为字面量；`user.attr.<key>` 为列表，只能出现在 `IN` 右侧；属性不存在时视为空列表（`IN ()` 编译为 `FALSE`）。
- 组合：同一目标的全部匹配策略以 `OR` 合并；授权（`sec_grants.row_predicate`）作为额外的 `OR` 分支；没有匹配策略时，`default_deny=true` 编译为 `FALSE`，否则不加条件。`DATA_ADMIN` 与 `GUARD_ADMIN` 不受行策略约束。
- 测试 `tests/data/row_policy_cases.json`：表达式 → 期望的 DuckDB SQL 与 PostgreSQL SQL；注入尝试（`'; DROP`、函数调用、子查询、未知列）全部 400。

### 13.6.3 落地到每个数据出口

| 出口 | 实现 |
| :--- | :--- |
| AxAnalytics SQL（手写、透视、指标、问数、驾驶舱、出品、智能体 SQL 工具） | `sql_guard` 在规则 2 解析表引用时，若 `PolicyContext` 对该数据集有行或列策略，把引用替换为 `(SELECT <列清单，脱敏列以 ax_mask_* 包装并保持原列名> FROM lake."tenant_x"."t" WHERE <行条件>) AS <原别名或表名>`；规则 8 的再次解析覆盖改写结果；`SELECT *` 因此也只能拿到脱敏值 |
| 平台视图 `ax.nodes`、`ax.metric_*` | 同上，以节点类型为目标，条件作用于 `type_name` 与属性（`properties->>'x'`） |
| 业务节点与领域对象接口（`/api/xblue/nodes*`、`/api/xblue/objects*`、`/api/xblue/graphql*`、图查询、指标） | 行条件编译为 PostgreSQL 条件追加到查询；属性脱敏在序列化前按列策略处理 |
| AxQuery 文档召回 | 段落 `pii_spans` 中的片段按类别脱敏（无 `PII_RAW_READ` 时） |
| 推演（Step 10） | 克隆范围时按行策略过滤节点；结果中的节点属性脱敏 |
| 分享与出品 | 以创建者主体取数，自然继承 |
| 已发布端点、MCP、智能体工具 | 以令牌主体取数，自然继承 |

性能：策略改写后 `sql_guard` 总耗时 P95 ≤ 10 毫秒（基准环境）。

---

## 13.7 列级脱敏（`masking.rs`）

| `MaskRule` | 输入 → 输出 |
| :--- | :--- |
| `MASK_PHONE` | `13812345678` → `138****5678`；带 `+86` 前缀时保留前缀 |
| `MASK_ID_CARD` | `110101199003071234` → `110101********1234` |
| `MASK_EMAIL` | `zhangsan@example.com` → `z*******@example.com` |
| `MASK_BANK_CARD` | `6222020200112233445` → `***************3445` |
| `MASK_NAME` | `张三` → `张*`；`欧阳娜娜` → `欧阳**`（复姓表内的姓保留两字）；英文名保留首字母 |
| `MASK_ADDRESS` | 保留到区县（按“省 / 市 / 区 / 县 / 旗”后缀切分），其余替换为 `****` |
| `HMAC_HASH` | `hex(HMAC-SHA256(k_tenant, value))` 前 16 位；`k_tenant = HKDF-SHA256(AXPOINT_MASK_KEYS 当前密钥, info = tenant_id)`；同值同输出，便于关联 |
| `REDACT` | `[REDACTED]` |

- 一份 Rust 实现同时用于 REST 序列化、出品、文档召回与 DuckDB：`session.rs` 使用 `duckdb::vscalar::VScalar`（开启 `duckdb` crate 的 `vscalar` 特性）注册 `ax_mask_phone(VARCHAR)` 等 8 个向量化标量函数，`HMAC_HASH` 通过 `register_scalar_function_with_state` 把 `k_tenant` 作为 `VScalar::State` 传入（密钥只驻留 Rust 内存，不出现在任何 SQL 文本或目录中）。
- 默认规则：列画像 `pii_category ≠ NONE` 且无显式列策略的列，自动套用对应规则（`PHONE → MASK_PHONE`、`ID_CARD → MASK_ID_CARD`、`EMAIL → MASK_EMAIL`、`BANK_CARD → MASK_BANK_CARD`、`NAME → MASK_NAME`、`ADDRESS → MASK_ADDRESS`、`SECRET → REDACT`）；显式 `NONE` 可解除。
- 默认所有主体都看到脱敏值。具备 `PII_RAW_READ` 或在 `exempt` 中的主体，在请求中显式传 `reveal_pii: true`（`/sql/run`、`/pivot`、节点接口与出品请求均支持该字段）时看原文；不具备资格而传该字段返回 403 `PERMISSION_DENIED`；任何返回原文 PII 的请求在审计 `detail_json.raw_pii = true`。
- 测试数据 `tests/data/mask_cases.json`：每个规则 ≥ 10 例（含空值、空串、非法格式、全角字符、超长输入），Rust 单测与 DuckDB 函数测试共用。

---

## 13.8 PII 识别（Presidio）

### 13.8.1 Sidecar 引擎（`axpoint-parser/pii/`）

- `presidio-analyzer` + `presidio-anonymizer`；NLP 引擎 spaCy `zh_core_web_sm`（镜像构建时离线安装 wheel，不在运行时下载）。
- 自定义识别器（`recognizers.py`）：`CN_PHONE`（`1[3-9]\d{9}`，前后非数字）、`CN_ID_CARD`（18 位 + GB 11643 校验码，复用 Step 01 的校验函数）、`BANK_CARD`（16～19 位 + Luhn）、`EMAIL`、`CN_ADDRESS`（省市区关键词 + 门牌号模式）、`PERSON`（spaCy 实体 + 常见姓氏表提升置信度）、`SECRET`（`(?i)(password|secret|token|api[_-]?key)\s*[:=]\s*\S+`、PEM 头）。
- `POST /pii/analyze {texts: [..], language: "zh", score_threshold: 0.6}` → 每段 `[{category: PiiCategory, start, end, score}]`；`POST /pii/anonymize {texts, rules: {PHONE: "MASK_PHONE", ...}}` → 脱敏文本（规则与 §13.7 一致，Python 实现以同一份 `mask_cases.json` 测试保证一致）。

### 13.8.2 接入点

| 场景 | 行为 |
| :--- | :--- |
| 列画像（Step 02 同步、Step 01 解析） | 以 200 行样本调用 `/pii/analyze`，某类别命中率 ≥ 30% 的列标为该类别（`pii_confirmed=false` 时覆盖） |
| 文档入库 | 段落入库时识别，写 `hg_paras.pii_spans` |
| 大模型出境 | §13.8.3 |
| 智能体会话 | 用户消息与工具结果写入 `ax_messages` 前识别；消息展示按主体权限脱敏 |

### 13.8.3 出境过滤（`axpoint-llm::router`）

调用 `allow_pii=false` 的提供方（默认全部）前，对提示中的数据片段（检索段落、查询结果、工具输出；不含系统提示）调用本地识别：命中的值替换为占位符 `⟨PHONE_1⟩` 等并在本次调用内保存映射；模型输出中出现的占位符在返回用户前按主体权限还原为原文或脱敏值。占位符映射不落盘。测试：提示含手机号时，录制的出站请求体中不含该号码。

---

## 13.9 跨部门授权与审计

### 13.9.1 授权流程

1. 申请：`POST /api/axguard/grants {resource_kind, resource_ref, grantee_kind, grantee_ref, actions, row_predicate?, reason, expires_in_days (1..=365)}`（任何具备 `DATA_READ` 的用户可为自己或本部门申请）。
2. 创建 `ax_approvals(source_kind=GRANT, assignee_roles=['ADMIN'])`，并要求审批人属于资源属主部门或具备 `GUARD_ADMIN`。
3. 批准 → `status=ACTIVE`，`ax_policy_changed` 通知，审计 `POLICY_CHANGE`；拒绝 → `REJECTED`。
4. `RETENTION` 处理器每次运行把过期授权置为 `EXPIRED`；`DELETE /api/axguard/grants/{id}` 撤销。
5. 生效授权在 Cedar 实体 `Resource.granted` 与行策略 `OR` 分支中体现。

### 13.9.2 审计

- 防篡改链（`audit_chain.rs`）：`AuditWriter` 批量写入时在事务中 `SELECT ... FROM sec_audit_chain_heads WHERE tenant_id=$1 FOR UPDATE`，逐条计算 `row_hash = hex(SHA-256(prev_hash || canonical_json(行内容不含两个哈希列)))`，写回链头。多实例写入以行锁串行化。
- `GET /api/axguard/audit?action=&username=&resource=&from=&to=&status_code=&limit=&cursor=`（`GUARD_ADMIN`）；`GET /api/axguard/audit/export.csv?...`（流式 CSV，审计本次导出为 `EXPORT_FILE`）；`GET /api/axguard/audit/verify?from_id=&to_id=` 重新计算哈希链，返回 `{ok, first_broken_id}`。
- 统计：`GET /api/axguard/audit/stats?from=&to=` 按动作、用户、日聚合，供 `AuditExplorer` 图表。

---

## 13.10 路由

| 方法与路径 | 权限 |
| :--- | :--- |
| `POST /api/auth/login`、`POST /api/auth/refresh`、`POST /api/auth/logout`、`POST /api/auth/logout-all`、`GET /api/auth/oidc/start`、`GET /api/auth/oidc/callback`、`GET /api/auth/jwks.json`、`GET /api/auth/config`（是否启用 OIDC） | 见 §13.2 / 公开 |
| `GET /api/auth/me`、`POST /api/auth/password` | 已登录 |
| `GET /api/axguard/users`、`POST /users`、`PATCH /users/{id}`（角色、部门、密级、状态）、`POST /users/{id}/reset-password`、`PUT /users/{id}/attributes` | `GUARD_ADMIN` |
| `GET /roles`、`POST /roles`、`PATCH /roles/{id}`、`DELETE /roles/{id}`（内置角色不可删） | `GUARD_ADMIN` |
| `GET /departments`、`PUT /departments/{dept_id}`、`DELETE /departments/{dept_id}` | `GUARD_ADMIN` |
| `GET /policies`、`PUT /policies/{name}`、`DELETE /policies/{name}`、`POST /policies/validate`、`POST /policies/simulate`（给定主体、动作、资源返回判定与命中策略） | `GUARD_ADMIN` |
| `GET /row-policies`、`POST /row-policies`、`PUT /row-policies/{id}`、`DELETE /row-policies/{id}`、`PUT /row-policy-defaults`、`POST /row-policies/preview`（以指定用户身份预览某数据集前 20 行） | `GUARD_ADMIN` |
| `GET /column-policies`、`PUT /column-policies`、`DELETE /column-policies/{id}` | `GUARD_ADMIN` |
| `GET /grants`、`POST /grants`、`DELETE /grants/{id}` | `DATA_READ`（申请、查看自己的）/ `GUARD_ADMIN` |
| `GET /audit`、`GET /audit/export.csv`、`GET /audit/verify`、`GET /audit/stats` | `GUARD_ADMIN` |
| `GET /sessions?user_id=`、`DELETE /sessions/{family_id}` | `GUARD_ADMIN` |

---

## 13.11 验收

`scripts/seed/13-axguard.sh`：生成 JWT 密钥（`gen-jwt-key.sh k1`）；创建部门 `east`、`south`；创建本地用户 `ops_admin`（角色 `ADMIN`，部门 `east`，`clearance='SECRET'`）、`analyst_east`（角色 `ANALYST`，部门 `east`，`clearance='CONFIDENTIAL'`，属性 `warehouses=["W-1"]`）、`analyst_south`（角色 `ANALYST`，部门 `south`，`clearance='CONFIDENTIAL'`）、`auditor`（角色 `VIEWER`，`clearance='INTERNAL'`）与各自口令（写入 `.gate/ids.env` 的 `OPS_ADMIN_PASSWORD`、`EAST_PASSWORD`、`SOUTH_PASSWORD`、`AUDITOR_PASSWORD`，仅门禁环境）；数据集 `demo_orders` 设 `owner_dept=east`、`sensitivity=CONFIDENTIAL`；行策略 `east_warehouses`：`warehouse_code IN {{user.attr.warehouses}}`，适用角色 `ANALYST`；`phone` 列使用默认 `MASK_PHONE`；查询保存查询 `orders_by_status` 的 ID 输出为 `ORDERS_BY_STATUS_QUERY_ID`。

`scripts/gate/step13.sh`：

```bash
#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "$0")/lib.sh"

(cd axpoint-core && cargo test -p axpoint-security)
(cd axpoint-parser && uv run pytest tests/test_pii.py -q)

login() { curl -s -c "/tmp/cj_$1" -H "Content-Type: application/json" -X POST "$CORE_BASE_URL/api/auth/login" \
            -d "{\"tenant\":\"default\",\"username\":\"$1\",\"password\":\"$2\"}" | jq -r .access_token; }
as() { local tok=$1; shift; curl -s -H "Authorization: Bearer $tok" -H "Content-Type: application/json" "$@"; }

EAST=$(login analyst_east "$EAST_PASSWORD")
SOUTH=$(login analyst_south "$SOUTH_PASSWORD")
expect_json "jwt-shape" "{\"t\":\"$EAST\"}" '.t | split(".") | length == 3'

# 行策略：east 只看到 W-1 的订单
Q='{"sql":"SELECT DISTINCT warehouse_code FROM demo_orders ORDER BY 1"}'
expect_json "row-policy" "$(as "$EAST" -X POST "$CORE_BASE_URL/api/analytics/sql/run" -d "$Q")" '[.rows[].warehouse_code] == ["W-1"]'
expect_json "row-policy-filter" "$(as "$EAST" -X POST "$CORE_BASE_URL/api/analytics/sql/run" -d '{"sql":"SELECT count(*) AS n FROM demo_orders WHERE warehouse_code = '"'"'W-2'"'"'"}')" '.rows[0].n == 0'

# 列脱敏
expect_json "mask-phone" "$(as "$EAST" -X POST "$CORE_BASE_URL/api/analytics/sql/run" -d '{"sql":"SELECT phone FROM demo_orders LIMIT 1"}')" '.rows[0].phone | test("^1[3-9][0-9]\\*{4}[0-9]{4}$")'
expect_json "east-no-reveal" "$(as "$EAST" -X POST "$CORE_BASE_URL/api/analytics/sql/run" -d '{"sql":"SELECT phone FROM demo_orders LIMIT 1","reveal_pii":true}')" '.error_code == "PERMISSION_DENIED"'
expect_json "admin-reveal" "$(api POST /api/analytics/sql/run '{"sql":"SELECT phone FROM demo_orders LIMIT 1","reveal_pii":true}')" '.rows[0].phone | test("^1[3-9][0-9]{9}$")'

# Cedar：south 跨部门访问机密数据集被拒绝；授权后可访问
expect_code "cedar-deny" 403 POST "/api/analytics/sql/run" "$Q" "$SOUTH"
GID=$(as "$SOUTH" -X POST "$CORE_BASE_URL/api/axguard/grants" -d "{\"resource_kind\":\"DATASET\",\"resource_ref\":\"$DEMO_ORDERS_DATASET_ID\",\"grantee_kind\":\"DEPT\",\"grantee_ref\":\"south\",\"actions\":[\"DATA_READ\"],\"row_predicate\":\"warehouse_code = 'W-2'\",\"reason\":\"门禁\",\"expires_in_days\":1}" | jq -r .id)
APR=$(api GET /api/axguard/grants | jq -r ".items[] | select(.id==\"$GID\") | .approval_id")
api POST /api/axflow/approvals/$APR/decide '{"decision":"APPROVE"}' >/dev/null
wait_until "grant-active" 30 "as '$SOUTH' -X POST '$CORE_BASE_URL/api/analytics/sql/run' -d '$Q' | jq -e '[.rows[].warehouse_code] == [\"W-2\"]'"

# 刷新轮换与重放检测
R1=$(curl -s -b /tmp/cj_analyst_east -c /tmp/cj_east2 -X POST "$CORE_BASE_URL/api/auth/refresh" -o /dev/null -w '%{http_code}')
expect_eq "refresh-ok" "$R1" "200"
R2=$(curl -s -b /tmp/cj_analyst_east -X POST "$CORE_BASE_URL/api/auth/refresh" -o /dev/null -w '%{http_code}')
expect_eq "refresh-reuse" "$R2" "401"
R3=$(curl -s -b /tmp/cj_east2 -X POST "$CORE_BASE_URL/api/auth/refresh" -o /dev/null -w '%{http_code}')
expect_eq "family-revoked" "$R3" "401"

# 锁定
for i in 1 2 3 4 5; do curl -s -o /dev/null -H "Content-Type: application/json" -X POST "$CORE_BASE_URL/api/auth/login" -d '{"tenant":"default","username":"auditor","password":"wrong-password-x"}'; done
expect_eq "lockout" "$(curl -s -o /dev/null -w '%{http_code}' -H 'Content-Type: application/json' -X POST "$CORE_BASE_URL/api/auth/login" -d "{\"tenant\":\"default\",\"username\":\"auditor\",\"password\":\"$AUDITOR_PASSWORD\"}")" "401"

# 租户 forbid 策略
api PUT /api/axguard/policies/no_export '{"policy_text":"forbid (principal, action == AxPoint::Action::\"EXPORT_DATA\", resource);"}' >/dev/null
expect_code "forbid-export" 403 POST "/api/xblueview/exports" "{\"export_format\":\"CSV\",\"source\":{\"kind\":\"SAVED_QUERY\",\"saved_query_id\":\"$ORDERS_BY_STATUS_QUERY_ID\"}}" "$EAST"
api DELETE /api/axguard/policies/no_export >/dev/null

# PII 出境过滤（录制回放下检查出站请求体）
expect_json "pii-egress" "$(api POST /api/axguard/debug/llm-egress-preview '{"text":"客户电话 13812345678"}')" '.outbound | contains("13812345678") | not'

# 审计链
expect_json "audit-chain" "$(api GET /api/axguard/audit/verify)" '.ok == true'
LAST_ID=$(sql_owner "SELECT max(id) FROM sec_audit_logs WHERE tenant_id='default'")
ORIG=$(sql_owner "SELECT username FROM sec_audit_logs WHERE id=$LAST_ID")
sql_owner "UPDATE sec_audit_logs SET username='tampered' WHERE id=$LAST_ID" >/dev/null
expect_json "audit-tamper" "$(api GET /api/axguard/audit/verify)" '.ok == false and .first_broken_id == '"$LAST_ID"
sql_owner "UPDATE sec_audit_logs SET username='$ORIG' WHERE id=$LAST_ID" >/dev/null
expect_json "audit-restored" "$(api GET /api/axguard/audit/verify)" '.ok == true'

gate_pass step13
```

- `POST /api/axguard/debug/llm-egress-preview` 只在 `AXPOINT_ENV != production` 时注册（`GUARD_ADMIN`），返回出境过滤后的文本。
- `expect_code` 第 6 个参数为覆盖令牌（A5 §2）；`EAST_PASSWORD`、`SOUTH_PASSWORD`、`AUDITOR_PASSWORD`、`ORDERS_BY_STATUS_QUERY_ID` 来自 `.gate/ids.env`。
- 审计篡改用例以属主连接修改一行后写回原值，验证校验能定位断点且恢复后链完整。

**通过标准**：最后一行 `GATE step13 PASS`；`tests/cedar.rs`、`tests/row_policy.rs`、`tests/masking.rs` 全部通过。
