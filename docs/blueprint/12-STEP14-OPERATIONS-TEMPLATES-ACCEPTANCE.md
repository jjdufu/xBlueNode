# 12 · Step 14：运维、模板、演示与总验收

本步把平台交付为可运维的生产系统：生产部署清单（单机 Compose 与 Kubernetes 两种形态）、无状态多副本与数据库高可用、备份与恢复演练、全目标保留策略（含属主角色清理审计）、深度健康检查与运维面板、行业模板包、可一键装载的演示场景、交付追踪（工作项，启用 `ACTION_EMIT` 的 `WORK_ITEM` 动作），以及覆盖 Step 00～14 的总验收脚本。

---

## 14.0 目标、前置与契约

- **目标**：见上；`scripts/gate/all.sh` 在全新环境一次通过；恢复演练脚本在独立实例上重建数据并通过对账。
- **前置**：Step 13 通过。
- **契约清单**：`0014_ops_templates_delivery.sql`；`TemplateManifest`；`WorkItem`；`/api/axops/*`（完整）；页面 `axops`（`/ops`）。

### 文件变更清单

| 动作 | 路径 | 内容 |
| :--- | :--- | :--- |
| 新建 | `deploy/migrations/0014_ops_templates_delivery.sql` | §14.1 |
| 新建 | `deploy/docker-compose.prod.yml`、`deploy/.env.prod.example`、`deploy/caddy/Caddyfile` | 单机生产形态 |
| 新建 | `deploy/k8s/base/{kustomization.yaml, namespace.yaml, configmap.yaml, secret.example.yaml, core.yaml, parser.yaml, pdb.yaml, ingress.yaml, pvc.yaml, cronjobs.yaml, postgres-cluster.yaml}`、`deploy/k8s/overlays/prod/kustomization.yaml` | Kubernetes 形态 |
| 新建 | `scripts/ops/{backup.sh, restore-drill.sh, retention-audit.sh, health-report.sh}` | 运维脚本 |
| 新建 | `axpoint-core/crates/axpoint-server/src/ops/{mod.rs, health.rs, backup.rs, templates.rs, demo.rs, work_items.rs}` | 运维服务 |
| 修改 | `axpoint-core/crates/axpoint-server/src/routes/axops.rs` | §14.8 路由 |
| 修改 | `axpoint-core/crates/axpoint-flow/src/operators/action_emit.rs` | 启用 `WORK_ITEM` 动作 |
| 修改 | `axpoint-core/crates/axpoint-server/src/background.rs` | `BACKUP` 处理器；`RETENTION` 覆盖全部目标 |
| 新建 | `templates/packs/{supply_chain, manufacturing_equipment, retail_store}/…` | 模板包（§14.5） |
| 新建 | `scripts/demo/{supply_chain.md, manufacturing_equipment.md, retail_store.md}` | 演示讲解脚本 |
| 新建 | `xbluenode-web/src/pages/AxOpsPage.tsx`、`src/components/axops/{HealthPanel.tsx, JobsPanel.tsx, RetentionPanel.tsx, BackupPanel.tsx, TemplateGallery.tsx, WorkItemBoard.tsx}` | 运维界面 |
| 新建 | `scripts/gate/step14.sh`、`xbluenode-web/e2e/full-journey.spec.ts` | 门禁与端到端旅程 |
| 新建 | `scripts/check_routes.py` | 路由实现与 [A4](./A4-API-CONTRACT-INDEX.md) 双向比对（A4 §检查方法） |
| 新建 | `scripts/check_traceability.py` | 读取 A7 表格中的断言名，逐个确认出现在对应 `scripts/gate/stepNN.sh` 中，缺失项打印并以 1 退出 |
| 修改 | `scripts/gate/all.sh` | 最终形态：全新环境重建、全部 Step 门禁、端到端旅程（§14.9.1） |

---

## 14.1 迁移 `deploy/migrations/0014_ops_templates_delivery.sql`

