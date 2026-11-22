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
} from "lucide-react";
import { useI18n } from "@/i18n";
import {
  xblueviewApi,
  type CreateShareResponse,
  type DashboardStatus,
  type FilterDef,
  type ShareLinkRecord,
} from "@/lib/api/xblueview";
import { usePrincipal } from "@/lib/permissions";
import {
  Badge,
  Button,
  Input,
  Modal,
  PasswordInput,
  StatusBadge,
} from "@/components/ui/primitives";
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

const EXPIRY_PRESETS: readonly {
  hours: number;
  zh: string;
  en: string;
}[] = [
  { hours: 24, zh: "24h", en: "24h" },
  { hours: 72, zh: "3天", en: "3d" },
  { hours: 168, zh: "7天", en: "7d" },
  { hours: 720, zh: "30天", en: "30d" },
];

function summarizeFilterValue(val: unknown): string {
  if (Array.isArray(val)) {
    return val.map((v) => String(v ?? "")).join(" ~ ");
  }
  if (typeof val === "object" && val !== null) {
    return JSON.stringify(val);
  }
  return String(val ?? "");
}

interface ResolvedShareState {
  active: boolean;
  code: "ACTIVE" | "REVOKED" | "EXPIRED" | "EXHAUSTED";
}

function resolveShareState(link: ShareLinkRecord): ResolvedShareState {
  if (link.revoked_at) {
    return { active: false, code: "REVOKED" };
  }
  if (new Date(link.expires_at).getTime() <= Date.now()) {
    return { active: false, code: "EXPIRED" };
  }
  if (
    link.max_views !== null &&
    link.max_views !== undefined &&
    link.view_count >= link.max_views
  ) {
    return { active: false, code: "EXHAUSTED" };
  }
  return { active: true, code: "ACTIVE" };
}

