import { Fragment, useCallback, useEffect, useMemo, useState } from "react";
import {
  AlertTriangle,
  CheckCircle2,
  ChevronDown,
  ChevronRight,
  Download,
  Link2,
  RefreshCw,
  Search,
} from "lucide-react";
import { ApiError } from "@/lib/api";
import {
  axguardApi,
  type AuditLogEntry,
  type AuditStatsResponse,
  type AuditVerifyResponse,
} from "@/lib/api/axguard";
import { Badge, Button, Card, Input } from "@/components/ui/primitives";
import { cn } from "@/lib/utils";
import { AUDIT_ACTIONS } from "./constants";

export function AuditExplorer() {
  const [actionFilter, setActionFilter] = useState("");
  const [usernameFilter, setUsernameFilter] = useState("");
  const [resourceFilter, setResourceFilter] = useState("");
  const [statusCodeFilter, setStatusCodeFilter] = useState("");
  const [fromTime, setFromTime] = useState("");
  const [toTime, setToTime] = useState("");

  const [logs, setLogs] = useState<AuditLogEntry[]>([]);
  const [nextCursor, setNextCursor] = useState<string | null>(null);
  const [loadingLogs, setLoadingLogs] = useState(false);
  const [expandedId, setExpandedId] = useState<number | null>(null);

  const [stats, setStats] = useState<AuditStatsResponse>({
    by_action: {},
    by_user: [],
    by_day: [],
  });

  const [verifyFromId, setVerifyFromId] = useState("");
  const [verifyToId, setVerifyToId] = useState("");
  const [verifying, setVerifying] = useState(false);
  const [verifyResult, setVerifyResult] =
    useState<AuditVerifyResponse | null>(null);

  const [exporting, setExporting] = useState(false);
  const [banner, setBanner] = useState<string | null>(null);

  const buildFilterParams = useCallback(() => {
    const parsedCode = statusCodeFilter.trim()
      ? Number(statusCodeFilter.trim())
      : undefined;
    return {
      action: actionFilter || undefined,
      username: usernameFilter.trim() || undefined,
      resource: resourceFilter.trim() || undefined,
      status_code:
        parsedCode !== undefined && Number.isFinite(parsedCode)
          ? parsedCode
          : undefined,
      from: fromTime ? new Date(fromTime).toISOString() : undefined,
      to: toTime ? new Date(toTime).toISOString() : undefined,
    };
  }, [
    actionFilter,
    usernameFilter,
    resourceFilter,
    statusCodeFilter,
    fromTime,
    toTime,
  ]);

  const loadAuditData = useCallback(
    async (cursor?: string) => {
      setLoadingLogs(true);
      try {
        const filters = buildFilterParams();
        const page = await axguardApi.queryAuditLogs({
          ...filters,
          limit: 50,
          cursor,
        });
        setLogs((prev) => (cursor ? [...prev, ...page.items] : page.items));
        setNextCursor(page.next_cursor ?? null);

        if (!cursor) {
          const st = await axguardApi.queryAuditStats({
            from: filters.from,
            to: filters.to,
          });
          setStats(st);
        }
      } catch (err) {
        setBanner(
          err instanceof ApiError
            ? `[${err.errorCode}] ${err.message}`
            : String(err)
        );
      } finally {
        setLoadingLogs(false);
      }
    },
    [buildFilterParams]
  );

  useEffect(() => {
    void loadAuditData();
  }, [loadAuditData]);

  const handleVerifyChain = async () => {
    setVerifying(true);
    setBanner(null);
    try {
      const fromNum = verifyFromId.trim() ? Number(verifyFromId) : undefined;
      const toNum = verifyToId.trim() ? Number(verifyToId) : undefined;
      const res = await axguardApi.verifyAuditChain({
        from_id:
          fromNum !== undefined && Number.isFinite(fromNum)
            ? fromNum
            : undefined,
        to_id:
          toNum !== undefined && Number.isFinite(toNum) ? toNum : undefined,
      });
      setVerifyResult(res);
    } catch (err) {
      setBanner(
        err instanceof ApiError ? `[${err.errorCode}] ${err.message}` : String(err)
      );
    } finally {
      setVerifying(false);
    }
  };

  const handleExportCsv = async () => {
    setExporting(true);
    setBanner(null);
    try {
      const blob = await axguardApi.exportAuditCsv(buildFilterParams());
      const objUrl = URL.createObjectURL(blob);
      const anchor = document.createElement("a");
      anchor.href = objUrl;
      anchor.download = `axguard-audit-${new Date().toISOString().slice(0, 10)}.csv`;
      document.body.appendChild(anchor);
      anchor.click();
      anchor.remove();
      URL.revokeObjectURL(objUrl);
      setBanner("已导出审计日志 CSV 文件");
    } catch (err) {
      setBanner(
        err instanceof ApiError ? `[${err.errorCode}] ${err.message}` : String(err)
      );
    } finally {
      setExporting(false);
    }
  };

  const actionSeries = useMemo(
    () =>
      Object.entries(stats.by_action)
        .sort((a, b) => b[1] - a[1])
        .slice(0, 6),
    [stats.by_action]
  );

  const totalActionCount = useMemo(
    () => Object.values(stats.by_action).reduce((acc, n) => acc + n, 0),
    [stats.by_action]
  );

  const maxDayVal = useMemo(
    () => Math.max(1, ...stats.by_day.map(([, v]) => v)),
    [stats.by_day]
  );

  const maxActionVal = useMemo(
    () => Math.max(1, ...actionSeries.map(([, v]) => v)),
    [actionSeries]
  );

  return (
    <div className="space-y-4" data-testid="axguard-audit-panel">
      {banner && (
        <div
          role="status"
          className="rounded-md border border-border bg-muted/50 px-3 py-2 text-xs"
        >
          {banner}
        </div>
      )}

      {/* Hash Chain Tamper-Evident Verification Banner */}
      <Card
        className={cn(
          "p-4 space-y-3",
          verifyResult && !verifyResult.ok && "border-foreground bg-muted/60"
        )}
        data-testid="audit-verify-banner"
      >
        <div className="flex flex-wrap items-center justify-between gap-3">
          <div className="space-y-1">
            <div className="text-xs font-semibold flex items-center gap-1.5">
              <Link2 className="h-3.5 w-3.5 text-xblue" />
              SHA-256 防篡改审计哈希链完整性验真（sec_audit_logs）
            </div>
            <p className="text-[11px] text-muted-foreground">
              逐行重算 canonical_json 载荷与 prev_hash 链式摘要，精准定位任何越权篡改或断链记录。
            </p>
          </div>

          <div className="flex flex-wrap items-center gap-2">
            <Input
              type="number"
              placeholder="起始 ID (可选)"
              value={verifyFromId}
              onChange={(e) => setVerifyFromId(e.target.value)}
              className="h-8 w-28 text-xs font-mono-tech"
            />
            <Input
              type="number"
              placeholder="结束 ID (可选)"
              value={verifyToId}
              onChange={(e) => setVerifyToId(e.target.value)}
              className="h-8 w-28 text-xs font-mono-tech"
            />
            <Button
              type="button"
              disabled={verifying}
              onClick={() => void handleVerifyChain()}
              data-testid="audit-verify-btn"
            >
              {verifying ? "正在逐行验真..." : "哈希链完整性验真"}
            </Button>
          </div>
        </div>

        {verifyResult &&
          (verifyResult.ok ? (
            <div
              data-testid="audit-verify-ok-badge"
              className="flex items-center gap-2 rounded-md border border-xblue/40 bg-xblue-soft/60 px-3 py-2 text-xs"
            >
              <CheckCircle2 className="h-4 w-4 text-xblue shrink-0" />
              <span className="font-semibold">哈希链完整无篡改</span>
              <span className="font-mono-tech text-muted-foreground">
                · 已连续校验 {verifyResult.checked_count} 条审计记录
              </span>
            </div>
          ) : (
            <div
              role="alert"
              data-testid="audit-tamper-alert"
              className="flex flex-wrap items-center justify-between gap-2 rounded-md border border-foreground bg-background px-3 py-2.5 text-xs"
            >
              <div className="flex items-center gap-2">
                <AlertTriangle className="h-4 w-4 text-xblue shrink-0" />
                <span className="font-semibold">
                  篡改告警：检测到审计哈希链断裂！
                </span>
                <span className="font-mono-tech">
                  首个异常记录 ID: #{verifyResult.first_broken_id ?? "未知"}（已扫描{" "}
                  {verifyResult.checked_count} 条）
                </span>
              </div>
              <Badge variant="neutral">CHAIN BROKEN</Badge>
            </div>
          ))}
      </Card>

      {/* Top KPI & Visual Trend / Action Distribution */}
      <div className="grid grid-cols-1 lg:grid-cols-12 gap-4">
        <Card className="lg:col-span-4 p-4 space-y-3">
          <div className="text-xs font-semibold">审计概览指标</div>
          <div className="grid grid-cols-2 gap-2.5">
            <div className="rounded border border-border bg-muted/20 p-2.5">
              <div className="text-[11px] text-muted-foreground">统计总事件数</div>
              <div className="text-lg font-mono-tech font-semibold mt-0.5">
                {totalActionCount || logs.length}
              </div>
            </div>
            <div className="rounded border border-border bg-muted/20 p-2.5">
              <div className="text-[11px] text-muted-foreground">活跃主体数</div>
              <div className="text-lg font-mono-tech font-semibold mt-0.5">
                {stats.by_user.length}
              </div>
            </div>
          </div>

          <div className="space-y-1.5 pt-1">
            <div className="text-[11px] text-muted-foreground">活跃用户 Top 排行</div>
            {stats.by_user.length === 0 ? (
              <div className="text-[11px] text-muted-foreground">暂无聚合样本</div>
            ) : (
              stats.by_user.slice(0, 4).map(([uname, cnt]) => (
                <div
                  key={uname}
                  className="flex items-center justify-between text-xs font-mono-tech"
                >
                  <span className="truncate">{uname}</span>
                  <Badge variant="outline">{cnt}</Badge>
                </div>
              ))
            )}
          </div>
        </Card>

        <Card className="lg:col-span-4 p-4 space-y-2.5">
          <div className="text-xs font-semibold">按动作分布 (by_action)</div>
          {actionSeries.length === 0 ? (
            <div className="py-6 text-center text-[11px] text-muted-foreground">
              暂无动作分布数据
            </div>
          ) : (
            <div className="space-y-2">
              {actionSeries.map(([act, count]) => {
                const widthPct = Math.max(
                  6,
                  Math.round((count / maxActionVal) * 100)
                );
                return (
                  <div key={act} className="space-y-0.5">
                    <div className="flex justify-between text-[11px] font-mono-tech">
                      <span>{act}</span>
                      <span>{count}</span>
                    </div>
                    <div className="h-1.5 w-full rounded-full bg-muted overflow-hidden">
                      <div
                        className="h-full bg-xblue rounded-full"
                        style={{ width: `${widthPct}%` }}
                      />
                    </div>
                  </div>
                );
              })}
            </div>
          )}
        </Card>

        <Card className="lg:col-span-4 p-4 space-y-2.5">
          <div className="text-xs font-semibold">按日审计趋势 (by_day)</div>
          {stats.by_day.length === 0 ? (
            <div className="py-6 text-center text-[11px] text-muted-foreground">
              暂无日度趋势数据
            </div>
          ) : (
            <svg
              viewBox="0 0 260 110"
              className="w-full h-28 overflow-visible"
              role="img"
              aria-label="每日审计事件趋势图"
            >
              {stats.by_day.slice(-10).map(([day, count], idx, arr) => {
                const slotW = 240 / Math.max(1, arr.length);
                const barW = Math.max(8, Math.min(22, slotW - 6));
                const barH = Math.max(4, Math.round((count / maxDayVal) * 76));
                const x = 10 + idx * slotW + (slotW - barW) / 2;
                const y = 86 - barH;
                return (
                  <g key={day}>
                    <rect
                      x={x}
                      y={y}
                      width={barW}
                      height={barH}
                      rx={2}
                      fill="var(--xblue)"
                    />
                    <text
                      x={x + barW / 2}
                      y={102}
                      textAnchor="middle"
                      fontSize="8"
                      fill="var(--muted-foreground)"
                    >
                      {day.slice(5)}
                    </text>
                  </g>
                );
              })}
            </svg>
          )}
        </Card>
      </div>

      {/* Multi-Dimensional Filter Bar + Export CSV */}
      <Card className="p-3.5 flex flex-wrap items-end gap-2.5">
        <div className="space-y-1">
          <label className="text-[11px] text-muted-foreground">审计动作</label>
          <select
            value={actionFilter}
            onChange={(e) => setActionFilter(e.target.value)}
            className="h-8 rounded-md border border-input bg-background px-2 text-xs font-mono-tech"
          >
            <option value="">全部动作</option>
            {AUDIT_ACTIONS.map((act) => (
              <option key={act} value={act}>
                {act}
              </option>
            ))}
          </select>
        </div>

        <div className="space-y-1">
          <label className="text-[11px] text-muted-foreground">操作账号</label>
          <Input
            placeholder="username"
            value={usernameFilter}
            onChange={(e) => setUsernameFilter(e.target.value)}
            className="h-8 w-32 text-xs font-mono-tech"
          />
        </div>

        <div className="space-y-1">
          <label className="text-[11px] text-muted-foreground">资源标识</label>
          <Input
            placeholder="如 dataset:..."
            value={resourceFilter}
            onChange={(e) => setResourceFilter(e.target.value)}
            className="h-8 w-40 text-xs font-mono-tech"
          />
        </div>

        <div className="space-y-1">
          <label className="text-[11px] text-muted-foreground">HTTP 状态码</label>
          <Input
            type="number"
            placeholder="200 / 403"
            value={statusCodeFilter}
            onChange={(e) => setStatusCodeFilter(e.target.value)}
            className="h-8 w-24 text-xs font-mono-tech"
          />
        </div>

        <div className="space-y-1">
          <label className="text-[11px] text-muted-foreground">起始时间</label>
          <Input
            type="datetime-local"
            value={fromTime}
            onChange={(e) => setFromTime(e.target.value)}
            className="h-8 w-44 text-xs font-mono-tech"
          />
        </div>

        <div className="space-y-1">
          <label className="text-[11px] text-muted-foreground">截止时间</label>
          <Input
            type="datetime-local"
            value={toTime}
            onChange={(e) => setToTime(e.target.value)}
            className="h-8 w-44 text-xs font-mono-tech"
          />
        </div>

        <div className="flex items-center gap-2 ml-auto">
          <Button
            variant="outline"
            type="button"
            disabled={loadingLogs}
            onClick={() => void loadAuditData()}
          >
            <Search className="h-3.5 w-3.5" />
            查询
          </Button>
          <Button
            variant="outline"
            type="button"
            disabled={exporting}
            onClick={() => void handleExportCsv()}
            data-testid="audit-export-csv-btn"
          >
            <Download className="h-3.5 w-3.5 text-xblue" />
            {exporting ? "导出中..." : "导出 CSV"}
          </Button>
        </div>
      </Card>

      {/* Audit Log Table with Expandable Detail & Hash Inspector */}
      <Card className="overflow-hidden">
        <div className="overflow-x-auto">
          <table className="w-full text-xs border-collapse">
            <thead>
              <tr className="border-b border-border bg-muted/40 text-left text-muted-foreground">
                <th className="px-3 py-2.5 w-8" />
                <th className="px-3 py-2.5 font-medium">序号 ID</th>
                <th className="px-3 py-2.5 font-medium">时间</th>
                <th className="px-3 py-2.5 font-medium">主体账号</th>
                <th className="px-3 py-2.5 font-medium">动作</th>
                <th className="px-3 py-2.5 font-medium">目标资源</th>
                <th className="px-3 py-2.5 font-medium">状态码</th>
                <th className="px-3 py-2.5 font-medium">行哈希摘要 (row_hash)</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-border">
              {logs.length === 0 ? (
                <tr>
                  <td
                    colSpan={8}
                    className="px-4 py-8 text-center text-muted-foreground"
                  >
                    {loadingLogs ? "正在加载审计日志..." : "未检索到匹配的审计事件"}
                  </td>
                </tr>
              ) : (
                logs.map((entry) => {
                  const isExpanded = expandedId === entry.id;
                  const isBroken =
                    verifyResult &&
                    !verifyResult.ok &&
                    verifyResult.first_broken_id === entry.id;

                  return (
                    <Fragment key={entry.id}>
                      <tr
                        onClick={() =>
                          setExpandedId((prev) =>
                            prev === entry.id ? null : entry.id
                          )
                        }
                        className={cn(
                          "hover:bg-muted/40 cursor-pointer transition-colors",
                          isBroken && "bg-xblue-soft/60 font-medium"
                        )}
                      >
                        <td className="px-3 py-2 text-muted-foreground">
                          {isExpanded ? (
                            <ChevronDown className="h-3.5 w-3.5" />
                          ) : (
                            <ChevronRight className="h-3.5 w-3.5" />
                          )}
                        </td>
                        <td className="px-3 py-2 font-mono-tech">
                          #{entry.id}
                          {isBroken && (
                            <Badge variant="xblue" className="ml-1.5">
                              断点
                            </Badge>
                          )}
                        </td>
                        <td className="px-3 py-2 font-mono-tech text-muted-foreground whitespace-nowrap">
                          {entry.created_at.replace("T", " ").slice(0, 19)}
                        </td>
                        <td className="px-3 py-2 font-mono-tech">
                          {entry.username}
                        </td>
                        <td className="px-3 py-2">
                          <Badge variant="outline">{entry.action}</Badge>
                        </td>
                        <td className="px-3 py-2 font-mono-tech max-w-[220px] truncate">
                          {entry.resource_ref || "—"}
                        </td>
                        <td className="px-3 py-2 font-mono-tech">
                          <Badge
                            variant={
                              entry.status_code >= 400 ? "neutral" : "outline"
                            }
                          >
                            {entry.status_code}
                          </Badge>
                        </td>
                        <td className="px-3 py-2 font-mono-tech text-[11px] text-muted-foreground">
                          {entry.row_hash
                            ? `${entry.row_hash.slice(0, 16)}...`
                            : "—"}
                        </td>
                      </tr>

                      {isExpanded && (
                        <tr className="bg-muted/25">
                          <td colSpan={8} className="px-4 py-3 space-y-2">
                            <div className="grid grid-cols-1 md:grid-cols-3 gap-2 text-[11px] font-mono-tech">
                              <div>
                                <span className="text-muted-foreground">
                                  prev_hash:{" "}
                                </span>
                                <span className="break-all">
                                  {entry.prev_hash || "(GENESIS)"}
                                </span>
                              </div>
                              <div>
                                <span className="text-muted-foreground">
                                  row_hash:{" "}
                                </span>
                                <span className="break-all">
                                  {entry.row_hash || "—"}
                                </span>
                              </div>
                              <div>
                                <span className="text-muted-foreground">
                                  client_ip / trace_id:{" "}
                                </span>
                                <span>
                                  {entry.client_ip} · {entry.trace_id || "—"}
                                </span>
                              </div>
                            </div>
                            <pre className="rounded border border-border bg-background p-2.5 text-[11px] font-mono-tech overflow-x-auto">
                              {JSON.stringify(entry.detail_json ?? {}, null, 2)}
                            </pre>
                          </td>
                        </tr>
                      )}
                    </Fragment>
                  );
                })
              )}
            </tbody>
          </table>
        </div>

        {nextCursor && (
          <div className="border-t border-border p-3 flex justify-center">
            <Button
              variant="outline"
              type="button"
              disabled={loadingLogs}
              onClick={() => void loadAuditData(nextCursor)}
            >
              <RefreshCw className="h-3.5 w-3.5" />
              加载下一页审计记录
            </Button>
          </div>
        )}
      </Card>
    </div>
  );
}