```sql
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

CREATE TABLE IF NOT EXISTS ax_template_installs (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id    TEXT NOT NULL,
    pack_id      TEXT NOT NULL,
    pack_version TEXT NOT NULL,
    status       TEXT NOT NULL CHECK (status IN ('PLANNED','INSTALLING','INSTALLED','FAILED','UNINSTALLED')),
    plan         JSONB NOT NULL,                      -- 安装计划：对象清单与冲突
    object_map   JSONB NOT NULL DEFAULT '{}'::jsonb,  -- 模板对象键 → 创建出的对象 ID 与内容哈希
    error_message TEXT,
    installed_by UUID,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    finished_at  TIMESTAMPTZ
);
CREATE UNIQUE INDEX IF NOT EXISTS uq_ax_template_active ON ax_template_installs (tenant_id, pack_id)
    WHERE status IN ('INSTALLING','INSTALLED');

-- 平台级备份记录（不按租户隔离；只有 default 租户中具备 OPS_ADMIN 的主体可访问）
CREATE TABLE IF NOT EXISTS ax_backups (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    backup_kind  TEXT NOT NULL CHECK (backup_kind IN ('FULL','DATABASE','LAKE')),
    status       TEXT NOT NULL CHECK (status IN ('PENDING','RUNNING','SUCCEEDED','FAILED','VERIFIED')),
    location     TEXT NOT NULL DEFAULT '',
    size_bytes   BIGINT,
    manifest     JSONB NOT NULL DEFAULT '{}'::jsonb,  -- 文件清单与 SHA-256、迁移版本、湖快照号
    error_message TEXT,
    started_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    finished_at  TIMESTAMPTZ,
    verified_at  TIMESTAMPTZ
);
GRANT SELECT, INSERT, UPDATE ON ax_backups TO axpoint_app;

-- 审计保留清理后，验证从锚点开始
ALTER TABLE sec_audit_chain_heads ADD COLUMN IF NOT EXISTS anchor_id   BIGINT;
ALTER TABLE sec_audit_chain_heads ADD COLUMN IF NOT EXISTS anchor_hash TEXT;

SELECT ax_enable_tenant_rls(t) FROM unnest(ARRAY[
    'ax_work_items','ax_work_item_events','ax_template_installs'
]::regclass[]) AS t;
```

`item_no` 在插入时于租户事务中取 `pg_advisory_xact_lock(hashtext('wi:' || tenant_id))` 后 `SELECT COALESCE(max(item_no), 0) + 1`。

---

## 14.2 生产部署

### 14.2.1 单机形态（`deploy/docker-compose.prod.yml`）

| 服务 | 镜像 | 要点 |
| :--- | :--- | :--- |
| `caddy` | `caddy:2` | 唯一对外端口 80 / 443，自动 TLS（`AXPOINT_PUBLIC_HOST`）；反向代理到 `axpoint-core:9995`；SSE 路径关闭缓冲（`flush_interval -1`）；安全响应头 `Strict-Transport-Security`、`X-Content-Type-Options: nosniff`、`Content-Security-Policy: default-src 'self'; img-src 'self' data: blob:; style-src 'self' 'unsafe-inline'; connect-src 'self'`（配置瓦片底图时追加瓦片域名） |
| `postgres` | `pgvector/pgvector:pg16` | 不映射宿主机端口；`shared_buffers=25%` 内存、`wal_level=replica`、`archive_mode=on`、`archive_command` 写入 `/backups/wal/` |
| `axpoint-parser` | 与 Step 00 相同 | `deploy.replicas: 2`，只在内部网络 |
| `axpoint-core` | 与 Step 00 相同（distroless） | `deploy.replicas: 2`；`AXPOINT_ENV=production`；`AXPOINT_MIGRATE_ON_START=false`（迁移由 `migrate` 一次性服务执行） |
| `axpoint-ops` | `pgvector/pgvector:pg16`（装有 `rsync` 与 `scripts/ops/`） | 挂载共享数据卷与 `/backups`，执行每日定时备份 / 审计保留，并轮询消费手动触发的 `ax_backups(status='PENDING')` 请求，保持 `axpoint-core` 镜像无 shell 且不持有属主数据库口令 |
| `migrate` | `axpoint-core` 镜像 | `command: ["axpoint-server", "migrate"]`，`restart: "no"`，Core 依赖其 `service_completed_successfully` |

