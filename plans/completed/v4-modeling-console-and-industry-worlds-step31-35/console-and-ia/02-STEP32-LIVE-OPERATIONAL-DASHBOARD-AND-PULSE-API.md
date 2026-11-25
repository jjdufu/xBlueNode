# 02 · Step 32：全域运行脉冲接口（`/api/axops/pulse`）与真数据总控看板改版

本步彻底重塑平台首屏 `/`（`总控看板 / Dashboard`）：淘汰早期遗留的检索算法静态说明卡，在后端构建单次往返、毫秒级响应、严格遵循租户 RLS 隔离的全域态势聚合接口 `GET /api/axops/pulse`，并在前端打造集 **核心态势火花线指标卡、多行业业务模型工作台矩阵、24h 时序吞吐与风险分布 ECharts 图表、实时告警/待批动作与沙盘红线预警** 于一体的工业级总控台。

总览索引与工程约束见 [00-V4-MASTER-INDEX.md](../00-V4-MASTER-INDEX.md)、[IRON_RULES.md](../../../../IRON_RULES.md)、[API_CONTRACT.md](../../../../docs/API_CONTRACT.md) 与 [A4-API-CONTRACT-INDEX.md](../../../../docs/standards/A4-API-CONTRACT-INDEX.md)。

---

## 32.0 目标、前置与契约

- **目标**：
  1. **一站式全域脉冲接口 `GET /api/axops/pulse`**：单次请求聚合当前租户跨子图的对象模型规模、节点与关系总量、DuckLake 数据集总行数、近 24 小时分时指标写入吞吐、常驻流活跃数、各子图模型就绪度（`L0`～`L4`）与异常节点统计、待处置告警、待审批动作及近期 `AxSim` 沙盘 `deadline_step` 红线预警。
  2. **真数据驱动的 `DashboardPage.tsx` 总控大盘**：100% 绑定后端真实接口数据，零硬编码假数组；支持一键切换/直达任意行业子图的建模画布（`/studio?subgraph=...`）、推演沙盘（`/sim?sandbox=...`）或五区数字看板（`/view/...`），并在顶栏提供 `装载样例` 快捷入口（对接 Step 35 多行业数据包）。
  3. **智能折叠冷启动向导**：当租户下业务节点数 `> 0` 且湖表数据集数 `> 0` 时，自动将新手 5 步构建引导条收敛为右上角紧凑徽标，把首屏 100% 留给高价值实时业务态势。
- **前置**：L19 契约 PR 已合入（**本步无数据库迁移，直接聚合已有 `xb_*`、`ds_*`、`ax_*`、`xv_*` 与时序分区表**）。
- **覆盖能力项**：
  - `C94`：`step32` `"pulse-rls-isolated"`、`"dashboard-live-pulse"`、`"dashboard-subgraph-matrix"`

### 文件变更清单

| 负责角色 | 动作 | 路径 | 内容 |
| :--- | :--- | :--- | :--- |
| `orchestrator` | 修改 | `axpoint-core/crates/axpoint-domain/src/{ops.rs,ports.rs}`、`docs/API_CONTRACT.md`、`docs/standards/A4-API-CONTRACT-INDEX.md` | 定义 `PlatformPulseResponse` 强类型 DTO 与 `OpsRepository::fetch_platform_pulse` 端口签名，登记 `GET /api/axops/pulse` |
| `squad-domain-agent` | 修改 / 新建 | `axpoint-core/crates/axpoint-db/src/ops_repo/{mod.rs,pulse.rs}`、`axpoint-core/crates/axpoint-server/src/routes/axops.rs`、`axpoint-core/crates/axpoint-server/tests/step32_pulse_api.rs` | 在租户 RLS 事务内实现单轮高效聚合 SQL，挂载 `GET /api/axops/pulse`（要求 `DATA_READ` 权限），覆盖跨租户隔离集成测试 |
| `squad-web` | 修改 / 新建 | `xbluenode-web/src/lib/api/axops.ts`、`src/pages/DashboardPage.tsx`、`src/components/dashboard/{SubgraphMatrixGrid.tsx,PulseThroughputChart.tsx,ActionAndAlertFeed.tsx}` | 使用 `zod` 校验脉冲响应；重写总控看板四大视觉区块，接入 ECharts 6 语义主题与内联 SVG 火花线 |
| `squad-qa` | 新建 | `xbluenode-web/e2e/step32-live-dashboard.spec.ts`、`scripts/gate/step32.sh` | 校验 RLS 隔离、6 张 KPI 卡片数值与后端精确一致、ECharts Canvas 渲染、子图卡片深链跳转与无静态科普卡残留 |

