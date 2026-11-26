import { Fragment, useCallback, useEffect, useState } from "react";
import {
  Archive,
  ChevronDown,
  ChevronRight,
  HardDriveDownload,
  Lock,
  RefreshCw,
} from "lucide-react";
import {
  axopsApi,
  type BackupKind,
  type BackupRecord,
} from "@/lib/api/axops";
import {
  Button,
  Card,
  StatusBadge,
} from "@/components/ui/primitives";
import { useI18n } from "@/i18n";
import { usePrincipal } from "@/lib/permissions";
import { cn } from "@/lib/utils";
import { BACKUP_KIND_OPTIONS } from "./constants";
import { formatByteSize } from "./logic";

export interface BackupPanelProps {
  tenantId: string;
}

export function BackupPanel({ tenantId }: BackupPanelProps) {
  const { isZh, tx, tEnum, fmtTime } = useI18n();
  const { can } = usePrincipal();
  const canWrite = can("OPS_ADMIN");
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
      setErrorMsg(
        err instanceof Error
          ? err.message
          : tx("读取备份记录失败", "Failed to load backups")
      );
    } finally {
      setLoading(false);
    }
  }, [isDefaultTenant, tx]);

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
        tx(
          `已创建 ${tEnum(selectedKind)} 备份任务 (${created.id.slice(0, 8)})`,
          `Created ${selectedKind} backup (${created.id.slice(0, 8)})`
        )
      );
    } catch (err) {
      setErrorMsg(
        err instanceof Error
          ? err.message
          : tx("发起备份失败", "Failed to create backup")
      );
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
      setErrorMsg(
        err instanceof Error
          ? err.message
          : tx("刷新备份状态失败", "Failed to refresh backup")
      );
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
          {tx(
            "平台级备份仅对默认主租户开放",
            "Platform backups are restricted to the default tenant"
          )}
        </div>
        <p className="text-xs text-muted-foreground max-w-lg mx-auto">
          {tx(
            `物理实例与分析湖快照属于全平台共享存储资源（当前租户: ${tenantId}）。`,
            `Physical database and lake snapshots span the entire deployment (current tenant: ${tenantId}).`
          )}
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
              {tx(
                "一致性备份与恢复演练验真",
                "Consistent Backups & Recovery Verification"
              )}
            </div>
            <p className="text-xs text-muted-foreground">
              {tx(
                "目录库转储配合 DuckLake 增量硬链接同步，自动生成 SHA-256 校验清单。",
                "PostgreSQL directory dump paired with hard-link incremental DuckLake sync and SHA-256 manifests."
              )}
            </p>
          </div>

          <div className="flex flex-wrap items-center gap-2">
            {canWrite && (
              <Button
                type="button"
                variant="primary"
                size="sm"
                data-testid="create-backup-btn"
                disabled={creating}
                onClick={() => void handleCreateBackup()}
              >
                <HardDriveDownload className="h-3.5 w-3.5" />
                {creating
                  ? tx("创建中…", "Creating…")
                  : tx("创建备份", "Create Backup")}
              </Button>
            )}

            <Button
              type="button"
              variant="outline"
              size="sm"
              data-testid="refresh-backups-btn"
              disabled={loading}
              onClick={() => void loadBackups()}
            >
              <RefreshCw className={cn("h-3.5 w-3.5", loading && "animate-spin")} />
              {tx("刷新", "Refresh")}
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
              <div className="font-semibold">
                {isZh ? opt.label : opt.enLabel}{" "}
                <span className="font-mono-tech text-[11px] text-muted-foreground">
                  ({opt.kind})
                </span>
              </div>
              <div className="text-[11px] text-muted-foreground mt-0.5 leading-snug">
                {isZh ? opt.description : opt.enDescription}
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
                <th className="py-2.5 px-3 whitespace-nowrap">
                  {tx("类型 / ID", "Kind / ID")}
                </th>
                <th className="py-2.5 px-3 whitespace-nowrap">
                  {tx("状态", "Status")}
                </th>
                <th className="py-2.5 px-3 whitespace-nowrap">
                  {tx("归档大小", "Size")}
                </th>
                <th className="py-2.5 px-3 whitespace-nowrap">
                  {tx("存储位置", "Location")}
                </th>
                <th className="py-2.5 px-3 whitespace-nowrap">
                  {tx("时间线", "Timeline")}
                </th>
                <th className="py-2.5 px-3 text-right whitespace-nowrap">
                  {tx("操作", "Actions")}
                </th>
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
                    {loading
                      ? tx("加载中…", "Loading…")
                      : tx("暂无备份记录", "No backup records")}
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
                          <div className="font-semibold">
                            {tEnum(rec.backup_kind)}{" "}
                            <span className="font-mono-tech text-[10px] text-muted-foreground">
                              {rec.backup_kind}
                            </span>
                          </div>
                          <div className="font-mono-tech text-[11px] text-muted-foreground">
                            {rec.id.slice(0, 8)}…
                          </div>
                        </td>
                        <td className="py-2.5 px-3 whitespace-nowrap">
                          <StatusBadge status={rec.status} />
                        </td>
                        <td className="py-2.5 px-3 font-mono-tech whitespace-nowrap">
                          {formatByteSize(rec.size_bytes)}
                        </td>
                        <td className="py-2.5 px-3 font-mono-tech text-[11px] max-w-xs truncate">
                          {rec.location || tx("待分配目录", "Pending allocation")}
                        </td>
                        <td className="py-2.5 px-3 font-mono-tech text-[11px] text-muted-foreground whitespace-nowrap">
                          <div>{fmtTime(rec.started_at, true)}</div>
                          {rec.finished_at && (
                            <div>{fmtTime(rec.finished_at, true)}</div>
                          )}
                          {rec.verified_at && (
                            <div className="text-xblue">
                              ✓ {fmtTime(rec.verified_at, true)}
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
                            size="sm"
                            onClick={() => void handleRefreshSingle(rec.id)}
                          >
                            <RefreshCw className="h-3 w-3" />
                            {tx("刷新", "Refresh")}
                          </Button>
                        </td>
                      </tr>

                      {isExpanded && (
                        <tr className="bg-muted/20">
                          <td colSpan={7} className="p-3.5 space-y-2">
                            {rec.error_message && (
                              <div className="rounded border border-red-500/40 bg-red-500/10 p-2 text-[11px] font-mono-tech text-red-700 dark:text-red-300">
                                {rec.error_message}
                              </div>
                            )}
                            <div className="text-[11px] font-medium text-muted-foreground">
                              {tx("校验清单摘要", "Manifest Summary")}
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