`deploy/.env.prod.example` 列出生产必须提供的变量：`AXPOINT_PUBLIC_HOST`、`POSTGRES_PASSWORD`、`AXPOINT_APP_DB_PASSWORD`、`AXPOINT_LAKE_DB_PASSWORD`、`AXPOINT_INTERNAL_TOKEN`、`AXPOINT_CREDENTIAL_KEYS`、`AXPOINT_MASK_KEYS`、`AXPOINT_JWT_PRIVATE_KEY_PATH`、`AXPOINT_JWT_PUBLIC_KEYS_DIR`、`LLM_*`、`EMBED_*`，可选 `AXPOINT_OIDC_*`、`AXPOINT_MAP_TILE_URL`、`OTEL_EXPORTER_OTLP_ENDPOINT`。Core 在 `AXPOINT_ENV=production` 时启动自检：上述必填项缺失、口令为示例默认值、`AXPOINT_DEV_TOKEN` 被设置，任一成立即拒绝启动并输出缺失项。

### 14.2.2 Kubernetes 形态（`deploy/k8s/`）

- `core.yaml`：`Deployment` 2 副本，`readinessProbe` / `livenessProbe` 为 `GET /api/health`，`startupProbe` 允许 120 秒；资源请求 1 vCPU / 2 GiB，限制 4 vCPU / 8 GiB；`topologySpreadConstraints` 按节点分散。`parser.yaml` 同理（请求 1 vCPU / 3 GiB，含模型卷）。
- `pdb.yaml`：两者 `minAvailable: 1`。
- `pvc.yaml`：`axpoint-data`（`ReadWriteMany`，承载暂存、湖数据文件、出品文件、资产、模型）。
- `postgres-cluster.yaml`：CloudNativePG `Cluster`，3 实例（1 主 2 备，同步复制 `minSyncReplicas: 1`），`pgvector` 镜像，对象存储 WAL 归档与基础备份；应用通过 `-rw` 服务连接。
- `cronjobs.yaml`：`backup`（每日 02:00 租户时区等价 UTC 时刻执行 `scripts/ops/backup.sh`）、`retention-audit`（每日 03:00 执行 `scripts/ops/retention-audit.sh`）；手动备份由 `axpoint-ops` Deployment 消费 `ax_backups(PENDING)` 执行。
- `ingress.yaml`：TLS 终止，`nginx.ingress.kubernetes.io/proxy-buffering: "off"` 用于 SSE 路径，`proxy-body-size` 等于 `AXPOINT_MAX_UPLOAD_MB`。

### 14.2.3 高可用要点

| 组件 | 机制 |
| :--- | :--- |
| Core 多副本 | 无会话状态；任务以 `FOR UPDATE SKIP LOCKED` 租约分发；外发事件以 `ax_claim_outbox` 认领；全局单例循环用咨询锁；SSE 以 `LISTEN ax_view_events` 在每个副本广播（Step 11 §11.5）；DuckDB 实例为副本内缓存，可随时重建 |
| Parser 多副本 | 无状态；湖写入由 DuckLake 目录事务保证并发正确；同一数据集同步由 `uq_ax_jobs_sync_dataset` 保证单飞 |
| PostgreSQL | CloudNativePG 自动故障转移；单机形态以 WAL 归档 + 每日基础备份保证可恢复（RPO ≤ WAL 切换间隔，`archive_timeout=60s`） |
| 共享文件 | RWX 卷；湖数据文件只追加写入，快照由目录库引用 |

---

## 14.3 备份与恢复

### 14.3.1 `scripts/ops/backup.sh`