---

## 32.1 后端全域脉冲契约（`GET /api/axops/pulse`）

### 32.1.1 领域 DTO（`axpoint-domain/src/ops.rs`）

```rust
/// 单租户全域建模、数据、运行与决策态势快照（`GET /api/axops/pulse`）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlatformPulseResponse {
    /// 快照生成 UTC 时间戳。
    pub generated_at: DateTime<Utc>,
    /// 顶层核心规模与吞吐汇总。
    pub totals: PulseTotals,
    /// 过去 24 小时按整点聚合的时序点写入与数据同步行数序列（24 个桶，驱动火花线与双轴趋势图）。
    pub hourly_series: Vec<PulseHourlyBucket>,
    /// 当前租户各业务子图模型健康矩阵（按节点规模降序）。
    pub subgraphs: Vec<SubgraphPulseCard>,
    /// 最新未关闭（`OPEN` / `ACK`）告警列表（上限 10 条）。
    pub active_alerts: Vec<PulseAlertItem>,
    /// 待人工决策（`PENDING`）的流程与 `ActionSpec` 动作审批列表（上限 10 条）。
    pub pending_approvals: Vec<PulseApprovalItem>,
    /// 近期运行的推演场景及 `deadline_step` 红线预警摘要（上限 8 条）。
    pub recent_simulations: Vec<PulseSimulationItem>,
}

/// 顶层 6 维核心指标汇总。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PulseTotals {
    pub subgraph_count: i64,
    pub node_type_count: i64,
    pub node_count: i64,
    pub link_count: i64,
    pub dataset_count: i64,
    pub lake_total_rows: i64,
    pub metric_points_24h: i64,
    pub active_stream_count: i64,
    pub open_alert_count: i64,
    pub pending_approval_count: i64,
}

/// 分时吞吐统计桶。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PulseHourlyBucket {
    pub hour_utc: DateTime<Utc>,
    pub metric_points: i64,
    pub synced_rows: i64,
}

/// 单个业务子图工作台摘要卡片。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubgraphPulseCard {
    pub slug: String,
    pub display_name: String,
    pub description: Option<String>,
    pub is_dev_sandbox: bool,
    pub schema_version: i32,
    pub node_type_count: i64,
    pub edge_type_count: i64,
    pub node_count: i64,
    pub link_count: i64,
    pub cert_level: Option<RuntimeCertLevel>,
    pub readiness_score: Option<i32>,
    pub business_domains: Vec<String>,
    pub primary_dashboard_slug: Option<String>,
    pub latest_sandbox_id: Option<Uuid>,
}
```

### 32.1.2 查询性能与安全边界
1. **严格租户隔离**：所有 SQL 在 `begin_tenant_tx(pool, &principal.tenant_id)` 事务内执行，绝不跨租户统计；默认排除 `is_dev_sandbox = true` 的临时构建沙盒节点混入生产 `totals.node_count`（`SubgraphPulseCard` 单独标注 `is_dev_sandbox`）。
2. **索引友好聚合**：`lake_total_rows` 直接汇总 `ds_datasets.row_count`；`metric_points_24h` 与 `hourly_series` 扫描 `xb_metric_series` 的 `(tenant_id, ts)` 分区裁剪索引（`ts >= NOW() - INTERVAL '24 hours'`），单次请求 P95 延迟严格 `< 80ms`。

---

## 32.2 前端 `DashboardPage.tsx` 四区工业级布局

