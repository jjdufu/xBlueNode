import { Fragment, useCallback, useEffect, useState } from "react";
import {
  Archive,
  ChevronDown,
  ChevronRight,
  HardDriveDownload,
  Lock,
  RefreshCw,
  ShieldCheck,
} from "lucide-react";
import {
  axopsApi,
  type BackupKind,
  type BackupRecord,
  type BackupStatus,
} from "@/lib/api/axops";
import { Button, Card } from "@/components/ui/primitives";
import { cn } from "@/lib/utils";
import { BACKUP_KIND_OPTIONS, BACKUP_STATUS_LABELS } from "./constants";
import { formatByteSize } from "./logic";

export interface BackupPanelProps {
  tenantId: string;
}

function getBackupStatusClass(status: BackupStatus): string {
  switch (status) {
    case "VERIFIED":
      return "border border-xblue/50 bg-xblue-soft text-xblue";
    case "SUCCEEDED":
      return "border border-emerald-500/40 bg-emerald-500/10 text-emerald-700 dark:text-emerald-300";
    case "PENDING":
    case "RUNNING":
      return "border border-amber-500/40 bg-amber-500/10 text-amber-700 dark:text-amber-300";
    case "FAILED":
      return "border border-red-500/40 bg-red-500/10 text-red-600 dark:text-red-400";
  }
}

function formatDateTime(iso: string | null | undefined): string {
  if (!iso) return "—";
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  return d.toLocaleString("zh-CN", {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
    hour12: false,
  });
}