```bash
#!/usr/bin/env bash
# 用法：backup.sh [FULL|DATABASE|LAKE] [EXISTING_BACKUP_ID]；需要 MIGRATION_DATABASE_URL、AXPOINT_DATA_ROOT、AXPOINT_BACKUP_DIR
set -euo pipefail
KIND=${1:-FULL}
EXISTING_ID=${2:-}
TS=$(date -u +%Y%m%dT%H%M%SZ)
DEST="$AXPOINT_BACKUP_DIR/$TS"
mkdir -p "$DEST/files/lake" "$DEST/files/assets" "$AXPOINT_DATA_ROOT/lake" "$AXPOINT_DATA_ROOT/assets"
if [[ -n "$EXISTING_ID" ]]; then
  ID="$EXISTING_ID"
  psql "$MIGRATION_DATABASE_URL" -Atc "UPDATE ax_backups SET status='RUNNING', location='$DEST', started_at=now() WHERE id='$ID'" >/dev/null
else
  ID=$(psql "$MIGRATION_DATABASE_URL" -Atc "INSERT INTO ax_backups (backup_kind, status, location) VALUES ('$KIND','RUNNING','$DEST') RETURNING id")
fi
trap 'psql "$MIGRATION_DATABASE_URL" -c "UPDATE ax_backups SET status='"'"'FAILED'"'"', finished_at=now(), error_message='"'"'see logs'"'"' WHERE id='"'"'$ID'"'"'"' ERR
if [[ $KIND != LAKE ]]; then
  pg_dump "$MIGRATION_DATABASE_URL" --format=directory --jobs=4 --file="$DEST/db"
fi
if [[ $KIND != DATABASE ]]; then
  # 湖目录在数据库中（lake_catalog 模式），数据文件只追加：复制目录与数据文件即可得到一致快照
  LINK_ARGS=()
  [[ -d "$AXPOINT_BACKUP_DIR/latest/files/lake" ]] && LINK_ARGS=(--link-dest="$AXPOINT_BACKUP_DIR/latest/files/lake")
  rsync -a "${LINK_ARGS[@]}" "$AXPOINT_DATA_ROOT/lake/" "$DEST/files/lake/"
  rsync -a "$AXPOINT_DATA_ROOT/assets/" "$DEST/files/assets/"
fi
(cd "$DEST" && find . -type f -print0 | sort -z | xargs -0 sha256sum > MANIFEST.sha256)
MIG=$(psql "$MIGRATION_DATABASE_URL" -Atc "SELECT max(version) FROM _sqlx_migrations")
SIZE=$(du -sb "$DEST" | cut -f1)
FILES=$(wc -l < "$DEST/MANIFEST.sha256" | awk '{print $1}')
psql "$MIGRATION_DATABASE_URL" -v ON_ERROR_STOP=1 -c "UPDATE ax_backups SET status='SUCCEEDED', finished_at=now(), size_bytes=$SIZE,
  manifest=jsonb_build_object('migration', $MIG, 'files', $FILES) WHERE id='$ID'"
ln -sfn "$DEST" "$AXPOINT_BACKUP_DIR/latest"
find "$AXPOINT_BACKUP_DIR" -maxdepth 1 -mindepth 1 -type d -mtime +"${AXPOINT_BACKUP_KEEP_DAYS:-14}" -exec rm -rf {} +
echo "backup: $DEST"
```

数据库备份先于湖文件复制：湖目录记录的快照引用的文件在复制时一定已存在（只追加），恢复后目录中晚于数据库备份时刻的快照不存在，多出来的文件由 `LAKE_MAINTENANCE` 清理。出品文件不备份（可重新生成）。`POST /api/axops/backups` 插入 `ax_backups(status='PENDING')` 并写审计 `BACKUP_RUN`：生产容器形态由 `axpoint-ops` 认领并调用 `backup.sh <KIND> <ID>`，本地非容器开发形态如配置 `AXPOINT_BACKUP_CMD` 则由 `BACKUP` 任务直接调用。

### 14.3.2 `scripts/ops/restore-drill.sh`

```bash
#!/usr/bin/env bash
# 用法：restore-drill.sh <备份目录>；在独立的临时 PostgreSQL 容器与临时数据目录中恢复并对账
set -euo pipefail
SRC=${1:?backup dir}
(cd "$SRC" && sha256sum --quiet -c MANIFEST.sha256)
docker run -d --rm --name axp-drill-pg -e POSTGRES_USER=axpoint -e POSTGRES_PASSWORD=drill -e POSTGRES_DB=xbluenode \
  -p 127.0.0.1:55432:5432 pgvector/pgvector:pg16 >/dev/null
DRILL_ROOT=$(mktemp -d)
trap 'docker rm -f axp-drill-pg >/dev/null 2>&1 || true; rm -rf "$DRILL_ROOT"' EXIT
until docker exec axp-drill-pg pg_isready -U axpoint -d xbluenode >/dev/null 2>&1; do sleep 1; done
DRILL_URL=postgres://axpoint:drill@127.0.0.1:55432/xbluenode
psql "$DRILL_URL" -c "CREATE ROLE axpoint_app LOGIN PASSWORD 'drill'; CREATE ROLE axpoint_lake LOGIN PASSWORD 'drill';"
pg_restore --dbname="$DRILL_URL" --jobs=4 --no-owner --role=axpoint "$SRC/db"
cp -a "$SRC/files/." "$DRILL_ROOT/"
# 对账：各表行数与源库一致；湖中每个数据集最新快照可读且行数与 ds_sync_jobs 最近一次成功记录一致
python3 scripts/ops/drill_reconcile.py --source "$MIGRATION_DATABASE_URL" --target "$DRILL_URL" --lake-root "$DRILL_ROOT"
echo "restore-drill: OK"
```

