import { useCallback, useEffect, useMemo, useState } from "react";
import {
  AlertTriangle,
  Check,
  CheckCircle2,
  Copy,
  KeyRound,
  Lock,
  Share2,
  Trash2,
  X,
} from "lucide-react";
import {
  xblueviewApi,
  type CreateShareResponse,
  type DashboardStatus,
  type FilterDef,
  type ShareLinkRecord,
} from "@/lib/api/xblueview";
import { Badge, Button, Input } from "@/components/ui/primitives";
import { isNonEmptyFilterValue } from "./logic";

export interface ShareDialogTarget {
  targetKind: "DASHBOARD" | "EXPORT_FILE";
  targetId: string;
  targetName: string;
  dashboardStatus?: DashboardStatus;
  dashboardFilters?: FilterDef[];
  activeFilterValues?: Record<string, unknown>;
}

export interface ShareDialogProps {
  open: boolean;
  target: ShareDialogTarget | null;
  onClose: () => void;
  onPublishDashboard?: () => Promise<void>;
}

const EXPIRY_PRESETS: readonly { hours: number; label: string }[] = [
  { hours: 24, label: "24h" },
  { hours: 72, label: "72h (3天)" },
  { hours: 168, label: "168h (7天)" },
  { hours: 720, label: "720h (30天)" },
];