export function BackupPanel({ tenantId }: BackupPanelProps) {
  const isDefaultTenant = tenantId === "default";
  const [selectedKind, setSelectedKind] = useState<BackupKind>("FULL");
  const [backups, setBackups] = useState<BackupRecord[]>([]);
  const [loading, setLoading] = useState(false);
  const [creating, setCreating] = useState(false);
  const [expandedId, setExpandedId] = useState<string | null>(null);
  const [noticeMsg, setNoticeMsg] = useState<string | null>(null);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  const loadBackups = useCallback(async () => {
    if (!isDefaultTenant) {
      setBackups([]);
      return;
    }
    setLoading(true);
    setErrorMsg(null);
    try {
      const res = await axopsApi.listBackups({ limit: 50 });
      setBackups(Array.isArray(res?.items) ? res.items : []);
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : "读取备份记录失败");
    } finally {
      setLoading(false);
    }
  }, [isDefaultTenant]);

  useEffect(() => {
    void loadBackups();
  }, [loadBackups]);

  const handleCreateBackup = async () => {
    if (!isDefaultTenant) return;
    setCreating(true);
    setErrorMsg(null);
    setNoticeMsg(null);
    try {
      const created = await axopsApi.createBackup(selectedKind);
      setBackups((prev) => [
        created,
        ...prev.filter((item) => item.id !== created.id),
      ]);
      setNoticeMsg(
        `已发起 ${selectedKind} 平台备份请求 (ID: ${created.id.slice(0, 8)} · 状态: ${created.status})`
      );
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : "发起备份失败");
    } finally {
      setCreating(false);
    }
  };

  const handleRefreshSingle = async (backupId: string) => {
    try {
      const latest = await axopsApi.getBackup(backupId);
      setBackups((prev) =>
        prev.map((item) => (item.id === backupId ? latest : item))
      );
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : "刷新备份状态失败");
    }
  };

  if (!isDefaultTenant) {
    return (
      <Card
        className="p-8 text-center space-y-2.5"
        data-testid="backup-non-default-tenant-notice"
      >
        <Lock className="h-6 w-6 text-muted-foreground mx-auto" />
        <div className="text-sm font-semibold">
          平台级备份仅在 default 租户向 OPS_ADMIN 开放
        </div>
        <p className="text-xs text-muted-foreground max-w-lg mx-auto">
          数据库实例与 DuckLake 数据卷备份涵盖全平台物理快照（当前租户:{" "}
          <span className="font-mono-tech">{tenantId}</span>
          ）。请切换至 default 租户下的平台管理员账号查看或触发备份任务。
        </p>
      </Card>
    );
  }

  return (
    <div className="space-y-4" data-testid="axops-backup-panel">
      {/* Create Backup Header Card */}
      <Card className="p-4 space-y-3">
        <div className="flex flex-wrap items-start justify-between gap-3">
          <div className="space-y-1">
            <div className="text-sm font-semibold flex items-center gap-2">
              <Archive className="h-4 w-4 text-xblue" />
              平台级一致性备份与恢复演练验真 (ax_backups)
            </div>
            <p className="text-xs text-muted-foreground">
              先执行 PostgreSQL 目录转储，再通过硬链接增量同步 DuckLake 只追加文件与资产，并生成 SHA-256 校验清单（RPO ≤ 60s / RTO ≤ 30min）。
            </p>
          </div>

          <div className="flex flex-wrap items-center gap-2">
            <select
              data-testid="backup-kind-select"
              value={selectedKind}
              onChange={(e) => setSelectedKind(e.target.value as BackupKind)}
              className="h-8 rounded-md border border-input bg-background px-2.5 text-xs font-medium"
            >
              {BACKUP_KIND_OPTIONS.map((opt) => (
                <option key={opt.kind} value={opt.kind}>
                  {opt.label}
                </option>
              ))}
            </select>

            <Button
              type="button"
              variant="primary"
              className="h-8 px-3 text-xs"
              data-testid="create-backup-btn"
              disabled={creating}
              onClick={() => void handleCreateBackup()}
            >
              <HardDriveDownload className="h-3.5 w-3.5" />
              {creating ? "创建中..." : "创建备份"}
            </Button>

            <Button
              type="button"
              variant="outline"
              className="h-8 px-2.5 text-xs"
              data-testid="refresh-backups-btn"
              disabled={loading}
              onClick={() => void loadBackups()}
            >
              <RefreshCw className={cn("h-3.5 w-3.5", loading && "animate-spin")} />
              刷新
            </Button>
          </div>
        </div>

        <div className="grid grid-cols-1 md:grid-cols-3 gap-2.5 pt-1">
          {BACKUP_KIND_OPTIONS.map((opt) => (
            <button
              key={opt.kind}
              type="button"
              onClick={() => setSelectedKind(opt.kind)}
              className={cn(
                "rounded-md border p-2.5 text-left text-xs transition-colors cursor-pointer",
                selectedKind === opt.kind
                  ? "border-xblue bg-xblue-soft/40 text-foreground"
                  : "border-border bg-muted/20 text-muted-foreground hover:text-foreground"
              )}
            >
              <div className="font-semibold font-mono-tech">{opt.label}</div>
              <div className="text-[11px] text-muted-foreground mt-0.5 leading-snug">
                {opt.description}
              </div>
            </button>
          ))}
        </div>
      </Card>

      {errorMsg && (
        <div
          role="alert"
          className="rounded-md border border-red-500/40 bg-red-500/10 px-3.5 py-2 text-xs text-red-700 dark:text-red-300"
        >
          {errorMsg}
        </div>
      )}

      {noticeMsg && (
        <div
          data-testid="axops-backup-notice"
          className="rounded-md border border-xblue/35 bg-xblue-soft px-3.5 py-2 text-xs text-xblue"
        >
          {noticeMsg}
        </div>
      )}

      {/* Backups List */}
      <Card className="overflow-hidden">
        <div className="overflow-x-auto">
          <table className="w-full text-left border-collapse text-xs">
            <thead>
              <tr className="border-b border-border bg-muted/40 text-[11px] text-muted-foreground">
                <th className="py-2.5 px-3 w-8" />
                <th className="py-2.5 px-3">备份类型 / ID</th>
                <th className="py-2.5 px-3">状态</th>
                <th className="py-2.5 px-3">归档大小</th>
                <th className="py-2.5 px-3">存储路径 (location)</th>
                <th className="py-2.5 px-3">开始 / 完成 / 验真时间</th>
                <th className="py-2.5 px-3 text-right">操作</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-border">
              {backups.length === 0 ? (
                <tr>
                  <td
                    colSpan={7}
                    className="py-10 text-center text-muted-foreground"
                    data-testid="axops-backups-empty"
                  >
                    {loading ? "正在加载备份记录..." : "暂无平台备份记录"}
                  </td>
                </tr>
              ) : (
                backups.map((rec) => {
                  const isExpanded = expandedId === rec.id;
                  return (
                    <Fragment key={rec.id}>
                      <tr
                        data-testid={`backup-row-${rec.id}`}
                        className="hover:bg-muted/30 transition-colors cursor-pointer"
                        onClick={() =>
                          setExpandedId((prev) =>
                            prev === rec.id ? null : rec.id
                          )
                        }
                      >
                        <td className="py-2.5 px-3 text-muted-foreground">
                          {isExpanded ? (
                            <ChevronDown className="h-3.5 w-3.5" />
                          ) : (
                            <ChevronRight className="h-3.5 w-3.5" />
                          )}
                        </td>
                        <td className="py-2.5 px-3">
                          <div className="font-semibold font-mono-tech">
                            {rec.backup_kind}
                          </div>
                          <div className="font-mono-tech text-[11px] text-muted-foreground">
                            {rec.id}
                          </div>
                        </td>
                        <td className="py-2.5 px-3 whitespace-nowrap">
                          <span
                            className={cn(
                              "inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-[11px] font-mono-tech font-medium",
                              getBackupStatusClass(rec.status)
                            )}
                          >
                            {rec.status === "VERIFIED" && (
                              <ShieldCheck className="h-3 w-3" />
                            )}
                            {BACKUP_STATUS_LABELS[rec.status] ?? rec.status}
                          </span>
                        </td>
                        <td className="py-2.5 px-3 font-mono-tech whitespace-nowrap">
                          {formatByteSize(rec.size_bytes)}
                        </td>
                        <td className="py-2.5 px-3 font-mono-tech text-[11px] max-w-xs truncate">
                          {rec.location || "（等待 axpoint-ops 分配目录）"}
                        </td>
                        <td className="py-2.5 px-3 font-mono-tech text-[11px] text-muted-foreground whitespace-nowrap">
                          <div>始: {formatDateTime(rec.started_at)}</div>
                          <div>终: {formatDateTime(rec.finished_at)}</div>
                          {rec.verified_at && (
                            <div className="text-xblue">
                              验: {formatDateTime(rec.verified_at)}
                            </div>
                          )}
                        </td>
                        <td
                          className="py-2.5 px-3 text-right"
                          onClick={(e) => e.stopPropagation()}
                        >
                          <Button
                            type="button"
                            variant="ghost"
                            className="h-7 px-2 text-[11px]"
                            onClick={() => void handleRefreshSingle(rec.id)}
                          >
                            <RefreshCw className="h-3 w-3" />
                            刷新单条
                          </Button>
                        </td>
                      </tr>

                      {isExpanded && (
                        <tr className="bg-muted/20">
                          <td colSpan={7} className="p-3.5 space-y-2">
                            {rec.error_message && (
                              <div className="rounded border border-red-500/40 bg-red-500/10 p-2 text-[11px] font-mono-tech text-red-700 dark:text-red-300">
                                失败详情: {rec.error_message}
                              </div>
                            )}
                            <div className="text-[11px] font-medium text-muted-foreground">
                              备份元数据与校验清单摘要 (manifest):
                            </div>
                            <pre className="rounded border border-border bg-background p-2.5 text-[11px] font-mono-tech overflow-x-auto">
                              {JSON.stringify(rec.manifest ?? {}, null, 2)}
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
      </Card>
    </div>
  );
}