```text
┌──────────────────────────────────────────────────────────────────────────────────────────┐
│ 总控看板                                              [在线] [装载样例] [模板库] [刷新]   │
├─────────────┬─────────────┬─────────────┬─────────────┬─────────────┬────────────────────┤
│ 业务节点总数│ 拓扑关系边数│ 湖表总行数  │ 24h 时序吞吐│ 认证业务子图│ 待处置告警 / 审批  │
│  2,415 +火花│  6,680 +火花│ 148,200 行  │  86,400 点  │  5 个 (L4)  │   4 告警 · 2 待批  │
├───────────────────────────────────────────────────┬──────────────────────────────────────┤
│ 多行业业务模型矩阵（SubgraphMatrixGrid）          │ 24h 时序指标与数据入湖趋势（ECharts）│
│ ┌─────────────────────┐ ┌─────────────────────┐   │  [双轴平滑面积柱状组合图]            │
│ │ 新型电力与配网调度  │ │ 集团授信与互保风控  │   │                                      │
│ │ L4 · 324 节点 548 边│ │ L4 · 1210 节点 4260边│  │ 各子图规模与告警分布（水平条形图）   │
│ │ [建模] [推演] [看板]│ │ [建模] [推演] [看板]│   │                                      │
│ └─────────────────────┘ └─────────────────────┘   │                                      │
│ ┌─────────────────────┐ ┌─────────────────────┐   │                                      │
│ │ 半导体全球多级供应链│ │ 城市急诊与床位调度  │   │                                      │
│ └─────────────────────┘ └─────────────────────┘   │                                      │
├───────────────────────────────────────────────────┼──────────────────────────────────────┤
│ 实时告警与待批动作（ActionAndAlertFeed）          │ 近期推演沙盘与红线预警               │
│ • [危急] 220kV 城北变 主变负载率越限 (94.2%)      │ • 主变跳闸晚高峰推演 · 最晚第 3 步干预│
│ • [待批] 晶圆安全库存跨仓调拨 (dispatch_reserve)  │ • 互保违约传染推演   · 影响 18 家法人 │
└───────────────────────────────────────────────────┴──────────────────────────────────────┘
```

- **微文案红线**：所有卡片动作按钮遵守 2～4 字极简动词（`建模`、`推演`、`看板`、`处置`、`审批`、`装载样例`、`模板库`、`刷新`），无任何冗长引导横幅。

---

## 32.3 可执行门禁（`scripts/gate/step32.sh`）

```bash
#!/usr/bin/env bash
# Step 32 Gate：/api/axops/pulse 租户隔离、真数据聚合准确性与总控看板前端渲染验收
set -euo pipefail
source "$(dirname "$0")/lib.sh"

python3 "$ROOT/scripts/lint-guard.py"
python3 "$ROOT/scripts/web-guard.py"
python3 "$ROOT/scripts/check_routes.py" \
  --routes "$ROOT/axpoint-core/crates/axpoint-server/src/routes" \
  --index "$ROOT/docs/standards/A4-API-CONTRACT-INDEX.md"

(cd "$ROOT/axpoint-core" && cargo test -p axpoint-server --test step32_pulse_api --locked)

PULSE_DEFAULT=$(api GET /api/axops/pulse)
PULSE_OTHER=$(api_tenant other GET /api/axops/pulse)
expect_json "pulse-rls-isolated" "$(jq -n --argjson d "$PULSE_DEFAULT" --argjson o "$PULSE_OTHER" '{d:$d, o:$o}')" '
  (.d.totals.node_count > 0)
  and (.o.totals.node_count == 0)
  and ((.o.subgraphs | length) <= 1)
'

PW_PORT="${PLAYWRIGHT_PORT:-$(( 4173 + ${WT_SLOT:-0} * 100 ))}"
(cd "$ROOT/xbluenode-web" && npm run build && PLAYWRIGHT_PORT="$PW_PORT" npx playwright test e2e/step32-live-dashboard.spec.ts)
echo "ASSERT dashboard-live-pulse PASS"
echo "ASSERT dashboard-subgraph-matrix PASS"

gate_pass step32
```