function formatShareTime(raw: string | null | undefined): string {
  if (!raw) return "—";
  const d = new Date(raw);
  if (Number.isNaN(d.getTime())) return raw;
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(
    d.getHours()
  )}:${pad(d.getMinutes())}`;
}

function summarizeFilterValue(val: unknown): string {
  if (Array.isArray(val)) {
    return val.map((v) => String(v ?? "")).join(" ~ ");
  }
  if (typeof val === "object" && val !== null) {
    return JSON.stringify(val);
  }
  return String(val ?? "");
}

function resolveShareStatus(link: ShareLinkRecord): {
  label: string;
  active: boolean;
} {
  if (link.revoked_at) {
    return { label: "已撤销", active: false };
  }
  if (new Date(link.expires_at).getTime() <= Date.now()) {
    return { label: "已过期", active: false };
  }
  if (
    link.max_views !== null &&
    link.max_views !== undefined &&
    link.view_count >= link.max_views
  ) {
    return { label: "次数耗尽", active: false };
  }
  return { label: "生效中", active: true };
}

export function ShareDialog({
  open,
  target,
  onClose,
  onPublishDashboard,
}: ShareDialogProps) {
  const [expiresInHours, setExpiresInHours] = useState<number>(72);
  const [maxViewsStr, setMaxViewsStr] = useState<string>("");
  const [password, setPassword] = useState<string>("");
  const [lockedKeySet, setLockedKeySet] = useState<Set<string>>(
    () => new Set()
  );
  const [allowFilters, setAllowFilters] = useState<boolean>(false);
  const [allowExport, setAllowExport] = useState<boolean>(false);

  const [creating, setCreating] = useState<boolean>(false);
  const [publishing, setPublishing] = useState<boolean>(false);
  const [createdShare, setCreatedShare] = useState<CreateShareResponse | null>(
    null
  );
  const [copied, setCopied] = useState<boolean>(false);

  const [shares, setShares] = useState<ShareLinkRecord[]>([]);
  const [loadingShares, setLoadingShares] = useState<boolean>(false);
  const [revokingId, setRevokingId] = useState<string | null>(null);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  const activeFilterCandidates = useMemo(() => {
    if (!target || target.targetKind !== "DASHBOARD") return [];
    const values = target.activeFilterValues ?? {};
    const defs = target.dashboardFilters ?? [];
    const items: { key: string; label: string; value: unknown }[] = [];
    const seen = new Set<string>();

    for (const def of defs) {
      const val = values[def.key];
      if (isNonEmptyFilterValue(val)) {
        seen.add(def.key);
        items.push({ key: def.key, label: def.label || def.key, value: val });
      }
    }
    for (const [k, val] of Object.entries(values)) {
      if (!seen.has(k) && isNonEmptyFilterValue(val)) {
        items.push({ key: k, label: k, value: val });
      }
    }
    return items;
  }, [target]);

  const loadShares = useCallback(async (targetId: string) => {
    setLoadingShares(true);
    try {
      const res = await xblueviewApi.listShares({
        target_id: targetId,
        limit: 50,
      });
      setShares(Array.isArray(res?.items) ? res.items : []);
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setLoadingShares(false);
    }
  }, []);

  useEffect(() => {
    if (!open || !target) return;
    setExpiresInHours(72);
    setMaxViewsStr("");
    setPassword("");
    setAllowFilters(false);
    setAllowExport(target.targetKind === "EXPORT_FILE");
    setCreatedShare(null);
    setCopied(false);
    setErrorMsg(null);
    setLockedKeySet(
      new Set(activeFilterCandidates.map((item) => item.key))
    );
    void loadShares(target.targetId);
  }, [activeFilterCandidates, loadShares, open, target]);

  if (!open || !target) return null;

  const isDraftDashboard =
    target.targetKind === "DASHBOARD" &&
    target.dashboardStatus !== undefined &&
    target.dashboardStatus !== "PUBLISHED";

  const fullShareUrl = createdShare
    ? `${typeof window !== "undefined" ? window.location.origin : ""}${createdShare.url}`
    : "";

  const handleCopyUrl = async () => {
    if (!fullShareUrl) return;
    try {
      await navigator.clipboard.writeText(fullShareUrl);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    } catch {
      setCopied(false);
    }
  };

  const handlePublishFirst = async () => {
    if (!onPublishDashboard) return;
    setPublishing(true);
    setErrorMsg(null);
    try {
      await onPublishDashboard();
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setPublishing(false);
    }
  };

  const handleCreateShare = async (e: React.FormEvent) => {
    e.preventDefault();
    if (isDraftDashboard) {
      setErrorMsg("请先发布驾驶舱后再生成分享链接");
      return;
    }

    const clampedHours = Math.max(
      1,
      Math.min(720, Math.round(expiresInHours) || 24)
    );
    let parsedMaxViews: number | null = null;
    if (maxViewsStr.trim().length > 0) {
      const n = Number(maxViewsStr);
      if (!Number.isInteger(n) || n < 1) {
        setErrorMsg("最大访问次数必须为大于等于 1 的整数");
        return;
      }
      parsedMaxViews = n;
    }

    const lockedFilters: Record<string, unknown> = {};
    if (target.targetKind === "DASHBOARD") {
      for (const item of activeFilterCandidates) {
        if (lockedKeySet.has(item.key)) {
          lockedFilters[item.key] = item.value;
        }
      }
    }

    setCreating(true);
    setErrorMsg(null);
    try {
      const res = await xblueviewApi.createShare({
        target_kind: target.targetKind,
        target_id: target.targetId,
        expires_in_hours: clampedHours,
        max_views: parsedMaxViews,
        password: password.trim().length > 0 ? password.trim() : null,
        locked_filters: lockedFilters,
        allow_filters: target.targetKind === "DASHBOARD" ? allowFilters : false,
        allow_export:
          target.targetKind === "EXPORT_FILE" ? true : allowExport,
      });
      setCreatedShare(res);
      setPassword("");
      await loadShares(target.targetId);
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setCreating(false);
    }
  };

  const handleRevoke = async (shareId: string) => {
    setRevokingId(shareId);
    setErrorMsg(null);
    try {
      await xblueviewApi.revokeShare(shareId);
      await loadShares(target.targetId);
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setRevokingId(null);
    }
  };

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/45 p-4"
      data-testid="share-dialog-backdrop"
      onClick={onClose}
    >
      <div
        className="w-full max-w-2xl max-h-[90vh] rounded-md border border-border bg-background text-foreground shadow-xl flex flex-col overflow-hidden"
        data-testid="share-dialog"
        onClick={(e) => e.stopPropagation()}
      >
        {/* Header */}
        <div className="px-4 py-3 border-b border-border flex items-center justify-between gap-2 shrink-0">
          <div className="flex items-center gap-2 min-w-0">
            <Share2 className="h-4 w-4 text-xblue shrink-0" />
            <h3 className="text-sm font-semibold truncate">
              安全分享 · {target.targetName}
            </h3>
            <Badge variant="xblue">{target.targetKind}</Badge>
          </div>
          <button
            type="button"
            onClick={onClose}
            className="rounded p-1 text-muted-foreground hover:text-foreground hover:bg-muted cursor-pointer"
            aria-label="关闭安全分享窗口"
          >
            <X className="h-4 w-4" />
          </button>
        </div>

        <div className="flex-1 overflow-y-auto p-4 space-y-5 text-xs">
          {errorMsg && (
            <div
              className="rounded border border-foreground/30 bg-muted px-3 py-2 text-xs"
              data-testid="share-dialog-error"
            >
              {errorMsg}
            </div>
          )}

          {/* Draft Dashboard Hint */}
          {isDraftDashboard && (
            <div
              className="rounded-md border border-xblue/40 bg-xblue-soft/40 p-3 flex flex-wrap items-center justify-between gap-3"
              data-testid="share-draft-warning"
            >
              <div className="flex items-start gap-2">
                <AlertTriangle className="h-4 w-4 text-xblue shrink-0 mt-0.5" />
                <div>
                  <div className="font-semibold text-foreground">
                    驾驶舱当前处于草稿 (DRAFT) 状态
                  </div>
                  <div className="text-[11px] text-muted-foreground mt-0.5">
                    安全分享要求驾驶舱状态为已发布 (PUBLISHED)，以确保外部受访视图版本稳定。
                  </div>
                </div>
              </div>
              {onPublishDashboard && (
                <Button
                  type="button"
                  disabled={publishing}
                  onClick={() => void handlePublishFirst()}
                  className="h-8 px-3 text-xs"
                  data-testid="share-publish-dashboard-btn"
                >
                  <CheckCircle2 className="h-3.5 w-3.5" />
                  {publishing ? "发布中..." : "立即发布驾驶舱"}
                </Button>
              )}
            </div>
          )}

          {/* One-Time Share Token Result Banner */}
          {createdShare && (
            <div
              className="rounded-md border border-xblue bg-xblue-soft/50 p-3.5 space-y-2"
              data-testid="created-share-banner"
            >
              <div className="flex items-center justify-between gap-2">
                <span className="font-semibold text-foreground flex items-center gap-1.5">
                  <CheckCircle2 className="h-4 w-4 text-xblue" />
                  一次性安全分享链接已生成
                </span>
                <span className="font-mono-tech text-[11px] tabular-nums text-muted-foreground">
                  到期: {formatShareTime(createdShare.expires_at)}
                </span>
              </div>
              <div className="flex items-center gap-2">
                <Input
                  readOnly
                  value={fullShareUrl}
                  className="h-8 text-xs font-mono-tech flex-1"
                  data-testid="created-share-url"
                />
                <Button
                  type="button"
                  onClick={() => void handleCopyUrl()}
                  className="h-8 px-3 text-xs shrink-0"
                  data-testid="copy-share-url-btn"
                >
                  {copied ? (
                    <>
                      <Check className="h-3.5 w-3.5" />
                      已复制
                    </>
                  ) : (
                    <>
                      <Copy className="h-3.5 w-3.5" />
                      复制链接
                    </>
                  )}
                </Button>
              </div>
              <div className="text-[11px] text-muted-foreground">
                注意：完整令牌（<span className="font-mono-tech">axs_...</span>）仅在创建时返回一次，服务端仅存储 SHA-256 摘要，请立即复制保存。
              </div>
            </div>
          )}

          {/* Create Share Form */}
          <form
            onSubmit={(e) => void handleCreateShare(e)}
            className="space-y-3.5 rounded-md border border-border p-3.5 bg-card"
          >
            <div className="font-semibold text-foreground">创建新分享链接</div>

            {/* Validity Hours */}
            <div className="space-y-1.5">
              <label className="block text-muted-foreground">
                有效期（小时，1 ~ 720h）
              </label>
              <div className="flex flex-wrap items-center gap-2">
                {EXPIRY_PRESETS.map((preset) => (
                  <button
                    key={preset.hours}
                    type="button"
                    onClick={() => setExpiresInHours(preset.hours)}
                    data-testid={`share-preset-${preset.hours}h`}
                    className={`rounded border px-2.5 py-1 text-xs font-mono-tech cursor-pointer ${
                      expiresInHours === preset.hours
                        ? "border-xblue bg-xblue-soft text-xblue font-semibold"
                        : "border-border bg-background text-muted-foreground hover:text-foreground"
                    }`}
                  >
                    {preset.label}
                  </button>
                ))}
                <Input
                  type="number"
                  min={1}
                  max={720}
                  value={expiresInHours}
                  onChange={(e) =>
                    setExpiresInHours(Number(e.target.value) || 24)
                  }
                  className="h-8 w-24 text-xs font-mono-tech tabular-nums"
                  data-testid="share-expires-hours-input"
                />
              </div>
            </div>

            {/* Max Views & Password */}
            <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
              <div className="space-y-1">
                <label className="block text-muted-foreground">
                  最大访问次数上限（可选）
                </label>
                <Input
                  type="number"
                  min={1}
                  value={maxViewsStr}
                  onChange={(e) => setMaxViewsStr(e.target.value)}
                  placeholder="留空表示有效期内不限次数"
                  className="h-8 text-xs font-mono-tech tabular-nums"
                  data-testid="share-max-views-input"
                />
              </div>
              <div className="space-y-1">
                <label className="block text-muted-foreground">
                  访问口令保护（可选，Argon2id 存储）
                </label>
                <Input
                  type="password"
                  value={password}
                  onChange={(e) => setPassword(e.target.value)}
                  placeholder="留空免口令访问"
                  className="h-8 text-xs"
                  data-testid="share-password-input"
                />
              </div>
            </div>

            {/* Dashboard-Specific Filter Locking & Permissions */}
            {target.targetKind === "DASHBOARD" && (
              <div className="space-y-3 pt-2 border-t border-border/70">
                <div className="space-y-1.5">
                  <div className="flex items-center gap-1.5 font-medium text-foreground">
                    <Lock className="h-3.5 w-3.5 text-xblue" />
                    锁定当前筛选条件（访客不可篡改或越权绕过）
                  </div>
                  {activeFilterCandidates.length === 0 ? (
                    <div className="text-[11px] text-muted-foreground">
                      当前驾驶舱未设置非空全局筛选值；如需固定租户/战区筛选，请先在看板筛选栏填入后再打开分享。
                    </div>
                  ) : (
                    <div className="flex flex-wrap gap-2">
                      {activeFilterCandidates.map((item) => {
                        const checked = lockedKeySet.has(item.key);
                        return (
                          <label
                            key={item.key}
                            className={`inline-flex items-center gap-1.5 rounded border px-2.5 py-1 cursor-pointer ${
                              checked
                                ? "border-xblue bg-xblue-soft/50 text-foreground"
                                : "border-border bg-background text-muted-foreground"
                            }`}
                          >
                            <input
                              type="checkbox"
                              checked={checked}
                              onChange={(e) => {
                                setLockedKeySet((prev) => {
                                  const next = new Set(prev);
                                  if (e.target.checked) {
                                    next.add(item.key);
                                  } else {
                                    next.delete(item.key);
                                  }
                                  return next;
                                });
                              }}
                              className="h-3.5 w-3.5 accent-[#2563eb]"
                              data-testid={`share-lock-filter-${item.key}`}
                            />
                            <span className="text-xs font-medium">
                              {item.label}:
                            </span>
                            <span className="font-mono-tech text-[11px]">
                              {summarizeFilterValue(item.value)}
                            </span>
                          </label>
                        );
                      })}
                    </div>
                  )}
                </div>

                <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
                  <label className="flex items-center justify-between gap-2 rounded border border-border bg-background px-3 py-2 cursor-pointer">
                    <div>
                      <div className="font-medium text-foreground">
                        允许调整未锁定筛选
                      </div>
                      <div className="text-[11px] text-muted-foreground">
                        允许访客自由切换未锁定的筛选器
                      </div>
                    </div>
                    <input
                      type="checkbox"
                      checked={allowFilters}
                      onChange={(e) => setAllowFilters(e.target.checked)}
                      className="h-4 w-4 accent-[#2563eb] cursor-pointer"
                      data-testid="share-allow-filters-toggle"
                    />
                  </label>

                  <label className="flex items-center justify-between gap-2 rounded border border-border bg-background px-3 py-2 cursor-pointer">
                    <div>
                      <div className="font-medium text-foreground">
                        允许访客下载报告
                      </div>
                      <div className="text-[11px] text-muted-foreground">
                        允许在分享页触发多格式文件下载
                      </div>
                    </div>
                    <input
                      type="checkbox"
                      checked={allowExport}
                      onChange={(e) => setAllowExport(e.target.checked)}
                      className="h-4 w-4 accent-[#2563eb] cursor-pointer"
                      data-testid="share-allow-export-toggle"
                    />
                  </label>
                </div>
              </div>
            )}

            <div className="flex justify-end pt-1">
              <Button
                type="submit"
                disabled={creating || isDraftDashboard}
                data-testid="submit-create-share-btn"
              >
                <Share2 className="h-3.5 w-3.5" />
                {creating ? "生成分享链接中..." : "生成安全分享链接"}
              </Button>
            </div>
          </form>

          {/* Existing Share Links Management */}
          <div className="space-y-2">
            <div className="flex items-center justify-between">
              <span className="font-semibold text-foreground">
                已创建分享链接 ({shares.length})
              </span>
              {loadingShares && (
                <span className="text-[11px] text-muted-foreground">
                  加载中...
                </span>
              )}
            </div>

            {shares.length === 0 ? (
              <div className="rounded-md border border-dashed border-border py-6 text-center text-muted-foreground">
                当前对象暂无历史分享记录
              </div>
            ) : (
              <div className="space-y-2">
                {shares.map((link) => {
                  const st = resolveShareStatus(link);
                  const lockedCount = Object.keys(
                    link.locked_filters ?? {}
                  ).length;
                  return (
                    <div
                      key={link.id}
                      data-testid={`share-link-row-${link.id}`}
                      className="rounded-md border border-border bg-card px-3 py-2.5 flex flex-wrap items-center justify-between gap-2"
                    >
                      <div className="space-y-1 min-w-0">
                        <div className="flex flex-wrap items-center gap-1.5">
                          <span className="font-mono-tech font-semibold text-xs">
                            axs_{link.token_prefix}...
                          </span>
                          <Badge variant={st.active ? "xblue" : "outline"}>
                            {st.label}
                          </Badge>
                          {link.has_password && (
                            <Badge variant="neutral" className="gap-1">
                              <KeyRound className="h-2.5 w-2.5" />
                              口令保护
                            </Badge>
                          )}
                          {lockedCount > 0 && (
                            <Badge variant="outline">
                              锁定筛选 {lockedCount}
                            </Badge>
                          )}
                        </div>
                        <div className="flex flex-wrap items-center gap-3 text-[11px] font-mono-tech tabular-nums text-muted-foreground">
                          <span>
                            访问次数: {link.view_count} /{" "}
                            {link.max_views ?? "∞"}
                          </span>
                          <span>到期: {formatShareTime(link.expires_at)}</span>
                        </div>
                      </div>

                      {!link.revoked_at && (
                        <Button
                          type="button"
                          variant="outline"
                          disabled={revokingId === link.id}
                          onClick={() => void handleRevoke(link.id)}
                          className="h-7 px-2.5 text-xs"
                          data-testid={`revoke-share-${link.id}`}
                        >
                          <Trash2 className="h-3 w-3" />
                          {revokingId === link.id ? "撤销中..." : "撤销"}
                        </Button>
                      )}
                    </div>
                  );
                })}
              </div>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