`scripts/ops/drill_reconcile.py`：比较源库与恢复库的 `sec_*`、`ds_*`、`xb_*`、`ax_*`、`xv_*`、`an_*`、`hg_*` 全部表行数（备份开始之后写入源库的行不参与比较，按 `created_at <= 备份开始时间` 过滤后比较）；以恢复库目录连接 DuckLake（`ATTACH 'ducklake:postgres:...' AS lake (DATA_PATH '<DRILL_ROOT>/lake/', METADATA_SCHEMA 'lake_catalog', OVERRIDE_DATA_PATH true, READ_ONLY)`，覆盖元数据中记录的原数据根路径）逐个读取数据集最新快照计数；任一不一致退出码 1。演练成功后把对应 `ax_backups.status` 置为 `VERIFIED`。

**恢复目标**：RPO ≤ 24 小时（单机仅每日备份）或 ≤ 60 秒（启用 WAL 归档 / CloudNativePG）；RTO ≤ 30 分钟（100 GB 以内数据）。

---

## 14.4 保留策略与运维面板

### 14.4.1 保留

`RETENTION` 任务（每小时由调度单例为每个租户入队）按 `ax_retention_policies` 处理：

| 目标 | 动作 |
| :--- | :--- |
| `METRIC_SERIES` | 当前租户下超期点执行 `DELETE`；调度单例每小时额外调用一次 `SELECT ax_drop_expired_metric_partitions()`（Step 03 `SECURITY DEFINER` 函数，仅当整月分区早于所有租户最大 `keep_days` 时安全卸载） |
| `METRIC_ROLLUP_MINUTE` | 删除过期 `MINUTE` 粒度汇总 |
| `AUDIT` | 应用角色无删除权限，由 `scripts/ops/retention-audit.sh` 以属主连接执行（§14.4.2） |
| `EXPORT_FILE` | 删除过期文件，`xv_export_jobs.status = EXPIRED` |
| `FLOW_STEP_LOG` | 删除过期 `ax_flow_step_logs` |
| `JOB` / `OUTBOX` | 删除过期且已结束的任务 / 已处理事件 |
| `LAKE_SNAPSHOT` | 调用 Sidecar `/lake/maintenance`（`ducklake_expire_snapshots` + `ducklake_cleanup_old_files`） |
| `MESSAGE` / `CONVERSATION` | 删除过期 `ax_messages` / `ax_conversations` |
| `QUERY_HISTORY` | 删除过期 `an_query_history` 与 `an_ask_turns` |

同一次运行还把过期授权（`sec_grants`）置为 `EXPIRED`，并删除过期的 `sec_login_states` 与 `sec_sessions`。

### 14.4.2 `scripts/ops/retention-audit.sh`

以 `MIGRATION_DATABASE_URL` 连接，对每个租户：读取 `AUDIT` 的 `keep_days`；在一个事务中锁定 `sec_audit_chain_heads` 行，删除 `created_at < now() - keep_days` 的审计行，把剩余最小 `id` 与其 `prev_hash` 写入 `anchor_id`、`anchor_hash`；`/api/axguard/audit/verify` 从锚点开始校验。脚本自身写一条 `POLICY_CHANGE` 审计（`detail_json.retention_deleted = <行数>`）。

### 14.4.3 运维面板

- `GET /api/axops/health/deep`（`OPS_ADMIN`）：`{database: {ok, latency_ms, replication_lag_s}, parser: {ok, version}, lake: {ok, snapshot_id}, llm: {ok, provider}, embed: {ok}, disk: {data_root_free_bytes}, jobs: {pending, running, failed_24h, oldest_pending_s}, outbox: {lag_s, pending}, migrations: {current, pending}, version}`；每项独立超时 3 秒。
- `AxOpsPage`：健康面板、任务列表（筛选、重试 `POST /jobs/{id}/retry`、取消 `POST /jobs/{id}/cancel`）、外发积压、保留策略编辑、备份列表与手动备份、模板库、工作项看板。
- `scripts/ops/health-report.sh`：调用深度健康接口并以表格输出，任一项 `ok=false` 退出码 1（供外部监控）。
- Prometheus 告警规则示例 `deploy/k8s/base/prometheus-rules.yaml`：`axpoint_outbox_lag_seconds > 60`、`rate(axpoint_jobs_total{status="FAILED"}[15m]) > 0.1`、5xx 比例 > 1%。

