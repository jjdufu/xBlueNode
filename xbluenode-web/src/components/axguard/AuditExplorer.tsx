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
import { axhubApi, type Dataset } from "@/lib/api/axhub";
import {
  axguardApi,
  type AuditLogEntry,
  type AuditStatsResponse,
  type AuditVerifyResponse,
  type GuardUserRecord,
} from "@/lib/api/axguard";
import { Badge, Button, Card, Input, Select } from "@/components/ui/primitives";
import { useI18n } from "@/i18n";
import { usePrincipal } from "@/lib/permissions";
import { cn } from "@/lib/utils";
import { AUDIT_ACTIONS } from "./constants";

export interface AuditExplorerProps {
  datasets?: Dataset[];
  users?: GuardUserRecord[];
}

export function AuditExplorer({
  datasets = [],
  users = [],
}: AuditExplorerProps) {
  const { tx, tEnum, fmtNum, fmtTime } = useI18n();
  const { can } = usePrincipal();
  const canReadDatasets = can("DATA_READ");
  const canListUsers = can("GUARD_ADMIN");

  const [fallbackDatasets, setFallbackDatasets] = useState<Dataset[]>([]);
  const [fallbackUsers, setFallbackUsers] = useState<GuardUserRecord[]>([]);

  useEffect(() => {
    if (datasets.length > 0 || !canReadDatasets) return;
    let cancelled = false;
    void axhubApi
      .listDatasets({ limit: 100 })
      .then((res) => {
        if (!cancelled && Array.isArray(res?.items)) {
          setFallbackDatasets(res.items);
        }
      })
      .catch(() => null);
    return () => {
      cancelled = true;
    };
  }, [canReadDatasets, datasets.length]);

  useEffect(() => {
    if (users.length > 0 || !canListUsers) return;
    let cancelled = false;
    void axguardApi
      .listUsers()
      .then((list) => {
        if (!cancelled) setFallbackUsers(list);
      })
      .catch(() => null);
    return () => {
      cancelled = true;
    };
  }, [canListUsers, users.length]);

  const datasetLabelById = useMemo(() => {
    const list = datasets.length > 0 ? datasets : fallbackDatasets;
    const map = new Map<string, string>();
    for (const d of list) {
      map.set(
        d.id,
        d.display_name && d.display_name !== d.name
          ? `${d.name} (${d.display_name})`
          : d.name
      );
    }
    return map;
  }, [datasets, fallbackDatasets]);

  const userLabelById = useMemo(() => {
    const list = users.length > 0 ? users : fallbackUsers;
    const map = new Map<string, string>();
    for (const u of list) {
      map.set(
        u.id,
        u.display_name && u.display_name !== u.username
          ? `${u.username} (${u.display_name})`
          : u.username
      );
    }
    return map;
  }, [fallbackUsers, users]);

  const formatResourceTarget = useCallback(
    (rawRef: string): string => {
      if (!rawRef) return "—";
      if (rawRef.startsWith("dataset:")) {
        const idPart = rawRef.slice("dataset:".length);
        const resolved = datasetLabelById.get(idPart);
        if (resolved) {
          return `dataset:${resolved}`;
        }
      }
      if (rawRef.startsWith("user:")) {
        const idPart = rawRef.slice("user:".length);
        const resolved = userLabelById.get(idPart);
        if (resolved) {
          return `user:${resolved}`;
        }
      }
      return rawRef;
    },
    [datasetLabelById, userLabelById]
  );

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
      setBanner(tx("已导出审计日志 CSV", "Exported audit logs CSV"));
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
              {tx(
                "SHA-256 防篡改审计哈希链验真",
                "SHA-256 Tamper-Evident Hash Chain Verification"
              )}
            </div>
            <p className="text-[11px] text-muted-foreground">
              {tx(
                "逐行重算载荷与链式摘要，定位篡改或断链记录。",
                "Recomputes canonical payloads and prev_hash links across audit records."
              )}
            </p>
          </div>

          <div className="flex flex-wrap items-center gap-2">
            <Input
              type="number"
              placeholder={tx("起始 ID", "Start ID")}
              value={verifyFromId}
              onChange={(e) => setVerifyFromId(e.target.value)}
              className="h-8 w-32 text-xs font-mono-tech"
            />
            <Input
              type="number"
              placeholder={tx("结束 ID", "End ID")}
              value={verifyToId}
              onChange={(e) => setVerifyToId(e.target.value)}
              className="h-8 w-32 text-xs font-mono-tech"
            />
            <Button
              type="button"
              disabled={verifying}
              onClick={() => void handleVerifyChain()}
              data-testid="audit-verify-btn"
            >
              {verifying
                ? tx("验真中…", "Verifying…")
                : tx("哈希链验真", "Verify Chain")}
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
              <span className="font-semibold">
                {tx("哈希链完整无篡改", "Hash chain verified intact")}
              </span>
              <span className="font-mono-tech text-muted-foreground">
                ·{" "}
                {tx(
                  `已校验 ${fmtNum(verifyResult.checked_count, 0)} 条`,
                  `${fmtNum(verifyResult.checked_count, 0)} records checked`
                )}
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
                  {tx("审计哈希链断裂", "Hash chain integrity broken")}
                </span>
                <span className="font-mono-tech">
                  {tx(
                    `首个异常记录 ID: #${verifyResult.first_broken_id ?? "—"}（已扫描 ${verifyResult.checked_count} 条）`,
                    `First broken ID: #${verifyResult.first_broken_id ?? "—"} (${verifyResult.checked_count} scanned)`
                  )}
                </span>
              </div>
              <Badge variant="danger">
                {tx("哈希链断裂", "Chain Broken")}
              </Badge>
            </div>
          ))}
      </Card>

      {/* Top KPI & Visual Trend / Action Distribution */}
      <div className="grid grid-cols-1 lg:grid-cols-12 gap-4">
        <Card className="lg:col-span-4 p-4 space-y-3">
          <div className="text-xs font-semibold">
            {tx("审计概览", "Audit Summary")}
          </div>
          <div className="grid grid-cols-2 gap-2.5">
            <div className="rounded border border-border bg-muted/20 p-2.5">
              <div className="text-[11px] text-muted-foreground">
                {tx("总事件数", "Total Events")}
              </div>
              <div className="text-lg font-mono-tech font-semibold mt-0.5">
                {fmtNum(totalActionCount || logs.length, 0)}
              </div>
            </div>
            <div className="rounded border border-border bg-muted/20 p-2.5">
              <div className="text-[11px] text-muted-foreground">
                {tx("活跃主体", "Active Actors")}
              </div>
              <div className="text-lg font-mono-tech font-semibold mt-0.5">
                {fmtNum(stats.by_user.length, 0)}
              </div>
            </div>
          </div>

          <div className="space-y-1.5 pt-1">
            <div className="text-[11px] text-muted-foreground">
              {tx("活跃用户排行", "Top Active Users")}
            </div>
            {stats.by_user.length === 0 ? (
              <div className="text-[11px] text-muted-foreground">
                {tx("暂无统计", "No data")}
              </div>
            ) : (
              stats.by_user.slice(0, 4).map(([uname, cnt]) => (
                <div
                  key={uname}
                  className="flex items-center justify-between text-xs font-mono-tech"
                >
                  <span className="truncate">{uname}</span>
                  <Badge variant="outline">{fmtNum(cnt, 0)}</Badge>
                </div>
              ))
            )}
          </div>
        </Card>

        <Card className="lg:col-span-4 p-4 space-y-2.5">
          <div className="text-xs font-semibold">
            {tx("动作分布", "Events by Action")}
          </div>
          {actionSeries.length === 0 ? (
            <div className="py-6 text-center text-[11px] text-muted-foreground">
              {tx("暂无数据", "No data")}
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
                      <span title={act}>{tEnum(act)}</span>
                      <span>{fmtNum(count, 0)}</span>
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
          <div className="text-xs font-semibold">
            {tx("日度审计趋势", "Daily Audit Trend")}
          </div>
          {stats.by_day.length === 0 ? (
            <div className="py-6 text-center text-[11px] text-muted-foreground">
              {tx("暂无趋势数据", "No trend data")}
            </div>
          ) : (
            <svg
              viewBox="0 0 280 120"
              className="w-full h-32 overflow-visible"
              role="img"
              aria-label={tx("日度审计事件趋势", "Daily audit event trend")}
            >
              {[0, 0.5, 1].map((ratio) => {
                const gy = Math.round(92 - ratio * 66);
                const val = Math.round(maxDayVal * ratio);
                return (
                  <g key={ratio}>
                    <line
                      x1={28}
                      x2={272}
                      y1={gy}
                      y2={gy}
                      stroke="var(--border)"
                      strokeDasharray={ratio === 0 ? undefined : "2 2"}
                      strokeWidth={1}
                    />
                    <text
                      x={24}
                      y={gy + 3}
                      textAnchor="end"
                      fontSize="8"
                      fill="var(--muted-foreground)"
                    >
                      {val}
                    </text>
                  </g>
                );
              })}
              {stats.by_day.slice(-10).map(([day, count], idx, arr) => {
                const slotW = 240 / Math.max(1, arr.length);
                const barW = Math.max(10, Math.min(24, slotW - 6));
                const barH = Math.max(4, Math.round((count / maxDayVal) * 66));
                const x = 30 + idx * slotW + (slotW - barW) / 2;
                const y = 92 - barH;
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
                      y={Math.max(10, y - 4)}
                      textAnchor="middle"
                      fontSize="8"
                      fill="var(--foreground)"
                    >
                      {fmtNum(count, 0)}
                    </text>
                    <text
                      x={x + barW / 2}
                      y={108}
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
          <label className="block text-[11px] text-muted-foreground">
            {tx("动作类型", "Action")}
          </label>
          <Select
            value={actionFilter}
            onChange={(e) => setActionFilter(e.target.value)}
            className="h-8"
          >
            <option value="">{tx("全部动作", "All Actions")}</option>
            {AUDIT_ACTIONS.map((act) => (
              <option key={act} value={act}>
                {tEnum(act)} ({act})
              </option>
            ))}
          </Select>
        </div>

        <div className="space-y-1">
          <label className="block text-[11px] text-muted-foreground">
            {tx("操作账号", "Username")}
          </label>
          <Input
            placeholder="ops_admin"
            value={usernameFilter}
            onChange={(e) => setUsernameFilter(e.target.value)}
            className="h-8 w-32 text-xs font-mono-tech"
          />
        </div>

        <div className="space-y-1">
          <label className="text-[11px] text-muted-foreground">
            {tx("目标资源", "Resource")}
          </label>
          <Input
            placeholder="dataset:..."
            value={resourceFilter}
            onChange={(e) => setResourceFilter(e.target.value)}
            className="h-8 w-40 text-xs font-mono-tech"
          />
        </div>

        <div className="space-y-1">
          <label className="text-[11px] text-muted-foreground">
            {tx("状态码", "Status Code")}
          </label>
          <Input
            type="number"
            placeholder="200"
            value={statusCodeFilter}
            onChange={(e) => setStatusCodeFilter(e.target.value)}
            className="h-8 w-24 text-xs font-mono-tech"
          />
        </div>

        <div className="space-y-1">
          <label className="text-[11px] text-muted-foreground">
            {tx("起始时间", "From")}
          </label>
          <Input
            type="datetime-local"
            value={fromTime}
            onChange={(e) => setFromTime(e.target.value)}
            className="h-8 w-44 text-xs font-mono-tech"
          />
        </div>

        <div className="space-y-1">
          <label className="text-[11px] text-muted-foreground">
            {tx("截止时间", "To")}
          </label>
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
            {tx("查询", "Query")}
          </Button>
          <Button
            variant="outline"
            type="button"
            disabled={exporting}
            onClick={() => void handleExportCsv()}
            data-testid="audit-export-csv-btn"
          >
            <Download className="h-3.5 w-3.5 text-xblue" />
            {exporting
              ? tx("导出中…", "Exporting…")
              : tx("导出 CSV", "Export CSV")}
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
                <th className="px-3 py-2.5 font-medium whitespace-nowrap">ID</th>
                <th className="px-3 py-2.5 font-medium whitespace-nowrap">
                  {tx("时间", "Timestamp")}
                </th>
                <th className="px-3 py-2.5 font-medium whitespace-nowrap">
                  {tx("账号", "Actor")}
                </th>
                <th className="px-3 py-2.5 font-medium whitespace-nowrap">
                  {tx("动作", "Action")}
                </th>
                <th className="px-3 py-2.5 font-medium whitespace-nowrap">
                  {tx("目标资源", "Target")}
                </th>
                <th className="px-3 py-2.5 font-medium whitespace-nowrap">
                  {tx("状态码", "HTTP")}
                </th>
                <th className="px-3 py-2.5 font-medium whitespace-nowrap">
                  {tx("行哈希摘要", "Row Hash")}
                </th>
              </tr>
            </thead>
            <tbody className="divide-y divide-border">
              {logs.length === 0 ? (
                <tr>
                  <td
                    colSpan={8}
                    className="px-4 py-8 text-center text-muted-foreground"
                  >
                    {loadingLogs
                      ? tx("加载中…", "Loading…")
                      : tx("暂无审计记录", "No audit records")}
                  </td>
                </tr>
              ) : (
                logs.map((entry) => {
                  const isExpanded = expandedId === entry.id;
                  const isBroken =
                    verifyResult &&
                    !verifyResult.ok &&
                    verifyResult.first_broken_id === entry.id;
                  const displayTarget = formatResourceTarget(entry.resource_ref);

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
                        <td className="px-3 py-2 font-mono-tech whitespace-nowrap">
                          #{entry.id}
                          {isBroken && (
                            <Badge variant="danger" className="ml-1.5">
                              {tx("断点", "Broken")}
                            </Badge>
                          )}
                        </td>
                        <td className="px-3 py-2 font-mono-tech text-muted-foreground whitespace-nowrap">
                          {fmtTime(entry.created_at)}
                        </td>
                        <td className="px-3 py-2 font-mono-tech whitespace-nowrap">
                          {entry.username}
                        </td>
                        <td className="px-3 py-2 whitespace-nowrap">
                          <Badge variant="outline" title={entry.action}>
                            {tEnum(entry.action)}
                          </Badge>
                        </td>
                        <td
                          className="px-3 py-2 font-mono-tech max-w-[260px] truncate"
                          title={entry.resource_ref || undefined}
                        >
                          {displayTarget}
                        </td>
                        <td className="px-3 py-2 font-mono-tech whitespace-nowrap">
                          <Badge
                            variant={
                              entry.status_code >= 400 ? "danger" : "outline"
                            }
                          >
                            {entry.status_code}
                          </Badge>
                        </td>
                        <td className="px-3 py-2 font-mono-tech text-[11px] text-muted-foreground whitespace-nowrap">
                          {entry.row_hash
                            ? `${entry.row_hash.slice(0, 16)}…`
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
              {tx("加载更多", "Load More")}
            </Button>
          </div>
        )}
      </Card>
    </div>
  );
}