export function ShareDialog({
  open,
  target,
  onClose,
  onPublishDashboard,
}: ShareDialogProps) {
  const { tx, tEnum, fmtTime, fmtNum, isZh } = useI18n();
  const { can } = usePrincipal();
  const canShare = can("VIEW_SHARE");
  const canEdit = can("VIEW_EDIT");

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
  const [copiedRowId, setCopiedRowId] = useState<string | null>(null);

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
    setCopiedRowId(null);
    setErrorMsg(null);
    setLockedKeySet(
      new Set(activeFilterCandidates.map((item) => item.key))
    );
    void loadShares(target.targetId);
  }, [activeFilterCandidates, loadShares, open, target]);

  const { activeShares, inactiveShares } = useMemo(() => {
    const active: { link: ShareLinkRecord; state: ResolvedShareState }[] = [];
    const inactive: { link: ShareLinkRecord; state: ResolvedShareState }[] = [];
    for (const link of shares) {
      const st = resolveShareState(link);
      if (st.active) {
        active.push({ link, state: st });
      } else {
        inactive.push({ link, state: st });
      }
    }
    return { activeShares: active, inactiveShares: inactive };
  }, [shares]);

  const isDraftDashboard =
    Boolean(target) &&
    target?.targetKind === "DASHBOARD" &&
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

  const handleCopyRowValue = async (rowId: string, text: string) => {
    try {
      await navigator.clipboard.writeText(text);
      setCopiedRowId(rowId);
      setTimeout(() => {
        setCopiedRowId((curr) => (curr === rowId ? null : curr));
      }, 2000);
    } catch {
      setCopiedRowId(null);
    }
  };

  const handlePublishFirst = async () => {
    if (!onPublishDashboard || !canEdit) return;
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
    if (!target || !canShare) return;
    if (isDraftDashboard) {
      setErrorMsg(
        tx(
          "驾驶舱需发布为正式版本后方可分享",
          "Publish the dashboard before creating a share link"
        )
      );
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
        setErrorMsg(
          tx(
            "最大访问次数须为正整数",
            "Max views must be a positive integer"
          )
        );
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
    if (!target || !canShare) return;
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

  const renderStatusBadge = (state: ResolvedShareState) => {
    if (state.code === "ACTIVE") {
      return <StatusBadge status="ACTIVE" />;
    }
    if (state.code === "REVOKED") {
      return (
        <Badge variant="danger">{tx("已撤销", "Revoked")}</Badge>
      );
    }
    if (state.code === "EXPIRED") {
      return <StatusBadge status="EXPIRED" />;
    }
    return (
      <Badge variant="warning">{tx("次数耗尽", "Limit Reached")}</Badge>
    );
  };

  return (
    <Modal
      open={open && Boolean(target)}
      onClose={onClose}
      testId="share-dialog"
      maxWidthClass="max-w-2xl"
      title={
        target ? (
          <>
            <Share2 className="h-4 w-4 text-xblue shrink-0" />
            <span className="truncate">
              {tx("安全分享", "Share")} · {target.targetName}
            </span>
            <Badge variant="xblue">{tEnum(target.targetKind)}</Badge>
          </>
        ) : (
          tx("安全分享", "Share")
        )
      }
    >
      {target && (
        <div className="space-y-4 text-xs">
          {errorMsg && (
            <div
              className="rounded border border-red-500/40 bg-red-500/10 px-3 py-2 text-red-700 dark:text-red-300"
              data-testid="share-dialog-error"
            >
              {errorMsg}
            </div>
          )}

          {isDraftDashboard && (
            <div
              className="rounded-md border border-amber-500/40 bg-amber-500/10 p-3 flex flex-wrap items-center justify-between gap-3"
              data-testid="share-draft-warning"
            >
              <div className="flex items-start gap-2">
                <AlertTriangle className="h-4 w-4 text-amber-600 dark:text-amber-400 shrink-0 mt-0.5" />
                <div>
                  <div className="font-semibold text-foreground">
                    {tx(
                      "驾驶舱处于草稿状态",
                      "Dashboard is currently in Draft"
                    )}
                  </div>
                  <div className="text-[11px] text-muted-foreground mt-0.5">
                    {tx(
                      "外部共享要求驾驶舱处于已发布版本",
                      "External sharing requires a published dashboard version"
                    )}
                  </div>
                </div>
              </div>
              {onPublishDashboard && canEdit && (
                <Button
                  type="button"
                  size="sm"
                  disabled={publishing}
                  onClick={() => void handlePublishFirst()}
                  data-testid="share-publish-dashboard-btn"
                >
                  <CheckCircle2 className="h-3.5 w-3.5" />
                  {publishing
                    ? tx("发布中…", "Publishing…")
                    : tx("发布驾驶舱", "Publish")}
                </Button>
              )}
            </div>
          )}

          {createdShare && (
            <div
              className="rounded-md border border-xblue bg-xblue-soft/50 p-3.5 space-y-2"
              data-testid="created-share-banner"
            >
              <div className="flex items-center justify-between gap-2">
                <span className="font-semibold text-foreground flex items-center gap-1.5">
                  <CheckCircle2 className="h-4 w-4 text-xblue" />
                  {tx("一次性分享链接已生成", "One-time share link created")}
                </span>
                <span className="font-mono-tech text-[11px] tabular-nums text-muted-foreground">
                  {tx("到期", "Exp")}: {fmtTime(createdShare.expires_at, false)}
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
                  size="sm"
                  onClick={() => void handleCopyUrl()}
                  className="h-8 px-3 shrink-0"
                  data-testid="copy-share-url-btn"
                >
                  {copied ? (
                    <>
                      <Check className="h-3.5 w-3.5" />
                      {tx("已复制", "Copied")}
                    </>
                  ) : (
                    <>
                      <Copy className="h-3.5 w-3.5" />
                      {tx("复制链接", "Copy URL")}
                    </>
                  )}
                </Button>
              </div>
              <div className="text-[11px] text-muted-foreground">
                {tx(
                  "完整令牌仅在创建时返回一次，服务端仅保存 SHA-256 摘要。",
                  "Full token is returned once on creation; server stores only its SHA-256 digest."
                )}
              </div>
            </div>
          )}

          {canShare && (
            <form
              onSubmit={(e) => void handleCreateShare(e)}
              className="space-y-3.5 rounded-md border border-border p-3.5 bg-card"
            >
              <div className="font-semibold text-foreground">
                {tx("新建分享链接", "New Share Link")}
              </div>

              <div className="space-y-1.5">
                <label className="block text-muted-foreground">
                  {tx("有效期 (1–720 小时)", "Validity (1–720 hours)")}
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
                      {isZh ? preset.zh : preset.en}
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

              <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
                <div className="space-y-1">
                  <label className="block text-muted-foreground">
                    {tx("访问次数上限", "Max Views")}
                  </label>
                  <Input
                    type="number"
                    min={1}
                    value={maxViewsStr}
                    onChange={(e) => setMaxViewsStr(e.target.value)}
                    placeholder="100"
                    className="h-8 text-xs font-mono-tech tabular-nums"
                    data-testid="share-max-views-input"
                  />
                </div>
                <div className="space-y-1">
                  <label className="block text-muted-foreground">
                    {tx("访问口令", "Access Password")}
                  </label>
                  <PasswordInput
                    value={password}
                    onChange={(e) => setPassword(e.target.value)}
                    placeholder="••••••••"
                    className="h-8 text-xs"
                    data-testid="share-password-input"
                  />
                </div>
              </div>

              {target.targetKind === "DASHBOARD" && (
                <div className="space-y-3 pt-2 border-t border-border/70">
                  <div className="space-y-1.5">
                    <div className="flex items-center gap-1.5 font-medium text-foreground">
                      <Lock className="h-3.5 w-3.5 text-xblue" />
                      {tx("锁定当前筛选条件", "Lock Active Filters")}
                    </div>
                    {activeFilterCandidates.length === 0 ? (
                      <div className="text-[11px] text-muted-foreground">
                        {tx(
                          "当前看板无非空全局筛选值",
                          "No active filter values set on current dashboard"
                        )}
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
                          {tx("允许未锁定筛选", "Allow Unlocked Filters")}
                        </div>
                        <div className="text-[11px] text-muted-foreground">
                          {tx(
                            "访客可调整未锁定的筛选条件",
                            "Visitors can adjust unlocked filters"
                          )}
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
                          {tx("允许下载报告", "Allow Export Download")}
                        </div>
                        <div className="text-[11px] text-muted-foreground">
                          {tx(
                            "访客可在分享页下载多格式文件",
                            "Visitors can download reports from share view"
                          )}
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
                  {creating
                    ? tx("生成中…", "Creating…")
                    : tx("生成链接", "Create Link")}
                </Button>
              </div>
            </form>
          )}

          {/* Active vs Expired/Revoked Share Links */}
          <div className="space-y-3">
            <div className="space-y-2">
              <div className="flex items-center justify-between">
                <span className="font-semibold text-foreground">
                  {tx("生效中链接", "Active Links")} ({activeShares.length})
                </span>
                {loadingShares && (
                  <span className="text-[11px] text-muted-foreground">
                    {tx("加载中…", "Loading…")}
                  </span>
                )}
              </div>

              {activeShares.length === 0 ? (
                <div className="rounded-md border border-dashed border-border py-5 text-center text-muted-foreground">
                  {tx("暂无生效中的分享链接", "No active share links")}
                </div>
              ) : (
                <div className="space-y-2">
                  {activeShares.map(({ link, state }) => {
                    const lockedCount = Object.keys(
                      link.locked_filters ?? {}
                    ).length;
                    const isJustCreated = createdShare?.id === link.id;
                    const copyPayload = isJustCreated
                      ? fullShareUrl
                      : `axs_${link.token_prefix}`;
                    return (
                      <div
                        key={link.id}
                        data-testid={`share-link-row-${link.id}`}
                        className="rounded-md border border-border bg-card px-3 py-2.5 flex flex-wrap items-center justify-between gap-2"
                      >
                        <div className="space-y-1 min-w-0">
                          <div className="flex flex-wrap items-center gap-1.5">
                            <span className="font-mono-tech font-semibold text-xs">
                              axs_{link.token_prefix}…
                            </span>
                            {renderStatusBadge(state)}
                            {link.has_password && (
                              <Badge variant="neutral" className="gap-1">
                                <KeyRound className="h-2.5 w-2.5" />
                                {tx("口令保护", "Password")}
                              </Badge>
                            )}
                            {lockedCount > 0 && (
                              <Badge variant="outline">
                                {tx(
                                  `锁定筛选 ${fmtNum(lockedCount, 0)}`,
                                  `Locked ${fmtNum(lockedCount, 0)}`
                                )}
                              </Badge>
                            )}
                          </div>
                          <div className="flex flex-wrap items-center gap-3 text-[11px] font-mono-tech tabular-nums text-muted-foreground">
                            <span>
                              {tx("访问", "Views")}: {link.view_count} /{" "}
                              {link.max_views ?? "∞"}
                            </span>
                            <span>
                              {tx("到期", "Exp")}:{" "}
                              {fmtTime(link.expires_at, false)}
                            </span>
                          </div>
                        </div>

                        <div className="flex items-center gap-1.5">
                          <Button
                            type="button"
                            variant="outline"
                            size="sm"
                            onClick={() =>
                              void handleCopyRowValue(link.id, copyPayload)
                            }
                            title={
                              isJustCreated
                                ? tx("复制完整链接", "Copy share URL")
                                : tx("复制令牌前缀", "Copy token prefix")
                            }
                          >
                            {copiedRowId === link.id ? (
                              <>
                                <Check className="h-3 w-3 text-xblue" />
                                {tx("已复制", "Copied")}
                              </>
                            ) : (
                              <>
                                <Copy className="h-3 w-3" />
                                {isJustCreated
                                  ? tx("复制链接", "Copy URL")
                                  : tx("复制标识", "Copy ID")}
                              </>
                            )}
                          </Button>
                          {canShare && (
                            <Button
                              type="button"
                              variant="danger"
                              size="sm"
                              disabled={revokingId === link.id}
                              onClick={() => void handleRevoke(link.id)}
                              data-testid={`revoke-share-${link.id}`}
                            >
                              <Trash2 className="h-3 w-3" />
                              {revokingId === link.id
                                ? tx("撤销中…", "Revoking…")
                                : tx("撤销", "Revoke")}
                            </Button>
                          )}
                        </div>
                      </div>
                    );
                  })}
                </div>
              )}
            </div>

            {inactiveShares.length > 0 && (
              <div className="space-y-2 pt-2 border-t border-border">
                <div className="font-semibold text-muted-foreground">
                  {tx("已失效 / 已撤销", "Expired & Revoked")} (
                  {inactiveShares.length})
                </div>
                <div className="space-y-1.5">
                  {inactiveShares.map(({ link, state }) => (
                    <div
                      key={link.id}
                      data-testid={`share-link-row-${link.id}`}
                      className="rounded-md border border-border/70 bg-muted/20 px-3 py-2 flex flex-wrap items-center justify-between gap-2 opacity-80"
                    >
                      <div className="space-y-0.5 min-w-0">
                        <div className="flex flex-wrap items-center gap-1.5">
                          <span className="font-mono-tech text-xs text-muted-foreground">
                            axs_{link.token_prefix}…
                          </span>
                          {renderStatusBadge(state)}
                          {link.has_password && (
                            <Badge variant="outline" className="gap-1">
                              <KeyRound className="h-2.5 w-2.5" />
                              {tx("口令保护", "Password")}
                            </Badge>
                          )}
                        </div>
                        <div className="flex flex-wrap items-center gap-3 text-[11px] font-mono-tech tabular-nums text-muted-foreground">
                          <span>
                            {tx("访问", "Views")}: {link.view_count} /{" "}
                            {link.max_views ?? "∞"}
                          </span>
                          <span>
                            {link.revoked_at
                              ? `${tx("撤销", "Revoked")}: ${fmtTime(
                                  link.revoked_at,
                                  false
                                )}`
                              : `${tx("到期", "Exp")}: ${fmtTime(
                                  link.expires_at,
                                  false
                                )}`}
                          </span>
                        </div>
                      </div>
                    </div>
                  ))}
                </div>
              </div>
            )}
          </div>
        </div>
      )}
    </Modal>
  );
}