---

## 14.5 模板包

### 14.5.1 目录结构

```text
templates/packs/supply_chain/
├── manifest.json
├── schema.json            # SchemaDoc（Step 05 格式，origin = TEMPLATE）
├── sample_data/           # CSV 样例数据（UTF-8）
│   ├── factories.csv  warehouses.csv  orders.csv  inventory_hourly.csv
├── mappings.json          # 数据集 → 节点 / 关系映射（Step 06 格式）
├── metrics.json           # 指标定义（Step 04 格式）
├── rules.json             # 告警规则（Step 08 格式，通知渠道为空，安装后由用户绑定）
├── flows/*.json           # 流程定义（Step 07 格式，安装后部署到 TEST）
├── dashboards/*.json      # 驾驶舱（Step 11 格式，数据源以对象键引用）
├── agents/*.json          # 智能体（Step 09 格式）
└── README.md              # 面向业务人员的说明
```

`manifest.json`：

```json
{
  "id": "supply_chain", "name": "供应链协同", "version": "1.0.0", "industry": "制造 / 流通",
  "description": "工厂—仓库—订单三层网络，含库存监控、断供推演与履约驾驶舱",
  "contents": {
    "schema": "schema.json",
    "datasets": [{"key": "orders", "file": "sample_data/orders.csv", "sync_mode": "SNAPSHOT"}],
    "mappings": "mappings.json", "metrics": "metrics.json", "rules": "rules.json",
    "flows": ["flows/daily_refresh.json"], "dashboards": ["dashboards/overview.json"], "agents": ["agents/supply_analyst.json"]
  },
  "sha256": { "schema.json": "…", "sample_data/orders.csv": "…" }
}
```

文件中引用其他对象一律使用 `{"$ref": "<类别>:<键>"}`（例如 `{"$ref": "dataset:orders"}`），安装时替换为实际 ID。

### 14.5.2 三个模板包

| 包 | 节点类型 | 关键内容 |
| :--- | :--- | :--- |
| `supply_chain` | Factory、Warehouse、Order、Supplier | 产能—库存—履约传导规则、`fill_rate` 派生、低库存告警、断供推演场景、履约驾驶舱 |
| `manufacturing_equipment` | Line、Machine、Sensor、WorkOrder | 设备温度 / 振动时序（MQTT 样例回放）、OEE 指标、超温告警转工作项、产线平面图驾驶舱 |
| `retail_store` | Region、Store、Sku、Promotion | 门店销售与库存、区域汇总、促销影响推演、销售日报出品模板（DOCX） |

### 14.5.3 安装流程（`ops/templates.rs`）

1. `POST /api/axops/templates/{pack_id}/plan`：校验 `sha256`；按对象类别列出将创建的对象；名称冲突（节点类型、数据集、指标、流程 slug、驾驶舱 slug 已存在）列入 `conflicts`，可选择 `SKIP` 或 `RENAME`（追加 `_<pack_id>` 后缀）。写 `ax_template_installs(status=PLANNED)`。
2. `POST /api/axops/templates/installs/{id}/apply`（`OPS_ADMIN`）：按依赖顺序以内部服务调用创建：SchemaDoc 草稿并发布 → 上传样例数据并创建数据集、等待同步 → 映射并触发物化 → 指标 → 规则 → 流程（部署 `TEST`）→ 驾驶舱（发布）→ 智能体。每创建一个对象写入 `object_map`（含内容哈希）。任一步失败：状态 `FAILED`，已创建对象保留并在响应中列出，可再次 `apply` 从断点继续（已在 `object_map` 中的对象跳过）。成功后审计 `TEMPLATE_INSTALL`。
3. `POST /api/axops/templates/installs/{id}/uninstall`：只删除内容哈希与安装时一致（未被用户修改）的对象，被修改的对象列入 `kept`。

---

## 14.6 演示场景

`POST /api/axops/demo/{scenario}/load`（`OPS_ADMIN`，`scenario ∈ supply_chain | manufacturing_equipment | retail_store`）= 安装同名模板包 + 运行一次刷新流程 + 生成演示数据（时序回放 30 日）+ 创建预置沙盘与场景。`scripts/demo/<scenario>.md` 为讲解脚本，每一步给出页面、操作与应看到的结果，例如 `supply_chain.md`：

