import { useEffect, useState } from "react";
import { GitCompare, History, RotateCcw, User } from "lucide-react";
import { useI18n } from "@/i18n";
import { usePrincipal } from "@/lib/permissions";
import {
  xblueApi,
  type SchemaDiff,
  type SchemaVersionSummary,
} from "@/lib/api/xblue";
import {
  Badge,
  Button,
  Drawer,
  EmptyState,
  Select,
} from "@/components/ui/primitives";
import { cn } from "@/lib/utils";
import { DiffView } from "./DiffView";

export interface VersionPanelProps {
  open: boolean;
  subgraphSlug: string;
  versions: SchemaVersionSummary[];
  onClose: () => void;
  onRestoreVersion: (version: number) => Promise<void>;
}

function normalizeCommitSummary(
  raw: string | null | undefined,
  version: number,
  tx: (zh: string, en: string) => string
): string {
  const trimmed = (raw ?? "").trim();
  if (!trimmed || trimmed.length <= 1) {
    return tx(`模型版本快照 v${version}`, `Schema snapshot v${version}`);
  }
  if (trimmed.toLowerCase() === "direct schema save") {
    return tx("模型结构直接发布", "Direct schema release");
  }
  return trimmed;
}

export function VersionPanel({
  open,
  subgraphSlug,
  versions,
  onClose,
  onRestoreVersion,
}: VersionPanelProps) {
  const { tx, fmtTime } = useI18n();
  const { can } = usePrincipal();
  const canEdit = can("STUDIO_EDIT");

  const sorted = [...versions].sort((a, b) => b.version - a.version);
  const latestVersion = sorted[0]?.version ?? 1;
  const [verA, setVerA] = useState<number>(
    sorted[1]?.version ?? sorted[0]?.version ?? 1
  );
  const [verB, setVerB] = useState<number>(sorted[0]?.version ?? 1);
  const [diffResult, setDiffResult] = useState<SchemaDiff | null>(null);
  const [diffLoading, setDiffLoading] = useState(false);
  const [restoringVer, setRestoringVer] = useState<number | null>(null);
  const [msg, setMsg] = useState<string | null>(null);

  useEffect(() => {
    if (!open) return;
    const latest = sorted[0]?.version ?? 1;
    const prev = sorted[1]?.version ?? latest;
    setVerA(prev);
    setVerB(latest);
    setMsg(null);

    let cancelled = false;
    if (sorted.length >= 2) {
      setDiffLoading(true);
      void xblueApi
        .diffSchemaVersions(prev, latest, subgraphSlug)
        .then((d) => {
          if (!cancelled) setDiffResult(d);
        })
        .catch(() => {
          if (!cancelled) setDiffResult(null);
        })
        .finally(() => {
          if (!cancelled) setDiffLoading(false);
        });
    } else if (sorted.length === 1) {
      void xblueApi
        .getSchemaVersion(latest, subgraphSlug)
        .then((detail) => {
          if (!cancelled) setDiffResult(detail.diff);
        })
        .catch(() => {
          if (!cancelled) setDiffResult(null);
        });
    } else {
      setDiffResult(null);
    }

    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, subgraphSlug, versions.length]);

  const runDiff = async (overrideA?: number, overrideB?: number) => {
    const fromV = overrideA ?? verA;
    const toV = overrideB ?? verB;
    setVerA(fromV);
    setVerB(toV);
    setDiffLoading(true);
    setMsg(null);
    try {
      const d = await xblueApi.diffSchemaVersions(fromV, toV, subgraphSlug);
      setDiffResult(d);
    } catch (err) {
      setMsg(
        err instanceof Error
          ? err.message
          : tx("加载版本差异失败", "Failed to load version diff")
      );
    } finally {
      setDiffLoading(false);
    }
  };

  const handleRestore = async (v: number) => {
    if (!canEdit || v === latestVersion) return;
    setRestoringVer(v);
    setMsg(null);
    try {
      await onRestoreVersion(v);
      onClose();
    } catch (err) {
      setMsg(
        err instanceof Error
          ? err.message
          : tx("恢复草案失败", "Failed to restore draft")
      );
    } finally {
      setRestoringVer(null);
    }
  };

  return (
    <Drawer
      open={open}
      onClose={onClose}
      testId="version-panel"
      closeTestId="close-version-panel-btn"
      widthClass="max-w-xl"
      title={
        <>
          <History className="h-4 w-4 text-xblue shrink-0" />
          <span>
            {tx("版本历史与差异回滚", "Version History & Diff")} ·{" "}
            {subgraphSlug}
          </span>
        </>
      }
    >
      {msg && (
        <div className="rounded border border-border bg-muted p-2 text-xs">
          {msg}
        </div>
      )}

      {/* Arbitrary Two-Version Comparison Bar & DiffView (Prominent at Top) */}
      {sorted.length >= 1 && (
        <div className="space-y-2.5 rounded-md border border-border bg-muted/15 p-3">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <span className="font-semibold text-xs flex items-center gap-1.5">
              <GitCompare className="h-3.5 w-3.5 text-xblue" />
              {tx("版本差异对比", "Version Diff")}
            </span>
            <div className="flex items-center gap-1.5">
              <Select
                data-testid="diff-from-version"
                value={verA}
                onChange={(e) => setVerA(Number(e.target.value))}
                className="h-7 w-24 px-2 font-mono-tech"
              >
                {sorted.map((v) => (
                  <option key={`a-${v.version}`} value={v.version}>
                    {tx(`基准 v${v.version}`, `From v${v.version}`)}
                  </option>
                ))}
              </Select>
              <span className="text-muted-foreground">→</span>
              <Select
                data-testid="diff-to-version"
                value={verB}
                onChange={(e) => setVerB(Number(e.target.value))}
                className="h-7 w-24 px-2 font-mono-tech"
              >
                {sorted.map((v) => (
                  <option key={`b-${v.version}`} value={v.version}>
                    {tx(`目标 v${v.version}`, `To v${v.version}`)}
                  </option>
                ))}
              </Select>
              <Button
                variant="outline"
                size="sm"
                data-testid="run-version-diff-btn"
                disabled={diffLoading}
                onClick={() => void runDiff()}
              >
                {diffLoading
                  ? tx("对比中…", "Comparing…")
                  : tx("对比", "Compare")}
              </Button>
            </div>
          </div>

          {diffResult && (
            <DiffView
              diff={diffResult}
              fromLabel={`v${verA}`}
              toLabel={`v${verB}`}
            />
          )}
        </div>
      )}

      {/* Borderless Timeline Version History */}
      <div className="space-y-2.5 pt-1">
        <div className="font-semibold text-xs">
          {tx("已发布时间轴", "Release Timeline")} ({sorted.length})
        </div>
        {sorted.length === 0 ? (
          <EmptyState title={tx("暂无发布版本", "No published versions")} />
        ) : (
          <div
            className="relative ml-2 border-l border-border pl-4 space-y-3"
            data-testid="version-list"
          >
            {sorted.map((item, idx) => {
              const isCurrentBaseline = idx === 0;
              const operator =
                (item as SchemaVersionSummary & { published_by?: string })
                  .published_by || "ops_admin";
              return (
                <div
                  key={`${item.version}-${idx}`}
                  data-testid={`version-item-v${item.version}`}
                  className="relative flex items-start justify-between gap-3 py-1"
                >
                  <span
                    className={cn(
                      "absolute -left-[21px] top-2.5 h-2.5 w-2.5 rounded-full ring-4 ring-card",
                      isCurrentBaseline ? "bg-xblue" : "bg-muted-foreground/45"
                    )}
                  />
                  <div className="min-w-0 space-y-1">
                    <div className="flex flex-wrap items-center gap-1.5">
                      <span
                        className={cn(
                          "font-mono-tech text-xs font-semibold",
                          isCurrentBaseline ? "text-xblue" : "text-foreground"
                        )}
                      >
                        v{item.version}
                      </span>
                      {isCurrentBaseline && (
                        <Badge variant="xblue">
                          {tx("当前基线", "Current Baseline")}
                        </Badge>
                      )}
                      <span className="font-medium text-xs truncate">
                        {normalizeCommitSummary(
                          item.change_summary,
                          item.version,
                          tx
                        )}
                      </span>
                    </div>
                    <div className="flex flex-wrap items-center gap-2 text-[11px] text-muted-foreground">
                      <span className="inline-flex items-center gap-1 font-mono-tech">
                        <User className="h-3 w-3" />
                        {operator}
                      </span>
                      <span>·</span>
                      <span className="font-mono-tech">
                        {fmtTime(item.published_at)}
                      </span>
                      {!isCurrentBaseline && (
                        <>
                          <span>·</span>
                          <button
                            type="button"
                            onClick={() =>
                              void runDiff(item.version, latestVersion)
                            }
                            className="text-xblue hover:underline cursor-pointer"
                          >
                            {tx("对比基线", "Diff vs Latest")}
                          </button>
                        </>
                      )}
                    </div>
                  </div>

                  {canEdit && (
                    <Button
                      variant="outline"
                      size="sm"
                      data-testid={`restore-version-v${item.version}`}
                      disabled={isCurrentBaseline || restoringVer !== null}
                      title={
                        isCurrentBaseline
                          ? tx("当前已是线上基线版本", "Already active baseline")
                          : undefined
                      }
                      onClick={() => void handleRestore(item.version)}
                    >
                      <RotateCcw className="h-3 w-3" />
                      {restoringVer === item.version
                        ? tx("恢复中…", "Restoring…")
                        : isCurrentBaseline
                          ? tx("线上基线", "Active")
                          : tx("恢复草案", "Restore")}
                    </Button>
                  )}
                </div>
              );
            })}
          </div>
        )}
      </div>
    </Drawer>
  );
}