1. `/hub`：看到 `orders` 数据集，列画像中 `phone` 标为 PII。
2. `/studio`：模型画布显示四类节点与关系，`Warehouse.fill_rate` 为派生指标。
3. `/graph`：搜索“华东仓”，展开两层邻居。
4. `/analytics`：问“各仓库上月履约率”，得到表格与柱图。
5. `/alerts`：低库存告警处于 `OPEN`，点击“转工作项”。
6. `/sim`：打开场景“F-1 产能下降 30%”，影响表首行为受影响最大的订单；对比两个场景。
7. `/view/supply-overview`：驾驶舱实时变化；导出 PPTX；创建 1 小时分享链接。
8. `/agents`：向 `supply_analyst` 提问“哪些订单有延期风险”，回答引用数据与文档。

---

## 14.7 交付追踪（工作项）

- 来源：手动创建；`ACTION_EMIT` 的 `WORK_ITEM` 动作（`{kind: WORK_ITEM, title, assignee?, priority?, node_refs?}`，`source_kind=FLOW_ACTION`，`source_ref=<run_id>`，幂等键沿用 `ax_action_log`）；告警“转工作项”（`ALERT`，`source_ref=<alert_id>`，告警解决时在工作项上追加 `LINK` 事件）；推演决策（`SIM_DECISION`）；模板安装后的待办（`TEMPLATE`，例如“绑定通知渠道”）。
- 状态流转：`OPEN → IN_PROGRESS → DONE`，任意状态可 `BLOCKED` / `CANCELED`；`DONE`、`CANCELED` 写 `closed_at`。每次变化写 `ax_work_item_events`。
- `WorkItemBoard`：看板（按状态分列，拖拽改状态）、列表（筛选负责人、优先级、来源）、详情（事件时间线、关联节点跳转图浏览器）。

---

## 14.8 路由（`/api/axops` 下）

| 方法与路径 | 权限 |
| :--- | :--- |
| `GET /jobs`、`GET /jobs/{id}` | `DATA_READ` |
| `POST /jobs/{id}/retry`、`POST /jobs/{id}/cancel`、`POST /jobs/rollup-now` | `OPS_ADMIN` |
| `GET /health/deep`、`GET /outbox/stats`、`GET /system/info` | `OPS_ADMIN` |
| `GET /retention`、`PUT /retention/{target}`、`POST /retention/run-now` | `OPS_ADMIN` |
| `GET /backups`、`POST /backups`、`GET /backups/{id}` | `OPS_ADMIN`（仅 `default` 租户） |
| `GET /templates`、`GET /templates/{pack_id}`、`POST /templates/{pack_id}/plan`、`POST /templates/installs/{id}/apply`、`POST /templates/installs/{id}/uninstall`、`GET /templates/installs` | `DATA_READ` / `OPS_ADMIN` |
| `POST /demo/{scenario}/load` | `OPS_ADMIN` |
| `GET /work-items`、`POST /work-items`、`GET /work-items/{id}`、`PATCH /work-items/{id}`、`POST /work-items/{id}/comments` | `DATA_READ` / `FLOW_RUN` |

---

## 14.9 验收

`scripts/gate/step14.sh`：

```bash
#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "$0")/lib.sh"

(cd axpoint-core && cargo test -p axpoint-server ops::)

# 深度健康
expect_json "health-deep" "$(api GET /api/axops/health/deep)" '.database.ok and .parser.ok and .lake.ok and .migrations.pending == 0'

# 模板安装：计划 → 应用 → 对象存在 → 卸载
PLAN=$(api POST /api/axops/templates/retail_store/plan '{"on_conflict":"RENAME"}')
IID=$(echo "$PLAN" | jq -r .id)
api POST /api/axops/templates/installs/$IID/apply '{}' >/dev/null
wait_until "template-installed" 300 "api GET /api/axops/templates/installs | jq -e '.items[] | select(.id==\"$IID\") | .status == \"INSTALLED\"'"
expect_json "template-dashboard" "$(api GET /api/xblueview/dashboards/by-slug/retail-overview)" '.status == "PUBLISHED"'
expect_eq "audit-template" "$(sql_app default "SELECT count(*) > 0 FROM sec_audit_logs WHERE action='TEMPLATE_INSTALL'")" "t"
api POST /api/axops/templates/installs/$IID/uninstall '{}' >/dev/null
expect_code "template-uninstalled" 404 GET "/api/xblueview/dashboards/by-slug/retail-overview"

# 工作项：流程动作生成
FLOW=$(api POST /api/axflow/flows '{"name":"wi_gate","slug":"wi-gate","nodes":[{"id":"a","kind":"ACTION_EMIT","label":"建工作项","pos":{"x":0,"y":0},"config":{"actions":[{"kind":"WORK_ITEM","title":"门禁工作项","priority":"P1"}]}}],"edges":[],"trigger_cfg":{"type":"MANUAL"}}' | jq -r .id)
VER=$(api POST /api/axflow/flows/$FLOW/versions '{}' | jq -r .version)
api POST /api/axflow/flows/$FLOW/deploy "{\"environment\":\"PROD\",\"version\":$VER}" >/dev/null
RUN=$(api POST /api/axflow/flows/$FLOW/runs '{"environment":"PROD"}' | jq -r .run_id)
wait_run_status "$RUN" SUCCEEDED
expect_json "work-item" "$(api GET '/api/axops/work-items?source_kind=FLOW_ACTION')" '[.items[] | select(.title=="门禁工作项")] | length == 1'

# 保留：过期任务被清理
sql_owner "INSERT INTO ax_jobs (tenant_id, kind, status, created_at, updated_at) VALUES ('default','RETENTION','SUCCEEDED', now() - interval '400 days', now() - interval '400 days')" >/dev/null
wait_job "$(api POST /api/axops/retention/run-now | jq -r .job_id)"
expect_eq "retention-jobs" "$(sql_owner "SELECT count(*) FROM ax_jobs WHERE created_at < now() - interval '399 days'")" "0"

# 审计保留脚本与锚点
AXPOINT_AUDIT_FORCE_KEEP_DAYS=1 bash scripts/ops/retention-audit.sh >/dev/null
expect_json "audit-anchor" "$(api GET /api/axguard/audit/verify)" '.ok == true'

# 备份与恢复演练
export AXPOINT_BACKUP_DIR=$PWD/.gate/backups
bash scripts/ops/backup.sh FULL
bash scripts/ops/restore-drill.sh "$AXPOINT_BACKUP_DIR/latest"

gate_pass step14
```

- `POST /api/axops/retention/run-now`（`OPS_ADMIN`）为当前租户立即入队 `RETENTION` 并返回 `job_id`。
- `AXPOINT_AUDIT_FORCE_KEEP_DAYS` 只被 `retention-audit.sh` 读取，用于门禁覆盖 `keep_days`。
- Step 13 门禁在篡改用例后把被篡改的字段写回原值，哈希链恢复完整，因此 `all.sh` 连续执行 Step 13 与 Step 14 时审计校验结果一致。

### 14.9.1 总验收 `scripts/gate/all.sh`

```bash
#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
bash scripts/dev-down.sh || true
bash scripts/dev-up.sh
docker compose -f deploy/docker-compose.test.yml --env-file .env.test up -d --wait
set -a; source .env.test; set +a
bash scripts/start-services.sh            # 后台启动 Sidecar 与 Core，等待 /api/health 与 /health
bash scripts/seed-demo.sh
bash scripts/naming-guard.sh
for n in $(seq -w 0 14); do
  bash "scripts/gate/step${n}.sh" | tee ".gate/step${n}.log"
  tail -n1 ".gate/step${n}.log" | grep -qx "GATE step${n} PASS"
done
(cd xbluenode-web && npx playwright test e2e/full-journey.spec.ts)
echo "ALL GATES PASS"
```

`scripts/start-services.sh` 与 `scripts/dev-down.sh` 见 [01](./01-STEP00-FOUNDATION-AND-GUARD-SKELETON.md) §00.10。

`e2e/full-journey.spec.ts`（通过 `/login` 以本地账号 `ops_admin` 登录）：上传 `supply_chain.xlsx` → 数据集出现 → 在 xBlueStudio 发布模型 → 图浏览器看到节点 → 分析页执行查询 → 推演页运行场景 → 驾驶舱组件渲染 → 导出 XLSX 下载成功 → 退出登录后访问 `/view` 跳转 `/login`。

**通过标准**：`all.sh` 输出最后一行 `ALL GATES PASS`；`restore-drill.sh` 输出 `restore-drill: OK`；基准环境中各 Step 的性能断言成立。
